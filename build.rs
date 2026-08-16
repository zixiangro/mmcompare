// 开发便利：Windows 下把 FFMPEG_DIR/bin 的 dll 拷到 target/<profile>/，cargo run/test 无需手动配 PATH。
// CI 的 Windows runner 同样受益；Linux/macOS 由 workflow 设置 LD_LIBRARY_PATH。
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let Ok(ffmpeg_dir) = std::env::var("FFMPEG_DIR") else {
        return;
    };
    let bin = std::path::Path::new(&ffmpeg_dir).join("bin");
    let Ok(entries) = std::fs::read_dir(&bin) else {
        return;
    };
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());
    let out = std::path::Path::new(&target).join(std::env::var("PROFILE").unwrap());
    let _ = std::fs::create_dir_all(&out);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "dll") {
            let _ = std::fs::copy(&path, out.join(path.file_name().unwrap()));
        }
    }
}
