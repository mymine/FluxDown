//! 应用图标图像管线（纯函数，无 OS 依赖）：读取用户图片 → 规范化为 1024² 透明底母版 PNG →
//! 派生各平台需要的尺寸（缩放 PNG、Windows 多尺寸 ICO、X11 ARGB 数组）。

use std::io::{Cursor, Read};
use std::path::Path;

use fluxdown_protocol::app_icon::{APP_ICON_IMAGE_MAX_SIZE, APP_ICON_IMAGE_MIN_SIZE};
#[cfg(any(windows, test))]
use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::{self, FilterType};
use image::{ColorType, ImageEncoder, ImageReader, Limits, RgbaImage};

/// 输入文件大小上限。
pub(crate) const MAX_SOURCE_BYTES: u64 = 20 * 1024 * 1024;
/// 输入图片的边长上限（像素）。
pub(crate) const MAX_SOURCE_DIMENSION: u32 = 8192;
/// 解码时允许的最大内存分配（8192² RGBA = 256 MiB，留出解码器的中间缓冲）。
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;
/// 母版边长。
pub(crate) const MASTER_SIZE: u32 = APP_ICON_IMAGE_MAX_SIZE;
/// 修订号的十六进制位数。
const REVISION_LEN: usize = 12;
/// Windows ICO 的尺寸序列（覆盖 100%–400% 缩放下任务栏 / 标题栏 / Alt-Tab 的取值）。
#[cfg(any(windows, test))]
pub(crate) const ICO_SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

#[derive(Debug, thiserror::Error)]
pub(crate) enum ImageError {
    #[error("image path must be absolute")]
    NotAbsolute,
    #[error("image path is not a regular file")]
    NotAFile,
    #[error("image file exceeds {} MiB", MAX_SOURCE_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("image file is empty or has zero dimensions")]
    Empty,
    #[error("could not read image: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not decode image: {0}")]
    Image(#[from] image::ImageError),
}

/// 规范化后的母版：PNG 字节 + 内容修订号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Master {
    pub png: Vec<u8>,
    pub revision: String,
}

/// 读取用户给的图片文件：必须是绝对路径下不超过 [`MAX_SOURCE_BYTES`] 的普通文件。
pub(crate) fn read_source(path: &Path) -> Result<Vec<u8>, ImageError> {
    if !path.is_absolute() {
        return Err(ImageError::NotAbsolute);
    }
    // 先看元数据再打开：FIFO / 设备文件的 `open` 本身就可能一直阻塞。
    if !std::fs::metadata(path)?.is_file() {
        return Err(ImageError::NotAFile);
    }
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(ImageError::NotAFile);
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(ImageError::TooLarge);
    }
    // 元数据可能过期（文件被追加）：读取本身也按上限 +1 截断判定。
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(ImageError::TooLarge);
    }
    Ok(bytes)
}

/// 按内容猜格式并解码（png / jpeg / webp / gif 首帧 / bmp / ico），带尺寸与内存上限。
pub(crate) fn decode_limited(bytes: &[u8]) -> Result<RgbaImage, ImageError> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    let decoded = reader.decode()?.into_rgba8();
    if decoded.width() == 0 || decoded.height() == 0 {
        return Err(ImageError::Empty);
    }
    Ok(decoded)
}

/// 等比 contain 到 [`MASTER_SIZE`]² 透明画布并居中（Lanczos3；小图同样放大到贴边）。
pub(crate) fn normalize(source: &RgbaImage) -> RgbaImage {
    let (width, height) = source.dimensions();
    let longest = width.max(height).max(1);
    let scaled = |side: u32| {
        let value = (u64::from(side) * u64::from(MASTER_SIZE) + u64::from(longest) / 2)
            / u64::from(longest);
        u32::try_from(value)
            .unwrap_or(MASTER_SIZE)
            .clamp(1, MASTER_SIZE)
    };
    let (target_width, target_height) = (scaled(width), scaled(height));
    let fitted = if (target_width, target_height) == (width, height) {
        source.clone()
    } else {
        imageops::resize(source, target_width, target_height, FilterType::Lanczos3)
    };
    let mut canvas = RgbaImage::new(MASTER_SIZE, MASTER_SIZE);
    imageops::overlay(
        &mut canvas,
        &fitted,
        i64::from((MASTER_SIZE - target_width) / 2),
        i64::from((MASTER_SIZE - target_height) / 2),
    );
    canvas
}

