//! 视频解码封装：信息读取、首帧提取、按需 seek 解码与连续播放解码。
//! 零 GUI 依赖，纯数据。只初始化视频流（D5：不做音频）。
//! 解码链路：输入 → 视频流 → 解码器 → swscale 转 RGB24（可降采样）。

use std::path::Path;
use std::sync::Once;

use ffmpeg_next as ffmpeg;

static INIT: Once = Once::new();

fn init() {
    INIT.call_once(|| {
        ffmpeg::init().expect("ffmpeg init failed");
    });
}

#[derive(Clone)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub frame_rate: f64,
    /// 容器旋转元数据（0/90/180/270，顺时针；无元数据 = 0）。
    /// 解码帧已按此值旋转，width/height 是旋转后的显示尺寸。
    pub rotation: i32,
}

pub struct DecodedFrame {
    pub pts_secs: f64,
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 视频解码会话：持有一个打开的输入，支持随机 seek 与连续解码。
/// 由 imlayout 的解码线程使用（ADR-0001：线程原语物理隔离在加载方法组）。
pub struct VideoDecoder {
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    parameters: ffmpeg::codec::Parameters,
    time_base: f64,
    info: VideoInfo,
    dst_w: u32,
    dst_h: u32,
    rotation: i32,
    eof: bool,
}

/// 读取流的显示旋转（度）：display matrix 的 (b, a) 反正切，量化到 90° 步长。
/// 无元数据返回 0。矩阵值为主机字节序 i32（16.16 定点）。
fn stream_rotation(stream: &ffmpeg::format::stream::Stream) -> i32 {
    use ffmpeg::codec::packet::side_data::Type;
    for sd in stream.side_data() {
        if sd.kind() == Type::DisplayMatrix {
            let d = sd.data();
            if d.len() >= 36 {
                let m: Vec<i32> = d[..36]
                    .chunks_exact(4)
                    .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                let (a, b) = (m[0] as f64 / 65536.0, m[1] as f64 / 65536.0);
                let deg = b.atan2(a).to_degrees().round() as i32;
                return ((deg + 45) / 90 * 90).rem_euclid(360);
            }
        }
    }
    0
}

/// RGB24 顺时针旋转 90°：原 (x,y) → 新 (h-1-y, x)，尺寸 w×h → h×w。
fn rotate_rgb_90_cw(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; rgb.len()];
    for y in 0..h {
        for x in 0..w {
            let src = ((y * w + x) * 3) as usize;
            let dst = (((h - 1 - y) + x * h) * 3) as usize;
            out[dst..dst + 3].copy_from_slice(&rgb[src..src + 3]);
        }
    }
    out
}

/// RGB24 逆时针旋转 90°：原 (x,y) → 新 (y, w-1-x)。
fn rotate_rgb_90_ccw(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; rgb.len()];
    for y in 0..h {
        for x in 0..w {
            let src = ((y * w + x) * 3) as usize;
            let dst = ((y + (w - 1 - x) * h) * 3) as usize;
            out[dst..dst + 3].copy_from_slice(&rgb[src..src + 3]);
        }
    }
    out
}

/// RGB24 旋转 180°：原 (x,y) → 新 (w-1-x, h-1-y)。
fn rotate_rgb_180(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; rgb.len()];
    for y in 0..h {
        for x in 0..w {
            let src = ((y * w + x) * 3) as usize;
            let dst = (((h - 1 - y) * w + (w - 1 - x)) * 3) as usize;
            out[dst..dst + 3].copy_from_slice(&rgb[src..src + 3]);
        }
    }
    out
}

