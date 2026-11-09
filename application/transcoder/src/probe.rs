//! Reads the input file's technical metadata with ffprobe.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::ffmpeg::{if_match_header, is_url, redact};

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

/// `input` is a file path or an http(s) URL. For a URL ffprobe only fetches
/// the parts of the file it needs to read the headers; with `etag`, only from
/// that version of the object.
pub fn probe(input: &str, etag: Option<&str>) -> Result<SourceInfo> {
    if !is_url(input) && !Path::new(input).is_file() {
        bail!("input file {input} does not exist");
    }
    let mut command = Command::new("ffprobe");
    command.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]);
    if let (true, Some(etag)) = (is_url(input), etag) {
        command.args(["-headers", &if_match_header(etag)]);
    }
    let out = command
        .arg(input)
        .output()
        .context("could not start ffprobe (is ffmpeg installed?)")?;
    if !out.status.success() {
        bail!(
            "ffprobe failed: {}",
            redact(String::from_utf8_lossy(&out.stderr).trim())
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
