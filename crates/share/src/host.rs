//! 宿主（app）注入的窗口入口：FluxMesh视图自己不创建窗口，只通过这些回调请求 app 打开。
//! 未注入的入口对应按钮无动作。

use std::rc::Rc;

use gpui::{App, Window};

use crate::model::PanelTarget;

/// 打开 / 置前一个分享面板窗口。
pub type PanelOpener = Rc<dyn Fn(PanelTarget, &mut Window, &mut App)>;
/// 打开 / 置前接收确认窗口：参数是取件码或分享链接（空串 = 只打开输入态）。
pub type ReceiveOpener = Rc<dyn Fn(String, &mut Window, &mut App)>;
/// 切到分享页（活动栏路由）。
pub type ManageOpener = Rc<dyn Fn(&mut Window, &mut App)>;
/// 打开设置 › FluxMesh › 网络诊断（`Option<String>` = 要聚焦的会话 ID）。
pub type DiagnosticsOpener = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;
/// 打开账号页（购买流量包 / 升级会员 / 登录）。
pub type AccountOpener = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone, Default)]
pub struct ShareHost {
    pub open_panel: Option<PanelOpener>,
    pub open_receive: Option<ReceiveOpener>,
    pub open_manage: Option<ManageOpener>,
    pub open_diagnostics: Option<DiagnosticsOpener>,
    pub open_account: Option<AccountOpener>,
}
