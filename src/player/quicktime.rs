//! Talking to QuickTime Player. Files are opened through Launch Services
//! (`open -a`) because QuickTime is sandboxed: a file handed over by
//! AppleScript comes without read access, and QuickTime reports it as an
//! unsupported format. AppleScript is only used to seek, play and read playback
//! positions.
//!
//! QuickTime 10.5 leaves a document's `path` empty, so documents are matched by
//! `name` (the file name, which contains "[<video id>]").

use std::path::Path;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use super::Playing;
use crate::media;

// Guarded by `is running` so that polling never launches QuickTime.
const LIST_SCRIPT: &str = r#"
if application "QuickTime Player" is running then
    tell application "QuickTime Player"
        set out to ""
        repeat with d in documents
            try
                set out to out & (name of d) & tab & ((current time of d) as text) & tab & ((duration of d) as text) & linefeed
            end try
        end repeat
        return out
    end tell
end if
return ""
"#;

const PLAY_SCRIPT: &str = r#"
on run argv
    set needle to "[" & item 1 of argv & "]"
    set t to (item 2 of argv) as integer
    tell application "QuickTime Player"
        repeat 40 times
            repeat with d in documents
                if (name of d) contains needle then
                    if t > 0 then set current time of d to t
                    play d
                    return
                end if
            end repeat
            delay 0.25
        end repeat
    end tell
    error "QuickTime did not open the video"
end run
"#;

fn osascript(script: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .args(args)
        .output()
        .context("running osascript")?;
    if !out.status.success() {
        bail!("osascript: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Documents currently open in QuickTime (empty if it isn't running).
pub fn documents() -> Result<Vec<Playing>> {
    let out = osascript(LIST_SCRIPT, &[])?;
    // AppleScript formats reals with the locale's decimal separator.
    let num = |s: &str| s.trim().replace(',', ".").parse::<f64>().unwrap_or(0.0);
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let id = media::id_from_path(Path::new(parts.next()?))?;
            Some(Playing {
                id,
                position: num(parts.next()?),
                duration: num(parts.next()?),
            })
        })
        .collect())
}

/// Open the file of video `id` in QuickTime, seek to `start_secs` and play.
pub fn open(path: &Path, id: &str, start_secs: u64) -> Result<()> {
    let status = Command::new("open")
        .args(["-a", "QuickTime Player"])
        .arg(path)
        .status()
        .context("running open")?;
    if !status.success() {
        bail!("`open -a \"QuickTime Player\"` failed ({status})");
    }
    // "--" so that IDs starting with '-' aren't taken as osascript options.
    osascript(PLAY_SCRIPT, &["--", id, &start_secs.to_string()])?;
    Ok(())
}
