# Changelog

## 0.7.1 — 2026-10-08

- Keep Linux MPRIS metadata and artwork responsive during playback updates.
- Encode artwork file URLs correctly and discover embedded artwork across tag types.
- Refresh media metadata when duration changes, retain filename title fallbacks, and report metadata publishing failures.
- Follow timestamped lyrics automatically, accounting for wrapped lines, and highlight the current lyric across the full row.
- Remove the lyrics Tab mode; use PageUp and PageDown to scroll plain lyrics.
- Add regression coverage for artwork URLs, lyric scrolling, and MPRIS delivery through playerctl and Quickshell.

SpectrumOS integration: Quickshell 0.3.1 exposes the artist as a string. SpectrumOS must use `player.trackArtist` instead of calling `.join()` on `player.trackArtists`. This receiver fix was applied locally in SpectrumOS and is separate from this Stash release.
