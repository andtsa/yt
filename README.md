
# we have youtube at home

editor requirements:

- mark videos as watched, partially watched, or watchlisted
- add comments/notes with videos
- save the link instead of the mp4 file so i can remove downloads
- see all the videos in my library
- monitor disk usage and recommend deleting / offloading videos
- declarative config! 
    - max disk usage
    - download through proxy
    - use random proxy?


## usage

`cargo install --path .` installs the `yt` binary; `set -Ux YT_HOME ~/proj/youtube-at-home` lets it run from anywhere.

- `config.toml` — declarative settings (media dirs, disk budget, format, proxies, watchlist descriptions)
- `library.json` — every video's link, title, channel, watchlists, status, position, notes (safe to commit)
- video files are named `<title> [<id>].mp4`; "downloaded" just means such a file exists in a media dir

```
yt add <url|playlist> -l physics [-n note] [--get]   # save link + metadata
yt ls [search] [-l list] [-s todo|partial|watched] [--local|--remote]
yt lists                                             # per-watchlist counts/sizes/time left
yt get <ids/urls/search> | -l physics [-s todo]      # download (e.g. before a flight)
yt rm  <ids/urls/search> | -a -s watched [--forget]  # delete files, keep entries
yt watch <target>        # download if needed, open in QuickTime at saved position, track progress
yt sync                  # save positions of whatever is open in QuickTime
yt mark watched <targets> · yt tag math <targets> · yt untag … · yt note <target> [text]
yt info <target> · yt du · yt import [-l list]
yt find <target> · yt relink <target> <new-url>      # recover from dead links
```

### yt-dlp

yt-dlp is vendored as a shallow git submodule in `yt-dlp/` and run with `python3` (needs ≥3.10);
if it isn't checked out, `ytdlp` from `$PATH` is used instead (override with `ytdlp = "..."` in `config.toml`).

```sh
git clone --recurse-submodules <this repo>          # or: git submodule update --init
# update to a newer release:
git -C yt-dlp fetch --depth 1 origin tag 2026.09.xx && git -C yt-dlp checkout 2026.09.xx && git add yt-dlp
```
