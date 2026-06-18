# Future Improvements for STASH

This document details potential features, optimizations, and structural enhancements that can be implemented next to improve the player.

---

## 1. Interactive Queue Management
- **Item Removal**: Allow users to delete specific items from the queue (e.g., using `d` or `x` key) while viewing the playback queue screen.
- **Reordering**: Implement moving items up or down within the active playlist (e.g., using `Ctrl+Up` / `Ctrl+Down` or `J` / `K` modifications).
- **Clear Queue**: Add a shortcut to wipe the active queue without needing to play a new directory.

## 2. Metadata Caching (Database)
- **Local DB (SQLite/sled)**: Create a background indexing service that extracts ID3 tags (artist, album, genre, duration) and saves them to a lightweight local database.
- **Instant Search**: Allow searching the entire music library by tags (e.g., searching for "Daft Punk" matches artist tags even if the folder name is different), rather than relying solely on file names and directories.
- **Faster Startup**: Avoid scanning directories recursively on every startup.

## 3. Dynamic Visualizer Customizations
- **Aesthetic Modes**: Add configuration options or visual keys to cycle visualizer designs (e.g., single-channel spectrum, stereo dual-waveforms, or simple signal levels).
- **Adjustable Decay**: Allow customizing gravity / decay speed of frequency bars within the visualizer config.

## 4. Enhanced Playlists Support
- **M3U / PLS Import**: Add parser support to load standard playlist formats (`.m3u`, `.pls`) directly into the queue.
- **Exporting playlists**: Allow saving the current playback queue back to an `.m3u` file.

## 5. Rich MPRIS Features
- **Seeking Support**: Add seeking control hooks to the MPRIS/souvlaki integration, allowing system utilities (like `playerctl`) or desktop widgets to seek forward/backward directly.
- **Album Art Rendering**: Fetch and expose album art to the OS via local file schemes or embedded metadata paths.

## 6. Playback Control Enhancements
- **Dynamic Audio Output**: Allow users to switch output audio devices (e.g., headphones vs. speakers) from a menu in the TUI.
- **Logarithmic Volume Control**: Change volume increments to a logarithmic scale, matching natural human hearing perception much better than linear steps.
