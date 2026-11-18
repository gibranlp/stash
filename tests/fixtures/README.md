# Audio fixtures

These files contain one second of a generated 440 Hz sine wave, encoded as MP3, FLAC, Ogg Vorbis, and M4A. They contain no recorded music. Artwork tests use them to verify that embedding an image preserves decoded audio and existing metadata.

Generated with FFmpeg (replace `EXT` with the format):

```sh
ffmpeg -f lavfi -i sine=frequency=440:sample_rate=44100 -t 1 \
  -metadata title='Artwork test' -metadata artist='Test artist' \
  -metadata album='Test album' artwork.EXT
```
