use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::{Duration, Instant};
use rodio::{Decoder, OutputStream, Sink, Source};
use lofty::prelude::*;
use lofty::probe::Probe;
use crate::models::{AudioMetadata, PlaybackStatus};

#[derive(Debug)]
pub enum AudioCommand {
    Play(PathBuf),
    Pause,
    Resume,
    Stop,
    SetVolume(u32),
    Seek(Duration),
}

pub struct AudioSharedState {
    pub current_track: Option<PathBuf>,
    pub status: PlaybackStatus,
    pub elapsed_secs: u64,
    pub duration_secs: u64,
    pub volume: u32,
    pub repeat: bool,
    pub shuffle: bool,
    pub metadata: Option<AudioMetadata>,
    pub device_error: Option<String>,
    pub visualizer_data: Vec<f32>,
}

pub struct AudioEngine {
    pub command_tx: Sender<AudioCommand>,
    pub shared_state: Arc<Mutex<AudioSharedState>>,
}

impl AudioEngine {
    pub fn new(event_tx: Sender<crate::events::Event>, default_volume: u32) -> Self {
        let (command_tx, command_rx) = channel::<AudioCommand>();
        let shared_state = Arc::new(Mutex::new(AudioSharedState {
            current_track: None,
            status: PlaybackStatus::Stopped,
            elapsed_secs: 0,
            duration_secs: 0,
            volume: default_volume,
            repeat: false,
            shuffle: false,
            metadata: None,
            device_error: None,
            visualizer_data: vec![0.0; 160],
        }));

        let state_clone = Arc::clone(&shared_state);
        thread::spawn(move || {
            // Initialize audio stream. If it fails, capture the error.
            let stream_result = OutputStream::try_default();
            let mut _stream = None;
            let mut stream_handle = None;

            match stream_result {
                Ok((s, h)) => {
                    _stream = Some(s);
                    stream_handle = Some(h);
                }
                Err(e) => {
                    let mut st = state_clone.lock().unwrap();
                    st.device_error = Some(e.to_string());
                }
            }

            let mut sink: Option<Sink> = None;
            if let Some(ref handle) = stream_handle {
                if let Ok(s) = Sink::try_new(handle) {
                    sink = Some(s);
                }
            }

            let mut last_tick = Instant::now();
            let mut elapsed_millis: u128 = 0;
            let (visualizer_tx, visualizer_rx) = channel::<Vec<f32>>();
            let mut sliding_buffer = vec![0.0; 512];

            loop {
                // Process incoming command
                while let Ok(cmd) = command_rx.try_recv() {
                    match cmd {
                        AudioCommand::Play(path) => {
                            if let Some(ref s) = sink {
                                s.stop();
                            }
                            elapsed_millis = 0;

                            // Create a new sink to reset audio buffers completely
                            if let Some(ref handle) = stream_handle {
                                if let Ok(s) = Sink::try_new(handle) {
                                    sink = Some(s);
                                }
                            }

                            if let Ok(file) = File::open(&path) {
                                let reader = BufReader::new(file);
                                if let Ok(source) = Decoder::new(reader) {
                                    // Extract duration and metadata
                                    let mut title = None;
                                    let mut artist = None;
                                    let mut album = None;
                                    let mut duration_secs = None;

                                    if let Ok(tagged_file) = Probe::open(&path).and_then(|p| p.read()) {
                                        if let Some(tag) = tagged_file.primary_tag() {
                                            title = tag.title().map(|s| s.to_string());
                                            artist = tag.artist().map(|s| s.to_string());
                                            album = tag.album().map(|s| s.to_string());
                                        }
                                        let properties = tagged_file.properties();
                                        duration_secs = Some(properties.duration().as_secs());
                                    }

                                    let meta = AudioMetadata {
                                        title: title.or_else(|| {
                                            path.file_name().map(|f| f.to_string_lossy().into_owned())
                                        }),
                                        artist,
                                        album,
                                        duration_secs,
                                    };

                                    let current_vol = {
                                        let st = state_clone.lock().unwrap();
                                        st.volume
                                    };

                                    if let Some(ref s) = sink {
                                        s.set_volume(current_vol as f32 / 100.0);
                                        let vis_source = VisualizerSource::new(source.convert_samples::<f32>(), visualizer_tx.clone());
                                        s.append(vis_source);
                                    }

                                    let mut st = state_clone.lock().unwrap();
                                    st.current_track = Some(path.clone());
                                    st.status = PlaybackStatus::Playing;
                                    st.elapsed_secs = 0;
                                    st.duration_secs = duration_secs.unwrap_or(0);
                                    st.metadata = Some(meta);
                                    last_tick = Instant::now();
                                }
                            }
                        }
                        AudioCommand::Pause => {
                            if let Some(ref s) = sink {
                                s.pause();
                            }
                            let mut st = state_clone.lock().unwrap();
                            st.status = PlaybackStatus::Paused;
                        }
                        AudioCommand::Resume => {
                            if let Some(ref s) = sink {
                                s.play();
                            }
                            let mut st = state_clone.lock().unwrap();
                            st.status = PlaybackStatus::Playing;
                            last_tick = Instant::now();
                        }
                        AudioCommand::Stop => {
                            if let Some(ref s) = sink {
                                s.stop();
                            }
                            let mut st = state_clone.lock().unwrap();
                            st.current_track = None;
                            st.status = PlaybackStatus::Stopped;
                            st.elapsed_secs = 0;
                            st.duration_secs = 0;
                            st.metadata = None;
                            st.visualizer_data.fill(0.0);
                            elapsed_millis = 0;
                        }
                        AudioCommand::SetVolume(vol) => {
                            let mut st = state_clone.lock().unwrap();
                            st.volume = vol.min(100);
                            if let Some(ref s) = sink {
                                s.set_volume(st.volume as f32 / 100.0);
                            }
                        }
                        AudioCommand::Seek(pos) => {
                            if let Some(ref s) = sink {
                                if s.try_seek(pos).is_ok() {
                                    elapsed_millis = pos.as_millis();
                                    let mut st = state_clone.lock().unwrap();
                                    st.elapsed_secs = pos.as_secs();
                                    last_tick = Instant::now();
                                }
                            }
                        }
                    }
                }

                // Process visualizer sample buffer
                let mut new_samples = Vec::new();
                while let Ok(mut chunk) = visualizer_rx.try_recv() {
                    new_samples.append(&mut chunk);
                }

                if !new_samples.is_empty() {
                    let take_len = new_samples.len().min(512);
                    let start_idx = new_samples.len() - take_len;
                    let latest_chunk = &new_samples[start_idx..];

                    let shift = latest_chunk.len();
                    if shift >= 512 {
                        sliding_buffer = latest_chunk.to_vec();
                    } else {
                        sliding_buffer.drain(0..shift);
                        sliding_buffer.extend_from_slice(latest_chunk);
                    }

                    // Apply Hanning Window to decrease spectral leakage
                    let mut fft_input = vec![Complex::new(0.0, 0.0); 512];
                    for i in 0..512 {
                        let multiplier = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / 511.0).cos());
                        fft_input[i] = Complex::new(sliding_buffer[i] * multiplier, 0.0);
                    }
                    fft(&mut fft_input);

                    // Compute FFT Bins Magnitude
                    let mut magnitudes = vec![0.0; 256];
                    for i in 0..256 {
                        let c = fft_input[i];
                        magnitudes[i] = (c.re * c.re + c.im * c.im).sqrt();
                    }

                    // Group into 160 frequency bands logarithmically
                    let num_bars = 160;
                    let mut new_bars = vec![0.0; num_bars];
                    for i in 0..num_bars {
                        let start = (256.0 * (i as f32 / num_bars as f32).powf(1.8)) as usize;
                        let end = (256.0 * ((i + 1) as f32 / num_bars as f32).powf(1.8)) as usize;
                        let end = end.min(256).max(start + 1);

                        let sum: f32 = magnitudes[start..end].iter().sum();
                        let avg = sum / (end - start) as f32;

                        let boost = 1.0 + (i as f32 / num_bars as f32) * 2.5;
                        new_bars[i] = avg * boost * 0.7; // Gain scaling multiplier
                    }

                    // Write to shared state visualizer data using decay filter
                    let mut st = state_clone.lock().unwrap();
                    if st.visualizer_data.len() != num_bars {
                        st.visualizer_data = vec![0.0; num_bars];
                    }
                    for i in 0..num_bars {
                        st.visualizer_data[i] = (st.visualizer_data[i] * 0.70).max(new_bars[i]);
                    }
                } else {
                    let mut st = state_clone.lock().unwrap();
                    if st.status != PlaybackStatus::Playing {
                        st.visualizer_data.fill(0.0);
                    } else {
                        // Smoothly decay visuals if no new samples are actively fetched
                        for val in &mut st.visualizer_data {
                            *val = (*val * 0.85).max(0.0);
                        }
                    }
                }

                let now = Instant::now();
                let delta = now.duration_since(last_tick);
                last_tick = now;

                let send_finished = {
                    let mut st = state_clone.lock().unwrap();
                    if st.status == PlaybackStatus::Playing {
                        elapsed_millis += delta.as_millis();
                        st.elapsed_secs = (elapsed_millis / 1000) as u64;

                        // Check if playback has finished
                        let mut empty = true;
                        if let Some(ref s) = sink {
                            empty = s.empty();
                        }

                        if empty {
                            st.status = PlaybackStatus::Stopped;
                            st.elapsed_secs = 0;
                            st.current_track = None;
                            st.metadata = None;
                            st.visualizer_data.fill(0.0);
                            elapsed_millis = 0;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if send_finished {
                    let _ = event_tx.send(crate::events::Event::AudioFinished);
                }

                // 30ms sleep allows high resolution updates (approx. 33 FPS) for visualizers
                thread::sleep(Duration::from_millis(30));
            }
        });

        Self {
            command_tx,
            shared_state,
        }
    }

    pub fn play(&self, path: PathBuf) {
        let _ = self.command_tx.send(AudioCommand::Play(path));
    }

    pub fn pause(&self) {
        let _ = self.command_tx.send(AudioCommand::Pause);
    }

    pub fn resume(&self) {
        let _ = self.command_tx.send(AudioCommand::Resume);
    }

    pub fn stop(&self) {
        let _ = self.command_tx.send(AudioCommand::Stop);
    }

    pub fn set_volume(&self, volume: u32) {
        let _ = self.command_tx.send(AudioCommand::SetVolume(volume));
    }

    pub fn seek(&self, position: Duration) {
        let _ = self.command_tx.send(AudioCommand::Seek(position));
    }
}

// Custom Rodio Source that intercepts samples and forwards them to the visualizer
pub struct VisualizerSource<I>
where
    I: Source<Item = f32>,
{
    input: I,
    sender: Sender<Vec<f32>>,
    buffer: Vec<f32>,
    channels: u16,
}

impl<I> VisualizerSource<I>
where
    I: Source<Item = f32>,
{
    pub fn new(input: I, sender: Sender<Vec<f32>>) -> Self {
        let channels = input.channels();
        Self {
            input,
            sender,
            buffer: Vec::with_capacity(512 * channels as usize),
            channels,
        }
    }
}

impl<I> Iterator for VisualizerSource<I>
where
    I: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.input.next();
        if let Some(sample) = item {
            self.buffer.push(sample);
            let chunk_size = 512 * self.channels as usize;
            if self.buffer.len() >= chunk_size {
                let chunk = std::mem::replace(&mut self.buffer, Vec::with_capacity(chunk_size));
                let mut mono = Vec::with_capacity(chunk.len() / self.channels as usize);
                let step = self.channels as usize;
                for chunk_slice in chunk.chunks_exact(step) {
                    let sum: f32 = chunk_slice.iter().sum();
                    mono.push(sum / self.channels as f32);
                }
                let _ = self.sender.send(mono);
            }
        }
        item
    }
}

impl<I> Source for VisualizerSource<I>
where
    I: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.input.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.input.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.input.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }
}

// Complex number structure for raw Cooley-Tukey FFT implementation
#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }
    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }
    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }
    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }
}

// Cooley-Tukey Radix-2 FFT
fn fft(input: &mut [Complex]) {
    let n = input.len();
    if n <= 1 {
        return;
    }

    let mut even = vec![Complex::new(0.0, 0.0); n / 2];
    let mut odd = vec![Complex::new(0.0, 0.0); n / 2];
    for i in 0..n / 2 {
        even[i] = input[2 * i];
        odd[i] = input[2 * i + 1];
    }

    fft(&mut even);
    fft(&mut odd);

    for k in 0..n / 2 {
        let t = std::f32::consts::PI * 2.0 * (k as f32) / (n as f32);
        let w = Complex::new(t.cos(), -t.sin());
        let odd_w = odd[k].mul(w);
        input[k] = even[k].add(odd_w);
        input[k + n / 2] = even[k].sub(odd_w);
    }
}
