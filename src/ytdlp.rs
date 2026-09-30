use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde_json::Value;

use crate::config::Config;
use crate::paths::Paths;

#[derive(Debug, Clone)]
pub struct Meta {
    pub id: String,
    pub url: String,
    pub title: String,
    pub channel: String,
    pub channel_id: Option<String>,
    pub duration: Option<u64>,
    pub upload_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Config,
    Managed,
    Path,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Source::Config => "from config.toml",
            Source::Managed => "installed by `yt setup`",
            Source::Path => "from PATH",
        })
    }
}

/// A located yt-dlp executable.
#[derive(Debug, Clone)]
pub struct YtDlp {
    pub program: PathBuf,
    pub source: Source,
}

/// File name of the yt-dlp binary managed by `yt setup`.
pub fn managed_name() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    }
}

impl YtDlp {
    /// The `ytdlp` config setting, else the copy installed by `yt setup`,
    /// else `yt-dlp` (or `ytdlp`) on PATH.
    pub fn locate(cfg: &Config, paths: &Paths) -> Option<Self> {
        if let Some(p) = &cfg.ytdlp {
            // A bare name is looked up on PATH, anything else is a path.
            let program = if p.components().count() > 1 {
                paths.resolve(p)
            } else {
                p.clone()
            };
            return Some(Self {
                program,
                source: Source::Config,
            });
        }
        let managed = paths.bin_dir().join(managed_name());
        if managed.is_file() {
            return Some(Self {
                program: managed,
                source: Source::Managed,
            });
        }
        ["yt-dlp", "ytdlp"]
            .iter()
            .find_map(|name| which::which(name).ok())
            .map(|program| Self {
                program,
                source: Source::Path,
            })
    }

    pub fn version(&self) -> Result<String> {
        let out = Command::new(&self.program)
            .arg("--version")
            .output()
            .with_context(|| format!("running {}", self.program.display()))?;
        if !out.status.success() {
            bail!("{} --version failed", self.program.display());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn command(&self, cfg: &Config) -> Command {
        let mut cmd = Command::new(&self.program);
        if let Some(proxy) = cfg.pick_proxy() {
            cmd.args(["--proxy", proxy]);
        }
        cmd
    }
}

/// Fetch metadata for videos, playlists or searches ("ytsearch5:..."), without
/// downloading. Playlists are expanded cheaply; their entries lack the channel,
/// so unless `fast` is set those are fetched individually in a second pass.
pub fn fetch(ytdlp: &YtDlp, cfg: &Config, urls: &[String], fast: bool) -> Result<Vec<Meta>> {
    let (mut metas, incomplete) = fetch_raw(ytdlp, cfg, urls, true)?;
    if incomplete.is_empty() {
        return Ok(metas);
    }
    if !fast {
        let urls: Vec<String> = incomplete.iter().map(|&i| metas[i].url.clone()).collect();
        eprintln!(
            "fetching details for {} playlist entries (use --fast to skip)…",
            urls.len()
        );
        let (full, _) = fetch_raw(ytdlp, cfg, &urls, false)?;
        for m in full {
            if let Some(slot) = metas.iter_mut().find(|x| x.id == m.id) {
                *slot = m;
            }
        }
    }
    Ok(metas)
}

/// Returns the parsed entries and the indices of entries that are missing their
/// channel.
fn fetch_raw(
    ytdlp: &YtDlp,
    cfg: &Config,
    urls: &[String],
    flat: bool,
) -> Result<(Vec<Meta>, Vec<usize>)> {
    let mut cmd = ytdlp.command(cfg);
    cmd.args(["-J", "--no-warnings", "--ignore-errors"]);
    if flat {
        cmd.arg("--flat-playlist");
    }
    let out = cmd
        .args(urls)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running {}", ytdlp.program.display()))?;

    let mut metas = vec![];
    let mut incomplete = vec![];
    // One JSON document per URL.
    for line in out.stdout.split(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let v: Value = serde_json::from_slice(line).context("parsing yt-dlp output")?;
        collect(&v, None, &mut metas, &mut incomplete);
    }
    if metas.is_empty() && !out.status.success() {
        bail!("yt-dlp failed ({})", out.status);
    }
    Ok((metas, incomplete))
}

fn collect(v: &Value, parent: Option<&Value>, out: &mut Vec<Meta>, incomplete: &mut Vec<usize>) {
    if let Some(entries) = v.get("entries").and_then(Value::as_array) {
        for e in entries {
            collect(e, Some(v), out, incomplete);
        }
        return;
    }
    let get = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let Some(id) = get(v, "id") else { return };
    let title = get(v, "title").unwrap_or_else(|| id.clone());
    if title == "[Private video]" || title == "[Deleted video]" {
        eprintln!("skipping {id}: {title}");
        return;
    }
    let channel = get(v, "channel").or_else(|| get(v, "uploader"));
    if channel.is_none() {
        incomplete.push(out.len());
    }
    // Fallback for --fast: the playlist owner, which is usually but not always
    // right.
    let channel = channel
        .or_else(|| parent.and_then(|p| get(p, "channel").or_else(|| get(p, "uploader"))))
        .unwrap_or_else(|| "?".into());
    out.push(Meta {
        url: get(v, "webpage_url")
            .or_else(|| get(v, "url"))
            .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={id}")),
        channel_id: get(v, "channel_id"),
        duration: v
            .get("duration")
            .and_then(Value::as_f64)
            .map(|d| d.round() as u64),
        upload_date: get(v, "upload_date").map(|d| match d.len() {
            8 => format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]),
            _ => d,
        }),
        id,
        title,
        channel,
    });
}

