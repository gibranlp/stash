# STASH

STASH is a fast, keyboard-driven terminal music browser, player, and file organizer written in Rust.

---

See [the changelog](CHANGELOG.md) for release notes.

## Features

- **File Browser**: Dual-pane navigation with file previews. Copy, move, and delete files in the background with a live progress bar. Select multiple files, search by name, and jump pages with `PageUp` / `PageDown`.
- **Music Player**: Play any track with `Enter`, queue up songs, skip, seek, adjust volume, and toggle shuffle/repeat. Shows embedded lyrics with an automatic online fallback. Timestamped lyrics follow playback automatically and highlight the current line; `PageUp` / `PageDown` scroll plain lyrics. Publishes track metadata and album artwork to desktop MPRIS clients.
- **Library**: Scans your music folders into one searchable, sortable list. Organize tracks into playlists and edit tags inline.
- **Library Healer**: Finds tracks with missing or broken metadata and proposes fixes from the filename or online lookups, so you can review and apply them with one key.

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

Previews load in background workers. Text previews show up to 256 KiB / 5,000 lines; images use decoder limits of 8,192 pixels per dimension and a 128 MiB allocation budget, then display a thumbnail. Files that cannot be previewed show an error.

See [the performance and release review](PERFORMANCE_REVIEW.md) for validation results and remaining release checks.

---

## Author

- **gibranlp**
- Homepage: [gibranlp.dev](https://gibranlp.dev)
- Repository: [github.com/gibranlp/stash](https://github.com/gibranlp/stash)
