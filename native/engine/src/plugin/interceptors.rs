//! 下载拦截平面的插件侧：按任务挑选匹配插件，并把各阶段调用转给脚本运行时。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::{
    CIRCUIT_BREAKER_THRESHOLD, EXTERNAL_TOOL_HOOK_BUDGET, PluginManager, build_typed_settings_json,
    value_of,
};
use crate::intercept::{InterceptStage, InterceptorEntry, TaskInterceptor};
use crate::plugin::audit::PluginAuditRow;
use crate::plugin::manifest::{
    InterceptFailure, PERMISSION_AUTH, PERMISSION_FFMPEG, PERMISSION_INTERCEPT, PERMISSION_YTDLP,
    url_glob_match,
};
use crate::plugin::quickjs::HARD_TIMEOUT_CEILING;
use crate::plugin::runtime::{
    ExecutionBudget, HostContext, PluginEntryKind, PluginError, PluginScript,
};

/// 未声明 `timeoutMs` 时的单次调用超时。
const DEFAULT_INTERCEPT_TIMEOUT: Duration = Duration::from_secs(5);
/// 直播分片阶段每轮轮询都会调用，超时额外压低。
const LIVE_INTERCEPT_TIMEOUT: Duration = Duration::from_secs(2);

impl PluginManager {
    /// 为一个任务挑选参与拦截的插件；无匹配返回 noop 拦截器。
    pub async fn task_interceptor(
        self: &Arc<Self>,
        task_id: &str,
        source_url: &str,
        current_url: &str,
    ) -> Arc<dyn TaskInterceptor> {
        let snapshot = self.plugins.read().await.clone();
        let mut entries: Vec<InterceptorEntry> = snapshot
            .iter()
            .filter(|p| p.enabled && p.manifest.has_permission(PERMISSION_INTERCEPT))
            .filter_map(|p| {
                let decl = p.manifest.interceptors.as_ref()?;
                let hit =
                    decl.match_decl.urls.iter().any(|pat| {
                        url_glob_match(pat, source_url) || url_glob_match(pat, current_url)
                    });
                if !hit {
                    return None;
                }
                let stages: Vec<InterceptStage> = decl
                    .stages
                    .iter()
                    .filter_map(|s| InterceptStage::parse(s))
                    .collect();
                Some(InterceptorEntry {
                    identity: p.manifest.identity.clone(),
                    stages,
                    fail_task: decl.on_failure == InterceptFailure::Fail,
                })
            })
            .collect();
        if entries.is_empty() {
            return crate::intercept::noop();
        }
        entries.sort_by(|a, b| a.identity.cmp(&b.identity));
        Arc::new(PluginTaskInterceptor {
            pm: self.clone(),
            task_id: task_id.to_string(),
            source_url: source_url.to_string(),
            entries,
        })
    }

