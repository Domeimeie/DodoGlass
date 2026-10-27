//! Reads the input file's technical metadata with ffprobe.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub audio_codec: Option<String>,
}

impl SourceInfo {
    pub fn has_audio(&self) -> bool {
        self.audio_codec.is_some()
    }
}

#[derive(Deserialize)]
struct Output {
    format: Format,
    streams: Vec<Stream>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: String,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
}

pub fn probe(input: &Path) -> Result<SourceInfo> {
    if !input.is_file() {
        bail!("input file {} does not exist", input.display());
    }
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(input)
        .output()
        .context("could not start ffprobe (is ffmpeg installed?)")?;
    if !out.status.success() {
        bail!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let parsed: Output =
        serde_json::from_slice(&out.stdout).context("unexpected ffprobe output")?;

    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type == "video")
        .context("input has no video track")?;
    let audio = parsed.streams.iter().find(|s| s.codec_type == "audio");

    Ok(SourceInfo {
        duration: parsed
            .format
            .duration
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.0),
        width: video.width.context("video track has no width")?,
        height: video.height.context("video track has no height")?,
        fps: video
            .avg_frame_rate
            .as_deref()
            .map(parse_rate)
            .unwrap_or(0.0),
        audio_codec: audio.map(|a| a.codec_name.clone().unwrap_or_else(|| "?".into())),
    })
}

/// ffprobe writes frame rates as fractions, e.g. "24/1" or "30000/1001".
fn parse_rate(rate: &str) -> f64 {
    match rate.split_once('/') {
        Some((n, d)) => match (n.parse::<f64>(), d.parse::<f64>()) {
            (Ok(n), Ok(d)) if d != 0.0 => n / d,
            _ => 0.0,
        },
        None => rate.parse().unwrap_or(0.0),
    }
}
