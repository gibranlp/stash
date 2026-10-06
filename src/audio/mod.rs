use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, sync_channel, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant};
use rodio::{Decoder, OutputStream, Sink, Source};
use lofty::prelude::*;
use lofty::probe::Probe;
use crate::models::{AudioMetadata, LyricsState, PlaybackStatus, RepeatMode};

#[derive(Debug)]
pub enum AudioCommand {
    Play(PathBuf),
    Pause,
    Resume,
    Stop,
    SetVolume(u32),
    Seek(Duration),
}

#[derive(Clone)]
pub struct VisualizerFrame {
    pub samples: [f32; 512],
    pub left_peak: f32,
    pub right_peak: f32,
}

#[derive(Clone)]
pub struct AudioSharedState {
    pub current_track: Option<PathBuf>,
    pub status: PlaybackStatus,
    pub elapsed_secs: u64,
    pub elapsed_millis: u64,
    pub duration_secs: u64,
    pub volume: u32,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    pub metadata: Option<AudioMetadata>,
    pub lyrics_state: LyricsState,
    pub device_error: Option<String>,
    pub visualizer_data: Vec<f32>,
    pub visualizer_peaks: Vec<f32>,
    pub visualizer_decay: f32,
    pub left_level: f32,
    pub right_level: f32,
}

pub struct AudioEngine {
    pub command_tx: Sender<AudioCommand>,
    pub shared_state: Arc<Mutex<AudioSharedState>>,
}

