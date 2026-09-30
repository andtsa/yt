mod config;
mod library;
mod media;
mod quicktime;
mod ytdlp;

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use config::CONFIG_FILE;
use config::Config;
use library::LIBRARY_FILE;
use library::Library;
use library::Status;
use library::Video;
use library::today;
use media::LocalFile;
use media::MediaIndex;

#[derive(Parser)]
#[command(name = "yt", version, about = "we have youtube at home")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create config.toml and library.json in the current directory
    Init,
    /// Add videos or whole playlists by URL (saves metadata only, nothing is
    /// downloaded)
    Add {
        #[arg(required = true)]
        urls: Vec<String>,
        /// Watchlist(s) to put the videos in
        #[arg(short, long = "list")]
        lists: Vec<String>,
        /// Attach a note
        #[arg(short, long)]
        note: Option<String>,
        /// Download right away
        #[arg(short, long)]
        get: bool,
        /// For playlists: don't fetch each video's details (channel may be the
        /// playlist owner's)
        #[arg(long)]
        fast: bool,
    },
    /// List videos (● downloaded, ○ not downloaded; ◐ partially watched, ✓
    /// watched)
    Ls {
        /// Only videos whose title or channel contains this
        search: Option<String>,
        #[arg(short, long = "list")]
        lists: Vec<String>,
        #[arg(short, long)]
        status: Option<Status>,
        /// Only downloaded videos
        #[arg(long, conflicts_with = "remote")]
        local: bool,
        /// Only videos that aren't downloaded
        #[arg(long)]
        remote: bool,
    },
    /// Show watchlists with counts and sizes
    Lists,
    /// Download videos
    Get(Select),
    /// Delete downloaded files (the library entries stay, so they can be
    /// downloaded again)
    Rm {
        #[command(flatten)]
        sel: Select,
        /// Also remove the entries from the library
        #[arg(long)]
        forget: bool,
    },
    /// Download if needed, open in QuickTime at the saved position, and track
    /// progress
    Watch {
        target: String,
        #[arg(long)]
        from_start: bool,
        /// Don't wait around to track progress (run `yt sync` before closing
        /// the window)
        #[arg(short, long)]
        detach: bool,
    },
    /// Save playback positions of the videos currently open in QuickTime
    Sync,
    /// Set the watch status
    Mark {
        #[arg(value_name = "STATUS")]
        to: Status,
        #[command(flatten)]
        sel: Select,
    },
    /// Add videos to a watchlist
    Tag {
        /// which watchlist
        list: String,
        #[command(flatten)]
        sel: Select,
    },
    /// Remove videos from a watchlist
    Untag {
        list: String,
        #[command(flatten)]
        sel: Select,
    },
    /// Show a video's notes, or append one
    Note { target: String, text: Vec<String> },
    /// Show everything known about a video
    Info { target: String },
    /// Disk usage and suggestions for what to delete
    Du,
    /// Add video files already in the media dirs to the library
    Import {
        #[arg(short, long = "list")]
        lists: Vec<String>,
    },
    /// Search YouTube for a video by its saved channel + title (e.g. when the
    /// link died)
    Find { target: String },
    /// Point an entry at a new URL, keeping its status, watchlists and notes
    Relink { target: String, url: String },
}

/// Which videos a command applies to.
#[derive(Args)]
struct Select {
    /// Video IDs, URLs, or title/channel search terms (each must match exactly
    /// one video)
    targets: Vec<String>,
    /// Every video in these watchlists
    #[arg(short, long = "list")]
    lists: Vec<String>,
    /// Restrict --list/--all to videos with this status
    #[arg(short, long)]
    status: Option<Status>,
    /// Every video in the library
    #[arg(short, long)]
    all: bool,
}

struct Ctx {
    root: PathBuf,
    cfg: Config,
    lib: Library,
}

impl Ctx {
    fn media(&self) -> MediaIndex {
        media::scan(&self.cfg.media_dirs(&self.root))
    }

