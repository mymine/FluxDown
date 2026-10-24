//! 出站 HTTP 的共享工具：客户端构造、错误链展开、对端响应分类。
//!
//! 配对是**跨设备**协议，发起方看到的只有对端（或对端前面的反代）的 HTTP 响应。此前发起方
//! 把任何 400 都改写成「配对码错误」，反代对明文打 443 时返回的 nginx/Cloudflare 400 页面
//! 就被误报成「配对码过期或不正确」。分类规则集中在这里：
//!
//! - **FluxDown 的错误体**恒为 `{"success":false,"message":"…"}`：能按稳定契约串还原就还原，
//!   还原不了也是「对端 FluxDown 拒绝了」，不是「不是 FluxDown」。
//! - 其余 4xx / 3xx（重定向）/ HTML / 非 JSON → [`LinkError::NotFluxDown`]；
//!   502/503/504（反代后面没有服务）与网络失败 → [`LinkError::Io`]（对端不可达）。

use serde_json::Value;

use super::error::{LinkError, LinkResult};

/// 出站请求共用的 HTTP 客户端：不走系统代理（回环 / 局域网直连），**不跟随重定向**
/// （对 POST 的 301/302 静默改成 GET 会把「地址填成 http 而对端要求 https」伪装成别的错误）。
#[must_use]
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 展开错误的整条 `source()` 链（`error sending request: … : connection refused`），
/// 让日志与错误信息带上真正的根因，而不只是最外层的一句话。
#[must_use]
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// 请求发送失败（DNS / 连接 / TLS / 超时）→ 对端不可达，附完整根因。
#[must_use]
pub fn send_failure(error: &reqwest::Error) -> LinkError {
    LinkError::Io(error_chain(error))
}

