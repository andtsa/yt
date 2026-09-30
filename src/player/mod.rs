//! Video players. QuickTime (macOS) and VLC report playback progress, so `yt
//! watch` and `yt sync` can record where you stopped; "system" just opens the
//! file with the default app.

mod quicktime;
pub mod vlc;

use std::path::Path;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::config::Config;
use crate::config::PlayerChoice;
use crate::paths::Paths;

/// A video a player currently has open.
pub struct Playing {
    pub id: String,
    pub position: f64,
    pub duration: f64,
}

pub enum Player {
    QuickTime,
    Vlc(std::path::PathBuf),
    System,
}

impl Player {
    pub fn resolve(cfg: &Config, paths: &Paths) -> Result<Self> {
        let vlc = || vlc::locate(cfg.vlc.as_deref().map(|p| paths.resolve(p)));
        Ok(match cfg.player {
            PlayerChoice::Auto if cfg!(target_os = "macos") => Player::QuickTime,
            PlayerChoice::Auto => vlc().map_or(Player::System, Player::Vlc),
            PlayerChoice::Quicktime if !cfg!(target_os = "macos") => {
                bail!("QuickTime is only available on macOS; set player = \"vlc\" or \"system\"")
            }
            PlayerChoice::Quicktime => Player::QuickTime,
            PlayerChoice::Vlc => Player::Vlc(vlc().context(
                "VLC not found: install it (see `yt doctor`) or set `vlc = \"<path>\"` in config.toml",
            )?),
            PlayerChoice::System => Player::System,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Player::QuickTime => "QuickTime",
            Player::Vlc(_) => "VLC",
            Player::System => "the default video app",
        }
    }

    pub fn tracks_progress(&self) -> bool {
        !matches!(self, Player::System)
    }

    pub fn can_play(&self, file: &Path) -> bool {
        match self {
            Player::QuickTime => file
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ["mp4", "m4v", "mov"].contains(&e.to_ascii_lowercase().as_str())),
            _ => true,
        }
    }

    pub fn open(&self, file: &Path, id: &str, start_secs: u64, paths: &Paths) -> Result<()> {
        match self {
            Player::QuickTime => quicktime::open(file, id, start_secs),
            Player::Vlc(vlc) => vlc::open(vlc, file, id, start_secs, &paths.machine),
            Player::System => open_with_default_app(file),
        }
    }

    pub fn playing(&self, paths: &Paths) -> Vec<Playing> {
        match self {
            Player::QuickTime => quicktime::documents().unwrap_or_default(),
            Player::Vlc(_) => vlc::playing(&paths.machine),
            Player::System => vec![],
        }
    }
}

/// Everything any player is playing, whichever player is configured.
pub fn all_playing(paths: &Paths) -> Vec<Playing> {
    let mut playing = vlc::playing(&paths.machine);
    if cfg!(target_os = "macos") {
        playing.extend(quicktime::documents().unwrap_or_default());
    }
    playing
}

fn open_with_default_app(file: &Path) -> Result<()> {
    let status = if cfg!(target_os = "macos") {
        Command::new("open").arg(file).status()
    } else if cfg!(windows) {
        // The empty argument is the window title `start` expects first.
        Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(file)
            .status()
    } else {
        Command::new("xdg-open").arg(file).status()
    }
    .context("opening the video")?;
    if !status.success() {
        bail!("couldn't open {} ({status})", file.display());
    }
    Ok(())
}
