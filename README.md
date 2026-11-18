# STASH

STASH is a fast, keyboard-driven terminal music browser, player, and file organizer written in Rust.

---

See [the changelog](CHANGELOG.md) for release notes.

## Features

- **File Browser**: Dual-pane navigation with file previews. Copy, move, and delete files in the background with a live progress bar. Select multiple files, search by name, and jump pages with `PageUp` / `PageDown`.
- **Music Player**: Play any track with `Enter`, queue up songs, skip, seek, adjust volume, and toggle shuffle/repeat. Shows embedded lyrics with an automatic online fallback. Online lookup tolerates featured artists, release suffixes, and artist/title filenames, and caches successful results for offline replay. Timestamped lyrics follow playback automatically and highlight the current line; `PageUp` / `PageDown` scroll plain lyrics. Publishes track metadata and album artwork to desktop MPRIS clients.
- **Discord Rich Presence**: Shows “Listening to Stash” with the song, artist, and playback progress in Discord or Vesktop on Linux/macOS. Reconnects automatically and clears the activity when playback pauses, stops, or Stash exits.
- **Library**: Scans your music folders into one searchable, sortable list. Organize tracks into playlists and edit tags inline.
- **Library Healer**: Finds tracks with missing or broken metadata and proposes fixes from the filename or online lookups, so you can review and apply them with one key.
- **Internet Radio**: Tab `3` tunes in to the HAK.FM station lineup (SomaFM streams) with the same player bar, volume, spectrum visualizer, MPRIS and Discord activity as local tracks. Shows the song on air and the station's recently played tracks; star favorites with `f`, filter them with `F`, and search with `/`.

---

## Installation