/// FluxDown 错误体：`{"success":false,"message":"…"}`。返回其 `message`。
fn fluxdown_error_message(body: &[u8]) -> Option<String> {
    let json: Value = serde_json::from_slice(body).ok()?;
    if json.get("success").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    json.get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// 把一个**非成功**响应分类成本端错误（见模块文档）。
pub async fn classify_failure(resp: reqwest::Response) -> LinkError {
    let status = resp.status();
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = resp.bytes().await.unwrap_or_default();
    classify_status_body(status, location.as_deref(), &body)
}

/// 对端 401 响应里的诊断：`{"reason":"notPaired"|"clockSkew"|"unauthorized","serverTime":秒}`。
/// 响应本身未经认证，只用来在日志里留下线索，并让发起方给出「两台设备时间相差过大」的准确
/// 提示（[`LinkError::ClockSkew`]）；**不得**据此改变任何持久化状态。返回 `reason`。
fn log_auth_rejection(body: &[u8]) -> Option<String> {
    let json = serde_json::from_slice::<Value>(body).ok()?;
    let reason = json.get("reason").and_then(Value::as_str)?;
    let server_time = json.get("serverTime").and_then(Value::as_i64);
    tracing::warn!(reason, server_time, "the peer rejected the link request");
    Some(reason.to_owned())
}

fn classify_status_body(
    status: reqwest::StatusCode,
    location: Option<&str>,
    body: &[u8],
) -> LinkError {
    if let Some(message) = fluxdown_error_message(body) {
        if status.as_u16() == 401 && log_auth_rejection(body).as_deref() == Some("clockSkew") {
            return LinkError::ClockSkew;
        }
        return LinkError::from_wire_message(&message).unwrap_or_else(|| match status.as_u16() {
            400 => LinkError::BadPayload(message),
            401 => LinkError::Unauthorized,
            _ => LinkError::Io(format!("peer returned HTTP {status}: {message}")),
        });
    }
    if status.is_redirection() {
        return LinkError::NotFluxDown(match location {
            Some(location) => format!("HTTP {status} redirect to {location}"),
            None => format!("HTTP {status} redirect"),
        });
    }
    if matches!(status.as_u16(), 502..=504) {
        return LinkError::Io(format!(
            "gateway returned HTTP {status} (upstream not reachable)"
        ));
    }
    LinkError::NotFluxDown(format!("HTTP {status}, response is not a FluxDown error"))
}

/// 读取**成功**响应体并要求是 JSON 对象；反代把请求转给了别的服务（SPA 兜底 HTML、
/// 别家 API）时不是 JSON 对象 → [`LinkError::NotFluxDown`]。
pub async fn read_json_object(resp: reqwest::Response) -> LinkResult<Value> {
    let status = resp.status();
    let body = resp.bytes().await.map_err(|error| send_failure(&error))?;
    serde_json::from_slice::<Value>(&body)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| LinkError::NotFluxDown(format!("HTTP {status} but the body is not JSON")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use reqwest::StatusCode;

    use super::*;

    #[test]
    fn fluxdown_error_bodies_keep_their_meaning() {
        let body = br#"{"success":false,"message":"pairing code invalid or expired"}"#;
        assert!(matches!(
            classify_status_body(StatusCode::BAD_REQUEST, None, body),
            LinkError::InvalidCode
        ));
        let throttled =
            br#"{"success":false,"message":"too many failed pairing attempts, try again later"}"#;
        assert!(matches!(
            classify_status_body(StatusCode::BAD_REQUEST, None, throttled),
            LinkError::Throttled
        ));
    }

    #[test]
    fn html_400_from_a_reverse_proxy_is_not_a_wrong_code() {
        // nginx 对明文请求打到 TLS 端口时的经典 400 页面。
        let body = b"<html><head><title>400 The plain HTTP request was sent to HTTPS port</title>";
        assert!(matches!(
            classify_status_body(StatusCode::BAD_REQUEST, None, body),
            LinkError::NotFluxDown(_)
        ));
        // 别家 API 的 JSON 400（没有 success=false）同样不是 FluxDown。
        assert!(matches!(
            classify_status_body(
                StatusCode::BAD_REQUEST,
                None,
                br#"{"message":"Bad Request"}"#
            ),
            LinkError::NotFluxDown(_)
        ));
    }

    #[test]
    fn redirect_is_reported_with_its_target() {
        let error = classify_status_body(
            StatusCode::MOVED_PERMANENTLY,
            Some("https://nas.example.com/"),
            b"",
        );
        match error {
            LinkError::NotFluxDown(detail) => assert!(detail.contains("https://nas.example.com/")),
            other => panic!("expected NotFluxDown, got {other:?}"),
        }
    }

    #[test]
    fn dead_upstream_behind_a_proxy_is_unreachable_not_not_fluxdown() {
        for status in [
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(matches!(
                classify_status_body(status, None, b"<html>bad gateway</html>"),
                LinkError::Io(_)
            ));
        }
    }

    #[test]
    fn unknown_fluxdown_message_on_400_is_a_payload_error() {
        let body = br#"{"success":false,"message":"invalid link payload: bad base64"}"#;
        assert!(matches!(
            classify_status_body(StatusCode::BAD_REQUEST, None, body),
            LinkError::BadPayload(_)
        ));
    }

    #[test]
    fn peer_401_reasons_are_diagnostics_except_clock_skew() {
        // reason / serverTime 只作诊断：发起方不会据此推断「对端已解除配对」之类的持久化结论；
        // 唯一例外是时钟偏差——它是临时的、可由用户修正的，要给出准确提示。
        for (reason, skew) in [
            ("notPaired", false),
            ("clockSkew", true),
            ("unauthorized", false),
        ] {
            let body = serde_json::json!({
                "success": false,
                "message": "unauthorized",
                "reason": reason,
                "serverTime": 1_700_000_000,
            })
            .to_string();
            let error = classify_status_body(StatusCode::UNAUTHORIZED, None, body.as_bytes());
            assert_eq!(matches!(error, LinkError::ClockSkew), skew, "{reason}");
            assert_eq!(matches!(error, LinkError::Unauthorized), !skew, "{reason}");
        }
    }
}
