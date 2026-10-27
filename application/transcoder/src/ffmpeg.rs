//! Builds the ffmpeg commands from the config. Running them is `runner.rs`.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::probe::SourceInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Hls,
    Dash,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Hls => "HLS",
            Format::Dash => "DASH",
        }
    }

    /// Column of this format in per-video tables.
    pub fn slot(self) -> usize {
        match self {
            Format::Hls => 0,
            Format::Dash => 1,
        }
    }
}

/// One input file.
#[derive(Debug, Clone)]
pub struct Video {
    pub path: PathBuf,
    /// File name without extension; also the name of its output folder.
    pub name: String,
    pub out_dir: PathBuf,
    /// The ffprobe result, or why probing failed.
    pub source: Result<SourceInfo, String>,
}

impl Video {
    pub fn duration(&self) -> f64 {
        self.source.as_ref().map(|s| s.duration).unwrap_or(0.0)
    }
}

/// One ffmpeg run: one video in one format.
#[derive(Debug, Clone)]
pub struct Job {
    /// Index into the list of videos.
    pub video: usize,
    pub format: Format,
    pub args: Vec<String>,
    /// The manifest the player opens.
    pub manifest: PathBuf,
}

impl Job {
    /// The command as you would type it in a terminal.
    pub fn command_line(&self) -> String {
        std::iter::once("ffmpeg".to_string())
            .chain(self.args.iter().map(|a| shell_quote(a)))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The jobs for one video, one per requested format.
pub fn build_jobs(
    index: usize,
    video: &Video,
    src: &SourceInfo,
    cfg: &Config,
    formats: &[Format],
) -> Vec<Job> {
    formats
        .iter()
        .map(|&format| match format {
            Format::Hls => hls_job(index, video, cfg, src),
            Format::Dash => dash_job(index, video, cfg, src),
        })
        .collect()
}

/// The part both formats share: split + scale into the ladder, encode video and audio.
/// `audio_copies` is how many times the audio track is mapped (HLS: once per rendition,
/// DASH: once, shared by all renditions).
fn encode_args(input: &Path, cfg: &Config, src: &SourceInfo, audio_copies: usize) -> Vec<String> {
    let n = cfg.renditions.len();
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-nostats".into(),
        // Machine-readable progress (key=value lines) on stdout.
        "-progress".into(),
        "pipe:1".into(),
        "-i".into(),
        input.display().to_string(),
    ];

    // [0:v]split=N[s0][s1]...;[s0]scale=-2:1080[v0];...  (plus ,fps=30 if configured)
    // No setsar=1: when the width gets rounded (480p → 854 instead of 853.3), scale
    // adjusts the pixel shape so every rendition keeps exactly the source's aspect
    // ratio. DASH refuses renditions with even slightly different aspect ratios.
    let mut filter = format!("[0:v]split={n}");
    for i in 0..n {
        filter.push_str(&format!("[s{i}]"));
    }
    for (i, r) in cfg.renditions.iter().enumerate() {
        filter.push_str(&format!(";[s{i}]scale=-2:{}", r.height));
        // Keyframes are placed by time, not frame count, so renditions with
        // different frame rates still get segments at the same moments.
        if let Some(fps) = cfg.fps_for(r) {
            filter.push_str(&format!(",fps={fps}"));
        }
        filter.push_str(&format!("[v{i}]"));
    }
    args.extend(["-filter_complex".into(), filter]);

    for i in 0..n {
        args.extend(["-map".into(), format!("[v{i}]")]);
    }
    if src.has_audio() {
        for _ in 0..audio_copies {
            args.extend(["-map".into(), "0:a:0".into()]);
        }
    }

    args.extend(
        [
            "-c:v",
            "libx264",
            "-preset",
            &cfg.preset,
            "-pix_fmt",
            "yuv420p",
            // A keyframe at every segment boundary, and no extra ones from scene cuts.
            "-force_key_frames",
            &format!("expr:gte(t,n_forced*{})", cfg.segment_duration),
            "-sc_threshold",
            "0",
        ]
        .map(String::from),
    );
    for (i, r) in cfg.renditions.iter().enumerate() {
        args.extend([
            format!("-b:v:{i}"),
            format!("{}k", r.bitrate_kbps),
            format!("-maxrate:v:{i}"),
            format!("{}k", r.maxrate_kbps()),
            format!("-bufsize:v:{i}"),
            format!("{}k", r.bufsize_kbps()),
        ]);
    }
    if src.has_audio() {
        args.extend(
            [
                "-c:a",
                "aac",
                "-ac",
                "2",
                "-b:a",
                &format!("{}k", cfg.audio_bitrate_kbps),
            ]
            .map(String::from),
        );
    }
    args
}

fn hls_job(index: usize, video: &Video, cfg: &Config, src: &SourceInfo) -> Job {
    let dir = video.out_dir.join("hls");
    let mut args = encode_args(&video.path, cfg, src, cfg.renditions.len());

    // v:0,a:0,name:1080p v:1,a:1,name:720p ...
    let stream_map = cfg
        .renditions
        .iter()
        .enumerate()
        .map(|(i, r)| {
            if src.has_audio() {
                format!("v:{i},a:{i},name:{}", r.name)
            } else {
                format!("v:{i},name:{}", r.name)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    args.extend([
        "-f".into(),
        "hls".into(),
        "-hls_time".into(),
        cfg.segment_duration.to_string(),
        "-hls_playlist_type".into(),
        "vod".into(),
        "-hls_segment_filename".into(),
        dir.join("%v").join("seg_%03d.ts").display().to_string(),
        "-var_stream_map".into(),
        stream_map,
        "-master_pl_name".into(),
        "master.m3u8".into(),
        dir.join("%v").join("index.m3u8").display().to_string(),
    ]);
    Job {
        video: index,
        format: Format::Hls,
        args,
        manifest: dir.join("master.m3u8"),
    }
}

fn dash_job(index: usize, video: &Video, cfg: &Config, src: &SourceInfo) -> Job {
    let dir = video.out_dir.join("dash");
    let manifest = dir.join("manifest.mpd");
    let mut args = encode_args(&video.path, cfg, src, 1);
    let sets = if src.has_audio() {
        "id=0,streams=v id=1,streams=a"
    } else {
        "id=0,streams=v"
    };
    args.extend([
        "-f".into(),
        "dash".into(),
        "-seg_duration".into(),
        cfg.segment_duration.to_string(),
        "-use_template".into(),
        "1".into(),
        "-use_timeline".into(),
        "0".into(),
        "-adaptation_sets".into(),
        sets.into(),
        manifest.display().to_string(),
    ]);
    Job {
        video: index,
        format: Format::Dash,
        args,
        manifest,
    }
}

fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=%,+@".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}
