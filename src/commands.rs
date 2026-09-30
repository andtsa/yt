use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::thread;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::config::Config;
use crate::fmt;
use crate::library;
use crate::library::Library;
use crate::library::Status;
use crate::library::Video;
use crate::library::today;
use crate::media;
use crate::media::LocalFile;
use crate::media::MediaIndex;
use crate::paths::Paths;
use crate::player;
use crate::player::Player;
use crate::select::Select;
use crate::select::find_one;
use crate::select::select;
use crate::ytdlp;
use crate::ytdlp::YtDlp;

pub struct Ctx {
    pub paths: Paths,
    pub cfg: Config,
    /// Snapshot loaded at startup; changes go through `update`.
    pub lib: Library,
}

impl Ctx {
    pub fn load(paths: Paths) -> Result<Self> {
        Ok(Self {
            cfg: Config::load(&paths)?,
            lib: Library::load(&paths.library)?,
            paths,
        })
    }

    fn media(&self) -> MediaIndex {
        media::scan(&self.cfg.media_dirs(&self.paths))
    }

    fn ytdlp(&self) -> Result<YtDlp> {
        YtDlp::locate(&self.cfg, &self.paths)
            .context("yt-dlp not found: run `yt setup` to install it")
    }

    /// Lock the library, reload it, apply `f`, and save. Reloading under the
    /// lock means changes made meanwhile by other `yt` commands are kept.
    fn update<R>(&mut self, f: impl FnOnce(&mut Library) -> R) -> Result<R> {
        let _lock = library::lock(&self.paths.library)?;
        self.lib = Library::load(&self.paths.library)?;
        let r = f(&mut self.lib);
        self.lib.save(&self.paths.library)?;
        Ok(r)
    }

    /// Like `update`, but only saves if `f` returns true.
    fn update_if(&mut self, f: impl FnOnce(&mut Library) -> bool) -> Result<bool> {
        let _lock = library::lock(&self.paths.library)?;
        self.lib = Library::load(&self.paths.library)?;
        let changed = f(&mut self.lib);
        if changed {
            self.lib.save(&self.paths.library)?;
        }
        Ok(changed)
    }
}

pub fn add(
    ctx: &mut Ctx,
    urls: &[String],
    lists: &[String],
    note: Option<&str>,
    get: bool,
    fast: bool,
) -> Result<()> {
    let metas = ytdlp::fetch(&ctx.ytdlp()?, &ctx.cfg, urls, fast)?;
    let ids = ctx.update(|lib| {
        let mut ids = vec![];
        for m in metas {
            let id = m.id.clone();
            let verb = if lib.get(&id).is_some() {
                "exists"
            } else {
                lib.videos.push(Video::from_meta(m));
                "added"
            };
            let v = lib.get_mut(&id).expect("just added");
            v.lists.extend(lists.iter().cloned());
            if let Some(n) = note {
                v.add_note(n);
            }
            println!("{verb:>6}  {}  {}  {}", v.id, v.channel, v.title);
            ids.push(id);
        }
        ids
    })?;
    if get { download(ctx, &ids) } else { Ok(()) }
}

pub fn download(ctx: &mut Ctx, ids: &[String]) -> Result<()> {
    let media = ctx.media();
    let missing: Vec<&Video> = ids
        .iter()
        .filter_map(|id| ctx.lib.get(id))
        .filter(|v| !media.contains_key(&v.id))
        .collect();
    if ids.len() > missing.len() {
        println!("{} already downloaded", ids.len() - missing.len());
    }
    if missing.is_empty() {
        return Ok(());
    }
    let est: u64 = missing.iter().map(|v| v.est_size()).sum();
    println!(
        "downloading {} video(s), ~{}",
        missing.len(),
        fmt::size(est)
    );
    if let Some(max) = ctx.cfg.max_bytes() {
        let used: u64 = media.values().map(|f| f.size).sum();
        if used + est > max {
            eprintln!(
                "warning: this goes over max_disk_gb ({} used + ~{} > {}); see `yt du`",
                fmt::size(used),
                fmt::size(est),
                fmt::size(max)
            );
        }
    }
    let urls: Vec<String> = missing.iter().map(|v| v.url.clone()).collect();
    let missing: Vec<String> = missing.iter().map(|v| v.id.clone()).collect();
    let dir = ctx.cfg.download_dir(&ctx.paths)?;
    let urls: Vec<&str> = urls.iter().map(String::as_str).collect();
    ytdlp::download(&ctx.ytdlp()?, &ctx.cfg, &dir, &urls)?;

    let media = ctx.media();
    let failed = ctx.update(|lib| {
        let mut failed = vec![];
        for id in &missing {
            let Some(v) = lib.get_mut(id) else { continue };
            match media.get(id) {
                Some(f) => v.size = Some(f.size),
                None => failed.push(format!("  {}  {}  {}", v.id, v.channel, v.title)),
            }
        }
        failed
    })?;
    if !failed.is_empty() {
        eprintln!("\nfailed to download {} video(s):", failed.len());
        eprintln!("{}", failed.join("\n"));
        eprintln!(
            "if a link is dead, `yt find <id>` searches for it and `yt relink <id> <url>` fixes it"
        );
    }
    Ok(())
}

