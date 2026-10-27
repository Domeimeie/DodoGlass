//! The transcoding config: segment length and the ladder of renditions.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The ladder shipped with the program, used when no `--config` is given.
pub const DEFAULT_CONFIG: &str = include_str!("../ladder.toml");

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_segment_duration")]
    pub segment_duration: f64,
    #[serde(default = "default_preset")]
    pub preset: String,
    #[serde(default = "default_audio_bitrate")]
    pub audio_bitrate_kbps: u32,
    /// Frame rate for every rendition; None keeps the source's.
    pub fps: Option<f64>,
    #[serde(rename = "rendition")]
    pub renditions: Vec<Rendition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rendition {
    pub name: String,
    pub height: u32,
    pub bitrate_kbps: u32,
    pub maxrate_kbps: Option<u32>,
    pub bufsize_kbps: Option<u32>,
    /// Frame rate for this rendition; overrides `Config::fps`.
    pub fps: Option<f64>,
}

fn default_segment_duration() -> f64 {
    4.0
}

fn default_preset() -> String {
    "veryfast".into()
}

fn default_audio_bitrate() -> u32 {
    128
}

impl Rendition {
    pub fn maxrate_kbps(&self) -> u32 {
        self.maxrate_kbps.unwrap_or(self.bitrate_kbps * 107 / 100)
    }

    pub fn bufsize_kbps(&self) -> u32 {
        self.bufsize_kbps.unwrap_or(self.bitrate_kbps * 3 / 2)
    }
}

impl Config {
    /// The frame rate a rendition is encoded at, or None to keep the source's.
    pub fn fps_for(&self, r: &Rendition) -> Option<f64> {
        r.fps.or(self.fps)
    }

    /// Reads the config from `path`, or uses the built-in ladder.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let text = match path {
            Some(p) => fs::read_to_string(p)
                .with_context(|| format!("cannot read config {}", p.display()))?,
            None => DEFAULT_CONFIG.to_string(),
        };
        let config: Config = toml::from_str(&text).context("invalid config")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.renditions.is_empty() {
            bail!("config has no [[rendition]] entries");
        }
        if self.segment_duration <= 0.0 {
            bail!("segment_duration must be greater than 0");
        }
        let fps_ok = |fps: Option<f64>| fps.is_none_or(|f| f > 0.0 && f <= 240.0);
        if !fps_ok(self.fps) {
            bail!("fps must be between 0 and 240");
        }
        let mut names = HashSet::new();
        for r in &self.renditions {
            // The name becomes a folder name and part of ffmpeg's stream map.
            if r.name.is_empty()
                || !r
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                bail!(
                    "rendition name {:?} may only contain letters, digits, - and _",
                    r.name
                );
            }
            if !names.insert(&r.name) {
                bail!("rendition name {:?} is used twice", r.name);
            }
            // H.264 with 4:2:0 colour needs even picture sizes.
            if r.height == 0 || r.height % 2 != 0 {
                bail!("rendition {:?}: height must be an even number", r.name);
            }
            if !fps_ok(r.fps) {
                bail!("rendition {:?}: fps must be between 0 and 240", r.name);
            }
            if r.bitrate_kbps == 0 {
                bail!(
                    "rendition {:?}: bitrate_kbps must be greater than 0",
                    r.name
                );
            }
        }
        Ok(())
    }
}
