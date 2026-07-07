use std::path::PathBuf;

/// Raw decoded image data (CPU-side, no GPU texture yet).
/// Can be sent across threads.
pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub size: [usize; 2],
    pub path: PathBuf,
}

/// Decode image bytes into raw RGBA pixels. Safe to call from any thread.
pub fn decode_image_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let rgba = img.into_raw();
    Some(DecodedImage {
        rgba,
        size,
        path: PathBuf::new(),
    })
}

/// Compute average brightness (BT.601 luma) of a rectangular region.
/// `selection` is in normalized coords [x1, y1, x2, y2] (0..1).
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
            sum += 0.299 * rgba[idx] as f64
                + 0.587 * rgba[idx + 1] as f64
                + 0.114 * rgba[idx + 2] as f64;
            count += 1;
        }
    }
    (sum / count as f64) as f32
}

/// Build the label text displayed at the bottom-left of each cell.
/// May contain newlines.
pub fn format_cell_label(y: f32) -> String {
    format!("Avg Y: {:.1}", y)
}
