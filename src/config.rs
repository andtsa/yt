use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;

use crate::paths::Paths;

/// Prefer H.264 (<=1080p) + AAC, which every player (including QuickTime) can
/// play; YouTube only serves VP9/AV1 above 1080p.
pub const DEFAULT_FORMAT: &str = "bv*[vcodec^=avc1][height<=1080]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b";

pub const TEMPLATE: &str = r#"# youtube-at-home config. Every key is optional.

# Directories that hold video files ("<title> [<video id>].<ext>").
# All of them are searched; new downloads go to the first one that currently exists,
# so an unplugged external drive is simply skipped.
# Relative paths are relative to this file, ~ is the home directory.
# Default: "media" next to library.json for a portable library, otherwise
# ~/Movies/youtube-at-home (macOS) or ~/Videos/youtube-at-home (Linux, Windows).
# media_dirs = ["~/Videos/youtube-at-home", "/Volumes/External/youtube"]

# Soft budget for downloaded videos: `yt du` and `yt get` warn when it is exceeded.
# max_disk_gb = 50

# Video player: "auto", "quicktime" (macOS only), "vlc" or "system".
# auto = QuickTime on macOS, otherwise VLC if it's installed, otherwise the system default.
# QuickTime and VLC report playback progress; "system" just opens the file.
# player = "auto"
# Path to the VLC executable, if it isn't found automatically.
# vlc = "/Applications/VLC.app"

# yt-dlp format selector. The default prefers H.264 <=1080p, which every player can play.
# format = "bv*[vcodec^=avc1][height<=1080]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b"

# A random one is used for each yt-dlp invocation. Empty = no proxy.
# proxies = ["socks5://127.0.0.1:1080"]

# yt-dlp executable. Default: the copy installed by `yt setup`, else yt-dlp on PATH.
# ytdlp = "/usr/local/bin/yt-dlp"
# extra_args = ["--embed-subs", "--sub-langs", "en.*"]

# Optional descriptions. A watchlist also exists as soon as any video is in it.
[watchlists]
# physics = "QM, E&M, stat mech"
# math = ""
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerChoice {
    #[default]
    Auto,
    Quicktime,
    Vlc,
    System,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub media_dirs: Vec<PathBuf>,
    pub max_disk_gb: Option<f64>,
    pub player: PlayerChoice,
    pub vlc: Option<PathBuf>,
    pub format: String,
    pub proxies: Vec<String>,
    pub ytdlp: Option<PathBuf>,
    pub extra_args: Vec<String>,
    pub watchlists: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            media_dirs: vec![],
            max_disk_gb: None,
            player: PlayerChoice::Auto,
            vlc: None,
            format: DEFAULT_FORMAT.into(),
            proxies: vec![],
            ytdlp: None,
            extra_args: vec![],
            watchlists: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Self> {
        if !paths.config.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&paths.config)?;
        toml::from_str(&text).with_context(|| format!("parsing {}", paths.config.display()))
    }

    pub fn media_dirs(&self, paths: &Paths) -> Vec<PathBuf> {
        if self.media_dirs.is_empty() {
            return vec![paths.default_media.clone()];
        }
        self.media_dirs.iter().map(|d| paths.resolve(d)).collect()
    }

    /// The first media dir that exists. The default one is created on demand;
    /// configured ones aren't, since they may be on an unplugged drive.
    pub fn download_dir(&self, paths: &Paths) -> Result<PathBuf> {
        if self.media_dirs.is_empty() {
            fs::create_dir_all(&paths.default_media)?;
        }
        self.media_dirs(paths)
            .into_iter()
            .find(|d| d.is_dir())
            .context("none of the configured media_dirs exist")
    }

    pub fn max_bytes(&self) -> Option<u64> {
        self.max_disk_gb.map(|gb| (gb * 1e9) as u64)
    }

    pub fn pick_proxy(&self) -> Option<&str> {
        if self.proxies.is_empty() {
            return None;
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0) as usize;
        Some(&self.proxies[nanos % self.proxies.len()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        let cfg: Config = toml::from_str(TEMPLATE).unwrap();
        assert!(cfg.media_dirs.is_empty());
        assert_eq!(cfg.player, PlayerChoice::Auto);
        assert_eq!(cfg.format, DEFAULT_FORMAT);
    }

    #[test]
    fn parses_settings_and_rejects_typos() {
        let cfg: Config = toml::from_str(
            "media_dirs = [\"media\"]\nplayer = \"vlc\"\nmax_disk_gb = 3\n[watchlists]\nmath = \"\"",
        )
        .unwrap();
        assert_eq!(cfg.player, PlayerChoice::Vlc);
        assert_eq!(cfg.max_bytes(), Some(3_000_000_000));
        assert!(cfg.watchlists.contains_key("math"));
        assert!(toml::from_str::<Config>("max_disk = 3").is_err());
    }

    #[test]
    fn default_media_dir_is_used_when_unset() {
        let root = std::env::temp_dir().join("lib");
        let paths = Paths::portable(root.clone()).unwrap();
        let cfg = Config::default();
        assert_eq!(cfg.media_dirs(&paths), vec![root.join("media")]);
    }
}
