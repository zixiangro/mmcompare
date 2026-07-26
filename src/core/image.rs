use std::path::PathBuf;

pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub size: [usize; 2],
    pub path: PathBuf,
    pub raw_bytes: Vec<u8>,
}

// ── Decode ────────────────────────────────────────────

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
        raw_bytes: Vec::new(),
    })
}

// ── Rotation (pure pixel op) ──────────────────────────

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

// ── Y helpers ─────────────────────────────────────────

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

// ── Selection stats ───────────────────────────────────

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

// ── EXIF ──────────────────────────────────────────────

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

        // Camera
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

        // Lens
        if let Some(v) = get(nom_exif::ExifTag::LensModel).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                lines.push(v.to_string());
            }
        }

        // Aperture
        if let Some(v) = get(nom_exif::ExifTag::FNumber).and_then(|v| exif_f64_or_urational(v)) {
            lines.push(format!("f/{:.1}", v));
        }

        // Shutter
        if let Some(v) = get(nom_exif::ExifTag::ExposureTime).and_then(|v| exif_f64_or_urational(v))
        {
            if v < 1.0 {
                lines.push(format!("1/{}s", (1.0 / v).round() as u32));
            } else {
                lines.push(format!("{:.0}s", v));
            }
        }

        // ISO
        if let Some(v) = get(nom_exif::ExifTag::ISOSpeedRatings).and_then(|v| v.as_u32()) {
            lines.push(format!("ISO {}", v));
        } else if let Some(v) = get(nom_exif::ExifTag::ISOSpeedRatings).and_then(|v| v.as_u16()) {
            lines.push(format!("ISO {}", v));
        } else if let Some(v) = get(nom_exif::ExifTag::ISOSpeedRatings).and_then(|v| v.as_str()) {
            lines.push(format!("ISO {}", v));
        }

        // Flash
        if let Some(v) = get(nom_exif::ExifTag::Flash).and_then(|v| v.as_u16()) {
            lines.push(format!("Flash: {}", if v & 1 != 0 { "On" } else { "Off" }));
        } else if let Some(v) = get(nom_exif::ExifTag::Flash).and_then(|v| v.as_str()) {
            lines.push(format!("Flash: {}", v));
        }

        Some(lines.join("\n"))
    }));
    result.unwrap_or_default().unwrap_or_default()
}
