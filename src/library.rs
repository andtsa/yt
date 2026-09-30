use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

use crate::ytdlp::Meta;

/// Rough size of a 1080p H.264 download, used until a video has actually been
/// downloaded once.
const EST_BYTES_PER_SEC: u64 = 100_000;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Todo,
    Partial,
    Watched,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Video {
    pub id: String,
    pub url: String,
    pub title: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_date: Option<String>,
    /// Size of the last downloaded file (kept after deletion, for estimates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub lists: BTreeSet<String>,
    #[serde(default)]
    pub status: Status,
    /// Playback position in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    pub added: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_watched: Option<String>,
}

impl Video {
    pub fn from_meta(m: Meta) -> Self {
        Self {
            id: m.id,
            url: m.url,
            title: m.title,
            channel: m.channel,
            channel_id: m.channel_id,
            duration: m.duration,
            upload_date: m.upload_date,
            size: None,
            lists: BTreeSet::new(),
            status: Status::Todo,
            position: None,
            notes: String::new(),
            added: today(),
            last_watched: None,
        }
    }

    /// Replace the metadata that identifies the video, keeping status, lists
    /// and notes.
    pub fn relink(&mut self, m: Meta) {
        self.id = m.id;
        self.url = m.url;
        self.title = m.title;
        self.channel = m.channel;
        self.channel_id = m.channel_id;
        self.duration = m.duration.or(self.duration);
        self.upload_date = m.upload_date;
        self.size = None;
    }

    pub fn est_size(&self) -> u64 {
        self.size
            .or(self.duration.map(|d| d * EST_BYTES_PER_SEC))
            .unwrap_or(0)
    }

    pub fn matches(&self, needle_lower: &str) -> bool {
        self.title.to_lowercase().contains(needle_lower)
            || self.channel.to_lowercase().contains(needle_lower)
    }

    pub fn add_note(&mut self, text: &str) {
        if !self.notes.is_empty() {
            self.notes.push('\n');
        }
        self.notes.push_str(&format!("[{}] {}", today(), text));
    }

    /// Record a playback position reported by the player. Returns whether
    /// anything changed.
    pub fn record_position(&mut self, pos: f64, dur: f64) -> bool {
        if self.status == Status::Watched || pos < 5.0 {
            return false;
        }
        let near_end = dur > 0.0 && (pos >= dur * 0.95 || (dur - pos < 30.0 && pos > dur * 0.5));
        if near_end {
            self.status = Status::Watched;
            self.position = None;
            return true;
        }
        let pos = pos as u64;
        if self.status == Status::Partial && self.position == Some(pos) {
            return false;
        }
        self.status = Status::Partial;
        self.position = Some(pos);
        true
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Library {
    pub videos: Vec<Video>,
}

impl Library {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Atomically replace the library file (write to a temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        fs::write(&tmp, text)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Video> {
        self.videos.iter().find(|v| v.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Video> {
        self.videos.iter_mut().find(|v| v.id == id)
    }
}

/// Exclusive lock on the library, held while it is reloaded, changed and saved,
/// so that concurrent `yt` commands don't overwrite each other's changes.
/// Released when dropped.
pub fn lock(path: &Path) -> Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let lock_path = path.with_extension("json.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    file.lock().context("locking the library")?;
    Ok(file)
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(duration: Option<u64>) -> Video {
        Video::from_meta(Meta {
            id: "abc".into(),
            url: "https://www.youtube.com/watch?v=abc".into(),
            title: "Title".into(),
            channel: "Channel".into(),
            channel_id: None,
            duration,
            upload_date: None,
        })
    }

    #[test]
    fn record_position_progresses_to_watched() {
        let mut v = video(Some(1000));
        assert!(
            !v.record_position(2.0, 1000.0),
            "the first seconds are ignored"
        );
        assert!(v.record_position(300.4, 1000.0));
        assert_eq!((v.status, v.position), (Status::Partial, Some(300)));
        assert!(
            !v.record_position(300.9, 1000.0),
            "same second is not a change"
        );
        assert!(v.record_position(960.0, 1000.0));
        assert_eq!((v.status, v.position), (Status::Watched, None));
        assert!(
            !v.record_position(10.0, 1000.0),
            "rewatching keeps it watched"
        );
    }

    #[test]
    fn near_end_of_short_video() {
        let mut v = video(Some(100));
        assert!(v.record_position(40.0, 100.0));
        assert_eq!(v.status, Status::Partial);
        assert!(v.record_position(80.0, 100.0));
        assert_eq!(v.status, Status::Watched);
    }

    #[test]
    fn size_estimate() {
        assert_eq!(video(Some(10)).est_size(), 1_000_000);
        let mut v = video(Some(10));
        v.size = Some(42);
        assert_eq!(v.est_size(), 42);
        assert_eq!(video(None).est_size(), 0);
    }

    #[test]
    fn save_load_roundtrip_with_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("library.json");
        let _lock = lock(&path).unwrap();
        let mut lib = Library::default();
        lib.videos.push(video(Some(10)));
        lib.get_mut("abc").unwrap().lists.insert("math".into());
        lib.save(&path).unwrap();
        let back = Library::load(&path).unwrap();
        assert_eq!(back.videos.len(), 1);
        assert!(back.get("abc").unwrap().lists.contains("math"));
        assert!(
            Library::load(&dir.path().join("missing.json"))
                .unwrap()
                .videos
                .is_empty()
        );
    }
}