pub fn ls(
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

    let color = fmt::color();
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
        fmt::size(local_bytes),
        fmt::dur(secs)
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
        Some(f) => fmt::size(f.size),
        None => format!("~{}", fmt::size(v.est_size())),
    };
    let lists = v.lists.iter().cloned().collect::<Vec<_>>().join(",");
    let line = format!(
        "{mark} {status} {:<11}  {:>7}  {:>6}  {:<18}  {:<60}  {lists}",
        v.id,
        v.duration.map(fmt::dur).unwrap_or_default(),
        size,
        fmt::trunc(&v.channel, 18),
        fmt::trunc(&v.title, 60),
    );
    if color && local.is_none() {
        println!("\x1b[2m{line}\x1b[0m");
    } else {
        println!("{line}");
    }
}

pub fn show_lists(ctx: &Ctx) {
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
            fmt::dur(secs),
            local.len(),
            fmt::size(local.iter().map(|f| f.size).sum()),
            fmt::size(remote),
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

pub fn rm(ctx: &mut Ctx, sel: &Select, forget: bool) -> Result<()> {
    let ids = select(&ctx.lib, sel)?;
    let media = ctx.media();
    let mut deleted: Vec<(&str, u64)> = vec![];
    let mut undeletable: HashSet<&str> = HashSet::new();
    for id in &ids {
        let Some(f) = media.get(id) else { continue };
        match fs::remove_file(&f.path) {
            Ok(()) => {
                println!("deleted  {}", f.path.display());
                deleted.push((id, f.size));
            }
            Err(e) => {
                let hint = if cfg!(windows) {
                    " (is it open in a player?)"
                } else {
                    ""
                };
                eprintln!("couldn't delete {}: {e}{hint}", f.path.display());
                undeletable.insert(id);
            }
        }
    }
    let forgotten = ctx.update(|lib| {
        for (id, size) in &deleted {
            if let Some(v) = lib.get_mut(id) {
                v.size = Some(*size);
            }
        }
        if !forget {
            return 0;
        }
        // Keep entries whose file is still there, so it isn't left untracked.
        let before = lib.videos.len();
        lib.videos
            .retain(|v| !ids.contains(&v.id) || undeletable.contains(v.id.as_str()));
        before - lib.videos.len()
    })?;
    if forget {
        println!("forgot {forgotten} video(s)");
    }
    let freed: u64 = deleted.iter().map(|(_, s)| s).sum();
    println!("freed {} ({} file(s))", fmt::size(freed), deleted.len());
    if !undeletable.is_empty() {
        bail!("{} file(s) couldn't be deleted", undeletable.len());
    }
    Ok(())
}

pub fn watch(ctx: &mut Ctx, target: &str, from_start: bool, detach: bool) -> Result<()> {
    let id = find_one(&ctx.lib, target)?.id.clone();
    let player = Player::resolve(&ctx.cfg, &ctx.paths)?;
    download(ctx, std::slice::from_ref(&id))?;
    let media = ctx.media();
    let file = media.get(&id).context("video isn't available locally")?;
    if !player.can_play(&file.path) {
        eprintln!(
            "warning: {} can't play {}; re-download it with `yt rm {id} && yt get {id}`, or set player = \"vlc\"",
            player.name(),
            file.path.display()
        );
    }

    let v = ctx
        .lib
        .get(&id)
        .context("video disappeared from the library")?;
    let start = if from_start {
        0
    } else {
        v.position.unwrap_or(0)
    };
    let from = if start > 0 {
        format!(" (from {})", fmt::dur(start))
    } else {
        String::new()
    };
    println!("▶ {}{from}", v.title);
    player.open(&file.path, &id, start, &ctx.paths)?;
    ctx.update(|lib| {
        if let Some(v) = lib.get_mut(&id) {
            if v.status == Status::Todo {
                v.status = Status::Partial;
            }
            v.last_watched = Some(today());
        }
    })?;
    if !player.tracks_progress() {
        println!(
            "{} doesn't report progress; afterwards run `yt mark watched {id}`",
            player.name()
        );
        return Ok(());
    }
    if detach {
        println!("run `yt sync` to save your position before closing the player");
        return Ok(());
    }

    println!(
        "tracking progress until the video is closed in {} (ctrl-c to stop tracking)",
        player.name()
    );
    let (mut seen, mut misses) = (false, 0);
    loop {
        thread::sleep(Duration::from_secs(5));
        match player.playing(&ctx.paths).into_iter().find(|p| p.id == id) {
            Some(p) => {
                seen = true;
                ctx.update_if(|lib| {
                    lib.get_mut(&id)
                        .is_some_and(|v| v.record_position(p.position, p.duration))
                })?;
            }
            None => {
                misses += 1;
                if seen || misses >= 6 {
                    break;
                }
            }
        }
    }
    let lib = Library::load(&ctx.paths.library)?;
    if let Some(v) = lib.get(&id) {
        match (v.status, v.position) {
            (Status::Partial, Some(p)) => println!("stopped at {}", fmt::dur(p)),
            (s, _) => println!("status: {s:?}"),
        }
    }
    Ok(())
}

pub fn sync(ctx: &mut Ctx, quiet: bool) -> Result<()> {
    let playing = player::all_playing(&ctx.paths);
    let mut msgs = vec![];
    ctx.update_if(|lib| {
        for p in &playing {
            if let Some(v) = lib.get_mut(&p.id)
                && v.record_position(p.position, p.duration)
            {
                let at = v
                    .position
                    .map(|p| format!(" at {}", fmt::dur(p)))
                    .unwrap_or_default();
                msgs.push(format!("{:?}{at}  {}", v.status, v.title));
            }
        }
        !msgs.is_empty()
    })?;
    if !quiet {
        if msgs.is_empty() {
            println!("nothing to update");
        }
        for m in msgs {
            println!("{m}");
        }
    }
    Ok(())
}

pub fn mark(ctx: &mut Ctx, status: Status, sel: &Select) -> Result<()> {
    let ids = select(&ctx.lib, sel)?;
    ctx.update(|lib| {
        for id in &ids {
            let Some(v) = lib.get_mut(id) else { continue };
            v.status = status;
            if status != Status::Partial {
                v.position = None;
            }
            println!("{status:?}  {}  {}", v.id, v.title);
        }
    })
}

pub fn tag(ctx: &mut Ctx, list: &str, sel: &Select, add: bool) -> Result<()> {
    let ids = select(&ctx.lib, sel)?;
    ctx.update(|lib| {
        for id in &ids {
            if let Some(v) = lib.get_mut(id) {
                if add {
                    v.lists.insert(list.to_string());
                } else {
                    v.lists.remove(list);
                }
            }
        }
    })?;
    let verb = if add { "in" } else { "removed from" };
    println!("{} video(s) {verb} {list}", ids.len());
    Ok(())
}

pub fn note(ctx: &mut Ctx, target: &str, text: &[String]) -> Result<()> {
    let v = find_one(&ctx.lib, target)?;
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
    let id = v.id.clone();
    ctx.update(|lib| {
        if let Some(v) = lib.get_mut(&id) {
            v.add_note(&text.join(" "));
        }
    })
}

pub fn info(ctx: &Ctx, target: &str) -> Result<()> {
    let v = find_one(&ctx.lib, target)?;
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
    println!("duration  {}", v.duration.map(fmt::dur).unwrap_or_else(na));
    println!("uploaded  {}", v.upload_date.clone().unwrap_or_else(na));
    println!(
        "lists     {}",
        v.lists.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    println!(
        "status    {:?}{}",
        v.status,
        v.position
            .map(|p| format!(" at {}", fmt::dur(p)))
            .unwrap_or_default()
    );
    println!("added     {}", v.added);
    println!("watched   {}", v.last_watched.clone().unwrap_or_else(na));
    match media.get(&v.id) {
        Some(f) => println!("file      {} ({})", f.path.display(), fmt::size(f.size)),
        None => println!("file      not downloaded (~{})", fmt::size(v.est_size())),
    }
    if !v.notes.is_empty() {
        println!("notes\n{}", v.notes);
    }
    Ok(())
}

pub fn du(ctx: &Ctx) {
    let media = ctx.media();
    for dir in ctx.cfg.media_dirs(&ctx.paths) {
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
            fmt::size(files.iter().map(|f| f.size).sum()),
            files.len()
        );
    }
    let used: u64 = media.values().map(|f| f.size).sum();
    match ctx.cfg.max_bytes() {
        Some(max) => println!(
            "total {} of {} ({}%)",
            fmt::size(used),
            fmt::size(max),
            used * 100 / max.max(1)
        ),
        None => println!(
            "total {} (set max_disk_gb in {} for a budget)",
            fmt::size(used),
            ctx.paths.config.display()
        ),
    }

    let untracked: Vec<&LocalFile> = media
        .iter()
        .filter(|(id, _)| ctx.lib.get(id).is_none())
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
            fmt::size(total)
        );
        for (v, f) in watched {
            println!(
                "  {:>6}  {}  {}",
                fmt::size(f.size),
                v.id,
                fmt::trunc(&v.title, 70)
            );
        }
    }
    if ctx.cfg.max_bytes().is_some_and(|max| used > max) {
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
                fmt::size(f.size),
                v.id,
                fmt::trunc(&v.title, 70)
            );
        }
    }
}