### One-line install (Linux / macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/gibranlp/stash/main/install.sh | sh
```

This installs to `/usr/bin` on Linux and `/usr/local/bin` on macOS, asking for your password with `sudo`. A previous install in `~/.local/bin` is removed so it cannot shadow the new one.

### One-line install (Windows — PowerShell)

```powershell
irm https://raw.githubusercontent.com/gibranlp/stash/main/install.ps1 | iex
```

### Build from source

```bash
cargo install --path .
```

---

## Usage

```bash
stash               # open in the default music directory
stash /media/Music  # open in a specific folder
```

Press `?` inside STASH for the full keyboard shortcuts guide.

Stash uses its built-in audio decoder first. Install `ffmpeg` (including `ffprobe`) to play additional formats such as Opus, including WebM audio saved with an `.mp3` extension. Unsupported files are decoded as a stream without modifying the original; pause, volume, seeking, and the visualizer remain available. Files ending in `.opus` are also recognized as audio. Ordinary video files still open in your desktop's default application.

Internet radio requires `ffmpeg` and a network connection. Pausing a station drops the connection and resuming tunes in live again; if a stream disconnects, press `Enter` or `Space` to reconnect.

Previews load in background workers. Text previews show up to 256 KiB / 5,000 lines; images use decoder limits of 8,192 pixels per dimension and a 128 MiB allocation budget, then display a thumbnail. Files that cannot be previewed show an error.

Stash checks GitHub for newer stable releases at startup and every six hours. When one is available, a popup offers to install it: press `Y` and Stash closes, downloads and verifies the update, replaces the installed executable and reopens with the same arguments; press `N` to skip it for now. Press `U` at any time to check again or open the prompt. Failed checks retry after 15 minutes. If installing fails, the current version is kept and Stash reopens. The installation directory must be writable; system package installations may require updating through the package manager.

Set `"auto_update": false` in Stash's `config.json` to stop the popup; the footer still announces new versions and `U` installs them. Downloaded files must match the release's expected size and SHA-256 checksum; incomplete or invalid downloads leave the installed executable untouched.

Linux updates install to `/usr/bin/stash` and remove an older copy in `~/.local/bin`. When administrator rights are required, the updater asks for your password with `sudo` (or `doas`) to install the new version.

On Linux, Stash mounts USB sticks and external drives through udisks2 (`udisksctl`) when they are plugged in, or when Stash starts. Set `"auto_mount": false` in `config.json` to turn this off.

### Discord / Vesktop activity

Keep Discord or Vesktop running while playing music in Stash. In Discord, enable **Activity Privacy → Share your detected activities with others**. In Vesktop, enable **Rich Presence (arRPC)** in Vesktop settings ([arRPC setup](https://github.com/OpenAsar/arrpc#custom-clients)).

Stash uses its built-in Discord application named **Stash**; no bot invite, token, or Spotify connection is needed. The activity shows the song title and artist, using the filename when a title tag is missing. Seeking and late metadata updates refresh within five seconds. You can override the application with `discord_app_id` in Stash’s `config.json`; `null` uses the default Stash application. Discord Rich Presence is currently supported on Unix platforms; the Windows build does not publish activity.

See [the performance and release review](PERFORMANCE_REVIEW.md) for validation results and remaining release checks.

---

## Author

- **gibranlp**
- Homepage: [gibranlp.dev](https://gibranlp.dev)
- Repository: [github.com/gibranlp/stash](https://github.com/gibranlp/stash)

### Automatic album covers

As you play local music, Stash checks for cover.jpg, folder.jpg, front.jpg (or PNG), then embedded artwork. If none is available, it searches MusicBrainz using the album and album artist (or track artist) tags, and downloads a matching front cover from the Cover Art Archive. Songs without album and artist tags are skipped. Covers are cached for offline playback and shared by tracks from the same album; failed lookups retry after an hour when a track is played again.

Stash also embeds available covers in supported writable music files, including MP3, FLAC, Ogg Vorbis, and M4A, preserving existing embedded artwork and other tags. It edits a temporary copy before replacing the original so playback can continue. Read-only files, symbolic links, and hard links keep using the displayed cover without being modified. Downloaded covers stay in Stash's cache rather than creating a shared cover.jpg in folders that may contain multiple albums.

Both features are enabled by default. Set `"download_album_art": false` or `"embed_album_art": false` in `config.json` and restart Stash to disable either behavior. Online lookups send album and artist names to MusicBrainz.

### Podcasts (4)

Press `4` for Podcasts. Use `/` to search the public Apple Podcasts directory, or `a` to paste an HTTP(S) podcast RSS feed URL. Adding a valid feed URL subscribes to it automatically; use `f` to subscribe or unsubscribe from a selected search result or opened podcast. `F` focuses your subscriptions. Subscriptions are local to Stash and persist across restarts.

The left column has search results above a permanent subscriptions list. Use `Tab` or the left/right arrows to switch between search results, subscriptions, and episodes; `j`/`k` or the up/down arrows select an item. `Enter` opens a podcast, or downloads and plays an episode. `d` downloads an episode without playing it, `R` refreshes the selected feed, `Space` pauses/resumes, and `s` stops playback. In Episodes, `o` cycles newest first, oldest first, title A–Z, and title Z–A while keeping the selected episode highlighted. Download confirmations disappear after four seconds. A `↓` marks downloaded episodes.

Stash creates `~/Podcasts` when you open this tab. Episodes go into named podcast subfolders there; subscriptions are stored in `~/Podcasts/subscriptions.json`, and cached feeds in `~/Podcasts/.feeds`. Existing episode downloads are reused, and cached feeds remain usable offline. Subscribing does not download an entire back catalog: choose the episodes you want with `Enter` or `d`.

### Tabs and shared playback shortcuts

Tabs use the same order everywhere: **1 Browser · 2 Player · 3 Radio · 4 Podcasts · 5 Library · 6 Healer**. Tab numbers do not switch screens while you are typing in a search field or dialog.

Player, Radio, and Podcasts share `Space` / `p` for play/pause, `s` to stop, `n` / `b` for next/previous, `+` / `-` for volume, `/` to search, and `?` for help. `Enter` activates the highlighted item. For tracks and downloaded podcast episodes, `H` / `L` or `Shift+Left` / `Shift+Right` seek five seconds; live radio cannot seek. Next/previous on Podcasts follows the opened episode list and downloads an episode if needed. Stopping cancels pending podcast autoplay while letting a download finish.

Use `f` to favorite a radio station or subscribe to a podcast; `F` filters radio favorites or focuses podcast subscriptions. Podcasts use `R` to refresh and `o` to sort episodes. Library also accepts `o` to sort. `r` cycles repeat for tracks/episodes and `z` shuffles the Player queue; unavailable live-radio operations show a notice. `v` cycles the visualizer in Player/Radio, and `[` / `]` adjust its decay (`<` / `>` remain aliases). Browser selection/file operations and Healer editing retain their contextual keys.
