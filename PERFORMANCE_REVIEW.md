# Performance and release review

## 2026-11-18 performance pass

Reviewed rendering, audio/visualizer processing, recursive search and directory browsing, queue insertion, library sorting/scanning, preview workers, artwork enrichment, podcast RSS/download workers, radio updates, statistics persistence, and update/file-operation paths. The changes below target measured hot paths and worker lifetime issues; this is not a claim that every workload is optimized.

### Changes

- Browser, Queue, Library, and Podcasts build only the visible single-line list rows. Selection and stored scroll offsets remain absolute, including filtered/shuffled queue numbering. A buffer-equivalence regression test compares full and windowed list output at the top, middle, and bottom of small and large viewports.
- Library and directory sorts compute lowercase keys once per item, rather than allocating strings repeatedly inside comparisons. Missing metadata, smart-playlist ordering, and case-insensitive ordering are covered by regression tests.
- Batch queue insertion uses a temporary membership hash set. It preserves input order, removes duplicates, and preserves the current track and the already-played shuffle prefix.
- Recursive browser search runs in one cancellable worker, with an 80 ms typing debounce, generation checks, and a 100-result limit. Replaced queries stop their walks; obsolete results cannot remain selectable. Clearing a query cancels pending work.
- The audio worker sleeps on its command channel when paused/stopped, with commands waking it immediately. Idle timeout wakeups fall from about 62 per second to four. An explicit shutdown command releases the worker when its engine is dropped; previously an internally retained sender prevented channel disconnection from stopping it.
- Visualizer Hann-window and frequency-band tables are computed once per worker instead of on every frame. The extra 512-sample heap buffer/copy is removed. Audio callback sample preservation remains covered by tests.
- Embedded-cover discovery skips audio-property scanning, because artwork lookup only needs tags. Directory classification reuses directory-entry file types, while preserving symlink behavior.

### Measured results

Optimized Linux build, 160×45 terminal geometry. List rendering uses the same synthetic fixtures before/after, 30 measured draws after one warm-up draw. These timings measure widget construction, rendering and TestBackend buffer differences, not terminal/GPU presentation or filesystem scan time.

| Workload | Before | After |
| --- | ---: | ---: |
| Browser, 10,000 files | 5.262 ms/frame | 0.461 ms/frame |
| Queue, 10,000 tracks | 2.852 ms/frame | 0.416 ms/frame |
| Library, 10,000 tracks sorted by artist | 27.153 ms/frame | 2.717 ms/frame |
| Podcasts, 2,000 episodes | 0.694 ms/frame | 0.306 ms/frame |
| Enqueue 10,000 unique paths, shuffle inactive | 2737.815 ms | 2.235 ms |

A separate pseudoterminal run compared the saved pre-change release binary with the new release binary, using a directory of 10,000 empty MP3 files, private configuration/cache directories, a private D-Bus session, disabled automatic mounts/update prompts, and no playback. After a 0.5-second warm-up, process CPU was sampled for three seconds while terminal output was drained continuously:

| Measurement | Before | After |
| --- | ---: | ---: |
| CPU, percentage of one core | 9.32% | 1.67% |
| Resident memory at end | 27,668 KiB | 21,316 KiB |
| First browser frame | 58.5 ms | 53.2 ms |

This short local sample is workload-specific; it does not predict CPU/memory savings for every library, terminal, or audio decoder. The rotating-cover probe also completed 600 encoded frames with the same resident memory after warm-up and at completion; no bank of animation frames is retained.

### Verification and reproduction

- `cargo test --locked --offline`: 92 tests passed; six manual/network unit probes and the separate MPRIS integration test are ignored by default.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: passed.
- `cargo test --locked --offline --release --bin stash probe -- --ignored --nocapture --test-threads=1`: rendering, bulk queue and vinyl resource probes passed.
- `dbus-run-session -- cargo test --locked --offline --test mpris -- --ignored`: passed, including the installed desktop client's metadata check when available.
- `cargo build --locked --offline --release`: passed.
- Pseudoterminal smoke check: both binaries rendered the 10,000-file browser and exited cleanly through `q`.

Directory refresh and some explicit filesystem operations still run synchronously, so slow removable/network drives can remain a source of pauses. Library sorting still computes a view on each draw; it now avoids per-comparison string allocations but does not keep a persistent sorted-view cache. Tests ran locally on Linux; real-terminal animation quality, audible playback and other platforms still require their own verification.

## Historical review: 2026-10-06

Reviewed the working tree on 2026-10-06, including the existing uncommitted changes.

## Fixed

| Finding | Change |
| --- | --- |
| Failed image decoding left the preview loading forever. | Failed image and text reads now finish with a visible error. |
| Each selection spawned another preview thread, with stale work competing for CPU and disk. | Each preview stage has one worker and one replaceable pending request. Generation checks reject stale results, including repeated selection of the same path. |
| Text previews read entire files. | Preview reads stop at 256 KiB and display at most 5,000 lines, with a truncation marker. |
| Large image decoding/resizing and Kitty/Sixel encoding delayed navigation. | Image loads use decoder limits and a 1,024-pixel thumbnail. Graphics resizing and encoding run in workers. Halfblock rendering resizes directly to terminal cells. |
| Album-art discovery scanned audio tags on the UI thread. | Cover discovery runs in a worker, even when system media controls are unavailable. Media metadata updates when tags or cover art arrive. |
| The device callback allocated visualizer buffers and used an unbounded queue. | Fixed-size sample frames and a two-frame queue; a full queue drops visualization frames while preserving audio samples. |
| The recursive FFT allocated hundreds of temporary vectors per frame. | Iterative in-place FFT; stack arrays for spectral calculations. |
| Rendering held the shared audio mutex while building the player/visualizer. | Rendering uses a snapshot and releases the mutex before constructing widgets. |
| Idle redraws and unbounded event batches wasted work or delayed rendering. | Idle timeout redraws are skipped, and input batches are bounded. Playback retains periodic redraws. |
| Playback failures could leave stale state without an error. | Device/file/decoder failures clear playback state and show an error. |
| Two tests referenced removed methods; release builds had no test gate. | Updated tests to current behavior, added CI tests/lint/release builds, and added Linux tests to the release workflow. |

## Local verification

- `cargo test --offline --locked`: 27 tests passed.
- `cargo clippy --offline --locked --all-targets -- -D warnings`: passed.
- `cargo build --offline --locked --release`: passed.
- `git diff --check`: passed.
- Pseudoterminal smoke tests: invalid image error, text preview, valid BMP, player rendering, silent WAV playback, and clean exit. Halfblock, Kitty, and Sixel output paths passed. These check terminal output, not how each real terminal displays graphics.
- Optimized 512-point FFT microbenchmark, 10,000 iterations: recursive 34.49 µs/frame; iterative 5.97 µs/frame (about 5.8× faster). This measures the transform, not whole-app latency or audible playback quality.

## Before release

- Listen to representative MP3, FLAC, M4A, Ogg, and WAV files while browsing, changing tracks, seeking, and resizing the terminal. Local automated playback used a silent WAV.
- Validate real Kitty/Sixel terminals and macOS/Windows builds, media keys, and audio devices. Only Linux was run locally; hosted CI has not run for these changes yet.
- Check large local libraries and removable/network drives. Directory refresh and recursive search still perform synchronous filesystem work; these are separate potential sources of UI pauses.

Test the built app with `./target/release/stash /path/to/music`. No release was published.
