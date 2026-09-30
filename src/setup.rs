//! `yt setup` (first-run setup, installing yt-dlp) and `yt doctor` (checking
//! the dependencies).

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use sha2::Digest;
use sha2::Sha256;

use crate::config;
use crate::config::Config;
use crate::library::Library;
use crate::paths::Paths;
use crate::player::Player;
use crate::ytdlp;
use crate::ytdlp::Source;
use crate::ytdlp::YtDlp;

const RELEASES: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";

/// Name of the standalone yt-dlp build for this platform in yt-dlp's GitHub
/// releases. These bundle Python, so nothing else is needed.
fn release_asset() -> Option<&'static str> {
    let musl = cfg!(target_env = "musl");
    Some(match (env::consts::OS, env::consts::ARCH) {
        ("macos", _) => "yt-dlp_macos",
        ("linux", "x86_64") if musl => "yt-dlp_musllinux",
        ("linux", "aarch64") if musl => "yt-dlp_musllinux_aarch64",
        ("linux", "x86_64") => "yt-dlp_linux",
        ("linux", "aarch64") => "yt-dlp_linux_aarch64",
        ("windows", "x86_64") => "yt-dlp.exe",
        ("windows", "x86") => "yt-dlp_x86.exe",
        ("windows", "aarch64") => "yt-dlp_arm64.exe",
        _ => return None,
    })
}

pub struct SetupOptions {
    /// Install the managed yt-dlp even if one is already available.
    pub install_ytdlp: bool,
    /// Replace the managed yt-dlp with the latest release.
    pub update: bool,
}

pub fn setup(paths: &Paths, opts: SetupOptions) -> Result<()> {
    if !paths.config.exists() {
        if let Some(dir) = paths.config.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&paths.config, config::TEMPLATE)?;
        println!("created {}", paths.config.display());
    }
    if !paths.library.exists() {
        Library::default().save(&paths.library)?;
        println!("created {}", paths.library.display());
    }
    let cfg = Config::load(paths)?;
    let dir = cfg.download_dir(paths)?;
    println!("videos go to {}", dir.display());

    let found = YtDlp::locate(&cfg, paths);
    match &found {
        Some(y) if opts.update && y.source != Source::Managed => println!(
            "yt-dlp at {} is {}; update it the way it was installed (or run `{} -U`)",
            y.program.display(),
            y.source,
            y.program.display()
        ),
        Some(y) if !opts.update && !opts.install_ytdlp => {
            println!("using yt-dlp at {} ({})", y.program.display(), y.source)
        }
        _ => install_ytdlp(paths)?,
    }
    println!();
    doctor(paths)
}

/// Download the latest standalone yt-dlp into the managed bin dir, verifying
/// its SHA-256 against the release's checksum file.
pub fn install_ytdlp(paths: &Paths) -> Result<()> {
    let Some(asset) = release_asset() else {
        bail!(
            "there's no standalone yt-dlp build for {}/{}: install yt-dlp yourself (https://github.com/yt-dlp/yt-dlp#installation)",
            env::consts::OS,
            env::consts::ARCH
        );
    };
    println!("downloading {asset} from {RELEASES}…");
    let sums = ureq::get(format!("{RELEASES}/SHA2-256SUMS"))
        .call()?
        .body_mut()
        .read_to_string()?;
    let expected = sums
        .lines()
        .find_map(|l| {
            let (hash, name) = l.split_once(char::is_whitespace)?;
            (name.trim() == asset).then(|| hash.to_ascii_lowercase())
        })
        .with_context(|| format!("{asset} is missing from SHA2-256SUMS"))?;
    let bytes = ureq::get(format!("{RELEASES}/{asset}"))
        .call()?
        .body_mut()
        .with_config()
        .limit(500 * 1024 * 1024)
        .read_to_vec()?;
    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}");
    }

    let bin_dir = paths.bin_dir();
    fs::create_dir_all(&bin_dir)?;
    let target = bin_dir.join(ytdlp::managed_name());
    let tmp = target.with_extension("download");
    fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&tmp, &target)?;
    println!(
        "installed yt-dlp ({} MB) at {}",
        bytes.len() / 1_000_000,
        target.display()
    );
    Ok(())
}

