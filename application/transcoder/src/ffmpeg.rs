//! Builds the ffmpeg commands from the config. Running them is `runner.rs`.

use std::borrow::Cow;
use std::path::PathBuf;

use crate::config::Config;
use crate::probe::SourceInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
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

    /// Folder (or key prefix) of this format's output below the video's.
    pub fn dir_name(self) -> &'static str {
        match self {
            Format::Hls => "hls",
            Format::Dash => "dash",
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

/// One input video.
#[derive(Debug, Clone)]
pub struct Video {
    /// What ffmpeg opens: a file path, or an http(s) URL it streams from.
    pub input: String,
    /// For a URL: only read this version of the object (`If-Match`).
    pub input_etag: Option<String>,
    /// File name without extension; also the name of its output folder.
    pub name: String,
    pub out_dir: OutDir,
    /// The ffprobe result, or why probing failed.
    pub source: Result<SourceInfo, String>,
}

impl Video {
    pub fn duration(&self) -> f64 {
        self.source.as_ref().map(|s| s.duration).unwrap_or(0.0)
    }
}

/// Where ffmpeg writes its output.
#[derive(Debug, Clone, PartialEq)]
pub enum OutDir {
    /// A folder on disk.
    Local(PathBuf),
    /// ffmpeg uploads every file with HTTP PUT below `url` (see `upload.rs`).
    /// `shown` is where the files end up, for logs and status messages.
    Http { url: String, shown: String },
}

impl OutDir {
    pub fn join(&self, part: &str) -> OutDir {
        match self {
            OutDir::Local(dir) => OutDir::Local(dir.join(part)),
            OutDir::Http { url, shown } => OutDir::Http {
                url: format!("{url}/{part}"),
                shown: format!("{shown}/{part}"),
            },
        }
    }

    /// The path or URL ffmpeg is given.
    fn target(&self) -> String {
        match self {
            OutDir::Local(dir) => dir.display().to_string(),
            OutDir::Http { url, .. } => url.clone(),
        }
    }

    /// For people: a path, or `s3://bucket/key`.
    pub fn display(&self) -> String {
        match self {
            OutDir::Local(dir) => dir.display().to_string(),
            OutDir::Http { shown, .. } => shown.clone(),
        }
    }

    fn is_http(&self) -> bool {
        matches!(self, OutDir::Http { .. })
    }
}

/// One ffmpeg run: one video in one format.
#[derive(Debug, Clone)]
pub struct Job {
    /// Index into the list of videos.
    pub video: usize,
    pub format: Format,
    pub args: Vec<String>,
    /// The folder of this format's output (`<out>/<video>/hls`).
    pub dir: OutDir,
    /// The manifest the player opens.
    pub manifest: OutDir,
}

impl Job {
    /// The command as you would type it in a terminal, with signed URLs
    /// redacted (see [`redact`]).
    pub fn command_line(&self) -> String {
        std::iter::once("ffmpeg".to_string())
            .chain(self.args.iter().map(|a| shell_quote(&redact(a))))
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
fn encode_args(video: &Video, cfg: &Config, src: &SourceInfo, audio_copies: usize) -> Vec<String> {
    let input = video.input.as_str();
    let n = cfg.renditions.len();
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-nostats".into(),
        // Machine-readable progress (key=value lines) on stdout.
        "-progress".into(),
        "pipe:1".into(),
    ];
    if is_url(input) {
        // Reading over HTTP: ffmpeg fetches the file in ranges as it needs
        // them (and jumps to the index at the end of an MP4 the same way).
        // If the connection drops, continue where it stopped instead of failing.
        args.extend(
            [
                "-reconnect",
                "1",
                "-reconnect_on_network_error",
                "1",
                "-reconnect_delay_max",
                "10",
            ]
            .map(String::from),
        );
        if let Some(etag) = &video.input_etag {
            args.extend(["-headers".into(), if_match_header(etag)]);
        }
    }
    args.extend(["-i".into(), input.to_string()]);

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
    let dir = video.out_dir.join(Format::Hls.dir_name());
    let mut args = encode_args(video, cfg, src, cfg.renditions.len());

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

    args.extend(["-f".into(), "hls".into()]);
    args.extend(http_output_args(&dir));
    args.extend([
        "-hls_time".into(),
        cfg.segment_duration.to_string(),
        "-hls_playlist_type".into(),
        "vod".into(),
        "-hls_segment_filename".into(),
        dir.join("%v").join("seg_%03d.ts").target(),
        "-var_stream_map".into(),
        stream_map,
        "-master_pl_name".into(),
        "master.m3u8".into(),
        dir.join("%v").join("index.m3u8").target(),
    ]);
    Job {
        video: index,
        format: Format::Hls,
        args,
        manifest: dir.join("master.m3u8"),
        dir,
    }
}

fn dash_job(index: usize, video: &Video, cfg: &Config, src: &SourceInfo) -> Job {
    let dir = video.out_dir.join(Format::Dash.dir_name());
    let manifest = dir.join("manifest.mpd");
    let mut args = encode_args(video, cfg, src, 1);
    let sets = if src.has_audio() {
        "id=0,streams=v id=1,streams=a"
    } else {
        "id=0,streams=v"
    };
    args.extend(["-f".into(), "dash".into()]);
    args.extend(http_output_args(&dir));
    args.extend([
        "-seg_duration".into(),
        cfg.segment_duration.to_string(),
        "-use_template".into(),
        "1".into(),
        "-use_timeline".into(),
        "0".into(),
        "-adaptation_sets".into(),
        sets.into(),
        manifest.target(),
    ]);
    Job {
        video: index,
        format: Format::Dash,
        args,
        manifest,
        dir,
    }
}

/// For HTTP output: upload every file with PUT, over one kept-alive connection.
fn http_output_args(dir: &OutDir) -> Vec<String> {
    if dir.is_http() {
        ["-method", "PUT", "-http_persistent", "1"]
            .map(String::from)
            .to_vec()
    } else {
        Vec::new()
    }
}

/// ffmpeg's `-headers` value: sent with every request, including the range
/// requests after a seek or reconnect. The storage answers 412 if the object
/// is no longer this version.
pub fn if_match_header(etag: &str) -> String {
    format!("If-Match: \"{}\"\r\n", etag.trim_matches('"'))
}

pub fn is_url(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

/// Hides the query string of signed URLs (`...?X-Amz-Signature=...`) so they
/// don't end up in logs: whoever has the full URL can read the video until it
/// expires. Applied to the command line and to every line ffmpeg prints.
pub fn redact(text: &str) -> Cow<'_, str> {
    if !text.contains("X-Amz-") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('?') {
        let query = &rest[start + 1..];
        if !query.starts_with("X-Amz-") {
            out.push_str(&rest[..=start]);
            rest = query;
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str("?<signature redacted>");
        let end = query
            .find(|c: char| c.is_whitespace() || "'\"<>".contains(c))
            .unwrap_or(query.len());
        rest = &query[end..];
    }
    out.push_str(rest);
    Cow::Owned(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_signed_urls() {
        let url = "http://localhost:9000/ingest/a.mp4?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature=abc123";
        assert_eq!(
            redact(&format!("Input #0, mov, from '{url}':")),
            "Input #0, mov, from 'http://localhost:9000/ingest/a.mp4?<signature redacted>':"
        );
        assert_eq!(redact(&format!("-i {url} -f hls")), "-i http://localhost:9000/ingest/a.mp4?<signature redacted> -f hls");
        assert_eq!(redact("plain line, no url?"), "plain line, no url?");
        assert_eq!(redact("what? X-Amz-Date is mentioned"), "what? X-Amz-Date is mentioned");
    }

    #[test]
    fn http_output_uploads_with_put() {
        let cfg = Config::load(None).unwrap();
        let src = SourceInfo {
            duration: 10.0,
            width: 1920,
            height: 1080,
            fps: 24.0,
            audio_codec: Some("aac".into()),
        };
        let video = Video {
            input: "http://localhost:9000/ingest/a.mp4?X-Amz-Signature=x".into(),
            input_etag: Some("abc-2".into()),
            name: "a".into(),
            out_dir: OutDir::Http {
                url: "http://127.0.0.1:1234/tok/a".into(),
                shown: "s3://final/a".into(),
            },
            source: Ok(src.clone()),
        };
        let jobs = build_jobs(0, &video, &src, &cfg, &[Format::Hls, Format::Dash]);
        for job in &jobs {
            let args = job.args.join(" ");
            assert!(args.contains("-reconnect 1"), "{args}");
            assert!(args.contains("-method PUT"), "{args}");
            assert!(job.args.contains(&"If-Match: \"abc-2\"\r\n".to_string()));
            assert!(!job.command_line().contains("X-Amz-Signature"));
        }
        assert!(jobs[0].args.contains(&"http://127.0.0.1:1234/tok/a/hls/%v/seg_%03d.ts".to_string()));
        assert_eq!(jobs[0].manifest.display(), "s3://final/a/hls/master.m3u8");
        assert_eq!(jobs[1].manifest.display(), "s3://final/a/dash/manifest.mpd");
    }

    #[test]
    fn local_output_has_no_http_options() {
        let cfg = Config::load(None).unwrap();
        let src = SourceInfo {
            duration: 10.0,
            width: 1280,
            height: 720,
            fps: 30.0,
            audio_codec: None,
        };
        let video = Video {
            input: "in/a.mkv".into(),
            input_etag: None,
            name: "a".into(),
            out_dir: OutDir::Local("out/a".into()),
            source: Ok(src.clone()),
        };
        let args = build_jobs(0, &video, &src, &cfg, &[Format::Hls])[0].args.join(" ");
        assert!(!args.contains("-method"));
        assert!(!args.contains("-reconnect"));
    }
}
