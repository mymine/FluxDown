//! agent 自身外网下载（应用更新、嗅探规则包……）的统一出口。
//!
//! 出口方案只有一个事实源：daemon 的 `daemon.egress.plan`——与下载任务、托管组件、插件包同一套
//! 全局代理与插件改写规则（GitHub 加速等）。agent 不读下载设置，只按方案装配 HTTP 客户端，
//! 并在插件改写地址失败或内容校验不符时回退原地址。
//!
//! daemon 不可达（未启动、崩溃、旧版本不认识该方法）时退回系统默认网络（环境 / 系统代理）：
//! 自更新是修复 daemon 的途径，不能因 daemon 故障而不可用。

use std::sync::Arc;

use fluxdown_protocol::{EgressPlanDto, EgressPlanParams, method};

use crate::daemon_client::DaemonClient;
use crate::http_client::HttpClientError;

/// 出口方案来源（运行中的 daemon）。
#[async_trait::async_trait]
pub trait EgressSource: Send + Sync {
    async fn plan(&self, params: EgressPlanParams) -> Result<EgressPlanDto, String>;
}

#[async_trait::async_trait]
impl EgressSource for DaemonClient {
    async fn plan(&self, params: EgressPlanParams) -> Result<EgressPlanDto, String> {
        self.call(method::DAEMON_EGRESS_PLAN, Some(params))
            .await
            .map_err(|error| format!("{error:?}"))
    }
}

type Provider = Arc<dyn Fn() -> Option<Arc<dyn EgressSource>> + Send + Sync>;

/// 出口网络。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Network {
    /// 拿不到出口方案：跟随系统代理（环境变量 / 系统设置，含 macOS 例外列表）。
    SystemDefault,
    /// 直连：不得再套用环境 / 系统代理。
    Direct,
    /// 经代理（`url` 可能含认证信息，不得写日志）。
    Proxy { url: String, no_proxy: String },
}

impl Network {
    fn apply(
        &self,
        builder: reqwest::ClientBuilder,
    ) -> Result<reqwest::ClientBuilder, reqwest::Error> {
        Ok(match self {
            Self::SystemDefault => crate::system_proxy::follow_system(builder),
            Self::Direct => builder.no_proxy(),
            Self::Proxy { url, no_proxy } => builder
                .proxy(reqwest::Proxy::all(url)?.no_proxy(reqwest::NoProxy::from_string(no_proxy))),
        })
    }
}

/// 一次请求尝试。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressAttempt {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// 改写该地址的插件；原地址为 `None`。
    pub plugin: Option<String>,
}

/// 一个地址的出口方案：网络 + 依次尝试的地址（插件改写在前，原地址兜底）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressPlan {
    pub network: Network,
    attempts: Vec<EgressAttempt>,
}

impl EgressPlan {
    fn system_default(url: &str) -> Self {
        Self {
            network: Network::SystemDefault,
            attempts: vec![original(url)],
        }
    }

    fn from_dto(url: &str, dto: EgressPlanDto) -> Self {
        let network = match dto.proxy {
            Some(proxy) => Network::Proxy {
                url: proxy.url,
                no_proxy: proxy.no_proxy,
            },
            None => Network::Direct,
        };
        let mut attempts = Vec::with_capacity(2);
        if dto.plugin.is_some() && dto.url != url {
            attempts.push(EgressAttempt {
                url: dto.url,
                headers: dto.headers.into_iter().collect(),
                plugin: dto.plugin,
            });
        }
        attempts.push(original(url));
        Self { network, attempts }
    }

    /// 依次尝试的地址：插件改写地址（若有）在前，原地址兜底。
    #[must_use]
    pub fn attempts(&self) -> &[EgressAttempt] {
        &self.attempts
    }
}

fn original(url: &str) -> EgressAttempt {
    EgressAttempt {
        url: url.to_owned(),
        headers: Vec::new(),
        plugin: None,
    }
}

/// agent 侧出口入口；廉价可克隆。
#[derive(Clone)]
pub struct Egress {
    provider: Provider,
}

impl Egress {
    /// 经本 agent 连接的 daemon 取方案。
    #[must_use]
    pub fn daemon(client: Arc<DaemonClient>) -> Self {
        let source: Arc<dyn EgressSource> = client;
        Self::dynamic(move || Some(Arc::clone(&source)))
    }

