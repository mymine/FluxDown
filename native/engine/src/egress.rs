//! 宿主自身外网请求（应用更新、受管组件、插件包、规则包、tracker / server.met 订阅……）的唯一出口。
//!
//! 与下载任务共用两条规则，避免「任务经插件很快、宿主自己下载却很慢」这类分叉：
//!
//! 1. **全局代理**：客户端一律由 [`client_builder`] 按 `proxy_*` 配置装配（与下载任务同一个
//!    装配函数）；[`Egress`] 每次调用都读取最新配置，设置改了下一次请求即生效。
//! 2. **插件改写**：内容经独立来源的哈希 / 签名校验的载荷（[`Egress::route_verified`]）按下载
//!    任务同一套 resolver URL 规则交给插件改写（如 GitHub 加速）；多个插件命中时按无人值守任务的
//!    规则定案，不弹框。
//!
//! 完整性锚（校验和文件、插件索引、版本清单）只走代理、绝不交给插件：插件若能同时改写载荷与其
//! 校验和，就能替换安装包。插件结果只接受单一 https 直链；返回空、多文件清单、变体、独立音轨、
//! 请求补丁或插件出错一律视为不改写。调用方在改写地址下载失败或内容校验不符时必须按
//! [`EgressRoute::attempts`] 回退原地址——加速失败绝不阻断宿主自身下载。
//!
//! 下载任务的拦截器阶段（`Prepare` … `Finalize`）绑定任务生命周期（落盘、重命名、mux），
//! 不作用于宿主自身下载。

#[cfg(feature = "plugins")]
use std::collections::HashMap;
#[cfg(feature = "plugins")]
use std::sync::Arc;

use crate::db::{Db, DbError};
use crate::downloader::DownloadError;
#[cfg(feature = "plugins")]
use crate::logger::{log_info, log_warn};
#[cfg(feature = "plugins")]
use crate::plugin::{PluginManager, ResolveRequest, ResolveResult};
use crate::proxy_config::{ProxyConfig, normalize_no_proxy};

/// 出口方案无法得出。
#[derive(Debug, thiserror::Error)]
pub enum EgressError {
    #[error("read proxy settings: {0}")]
    Config(#[from] DbError),
    #[error("build HTTP client: {0}")]
    Client(#[from] DownloadError),
    #[error("resolve proxy: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// 一次请求尝试。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressAttempt {
    pub url: String,
    /// 改写插件要求附带的请求头；原地址为空。
    pub headers: Vec<(String, String)>,
    /// 改写该地址的插件；原地址为 `None`。
    pub plugin: Option<String>,
}

/// 插件改写结论。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressRewrite {
    pub plugin: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// 一个宿主自身下载地址的请求路线：插件改写地址在前，原地址兜底。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressRoute {
    source_url: String,
    rewrite: Option<EgressRewrite>,
}

impl EgressRoute {
    /// 不经插件改写的路线。
    #[must_use]
    pub fn direct(url: impl Into<String>) -> Self {
        Self {
            source_url: url.into(),
            rewrite: None,
        }
    }

    #[must_use]
    pub fn source_url(&self) -> &str {
        &self.source_url
    }

    #[must_use]
    pub fn rewrite(&self) -> Option<&EgressRewrite> {
        self.rewrite.as_ref()
    }

    /// 依次尝试的请求：改写地址（若有）在前，原地址兜底。
    ///
    /// ```
    /// use fluxdown_engine::egress::EgressRoute;
    ///
    /// let attempts = EgressRoute::direct("https://example.com/a.zip").attempts();
    /// assert_eq!(attempts.len(), 1);
    /// assert_eq!(attempts[0].url, "https://example.com/a.zip");
    /// ```
    #[must_use]
    pub fn attempts(&self) -> Vec<EgressAttempt> {
        let original = EgressAttempt {
            url: self.source_url.clone(),
            headers: Vec::new(),
            plugin: None,
        };
        match &self.rewrite {
            Some(rewrite) => vec![
                EgressAttempt {
                    url: rewrite.url.clone(),
                    headers: rewrite.headers.clone(),
                    plugin: Some(rewrite.plugin.clone()),
                },
                original,
            ],
            None => vec![original],
        }
    }
}

/// 已解析出的出口代理。`url` 可能含认证信息，不得写日志。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyEndpoint {
    pub url: String,
    /// 规范化后的直连名单（reqwest `NoProxy` 语法）。
    pub no_proxy: String,
}

