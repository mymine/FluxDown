//! Windows completion toasts use a dedicated protocol and expiring opaque local tokens.
//! No callbacks, polling, runtime, COM activator, or file paths in activation URIs.
//! Requires windows 0.61.3: Data_Xml_Dom, Foundation, UI_Notifications.

#[path = "windows_activation.rs"]
mod activation;

#[cfg(windows)]
pub(super) fn handle_activation(args: &[String]) -> Option<Result<(), String>> {
    activation::handle_activation(args)
}

#[cfg(windows)]
pub(super) fn show(
    data_dir: &std::path::Path,
    title: &str,
    body: &str,
    actions: Option<&super::CompletionActions>,
    silent: bool,
) -> Result<(), String> {
    // Use a short-lived dedicated thread that has never initialized COM: windows-core then
    // joins the process MTA through `CoIncrementMTAUsage`, which it never releases. That keeps
    // its process-wide cached activation factories valid for every later toast. Never pair
    // RoInitialize/RoUninitialize per toast: tearing down the last apartment in the process
    // (headless `--server` has no other) unloads the WinRT servers behind those cached
    // factories, and the next toast dereferences a dangling factory (0xC0000005).
    // It is joined before returning: no per-toast resident thread or callback lifetime.
    let data_dir = data_dir.to_owned();
    let title = title.to_owned();
    let body = body.to_owned();
    let actions = actions.cloned();
    std::thread::Builder::new()
        .name("fluxdown-toast".into())
        .spawn(move || runtime::show(&data_dir, &title, &body, actions.as_ref(), silent))
        .map_err(|error| format!("could not start toast sender: {error}"))?
        .join()
        .map_err(|_| "toast sender panicked".to_owned())?
}

fn toast_xml(title: &str, body: &str, actions: Option<(&str, &str, &str)>, silent: bool) -> String {
    let launch = actions.map_or_else(String::new, |(token, _, _)| {
        format!(
            " activationType=\"protocol\" launch=\"{}\"",
            activation::uri(token, "open-folder"),
        )
    });
    let mut xml = format!(
        "<toast{launch}><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual>",
        escape_xml(title),
        escape_xml(body),
    );
    if let Some((token, file, folder)) = actions {
        xml.push_str(&format!(
            "<actions><action content=\"{}\" arguments=\"{}\" activationType=\"protocol\"/><action content=\"{}\" arguments=\"{}\" activationType=\"protocol\"/></actions>",
            escape_xml(file), activation::uri(token, "open-file"),
            escape_xml(folder), activation::uri(token, "open-folder"),
        ));
    }
    if silent {
        // 提示音由 agent 自己播放（`desktop.sound.task_completed`），避免系统通知音叠加。
        xml.push_str("<audio silent=\"true\"/>");
    }
    xml.push_str("</toast>");
    xml
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t' => escaped.push_str("&#9;"),
            '\n' => escaped.push_str("&#10;"),
            '\r' => escaped.push_str("&#13;"),
            '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}' => {
                escaped.push(ch)
            }
            _ => escaped.push('\u{fffd}'),
        }
    }
    escaped
}

#[cfg(windows)]
mod runtime {
    use std::path::Path;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::{DateTime, IReference, PropertyValue};
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::{HSTRING, Interface};

    use super::super::{CompletionActions, WINDOWS_AUMID};
    use super::{activation, toast_xml};

    const GROUP: &str = "completion";

    pub(super) fn show(
        data_dir: &Path,
        title: &str,
        body: &str,
        actions: Option<&CompletionActions>,
        silent: bool,
    ) -> Result<(), String> {
        // Hold the emission lease through Show: concurrent emitters cannot reuse a slot
        // between token creation and delivery. File close releases the cross-process lock.
        let stored = actions
            .map(|actions| {
                activation::register_protocol(data_dir)?;
                activation::store(data_dir, &actions.path, remove_history)
            })
            .transpose()?;
        let xml = toast_xml(
            title,
            body,
            stored.as_ref().zip(actions).map(|(stored, actions)| {
                (
                    stored.token.as_str(),
                    actions.open_file_label.as_str(),
                    actions.open_folder_label.as_str(),
                )
            }),
            silent,
        );
        let result = deliver(&xml, stored.as_ref());
        if let Err(delivery_error) = &result
            && let Some(stored) = &stored
        {
            stored.remove().map_err(|cleanup_error| {
                format!("{delivery_error}; token cleanup also failed: {cleanup_error}")
            })?;
        }
        result
    }

