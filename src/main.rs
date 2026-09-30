mod commands;
mod config;
mod fmt;
mod library;
mod media;
mod paths;
mod player;
mod select;
mod setup;
mod ytdlp;

use std::env;
use std::fs;

use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use commands::Ctx;
use library::Library;
use library::Status;
use paths::Paths;
use select::Select;

#[derive(Parser)]
#[command(
    name = "yt",
    version,
    about = "we have youtube at home: a local library of videos and watchlists"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// First-time setup: create the config and library, install yt-dlp if it's
    /// missing, and check the other dependencies
    Setup {
        /// Install yt-dlp even if one is already available
        #[arg(long)]
        ytdlp: bool,
        /// Update the yt-dlp installed by `yt setup` to the latest release
        #[arg(long)]
        update: bool,
    },
    /// Check dependencies and show where everything is stored
    Doctor,
    /// Create a portable library (config.toml, library.json, media/) in the
    /// current directory
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
    /// Download if needed, open in the player at the saved position, and track
    /// progress
    Watch {
        target: String,
        #[arg(long)]
        from_start: bool,
        /// Don't wait around to track progress (run `yt sync` before closing
        /// the player)
        #[arg(short, long)]
        detach: bool,
    },
    /// Save the playback positions of videos open in QuickTime or VLC
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

fn main() -> Result<()> {
    let cli = Cli::parse_from(env::args().map(select::protect_id));
    match cli.cmd {
        Cmd::Init => return init(),
        Cmd::Setup { ytdlp, update } => {
            let opts = setup::SetupOptions {
                install_ytdlp: ytdlp,
                update,
            };
            return setup::setup(&Paths::discover()?, opts);
        }
        Cmd::Doctor => return setup::doctor(&Paths::discover()?),
        _ => {}
    }

    let mut ctx = Ctx::load(Paths::discover()?)?;
    match cli.cmd {
        Cmd::Init | Cmd::Setup { .. } | Cmd::Doctor => unreachable!(),
        Cmd::Add {
            urls,
            lists,
            note,
            get,
            fast,
        } => commands::add(&mut ctx, &urls, &lists, note.as_deref(), get, fast),
        Cmd::Ls {
            search,
            lists,
            status,
            local,
            remote,
        } => {
            // Pick up positions from an open player first; never fail `ls` over
            // it.
            if let Err(e) = commands::sync(&mut ctx, true) {
                eprintln!("warning: couldn't sync playback positions: {e:#}");
            }
            commands::ls(&ctx, search.as_deref(), &lists, status, local, remote);
            Ok(())
        }
        Cmd::Lists => {
            commands::show_lists(&ctx);
            Ok(())
        }
        Cmd::Get(sel) => {
            let ids = select::select(&ctx.lib, &sel)?;
            commands::download(&mut ctx, &ids)
        }
        Cmd::Rm { sel, forget } => commands::rm(&mut ctx, &sel, forget),
        Cmd::Watch {
            target,
            from_start,
            detach,
        } => commands::watch(&mut ctx, &target, from_start, detach),
        Cmd::Sync => commands::sync(&mut ctx, false),
        Cmd::Mark { to, sel } => commands::mark(&mut ctx, to, &sel),
        Cmd::Tag { list, sel } => commands::tag(&mut ctx, &list, &sel, true),
        Cmd::Untag { list, sel } => commands::tag(&mut ctx, &list, &sel, false),
        Cmd::Note { target, text } => commands::note(&mut ctx, &target, &text),
        Cmd::Info { target } => commands::info(&ctx, &target),
        Cmd::Du => {
            commands::du(&ctx);
            Ok(())
        }
        Cmd::Import { lists } => commands::import(&mut ctx, &lists),
        Cmd::Find { target } => commands::find(&ctx, &target),
        Cmd::Relink { target, url } => commands::relink(&mut ctx, &target, &url),
    }
}

fn init() -> Result<()> {
    let paths = Paths::portable(env::current_dir()?)?;
    if !paths.config.exists() {
        fs::write(&paths.config, config::TEMPLATE)?;
        println!("created {}", paths.config.display());
    }
    fs::create_dir_all(&paths.default_media)?;
    if !paths.library.exists() {
        Library::default().save(&paths.library)?;
        println!("created {}", paths.library.display());
    }
    println!(
        "yt uses this library when run from inside {0}; to use it from anywhere, set YT_HOME={0}",
        paths.base.display()
    );
    Ok(())
}
