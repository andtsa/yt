use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde_json::Value;

use crate::config::Config;

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

fn command(cfg: &Config) -> Command {
    let mut cmd = Command::new(&cfg.ytdlp_cmd[0]);
    cmd.args(&cfg.ytdlp_cmd[1..]);
    if let Some(proxy) = cfg.pick_proxy() {
        cmd.args(["--proxy", proxy]);
    }
    cmd
}

/// Fetch metadata for videos, playlists or searches ("ytsearch5:..."), without
/// downloading. Playlists are expanded cheaply; their entries lack the channel,
/// so unless `fast` is set those are fetched individually in a second pass.
pub fn fetch(cfg: &Config, urls: &[String], fast: bool) -> Result<Vec<Meta>> {
    let (mut metas, incomplete) = fetch_raw(cfg, urls, true)?;
    if incomplete.is_empty() {
        return Ok(metas);
    }
    if !fast {
        let urls: Vec<String> = incomplete.iter().map(|&i| metas[i].url.clone()).collect();
        eprintln!(
            "fetching details for {} playlist entries (use --fast to skip)…",
            urls.len()
        );
        let (full, _) = fetch_raw(cfg, &urls, false)?;
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
fn fetch_raw(cfg: &Config, urls: &[String], flat: bool) -> Result<(Vec<Meta>, Vec<usize>)> {
    let mut cmd = command(cfg);
    cmd.args(["-J", "--no-warnings", "--ignore-errors"]);
    if flat {
        cmd.arg("--flat-playlist");
    }
    let out = cmd
        .args(urls)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running {}", cfg.ytdlp_cmd.join(" ")))?;

    let mut metas = vec![];
    let mut incomplete = vec![];
    // One JSON document per URL.
    for line in out.stdout.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
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
pub fn download(cfg: &Config, dir: &Path, urls: &[&str]) -> Result<bool> {
    let template = dir.join("%(title)s [%(id)s].%(ext)s");
    let status = command(cfg)
        .args(["-f", &cfg.format])
        .args([
            "--merge-output-format",
            "mp4",
            "--embed-metadata",
            "--embed-chapters",
        ])
        .args(["--no-playlist", "--ignore-errors", "-o"])
        .arg(template)
        .args(&cfg.extra_args)
        .args(urls)
        .status()
        .with_context(|| format!("running {}", cfg.ytdlp_cmd.join(" ")))?;
    Ok(status.success())
}