    fn save(&self) -> Result<()> {
        self.lib.save(&self.root)
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse_from(env::args().map(|a| protect_id(&a)));
    if let Cmd::Init = cli.cmd {
        return init();
    }
    let root = find_root()?;
    let mut ctx = Ctx {
        cfg: Config::load(&root)?,
        lib: Library::load(&root)?,
        root,
    };

    match cli.cmd {
        Cmd::Init => unreachable!(),
        Cmd::Add {
            urls,
            lists,
            note,
            get,
            fast,
        } => add(&mut ctx, &urls, &lists, note.as_deref(), get, fast),
        Cmd::Ls {
            search,
            lists,
            status,
            local,
            remote,
        } => {
            let _ = sync(&mut ctx, true);
            ls(&ctx, search.as_deref(), &lists, status, local, remote);
            Ok(())
        }
        Cmd::Lists => {
            show_lists(&ctx);
            Ok(())
        }
        Cmd::Get(sel) => {
            let idxs = select(&ctx.lib, &sel)?;
            download(&mut ctx, &idxs)
        }
        Cmd::Rm { sel, forget } => rm(&mut ctx, &sel, forget),
        Cmd::Watch {
            target,
            from_start,
            detach,
        } => watch(&mut ctx, &target, from_start, detach),
        Cmd::Sync => sync(&mut ctx, false),
        Cmd::Mark { to: status, sel } => {
            for i in select(&ctx.lib, &sel)? {
                let v = &mut ctx.lib.videos[i];
                v.status = status;
                if status != Status::Partial {
                    v.position = None;
                }
                println!("{:?}  {}  {}", status, v.id, v.title);
            }
            ctx.save()
        }
        Cmd::Tag { list, sel } => {
            let idxs = select(&ctx.lib, &sel)?;
            for &i in &idxs {
                ctx.lib.videos[i].lists.insert(list.clone());
            }
            println!("{} video(s) in {list}", idxs.len());
            ctx.save()
        }
        Cmd::Untag { list, sel } => {
            let idxs = select(&ctx.lib, &sel)?;
            for &i in &idxs {
                ctx.lib.videos[i].lists.remove(&list);
            }
            println!("{} video(s) removed from {list}", idxs.len());
            ctx.save()
        }
        Cmd::Note { target, text } => {
            let i = find_one(&ctx.lib, &target)?;
            let v = &mut ctx.lib.videos[i];
            if text.is_empty() {
                println!(
                    "{}",
                    if v.notes.is_empty() {
                        "(no notes)"
                    } else {
                        &v.notes
                    }
                );
                return Ok(());
            }
            v.add_note(&text.join(" "));
            ctx.save()
        }
        Cmd::Info { target } => {
            info(&ctx, find_one(&ctx.lib, &target)?);
            Ok(())
        }
        Cmd::Du => {
            du(&ctx);
            Ok(())
        }
        Cmd::Import { lists } => import(&mut ctx, &lists),
        Cmd::Find { target } => find(&ctx, &target),
        Cmd::Relink { target, url } => relink(&mut ctx, &target, &url),
    }
}

fn init() -> Result<()> {
    let dir = env::current_dir()?;
    if !dir.join(CONFIG_FILE).exists() {
        fs::write(dir.join(CONFIG_FILE), config::TEMPLATE)?;
        println!("created {CONFIG_FILE}");
    }
    fs::create_dir_all(dir.join("media"))?;
    if !dir.join(LIBRARY_FILE).exists() {
        Library::default().save(&dir)?;
        println!("created {LIBRARY_FILE}");
    }
    println!(
        "tip: `set -Ux YT_HOME {}` to use yt from anywhere",
        dir.display()
    );
    Ok(())
}

/// $YT_HOME, or the nearest ancestor of the current directory with a library or
/// config file.
fn find_root() -> Result<PathBuf> {
    if let Some(home) = env::var_os("YT_HOME") {
        return Ok(PathBuf::from(home));
    }
    let cwd = env::current_dir()?;
    cwd.ancestors()
        .find(|d| d.join(LIBRARY_FILE).exists() || d.join(CONFIG_FILE).exists())
        .map(|d| d.to_path_buf())
        .context("no library here: run `yt init` in your library folder, or set YT_HOME")
}

// ---------- selecting videos ----------

/// YouTube IDs can start with '-' (e.g. "-j8PzkZ70Lg"), which clap would read
/// as flags. Such arguments are rewritten to "id:-j8PzkZ70Lg" before parsing.
fn protect_id(arg: &str) -> String {
    let looks_like_id = arg.len() == 11
        && arg.starts_with('-')
        && !arg.starts_with("--")
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if looks_like_id {
        format!("id:{arg}")
    } else {
        arg.to_string()
    }
}

fn extract_id(s: &str) -> Option<&str> {
    let rest = ["v=", "youtu.be/", "/shorts/", "/live/"]
        .iter()
        .find_map(|m| s.find(m).map(|i| &s[i + m.len()..]))?;
    let end = rest.find(['&', '?', '#', '/']).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn find_one(lib: &Library, target: &str) -> Result<usize> {
    let target = target.strip_prefix("id:").unwrap_or(target);
    let id = extract_id(target).unwrap_or(target);
    if let Some(i) = lib.index_of(id) {
        return Ok(i);
    }
    let needle = target.to_lowercase();
    let hits: Vec<usize> = (0..lib.videos.len())
        .filter(|&i| lib.videos[i].matches(&needle))
        .collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => bail!("no video matches {target:?}"),
        _ => {
            let mut msg = format!(
                "{target:?} matches {} videos, be more specific:",
                hits.len()
            );
            for &i in hits.iter().take(10) {
                let v = &lib.videos[i];
                msg += &format!("\n  {}  {}  {}", v.id, v.channel, v.title);
            }
            bail!(msg)
        }
    }
}

fn select(lib: &Library, sel: &Select) -> Result<Vec<usize>> {
    if sel.targets.is_empty() && sel.lists.is_empty() && !sel.all {
        bail!("nothing selected: give video IDs/URLs/search terms, --list <name> or --all");
    }
    let mut picked = BTreeSet::new();
    for t in &sel.targets {
        picked.insert(find_one(lib, t)?);
    }
    if sel.all || !sel.lists.is_empty() {
        for (i, v) in lib.videos.iter().enumerate() {
            let in_lists = sel.all || sel.lists.iter().any(|l| v.lists.contains(l));
            if in_lists && sel.status.is_none_or(|s| v.status == s) {
                picked.insert(i);
            }
        }
    }
    Ok(picked.into_iter().collect())
}

// ---------- commands ----------

fn add(
    ctx: &mut Ctx,
    urls: &[String],
    lists: &[String],
    note: Option<&str>,
    get: bool,
    fast: bool,
) -> Result<()> {
    let metas = ytdlp::fetch(&ctx.cfg, urls, fast)?;
    let mut idxs = vec![];
    for m in metas {
        let (verb, i) = match ctx.lib.index_of(&m.id) {
            Some(i) => ("exists", i),
            None => {
                ctx.lib.videos.push(Video::from_meta(m));
                ("added", ctx.lib.videos.len() - 1)
            }
        };
        let v = &mut ctx.lib.videos[i];
        v.lists.extend(lists.iter().cloned());
        if let Some(n) = note {
            v.add_note(n);
        }
        println!("{verb:>6}  {}  {}  {}", v.id, v.channel, v.title);
        idxs.push(i);
    }
    ctx.save()?;
    if get { download(ctx, &idxs) } else { Ok(()) }
}

fn download(ctx: &mut Ctx, idxs: &[usize]) -> Result<()> {
    let media = ctx.media();
    let missing: Vec<usize> = idxs
        .iter()
        .copied()
        .filter(|&i| !media.contains_key(&ctx.lib.videos[i].id))
        .collect();
    if idxs.len() > missing.len() {
        println!("{} already downloaded", idxs.len() - missing.len());
    }
    if missing.is_empty() {
        return Ok(());
    }
    let est: u64 = missing.iter().map(|&i| ctx.lib.videos[i].est_size()).sum();
    println!("downloading {} video(s), ~{}", missing.len(), fmt_size(est));
    if let Some(max) = ctx.cfg.max_bytes() {
        let used: u64 = media.values().map(|f| f.size).sum();
        if used + est > max {
            eprintln!(
                "warning: this goes over max_disk_gb ({} used + ~{} > {}); see `yt du`",
                fmt_size(used),
                fmt_size(est),
                fmt_size(max)
            );
        }
    }
    let dir = ctx.cfg.download_dir(&ctx.root)?;
    let urls: Vec<&str> = missing
        .iter()
        .map(|&i| ctx.lib.videos[i].url.as_str())
        .collect();
    ytdlp::download(&ctx.cfg, &dir, &urls)?;

    let media = ctx.media();
    let mut failed = vec![];
    for &i in &missing {
        let v = &mut ctx.lib.videos[i];
        match media.get(&v.id) {
            Some(f) => v.size = Some(f.size),
            None => failed.push(i),
        }
    }
    ctx.save()?;
    if !failed.is_empty() {
        eprintln!("\nfailed to download {} video(s):", failed.len());
        for i in failed {
            let v = &ctx.lib.videos[i];
            eprintln!("  {}  {}  {}", v.id, v.channel, v.title);
        }
        eprintln!(
            "if a link is dead, `yt find <id>` searches for it and `yt relink <id> <url>` fixes it"
        );
    }
    Ok(())
}

fn ls(
    ctx: &Ctx,
    search: Option<&str>,
    lists: &[String],
    status: Option<Status>,
    local: bool,
    remote: bool,
) {
    let media = ctx.media();
    let needle = search.map(str::to_lowercase);
    let rows: Vec<&Video> = ctx
        .lib
        .videos
        .iter()
        .filter(|v| lists.is_empty() || lists.iter().any(|l| v.lists.contains(l)))
        .filter(|v| status.is_none_or(|s| v.status == s))
        .filter(|v| needle.as_deref().is_none_or(|n| v.matches(n)))
        .filter(|v| {
            let is_local = media.contains_key(&v.id);
            (!local || is_local) && (!remote || !is_local)
        })
        .collect();

    let color = std::io::stdout().is_terminal();
    for v in &rows {
        print_row(v, media.get(&v.id), color);
    }
    let n_local = rows.iter().filter(|v| media.contains_key(&v.id)).count();
    let local_bytes: u64 = rows
        .iter()
        .filter_map(|v| media.get(&v.id))
        .map(|f| f.size)
        .sum();
    let secs: u64 = rows.iter().filter_map(|v| v.duration).sum();
    println!(
        "\n{} video(s), {} downloaded ({}), {} total",
        rows.len(),
        n_local,
        fmt_size(local_bytes),
        fmt_dur(secs)
    );
}

fn print_row(v: &Video, local: Option<&LocalFile>, color: bool) {
    let mark = if local.is_some() { "●" } else { "○" };
    let status = match (v.status, v.position, v.duration) {
        (Status::Watched, ..) => "✓   ".to_string(),
        (Status::Partial, Some(p), Some(d)) if d > 0 => format!("◐{:>2}%", (p * 100 / d).min(99)),
        (Status::Partial, ..) => "◐   ".to_string(),
        (Status::Todo, ..) => "    ".to_string(),
    };
    let size = match local {
        Some(f) => fmt_size(f.size),
        None => format!("~{}", fmt_size(v.est_size())),
    };
    let lists = v.lists.iter().cloned().collect::<Vec<_>>().join(",");
    let line = format!(
        "{mark} {status} {:<11}  {:>7}  {:>6}  {:<18}  {:<60}  {lists}",
        v.id,
        v.duration.map(fmt_dur).unwrap_or_default(),
        size,
        trunc(&v.channel, 18),
        trunc(&v.title, 60),
    );
    if color && local.is_none() {
        println!("\x1b[2m{line}\x1b[0m");
    } else {
        println!("{line}");
    }
}

fn show_lists(ctx: &Ctx) {
    let media = ctx.media();
    let mut names: BTreeSet<&str> = ctx.cfg.watchlists.keys().map(String::as_str).collect();
    names.extend(
        ctx.lib
            .videos
            .iter()
            .flat_map(|v| v.lists.iter().map(String::as_str)),
    );
    let unlisted = ctx.lib.videos.iter().any(|v| v.lists.is_empty());

    let summarize = |name: &str, vids: Vec<&Video>| {
        let local: Vec<&LocalFile> = vids.iter().filter_map(|v| media.get(&v.id)).collect();
        let remote: u64 = vids
            .iter()
            .filter(|v| !media.contains_key(&v.id))
            .map(|v| v.est_size())
            .sum();
        let watched = vids.iter().filter(|v| v.status == Status::Watched).count();
        let secs: u64 = vids
            .iter()
            .filter(|v| v.status != Status::Watched)
            .filter_map(|v| v.duration)
            .sum();
        let desc = ctx
            .cfg
            .watchlists
            .get(name)
            .map(String::as_str)
            .unwrap_or("");
        println!(
            "{name:<14} {:>4} videos  {:>4} watched  {:>8} left  {:>4} local ({:>6})  ~{:>6} to get   {desc}",
            vids.len(),
            watched,
            fmt_dur(secs),
            local.len(),
            fmt_size(local.iter().map(|f| f.size).sum()),
            fmt_size(remote),
        );
    };
    for name in names {
        summarize(
            name,
            ctx.lib
                .videos
                .iter()
                .filter(|v| v.lists.contains(name))
                .collect(),
        );
    }
    if unlisted {
        summarize(
            "(no list)",
            ctx.lib
                .videos
                .iter()
                .filter(|v| v.lists.is_empty())
                .collect(),
        );
    }
}

fn rm(ctx: &mut Ctx, sel: &Select, forget: bool) -> Result<()> {
    let idxs = select(&ctx.lib, sel)?;
    let media = ctx.media();
    let (mut freed, mut n) = (0, 0);
    for &i in &idxs {
        let v = &mut ctx.lib.videos[i];
        if let Some(f) = media.get(&v.id) {
            fs::remove_file(&f.path).with_context(|| format!("deleting {}", f.path.display()))?;
            v.size = Some(f.size);
            freed += f.size;
            n += 1;
            println!("deleted  {}", f.path.display());
        }
    }
    if forget {
        for &i in idxs.iter().rev() {
            let v = ctx.lib.videos.remove(i);
            println!("forgot   {}  {}", v.id, v.title);
        }
    }
    ctx.save()?;
    println!("freed {} ({n} file(s))", fmt_size(freed));
    Ok(())
}

fn watch(ctx: &mut Ctx, target: &str, from_start: bool, detach: bool) -> Result<()> {
    let i = find_one(&ctx.lib, target)?;
    download(ctx, &[i])?;
    let media = ctx.media();
    let id = ctx.lib.videos[i].id.clone();
    let file = media.get(&id).context("video isn't available locally")?;

    let playable = file
        .path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["mp4", "m4v", "mov"].contains(&e));
    if !playable {
        eprintln!(
            "warning: QuickTime can't play {}; re-download it with `yt rm {id} && yt get {id}`",
            file.path.display()
        );
    }
    let v = &mut ctx.lib.videos[i];
    let start = if from_start {
        0
    } else {
        v.position.unwrap_or(0)
    };
    println!(
        "▶ {}{}",
        v.title,
        if start > 0 {
            format!(" (from {})", fmt_dur(start))
        } else {
            String::new()
        }
    );
    quicktime::open(&file.path, &id, start)?;
    if v.status == Status::Todo {
        v.status = Status::Partial;
    }
    v.last_watched = Some(today());
    ctx.save()?;
    if detach {
        return Ok(());
    }

