//! 视图的承载能力：关闭承载它的容器、跳转主界面页面。
//!
//! 插件视图（可移植或桌面脚本）自己不知道被谁承载。宿主（app / extensions）创建视图时
//! 传入 [`ViewContainer`]，视图经它响应 `ctx.close()` / `fluxdown.close()`；跳转由 app 在
//! 启动期注入进程级跳转器（[`set_navigator`]），视图经 [`navigate`] 使用。

use std::rc::Rc;

use gpui::{AnyWindowHandle, App, Global, Window};
use gpui_component::WindowExt as _;

/// 关闭回调。调用方（视图侧）可能正处于容器自己的 update 栈内，所以实现必须自行 defer
/// 到安全时机再动窗口。
pub type CloseHandler = Rc<dyn Fn(&mut App)>;

/// 承载一个插件视图的容器。默认（`Default`）为不可关闭的嵌入页（活动栏页 / 任务详情 tab）。
#[derive(Clone, Default)]
pub struct ViewContainer {
    close: Option<CloseHandler>,
}

impl ViewContainer {
    /// 可关闭容器：`close` 负责（defer 后）关闭它。
    pub fn closable(close: impl Fn(&mut App) + 'static) -> Self {
        Self {
            close: Some(Rc::new(close)),
        }
    }

    /// 以 `window` 里当前最上层对话框为容器：关闭 = defer 后关闭该对话框。
    pub fn dialog(window: &Window) -> Self {
        Self::closable(close_on_window(window.window_handle(), |window, cx| {
            window.close_dialog(cx);
        }))
    }

    /// 以整个 `window` 为容器：关闭 = defer 后移除该窗口。
    pub fn window(window: &Window) -> Self {
        Self::closable(close_on_window(window.window_handle(), |window, _| {
            window.remove_window();
        }))
    }

    /// 容器是否可关闭。
    #[must_use]
    pub fn can_close(&self) -> bool {
        self.close.is_some()
    }

    /// 请求关闭容器；容器不可关闭时返回 `false`。
    pub fn close(&self, cx: &mut App) -> bool {
        match &self.close {
            Some(close) => {
                close(cx);
                true
            }
            None => false,
        }
    }
}

impl std::fmt::Debug for ViewContainer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewContainer")
            .field("closable", &self.can_close())
            .finish()
    }
}

/// 关闭回调：defer 后在 `handle` 对应窗口内执行 `act`；窗口已释放只记 debug。
fn close_on_window(
    handle: AnyWindowHandle,
    act: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> impl Fn(&mut App) + 'static {
    move |cx: &mut App| {
        let act = act.clone();
        cx.defer(move |cx| {
            if let Err(error) = handle.update(cx, |_, window, cx| act(window, cx)) {
                log::debug!("plugin view container released before close: {error:#}");
            }
        });
    }
}

/// 跳转器：`(调用方插件 identity, 路由 id, cx)` → 同步校验并（defer 后）执行；目标不存在返回
/// 可显示的原因。
pub type Navigator = Rc<dyn Fn(&str, &str, &mut App) -> Result<(), String>>;

struct NavigatorGlobal(Navigator);

impl Global for NavigatorGlobal {}

/// 安装进程级跳转器（app 启动期调用一次）。
pub fn set_navigator(cx: &mut App, navigator: Navigator) {
    cx.set_global(NavigatorGlobal(navigator));
}

/// 让主界面切到 `route`（内置页 id，或 `plugin:<identity>:<view>`）。`identity` 是发起方插件，
/// 跳转器据此拒绝跳到别的插件的页面。
pub fn navigate(cx: &mut App, identity: &str, route: &str) -> Result<(), String> {
    let Some(navigator) = cx
        .try_global::<NavigatorGlobal>()
        .map(|global| global.0.clone())
    else {
        return Err("navigation is not available".to_owned());
    };
    navigator(identity, route, cx)
}
