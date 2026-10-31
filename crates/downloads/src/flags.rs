//! 国家 / 地区旗帜（peer 所在地）。
//!
//! 旗帜是 lipis/flag-icons（MIT，qBittorrent 同款）的 4:3 彩色 SVG，原样放在 `assets/flags/`，
//! 由 [`FlagAssets`] 内嵌并提供（composition root 并入应用 `AssetSource`）。彩色旗帜不能走 `svg()`
//! （会被单色着色），所以按物理像素栅格化成位图再 `img()`：GPUI 资产缓存按（代码，像素宽）去重，
//! 高分屏不糊，也不会每个旗帜常驻 640×480 的位图。

use std::{borrow::Cow, future::Future, sync::Arc};

use gpui::{
    AnyElement, App, Asset, AssetSource, DevicePixels, Hsla, IntoElement as _, ParentElement as _,
    Pixels, RenderImage, Result, SharedString, Styled as _, SvgSize, Window, div, img, size,
};

/// 代码不在内嵌集合时回落的「未知」旗帜。
const UNKNOWN_FLAG: &str = "xx";
/// 栅格宽度上限（物理像素）；旗帜在界面里只有十几个逻辑像素。
const MAX_RASTER: u32 = 256;
/// 内嵌代码的最大长度（`gb-eng`、`es-ct` 等），超过必然不在集合里。
const MAX_CODE_LEN: usize = 8;

struct Flag {
    code: &'static str,
    path: &'static str,
    svg: &'static [u8],
}

macro_rules! flag_table {
    ($($code:literal),* $(,)?) => {
        /// 按代码升序（`binary_search`）；`flags::tests` 校验与 `assets/flags/` 目录一致。
        const FLAGS: &[Flag] = &[$(Flag {
            code: $code,
            path: concat!("fluxdown/flags/", $code, ".svg"),
            svg: include_bytes!(concat!("../../../assets/flags/", $code, ".svg")),
        },)*];
    };
}

flag_table! {
    "ad", "ae", "af", "ag", "ai", "al", "am", "ao", "aq", "ar",
    "arab", "as", "at", "au", "aw", "ax", "az", "ba", "bb", "bd",
    "be", "bf", "bg", "bh", "bi", "bj", "bl", "bm", "bn", "bo",
    "bq", "br", "bs", "bt", "bv", "bw", "by", "bz", "ca", "cc",
    "cd", "cefta", "cf", "cg", "ch", "ci", "ck", "cl", "cm", "cn",
    "co", "cp", "cr", "cu", "cv", "cw", "cx", "cy", "cz", "de",
    "dg", "dj", "dk", "dm", "do", "dz", "eac", "ec", "ee", "eg",
    "eh", "er", "es", "es-ct", "es-ga", "es-pv", "et", "eu", "fi", "fj",
    "fk", "fm", "fo", "fr", "ga", "gb", "gb-eng", "gb-nir", "gb-sct", "gb-wls",
    "gd", "ge", "gf", "gg", "gh", "gi", "gl", "gm", "gn", "gp",
    "gq", "gr", "gs", "gt", "gu", "gw", "gy", "hk", "hm", "hn",
    "hr", "ht", "hu", "ic", "id", "ie", "il", "im", "in", "io",
    "iq", "ir", "is", "it", "je", "jm", "jo", "jp", "ke", "kg",
    "kh", "ki", "km", "kn", "kp", "kr", "kw", "ky", "kz", "la",
    "lb", "lc", "li", "lk", "lr", "ls", "lt", "lu", "lv", "ly",
    "ma", "mc", "md", "me", "mf", "mg", "mh", "mk", "ml", "mm",
    "mn", "mo", "mp", "mq", "mr", "ms", "mt", "mu", "mv", "mw",
    "mx", "my", "mz", "na", "nc", "ne", "nf", "ng", "ni", "nl",
    "no", "np", "nr", "nu", "nz", "om", "pa", "pc", "pe", "pf",
    "pg", "ph", "pk", "pl", "pm", "pn", "pr", "ps", "pt", "pw",
    "py", "qa", "re", "ro", "rs", "ru", "rw", "sa", "sb", "sc",
    "sd", "se", "sg", "sh", "sh-ac", "sh-hl", "sh-ta", "si", "sj", "sk",
    "sl", "sm", "sn", "so", "sr", "ss", "st", "sv", "sx", "sy",
    "sz", "tc", "td", "tf", "tg", "th", "tj", "tk", "tl", "tm",
    "tn", "to", "tr", "tt", "tv", "tw", "tz", "ua", "ug", "um",
    "un", "us", "uy", "uz", "va", "vc", "ve", "vg", "vi", "vn",
    "vu", "wf", "ws", "xk", "xx", "ye", "yt", "za", "zm", "zw",

}

fn find(code: &str) -> Option<&'static Flag> {
    let code = code.trim();
    if code.is_empty() || code.len() > MAX_CODE_LEN {
        return None;
    }
    let mut buf = [0u8; MAX_CODE_LEN];
    let lower = &mut buf[..code.len()];
    lower.copy_from_slice(code.as_bytes());
    lower.make_ascii_lowercase();
    let lower = std::str::from_utf8(lower).ok()?;
    FLAGS
        .binary_search_by(|flag| flag.code.cmp(lower))
        .ok()
        .map(|index| &FLAGS[index])
}

fn unknown() -> &'static Flag {
    // `flags::tests` 保证 `xx` 在表里；万一缺失退回表首项，仍是合法旗帜而不是 panic。
    find(UNKNOWN_FLAG).unwrap_or(&FLAGS[0])
}