    println!("tracking progress until the QuickTime window is closed (ctrl-c to stop tracking)");
    let (mut seen, mut misses) = (false, 0);
    loop {
        thread::sleep(Duration::from_secs(5));
        let docs = quicktime::documents().unwrap_or_default();
        match docs.iter().find(|d| d.id == id) {
            Some(d) => {
                seen = true;
                // Reload so that changes made meanwhile by other `yt` commands
                // aren't overwritten.
                ctx.lib = Library::load(&ctx.root)?;
                let Some(i) = ctx.lib.index_of(&id) else {
                    break;
                };
                if ctx.lib.videos[i].record_position(d.position, d.duration) {
                    ctx.save()?;
                }
            }
            None => {
                misses += 1;
                if seen || misses >= 6 {
                    break;
                }
            }
        }
    }
    let lib = Library::load(&ctx.root)?;
    let Some(v) = lib.videos.iter().find(|v| v.id == id) else {
        return Ok(());
    };
    match (v.status, v.position) {
        (Status::Partial, Some(p)) => println!("stopped at {}", fmt_dur(p)),
        (s, _) => println!("status: {s:?}"),
    }
    Ok(())
}

fn sync(ctx: &mut Ctx, quiet: bool) -> Result<()> {
    let mut changed = false;
    for d in quicktime::documents()? {
        let Some(i) = ctx.lib.index_of(&d.id) else {
            continue;
        };
        let v = &mut ctx.lib.videos[i];
        if v.record_position(d.position, d.duration) {
            changed = true;
            if !quiet {
                let at = v
                    .position
                    .map(|p| format!(" at {}", fmt_dur(p)))
                    .unwrap_or_default();
                println!("{:?}{at}  {}", v.status, v.title);
            }
        }
    }
    if changed {
        ctx.save()?;
    } else if !quiet {
        println!("nothing to update");
    }
    Ok(())
}

