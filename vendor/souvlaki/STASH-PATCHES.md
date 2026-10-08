Vendored from souvlaki 0.8.3 (MIT), https://github.com/Sinono3/souvlaki.

Local Linux D-Bus backend changes:

- Reduce the idle connection poll from 1000 ms to 10 ms so playback updates do not delay queued metadata and artwork.
- Correct the root property name from `HasTracklist` to the MPRIS-specified `HasTrackList`.

Validate with `dbus-run-session -- cargo test --test mpris -- --ignored`.
The test checks playerctl and, when available, Quickshell and the installed SpectrumOS MediaState component.
