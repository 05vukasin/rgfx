//! Parsing `ffprobe`'s JSON report into a [`VideoInfo`] summary.
//!
//! This module is pure: it does not spawn any process, so its logic can be unit
//! tested against fixture JSON without ffmpeg installed.

use rgfx_core::{Error, Result};
use serde::Deserialize;
use std::time::Duration;

/// A summary of a video's first video stream, as reported by `ffprobe`.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frames per second, derived from the stream's average frame rate.
    ///
    /// `0.0` when ffprobe reports no usable frame rate (e.g. `0/0`).
    pub fps: f64,
    /// Total duration in seconds, if ffprobe reported one.
    pub duration_secs: Option<f64>,
    /// The video codec name (for example `"h264"`), if reported.
    pub codec: Option<String>,
}

impl VideoInfo {
    /// Parses the JSON produced by
    /// `ffprobe -print_format json -show_streams -show_format`.
    ///
    /// The first stream whose `codec_type` is `"video"` is used.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if the JSON is malformed, contains no video
    /// stream, or the video stream is missing its width/height. Never panics.
    pub fn from_ffprobe_json(json: &str) -> Result<Self> {
        let parsed: FfprobeOutput =
            serde_json::from_str(json).map_err(|e| Error::Decode(format!("ffprobe json: {e}")))?;

        let stream = parsed
            .streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("video"))
            .ok_or_else(|| Error::Decode("ffprobe reported no video stream".into()))?;

        let width = stream
            .width
            .ok_or_else(|| Error::Decode("video stream is missing width".into()))?;
        let height = stream
            .height
            .ok_or_else(|| Error::Decode("video stream is missing height".into()))?;

        // Prefer the average frame rate; fall back to the base (r) frame rate.
        let fps = stream
            .avg_frame_rate
            .as_deref()
            .and_then(parse_rational)
            .filter(|f| *f > 0.0)
            .or_else(|| stream.r_frame_rate.as_deref().and_then(parse_rational))
            .filter(|f| *f > 0.0)
            .unwrap_or(0.0);

        // Duration may live on the stream or on the container format.
        let duration_secs = stream
            .duration
            .as_deref()
            .and_then(|d| d.parse::<f64>().ok())
            .or_else(|| {
                parsed
                    .format
                    .as_ref()
                    .and_then(|f| f.duration.as_deref())
                    .and_then(|d| d.parse::<f64>().ok())
            })
            .filter(|d| d.is_finite() && *d >= 0.0);

        Ok(Self {
            width,
            height,
            fps,
            duration_secs,
            codec: stream.codec_name.clone(),
        })
    }

    /// The delay between consecutive frames, derived from [`VideoInfo::fps`].
    ///
    /// Returns `None` when the frame rate is unknown or non-positive, in which
    /// case the caller must drive timing externally.
    pub fn frame_delay(&self) -> Option<Duration> {
        if self.fps.is_finite() && self.fps > 0.0 {
            Some(Duration::from_secs_f64(1.0 / self.fps))
        } else {
            None
        }
    }
}

/// Parses an ffprobe rational string such as `"30/1"` or `"30000/1001"` into a
/// floating-point ratio. Returns `None` for malformed input or a zero
/// denominator.
fn parse_rational(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((num, den)) = s.split_once('/') {
        let num: f64 = num.trim().parse().ok()?;
        let den: f64 = den.trim().parse().ok()?;
        if den == 0.0 {
            return None;
        }
        Some(num / den)
    } else {
        s.parse().ok()
    }
}

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    #[serde(default)]
    format: Option<FfprobeFormat>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative `ffprobe -print_format json` report for a short clip
    /// with one video stream and one audio stream.
    const FIXTURE: &str = r#"{
        "streams": [
            {
                "codec_type": "audio",
                "codec_name": "aac"
            },
            {
                "codec_type": "video",
                "codec_name": "h264",
                "width": 1280,
                "height": 720,
                "avg_frame_rate": "30000/1001",
                "r_frame_rate": "30000/1001",
                "duration": "2.500000"
            }
        ],
        "format": {
            "duration": "2.560000"
        }
    }"#;

    #[test]
    fn parses_dims_fps_duration_codec() {
        let info = VideoInfo::from_ffprobe_json(FIXTURE).unwrap();
        assert_eq!((info.width, info.height), (1280, 720));
        assert!((info.fps - 29.97002997).abs() < 1e-6, "{}", info.fps);
        assert_eq!(info.duration_secs, Some(2.5));
        assert_eq!(info.codec.as_deref(), Some("h264"));
    }

    #[test]
    fn frame_delay_from_fps() {
        let info = VideoInfo::from_ffprobe_json(FIXTURE).unwrap();
        let delay = info.frame_delay().unwrap();
        // 1 / 29.97 ≈ 33.37ms
        assert!((delay.as_secs_f64() - 1.0 / 29.97002997).abs() < 1e-9);
    }

    #[test]
    fn falls_back_to_format_duration() {
        let json = r#"{
            "streams": [
                {"codec_type": "video", "codec_name": "vp9", "width": 640, "height": 480,
                 "avg_frame_rate": "24/1"}
            ],
            "format": {"duration": "10.0"}
        }"#;
        let info = VideoInfo::from_ffprobe_json(json).unwrap();
        assert_eq!(info.duration_secs, Some(10.0));
        assert!((info.fps - 24.0).abs() < 1e-9);
    }

    #[test]
    fn falls_back_to_r_frame_rate_when_avg_is_zero() {
        let json = r#"{
            "streams": [
                {"codec_type": "video", "width": 16, "height": 16,
                 "avg_frame_rate": "0/0", "r_frame_rate": "25/1"}
            ]
        }"#;
        let info = VideoInfo::from_ffprobe_json(json).unwrap();
        assert!((info.fps - 25.0).abs() < 1e-9);
        assert_eq!(info.codec, None);
    }

    #[test]
    fn zero_frame_rate_yields_no_delay() {
        let json = r#"{
            "streams": [
                {"codec_type": "video", "width": 16, "height": 16,
                 "avg_frame_rate": "0/0", "r_frame_rate": "0/0"}
            ]
        }"#;
        let info = VideoInfo::from_ffprobe_json(json).unwrap();
        assert_eq!(info.fps, 0.0);
        assert_eq!(info.frame_delay(), None);
    }

    #[test]
    fn no_video_stream_is_error_not_panic() {
        let json = r#"{"streams": [{"codec_type": "audio", "codec_name": "aac"}]}"#;
        assert!(matches!(
            VideoInfo::from_ffprobe_json(json),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn missing_dimensions_is_error() {
        let json = r#"{"streams": [{"codec_type": "video", "codec_name": "h264"}]}"#;
        assert!(matches!(
            VideoInfo::from_ffprobe_json(json),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn malformed_json_is_error_not_panic() {
        assert!(matches!(
            VideoInfo::from_ffprobe_json("not json at all"),
            Err(Error::Decode(_))
        ));
        assert!(VideoInfo::from_ffprobe_json("").is_err());
    }

    #[test]
    fn rational_parsing() {
        assert_eq!(parse_rational("30/1"), Some(30.0));
        assert_eq!(parse_rational("0/0"), None);
        assert_eq!(parse_rational("60"), Some(60.0));
        assert_eq!(parse_rational("garbage"), None);
    }
}
