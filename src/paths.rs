//! Where the config, library, media and helper files live.
//!
//! A library is either *portable* (a folder holding `library.json` and
//! optionally `config.toml`, found through `$YT_HOME` or by walking up from the
//! current directory) or *standard*, in the OS's usual places:
//!
//! |              | macOS                                  | Linux                        | Windows                         |
//! |--------------|----------------------------------------|------------------------------|---------------------------------|
//! | config       | ~/Library/Application Support/youtube-at-home | ~/.config/youtube-at-home | %APPDATA%\youtube-at-home\config |
//! | library      | same as config                         | ~/.local/share/youtube-at-home | %APPDATA%\youtube-at-home\data |
//! | media        | ~/Movies/youtube-at-home               | ~/Videos/youtube-at-home     | %USERPROFILE%\Videos\youtube-at-home |
//!
//! Per-machine files (the managed yt-dlp binary, the VLC session) always live
//! in the standard data directory.

use std::env;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use directories::BaseDirs;
use directories::ProjectDirs;
use directories::UserDirs;

pub const CONFIG_FILE: &str = "config.toml";
pub const LIBRARY_FILE: &str = "library.json";
const APP: &str = "youtube-at-home";

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub library: PathBuf,
    /// Relative `media_dirs` are resolved against this (the config's folder).
    pub base: PathBuf,
    /// Used when the config doesn't set `media_dirs`.
    pub default_media: PathBuf,
    /// Per-machine data: the managed yt-dlp and the player session.
    pub machine: PathBuf,
    pub portable: bool,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        if let Some(home) = env::var_os("YT_HOME") {
            return Self::portable(PathBuf::from(home));
        }
        let cwd = env::current_dir()?;
        if let Some(dir) = cwd.ancestors().find(|d| d.join(LIBRARY_FILE).is_file()) {
            return Self::portable(dir.to_path_buf());
        }
        Self::standard()
    }

    pub fn portable(root: PathBuf) -> Result<Self> {
        Ok(Self {
            config: root.join(CONFIG_FILE),
            library: root.join(LIBRARY_FILE),
            default_media: root.join("media"),
            base: root,
            machine: project_dirs()?.data_dir().to_path_buf(),
            portable: true,
        })
    }

    fn standard() -> Result<Self> {
        let dirs = project_dirs()?;
        let data = dirs.data_dir().to_path_buf();
        let default_media = UserDirs::new()
            .and_then(|u| u.video_dir().map(|v| v.join(APP)))
            .unwrap_or_else(|| data.join("media"));
        Ok(Self {
            config: dirs.config_dir().join(CONFIG_FILE),
            library: data.join(LIBRARY_FILE),
            base: dirs.config_dir().to_path_buf(),
            default_media,
            machine: data,
            portable: false,
        })
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.machine.join("bin")
    }

    /// Resolve a configured path: `~` is the home directory, relative paths
    /// are relative to the config's folder.
    pub fn resolve(&self, p: &Path) -> PathBuf {
        if let Ok(rest) = p.strip_prefix("~")
            && let Some(home) = BaseDirs::new()
        {
            return home.home_dir().join(rest);
        }
        // Collecting components drops interior "." segments.
        self.base.join(p).components().collect()
    }
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", APP).context("couldn't determine the home directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_relative_and_home() {
        let root = env::temp_dir().join("lib");
        let paths = Paths::portable(root.clone()).unwrap();
        assert_eq!(paths.resolve(Path::new("media")), root.join("media"));
        assert_eq!(paths.resolve(Path::new("./media")), root.join("media"));
        let abs = env::temp_dir().join("elsewhere");
        assert_eq!(paths.resolve(&abs), abs);
        let home = BaseDirs::new().unwrap().home_dir().to_path_buf();
        assert_eq!(paths.resolve(Path::new("~/v")), home.join("v"));
    }
}
