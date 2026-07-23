# STASH

STASH is a fast, keyboard-driven terminal music browser, player, and file organizer written in Rust.

---

## Features

- **File Browser**: Dual-pane navigation with file previews. Copy, move, and delete files in the background with a live progress bar. Select multiple files, search by name, and jump pages with `PageUp` / `PageDown`.
- **Music Player**: Play any track with `Enter`, queue up songs, skip, seek, adjust volume, and toggle shuffle/repeat. Shows embedded lyrics with an automatic online fallback.
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

---

## Author

- **gibranlp**
- Homepage: [gibranlp.dev](https://gibranlp.dev)
- Repository: [github.com/gibranlp/stash](https://github.com/gibranlp/stash)
