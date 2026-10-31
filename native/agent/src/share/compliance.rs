//! 合规：接收方举报（endpoint 签名）与所有者对下架的申诉。

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fluxdown_protocol::{
    ApplicationErrorCode, ErrorReason, RpcErrorData, ShareAppealDto, ShareAppealParams, ShareDto,
    ShareReportParams, ShareStatus,
};
use serde_json::{Value, json};

use super::cloud::share_error;
use super::service::{ShareService, invalid};

/// 被签名的消息，与 FluxCloud `share::download::report_by_receiver` 逐字节一致。
pub(super) fn report_signed_message(endpoint_z32: &str, timestamp: u64) -> String {
    format!("fluxdown-share-report/v1\n{endpoint_z32}\n{timestamp}")
}

const MAX_DETAIL_CHARS: usize = 2000;
const MAX_CONTACT_CHARS: usize = 200;
const MAX_STATEMENT_CHARS: usize = 1000;

fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

impl ShareService {
    /// `agent.share.report`：以本机 endpoint 私钥签名举报；未领取过的分享由云端回 `Forbidden`，重复举报静默成功。
    pub(super) async fn report(&self, params: ShareReportParams) -> Result<Value, RpcErrorData> {
        let detail = non_empty(params.detail);
        let contact = non_empty(params.contact);
        if params.share_id.trim().is_empty()
            || detail
                .as_deref()
                .is_some_and(|d| d.chars().count() > MAX_DETAIL_CHARS)
            || contact
                .as_deref()
                .is_some_and(|c| c.chars().count() > MAX_CONTACT_CHARS)
        {
            return Err(invalid("detail"));
        }
        let endpoint = self.secret.public().to_z32();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let signature = self
            .secret
            .sign(report_signed_message(&endpoint, timestamp).as_bytes());
        let mut body = json!({
            "shareId": params.share_id,
            "endpointId": endpoint,
            "timestamp": timestamp,
            "signature": URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            "reason": params.reason.canonical(),
        });
        if let Some(detail) = detail {
            body["detail"] = json!(detail);
        }
        if let Some(contact) = contact {
            body["contact"] = json!(contact);
        }
        self.cloud
            .share_report(&body)
            .await
            .map_err(|error| share_error(&error))?;
        Ok(json!({ "ok": true }))
    }

    /// `agent.share.appeal`：对已下架的分享申诉（每个分享一次），成功后写回本机分享的 `appeal`。
    pub(super) async fn appeal(&self, params: ShareAppealParams) -> Result<ShareDto, RpcErrorData> {
        let statement = params.statement.trim().to_owned();
        if statement.is_empty() || statement.chars().count() > MAX_STATEMENT_CHARS {
            return Err(invalid("statement"));
        }
        let stored = self.find_share(&params.share_id)?;
        if stored.dto.status != ShareStatus::Takedown {
            return Err(RpcErrorData::new(ApplicationErrorCode::Conflict, false)
                .with_reason(ErrorReason::ShareStopped));
        }
        let guest_token = self.owner_token(stored.guest_owned).await?;
        let appeal: ShareAppealDto = self
            .cloud
            .share_appeal(
                &params.share_id,
                &json!({ "statement": statement }),
                guest_token.as_deref(),
            )
            .await
            .map_err(|error| share_error(&error))?;
        let dto = self.with(|i| {
            i.share_epoch += 1;
            i.shares
                .iter_mut()
                .find(|s| s.dto.id == params.share_id)
                .map(|s| {
                    s.dto.appeal = Some(appeal);
                    s.dto.clone()
                })
        });
        let dto = dto.ok_or_else(|| {
            RpcErrorData::new(ApplicationErrorCode::NotFound, false)
                .with_reason(ErrorReason::ShareNotFound)
        })?;
        self.persist_shares().await;
        self.publish().await;
        Ok(dto)
    }
}
