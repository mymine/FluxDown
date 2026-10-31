//! 游客发送方会话：未登录时凭本机 endpoint 私钥签名向 FluxCloud 换游客令牌
//! （`POST /api/v1/share/guest-session`），用于创建 / 修改 / 停止 / 对账游客分享与上报遥测。
//!
//! 令牌只绑定 endpoint、与账号无关：本机登录后，之前以游客身份创建的分享仍须用它管理。
//! 令牌缓存在内存，临近到期（[`REFRESH_MARGIN_SECS`]）才重新签名换取，不做周期刷新。
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fluxdown_protocol::RpcErrorData;
use serde::Deserialize;
use serde_json::json;

use super::cloud::share_error;
use super::service::ShareService;
use super::time::parse_rfc3339_secs;

/// 距到期不足该秒数即重新换取，保证一次调用期间令牌不会过期。
const REFRESH_MARGIN_SECS: u64 = 600;

/// 已换取的游客发送方令牌。
#[derive(Clone, Debug)]
pub(super) struct GuestSession {
    token: String,
    expires_at: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuestSessionResponse {
    guest_token: String,
    expires_at: String,
}

/// 被签名的消息，与 FluxCloud `share::guest::signed_message` 逐字节一致。
fn signed_message(endpoint_z32: &str, timestamp: u64) -> String {
    format!("fluxdown-share-guest-session/v1\n{endpoint_z32}\n{timestamp}")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl ShareService {
    /// 游客发送方令牌：缓存仍新鲜则直接用，否则签名换取并同步给遥测出口。
    pub(super) async fn guest_sender_token(&self) -> Result<String, RpcErrorData> {
        let now = unix_now();
        if let Some(session) = self.with(|i| i.guest_session.clone())
            && session.expires_at.saturating_sub(now) > REFRESH_MARGIN_SECS
        {
            return Ok(session.token);
        }
        let endpoint = self.secret.public().to_z32();
        let signature = self.secret.sign(signed_message(&endpoint, now).as_bytes());
        let device_name = self.state.lock().await.device_name.clone();
        let mut body = json!({
            "endpointId": endpoint,
            "timestamp": now,
            "signature": URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        });
        if !device_name.is_empty() {
            body["deviceName"] = json!(device_name);
        }
        let response: GuestSessionResponse = self
            .cloud
            .share_guest_session(&body)
            .await
            .map_err(|error| share_error(&error))?;
        let expires_at = parse_rfc3339_secs(&response.expires_at).unwrap_or(now);
        let token = response.guest_token;
        let owned: Vec<String> = self.with(|i| {
            i.guest_session = Some(GuestSession {
                token: token.clone(),
                expires_at,
            });
            i.shares
                .iter()
                .filter(|s| s.guest_owned)
                .map(|s| s.dto.id.clone())
                .collect()
        });
        self.remember_guest_token(&owned, &token);
        Ok(token)
    }

    /// 某个分享的归属方令牌：游客分享 → 游客令牌；账号分享 → `None`（用账号鉴权）。
    pub(super) async fn owner_token(
        &self,
        guest_owned: bool,
    ) -> Result<Option<String>, RpcErrorData> {
        if guest_owned {
            self.guest_sender_token().await.map(Some)
        } else {
            Ok(None)
        }
    }

    /// 让遥测出口用游客令牌上报这些分享的发送方会话与网络快照。
    pub(super) fn remember_guest_token(&self, share_ids: &[String], token: &str) {
        let mut guests = self
            .telemetry
            .guests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for id in share_ids {
            guests.insert(id, token.to_owned());
        }
    }
}
