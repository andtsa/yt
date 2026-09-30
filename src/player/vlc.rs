//! VLC, controlled through its built-in web interface (the Lua "http"
//! interface, part of every desktop build on macOS, Linux and Windows).
//!
//! `yt` starts VLC with the interface listening on a random localhost port
//! with a random password, and saves both in a session file so that later
//! commands (`yt sync`, the next `yt watch`) can find it again.
//! `/requests/status.json` reports the file name, position and length.

use std::env;
use std::fs;
use std::hash::BuildHasher;
use std::hash::RandomState;
use std::net::Ipv4Addr;
use std::net::TcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use directories::BaseDirs;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use super::Playing;
use crate::media;

const SESSION_FILE: &str = "vlc-session.json";

#[derive(Serialize, Deserialize)]
struct Session {
    port: u16,
    password: String,
}

/// The VLC executable (or app bundle on macOS): the configured path, else the
/// usual install locations, else `vlc` on PATH.
pub fn locate(configured: Option<PathBuf>) -> Option<PathBuf> {
    if configured.is_some() {
        return configured;
    }
    let mut candidates = vec![];
    if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from("/Applications/VLC.app"));
        if let Some(home) = BaseDirs::new() {
            candidates.push(home.home_dir().join("Applications/VLC.app"));
        }
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = env::var_os(var) {
                candidates.push(PathBuf::from(dir).join(r"VideoLAN\VLC\vlc.exe"));
            }
        }
    }
    candidates
        .into_iter()
        .find(|p| p.exists())
        .or_else(|| which::which("vlc").ok())
}

/// Play `file` from `start_secs`, in the VLC started by an earlier `yt watch`
/// if it's still running, otherwise in a new one.
pub fn open(vlc: &Path, file: &Path, id: &str, start_secs: u64, state_dir: &Path) -> Result<()> {
    if let Some(session) = load_session(state_dir)
        && request(&session, "").is_ok()
    {
        let input = file.to_str().context("non-UTF-8 path")?;
        request(
            &session,
            &format!("command=in_play&input={}", percent_encode(input)),
        )?;
        if wait_until_playing(&session, id) && start_secs > 0 {
            request(&session, &format!("command=seek&val={start_secs}"))?;
        }
        return Ok(());
    }

    let session = Session {
        port: free_port()?,
        password: random_password(),
    };
    let mut args = vec![
        "--extraintf=http".to_string(),
        "--http-host=127.0.0.1".to_string(),
        format!("--http-port={}", session.port),
        format!("--http-password={}", session.password),
        // Quit at the end, so that `yt watch` notices the video is over.
        "--play-and-exit".to_string(),
    ];
    if cfg!(windows) {
        // Otherwise an already running VLC takes the file, without our web
        // interface. (macOS VLC rejects this option, and Linux defaults to it.)
        args.push("--no-one-instance".to_string());
    }

    // An option after the file applies to that file only; `--start-time` would
    // also apply to every video later opened in the same VLC.
    let item_start = format!(":start-time={start_secs}");

    if cfg!(target_os = "macos") {
        // Started directly from a terminal, the macOS app never becomes active
        // and playback stalls at 0:00, so go through Launch Services.
        let app = vlc
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .map(|p| p.as_os_str().to_owned())
            .unwrap_or_else(|| "VLC".into());
        let status = Command::new("open")
            .arg("-n")
            .arg("-a")
            .arg(app)
            .arg("--args")
            .args(&args)
            .arg(file)
            .arg(&item_start)
            .status()
            .context("running open")?;
        if !status.success() {
            bail!("couldn't start VLC ({status})");
        }
    } else {
        Command::new(vlc)
            .args(&args)
            .arg(file)
            .arg(&item_start)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting {}", vlc.display()))?;
    }
    fs::create_dir_all(state_dir)?;
    fs::write(
        state_dir.join(SESSION_FILE),
        serde_json::to_string(&session)?,
    )?;
    if !wait_until_playing(&session, id) {
        eprintln!("warning: VLC's web interface isn't answering, progress may not be tracked");
    }
    Ok(())
}

/// What the VLC started by `yt` is playing, if anything.
pub fn playing(state_dir: &Path) -> Vec<Playing> {
    load_session(state_dir)
        .and_then(|s| request(&s, "").ok())
        .and_then(|status| parse_status(&status))
        .into_iter()
        .collect()
}

fn parse_status(status: &Value) -> Option<Playing> {
    if status.get("state")?.as_str()? == "stopped" {
        return None;
    }
    let name = status
        .pointer("/information/category/meta/filename")?
        .as_str()?;
    Some(Playing {
        id: media::id_from_path(Path::new(name))?,
        position: status.get("time")?.as_f64()?,
        duration: status.get("length")?.as_f64()?,
    })
}

fn wait_until_playing(session: &Session, id: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let current = request(session, "").ok().and_then(|s| parse_status(&s));
        if current.is_some_and(|p| p.id == id) {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    false
}

fn load_session(state_dir: &Path) -> Option<Session> {
    let text = fs::read_to_string(state_dir.join(SESSION_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn request(session: &Session, query: &str) -> Result<Value> {
    let mut url = format!("http://127.0.0.1:{}/requests/status.json", session.port);
    if !query.is_empty() {
        url = format!("{url}?{query}");
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .into();
    let auth = base64(format!(":{}", session.password).as_bytes());
    let body = agent
        .get(&url)
        .header("Authorization", &format!("Basic {auth}"))
        .call()?
        .body_mut()
        .read_to_string()?;
    Ok(serde_json::from_str(&body)?)
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

fn random_password() -> String {
    let a = RandomState::new().hash_one(std::process::id());
    let b = RandomState::new().hash_one(Instant::now());
    format!("{a:016x}{b:016x}")
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b":secret"), "OnNlY3JldA==");
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(percent_encode("/a b/c？.mp4"), "%2Fa%20b%2Fc%EF%BC%9F.mp4");
    }

    #[test]
    fn status_parsing() {
        let status = json!({
            "state": "playing", "time": 63, "length": 338,
            "information": {"category": {"meta": {"filename": "Higher order derivatives [BLkz5LGWihw].mp4"}}},
        });
        let p = parse_status(&status).unwrap();
        assert_eq!(
            (p.id.as_str(), p.position, p.duration),
            ("BLkz5LGWihw", 63.0, 338.0)
        );
        assert!(parse_status(&json!({"state": "stopped", "time": 0, "length": 0})).is_none());
    }
}
