use std::borrow::Cow;
use std::sync::Arc;

use gpui::{App, AssetSource, Global, Image, ImageSource, Result, SharedString};

/// FluxDown 品牌图标路径。
pub const APP_LOGO_PATH: &str = "fluxdown/logo.png";
const APP_LOGO: &[u8] = include_bytes!("../../../assets/logo/fluxdown_logo.png");

/// 标题栏 logo 的运行期覆盖（应用图标设置：闪电 / 自定义）；未设置时回落内置 [`APP_LOGO_PATH`]。
/// 由 app 在偏好变化时通过 [`set_app_logo`] 更新，所有主窗口标题栏随之重绘。
pub(crate) struct AppLogo(Arc<Image>);

impl Global for AppLogo {}

/// 更换标题栏 logo；`None` 恢复内置默认。
pub fn set_app_logo(logo: Option<Arc<Image>>, cx: &mut App) {
    match logo {
        Some(image) => cx.set_global(AppLogo(image)),
        None => {
            if cx.has_global::<AppLogo>() {
                cx.remove_global::<AppLogo>();
                cx.refresh_windows();
            }
        }
    }
}

/// 标题栏当前使用的 logo 图像来源。
pub(crate) fn app_logo_source(cx: &App) -> ImageSource {
    match cx.try_global::<AppLogo>() {
        Some(logo) => ImageSource::from(Arc::clone(&logo.0)),
        None => ImageSource::from(APP_LOGO_PATH),
    }
}

/// 窗口 shell 拥有的嵌入资源。
pub struct ShellAssets;

impl AssetSource for ShellAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok((path == APP_LOGO_PATH).then_some(Cow::Borrowed(APP_LOGO)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(if APP_LOGO_PATH.starts_with(path) {
            vec![APP_LOGO_PATH.into()]
        } else {
            Vec::new()
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::AssetSource;

    use super::{APP_LOGO, APP_LOGO_PATH, ShellAssets};

    #[test]
    fn shell_assets_preserve_brand_icon() -> gpui::Result<()> {
        assert_eq!(ShellAssets.load(APP_LOGO_PATH)?.as_deref(), Some(APP_LOGO));
        Ok(())
    }
}
