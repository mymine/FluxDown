use std::borrow::Cow;

use fluxdown_ui_account::AccountAssets;
use fluxdown_ui_components::ComponentAssets;
use fluxdown_ui_downloads::FlagAssets;
use fluxdown_ui_icon_pack::IconPackAssets;
use fluxdown_ui_rss::RssAssets;
use fluxdown_ui_shell::ShellAssets;
use gpui::{AssetSource, Result, SharedString};

/// composition root 组合 shell、能力 crate 与 gpui-component 的资源。
pub(crate) struct DesktopAssets;

impl AssetSource for DesktopAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(asset) = ShellAssets.load(path)? {
            return Ok(Some(asset));
        }
        if let Some(asset) = ComponentAssets.load(path)? {
            return Ok(Some(asset));
        }
        if let Some(asset) = RssAssets.load(path)? {
            return Ok(Some(asset));
        }
        if let Some(asset) = AccountAssets.load(path)? {
            return Ok(Some(asset));
        }
        if let Some(asset) = FlagAssets.load(path)? {
            return Ok(Some(asset));
        }
        if let Some(asset) = IconPackAssets.load(path)? {
            return Ok(Some(asset));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut assets = gpui_component_assets::Assets.list(path)?;
        assets.extend(ShellAssets.list(path)?);
        assets.extend(ComponentAssets.list(path)?);
        assets.extend(RssAssets.list(path)?);
        assets.extend(AccountAssets.list(path)?);
        assets.extend(FlagAssets.list(path)?);
        assets.extend(IconPackAssets.list(path)?);
        Ok(assets)
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_ui_account::{CLOUD_ICON_PATH, CROWN_ICON_PATH};
    use fluxdown_ui_components::FluxIcon;
    use fluxdown_ui_rss::RSS_ICON_PATH;
    use fluxdown_ui_shell::APP_LOGO_PATH;
    use gpui::AssetSource;

    use super::DesktopAssets;

    #[test]
    fn desktop_assets_cover_shell_capabilities_and_component_icons() -> gpui::Result<()> {
        let assets = DesktopAssets;
        assert!(assets.load(APP_LOGO_PATH)?.is_some());
        assert!(assets.load(RSS_ICON_PATH)?.is_some());
        assert!(assets.load(CROWN_ICON_PATH)?.is_some());
        assert!(assets.load(CLOUD_ICON_PATH)?.is_some());
        assert!(assets.load(FluxIcon::FilePlay.asset_path())?.is_some());
        assert!(assets.load("icons/window-close.svg")?.is_some());
        let lucide = fluxdown_ui_icon_pack::builtin_pack("lucide").expect("builtin lucide");
        let (_, icon) = lucide
            .matched_icon("a.mp4", fluxdown_ui_icon_pack::FileKind::Video)
            .expect("video icon");
        assert!(assets.load(&icon.svg.path)?.is_some());
        for code in fluxdown_ui_downloads::flag_codes() {
            let path = fluxdown_ui_downloads::flag_asset_path(code);
            let bytes = match path {
                Some(path) => assets.load(path)?,
                None => None,
            };
            assert!(
                bytes.is_some_and(|bytes| bytes.starts_with(b"<svg")),
                "flag {code} does not resolve through DesktopAssets"
            );
        }
        Ok(())
    }
}
