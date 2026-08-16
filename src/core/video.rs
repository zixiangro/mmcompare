//! 视频解码封装（M1 spike）：信息读取与首帧提取。零 GUI 依赖，纯数据。
//! 只初始化视频流（D5：不做音频）。解码链路：输入 → 视频流 → 解码器 → swscale 转 RGB24。
// M1 spike 阶段尚未被 UI 调用（测试已覆盖）；M2 接入视频 cell 后移除。
#![allow(dead_code)]

use std::path::Path;
use std::sync::Once;

use ffmpeg_next as ffmpeg;

static INIT: Once = Once::new();

fn init() {
    INIT.call_once(|| {
        ffmpeg::init().expect("ffmpeg init failed");
    });
}

pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub frame_rate: f64,
}

struct Opened {
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    context: ffmpeg::codec::context::Context,
    duration_secs: f64,
    frame_rate: f64,
}

fn open_video(path: &Path) -> Result<Opened, ffmpeg::Error> {
    init();
    let ictx = ffmpeg::format::input(path)?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(ffmpeg::Error::StreamNotFound)?;
    let (stream_index, duration, frame_rate) = {
        let ffmpeg::Rational(num, den) = stream.avg_frame_rate();
        (
            stream.index(),
            ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE),
            if den > 0 {
                num as f64 / f64::from(den)
            } else {
                0.0
            },
        )
    };
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    Ok(Opened {
        ictx,
        stream_index,
        context,
        duration_secs: duration,
        frame_rate,
    })
}

pub fn read_info(path: &Path) -> Result<VideoInfo, ffmpeg::Error> {
    let opened = open_video(path)?;
    let decoder = opened.context.decoder().video()?;
    Ok(VideoInfo {
        width: decoder.width(),
        height: decoder.height(),
        duration_secs: opened.duration_secs,
        frame_rate: opened.frame_rate,
    })
}

/// 提取首帧，转 RGB24（紧密布局，无 padding），最长边限制为 max_dim（0 表示原始尺寸）。
pub fn first_frame(path: &Path, max_dim: u32) -> Result<(VideoInfo, Vec<u8>), ffmpeg::Error> {
    let mut opened = open_video(path)?;
    let mut decoder = opened.context.decoder().video()?;

    let (src_w, src_h) = (decoder.width(), decoder.height());
    let scale = match max_dim {
        0 => 1.0,
        m => f64::from(m) / f64::from(src_w.max(src_h)),
    };
    let (dst_w, dst_h) = (
        ((src_w as f64 * scale).round() as u32).max(1),
        ((src_h as f64 * scale).round() as u32).max(1),
    );

    let mut scaler = ffmpeg::software::scaling::context::Context::get(
        decoder.format(),
        src_w,
        src_h,
        ffmpeg::format::Pixel::RGB24,
        dst_w,
        dst_h,
        ffmpeg::software::scaling::flag::Flags::BILINEAR,
    )?;

    let info = VideoInfo {
        width: dst_w,
        height: dst_h,
        duration_secs: opened.duration_secs,
        frame_rate: opened.frame_rate,
    };

    for (stream, packet) in opened.ictx.packets() {
        if stream.index() != opened.stream_index {
            continue;
        }
        decoder.send_packet(&packet)?;
        let mut decoded = ffmpeg::util::frame::video::Video::empty();
        if decoder.receive_frame(&mut decoded).is_ok() {
            let mut rgb = ffmpeg::util::frame::video::Video::empty();
            scaler.run(&decoded, &mut rgb)?;
            let mut buf = Vec::with_capacity((dst_w * dst_h * 3) as usize);
            for i in 0..dst_h as usize {
                let row = &rgb.data(0)[i * rgb.stride(0)..i * rgb.stride(0) + dst_w as usize * 3];
                buf.extend_from_slice(row);
            }
            return Ok((info, buf));
        }
    }
    Err(ffmpeg::Error::Eof)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "tests/fixtures/sample.mp4";

    #[test]
    fn reads_info_from_sample() {
        let info = read_info(Path::new(SAMPLE)).expect("read_info failed");
        assert_eq!(info.width, 320);
        assert_eq!(info.height, 240);
        assert!(info.duration_secs >= 1.9 && info.duration_secs <= 2.1);
        assert!(info.frame_rate > 29.0 && info.frame_rate < 31.0);
    }

    #[test]
    fn extracts_first_frame() {
        let (info, buf) = first_frame(Path::new(SAMPLE), 0).expect("first_frame failed");
        assert_eq!(info.width, 320);
        assert_eq!(info.height, 240);
        assert_eq!(buf.len(), 320 * 240 * 3);
        assert!(buf.iter().any(|&b| b != 0), "frame should not be all black");
    }

    #[test]
    fn extracts_scaled_first_frame() {
        let (info, buf) = first_frame(Path::new(SAMPLE), 160).expect("first_frame failed");
        assert_eq!(info.width, 160);
        assert_eq!(info.height, 120);
        assert_eq!(buf.len(), 160 * 120 * 3);
    }

    #[test]
    fn missing_file_errors() {
        assert!(read_info(Path::new("tests/fixtures/nonexistent.mp4")).is_err());
        assert!(first_frame(Path::new("tests/fixtures/nonexistent.mp4"), 0).is_err());
    }
}
