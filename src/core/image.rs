//! 纯像素数据处理：解码、旋转、直方图、选区统计、标签格式化、EXIF。
//! 零 GUI 依赖，可脱离界面单独测试。

use std::path::PathBuf;

pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub size: [usize; 2],
    pub path: PathBuf,
}

pub fn decode_image_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let rgba = img.into_raw();
    if size[0] == 0 || size[1] == 0 || rgba.len() != size[0] * size[1] * 4 {
        return None;
    }
    Some(DecodedImage {
        rgba,
        size,
        path: PathBuf::new(),
    })
}

/// 解码并缩放到方形缩略图（文件夹列表/网格预览用）。
///
/// JPEG 走解码器级降采样（`jpeg_decoder::scale`，因子 1/8·1/4·1/2），
/// 只解码需要的 DCT 块，峰值内存约为全解码的 1/60；其余格式全解码后
/// 用快速缩略算法（链式半采样）。调用方负责设置 `path`。
pub fn decode_thumbnail_bytes(bytes: &[u8], size: u32) -> Option<DecodedImage> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        if let Some(img) = decode_jpeg_thumb(bytes, size) {
            return Some(img);
        }
    }
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.thumbnail(size, size).to_rgba8();
    let out_size = [img.width() as usize, img.height() as usize];
    Some(DecodedImage {
        rgba: img.into_raw(),
        size: out_size,
        path: PathBuf::new(),
    })
}

/// JPEG 解码器级降采样：scale 到接近目标尺寸后快速缩略。
/// 非标准 JPEG / 无法缩放时返回 `None`，由调用方回退全解码。
fn decode_jpeg_thumb(bytes: &[u8], size: u32) -> Option<DecodedImage> {
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_color_transform(jpeg_decoder::ColorTransform::RGB);
    let (w, h) = dec.scale(size as u16, size as u16).ok()?;
    let pixels = dec.decode().ok()?;
    let channels = match dec.info().map(|i| i.pixel_format) {
        Some(jpeg_decoder::PixelFormat::L8) | Some(jpeg_decoder::PixelFormat::L16) => 1,
        _ => 3,
    };
    let rgba: Vec<u8> = match channels {
        1 => pixels.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        _ => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
    };
    if rgba.len() != w as usize * h as usize * 4 {
        return None;
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba)?;
    let img = image::DynamicImage::ImageRgba8(img)
        .thumbnail(size, size)
        .to_rgba8();
    let out_size = [img.width() as usize, img.height() as usize];
    Some(DecodedImage {
        rgba: img.into_raw(),
        size: out_size,
        path: PathBuf::new(),
    })
}

pub fn rotate_rgba_90_cw(rgba: &[u8], w: usize, h: usize) -> (Vec<u8>, [usize; 2]) {
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 4;
            let dst = (x * h + (h - 1 - y)) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    (out, [h, w])
}

const LUMA_R: f32 = 0.299;
const LUMA_G: f32 = 0.587;
const LUMA_B: f32 = 0.114;

fn rgba_to_y(rgba: &[u8], idx: usize) -> f32 {
    LUMA_R * rgba[idx] as f32 + LUMA_G * rgba[idx + 1] as f32 + LUMA_B * rgba[idx + 2] as f32
}

pub fn compute_y_histogram(rgba: &[u8]) -> [u32; 256] {
    let mut hist = [0u32; 256];
    for idx in (0..rgba.len()).step_by(4) {
        let y = rgba_to_y(rgba, idx) as usize;
        hist[y.min(255)] += 1;
    }
    hist
}