impl AudioEngine {
    pub fn new(event_tx: Sender<crate::events::Event>, default_volume: u32, default_repeat: RepeatMode, default_shuffle: bool, default_decay: f32) -> Self {
        let (command_tx, command_rx) = channel::<AudioCommand>();
        let shared_state = Arc::new(Mutex::new(AudioSharedState {
            current_track: None,
            status: PlaybackStatus::Stopped,
            elapsed_secs: 0,
            elapsed_millis: 0,
            duration_secs: 0,
            volume: default_volume,
            repeat: default_repeat,
            shuffle: default_shuffle,
            metadata: None,
            lyrics_state: LyricsState::NotFound,
            device_error: None,
            visualizer_data: vec![0.0; 160],
            visualizer_peaks: vec![0.0; 160],
            visualizer_decay: default_decay,
            left_level: 0.0,
            right_level: 0.0,
        }));

        let state_clone = Arc::clone(&shared_state);
        thread::spawn(move || {
            // Si falla el dispositivo de audio, guardamos el error en el estado
            // pa que la UI lo pueda mostrar en lugar de tronar silencioso
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
            if let Some(ref handle) = stream_handle
                && let Ok(s) = Sink::try_new(handle) {
                    sink = Some(s);
                }

            let mut last_tick = Instant::now();
            let mut elapsed_millis: u128 = 0;
            let (visualizer_tx, visualizer_rx) = sync_channel::<VisualizerFrame>(2);
            let mut sliding_buffer = vec![0.0; 512];

            loop {
                loop {
                    let cmd = match command_rx.try_recv() {
                        Ok(cmd) => cmd,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    };
                    match cmd {
                        AudioCommand::Play(path) => {
                            if let Some(ref s) = sink {
                                s.stop();
                            }
                            elapsed_millis = 0;
                            while visualizer_rx.try_recv().is_ok() {}

                            let prepared = (|| -> Result<_, String> {
                                let handle = stream_handle.as_ref()
                                    .ok_or_else(|| "No audio output device is available".to_string())?;
                                let new_sink = Sink::try_new(handle).map_err(|e| e.to_string())?;
                                let file = File::open(&path).map_err(|e| e.to_string())?;
                                let source = Decoder::new(BufReader::new(file)).map_err(|e| e.to_string())?;
                                Ok((new_sink, source))
                            })();
                            let source = match prepared {
                                Ok((new_sink, source)) => {
                                    sink = Some(new_sink);
                                    source
                                }
                                Err(error) => {
                                    let mut st = state_clone.lock().unwrap();
                                    st.current_track = None;
                                    st.status = PlaybackStatus::Stopped;
                                    st.elapsed_secs = 0;
                                    st.elapsed_millis = 0;
                                    st.duration_secs = 0;
                                    st.metadata = None;
                                    st.lyrics_state = LyricsState::NotFound;
                                    st.device_error = Some(format!("Cannot play {}: {error}", path.display()));
                                    st.visualizer_data.fill(0.0);
                                    st.visualizer_peaks.fill(0.0);
                                    st.left_level = 0.0;
                                    st.right_level = 0.0;
                                    continue;
                                }
                            };
                            let decoder_duration = source.total_duration()
                                .map(|d| d.as_secs())
                                .filter(|&s| s > 0);

                            let current_vol = {
                                let st = state_clone.lock().unwrap();
                                st.volume
                            };

                            // Arrancamos la reproducción de volada, sin esperar
                            // tags ni letras — eso lo jalamos en otro hilo
                            if let Some(ref s) = sink {
                                s.set_volume(current_vol as f32 / 100.0);
                                let vis_source = VisualizerSource::new(source.convert_samples::<f32>(), visualizer_tx.clone());
                                s.append(vis_source);
                            }

                            {
                                let mut st = state_clone.lock().unwrap();
                                st.device_error = None;
                                st.current_track = Some(path.clone());
                                st.status = PlaybackStatus::Playing;
                                st.elapsed_secs = 0;
                                st.elapsed_millis = 0;
                                st.duration_secs = decoder_duration.unwrap_or(0);
                                st.metadata = None;
                                st.lyrics_state = LyricsState::Loading;
                            }
                            last_tick = Instant::now();

                            // Hilo aparte pa leer los tags y buscar letras
                            // sin bloquear el loop principal de audio
                            let bg_state = Arc::clone(&state_clone);
                            let bg_path = path.clone();
                            thread::spawn(move || {
                                let mut title = None;
                                let mut artist = None;
                                let mut album = None;
                                let mut duration_secs: Option<u64> = None;
                                let mut track = None;
                                let mut genre = None;
                                let mut year = None;
                                let mut bitrate = None;
                                let mut sample_rate = None;
                                let mut codec = None;
                                let mut lyrics = load_lyrics(&bg_path);

                                if let Ok(tagged_file) = Probe::open(&bg_path).and_then(|p| p.read()) {
                                    if let Some(tag) = tagged_file.primary_tag().or(tagged_file.first_tag()) {
                                        title = tag.title().map(|s| s.to_string());
                                        artist = tag.artist().map(|s| s.to_string());
                                        album = tag.album().map(|s| s.to_string());
                                        genre = tag.genre().map(|s| s.to_string());
                                        track = tag.track();
                                        year = tag.year();

                                        // Si no encontramos letra en disco, checamos si viene
                                        // embebida en el tag del archivo
                                        if lyrics.is_none()
                                            && let Some(embedded) = tag.get_string(&lofty::tag::ItemKey::Lyrics) {
                                                lyrics = non_empty_lyrics(embedded);
                                            }
                                    }
                                    let properties = tagged_file.properties();
                                    let lofty_dur = properties.duration().as_secs();
                                    if lofty_dur > 0 {
                                        duration_secs = Some(lofty_dur);
                                    }
                                    bitrate = properties.audio_bitrate();
                                    sample_rate = properties.sample_rate();
                                    codec = Some(format!("{:?}", tagged_file.file_type()));
                                }

                                // Si no hay título en el tag, usamos el nombre del archivo
                                let resolved_title = title.clone().or_else(|| {
                                    bg_path
                                        .file_stem()
                                        .map(|f| clean_filename_title(&f.to_string_lossy()))
                                        .filter(|title| !title.is_empty())
                                });

                                // Ojo: verificamos que la rola siga siendo la misma
                                // antes de escribir, no vaya a ser que ya cambiaron de track
                                {
                                    let mut st = bg_state.lock().unwrap();
                                    if st.current_track.as_deref() == Some(&bg_path) {
                                        if let Some(d) = duration_secs {
                                            st.duration_secs = d;
                                        }
                                        st.metadata = Some(AudioMetadata {
                                            title: resolved_title.clone(),
                                            artist: artist.clone(),
                                            album: album.clone(),
                                            duration_secs,
                                            track,
                                            genre,
                                            year,
                                            bitrate,
                                            sample_rate,
                                            codec,
                                            lyrics: lyrics.clone(),
                                        });
                                        st.lyrics_state = if lyrics.is_some() {
                                            LyricsState::Found(lyrics.clone().unwrap())
                                        } else {
                                            LyricsState::Fetching
                                        };
                                    }
                                }

                                // Si no encontramos letra local ni embebida, jalamos de internet
                                if lyrics.is_none() {
                                    if let Some(ref t) = resolved_title {
                                        let result = fetch_lyrics_online(
                                            t,
                                            artist.as_deref(),
                                            album.as_deref(),
                                            duration_secs,
                                        );
                                        let mut st = bg_state.lock().unwrap();
                                        if st.current_track.as_deref() == Some(&bg_path) {
                                            match result {
                                                Ok(Some(text)) => {
                                                    if let Some(ref mut m) = st.metadata {
                                                        m.lyrics = Some(text.clone());
                                                    }
                                                    st.lyrics_state = LyricsState::Found(text);
                                                }
                                                Ok(None) => {
                                                    st.lyrics_state = LyricsState::NotFound;
                                                }
                                                Err(e) => {
                                                    st.lyrics_state = LyricsState::Error(e);
                                                }
                                            }
                                        }
                                    } else {
                                        let mut st = bg_state.lock().unwrap();
                                        if st.current_track.as_deref() == Some(&bg_path) {
                                            st.lyrics_state = LyricsState::NotFound;
                                        }
                                    }
                                }
                            });
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
                            st.elapsed_millis = 0;
                            st.duration_secs = 0;
                            st.metadata = None;
                            st.lyrics_state = LyricsState::NotFound;
                            st.visualizer_data.fill(0.0);
                            st.visualizer_peaks.fill(0.0);
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
                            if let Some(ref s) = sink
                                && s.try_seek(pos).is_ok() {
                                    elapsed_millis = pos.as_millis();
                                    let mut st = state_clone.lock().unwrap();
                                    st.elapsed_secs = pos.as_secs();
                                    st.elapsed_millis = pos.as_millis().min(u64::MAX as u128) as u64;
                                    last_tick = Instant::now();
                                }
                        }
                    }
                }

                // Compute delta before decay so it can be used for time-normalized decay
                let now = Instant::now();
                let delta = now.duration_since(last_tick);
                last_tick = now;
                // How many 60fps frames elapsed — used to normalize decay to frame rate
                let dt_frames = (delta.as_secs_f32() * 60.0).clamp(0.5, 4.0);

                // Jalamos todos los frames del visualizador que hayan llegado en este tick
                let mut latest_frame = None;
                let mut last_left_peak = 0.0f32;
                let mut last_right_peak = 0.0f32;
                while let Ok(frame) = visualizer_rx.try_recv() {
                    last_left_peak = last_left_peak.max(frame.left_peak);
                    last_right_peak = last_right_peak.max(frame.right_peak);
                    latest_frame = Some(frame);
                }

                if let Some(frame) = latest_frame {
                    sliding_buffer.copy_from_slice(&frame.samples);

                    // Ventana de Hanning pa reducir el spectral leakage antes del FFT
                    let mut fft_input = [Complex::new(0.0, 0.0); 512];
                    for i in 0..512 {
                        let multiplier = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / 511.0).cos());
                        fft_input[i] = Complex::new(sliding_buffer[i] * multiplier, 0.0);
                    }
                    fft(&mut fft_input);

                    let mut magnitudes = [0.0; 256];
                    for i in 0..256 {
                        let c = fft_input[i];
                        magnitudes[i] = (c.re * c.re + c.im * c.im).sqrt();
                    }

                    // Agrupamos en 160 bandas con escala logarítmica (exp 1.8)
                    // pa que los graves no se coman todo el espacio visual
                    let num_bars = 160;
                    let mut new_bars = [0.0; 160];
                    for (i, bar) in new_bars.iter_mut().enumerate() {
                        let start = (256.0 * (i as f32 / num_bars as f32).powf(1.8)) as usize;
                        let end = (256.0 * ((i + 1) as f32 / num_bars as f32).powf(1.8)) as usize;
                        let end = end.min(256).max(start + 1);

                        let sum: f32 = magnitudes[start..end].iter().sum();
                        let avg = sum / (end - start) as f32;

                        let boost = 1.0 + (i as f32 / num_bars as f32) * 2.5;
                        let raw_lin = avg * boost;
                        // Log-scale normalization so typical music fills 40-85% height
                        // log10 range [-1, 2.5] mapped to [0, 1]
                        let log_val = raw_lin.max(1e-10_f32).log10();
                        *bar = ((log_val + 1.0) / 3.5).clamp(0.0, 1.0);
                    }

                    // Aplicamos el filtro de decay: cada barra baja gradualmente
                    // en lugar de caerse de golpe cuando no hay señal
                    let mut st = state_clone.lock().unwrap();
                    if st.visualizer_data.len() != num_bars {
                        st.visualizer_data = vec![0.0; num_bars];
                        st.visualizer_peaks = vec![0.0; num_bars];
                    }
                    // Time-normalized decay: config value is "per 60fps frame"; scale by
                    // actual elapsed frames so behavior is independent of thread wake rate.
                    let frame_decay = st.visualizer_decay.powf(dt_frames);
                    let peak_decay = 0.94_f32.powf(dt_frames);
                    let AudioSharedState { ref mut visualizer_data, ref mut visualizer_peaks, ref mut left_level, ref mut right_level, .. } = *st;
                    for ((vd, pk), &nb) in visualizer_data.iter_mut()
                        .zip(visualizer_peaks.iter_mut())
                        .zip(new_bars.iter())
                    {
                        *vd = (*vd * frame_decay).max(nb);
                        if nb > *pk {
                            *pk = nb;
                        } else {
                            *pk = (*pk * peak_decay).max(0.0);
                        }
                    }
                    *left_level = (*left_level * frame_decay).max(last_left_peak);
                    *right_level = (*right_level * frame_decay).max(last_right_peak);
                } else {
                    let mut st = state_clone.lock().unwrap();
                    if st.status != PlaybackStatus::Playing {
                        st.visualizer_data.fill(0.0);
                        st.visualizer_peaks.fill(0.0);
                        st.left_level = 0.0;
                        st.right_level = 0.0;
                    } else {
                        let frame_decay = st.visualizer_decay.powf(dt_frames);
                        let peak_decay = 0.94_f32.powf(dt_frames);
                        let AudioSharedState { ref mut visualizer_data, ref mut visualizer_peaks, ref mut left_level, ref mut right_level, .. } = *st;
                        for (val, pk) in visualizer_data.iter_mut()
                            .zip(visualizer_peaks.iter_mut())
                        {
                            *val = (*val * frame_decay).max(0.0);
                            *pk = (*pk * peak_decay).max(0.0);
                        }
                        *left_level = (*left_level * frame_decay).max(0.0);
                        *right_level = (*right_level * frame_decay).max(0.0);
                    }
                }

                // Sacamos el send_finished del bloque del mutex pa no mandarlo
                // con el lock tomado — el send puede bloquearse un momento
                let send_finished = {
                    let mut st = state_clone.lock().unwrap();
                    if st.status == PlaybackStatus::Playing {
                        elapsed_millis += delta.as_millis();
                        st.elapsed_secs = (elapsed_millis / 1000) as u64;
                        st.elapsed_millis = elapsed_millis.min(u64::MAX as u128) as u64;

                        let mut empty = true;
                        if let Some(ref s) = sink {
                            empty = s.empty();
                        }

                        if empty {
                            st.status = PlaybackStatus::Stopped;
                            st.elapsed_secs = 0;
                            st.elapsed_millis = 0;
                            st.current_track = None;
                            st.metadata = None;
                            st.visualizer_data.fill(0.0);
                            st.visualizer_peaks.fill(0.0);
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

                // 16ms = ~60 FPS audio state updates
                thread::sleep(Duration::from_millis(16));
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

// Wrapper sobre un Source de rodio que intercepta cada sample pa mandarlo
// al canal del visualizador sin interrumpir la reproducción normal
pub struct VisualizerSource<I>
where
    I: Source<Item = f32>,
{
    input: I,
    sender: SyncSender<VisualizerFrame>,
    frame: VisualizerFrame,
    frame_index: usize,
    channel_index: u16,
    sample_sum: f32,
    channels: u16,
}

impl<I> VisualizerSource<I>
where
    I: Source<Item = f32>,
{
    pub fn new(input: I, sender: SyncSender<VisualizerFrame>) -> Self {
        let channels = input.channels().max(1);
        Self {
            input,
            sender,
            frame: VisualizerFrame { samples: [0.0; 512], left_peak: 0.0, right_peak: 0.0 },
            frame_index: 0,
            channel_index: 0,
            sample_sum: 0.0,
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
            self.sample_sum += sample;
            if self.channel_index == 0 {
                self.frame.left_peak = self.frame.left_peak.max(sample.abs());
            }
            if self.channel_index == 1 || self.channels == 1 {
                self.frame.right_peak = self.frame.right_peak.max(sample.abs());
            }
            self.channel_index += 1;
            if self.channel_index == self.channels {
                self.frame.samples[self.frame_index] = self.sample_sum / self.channels as f32;
                self.sample_sum = 0.0;
                self.channel_index = 0;
                self.frame_index += 1;
                if self.frame_index == 512 {
                    // Never wait on visualization from the device callback; drop frames if busy.
                    let _ = self.sender.try_send(self.frame.clone());
                    self.frame_index = 0;
                    self.frame.left_peak = 0.0;
                    self.frame.right_peak = 0.0;
                }
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
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let res = self.input.try_seek(pos);
        // Al hacer seek limpiamos el buffer pa no mandar samples viejos al visualizador
        if res.is_ok() {
            self.frame_index = 0;
            self.channel_index = 0;
            self.sample_sum = 0.0;
            self.frame.left_peak = 0.0;
            self.frame.right_peak = 0.0;
        }
        res
    }
}

// Número complejo mínimo pa el FFT — solo lo que necesitamos, sin deps externas
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

// FFT Cooley-Tukey Radix-2 in-place — el input tiene que ser potencia de 2
fn fft(input: &mut [Complex]) {
    let n = input.len();
    if n <= 1 {
        return;
    }

    // Iterative radix-2 transform: no recursive heap allocations per visualizer frame.
    debug_assert!(n.is_power_of_two());
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j ^= bit;
        if i < j { input.swap(i, j); }
    }
    let mut width = 2;
    while width <= n {
        let angle = -2.0 * std::f32::consts::PI / width as f32;
        let step = Complex::new(angle.cos(), angle.sin());
        for block in input.chunks_exact_mut(width) {
            let mut rotation = Complex::new(1.0, 0.0);
            for k in 0..width / 2 {
                let even = block[k];
                let odd = block[k + width / 2].mul(rotation);
                block[k] = even.add(odd);
                block[k + width / 2] = even.sub(odd);
                rotation = rotation.mul(step);
            }
        }
        width *= 2;
    }
}

// Busca letra en disco junto al archivo de audio — primero .lrc, luego .txt
fn load_lyrics(path: &std::path::Path) -> Option<String> {
    for extension in ["lrc", "LRC", "txt", "TXT"] {
        let lyrics_path = path.with_extension(extension);
        if let Ok(content) = std::fs::read_to_string(&lyrics_path)
            && let Some(content) = non_empty_lyrics(&content)
        {
            return Some(content);
        }
    }
    None
}

fn clean_filename_title(filename: &str) -> String {
    let trimmed = filename.trim();
    let without_track_number = trimmed
        .find(|c: char| !c.is_ascii_digit() && !matches!(c, ' ' | '-' | '_' | '.'))
        .filter(|&index| index > 0 && index <= 5)
        .map(|index| &trimmed[index..])
        .unwrap_or(trimmed);
    without_track_number
        .trim_start_matches([' ', '-', '_', '.'])
        .trim()
        .to_string()
}

fn non_empty_lyrics(lyrics: &str) -> Option<String> {
    let lyrics = lyrics.trim_start_matches('\u{feff}').trim();
    (!lyrics.is_empty()).then(|| lyrics.to_string())
}

fn lyrics_from_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("syncedLyrics")
        .and_then(|value| value.as_str())
        .and_then(non_empty_lyrics)
        .or_else(|| {
            value
                .get("plainLyrics")
                .and_then(|value| value.as_str())
                .and_then(non_empty_lyrics)
        })
}

fn normalized_track_name(value: &str) -> String {
    let base = value
        .split(['(', '['])
        .next()
        .unwrap_or(value)
        .to_lowercase();
    base.chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn best_search_result(
    results: &serde_json::Value,
    title: &str,
    artist: Option<&str>,
    duration: Option<u64>,
) -> Option<String> {
    let expected_title = normalized_track_name(title);
    let expected_artist = artist.map(normalized_track_name);
    if expected_title.is_empty() {
        return None;
    }

    results
        .as_array()?
        .iter()
        .filter_map(|result| {
            let lyrics = lyrics_from_value(result)?;
            let candidate_title = result
                .get("trackName")
                .and_then(|value| value.as_str())
                .map(normalized_track_name)?;

            let mut score = if candidate_title == expected_title {
                100
            } else if candidate_title.contains(&expected_title)
                || expected_title.contains(&candidate_title)
            {
                40
            } else {
                return None;
            };

            if let Some(ref expected_artist) = expected_artist {
                let candidate_artist = result
                    .get("artistName")
                    .and_then(|value| value.as_str())
                    .map(normalized_track_name)
                    .unwrap_or_default();
                if candidate_artist == *expected_artist {
                    score += 50;
                } else if !candidate_artist.is_empty()
                    && (candidate_artist.contains(expected_artist)
                        || expected_artist.contains(&candidate_artist))
                {
                    score += 20;
                } else {
                    score -= 30;
                }
            }

            if let (Some(expected), Some(candidate)) = (
                duration,
                result.get("duration").and_then(|value| value.as_f64()),
            ) {
                let difference = (candidate - expected as f64).abs();
                score += if difference <= 3.0 {
                    30
                } else if difference <= 10.0 {
                    10
                } else if difference > 30.0 {
                    -15
                } else {
                    0
                };
            }

            Some((score, lyrics))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, lyrics)| lyrics)
}

fn friendly_request_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(429, _) => {
            "The lyrics service is busy right now. Please try again in a moment.".to_string()
        }
        ureq::Error::Status(code, _) if code >= 500 => {
            "The lyrics service is temporarily unavailable. Please try again later.".to_string()
        }
        ureq::Error::Status(_, _) => {
            "The lyrics service couldn't complete this request.".to_string()
        }
        ureq::Error::Transport(transport) => match transport.kind() {
            ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed => {
                "You're offline, so online lyrics couldn't be loaded.".to_string()
            }
            ureq::ErrorKind::Io => {
                "The lyrics lookup timed out. Please try again in a moment.".to_string()
            }
            _ => "Online lyrics couldn't be reached right now.".to_string(),
        },
    }
}

fn get_json(agent: &ureq::Agent, url: &str) -> Result<Option<serde_json::Value>, String> {
    let response = match agent
        .get(url)
        .set(
            "User-Agent",
            concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
        )
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(error) => return Err(friendly_request_error(error)),
    };

    response.into_json().map(Some).map_err(|_| {
        "The lyrics service sent an unexpected response. Please try again later.".to_string()
    })
}

// Consulta primero el endpoint exacto de LRCLIB y, si no encuentra la rola,
// usa búsqueda para tolerar álbumes, duraciones o sufijos de título distintos.
fn fetch_lyrics_online(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration: Option<u64>,
) -> Result<Option<String>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(15))
        .build();

    let mut url = format!(
        "https://lrclib.net/api/get?track_name={}",
        urlencoding::encode(title)
    );
    if let Some(a) = artist {
        url.push_str(&format!("&artist_name={}", urlencoding::encode(a)));
    }
    if let Some(al) = album {
        url.push_str(&format!("&album_name={}", urlencoding::encode(al)));
    }
    if let Some(d) = duration {
        url.push_str(&format!("&duration={}", d));
    }

    // LRCLIB requires artist_name for exact lookups. Files without an artist tag
    // can still use the more forgiving search endpoint below.
    if artist.is_some()
        && let Some(exact_match) = get_json(&agent, &url)?
        && let Some(lyrics) = lyrics_from_value(&exact_match)
    {
        return Ok(Some(lyrics));
    }

    let mut search_url = format!(
        "https://lrclib.net/api/search?track_name={}",
        urlencoding::encode(title)
    );
    if let Some(artist) = artist {
        search_url.push_str(&format!(
            "&artist_name={}",
            urlencoding::encode(artist)
        ));
    }

    let Some(search_results) = get_json(&agent, &search_url)? else {
        return Ok(None);
    };
    Ok(best_search_result(&search_results, title, artist, duration))
}

#[cfg(test)]
mod lyrics_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cleans_track_number_from_filename_title() {
        assert_eq!(clean_filename_title("01 - A Great Song"), "A Great Song");
        assert_eq!(clean_filename_title("12_song title"), "song title");
        assert_eq!(clean_filename_title("1985"), "1985");
    }

    #[test]
    fn ignores_empty_local_or_remote_lyrics() {
        assert_eq!(non_empty_lyrics(" \n\t"), None);
        assert_eq!(non_empty_lyrics("\u{feff}Words\n"), Some("Words".to_string()));
        assert_eq!(lyrics_from_value(&json!({ "plainLyrics": "" })), None);
    }

    #[test]
    fn search_fallback_prefers_matching_artist_and_duration() {
        let results = json!([
            {
                "trackName": "Home",
                "artistName": "Someone Else",
                "duration": 180.0,
                "plainLyrics": "wrong"
            },
            {
                "trackName": "Home (Remastered)",
                "artistName": "The Band",
                "duration": 201.5,
                "plainLyrics": "right"
            }
        ]);

        assert_eq!(
            best_search_result(&results, "Home", Some("The Band"), Some(202)),
            Some("right".to_string())
        );
    }
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;

    #[test]
    fn full_visualizer_queue_preserves_every_audio_sample() {
        let samples: Vec<f32> = (0..8192).map(|i| (i as f32 * 0.01).sin()).collect();
        let (tx, rx) = sync_channel(2);
        let source = SamplesBuffer::new(2, 44100, samples.clone());
        let output: Vec<_> = VisualizerSource::new(source, tx).collect();
        assert_eq!(output, samples);
        assert_eq!(rx.try_iter().count(), 2);
    }

    #[test]
    fn visualizer_mixes_channels_and_measures_peaks() {
        let samples = [0.5f32, -0.25].repeat(512);
        let (tx, rx) = sync_channel(2);
        let source = SamplesBuffer::new(2, 44100, samples);
        let _ = VisualizerSource::new(source, tx).count();
        let frame = rx.try_recv().unwrap();
        assert_eq!(frame.samples, [0.125; 512]);
        assert_eq!(frame.left_peak, 0.5);
        assert_eq!(frame.right_peak, 0.25);
    }

    #[test]
    fn fft_matches_direct_transform() {
        let input: Vec<_> = (0..64).map(|i| Complex::new((i as f32 * 0.7).sin(), 0.0)).collect();
        let mut actual = input.clone();
        fft(&mut actual);
        for (k, result) in actual.iter().enumerate() {
            let mut expected = Complex::new(0.0, 0.0);
            for (i, sample) in input.iter().enumerate() {
                let angle = -2.0 * std::f32::consts::PI * k as f32 * i as f32 / input.len() as f32;
                expected = expected.add(sample.mul(Complex::new(angle.cos(), angle.sin())));
            }
            assert!((result.re - expected.re).abs() < 0.001);
            assert!((result.im - expected.im).abs() < 0.001);
        }
    }
}
