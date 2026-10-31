//! 通用订阅 provider 接口。
//!
//! 订阅调度、退避、去重、过滤和建任务仍由引擎统一负责；provider 只负责把
//! 自己的数据源抓取并规范化成 [`crate::rss::parser::ParsedFeed`]。内置 RSS
//! provider 与插件 provider 走同一条回流路径。

use std::future::Future;
use std::pin::Pin;

use crate::proxy_config::ProxyConfig;
use crate::rss::parser::ParsedFeed;

/// provider 抓取请求。
#[derive(Debug, Clone)]
pub struct SubscriptionFetchRequest {
    /// 请求要调用的 provider ID。
    pub provider_id: String,
    /// 订阅 ID；provider 可用它关联本地状态或日志。
    pub source_id: String,
    /// provider 配置中的地址。
    pub url: String,
    /// provider 专属配置，通常是 JSON；引擎不解释其内容。
    pub provider_config: String,
    /// 订阅级 Cookie。插件 provider 经 `ctx.cookies` 原样交给脚本。
    pub cookies: String,
    /// 已解析的 User-Agent。插件 provider 只经 `ctx.userAgent` 透传，脚本
    /// 自行决定是否作为请求头发送。
    pub user_agent: String,
    /// 订阅自带代理原值（`tasks.proxy_url` 同语义：空串跟随全局，`direct://` /
    /// `system://` 哨兵）。插件 provider 据此为 `flux.fetch` / yt-dlp 制定出口计划。
    pub proxy_url: String,
    /// 已解析的代理配置。**仅内置 `rss` provider 使用**；插件 provider 按
    /// [`Self::proxy_url`]、站点规则与全局设定自行制定出口（见 `plugin::call_egress`）。
    pub proxy: ProxyConfig,
    /// 站点代理规则的逐请求路由器（重定向各跳按规则换出口）。只有内置 `rss`
    /// provider 使用；插件 provider 走 bridge，由 bridge 自行应用规则。
    pub egress: Option<std::sync::Arc<crate::rule_route::EgressRouter>>,
}

/// provider 抓取 future 的返回类型。
pub type SubscriptionFetchFuture = Pin<Box<dyn Future<Output = Result<ParsedFeed, String>> + Send>>;

/// 订阅数据源适配器。
///
/// 引擎只消费规范化后的 [`ParsedFeed`]，因此 RSS、站点插件或其他来源都
/// 可以复用同一套调度与条目状态机。provider 本身不应负责落库、过滤或建任务。
pub trait SubscriptionProvider: Send + Sync {
    /// 稳定的 provider ID，对应 [`crate::rss::model::RssSourceInfo::provider_id`]。
    fn id(&self) -> &str;

    /// 在 actor 之外抓取并规范化一批订阅条目。
    fn fetch(&self, request: SubscriptionFetchRequest) -> SubscriptionFetchFuture;
}
