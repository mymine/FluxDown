//! RPC 错误 → 文案键：优先按 `reason`（`errReasonShare*`），缺失或非FluxMesh原因时给通用失败提示。

use fluxdown_protocol::{ErrorReason, RpcErrorData};
use fluxdown_ui_i18n::Translator;

use crate::model::format_datetime;

/// 通用失败文案键（agent 不可达 / 未带细分原因）。
const GENERIC_ERROR_KEY: &str = "localServiceActionFailed";

/// 错误的文案键。
#[must_use]
pub fn error_key(error: &RpcErrorData) -> &'static str {
    reason_key(error.reason)
}

/// 错误原因的文案键（云端上传进度里的失败 / 暂停原因没有 `RpcErrorData`，直接按原因取）。
#[must_use]
pub fn reason_key(reason: Option<ErrorReason>) -> &'static str {
    match reason {
        Some(ErrorReason::ShareUnavailable) => "errReasonShareUnavailable",
        Some(ErrorReason::ShareNotFound) => "errReasonShareNotFound",
        Some(ErrorReason::ShareExpired) => "errReasonShareExpired",
        Some(ErrorReason::ShareStopped) => "errReasonShareStopped",
        Some(ErrorReason::ShareCodeInvalid) => "errReasonShareCodeInvalid",
        Some(ErrorReason::ShareRateLimited) => "errReasonShareRateLimited",
        Some(ErrorReason::ShareTurnstileRequired) => "errReasonShareTurnstileRequired",
        Some(ErrorReason::ShareFileChanged) => "errReasonShareFileChanged",
        Some(ErrorReason::ShareSenderOffline) => "errReasonShareSenderOffline",
        Some(ErrorReason::ShareTaskNotShareable) => "errReasonShareTaskNotShareable",
        Some(ErrorReason::ShareBridgeUnavailable) => "errReasonShareBridgeUnavailable",
        Some(ErrorReason::ShareQuotaExceeded) => "errReasonShareQuotaExceeded",
        Some(ErrorReason::ShareCloudNotReady) => "errReasonShareCloudNotReady",
        Some(ErrorReason::ShareTrafficExhausted) => "errReasonShareTrafficExhausted",
        Some(ErrorReason::ShareCloudUnavailable) => "errReasonShareCloudUnavailable",
        Some(ErrorReason::ShareOwnShare) => "errReasonShareOwnShare",
        Some(ErrorReason::ShareSourceMissing) => "errReasonShareSourceMissing",
        Some(ErrorReason::ShareSwarmLocked) => "errReasonShareSwarmLocked",
        Some(ErrorReason::ShareSwarmQuotaExhausted) => "errReasonShareSwarmQuotaExhausted",
        Some(ErrorReason::ShareSwarmLoginRequired) => "errReasonShareSwarmLoginRequired",
        Some(ErrorReason::ShareStrikeLimited) => "errReasonShareStrikeLimited",
        Some(ErrorReason::ShareAppealExists) => "errReasonShareAppealExists",
        _ => GENERIC_ERROR_KEY,
    }
}

/// 错误的本地化文案。
#[must_use]
pub fn error_text(translator: &Translator, error: &RpcErrorData) -> String {
    if error.reason == Some(ErrorReason::ShareStrikeLimited) {
        return strike_limited_text(translator, error.field.as_deref());
    }
    translator.text(error_key(error)).to_owned()
}

/// 累犯限制文案：`field` 携带解除时刻（RFC 3339），按本机时区格式化；缺失时用 `—` 占位。
fn strike_limited_text(translator: &Translator, until: Option<&str>) -> String {
    let time = until
        .filter(|value| !value.is_empty())
        .map_or_else(|| "—".to_owned(), format_datetime);
    translator.text_with("errReasonShareStrikeLimited", &[("time", time.as_str())])
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{ApplicationErrorCode, ErrorReason, RpcErrorData};

    use super::{error_key, strike_limited_text};

    #[test]
    fn share_reasons_map_to_their_keys_and_everything_else_is_generic() {
        let with =
            |reason| RpcErrorData::new(ApplicationErrorCode::Conflict, false).with_reason(reason);
        assert_eq!(
            error_key(&with(ErrorReason::ShareExpired)),
            "errReasonShareExpired"
        );
        assert_eq!(
            error_key(&with(ErrorReason::ShareBridgeUnavailable)),
            "errReasonShareBridgeUnavailable"
        );
        for (reason, key) in [
            (
                ErrorReason::ShareCloudNotReady,
                "errReasonShareCloudNotReady",
            ),
            (
                ErrorReason::ShareTrafficExhausted,
                "errReasonShareTrafficExhausted",
            ),
            (
                ErrorReason::ShareCloudUnavailable,
                "errReasonShareCloudUnavailable",
            ),
            (ErrorReason::ShareSwarmLocked, "errReasonShareSwarmLocked"),
            (
                ErrorReason::ShareSwarmQuotaExhausted,
                "errReasonShareSwarmQuotaExhausted",
            ),
            (
                ErrorReason::ShareSwarmLoginRequired,
                "errReasonShareSwarmLoginRequired",
            ),
        ] {
            assert_eq!(error_key(&with(reason)), key);
        }
        assert_eq!(
            error_key(&with(ErrorReason::SaveDirUnavailable)),
            "localServiceActionFailed"
        );
        assert_eq!(
            error_key(&RpcErrorData::new(ApplicationErrorCode::Unavailable, true)),
            "localServiceActionFailed"
        );
    }

    #[test]
    fn strike_and_appeal_reasons_have_keys() {
        let with =
            |reason| RpcErrorData::new(ApplicationErrorCode::Conflict, false).with_reason(reason);
        assert_eq!(
            error_key(&with(ErrorReason::ShareStrikeLimited)),
            "errReasonShareStrikeLimited"
        );
        assert_eq!(
            error_key(&with(ErrorReason::ShareAppealExists)),
            "errReasonShareAppealExists"
        );
    }

    #[test]
    fn strike_limited_text_fills_time_or_placeholder() -> Result<(), fluxdown_ui_i18n::I18nError> {
        let catalog = std::sync::Arc::new(fluxdown_ui_i18n::I18nCatalog::load_embedded()?);
        let translator = catalog.translator("en");
        assert!(strike_limited_text(&translator, None).contains('—'));
        assert!(strike_limited_text(&translator, Some("")).contains('—'));
        assert!(strike_limited_text(&translator, Some("not-a-time")).contains("not-a-time"));
        Ok(())
    }
}