pub(crate) fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        ColorType::Rgba8.into(),
    )?;
    Ok(out)
}

/// PNG 字节的内容修订号：BLAKE3 前 [`REVISION_LEN`] 位十六进制。
pub(crate) fn revision_of(png: &[u8]) -> String {
    let mut revision = blake3::hash(png).to_hex().to_string();
    revision.truncate(REVISION_LEN);
    revision
}

/// 把用户图片字节规范化成母版。
pub(crate) fn build_master(source_bytes: &[u8]) -> Result<Master, ImageError> {
    let normalized = normalize(&decode_limited(source_bytes)?);
    let png = encode_png(&normalized)?;
    let revision = revision_of(&png);
    Ok(Master { png, revision })
}

/// 解码已落盘的母版 PNG。
pub(crate) fn decode_master(png: &[u8]) -> Result<RgbaImage, ImageError> {
    decode_limited(png)
}

/// 缩放到 `size`×`size`（方形输入）。
pub(crate) fn resize_square(image: &RgbaImage, size: u32) -> RgbaImage {
    if image.width() == size && image.height() == size {
        return image.clone();
    }
    imageops::resize(image, size, size, FilterType::Lanczos3)
}

/// `agent.platform.appIconImage` 的尺寸钳制。
pub(crate) fn clamp_request_size(size: u32) -> u32 {
    size.clamp(APP_ICON_IMAGE_MIN_SIZE, APP_ICON_IMAGE_MAX_SIZE)
}

/// 缩放后的 PNG 字节。
pub(crate) fn resized_png(image: &RgbaImage, size: u32) -> Result<Vec<u8>, ImageError> {
    encode_png(&resize_square(image, size))
}

/// Apple HIG：图标形状占画布的 824/1024，四周留透明边距（同 `crates/app/src/app_icon.rs` 与
/// `scripts/gen_icons.ts::renderMacAppIcon`）。
#[cfg(any(target_os = "macos", test))]
const HIG_CONTENT: u32 = 824;
#[cfg(any(target_os = "macos", test))]
const HIG_CANVAS: u32 = 1024;

/// 把方形图片按 Apple HIG 内缩到 `canvas`×`canvas` 画布的 824/1024 并居中，四周透明（Finder 自定义
/// 图标不带系统的圆角蒙版与边距，缺了内缩会比别的应用图标大一圈）。
#[cfg(any(target_os = "macos", test))]
pub(crate) fn compose_hig_icon(source: &RgbaImage, canvas: u32) -> RgbaImage {
    let content = (canvas * HIG_CONTENT + HIG_CANVAS / 2) / HIG_CANVAS;
    let resized = resize_square(source, content);
    let mut out = RgbaImage::new(canvas, canvas);
    let offset = i64::from((canvas - content) / 2);
    imageops::overlay(&mut out, &resized, offset, offset);
    out
}