/// Download into `dir`. Returns whether yt-dlp reported success for everything.
pub fn download(ytdlp: &YtDlp, cfg: &Config, dir: &Path, urls: &[&str]) -> Result<bool> {
    // The directory goes in -P rather than -o, so that characters like '%' in
    // it aren't read as template fields.
    let status = ytdlp
        .command(cfg)
        .args(["-f", &cfg.format])
        .args([
            "--merge-output-format",
            "mp4",
            "--embed-metadata",
            "--embed-chapters",
        ])
        .args(["--no-playlist", "--ignore-errors", "-P"])
        .arg(dir)
        .args(["-o", "%(title)s [%(id)s].%(ext)s"])
        .args(&cfg.extra_args)
        .args(urls)
        .status()
        .with_context(|| format!("running {}", ytdlp.program.display()))?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(v: Value) -> (Vec<Meta>, Vec<usize>) {
        let (mut out, mut incomplete) = (vec![], vec![]);
        collect(&v, None, &mut out, &mut incomplete);
        (out, incomplete)
    }

    #[test]
    fn single_video() {
        let (metas, incomplete) = parse(json!({
            "id": "j0wJBEZdwLs", "title": "Laplace", "channel": "3Blue1Brown",
            "channel_id": "UC1", "duration": 2080.4, "upload_date": "20251012",
            "webpage_url": "https://www.youtube.com/watch?v=j0wJBEZdwLs",
        }));
        assert!(incomplete.is_empty());
        let m = &metas[0];
        assert_eq!(
            (m.id.as_str(), m.channel.as_str()),
            ("j0wJBEZdwLs", "3Blue1Brown")
        );
        assert_eq!(m.duration, Some(2080));
        assert_eq!(m.upload_date.as_deref(), Some("2025-10-12"));
    }

    #[test]
    fn flat_playlist_falls_back_to_owner_and_skips_private() {
        let (metas, incomplete) = parse(json!({
            "_type": "playlist", "channel": "Owner",
            "entries": [
                {"id": "a", "title": "A", "url": "https://www.youtube.com/watch?v=a", "duration": 60},
                {"id": "b", "title": "[Private video]"},
                {"id": "c", "title": "C", "channel": "Someone"},
            ],
        }));
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].channel, "Owner");
        assert_eq!(incomplete, vec![0]);
        assert_eq!(metas[1].channel, "Someone");
        assert_eq!(metas[1].url, "https://www.youtube.com/watch?v=c");
    }
}