#[derive(Clone, Copy)]
pub struct AvgStats {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

pub fn compute_selection_stats(rgba: &[u8], w: usize, h: usize, selection: &[f32; 4]) -> AvgStats {
    let x1 = (selection[0] * w as f32) as usize;
    let y1 = (selection[1] * h as f32) as usize;
    let x2 = ((selection[2] * w as f32) as usize).min(w - 1);
    let y2 = ((selection[3] * h as f32) as usize).min(h - 1);
    if x1 >= x2 || y1 >= y2 {
        return AvgStats {
            r: 0.0,
            g: 0.0,
            b: 0.0,
        };
    }
    let mut r_sum = 0u64;
    let mut g_sum = 0u64;
    let mut b_sum = 0u64;
    let mut count = 0u64;
    for y in y1..=y2 {
        for x in x1..=x2 {
            let idx = (y * w + x) * 4;
            r_sum += rgba[idx] as u64;
            g_sum += rgba[idx + 1] as u64;
            b_sum += rgba[idx + 2] as u64;
            count += 1;
        }
    }
    AvgStats {
        r: r_sum as f32 / count as f32 / 255.0,
        g: g_sum as f32 / count as f32 / 255.0,
        b: b_sum as f32 / count as f32 / 255.0,
    }
}

pub fn format_cell_label(s: &AvgStats) -> String {
    let luma = LUMA_R * s.r + LUMA_G * s.g + LUMA_B * s.b;
    let rg = if s.g > 0.0 { s.r / s.g } else { 0.0 };
    let bg = if s.g > 0.0 { s.b / s.g } else { 0.0 };
    let max = s.r.max(s.g).max(s.b);
    let min = s.r.min(s.g).min(s.b);
    let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
    format!(
        "Luma:{:.2}\nR/G:{:.2} B/G:{:.2}\nSat:{:.2}\nR:{:.2} G:{:.2} B:{:.2}",
        luma, rg, bg, sat, s.r, s.g, s.b
    )
}

fn exif_f64_or_urational(v: &nom_exif::EntryValue) -> Option<f64> {
    v.as_f64().or_else(|| {
        v.as_urational()
            .map(|r| r.numerator() as f64 / r.denominator() as f64)
    })
}

pub fn extract_exif(bytes: &[u8]) -> String {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ms = nom_exif::MediaSource::from_memory(bytes.to_vec()).ok()?;
        let mut parser = nom_exif::MediaParser::new();
        let exif = parser.parse_exif(ms).ok()?;
        let exif = nom_exif::Exif::from(exif);
        let get = |tag| exif.get(tag);

        let mut lines = Vec::new();

        if let Some(v) = get(nom_exif::ExifTag::Make).and_then(|v| v.as_str()) {
            let model = get(nom_exif::ExifTag::Model)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if model.is_empty() {
                lines.push(v.to_string());
            } else {
                lines.push(format!("{} {}", v, model));
            }
        } else if let Some(v) = get(nom_exif::ExifTag::Model).and_then(|v| v.as_str()) {
            lines.push(v.to_string());
        }

        if let Some(v) = get(nom_exif::ExifTag::LensModel).and_then(|v| v.as_str())
            && !v.is_empty()
        {
            lines.push(v.to_string());
        }

        if let Some(v) = get(nom_exif::ExifTag::FNumber).and_then(exif_f64_or_urational) {
            lines.push(format!("f/{:.1}", v));
        }

        if let Some(v) = get(nom_exif::ExifTag::ExposureTime).and_then(exif_f64_or_urational) {
            if v < 1.0 {
                lines.push(format!("1/{}s", (1.0 / v).round() as u32));
            } else {
                lines.push(format!("{:.0}s", v));
            }
        }

        if let Some(v) = get(nom_exif::ExifTag::ISOSpeedRatings).and_then(|v| v.as_u32()) {
            lines.push(format!("ISO {}", v));
        } else if let Some(v) = get(nom_exif::ExifTag::ISOSpeedRatings).and_then(|v| v.as_u16()) {
            lines.push(format!("ISO {}", v));
        } else if let Some(v) = get(nom_exif::ExifTag::ISOSpeedRatings).and_then(|v| v.as_str()) {
            lines.push(format!("ISO {}", v));
        }

        if let Some(v) = get(nom_exif::ExifTag::Flash).and_then(|v| v.as_u16()) {
            lines.push(format!("Flash: {}", if v & 1 != 0 { "On" } else { "Off" }));
        } else if let Some(v) = get(nom_exif::ExifTag::Flash).and_then(|v| v.as_str()) {
            lines.push(format!("Flash: {}", v));
        }

        Some(lines.join("\n"))
    }));
    result.unwrap_or_default().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_jpeg(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(
                    x,
                    y,
                    image::Rgba([(x % 255) as u8, (y % 255) as u8, 128, 255]),
                );
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        out.into_inner()
    }

    fn make_png(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(
                    x,
                    y,
                    image::Rgba([(x % 255) as u8, (y % 255) as u8, 128, 255]),
                );
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn jpeg_thumbnail_is_64x64() {
        let bytes = make_jpeg(4000, 3000);
        let thumb = decode_thumbnail_bytes(&bytes, 64).expect("jpeg thumb decode");
        assert_eq!(thumb.size[0].max(thumb.size[1]), 64);
        assert_eq!(thumb.rgba.len(), thumb.size[0] * thumb.size[1] * 4);
    }

    #[test]
    fn png_thumbnail_is_64x64() {
        let bytes = make_png(4000, 3000);
        let thumb = decode_thumbnail_bytes(&bytes, 64).expect("png thumb decode");
        assert_eq!(thumb.size[0].max(thumb.size[1]), 64);
        assert_eq!(thumb.rgba.len(), thumb.size[0] * thumb.size[1] * 4);
    }

    #[test]
    fn small_image_thumbnail_ok() {
        let bytes = make_jpeg(100, 80);
        let thumb = decode_thumbnail_bytes(&bytes, 64).expect("small jpeg thumb decode");
        assert_eq!(thumb.size[0].max(thumb.size[1]), 64);
    }

    #[test]
    fn grayscale_jpeg_thumbnail_ok() {
        let mut img = image::GrayImage::new(2000, 1500);
        for y in 0..1500 {
            for x in 0..2000 {
                img.put_pixel(x, y, image::Luma([(x % 255) as u8]));
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        let thumb = decode_thumbnail_bytes(&out.into_inner(), 64).expect("gray jpeg thumb decode");
        assert_eq!(thumb.size[0].max(thumb.size[1]), 64);
    }

    #[test]
    fn corrupt_bytes_fallback_to_none() {
        assert!(decode_thumbnail_bytes(b"not an image at all", 64).is_none());
    }
}
