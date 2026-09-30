use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

use crate::ytdlp::Meta;

pub const LIBRARY_FILE: &str = "library.json";

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
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(LIBRARY_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = root.join(LIBRARY_FILE);
        let tmp = path.with_extension("json.tmp");
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.videos.iter().position(|v| v.id == id)
    }
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
