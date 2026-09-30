//! Output formatting.

use std::env;
use std::io::IsTerminal;

pub fn dur(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1}G", b / 1e9)
    } else {
        format!("{:.0}M", b / 1e6)
    }
}

pub fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).chain(['…']).collect()
    }
}

/// Whether to use ANSI colors: only on a terminal, and on Windows only in
/// Windows Terminal (the classic console shows escape codes literally).
pub fn color() -> bool {
    std::io::stdout().is_terminal()
        && env::var_os("NO_COLOR").is_none()
        && (!cfg!(windows) || env::var_os("WT_SESSION").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(dur(0), "0:00");
        assert_eq!(dur(2080), "34:40");
        assert_eq!(dur(3600 + 62), "1:01:02");
    }

    #[test]
    fn sizes() {
        assert_eq!(size(11_000_000), "11M");
        assert_eq!(size(1_100_000_000), "1.1G");
    }

    #[test]
    fn truncation_counts_chars() {
        assert_eq!(trunc("short", 10), "short");
        assert_eq!(trunc("Laplace ｜ Transform", 9), "Laplace …");
    }
}