impl VideoDecoder {
    pub fn open(path: &Path, max_dim: u32) -> Result<Self, ffmpeg::Error> {
        init();
        let ictx = ffmpeg::format::input(path)?;
        let stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;
        let rotation = stream_rotation(&stream);
        let (stream_index, time_base, duration, frame_rate, parameters) = {
            let ffmpeg::Rational(tb_num, tb_den) = stream.time_base();
            let ffmpeg::Rational(fr_num, fr_den) = stream.avg_frame_rate();
            (
                stream.index(),
                if tb_den > 0 {
                    f64::from(tb_num) / f64::from(tb_den)
                } else {
                    0.0
                },
                ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE),
                if fr_den > 0 {
                    f64::from(fr_num) / f64::from(fr_den)
                } else {
                    0.0
                },
                stream.parameters().clone(),
            )
        };
        let decoder = ffmpeg::codec::context::Context::from_parameters(parameters.clone())?
            .decoder()
            .video()?;
        let (src_w, src_h) = (decoder.width(), decoder.height());
        let scale = match max_dim {
            0 => 1.0,
            m => f64::from(m) / f64::from(src_w.max(src_h)),
        };
        let (dst_w, dst_h) = (
            ((src_w as f64 * scale).round() as u32).max(1),
            ((src_h as f64 * scale).round() as u32).max(1),
        );
        // 旋转 90/270 时显示尺寸交换（解码帧在输出后旋转，缩放按原始方向计算）
        let (dst_w, dst_h) = if rotation % 180 == 90 {
            (dst_h, dst_w)
        } else {
            (dst_w, dst_h)
        };
        Ok(Self {
            ictx,
            stream_index,
            parameters,
            time_base,
            info: VideoInfo {
                width: dst_w,
                height: dst_h,
                duration_secs: duration,
                frame_rate,
                rotation,
            },
            dst_w,
            dst_h,
            rotation,
            eof: false,
        })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// 单帧时长（秒），帧率未知时按 30fps 兜底。
    pub fn frame_duration(&self) -> f64 {
        if self.info.frame_rate > 0.0 {
            1.0 / self.info.frame_rate
        } else {
            1.0 / 30.0
        }
    }

    /// 从最近关键帧 seek 到 target 并返回**pts >= target 的第一帧**（暂停时 seek/帧步进用）。
    pub fn seek_frame(&mut self, target_secs: f64) -> Result<Option<DecodedFrame>, ffmpeg::Error> {
        self.seek_to(target_secs)?;
        let mut out = None;
        self.decode(&mut |frame| {
            if frame.pts_secs >= target_secs {
                out = Some(frame);
                false
            } else {
                true
            }
        })?;
        Ok(out)
    }

    /// 从 from_secs 开始连续解码，每帧调用回调；回调返回 false 停止，EOF 自然结束。
    /// 播放循环在解码线程内一次性调用（解码器状态跨帧保持）。
    pub fn play(
        &mut self,
        from_secs: f64,
        mut on_frame: impl FnMut(DecodedFrame) -> bool,
    ) -> Result<(), ffmpeg::Error> {
        self.seek_to(from_secs)?;
        self.decode(&mut on_frame)
    }

    fn seek_to(&mut self, target_secs: f64) -> Result<(), ffmpeg::Error> {
        let ts = (target_secs.max(0.0) * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        self.ictx.seek(ts, i64::MIN..)?;
        self.eof = false;
        Ok(())
    }

    fn decode(
        &mut self,
        on_frame: &mut impl FnMut(DecodedFrame) -> bool,
    ) -> Result<(), ffmpeg::Error> {
        if self.eof {
            return Ok(());
        }
        let mut decoder =
            ffmpeg::codec::context::Context::from_parameters(self.parameters.clone())?
                .decoder()
                .video()?;
        let mut scaler = ffmpeg::software::scaling::context::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            ffmpeg::format::Pixel::RGB24,
            self.dst_w,
            self.dst_h,
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )?;
        for (stream, packet) in self.ictx.packets() {
            if stream.index() != self.stream_index {
                continue;
            }
            decoder.send_packet(&packet)?;
            let mut decoded = ffmpeg::util::frame::video::Video::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                let pts = decoded
                    .pts()
                    .map(|p| p as f64 * self.time_base)
                    .unwrap_or(0.0);
                let mut rgb = ffmpeg::util::frame::video::Video::empty();
                scaler.run(&decoded, &mut rgb)?;
                let mut buf = Vec::with_capacity((self.dst_w * self.dst_h * 3) as usize);
                for i in 0..self.dst_h as usize {
                    let row = &rgb.data(0)
                        [i * rgb.stride(0)..i * rgb.stride(0) + self.dst_w as usize * 3];
                    buf.extend_from_slice(row);
                }
                // 应用容器旋转元数据（手机竖拍方向正确显示）
                let (buf, w, h) = match self.rotation {
                    90 => (
                        rotate_rgb_90_cw(&buf, self.dst_w, self.dst_h),
                        self.dst_h,
                        self.dst_w,
                    ),
                    180 => (
                        rotate_rgb_180(&buf, self.dst_w, self.dst_h),
                        self.dst_w,
                        self.dst_h,
                    ),
                    270 => (
                        rotate_rgb_90_ccw(&buf, self.dst_w, self.dst_h),
                        self.dst_h,
                        self.dst_w,
                    ),
                    _ => (buf, self.dst_w, self.dst_h),
                };
                if !on_frame(DecodedFrame {
                    pts_secs: pts,
                    rgb: buf,
                    width: w,
                    height: h,
                }) {
                    return Ok(());
                }
            }
        }
        self.eof = true;
        Ok(())
    }
}

