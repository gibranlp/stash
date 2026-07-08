# STASH

STASH is a fast, keyboard-driven terminal music browser, player, and file organizer written in Rust. It combines dual-pane file system navigation with audio playback, custom virtual playlist management (Collections), asynchronous file operations, and a dynamic real-time FFT spectrum visualizer.

Designed for terminal power users, it supports smooth, standard list scrolling, quick page navigation, and seamlessly integrates with terminal color schemes (e.g. `pywal`).

---

## Key Features

1. **Dual-Pane File Browser**: Navigate directories recursively on the left, and view/play files on the right.
2. **Dynamic FFT Visualizer**: Real-time 160-channel audio frequency spectrum analyzer (Cooley-Tukey Radix-2 FFT + Hanning window) that adjusts dynamically to your terminal width, styled with standard ANSI colors to work out of the box with `pywal` schemes.
3. **Asynchronous File Operations**: Perform copy (`v`), move (`y`), and delete (`d`) operations on background worker threads with a progress bar and current filename overlay, ensuring the UI remains completely responsive.
4. **Quick Page Navigation**: Press `PageUp` and `PageDown` to jump selection cursor by 10 items.
5. **Virtual Collections**: Organize favorite tracks or files into virtual playlists (persisted in `~/.config/stash/collections.json`) without duplicate disk space.
6. **Playback Queue**: Queue multiple songs using standard commands and skip tracks cleanly with shuffle/repeat toggles.
7. **Live Search**: Recursively search directory items live with incremental character matching using walkdir.
8. **Directory Navigation Prepends**: Quick navigation directories `.` and `..` are automatically sorted at the top of the folder list.
9. **MPRIS D-Bus Integration**: Native support for OS media controls, letting you control playback (play/pause, next, previous) using `playerctl`, lock screens, or system media keys (like `XF86AudioPlay`).

---

## Keyboard Shortcuts Guide

### 1. Navigation Keys
- **`Up` / `Down` (`k` / `j`)**: Move highlight selector up or down.
- **`PageUp` / `PageDown`**: Jump selector up or down by 10 items.
- **`Left` (`h`)**: Focus Directories pane, or go to parent directory if already focused.
- **`Right` (`l`)**: Focus Files pane.
- **`Enter`**: Open directory (if focused on left pane) or start playing track (if focused on right pane).
- **`Backspace`**: Navigate to parent directory.

### 2. Selection & File Operations
- **`Space`**: Toggle selection on highlighted file (`[*]`).
- **`v`**: Copy selected files (prompts target directory).
- **`y`**: Move selected files (prompts target directory, robust cross-device copy fallback).
- **`d`**: Delete selected files (prompts confirmation).
- **`a`**: Add selected files to a virtual collection.
- **`c`**: Create a new virtual collection.
- **`C`**: Clear playback queue (from Queue screen).

### 3. Playback Controls
- **`Space`**: Pause / Resume active audio playback (on non-Explorer screens).
- **`Shift + Left` / `Shift + Right`** (or **`H` / `L`**): Seek track backward or forward by 5 seconds.
- **`n`**: Skip to next queued track.
- **`b`**: Skip back to previous queued track.
- **`s`**: Stop playback.
- **`q`**: Add highlighted/selected items to playback queue.
- **`Q`**: Toggle Playback Queue screen.
- **`+` / `-`**: Increase or decrease volume.
- **`r`**: Toggle Repeat mode.
- **`z`**: Toggle Shuffle mode.

### 4. Search & Helper overlays
- **`/`**: Open live incremental filename search.
- **`?`**: Toggle Help guide popup.
- **`Esc`**: Close popups, cancel prompts, or return to Browser view.
- **`Ctrl + C`**: Quit STASH.

---

## Installation

### One-line install (Linux / macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/gibranlp/stash/main/install.sh | sh
```

Installs to `~/.local/bin/stash`. The script detects your OS and architecture automatically and downloads the latest release binary from GitHub.

### One-line install (Windows — PowerShell)

```powershell
irm https://raw.githubusercontent.com/gibranlp/stash/main/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\stash\stash.exe` and adds it to your user PATH.

---

## Build from Source

Ensure you have Rust and Cargo installed.

### Prerequisites by Operating System

- **Linux**: `libasound2-dev` (ALSA) and `libdbus-1-dev` (MPRIS media keys).
  ```bash
  sudo apt-get install libasound2-dev libdbus-1-dev pkg-config   # Debian/Ubuntu
  sudo dnf install alsa-lib-devel dbus-devel                     # Fedora
  ```
- **macOS**: Works natively via CoreAudio. No extra dependencies.
- **Windows**: Works natively via WASAPI. No extra dependencies.

1. Install Rust (if not already installed):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

### Compile and install globally
```bash
cargo install --path .
```

### Build a standalone release binary
```bash
cargo build --release
cp target/release/stash ~/.local/bin/
```

---

## Usage

Start STASH in the default music directory (or configured fallback folder):
```bash
stash
```

Open STASH directly inside a specific folder:
```bash
stash /media/Music/rock
```

---

## Configuration & Storage

STASH stores configuration and database files under your user config directory:
- **Config JSON (`~/.config/stash/config.json`)**: Configures options like starting directories, default volume, and hidden file visibility.
- **Collections JSON (`~/.config/stash/collections.json`)**: Stores your virtual playlists map containing reference paths.