    fn deliver(xml: &str, stored: Option<&activation::Stored>) -> Result<(), String> {
        let operation = || -> windows::core::Result<()> {
            let document = XmlDocument::new()?;
            document.LoadXml(&HSTRING::from(xml))?;
            let toast = ToastNotification::CreateToastNotification(&document)?;
            if let Some(stored) = stored {
                toast.SetTag(&HSTRING::from(stored.slot.to_string()))?;
                toast.SetGroup(&HSTRING::from(GROUP))?;
                let expiry: IReference<DateTime> = PropertyValue::CreateDateTime(DateTime {
                    UniversalTime: stored.windows_expiry,
                })?
                .cast()?;
                toast.SetExpirationTime(&expiry)?;
            } else {
                // Informational notifications neither expose protocol actions nor grow history.
                toast.SetTag(&HSTRING::from("info"))?;
                toast.SetGroup(&HSTRING::from("information"))?;
            }
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(WINDOWS_AUMID))?
                .Show(&toast)
        };
        operation().map_err(|error| format!("could not show Windows toast: {error}"))
    }

    fn remove_history(slot: u8) -> Result<(), String> {
        ToastNotificationManager::History()
            .and_then(|history| {
                history.RemoveGroupedTagWithId(
                    &HSTRING::from(slot.to_string()),
                    &HSTRING::from(GROUP),
                    &HSTRING::from(WINDOWS_AUMID),
                )
            })
            .map_err(|error| format!("could not prune notification history: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{escape_xml, toast_xml};
    const TOKEN: &str = "d735c73f-0697-4f90-b330-c3c3a035c963";

    #[test]
    fn xml_is_safe_for_text_and_labels() {
        assert_eq!(
            escape_xml("<&>\"'\t\n\r\0\u{1f}\u{fffe}中📁"),
            "&lt;&amp;&gt;&quot;&apos;&#9;&#10;&#13;���中📁"
        );
        let hostile = "\"/><action arguments=\"file:///C:/evil\"/><text>";
        let xml = toast_xml(hostile, hostile, Some((TOKEN, hostile, hostile)), false);
        assert_eq!(xml.matches("<action ").count(), 2);
        assert_eq!(xml.matches("<text>").count(), 2);
        assert!(!xml.contains("arguments=\"file:"));
    }

    #[test]
    fn body_reveals_and_buttons_use_protocol_tokens() {
        let xml = toast_xml(
            "Done",
            "name.zip",
            Some((TOKEN, "Open File", "Open Folder")),
            false,
        );
        assert!(xml.contains(&format!(
            "launch=\"fluxdown-notification://{TOKEN}/open-folder\""
        )));
        assert!(xml.contains(&format!(
            "arguments=\"fluxdown-notification://{TOKEN}/open-file\""
        )));
        assert_eq!(xml.matches("activationType=\"protocol\"").count(), 3);
        assert!(!xml.contains("C:"));
    }

    #[test]
    fn informational_toast_has_no_protocol_activation() {
        let xml = toast_xml("Test", "Hello", None, false);
        assert!(!xml.contains("<audio"));
        assert!(!xml.contains("protocol"));
        assert!(!xml.contains("<actions>"));
        assert!(!xml.contains("launch="));
    }

    #[test]
    fn silent_toast_suppresses_the_system_sound() {
        let xml = toast_xml("Done", "name.zip", None, true);
        assert!(xml.ends_with("<audio silent=\"true\"/></toast>"));
    }
}