pub fn doctor(paths: &Paths) -> Result<()> {
    let cfg = Config::load(paths)?;
    let ok = |label: &str, detail: String| println!("  ✓ {label:<9} {detail}");
    let bad = |label: &str, detail: String| println!("  ✗ {label:<9} {detail}");

    println!(
        "library ({}):",
        if paths.portable {
            "portable"
        } else {
            "standard location"
        }
    );
    let exists = |p: &Path| if p.exists() { "" } else { " (not created yet)" };
    ok(
        "config",
        format!("{}{}", paths.config.display(), exists(&paths.config)),
    );
    match Library::load(&paths.library) {
        Ok(lib) => ok(
            "library",
            format!("{} ({} videos)", paths.library.display(), lib.videos.len()),
        ),
        Err(e) => bad("library", format!("{e:#}")),
    }
    for dir in cfg.media_dirs(paths) {
        if dir.is_dir() {
            ok("media", dir.display().to_string());
        } else if cfg.media_dirs.is_empty() {
            ok(
                "media",
                format!("{} (created on first download)", dir.display()),
            );
        } else {
            bad("media", format!("{} (doesn't exist)", dir.display()));
        }
    }

    println!("\ntools:");
    match YtDlp::locate(&cfg, paths) {
        Some(y) => match y.version() {
            Ok(v) => ok(
                "yt-dlp",
                format!("{v} at {} ({})", y.program.display(), y.source),
            ),
            Err(e) => bad("yt-dlp", format!("{e:#}")),
        },
        None => bad("yt-dlp", "not found; run `yt setup`".into()),
    }
    match tool_version("ffmpeg", "-version") {
        Some(v) => ok("ffmpeg", v),
        None => bad(
            "ffmpeg",
            format!(
                "not found (needed to merge video and audio): {}",
                hint("ffmpeg")
            ),
        ),
    }
    match tool_version("deno", "--version") {
        Some(v) => ok("deno", v),
        None => bad(
            "deno",
            format!(
                "not found (YouTube needs it for some videos): {}",
                hint("deno")
            ),
        ),
    }

    println!("\nplayer:");
    match Player::resolve(&cfg, paths) {
        Ok(Player::Vlc(path)) => ok("VLC", format!("{} (tracks progress)", path.display())),
        Ok(Player::QuickTime) => ok("QuickTime", "tracks progress".into()),
        Ok(Player::System) => bad(
            "system",
            format!(
                "opens videos but can't track progress; for that install VLC: {}",
                hint("vlc")
            ),
        ),
        Err(e) => bad("player", format!("{e:#}")),
    }
    Ok(())
}

/// First line of `<tool> <flag>`, if the tool is on PATH.
fn tool_version(tool: &str, flag: &str) -> Option<String> {
    let path = which::which(tool).ok()?;
    let out = Command::new(&path).arg(flag).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.lines().next().unwrap_or_default().trim().to_string())
}

fn hint(tool: &str) -> &'static str {
    match (env::consts::OS, tool) {
        ("macos", "ffmpeg") => "brew install ffmpeg",
        ("macos", "deno") => "brew install deno",
        ("macos", "vlc") => "brew install --cask vlc",
        ("windows", "ffmpeg") => "winget install Gyan.FFmpeg",
        ("windows", "deno") => "winget install DenoLand.Deno",
        ("windows", "vlc") => "winget install VideoLAN.VLC",
        (_, "ffmpeg") => "install the `ffmpeg` package (e.g. `sudo apt install ffmpeg`)",
        (_, "deno") => "curl -fsSL https://deno.land/install.sh | sh",
        (_, "vlc") => "install the `vlc` package (e.g. `sudo apt install vlc`)",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_platform_has_a_release_asset() {
        if cfg!(any(target_os = "macos", windows)) || env::consts::ARCH == "x86_64" {
            assert!(release_asset().is_some());
        }
    }
}
