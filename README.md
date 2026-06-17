# STASH

STASH is a fast, keyboard-driven terminal music browser, player, and file organizer written in Rust. It combines dual-pane file system navigation with audio playback, custom virtual playlist management (Collections), asynchronous file operations, and a dynamic real-time FFT spectrum visualizer.

Designed for terminal power users, it supports smooth, standard list scrolling, quick page navigation, and seamlessly integrates with terminal color schemes (e.g. `pywal`).

---

## Key Features

1. **Dual-Pane File Browser**: Navigate directories recursively on the left, and view/play files on the right.
2. **Dynamic FFT Visualizer**: Real-time 160-channel audio frequency spectrum analyzer (Cooley-Tukey Radix-2 FFT + Hanning window) that adjusts dynamically to your terminal width, styled with standard ANSI colors to work out of the box with `pywal` schemes.
3. **Asynchronous File Operations**: Perform copy (`v`), move (`y`), and delete (`d`) operations on background worker threads with a progress bar and current filename overlay, ensuring the UI remains completely responsive.
4. **Stateful Viewport Scrolling**: Standard scroll list handling where the cursor moves within the visible screen first, only scrolling the list viewport when hitting the top or bottom edges.
5. **Quick Page Navigation**: Press `PageUp` and `PageDown` to jump selection cursor by 10 items.
6. **Virtual Collections**: Organize favorite tracks or files into virtual playlists (persisted in `~/.config/stash/collections.json`) without duplicate disk space.
7. **Playback Queue**: Queue multiple songs using standard commands and skip tracks cleanly with shuffle/repeat toggles.
8. **Live Search**: Recursively search directory items live with incremental character matching using walkdir.
9. **Directory Navigation Prepends**: Quick navigation directories `.` and `..` are automatically sorted at the top of the folder list.

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

Ensure you have Rust and Cargo installed. On Linux, ensure `alsa-lib` development dependencies are installed (e.g. `libasound2-dev` on Debian/Ubuntu).

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
