use std::path::PathBuf;

pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub size: [usize; 2],
    pub path: PathBuf,
    pub raw_bytes: Vec<u8>,
}

pub fn decode_image_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let rgba = img.into_raw();
    Some(DecodedImage {
        rgba,
        size,
        path: PathBuf::new(),
        raw_bytes: Vec::new(),
    })
}

pub fn extract_exif(bytes: &[u8]) -> String {
    let ms = match nom_exif::MediaSource::from_memory(bytes.to_vec()) {
        Ok(ms) => ms,
        Err(_) => return String::new(),
    };
    let mut parser = nom_exif::MediaParser::new();
    let exif = match parser.parse_exif(ms) {
        Ok(exif) => nom_exif::Exif::from(exif),
        Err(_) => return String::new(),
    };
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
    if let Some(v) = get(nom_exif::ExifTag::FNumber).and_then(|v| v.as_f64()) {
        lines.push(format!("f/{:.1}", v));
    } else if let Some(v) = get(nom_exif::ExifTag::FNumber).and_then(|v| v.as_urational()) {
        let f = v.numerator() as f64 / v.denominator() as f64;
        lines.push(format!("f/{:.1}", f));
    }

    // Shutter
    if let Some(v) = get(nom_exif::ExifTag::ExposureTime).and_then(|v| v.as_f64()) {
        if v < 1.0 {
            lines.push(format!("1/{}s", (1.0 / v).round() as u32));
        } else {
            lines.push(format!("{:.0}s", v));
        }
    } else if let Some(v) = get(nom_exif::ExifTag::ExposureTime).and_then(|v| v.as_urational()) {
        let f = v.numerator() as f64 / v.denominator() as f64;
        if f < 1.0 {
            lines.push(format!("1/{}s", (1.0_f64 / f).round() as u32));
        } else {
            lines.push(format!("{:.0}s", f));
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

    lines.join("\n")
}

fn rgba_to_y(rgba: &[u8], idx: usize) -> f32 {
    0.299 * rgba[idx] as f32 + 0.587 * rgba[idx + 1] as f32 + 0.114 * rgba[idx + 2] as f32
}

pub fn compute_y_histogram(rgba: &[u8]) -> [u32; 256] {
    let mut hist = [0u32; 256];
    for idx in (0..rgba.len()).step_by(4) {
        let y = rgba_to_y(rgba, idx) as usize;
        hist[y.min(255)] += 1;
    }
    hist
}

pub fn compute_avg_y(rgba: &[u8], w: usize, h: usize, selection: &[f32; 4]) -> f32 {
    let x1 = (selection[0] * w as f32) as usize;
    let y1 = (selection[1] * h as f32) as usize;
    let x2 = ((selection[2] * w as f32) as usize).min(w - 1);
    let y2 = ((selection[3] * h as f32) as usize).min(h - 1);
    if x1 >= x2 || y1 >= y2 {
        return 0.0;
    }
    let mut sum = 0f64;
    let mut count = 0u64;
    for y in y1..=y2 {
        for x in x1..=x2 {
            let idx = (y * w + x) * 4;
            sum += rgba_to_y(rgba, idx) as f64;
            count += 1;
        }
    }
    (sum / count as f64) as f32
}

pub fn format_cell_label(y: f32) -> String {
    format!("Avg Y: {:.1}", y)
}
