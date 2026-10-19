//! 账户端口调用的结果解码与错误分类（购买对话框与推介页共用）。

use fluxdown_protocol::{ApplicationErrorCode, RpcErrorData};

/// 端口返回的 JSON 形状不符：按内部错误上报，原因写日志而不是静默吞掉。
pub(crate) fn invalid_payload(error: &serde_json::Error) -> RpcErrorData {
    log::warn!("unexpected account RPC payload: {error}");
    RpcErrorData::new(ApplicationErrorCode::Internal, false)
}

/// 暂时性错误（本机服务不可达 / 超时 / 断线）：轮询类流程应继续下一轮而不是中断。
pub(crate) fn is_transient(error: &RpcErrorData) -> bool {
    matches!(
        error.code,
        ApplicationErrorCode::Unavailable | ApplicationErrorCode::Timeout
    )
}

/// `serde_json::from_value` + [`invalid_payload`] 的组合：`decode!(value => Type)`。
macro_rules! decode {
    ($value:expr => $ty:ty) => {
        serde_json::from_value::<$ty>($value).map_err(|error| $crate::rpc::invalid_payload(&error))
    };
}
pub(crate) use decode;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_errors_are_unavailable_or_timeout() {
        assert!(is_transient(&RpcErrorData::new(
            ApplicationErrorCode::Unavailable,
            true
        )));
        assert!(is_transient(&RpcErrorData::new(
            ApplicationErrorCode::Timeout,
            false
        )));
        assert!(!is_transient(&RpcErrorData::new(
            ApplicationErrorCode::Conflict,
            false
        )));
    }

    #[test]
    fn decode_reports_internal_on_shape_mismatch() {
        let error = decode!(serde_json::json!({"a": 1}) => Vec<String>)
            .err()
            .map(|error| error.code);
        assert_eq!(error, Some(ApplicationErrorCode::Internal));
        assert_eq!(
            decode!(serde_json::json!(["x"]) => Vec<String>).ok(),
            Some(vec!["x".to_owned()])
        );
    }
}
