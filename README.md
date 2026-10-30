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
- **Internet Radio**: Tab `5` tunes in to the HAK.FM station lineup (SomaFM streams) with the same player bar, volume, spectrum visualizer, MPRIS and Discord activity as local tracks. Shows the song on air and the station's recently played tracks; star favorites with `f`, filter them with `F`, and search with `/`.

---

## Installation

### One-line install (Linux / macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/gibranlp/stash/main/install.sh | sh
```

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

### Discord / Vesktop activity

Keep Discord or Vesktop running while playing music in Stash. In Discord, enable **Activity Privacy → Share your detected activities with others**. In Vesktop, enable **Rich Presence (arRPC)** in Vesktop settings ([arRPC setup](https://github.com/OpenAsar/arrpc#custom-clients)).

Stash uses its built-in Discord application named **Stash**; no bot invite, token, or Spotify connection is needed. The activity shows the song title and artist, using the filename when a title tag is missing. Seeking and late metadata updates refresh within five seconds. You can override the application with `discord_app_id` in Stash’s `config.json`; `null` uses the default Stash application. Discord Rich Presence is currently supported on Unix platforms; the Windows build does not publish activity.

See [the performance and release review](PERFORMANCE_REVIEW.md) for validation results and remaining release checks.

---

## Author

- **gibranlp**
- Homepage: [gibranlp.dev](https://gibranlp.dev)
- Repository: [github.com/gibranlp/stash](https://github.com/gibranlp/stash)