#[cfg(any(windows, test))]
/// 多尺寸 ICO（PNG 压缩条目）。
pub(crate) fn build_ico(image: &RgbaImage) -> Result<Vec<u8>, ImageError> {
    let frames = ICO_SIZES
        .iter()
        .map(|&size| {
            let resized = resize_square(image, size);
            IcoFrame::as_png(resized.as_raw(), size, size, ColorType::Rgba8.into())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    IcoEncoder::new(&mut out).encode_images(&frames)?;
    Ok(out)
}

#[cfg(any(target_os = "linux", test))]
/// X11 `_NET_WM_ICON` 单个图标：`[width, height, ARGB…]`（CARDINAL/32，预乘无关，按 EWMH 为非预乘 ARGB）。
pub(crate) fn argb_cardinals(image: &RgbaImage, size: u32) -> Vec<u32> {
    let resized = resize_square(image, size);
    let mut out = Vec::with_capacity(2 + (size * size) as usize);
    out.push(size);
    out.push(size);
    out.extend(resized.pixels().map(|pixel| {
        let [r, g, b, a] = pixel.0;
        u32::from(a) << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
    }));
    out
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    #[test]
    fn non_square_image_is_contained_with_transparent_padding() {
        let normalized = normalize(&solid(200, 100, [255, 0, 0, 255]));

        assert_eq!(normalized.dimensions(), (MASTER_SIZE, MASTER_SIZE));
        // 2:1 的图缩放成 1024×512，上下各留 256 的透明边。
        assert_eq!(normalized.get_pixel(512, 0).0[3], 0);
        assert_eq!(normalized.get_pixel(512, MASTER_SIZE - 1).0[3], 0);
        assert_eq!(normalized.get_pixel(512, 255).0[3], 0);
        assert_eq!(normalized.get_pixel(512, 256).0, [255, 0, 0, 255]);
        assert_eq!(normalized.get_pixel(512, 767).0, [255, 0, 0, 255]);
        assert_eq!(normalized.get_pixel(512, 768).0[3], 0);
        assert_eq!(normalized.get_pixel(0, 512).0, [255, 0, 0, 255]);
        assert_eq!(
            normalized.get_pixel(MASTER_SIZE - 1, 512).0,
            [255, 0, 0, 255]
        );
    }

    #[test]
    fn tall_image_is_padded_on_the_sides() {
        let normalized = normalize(&solid(50, 100, [0, 0, 255, 255]));

        assert_eq!(normalized.get_pixel(255, 512).0[3], 0);
        assert_eq!(normalized.get_pixel(256, 512).0, [0, 0, 255, 255]);
        assert_eq!(normalized.get_pixel(767, 512).0, [0, 0, 255, 255]);
        assert_eq!(normalized.get_pixel(768, 512).0[3], 0);
    }

    #[test]
    fn extreme_aspect_ratio_never_collapses_to_zero_size() {
        let normalized = normalize(&solid(8192, 1, [9, 9, 9, 255]));

        assert_eq!(normalized.dimensions(), (MASTER_SIZE, MASTER_SIZE));
        assert!(normalized.pixels().any(|pixel| pixel.0[3] > 0));
    }

    #[test]
    fn master_revision_is_stable_and_tracks_content() {
        let red = encode_png(&solid(8, 8, [255, 0, 0, 255])).expect("encode red");
        let blue = encode_png(&solid(8, 8, [0, 0, 255, 255])).expect("encode blue");

        let first = build_master(&red).expect("master");
        let again = build_master(&red).expect("master again");
        let other = build_master(&blue).expect("other master");

        assert_eq!(first, again);
        assert_eq!(first.revision.len(), REVISION_LEN);
        assert!(first.revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first.revision, other.revision);
        assert_eq!(revision_of(&first.png), first.revision);
        let decoded = decode_master(&first.png).expect("master decodes");
        assert_eq!(decoded.dimensions(), (MASTER_SIZE, MASTER_SIZE));
    }

    #[test]
    fn ico_contains_every_requested_frame_size() {
        let master = normalize(&solid(64, 64, [10, 200, 30, 255]));

        let ico = build_ico(&master).expect("ico");
        let decoded_entries = u16::from_le_bytes([ico[4], ico[5]]);

        assert_eq!(&ico[0..4], &[0, 0, 1, 0], "ICONDIR header");
        assert_eq!(usize::from(decoded_entries), ICO_SIZES.len());
        let mut sizes = (0..usize::from(decoded_entries))
            .map(|index| {
                // ICONDIRENTRY 16 字节；宽度字节 0 表示 256。
                let width = ico[6 + index * 16];
                if width == 0 { 256 } else { u32::from(width) }
            })
            .collect::<Vec<_>>();
        sizes.sort_unstable();
        assert_eq!(sizes, ICO_SIZES.to_vec());
        // 整体仍可被通用解码器读回（取到最大帧）。
        let round_trip = decode_limited(&ico).expect("ico decodes");
        assert_eq!(round_trip.dimensions(), (256, 256));
    }

    #[test]
    fn oversized_dimensions_are_rejected_by_decode_limits() {
        let too_wide = encode_png(&solid(MAX_SOURCE_DIMENSION + 1, 1, [0, 0, 0, 255]))
            .expect("encode wide image");

        assert!(matches!(
            decode_limited(&too_wide),
            Err(ImageError::Image(_))
        ));
    }

    #[test]
    fn garbage_bytes_are_not_an_image() {
        assert!(decode_limited(b"definitely not an image").is_err());
    }

    #[test]
    fn read_source_requires_absolute_regular_files_within_the_limit() {
        let dir = tempfile_dir("read_source");
        let file = dir.join("icon.png");
        std::fs::write(&file, b"abc").expect("write file");

        assert!(matches!(
            read_source(Path::new("relative.png")),
            Err(ImageError::NotAbsolute)
        ));
        assert!(matches!(read_source(&dir), Err(ImageError::NotAFile)));
        assert_eq!(read_source(&file).expect("read"), b"abc");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn argb_cardinals_pack_non_premultiplied_channels() {
        let image = solid(4, 4, [0x11, 0x22, 0x33, 0x80]);

        let packed = argb_cardinals(&image, 4);

        assert_eq!(&packed[..2], &[4, 4]);
        assert_eq!(packed.len(), 2 + 16);
        assert!(packed[2..].iter().all(|&pixel| pixel == 0x8011_2233));
    }

    #[test]
    fn hig_icon_insets_content_to_824_of_1024_and_centres_it() {
        let composed = compose_hig_icon(&solid(1024, 1024, [9, 99, 199, 255]), 1024);

        assert_eq!(composed.dimensions(), (1024, 1024));
        // 内容边长 824，四周各留 100。
        assert_eq!(composed.get_pixel(99, 512).0[3], 0);
        assert_eq!(composed.get_pixel(100, 512).0, [9, 99, 199, 255]);
        assert_eq!(composed.get_pixel(923, 512).0, [9, 99, 199, 255]);
        assert_eq!(composed.get_pixel(924, 512).0[3], 0);
        assert_eq!(composed.get_pixel(512, 99).0[3], 0);
        assert_eq!(composed.get_pixel(512, 924).0[3], 0);
        assert_eq!(composed.get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn hig_icon_rounds_content_like_the_desktop_process_for_smaller_canvases() {
        // round(512 × 824/1024) = 412，四周各留 50（与 `crates/app/src/app_icon.rs` 的测试同值）。
        let composed = compose_hig_icon(&solid(1024, 1024, [255, 0, 0, 255]), 512);

        assert_eq!(composed.dimensions(), (512, 512));
        assert_eq!(composed.get_pixel(49, 256).0[3], 0);
        assert_eq!(composed.get_pixel(50, 256).0[3], 255);
        assert_eq!(composed.get_pixel(461, 256).0[3], 255);
        assert_eq!(composed.get_pixel(462, 256).0[3], 0);
    }

    #[test]
    fn request_size_is_clamped_to_protocol_range() {
        assert_eq!(clamp_request_size(1), APP_ICON_IMAGE_MIN_SIZE);
        assert_eq!(clamp_request_size(4096), APP_ICON_IMAGE_MAX_SIZE);
        assert_eq!(clamp_request_size(128), 128);
    }

    fn tempfile_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fluxdown-app-icon-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