    /// 调用某插件的 `intercept(ctx)`。
    pub(super) async fn intercept(
        &self,
        identity: &str,
        task_id: &str,
        stage: InterceptStage,
        arg_json: String,
        file_root: Option<PathBuf>,
    ) -> Result<Option<String>, PluginError> {
        let (manifest, streak, source) = {
            let snapshot = self.plugins.read().await.clone();
            let Some(p) = snapshot
                .iter()
                .find(|p| p.manifest.identity == identity && p.enabled)
            else {
                return Err(PluginError::Runtime(format!(
                    "插件 {identity} 已卸载或被禁用"
                )));
            };
            let source = p.intercept_source().await;
            (p.manifest.clone(), p.timeout_streak.clone(), source)
        };
        let Some(source) = source else {
            return Err(PluginError::Runtime(format!(
                "插件 {identity} 未提供 interceptors 入口"
            )));
        };

        let values = self.load_setting_values(identity).await;
        for f in &manifest.settings {
            if f.required && value_of(&values, f).is_none() {
                return Err(PluginError::MissingRequiredSetting(format!(
                    "插件 {identity} 需先配置「{}」",
                    f.title
                )));
            }
        }
        let settings_json = build_typed_settings_json(&manifest, &values);

        let finalize = stage == InterceptStage::Finalize;
        let ffmpeg_permitted = manifest.has_permission(PERMISSION_FFMPEG);
        let ytdlp_permitted = manifest.has_permission(PERMISSION_YTDLP);
        let long_running = finalize && (ffmpeg_permitted || ytdlp_permitted);
        let budget = if long_running {
            EXTERNAL_TOOL_HOOK_BUDGET
        } else {
            let mut timeout = manifest
                .interceptors
                .as_ref()
                .and_then(|d| d.timeout_ms)
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_INTERCEPT_TIMEOUT)
                .min(HARD_TIMEOUT_CEILING);
            if stage == InterceptStage::LiveSegments {
                timeout = timeout.min(LIVE_INTERCEPT_TIMEOUT);
            }
            if stage == InterceptStage::Refresh {
                timeout = timeout.min(DEFAULT_INTERCEPT_TIMEOUT);
            }
            ExecutionBudget {
                timeout,
                memory_limit_bytes: self.resolve_budget.memory_limit_bytes,
            }
        };

        let script = PluginScript {
            identity: identity.to_string(),
            source,
            entry_fn_hint: PluginEntryKind::Intercept,
            version: manifest.version.clone(),
            app_version: self.app_version.clone(),
        };
        let host = HostContext {
            ffmpeg_permitted,
            ffmpeg_root: if finalize { file_root } else { None },
            ytdlp_permitted,
            auth_permitted: manifest.has_permission(PERMISSION_AUTH),
        };
        let result = self
            .runtime
            .invoke_intercept(
                &script,
                arg_json,
                settings_json,
                self.bridge.clone(),
                budget,
                host,
                long_running,
                finalize.then(|| task_id.to_string()),
            )
            .await;

        match &result {
            Err(PluginError::Timeout) | Err(PluginError::MemoryLimitExceeded) => {
                let n = streak.fetch_add(1, Ordering::SeqCst) + 1;
                if n >= CIRCUIT_BREAKER_THRESHOLD {
                    self.trip_circuit_breaker(identity).await;
                }
            }
            _ => {
                streak.store(0, Ordering::SeqCst);
            }
        }

        let (outcome, detail) = match &result {
            Ok(_) => ("ok", String::new()),
            Err(e) => ("error", e.to_string()),
        };
        self.audit.record(PluginAuditRow::new(
            identity,
            "daemon",
            &format!("intercept.{}", stage.as_str()),
            task_id,
            outcome,
            &detail,
        ));
        result
    }
}

/// `Drop` 时中止底层任务：调用方丢弃 future 即终止 JS 与外部子进程。
struct AbortGuard<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortGuard<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 绑定一个任务的插件拦截器。
struct PluginTaskInterceptor {
    pm: Arc<PluginManager>,
    task_id: String,
    source_url: String,
    entries: Vec<InterceptorEntry>,
}

#[async_trait::async_trait]
impl TaskInterceptor for PluginTaskInterceptor {
    fn entries(&self) -> &[InterceptorEntry] {
        &self.entries
    }

    fn source_url(&self) -> &str {
        &self.source_url
    }

    async fn call(
        &self,
        identity: &str,
        stage: InterceptStage,
        arg_json: String,
        file_root: Option<PathBuf>,
    ) -> Result<Option<String>, String> {
        let pm = self.pm.clone();
        let task_id = self.task_id.clone();
        let identity = identity.to_string();
        let handle = self.pm.runtime_handle().spawn(async move {
            pm.intercept(&identity, &task_id, stage, arg_json, file_root)
                .await
        });
        let mut guard = AbortGuard(handle);
        match (&mut guard.0).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(format!("插件运行时任务异常: {e}")),
        }
    }
}
