# Performance and release review

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
