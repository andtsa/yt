# we have youtube at home

A local library for YouTube videos, organised in watchlists (physics, math, …).
It keeps the **link and metadata** of every video, so you can delete the video
files you won't watch soon and download them again with [yt-dlp] when you do:
one video, or a whole watchlist before a flight. It remembers what you've
watched and where you stopped.

```console
$ yt add https://www.youtube.com/playlist?list=PLZHQObOWTQDMsr9K-rj53DwVRMYO3t5Yr -l math
$ yt lists
math             12 videos     0 watched   3:11:00 left     0 local (    0M)  ~  1.1G to get
$ yt get -l math          # download the whole watchlist
$ yt watch "taylor"       # opens the video where you left off, tracks your progress
$ yt rm -a -s watched     # free the space; the entries stay
```

Works on macOS, Linux and Windows.

## Install

```sh
cargo install youtube-at-home   # installs the `yt` command
yt setup
```

`yt setup` creates the config and library, downloads yt-dlp if you don't have
it (the official standalone build, checksum-verified), and runs `yt doctor`,
which checks the other dependencies and tells you how to install what's
missing:

| | needed for | macOS | Windows | Linux |
|---|---|---|---|---|
| [ffmpeg] | merging video and audio | `brew install ffmpeg` | `winget install Gyan.FFmpeg` | `apt install ffmpeg` |
| [deno] | YouTube's JavaScript challenges | `brew install deno` | `winget install DenoLand.Deno` | [deno.land](https://deno.land) |
| [VLC] | tracking progress (optional on macOS) | `brew install --cask vlc` | `winget install VideoLAN.VLC` | `apt install vlc` |

Update yt-dlp with `yt setup --update` (if `yt setup` installed it), or the way
you installed it.

## Usage

```
yt add <url|playlist> -l physics [-n note] [--get]   save links + metadata (--get: download too)
yt ls [search] [-l list] [-s todo|partial|watched] [--local|--remote]
yt lists                                             watchlists with counts, sizes, time left
yt get  <videos> | -l physics [-s todo]              download
yt rm   <videos> | -a -s watched [--forget]          delete files, keep the entries
yt watch <video> [--from-start] [--detach]           play from where you stopped, track progress
yt sync                                              save positions of videos open in the player
yt mark watched|partial|todo <videos>
yt tag physics <videos>  ·  yt untag physics <videos>
yt note <video> [text]   ·  yt info <video>
yt du                                                disk usage, what to delete
yt import [-l list]                                  add video files that are already on disk
yt find <video>  ·  yt relink <video> <new-url>      recover from dead links
yt setup  ·  yt doctor  ·  yt init
```

`<video>` is a video ID, a URL, or part of the title or channel; it has to
match exactly one video. Commands that take `<videos>` also accept `-l <list>`
(every video in a watchlist) or `-a` (all), optionally narrowed with `-s
<status>`.

## Players and progress

| player | platforms | progress tracking |
|---|---|---|
| QuickTime | macOS (default there) | ✓ |
| VLC | macOS, Linux, Windows (default where installed) | ✓ |
| system | everywhere | – just opens the file with the default app |

`yt watch` opens the video at the saved position and records your progress
until you close it; when you get to the end the video is marked watched. If
you play a video some other way, run `yt sync` while it's still open.
Choose the player with `player = "..."` in the config.

VLC is controlled through its built-in web interface, which `yt` enables on a
random localhost port with a random password when it starts VLC. `yt sync`
only sees VLC windows that `yt watch` started.

## Where things are stored

|  | macOS | Linux | Windows |
|---|---|---|---|
| config.toml, library.json | `~/Library/Application Support/youtube-at-home` | `~/.config/youtube-at-home`, `~/.local/share/youtube-at-home` | `%APPDATA%\youtube-at-home` |
| videos | `~/Movies/youtube-at-home` | `~/Videos/youtube-at-home` | `%USERPROFILE%\Videos\youtube-at-home` |

`yt doctor` prints the exact paths. Video files are named
`<title> [<video id>].mp4`: a video counts as downloaded when such a file is
in one of the media directories, so deleting or moving files by hand is fine.

**Portable libraries:** `yt init` creates `config.toml`, `library.json` and
`media/` in the current folder. `yt` uses that library when run inside the
folder, or from anywhere with `YT_HOME=<folder>`.

## Configuration

Every setting is optional; `yt setup` writes a commented template.

```toml
media_dirs = ["~/Videos/youtube-at-home", "/Volumes/External/youtube"]  # all searched; downloads go to the first that exists
max_disk_gb = 50               # warn when downloads exceed this
player = "auto"                # "auto", "quicktime", "vlc" or "system"
vlc = "/path/to/vlc"           # if it isn't found automatically
format = "bv*[vcodec^=avc1][height<=1080]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b"
proxies = ["socks5://127.0.0.1:1080"]   # a random one per yt-dlp run
ytdlp = "/usr/local/bin/yt-dlp"          # default: the one `yt setup` installed, else PATH
extra_args = ["--embed-subs", "--sub-langs", "en.*"]

[watchlists]                   # optional descriptions
physics = "QM, E&M, stat mech"
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

[yt-dlp]: https://github.com/yt-dlp/yt-dlp
[ffmpeg]: https://ffmpeg.org
[deno]: https://deno.com
[VLC]: https://www.videolan.org/vlc/
