//! Native decoding first; stream unsupported formats through FFmpeg without
//! rewriting the original or buffering an entire song in memory.
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

use rodio::{Decoder, Source, source::SeekError};

pub(super) fn open(path: &Path) -> Result<Box<dyn Source<Item = f32> + Send>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    match Decoder::new(BufReader::new(file)) {
        Ok(source) => Ok(Box::new(source.convert_samples::<f32>())),
        Err(native_error) => FfmpegSource::new(path)
            .map(|source| Box::new(source) as Box<dyn Source<Item = f32> + Send>)
            .map_err(|e| format!("Built-in decoder: {native_error}; FFmpeg fallback: {e}")),
    }
}

// Live internet radio: always FFmpeg, which handles HTTP reconnects and ICY
// streams. Only http(s) is accepted so a station can't name a local protocol.
pub(super) fn open_stream(url: &str) -> Result<Box<dyn Source<Item = f32> + Send>, String> {
    let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("Unsupported stream URL: {url}"));
    }
    let mut source = FfmpegSource::start(Input::Url(parsed.into()), Duration::ZERO, None)
        .map_err(|e| e.to_string())?;
    source.prime().map_err(|e| e.to_string())?;
    Ok(Box::new(source))
}

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

enum Input {
    File(PathBuf),
    Url(String),
}

struct FfmpegSource {
    input: Input,
    child: Child,
    output: BufReader<ChildStdout>,
    first_sample: Option<f32>,
    duration: Option<Duration>,
}

impl FfmpegSource {
    fn new(path: &Path) -> io::Result<Self> {
        // An absolute path also prevents filenames from being treated as options
        // or FFmpeg protocol URLs. Arguments never pass through a shell.
        let input = Input::File(path.canonicalize()?);
        let mut source = Self::start(input, Duration::ZERO, probe_duration(path))?;
        source.prime()?;
        Ok(source)
    }

    fn start(input: Input, position: Duration, duration: Option<Duration>) -> io::Result<Self> {
        let mut command = Command::new("ffmpeg");
        command.args(["-nostdin", "-v", "error"]);
        match &input {
            Input::File(path) => {
                command
                    .arg("-ss")
                    .arg(format!("{:.9}", position.as_secs_f64()))
                    .arg("-i")
                    .arg(path);
            }
            Input::Url(url) => {
                // Give up on a dead connection after 15s instead of hanging silently.
                command
                    .args(["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "5"])
                    .args(["-rw_timeout", "15000000", "-i"])
                    .arg(url);
            }
        }
        let mut child = command
            .args([
                "-map",
                "0:a:0",
                "-vn",
                "-sn",
                "-dn",
                "-f",
                "f32le",
                "-acodec",
                "pcm_f32le",
                "-ar",
                "48000",
                "-ac",
                "2",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // Stash owns the terminal; decoder diagnostics must not corrupt it.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!(
                        "Cannot start ffmpeg (install FFmpeg for additional audio formats): {e}"
                    ),
                )
            })?;
        let output = BufReader::new(child.stdout.take().expect("piped FFmpeg stdout"));
        Ok(Self {
            input,
            child,
            output,
            first_sample: None,
            duration,
        })
    }

    fn read_sample(&mut self) -> io::Result<f32> {
        let mut bytes = [0; 4];
        self.output.read_exact(&mut bytes)?;
        Ok(f32::from_le_bytes(bytes))
    }

    fn prime(&mut self) -> io::Result<()> {
        self.first_sample = Some(self.read_sample().map_err(|e| {
            io::Error::new(
                e.kind(),
                "FFmpeg could not decode an audio stream from this file",
            )
        })?);
        Ok(())
    }
}

impl Iterator for FfmpegSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        self.first_sample.take().or_else(|| self.read_sample().ok())
    }
}

impl Source for FfmpegSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let Input::File(path) = &self.input else {
            return Err(SeekError::NotSupported { underlying_source: "live stream" });
        };
        let position = self
            .duration
            .map_or(position, |duration| position.min(duration));
        let mut replacement = Self::start(Input::File(path.clone()), position, self.duration)
            .map_err(|e| SeekError::Other(Box::new(e)))?;
        // Seeking to the end legitimately produces no samples.
        if Some(position) != self.duration {
            replacement
                .prime()
                .map_err(|e| SeekError::Other(Box::new(e)))?;
        }
        *self = replacement;
        Ok(())
    }
}

