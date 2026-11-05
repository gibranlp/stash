# Changelog

## 0.9.2 — 2026-11-04

- Open audio files directly from a file manager ("Open with Stash"), including `file://` URIs: Stash starts in the file's folder, queues it and plays the chosen track.
- Prefer synced lyrics: keep searching LRCLIB for a synced upload when the exact match is plain, add NetEase Cloud Music as a synced source and lyrics.ovh as a last-resort plain source, and look for synced lyrics online even when the file has plain local lyrics.
- Retry lyrics lookups after temporary failures (offline, timeouts, rate limits) with a growing delay, show a stale cached copy when services are unreachable, and re-check plain or missing lyrics after a week.
- Parse `[mm:ss:xx]` timestamps and strip enhanced-LRC per-word marks.
- Add a Mirror visualizer mode (`v`) and smooth the spectrum into one continuous, palette-colored shape with less flicker; the visualizer now fills the space below the queue.
- Shuffle the queue right away when playback starts with shuffle on.

## 0.9.1 — 2026-11-02

- Fix "permission denied" when updating a Stash installed in a system directory such as `/usr/bin`: the verified update is now installed with `sudo` (or `doas`), which asks for your password.
- The install script now installs to `/usr/bin` on Linux and `/usr/local/bin` on macOS (asking for your password) and removes an older copy in `~/.local/bin`.
- Automatically mount USB sticks and external drives through udisks2 when Stash starts or when they are plugged in (Linux). Set `"auto_mount": false` in `config.json` to disable.

## 0.9.0 — 2026-10-30

- Add an Internet Radio screen (`5`) with the HAK.FM station lineup streamed from SomaFM through the existing audio engine, so volume, the visualizer, MPRIS and Discord activity work as for local tracks.
- Show the song on air and the station's recently played tracks, refreshed every 30 seconds; media next/previous keys switch stations while radio plays.
- Star favorite stations (`f`), show only favorites (`F`), and search stations by name, genre or description (`/`). Favorites persist in `config.json` as `radio_favorites`.
- Pausing radio disconnects and resuming reconnects live; dropped streams no longer advance the playback queue.

- Offer updates in a popup instead of replacing the executable in the background. Pressing `Y` closes Stash, downloads and verifies the update, installs it and reopens Stash in the new version with the same arguments; a failed install keeps the current version and reopens it.
- Fix "access denied" when updating on Windows by moving the running executable aside before installing the new one.
- `auto_update: false` now disables the popup only; `U` still installs updates.

## 0.8.0 — 2026-10-25

- Automatically install newer stable GitHub releases in the background, with periodic checks, retryable errors, and update status on every screen. Add `auto_update: false` for manual confirmation.
- Select actual uploaded release assets, verify size and SHA-256, stream downloads to temporary files, and replace the executable atomically. Preserve the installed copy when verification fails and prevent duplicate updates before restart.

- Fall back to streaming FFmpeg decoding for unsupported audio, including WebM/Opus files named `.mp3`; preserve seeking and stop decoder processes when playback ends. Recognize `.opus` files as audio.

- Improve lyrics searches with cleaned titles, main-artist variants, artist/title filename parsing, and broader fallback queries.
- Reject unrelated artists and substring title matches; use plain lyrics when a matching edit has different timing.
- Cache successful online lyrics, inspect all embedded tag blocks, and cancel obsolete searches after skipping a track.
- Recover from exact-lookup errors, bound network requests, and respect lyrics-service rate limits.

- Make Discord/Vesktop “Listening to Stash” reconnect reliably, retaining the current song while the client is offline.
- Refresh song metadata and progress after seeking, and clear activity on pause, stop, or exit.
- Discover additional IPC sockets, including Vesktop Flatpak and temporary-directory sockets; skip stale endpoints.
- Validate Discord acknowledgements and handle IPC pings without blocking playback.

## 0.7.1 — 2026-10-08

- Keep Linux MPRIS metadata and artwork responsive during playback updates.
- Encode artwork file URLs correctly and discover embedded artwork across tag types.
- Refresh media metadata when duration changes, retain filename title fallbacks, and report metadata publishing failures.
- Follow timestamped lyrics automatically, accounting for wrapped lines, and highlight the current lyric across the full row.
- Remove the lyrics Tab mode; use PageUp and PageDown to scroll plain lyrics.
- Add regression coverage for artwork URLs, lyric scrolling, and MPRIS delivery through playerctl and Quickshell.

SpectrumOS integration: Quickshell 0.3.1 exposes the artist as a string. SpectrumOS must use `player.trackArtist` instead of calling `.join()` on `player.trackArtists`. This receiver fix was applied locally in SpectrumOS and is separate from this Stash release.