    /// 来源随宿主生命周期变化（原生 Android：本机主机可能未运行）；`None` = 当前没有。
    #[must_use]
    pub fn dynamic(
        provider: impl Fn() -> Option<Arc<dyn EgressSource>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            provider: Arc::new(provider),
        }
    }

    /// 没有 daemon 的宿主：恒为系统默认网络、原地址。
    #[must_use]
    pub fn system_default() -> Self {
        Self::dynamic(|| None)
    }

    /// `url` 的出口方案。`verified_payload`：调用方会按独立来源的哈希 / 签名校验内容，只有这类
    /// 载荷才允许插件改写；校验和、版本清单等完整性锚必须为 `false`。
    pub async fn plan(&self, url: &str, verified_payload: bool) -> EgressPlan {
        let Some(source) = (self.provider)() else {
            return EgressPlan::system_default(url);
        };
        let params = EgressPlanParams {
            url: url.to_owned(),
            verified_payload,
        };
        match source.plan(params).await {
            Ok(dto) => {
                let plan = EgressPlan::from_dto(url, dto);
                if let Some(plugin) = plan.attempts.first().and_then(|a| a.plugin.as_deref()) {
                    tracing::info!(plugin, "host download rewritten by plugin");
                }
                plan
            }
            Err(error) => {
                tracing::warn!(%error, "egress plan unavailable; using the system default network");
                EgressPlan::system_default(url)
            }
        }
    }
}

/// 按出口网络缓存的 HTTP 客户端：网络（代理）变化时重建。构建放在阻塞线程池
/// （加载系统根证书较慢，见 [`crate::http_client`]）。
pub struct EgressClient {
    configure: Arc<dyn Fn() -> reqwest::ClientBuilder + Send + Sync>,
    cached: tokio::sync::Mutex<Option<(Network, reqwest::Client)>>,
}

impl EgressClient {
    /// `configure` 给出除代理外的全部构建参数（超时、UA …）。
    pub fn new(configure: impl Fn() -> reqwest::ClientBuilder + Send + Sync + 'static) -> Self {
        Self {
            configure: Arc::new(configure),
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// `network` 对应的客户端；与上次相同则复用。
    pub async fn get(&self, network: &Network) -> Result<reqwest::Client, HttpClientError> {
        let mut cached = self.cached.lock().await;
        if let Some((current, client)) = cached.as_ref()
            && current == network
        {
            return Ok(client.clone());
        }
        let configure = Arc::clone(&self.configure);
        let target = network.clone();
        let client = tokio::task::spawn_blocking(move || {
            target
                .apply(configure())
                .and_then(reqwest::ClientBuilder::build)
        })
        .await??;
        *cached = Some((network.clone(), client.clone()));
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use fluxdown_protocol::{EgressPlanDto, EgressProxyDto};

    use super::{EgressAttempt, EgressPlan, Network};

    const SOURCE: &str = "https://github.com/o/r/releases/download/v1/a.zip";

    #[test]
    fn rewritten_plan_tries_the_plugin_address_then_the_original() {
        let plan = EgressPlan::from_dto(
            SOURCE,
            EgressPlanDto {
                url: "https://mirror.example/a.zip".to_owned(),
                headers: BTreeMap::from([("x-token".to_owned(), "1".to_owned())]),
                plugin: Some("fluxdown@github-accel".to_owned()),
                proxy: Some(EgressProxyDto {
                    url: "socks5h://127.0.0.1:7890".to_owned(),
                    no_proxy: "localhost".to_owned(),
                }),
            },
        );
        assert_eq!(
            plan.network,
            Network::Proxy {
                url: "socks5h://127.0.0.1:7890".to_owned(),
                no_proxy: "localhost".to_owned(),
            }
        );
        assert_eq!(
            plan.attempts(),
            [
                EgressAttempt {
                    url: "https://mirror.example/a.zip".to_owned(),
                    headers: vec![("x-token".to_owned(), "1".to_owned())],
                    plugin: Some("fluxdown@github-accel".to_owned()),
                },
                EgressAttempt {
                    url: SOURCE.to_owned(),
                    headers: Vec::new(),
                    plugin: None,
                },
            ]
        );
    }

    #[test]
    fn plan_without_proxy_connects_directly_instead_of_inheriting_system_proxy() {
        let plan = EgressPlan::from_dto(
            SOURCE,
            EgressPlanDto {
                url: SOURCE.to_owned(),
                ..EgressPlanDto::default()
            },
        );
        assert_eq!(plan.network, Network::Direct);
        assert_eq!(plan.attempts().len(), 1);
    }
}