impl Drop for FfmpegSource {
    fn drop(&mut self) {
        // Stop, skip, seek, and application exit must release the decoder even
        // when it is blocked writing into a full pipe (for example while paused).
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn probe_duration(path: &Path) -> Option<Duration> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path.canonicalize().ok()?)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let seconds = std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    Duration::try_from_secs_f64(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(extension: &str, format: &str, codec: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "stash-audio-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join(format!("Every Byte - A Music Parody.{extension}"));
            let status = Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=2",
                    "-c:a",
                    codec,
                    "-f",
                    format,
                ])
                .arg(&path)
                .status()
                .expect("audio regression tests require ffmpeg");
            assert!(status.success());
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn mislabeled_webm_opus_plays_and_seeks() {
        let fixture = Fixture::new("mp3", "webm", "libopus");
        assert!(Decoder::new(BufReader::new(File::open(&fixture.0).unwrap())).is_err());
        let mut source = open(&fixture.0).unwrap();
        assert_eq!(source.channels(), 2);
        assert_eq!(source.sample_rate(), 48_000);
        assert!((source.total_duration().unwrap().as_secs_f64() - 2.0).abs() < 0.05);
        let samples: Vec<_> = source.by_ref().collect();
        assert!((samples.len() as i64 - 192_000).abs() < 2000);
        assert!(samples.iter().all(|s| s.is_finite()));
        assert!(samples.iter().any(|s| s.abs() > 0.01));
        source.try_seek(Duration::from_secs(1)).unwrap();
        let remaining = source.by_ref().count();
        assert!(
            (remaining as i64 - 96_000).abs() < 2000,
            "remaining samples: {remaining}"
        );
        source.try_seek(Duration::ZERO).unwrap();
        assert!(source.next().is_some());
        source.try_seek(Duration::from_secs(10)).unwrap();
        assert!(source.next().is_none());
    }

    #[test]
    fn real_mp3_keeps_native_decoder() {
        let fixture = Fixture::new("mp3", "mp3", "libmp3lame");
        let native = Decoder::new(BufReader::new(File::open(&fixture.0).unwrap())).unwrap();
        let mut source = open(&fixture.0).unwrap();
        // Native fixture is mono at 44.1 kHz, unlike the fallback's stereo PCM.
        assert_eq!(source.channels(), native.channels());
        assert_eq!(source.sample_rate(), native.sample_rate());
        assert_eq!(source.channels(), 1);
        assert!(source.by_ref().any(|s| s.abs() > 0.01));
    }

    #[test]
    fn corrupt_audio_reports_failure() {
        let fixture = Fixture::new("mp3", "webm", "libopus");
        std::fs::write(&fixture.0, b"not an audio file").unwrap();
        let error = match open(&fixture.0) {
            Ok(_) => panic!("corrupt file must fail"),
            Err(error) => error,
        };
        assert!(error.contains("Built-in decoder:"));
        assert!(error.contains("FFmpeg could not decode"));
    }

    #[test]
    fn dropping_paused_source_reaps_process() {
        let fixture = Fixture::new("opus", "ogg", "libopus");
        let mut source = FfmpegSource::new(&fixture.0).unwrap();
        assert!(source.child.try_wait().unwrap().is_none());
        let pid = source.child.id();
        drop(source);
        #[cfg(target_os = "linux")]
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn stream_urls_must_be_http() {
        let error = open_stream("file:///etc/hostname").err().unwrap();
        assert!(error.contains("Unsupported stream URL"));
    }

    #[test]
    fn radio_stream_decodes_when_requested() {
        let Ok(url) = std::env::var("STASH_TEST_RADIO") else {
            return;
        };
        let mut source = open_stream(&url).unwrap();
        assert_eq!(source.total_duration(), None);
        let samples: Vec<_> = source.by_ref().take(SAMPLE_RATE as usize * 2).collect();
        assert!(samples.iter().all(|s| s.is_finite()));
        assert!(samples.iter().any(|s| s.abs() > 0.001));
        assert!(source.try_seek(Duration::from_secs(1)).is_err());
    }

    #[test]
    fn user_track_decodes_when_requested() {
        let Some(path) = std::env::var_os("STASH_TEST_AUDIO") else {
            return;
        };
        let mut source = open(Path::new(&path)).unwrap();
        let duration = source.total_duration().unwrap().as_secs_f64();
        let samples_per_second = source.sample_rate() as f64 * source.channels() as f64;
        let mut count = 0usize;
        let mut audible = false;
        for sample in source.by_ref() {
            assert!(sample.is_finite());
            audible |= sample.abs() > 0.001;
            count += 1;
        }
        assert!(audible && count > SAMPLE_RATE as usize * CHANNELS as usize);
        assert!((count as f64 / samples_per_second - duration).abs() < 1.0);
        source.try_seek(Duration::from_secs(60)).unwrap();
        assert!(source.next().is_some());
    }
}