/// 供不在本进程装配客户端的宿主（agent）使用的完整出口方案。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressPlan {
    pub route: EgressRoute,
    /// `None` = 直连。
    pub proxy: Option<ProxyEndpoint>,
}

/// 把全局代理设置解析成具体代理：`System` 读系统设置（可能起子进程，需在阻塞上下文调用），
/// `Auto` 与未配置一样直连（同下载任务的兜底 client）。
#[must_use]
pub fn proxy_endpoint(proxy: &ProxyConfig) -> Option<ProxyEndpoint> {
    let resolved = proxy.resolve();
    let url = resolved.to_proxy_url()?;
    Some(ProxyEndpoint {
        url,
        no_proxy: normalize_no_proxy(&resolved.no_proxy_list),
    })
}

/// 宿主自身请求的客户端装配：与下载任务的 client 同一套代理、TLS 与连接参数；调用方可追加
/// 自己的超时与重定向策略。`user_agent` 为空时使用引擎默认 UA。
pub fn client_builder(
    proxy: &ProxyConfig,
    user_agent: &str,
) -> Result<reqwest::ClientBuilder, DownloadError> {
    crate::downloader::build_client_builder(proxy, user_agent, false)
}

/// 读取当前配置的出口入口；廉价可克隆。
#[derive(Clone)]
pub struct Egress {
    db: Db,
    #[cfg(feature = "plugins")]
    plugins: Option<Arc<PluginManager>>,
}

impl Egress {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self {
            db,
            #[cfg(feature = "plugins")]
            plugins: None,
        }
    }

    /// 启用插件改写（[`Self::route_verified`]）。
    #[cfg(feature = "plugins")]
    #[must_use]
    pub fn with_plugins(mut self, plugins: Arc<PluginManager>) -> Self {
        self.plugins = Some(plugins);
        self
    }

    /// 当前全局代理设置。
    pub async fn proxy(&self) -> Result<ProxyConfig, EgressError> {
        Ok(ProxyConfig::from_config_map(
            &self.db.get_all_config().await?,
        ))
    }

    /// 按当前全局代理装配客户端（引擎默认 UA）。
    pub async fn client_builder(&self) -> Result<reqwest::ClientBuilder, EgressError> {
        let proxy = self.proxy().await?;
        // `System` 模式要读系统代理设置（可能起子进程），放到阻塞线程池。
        Ok(tokio::task::spawn_blocking(move || client_builder(&proxy, "")).await??)
    }

    /// 给不在本进程装配客户端的宿主的完整方案；`verified_payload` 语义见
    /// [`Self::route_verified`]，为假时只给代理、地址不变。
    pub async fn plan(&self, url: &str, verified_payload: bool) -> Result<EgressPlan, EgressError> {
        let proxy = self.proxy().await?;
        let proxy = tokio::task::spawn_blocking(move || proxy_endpoint(&proxy)).await?;
        let route = if verified_payload {
            self.route_verified(url).await
        } else {
            EgressRoute::direct(url)
        };
        Ok(EgressPlan { route, proxy })
    }

    /// 内容经独立来源校验的载荷地址：按下载任务的 resolver 规则交给插件改写。调用方必须在
    /// 改写地址失败或内容不符时回退原地址（[`EgressRoute::attempts`]）。
    #[cfg(feature = "plugins")]
    pub async fn route_verified(&self, url: &str) -> EgressRoute {
        match &self.plugins {
            Some(plugins) => route_with_plugins(plugins, url).await,
            None => EgressRoute::direct(url),
        }
    }

    /// 插件功能未编译：恒为原地址。
    #[cfg(not(feature = "plugins"))]
    pub async fn route_verified(&self, url: &str) -> EgressRoute {
        EgressRoute::direct(url)
    }
}