/// 旗帜资源路径；代码（大小写不敏感，允许两侧空白）不在内嵌集合时为 `None`。
pub fn flag_asset_path(code: &str) -> Option<&'static str> {
    find(code).map(|flag| flag.path)
}

/// 全部内嵌旗帜的小写代码（资源枚举与测试用）。
pub fn flag_codes() -> impl Iterator<Item = &'static str> {
    FLAGS.iter().map(|flag| flag.code)
}

/// 旗帜拥有的嵌入资源（`fluxdown/flags/<小写代码>.svg`）。
pub struct FlagAssets;

impl AssetSource for FlagAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FLAGS
            .iter()
            .find(|flag| flag.path == path)
            .map(|flag| Cow::Borrowed(flag.svg)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FLAGS
            .iter()
            .map(|flag| flag.path)
            .filter(|asset| asset.starts_with(path))
            .map(SharedString::new_static)
            .collect())
    }
}

#[derive(Clone, Hash)]
struct FlagRaster {
    path: &'static str,
    width: u32,
}

enum FlagImage {}

impl Asset for FlagImage {
    type Source = FlagRaster;
    type Output = std::result::Result<Arc<RenderImage>, SharedString>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        let bytes = FlagAssets
            .load(source.path)
            .map_err(|error| SharedString::from(error.to_string()))
            .and_then(|bytes| bytes.ok_or_else(|| SharedString::from("flag asset missing")));
        async move {
            let bytes = bytes?;
            let parsed = renderer
                .parse_svg(&bytes)
                .map_err(|error| SharedString::from(error.to_string()))?;
            // 旗帜都是 4:3，按宽度缩放即得高度。
            let width = DevicePixels(source.width as i32);
            renderer
                .render_parsed(&parsed, SvgSize::Size(size(width, width)))
                .map_err(|error| SharedString::from(error.to_string()))
        }
    }
}

/// 国旗槽位：宽 `width`、高 `width * 3 / 4`，圆角 + 细描边（白底旗帜在浅色主题下不会糊进背景）。
///
/// 槽位尺寸恒定：`code` 为 `None`（未知 / 内网 / 库未就绪）或栅格尚未完成时只留空位，
/// 所以国家信息晚到不会让行内布局跳动。不在内嵌集合的代码画「未知」旗帜。
pub(crate) fn flag_slot(
    code: Option<&str>,
    width: Pixels,
    radius: Pixels,
    border: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let slot = div()
        .relative()
        .flex_none()
        .w(width)
        .h(width * 0.75)
        .rounded(radius)
        .overflow_hidden();
    let Some(code) = code else {
        return slot.into_any_element();
    };
    let flag = find(code).unwrap_or_else(unknown);
    let physical = ((f32::from(width) * window.scale_factor()).ceil() as u32).clamp(1, MAX_RASTER);
    let key = FlagRaster {
        path: flag.path,
        width: physical,
    };
    match window.use_asset::<FlagImage>(&key, cx) {
        Some(Ok(image)) => slot
            .child(img(image).size_full())
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(radius)
                    .border_1()
                    .border_color(border),
            )
            .into_any_element(),
        // 栅格中 / 失败（内嵌资源不会失败）：保留空位。
        Some(Err(_)) | None => slot.into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use gpui::AssetSource as _;

    use super::{FLAGS, FlagAssets, MAX_CODE_LEN, find, flag_asset_path, flag_codes, unknown};

    #[test]
    fn table_is_sorted_unique_and_short_enough_for_lookup() {
        assert!(FLAGS.windows(2).all(|pair| pair[0].code < pair[1].code));
        assert!(FLAGS.iter().all(|flag| flag.code.len() <= MAX_CODE_LEN));
    }

    #[test]
    fn table_matches_assets_directory() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/flags");
        let on_disk: BTreeSet<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".svg").map(str::to_owned)
            })
            .collect();
        let embedded: BTreeSet<String> = flag_codes().map(str::to_owned).collect();
        assert_eq!(embedded, on_disk);
    }

    #[test]
    fn every_flag_loads_as_svg_through_the_asset_source() -> gpui::Result<()> {
        for code in flag_codes() {
            let path = flag_asset_path(code);
            let bytes = match path {
                Some(path) => FlagAssets.load(path)?,
                None => None,
            };
            assert!(
                bytes.is_some_and(|bytes| bytes.starts_with(b"<svg")),
                "{code} missing or not an svg"
            );
        }
        assert!(FlagAssets.load("fluxdown/flags/zz.svg")?.is_none());
        assert!(FlagAssets.load("icons/file.svg")?.is_none());
        assert_eq!(FlagAssets.list("fluxdown/flags/")?.len(), FLAGS.len());
        assert!(FlagAssets.list("icons/")?.is_empty());
        Ok(())
    }

    #[test]
    fn codes_normalize_case_and_whitespace() {
        assert_eq!(flag_asset_path("CN"), Some("fluxdown/flags/cn.svg"));
        assert_eq!(flag_asset_path(" us "), Some("fluxdown/flags/us.svg"));
        assert_eq!(flag_asset_path("GB-ENG"), Some("fluxdown/flags/gb-eng.svg"));
    }

    #[test]
    fn unrecognized_codes_have_no_path_and_fall_back_to_unknown_flag() {
        for code in ["", "  ", "ZZ", "中国", "TOOLONGCODE", "c\u{0}"] {
            assert!(flag_asset_path(code).is_none(), "{code:?}");
        }
        assert_eq!(unknown().code, "xx");
        assert!(find("ZZ").is_none());
    }
}
