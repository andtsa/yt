//! Picking videos from the library by ID, URL, search term or watchlist.

use std::collections::BTreeSet;

use anyhow::Result;
use anyhow::bail;
use clap::Args;

use crate::library::Library;
use crate::library::Status;
use crate::library::Video;

/// Which videos a command applies to.
#[derive(Args)]
pub struct Select {
    /// Video IDs, URLs, or title/channel search terms (each must match exactly
    /// one video)
    pub targets: Vec<String>,
    /// Every video in these watchlists
    #[arg(short, long = "list")]
    pub lists: Vec<String>,
    /// Restrict --list/--all to videos with this status
    #[arg(short, long)]
    pub status: Option<Status>,
    /// Every video in the library
    #[arg(short, long)]
    pub all: bool,
}

/// YouTube IDs can start with '-' (e.g. "-j8PzkZ70Lg"), which clap would read
/// as flags. Such arguments are rewritten to "id:-j8PzkZ70Lg" before parsing.
pub fn protect_id(arg: String) -> String {
    let looks_like_id = arg.len() == 11
        && arg.starts_with('-')
        && !arg.starts_with("--")
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if looks_like_id {
        format!("id:{arg}")
    } else {
        arg
    }
}

/// The video ID in a YouTube URL.
pub fn extract_id(s: &str) -> Option<&str> {
    let rest = ["v=", "youtu.be/", "/shorts/", "/live/"]
        .iter()
        .find_map(|m| s.find(m).map(|i| &s[i + m.len()..]))?;
    let end = rest.find(['&', '?', '#', '/']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// The one video matching an ID, URL, or title/channel search term.
pub fn find_one<'a>(lib: &'a Library, target: &str) -> Result<&'a Video> {
    let target = target.strip_prefix("id:").unwrap_or(target);
    let id = extract_id(target).unwrap_or(target);
    if let Some(v) = lib.get(id) {
        return Ok(v);
    }
    let needle = target.to_lowercase();
    let hits: Vec<&Video> = lib.videos.iter().filter(|v| v.matches(&needle)).collect();
    match hits.as_slice() {
        [v] => Ok(v),
        [] => bail!("no video matches {target:?}"),
        _ => {
            let mut msg = format!(
                "{target:?} matches {} videos, be more specific:",
                hits.len()
            );
            for v in hits.iter().take(10) {
                msg += &format!("\n  {}  {}  {}", v.id, v.channel, v.title);
            }
            bail!(msg)
        }
    }
}

/// IDs of the selected videos, in library order.
pub fn select(lib: &Library, sel: &Select) -> Result<Vec<String>> {
    if sel.targets.is_empty() && sel.lists.is_empty() && !sel.all {
        bail!("nothing selected: give video IDs/URLs/search terms, --list <name> or --all");
    }
    let mut picked = BTreeSet::new();
    for t in &sel.targets {
        picked.insert(find_one(lib, t)?.id.clone());
    }
    if sel.all || !sel.lists.is_empty() {
        for v in &lib.videos {
            let in_lists = sel.all || sel.lists.iter().any(|l| v.lists.contains(l));
            if in_lists && sel.status.is_none_or(|s| v.status == s) {
                picked.insert(v.id.clone());
            }
        }
    }
    Ok(lib
        .videos
        .iter()
        .filter(|v| picked.contains(&v.id))
        .map(|v| v.id.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytdlp::Meta;

    fn lib() -> Library {
        let mut lib = Library::default();
        for (id, title, list) in [
            ("j0wJBEZdwLs", "But what is a Laplace Transform?", "math"),
            (
                "-j8PzkZ70Lg",
                "Euler's Formula | Laplace Transform Prelude",
                "math",
            ),
            ("acN7E7AUHPk", "The Hydrogen Atom", "physics"),
        ] {
            let mut v = Video::from_meta(Meta {
                id: id.into(),
                url: format!("https://www.youtube.com/watch?v={id}"),
                title: title.into(),
                channel: "Channel".into(),
                channel_id: None,
                duration: None,
                upload_date: None,
            });
            v.lists.insert(list.into());
            lib.videos.push(v);
        }
        lib
    }

    fn sel(targets: &[&str], lists: &[&str], all: bool) -> Select {
        Select {
            targets: targets.iter().map(|s| s.to_string()).collect(),
            lists: lists.iter().map(|s| s.to_string()).collect(),
            status: None,
            all,
        }
    }

    #[test]
    fn protects_ids_starting_with_dash_only() {
        assert_eq!(protect_id("-j8PzkZ70Lg".into()), "id:-j8PzkZ70Lg");
        assert_eq!(protect_id("-l".into()), "-l");
        assert_eq!(protect_id("--list".into()), "--list");
        assert_eq!(protect_id("--from-start".into()), "--from-start");
        assert_eq!(protect_id("j0wJBEZdwLs".into()), "j0wJBEZdwLs");
    }

    #[test]
    fn extracts_ids_from_urls() {
        assert_eq!(
            extract_id("https://www.youtube.com/watch?v=abc&t=3"),
            Some("abc")
        );
        assert_eq!(
            extract_id("https://youtu.be/-j8PzkZ70Lg?t=3"),
            Some("-j8PzkZ70Lg")
        );
        assert_eq!(extract_id("https://youtube.com/shorts/xyz"), Some("xyz"));
        assert_eq!(extract_id("laplace"), None);
    }

    #[test]
    fn finds_by_id_url_and_unique_search() {
        let lib = lib();
        assert_eq!(find_one(&lib, "id:-j8PzkZ70Lg").unwrap().id, "-j8PzkZ70Lg");
        assert_eq!(
            find_one(&lib, "https://youtu.be/acN7E7AUHPk").unwrap().id,
            "acN7E7AUHPk"
        );
        assert_eq!(find_one(&lib, "hydrogen").unwrap().id, "acN7E7AUHPk");
        let err = find_one(&lib, "laplace").unwrap_err().to_string();
        assert!(err.contains("matches 2 videos"), "{err}");
        assert!(find_one(&lib, "nothing").is_err());
    }

    #[test]
    fn selects_by_list_and_targets_in_library_order() {
        let lib = lib();
        assert_eq!(
            select(&lib, &sel(&["hydrogen"], &["math"], false)).unwrap(),
            ["j0wJBEZdwLs", "-j8PzkZ70Lg", "acN7E7AUHPk"]
        );
        assert_eq!(select(&lib, &sel(&[], &[], true)).unwrap().len(), 3);
        assert!(select(&lib, &sel(&[], &[], false)).is_err());
    }
}