#[cfg(feature = "plugins")]
async fn route_with_plugins(plugins: &Arc<PluginManager>, url: &str) -> EgressRoute {
    let manager = Arc::clone(plugins);
    let source = url.to_owned();
    // resolver 只在插件专用 runtime 上执行（同下载任务的 resolve worker）；panic 由 JoinError 隔离。
    let joined = plugins
        .runtime_handle()
        .spawn(async move {
            let identity = manager.unattended_resolver(&source).await?;
            let request = ResolveRequest {
                task_id: String::new(),
                url: source,
                auth_ref: String::new(),
                cookies: String::new(),
                referrer: String::new(),
                user_agent: String::new(),
                extra_headers: HashMap::new(),
                resolver_item: String::new(),
            };
            let outcome = manager.resolve(&identity, request).await;
            Some((identity, outcome))
        })
        .await;
    let host = host_of(url);
    let (identity, outcome) = match joined {
        Ok(Some(found)) => found,
        Ok(None) => return EgressRoute::direct(url),
        Err(error) => {
            log_warn!("[egress] resolver task for {host} failed: {error}");
            return EgressRoute::direct(url);
        }
    };
    match outcome {
        Ok(Some(result)) => match single_direct_link(result) {
            Some((rewritten, headers)) if rewritten != url => {
                log_info!(
                    "[egress] plugin {identity} rewrote host download {host} -> {}",
                    host_of(&rewritten)
                );
                EgressRoute {
                    source_url: url.to_owned(),
                    rewrite: Some(EgressRewrite {
                        plugin: identity,
                        url: rewritten,
                        headers,
                    }),
                }
            }
            Some(_) => EgressRoute::direct(url),
            None => {
                log_info!(
                    "[egress] plugin {identity} result for {host} is not a single https link; using original"
                );
                EgressRoute::direct(url)
            }
        },
        Ok(None) => EgressRoute::direct(url),
        Err(error) => {
            log_warn!(
                "[egress] plugin {identity} resolve for {host} failed: {error:#}; using original"
            );
            EgressRoute::direct(url)
        }
    }
}

/// 只接受单一 https 直链（可带请求头）；其余形态对宿主自身下载没有意义。
#[cfg(feature = "plugins")]
fn single_direct_link(result: ResolveResult) -> Option<(String, Vec<(String, String)>)> {
    if result.manifest.is_some()
        || !result.variants.is_empty()
        || result
            .audio_url
            .as_deref()
            .is_some_and(|audio| !audio.is_empty())
        || result.request.is_some()
    {
        return None;
    }
    let url = result.url.trim();
    let https = url::Url::parse(url)
        .is_ok_and(|parsed| parsed.scheme() == "https" && parsed.host_str().is_some());
    if !https {
        return None;
    }
    let mut headers: Vec<(String, String)> = result
        .extra_headers
        .unwrap_or_default()
        .into_iter()
        .collect();
    headers.sort();
    Some((url.to_owned(), headers))
}

/// 日志只记主机名：完整地址可能带签名参数。
#[cfg(feature = "plugins")]
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .unwrap_or_else(|| "<invalid url>".to_owned())
}

#[cfg(all(test, feature = "plugins"))]
mod tests {
    use super::{EgressAttempt, EgressRewrite, EgressRoute, single_direct_link};
    use crate::plugin::ResolveResult;

    fn link(url: &str) -> ResolveResult {
        ResolveResult {
            url: url.to_owned(),
            ..ResolveResult::default()
        }
    }

    #[test]
    fn only_a_single_https_link_counts_as_a_rewrite() {
        assert_eq!(
            single_direct_link(link(" https://mirror.example/a.zip ")),
            Some(("https://mirror.example/a.zip".to_owned(), Vec::new()))
        );
        assert_eq!(
            single_direct_link(link("http://mirror.example/a.zip")),
            None
        );
        assert_eq!(single_direct_link(link("")), None);
        let mut with_audio = link("https://mirror.example/a.zip");
        with_audio.audio_url = Some("https://mirror.example/a.m4a".to_owned());
        assert_eq!(single_direct_link(with_audio), None);
        let mut with_manifest = link("https://mirror.example/a.zip");
        with_manifest.manifest = Some(crate::plugin::runtime::ResolveManifest::default());
        assert_eq!(single_direct_link(with_manifest), None);
    }

    #[test]
    fn rewritten_route_falls_back_to_the_original_address() {
        let route = EgressRoute {
            source_url: "https://github.com/o/r/releases/download/v1/a.zip".to_owned(),
            rewrite: Some(EgressRewrite {
                plugin: "fluxdown@github-accel".to_owned(),
                url: "https://mirror.example/a.zip".to_owned(),
                headers: vec![("x-token".to_owned(), "1".to_owned())],
            }),
        };
        assert_eq!(
            route.attempts(),
            vec![
                EgressAttempt {
                    url: "https://mirror.example/a.zip".to_owned(),
                    headers: vec![("x-token".to_owned(), "1".to_owned())],
                    plugin: Some("fluxdown@github-accel".to_owned()),
                },
                EgressAttempt {
                    url: "https://github.com/o/r/releases/download/v1/a.zip".to_owned(),
                    headers: Vec::new(),
                    plugin: None,
                },
            ]
        );
    }
}