pub fn import(ctx: &mut Ctx, lists: &[String]) -> Result<()> {
    let media = ctx.media();
    let mut new: Vec<(&String, &LocalFile)> = media
        .iter()
        .filter(|(id, _)| ctx.lib.get(id).is_none())
        .collect();
    if new.is_empty() {
        println!("nothing to import");
        return Ok(());
    }
    new.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    println!("fetching metadata for {} file(s)…", new.len());
    let url = |id: &str| format!("https://www.youtube.com/watch?v={id}");
    let urls: Vec<String> = new.iter().map(|(id, _)| url(id)).collect();
    let metas = ytdlp::fetch(&ctx.ytdlp()?, &ctx.cfg, &urls, false).unwrap_or_else(|e| {
        eprintln!("{e:#}");
        vec![]
    });
    let videos: Vec<Video> = new
        .into_iter()
        .map(|(id, f)| {
            let mut v = match metas.iter().find(|m| &m.id == id) {
                Some(m) => Video::from_meta(m.clone()),
                None => {
                    eprintln!("no metadata for {id}, using the file name");
                    let mut v = Video::from_meta(ytdlp::Meta {
                        id: id.clone(),
                        url: url(id),
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
            v
        })
        .collect();
    ctx.update(|lib| {
        for v in videos {
            if lib.get(&v.id).is_none() {
                println!("imported  {}  {}  {}", v.id, v.channel, v.title);
                lib.videos.push(v);
            }
        }
    })
}

pub fn find(ctx: &Ctx, target: &str) -> Result<()> {
    let v = find_one(&ctx.lib, target)?;
    let query = format!("ytsearch8:{} {}", v.channel.trim_matches('?'), v.title);
    for m in ytdlp::fetch(&ctx.ytdlp()?, &ctx.cfg, &[query], true)? {
        let same = if m.id == v.id { "=" } else { " " };
        println!(
            "{same} {}  {:>7}  {:<18}  {}",
            m.id,
            m.duration.map(fmt::dur).unwrap_or_default(),
            fmt::trunc(&m.channel, 18),
            m.title
        );
    }
    println!(
        "\n(= is the saved ID) fix a dead link with `yt relink {} <url>`",
        v.id
    );
    Ok(())
}

pub fn relink(ctx: &mut Ctx, target: &str, url: &str) -> Result<()> {
    let old = find_one(&ctx.lib, target)?.id.clone();
    let mut metas = ytdlp::fetch(&ctx.ytdlp()?, &ctx.cfg, &[url.to_string()], true)?;
    if metas.len() != 1 {
        bail!("{url} is not a single video");
    }
    let m = metas.remove(0);
    ctx.update(|lib| {
        if m.id != old
            && let Some(existing) = lib.get(&m.id)
        {
            bail!("{} is already in the library as {:?}", m.id, existing.title);
        }
        let v = lib
            .get_mut(&old)
            .context("video disappeared from the library")?;
        println!("{}  {}\n  -> {}  {}", v.id, v.title, m.id, m.title);
        v.relink(m);
        Ok(())
    })?
}