---


## Midnight Commander (mc) Integration

You can integrate STASH directly into Midnight Commander (`mc`) to quickly play files or browse folders:

### 1. File Associations (Open audio in STASH)
Open Midnight Commander, go to **Command -> Edit extension file** (or edit `~/.config/mc/mc.ext` directly), and add the following block:
```ini
# Open audio files in STASH
regex/\.(mp3|flac|ogg|wav|m4a|aac)$
    Open=stash "%f"
```
Pressing `Enter` on audio files inside `mc` will now queue and play them in `stash`.

### 2. User Menu Shortcut (Open folder in STASH)
To open the highlighted folder directly in STASH using the `mc` User Menu (accessible via `F2` key), add this to `~/.config/mc/menu` (or `mc.menu`):
```ini
s       Open current folder in STASH
    stash "%d"
```

---

## Recent Version Changelog

### v0.4.0

- **Lyrics Display**: The Queue screen info pane now shows embedded track lyrics (ID3v2 USLT, Vorbis LYRICS, MP4 ©lyr tags). If no embedded lyrics are found, STASH automatically fetches them from [LRCLIB](https://lrclib.net) as a fallback.
- **Live Lyrics Status**: A color-coded status message shows the current lyrics state in real time: `Loading…` (reading tags), `Fetching…` (querying LRCLIB), `Not Found` (nothing in the database), or an error description if the network request fails.
- **Lyrics Scrolling**: Press `Tab` inside the Queue screen to focus the lyrics pane and scroll with `Up` / `Down` / `PageUp` / `PageDown`. Press `Tab` again to return focus to the track list.
- **Instant Audio Start**: Pressing `Enter` on a track now starts playback immediately. Tag parsing, metadata loading, and lyrics fetching all happen in a background thread so the UI never blocks.
- **Non-Blocking Text Preview**: Text file previews in the Browser no longer read from disk on the render thread. The file is loaded in a background thread and cached; the pane shows a loading indicator until it is ready.
- **Faster Event Loop**: The main loop now drains all pending events before each draw, eliminating sluggish navigation when keys are pressed in quick succession.
- **Code Cleanup**: Removed the unreachable Collections screen (all navigation, rendering, and state for it), removed unused internal methods, and rewrote comments to be concise and in informal Mexican Spanish.

### v0.3.0

- **High-Resolution Graphics Support (Kitty/Sixel)**: Integrated native graphics protocols in terminal emulators supporting them (like Kitty or Sixel), falling back gracefully to Unicode block characters in others (like Alacritty).
- **Tabbed Pane Navigation**: Added `Tab` and `Shift+Tab` to cycle focus forward and backward between panels (`Directories` -> `Files` -> `Preview`), skipping `Preview` if no preview is available.
- **Dedicated Directory Traversal**: Configured `Left arrow` (or `h`) and `Right arrow` (or `l`) to traverse parent and child folders respectively, avoiding accidental pane switching.
- **Improved Drag-and-Drop Parsing**: Rewrote path parsing for bracketed paste drag-and-drops to correctly support absolute paths containing spaces, unquoted characters, or URL encodings.
- **Wayland / X11 File Manager Copy-Paste**: Corrected file path clipboard exporting (`Y`) by eliminating redundant plain text overrides, allowing graphical file managers (like Thunar or Dolphin) to receive `text/uri-list` data for direct pasting.
- **Structured Desktop Previews**: Added native preview rendering cards for `.desktop` configurations (displaying Name, Exec, Icon, categories, comments, and config details).
- **Streamlined Collections**: Removed collections creation, adding, and menu shortcuts to focus solely on high-speed directory selection, copying, and moving.

### v0.2.0

- **Recursive Directory Operations**: Enabled selecting folders entirely (using `Space` key) to copy, move, or delete them recursively.
- **Path Autocompletion**: Added `Tab` autocompletion for copy/move target inputs.
- **Subdirectory Indicators**: Added visual markers (`▸`) in the browser list indicating folders that contain further subdirectories.
- **Clean Layout Redraws**: Resolved screen corruption and leftover visual artifacts when closing file preview panels.
- **Improved Audio Feedback**: Handled connection and ALSA device playback issues gracefully in the visual player dashboard.
- **Preview Line Wrapping**: Enabled line wrapping in the code/text preview pane.
- **Cursor Visibility**: Added support for standard visual text cursor positioning in all text dialog windows (`Rename`, `CopyPath`, `MovePath`, `CreateCollection`, `Search`).
- **Compact Dialog Heights**: Shrunk progress, confirm, and rename/input dialog box heights to align closely with content limits.
- **Sudo-Safe Helper Launcher**: Handled spawning GUI and audio players (like VLC) safely when `stash` runs with `sudo` permissions by dropping root privileges to the original `SUDO_USER` and forwarding essential display/audio variables (`DISPLAY`, `XAUTHORITY`, `WAYLAND_DISPLAY`, etc.).

## Author

- **gibranlp**
- Homepage: [gibranlp.dev](https://gibranlp.dev)
- Repository: [github.com/gibranlp/stash](https://github.com/gibranlp/stash)
- Email: thisdoesnotwork@gibranlp.dev

---