//! Finding video files on disk. Whether a video is downloaded is never stored:
//! it is whatever file named "... [<id>].<ext>" exists in one of the media
//! dirs.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const VIDEO_EXTS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm"];

pub struct LocalFile {
    pub path: PathBuf,
    pub size: u64,
}

pub type MediaIndex = HashMap<String, LocalFile>;

pub fn scan(dirs: &[PathBuf]) -> MediaIndex {
    let mut index = MediaIndex::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = id_from_path(&path) else {
                continue;
            };
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            index.entry(id).or_insert(LocalFile { path, size });
        }
    }
    index
}

/// "Some Title [dQw4w9WgXcQ].mp4" -> "dQw4w9WgXcQ". Skips yt-dlp's
/// partial/intermediate files ("x [id].mp4.part", "x [id].f137.mp4").
pub fn id_from_path(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !VIDEO_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?.strip_suffix(']')?;
    let id = &stem[stem.rfind('[')? + 1..];
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then(|| id.to_string())
}

/// Title from a file name, for files whose metadata can no longer be fetched.
pub fn title_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match stem.rfind(" [") {
        Some(i) => stem[..i].to_string(),
        None => stem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_file_names() {
        let id = |name: &str| id_from_path(Path::new(name));
        assert_eq!(
            id("Laplace？ [j0wJBEZdwLs].mp4").as_deref(),
            Some("j0wJBEZdwLs")
        );
        assert_eq!(
            id("a [b] [-j8PzkZ70Lg].WEBM").as_deref(),
            Some("-j8PzkZ70Lg")
        );
        assert_eq!(id("x [abc].mp4.part"), None);
        assert_eq!(id("x [abc].f137.mp4"), None);
        assert_eq!(id("x [a b].mp4"), None);
        assert_eq!(id("notes.txt"), None);
    }

    #[test]
    fn scans_media_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("A [aaa].mp4"), b"12345").unwrap();
        fs::write(dir.path().join("A [aaa].mp4.part"), b"").unwrap();
        fs::write(dir.path().join("readme.md"), b"").unwrap();
        let missing = dir.path().join("unplugged");
        let index = scan(&[missing, dir.path().to_path_buf()]);
        assert_eq!(index.len(), 1);
        assert_eq!(index["aaa"].size, 5);
        assert_eq!(title_from_path(&index["aaa"].path), "A");
    }
}