/// 提取首帧（最长边限制为 max_dim，0 表示原始尺寸），转 RGB24 紧密布局。
pub fn first_frame(path: &Path, max_dim: u32) -> Result<(VideoInfo, Vec<u8>), ffmpeg::Error> {
    let mut dec = VideoDecoder::open(path, max_dim)?;
    let info = dec.info().clone();
    match dec.seek_frame(0.0)? {
        Some(frame) => Ok((info, frame.rgb)),
        None => Err(ffmpeg::Error::Eof),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "tests/fixtures/sample.mp4";

    #[test]
    fn reads_info_from_sample() {
        let info = VideoDecoder::open(Path::new(SAMPLE), 0)
            .expect("open failed")
            .info()
            .clone();
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
    fn seek_returns_frame_at_target() {
        let mut dec = VideoDecoder::open(Path::new(SAMPLE), 0).expect("open failed");
        let frame = dec.seek_frame(1.5).expect("seek failed").expect("frame");
        assert!(
            frame.pts_secs >= 1.45 && frame.pts_secs <= 1.55,
            "pts {} should be near 1.5",
            frame.pts_secs
        );
        assert_eq!(frame.rgb.len(), 320 * 240 * 3);
    }

    #[test]
    fn seek_later_is_forward() {
        let mut dec = VideoDecoder::open(Path::new(SAMPLE), 0).expect("open failed");
        let a = dec.seek_frame(0.5).expect("seek failed").expect("frame");
        let b = dec.seek_frame(1.5).expect("seek failed").expect("frame");
        assert!(b.pts_secs > a.pts_secs, "later seek is forward");
    }

    #[test]
    fn play_decodes_sequence() {
        let mut dec = VideoDecoder::open(Path::new(SAMPLE), 0).expect("open failed");
        let mut pts: Vec<f64> = Vec::new();
        dec.play(0.0, |frame| {
            pts.push(frame.pts_secs);
            pts.len() < 5
        })
        .expect("play failed");
        assert_eq!(pts.len(), 5);
        assert!(pts.windows(2).all(|w| w[1] > w[0]), "pts increasing");
        assert!((pts[1] - pts[0] - 1.0 / 30.0).abs() < 0.05, "30fps cadence");
    }

    #[test]
    fn rotation_metadata_is_applied() {
        // sample_rot90.mp4：蓝底 + 左上 80x80 白块，tkhd matrix 旋转 90（顺时针）
        let dec = VideoDecoder::open(Path::new("tests/fixtures/sample_rot90.mp4"), 0)
            .expect("open failed");
        let info = dec.info();
        assert_eq!((info.width, info.height), (240, 320), "旋转后显示尺寸交换");
        let (_, buf) =
            first_frame(Path::new("tests/fixtures/sample_rot90.mp4"), 0).expect("rotated frame");
        let px = |x: usize, y: usize| {
            [
                buf[(y * 240 + x) * 3],
                buf[(y * 240 + x) * 3 + 1],
                buf[(y * 240 + x) * 3 + 2],
            ]
        };
        // x264 有损压缩：颜色比较带容差
        let close = |a: [u8; 3], b: [u8; 3]| {
            a.iter()
                .zip(b)
                .all(|(x, y)| (*x as i32 - y as i32).abs() <= 8)
        };
        assert!(close(px(0, 0), [0, 0, 255]), "左上仍为蓝底: {:?}", px(0, 0));
        assert!(
            close(px(239, 0), [255, 255, 255]),
            "右上应为白块（原左上 90° 顺时针）: {:?}",
            px(239, 0)
        );
        assert!(
            close(px(0, 319), [0, 0, 255]),
            "左下仍为蓝底: {:?}",
            px(0, 319)
        );
        assert!(
            close(px(239, 319), [0, 0, 255]),
            "右下仍为蓝底: {:?}",
            px(239, 319)
        );
    }

    #[test]
    fn missing_file_errors() {
        assert!(VideoDecoder::open(Path::new("tests/fixtures/nonexistent.mp4"), 0).is_err());
        assert!(first_frame(Path::new("tests/fixtures/nonexistent.mp4"), 0).is_err());
    }
}
