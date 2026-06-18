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
- **`C`**: Toggle Collections screen dashboard.

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

## Installation & Compilation

Ensure you have Rust and Cargo installed.

### Prerequisites by Operating System:

- **Linux**: Ensure the `alsa-lib` development packages are installed (e.g. `libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora/RHEL/openSUSE).
- **macOS (Mac)**: Works natively out of the box using CoreAudio. No additional external libraries or dependencies are required.

1. Install Rust (if not already installed):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

### 1. Compile and Install globally
Run the following inside the project directory to install `stash` globally under your cargo binaries path:
```bash
cargo install --path .
```
Verify it is accessible by running:
```bash
stash
```

### 2. Manual Release Compilation
To build a standalone release binary:
```bash
cargo build --release
```
The compiled binary will be located at `target/release/stash`. You can copy it to your local binary paths:
```bash
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

## Recent Version Changelog (v0.2.0)

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