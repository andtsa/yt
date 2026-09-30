//! End-to-end test of the `yt` binary against a fake yt-dlp (a shell script, so
//! Unix only; the logic it exercises is platform-independent and covered by
//! unit tests everywhere).
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

const FAKE_YTDLP: &str = r#"#!/bin/sh
# Minimal stand-in for yt-dlp: -J prints metadata, otherwise "downloads".
json=0; dir=.; urls=""
while [ $# -gt 0 ]; do
    case "$1" in
        --version) echo 2099.01.01; exit 0 ;;
        -J) json=1 ;;
        -P) dir="$2"; shift ;;
        -f|-o|--proxy|--merge-output-format) shift ;;
        http*) urls="$urls $1" ;;
    esac
    shift
done
for u in $urls; do
    id="${u##*v=}"
    if [ $json = 1 ]; then
        printf '{"id":"%s","title":"Video %s","channel":"Chan","duration":600,"webpage_url":"%s"}\n' "$id" "$id" "$u"
    else
        printf 'fake video' > "$dir/Video $id [$id].mp4"
    fi
done
"#;

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let script = root.join("fake-yt-dlp");
        fs::write(&script, FAKE_YTDLP).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir(root.join("lib")).unwrap();
        fs::write(
            root.join("lib/config.toml"),
            format!(
                "ytdlp = {:?}\nplayer = \"system\"\nmedia_dirs = [\"media\"]\n",
                script.to_str().unwrap()
            ),
        )
        .unwrap();
        fs::create_dir(root.join("lib/media")).unwrap();
        fs::write(root.join("lib/library.json"), "{\"videos\": []}").unwrap();
        Env { dir }
    }

    fn lib(&self) -> std::path::PathBuf {
        self.dir.path().join("lib")
    }

    /// Run `yt`, isolated from the real home directory; panics unless it
    /// succeeds. Returns stdout.
    fn yt(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "yt {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let home = self.dir.path().join("home");
        Command::new(env!("CARGO_BIN_EXE_yt"))
            .args(args)
            .env("YT_HOME", self.lib())
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .output()
            .unwrap()
    }
}

fn file_exists(dir: &Path, id: &str) -> bool {
    dir.join(format!("Video {id} [{id}].mp4")).exists()
}

#[test]
fn add_get_mark_rm_forget() {
    let env = Env::new();
    let media = env.lib().join("media");
    // The second ID starts with '-', which must not be taken for a flag.
    let a = "https://www.youtube.com/watch?v=aaaaaaaaaaa";
    let b = "https://www.youtube.com/watch?v=-bbbbbbbbbb";

    let out = env.yt(&["add", a, b, "-l", "math"]);
    assert_eq!(out.matches("added").count(), 2, "{out}");
    let out = env.yt(&["ls"]);
    assert!(out.contains("2 video(s), 0 downloaded"), "{out}");

    env.yt(&["get", "-l", "math"]);
    assert!(file_exists(&media, "aaaaaaaaaaa"));
    assert!(file_exists(&media, "-bbbbbbbbbb"));
    let out = env.yt(&["ls", "--local"]);
    assert!(out.contains("2 video(s), 2 downloaded"), "{out}");

    env.yt(&["mark", "watched", "aaaaaaaaaaa"]);
    env.yt(&["tag", "physics", "-bbbbbbbbbb"]);
    let out = env.yt(&["lists"]);
    assert!(out.contains("physics"), "{out}");
    let out = env.yt(&["ls", "-s", "watched"]);
    assert!(out.contains("1 video(s)"), "{out}");

    // Deleting keeps the entry, so it can be downloaded again.
    env.yt(&["rm", "--all", "-s", "watched"]);
    assert!(!file_exists(&media, "aaaaaaaaaaa"));
    let out = env.yt(&["info", "aaaaaaaaaaa"]);
    assert!(out.contains("not downloaded"), "{out}");
    env.yt(&["get", "aaaaaaaaaaa"]);
    assert!(file_exists(&media, "aaaaaaaaaaa"));

    let out = env.yt(&["rm", "--forget", "-bbbbbbbbbb"]);
    assert!(out.contains("forgot 1"), "{out}");
    assert!(!file_exists(&media, "-bbbbbbbbbb"));
    let out = env.yt(&["ls"]);
    assert!(out.contains("1 video(s)"), "{out}");
}

#[test]
fn notes_and_ambiguous_search() {
    let env = Env::new();
    env.yt(&[
        "add",
        "https://www.youtube.com/watch?v=aaaaaaaaaaa",
        "https://www.youtube.com/watch?v=ccccccccccc",
    ]);
    let out = env.run(&["info", "video"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("matches 2 videos"));

    env.yt(&["note", "ccccccccccc", "revisit", "later"]);
    let out = env.yt(&["note", "ccccccccccc"]);
    assert!(out.contains("revisit later"), "{out}");
}

#[test]
fn doctor_reports_configured_ytdlp() {
    let env = Env::new();
    let out = env.yt(&["doctor"]);
    assert!(out.contains("2099.01.01"), "{out}");
    assert!(out.contains("portable"), "{out}");
}