fn info(ctx: &Ctx, i: usize) {
    let v = &ctx.lib.videos[i];
    let media = ctx.media();
    let na = || "-".to_string();
    println!("title     {}", v.title);
    println!(
        "channel   {}{}",
        v.channel,
        v.channel_id
            .as_ref()
            .map(|c| format!(" ({c})"))
            .unwrap_or_default()
    );
    println!("id        {}", v.id);
    println!("url       {}", v.url);
    println!("duration  {}", v.duration.map(fmt_dur).unwrap_or_else(na));
    println!("uploaded  {}", v.upload_date.clone().unwrap_or_else(na));
    println!(
        "lists     {}",
        v.lists.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    println!(
        "status    {:?}{}",
        v.status,
        v.position
            .map(|p| format!(" at {}", fmt_dur(p)))
            .unwrap_or_default()
    );
    println!("added     {}", v.added);
    println!("watched   {}", v.last_watched.clone().unwrap_or_else(na));
    match media.get(&v.id) {
        Some(f) => println!("file      {} ({})", f.path.display(), fmt_size(f.size)),
        None => println!("file      not downloaded (~{})", fmt_size(v.est_size())),
    }
    if !v.notes.is_empty() {
        println!("notes\n{}", v.notes);
    }
}

fn du(ctx: &Ctx) {
    let media = ctx.media();
    for dir in ctx.cfg.media_dirs(&ctx.root) {
        if !dir.is_dir() {
            println!("{}  (not available)", dir.display());
            continue;
        }
        let files: Vec<&LocalFile> = media
            .values()
            .filter(|f| f.path.parent() == Some(dir.as_path()))
            .collect();
        println!(
            "{}  {} in {} file(s)",
            dir.display(),
            fmt_size(files.iter().map(|f| f.size).sum()),
            files.len()
        );
    }
    let used: u64 = media.values().map(|f| f.size).sum();
    match ctx.cfg.max_bytes() {
        Some(max) => println!(
            "total {} of {} ({}%)",
            fmt_size(used),
            fmt_size(max),
            used * 100 / max.max(1)
        ),
        None => println!(
            "total {} (set max_disk_gb in {CONFIG_FILE} for a budget)",
            fmt_size(used)
        ),
    }

    let untracked: Vec<&LocalFile> = media
        .iter()
        .filter(|(id, _)| ctx.lib.index_of(id).is_none())
        .map(|(_, f)| f)
        .collect();
    if !untracked.is_empty() {
        println!(
            "\n{} file(s) not in the library, run `yt import`:",
            untracked.len()
        );
        for f in untracked {
            println!("  {}", f.path.display());
        }
    }

    let mut local: Vec<(&Video, &LocalFile)> = ctx
        .lib
        .videos
        .iter()
        .filter_map(|v| media.get(&v.id).map(|f| (v, f)))
        .collect();
    let watched: Vec<_> = local
        .iter()
        .filter(|(v, _)| v.status == Status::Watched)
        .collect();
    if !watched.is_empty() {
        let total: u64 = watched.iter().map(|(_, f)| f.size).sum();
        println!(
            "\nwatched and still downloaded ({}), `yt rm --all -s watched`:",
            fmt_size(total)
        );
        for (v, f) in watched {
            println!(
                "  {:>6}  {}  {}",
                fmt_size(f.size),
                v.id,
                trunc(&v.title, 70)
            );
        }
    }
    let over = ctx.cfg.max_bytes().is_some_and(|max| used > max);
    if over {
        // Least recently touched first, biggest first within that.
        local.retain(|(v, _)| v.status != Status::Watched);
        local.sort_by(|a, b| {
            let touched = |v: &Video| v.last_watched.clone().unwrap_or_else(|| v.added.clone());
            touched(a.0)
                .cmp(&touched(b.0))
                .then(b.1.size.cmp(&a.1.size))
        });
        println!("\nover budget; least recently touched unwatched downloads:");
        for (v, f) in local.iter().take(10) {
            println!(
                "  {:>6}  {}  {}",
                fmt_size(f.size),
                v.id,
                trunc(&v.title, 70)
            );
        }
    }
}

fn import(ctx: &mut Ctx, lists: &[String]) -> Result<()> {
    let media = ctx.media();
    let mut new: Vec<(&String, &LocalFile)> = media
        .iter()
        .filter(|(id, _)| ctx.lib.index_of(id).is_none())
        .collect();
    if new.is_empty() {
        println!("nothing to import");
        return Ok(());
    }
    new.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    println!("fetching metadata for {} file(s)…", new.len());
    let urls: Vec<String> = new
        .iter()
        .map(|(id, _)| format!("https://www.youtube.com/watch?v={id}"))
        .collect();
    let metas = ytdlp::fetch(&ctx.cfg, &urls, false).unwrap_or_else(|e| {
        eprintln!("{e:#}");
        vec![]
    });
    for (id, f) in new {
        let mut v = match metas.iter().find(|m| &m.id == id) {
            Some(m) => Video::from_meta(m.clone()),
            None => {
                eprintln!("no metadata for {id}, using the file name");
                let mut v = Video::from_meta(ytdlp::Meta {
                    id: id.clone(),
                    url: urls
                        .iter()
                        .find(|u| u.ends_with(id.as_str()))
                        .cloned()
                        .unwrap_or_default(),
                    title: media::title_from_path(&f.path),
                    channel: "?".into(),
                    channel_id: None,
                    duration: None,
                    upload_date: None,
                });
                v.add_note("imported from file; metadata unavailable");
                v
            }
        };
        v.size = Some(f.size);
        v.lists.extend(lists.iter().cloned());
        println!("imported  {}  {}  {}", v.id, v.channel, v.title);
        ctx.lib.videos.push(v);
    }
    ctx.save()
}

fn find(ctx: &Ctx, target: &str) -> Result<()> {
    let v = &ctx.lib.videos[find_one(&ctx.lib, target)?];
    let query = format!("ytsearch8:{} {}", v.channel.trim_matches('?'), v.title);
    for m in ytdlp::fetch(&ctx.cfg, &[query], true)? {
        let same = if m.id == v.id { "=" } else { " " };
        println!(
            "{same} {}  {:>7}  {:<18}  {}",
            m.id,
            m.duration.map(fmt_dur).unwrap_or_default(),
            trunc(&m.channel, 18),
            m.title
        );
    }
    println!(
        "\n(= is the saved ID) fix a dead link with `yt relink {} <url>`",
        v.id
    );
    Ok(())
}

fn relink(ctx: &mut Ctx, target: &str, url: &str) -> Result<()> {
    let i = find_one(&ctx.lib, target)?;
    let mut metas = ytdlp::fetch(&ctx.cfg, &[url.to_string()], true)?;
    if metas.len() != 1 {
        bail!("{url} is not a single video");
    }
    let m = metas.remove(0);
    if let Some(j) = ctx.lib.index_of(&m.id).filter(|&j| j != i) {
        bail!(
            "{} is already in the library as {:?}",
            m.id,
            ctx.lib.videos[j].title
        );
    }
    let v = &mut ctx.lib.videos[i];
    println!("{}  {}\n  -> {}  {}", v.id, v.title, m.id, m.title);
    v.relink(m);
    ctx.save()
}

// ---------- formatting ----------

fn fmt_dur(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn fmt_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1}G", b / 1e9)
    } else {
        format!("{:.0}M", b / 1e6)
    }
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).chain(['…']).collect()
    }
}
