use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;

pub const CONFIG_FILE: &str = "config.toml";

/// Entry point of the yt-dlp git submodule, relative to the library root.
const YTDLP_SUBMODULE: &str = "yt-dlp/yt_dlp/__main__.py";

/// Prefer H.264 (<=1080p) + AAC, since QuickTime can't play VP9/AV1.
pub const DEFAULT_FORMAT: &str = "bv*[vcodec^=avc1][height<=1080]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b";

pub const TEMPLATE: &str = r#"# youtube-at-home config. Every key is optional.

# Directories that hold video files ("<title> [<video id>].<ext>").
# All of them are searched; new downloads go to the first one that currently exists,
# so an unplugged external drive is simply skipped.
# Relative paths are relative to this file, ~ is expanded.
media_dirs = ["media"]

# Soft budget for downloaded videos: `yt du` and `yt get` warn when it is exceeded.
# max_disk_gb = 50

# yt-dlp format selector. The default prefers H.264 <=1080p because QuickTime can't play VP9/AV1.
# format = "bv*[vcodec^=avc1][height<=1080]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b"

# A random one is used for each yt-dlp invocation. Empty = no proxy.
# proxies = ["socks5://127.0.0.1:1080"]

# Command used to run yt-dlp. By default this is the yt-dlp git submodule
# (run with python3), or `ytdlp` from $PATH if the submodule isn't checked out.
# ytdlp = "ytdlp"
# extra_args = ["--embed-subs", "--sub-langs", "en.*"]

# Optional descriptions. A watchlist also exists as soon as any video is in it.
[watchlists]
# physics = "QM, E&M, stat mech"
# math = ""
"#;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub media_dirs: Vec<PathBuf>,
    pub max_disk_gb: Option<f64>,
    pub format: String,
    pub proxies: Vec<String>,
    pub ytdlp: Option<String>,
    /// Program and leading arguments that run yt-dlp, resolved on load.
    #[serde(skip)]
    pub ytdlp_cmd: Vec<String>,
    pub extra_args: Vec<String>,
    pub watchlists: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            media_dirs: vec!["media".into()],
            max_disk_gb: None,
            format: DEFAULT_FORMAT.into(),
            proxies: vec![],
            ytdlp: None,
            ytdlp_cmd: vec![],
            extra_args: vec![],
            watchlists: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(CONFIG_FILE);
        let mut cfg: Self = if path.exists() {
            let text = fs::read_to_string(&path)?;
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        } else {
            Self::default()
        };
        cfg.ytdlp_cmd = match &cfg.ytdlp {
            Some(cmd) => vec![cmd.clone()],
            None => {
                let submodule = root.join(YTDLP_SUBMODULE);
                if submodule.exists() {
                    vec!["python3".into(), submodule.to_string_lossy().into_owned()]
                } else {
                    vec!["ytdlp".into()]
                }
            }
        };
        Ok(cfg)
    }

    pub fn media_dirs(&self, root: &Path) -> Vec<PathBuf> {
        self.media_dirs.iter().map(|d| resolve(root, d)).collect()
    }

    pub fn download_dir(&self, root: &Path) -> Result<PathBuf> {
        self.media_dirs(root)
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

fn resolve(root: &Path, p: &Path) -> PathBuf {
    if let (Ok(rest), Some(home)) = (p.strip_prefix("~"), env::var_os("HOME")) {
        return PathBuf::from(home).join(rest);
    }
    // Collecting components drops interior "." segments.
    root.join(p).components().collect()
}
