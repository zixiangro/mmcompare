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
        path: PathBuf::new(), // placeholder, caller fills in
    })
}
