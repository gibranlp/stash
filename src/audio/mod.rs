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
    pub samples: Vec<f32>,
    pub left_peak: f32,
    pub right_peak: f32,
}

pub struct AudioSharedState {
    pub current_track: Option<PathBuf>,
    pub status: PlaybackStatus,
    pub elapsed_secs: u64,
    pub duration_secs: u64,
    pub volume: u32,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    pub metadata: Option<AudioMetadata>,
    pub lyrics_state: LyricsState,
    pub device_error: Option<String>,
    pub visualizer_data: Vec<f32>,
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
            duration_secs: 0,
            volume: default_volume,
            repeat: default_repeat,
            shuffle: default_shuffle,
            metadata: None,
            lyrics_state: LyricsState::NotFound,
            device_error: None,
            visualizer_data: vec![0.0; 160],
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
            let (visualizer_tx, visualizer_rx) = channel::<VisualizerFrame>();
            let mut sliding_buffer = vec![0.0; 512];

            loop {
                while let Ok(cmd) = command_rx.try_recv() {
                    match cmd {
                        AudioCommand::Play(path) => {
                            if let Some(ref s) = sink {
                                s.stop();
                            }
                            elapsed_millis = 0;

                            // Creamos un sink nuevo pa limpiar los buffers anteriores
                            // sin esto quedan restos del track previo
                            if let Some(ref handle) = stream_handle
                                && let Ok(s) = Sink::try_new(handle) {
                                    sink = Some(s);
                                }

                            if let Ok(file) = File::open(&path) {
                                let reader = BufReader::new(file);
                                if let Ok(source) = Decoder::new(reader) {
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
                                        st.current_track = Some(path.clone());
                                        st.status = PlaybackStatus::Playing;
                                        st.elapsed_secs = 0;
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
                                                        lyrics = Some(embedded.to_string());
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
                                            bg_path.file_name().map(|f| f.to_string_lossy().into_owned())
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
                            st.lyrics_state = LyricsState::NotFound;
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
                            if let Some(ref s) = sink
                                && s.try_seek(pos).is_ok() {
                                    elapsed_millis = pos.as_millis();
                                    let mut st = state_clone.lock().unwrap();
                                    st.elapsed_secs = pos.as_secs();
                                    last_tick = Instant::now();
                                }
                        }
                    }
                }

                // Jalamos todos los frames del visualizador que hayan llegado en este tick
                let mut new_samples = Vec::new();
                let mut last_left_peak = 0.0f32;
                let mut last_right_peak = 0.0f32;
                while let Ok(frame) = visualizer_rx.try_recv() {
                    let mut samples = frame.samples;
                    new_samples.append(&mut samples);
                    last_left_peak = last_left_peak.max(frame.left_peak);
                    last_right_peak = last_right_peak.max(frame.right_peak);
                }

                if !new_samples.is_empty() {
                    let take_len = new_samples.len().min(512);
                    let start_idx = new_samples.len() - take_len;
                    let latest_chunk = &new_samples[start_idx..];

                    // Buffer deslizante: tiramos los más viejos y metemos los nuevos
                    let shift = latest_chunk.len();
                    if shift >= 512 {
                        sliding_buffer = latest_chunk.to_vec();
                    } else {
                        sliding_buffer.drain(0..shift);
                        sliding_buffer.extend_from_slice(latest_chunk);
                    }

                    // Ventana de Hanning pa reducir el spectral leakage antes del FFT
                    let mut fft_input = vec![Complex::new(0.0, 0.0); 512];
                    for i in 0..512 {
                        let multiplier = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / 511.0).cos());
                        fft_input[i] = Complex::new(sliding_buffer[i] * multiplier, 0.0);
                    }
                    fft(&mut fft_input);

                    let mut magnitudes = vec![0.0; 256];
                    for i in 0..256 {
                        let c = fft_input[i];
                        magnitudes[i] = (c.re * c.re + c.im * c.im).sqrt();
                    }

                    // Agrupamos en 160 bandas con escala logarítmica (exp 1.8)
                    // pa que los graves no se coman todo el espacio visual
                    let num_bars = 160;
                    let mut new_bars = vec![0.0; num_bars];
                    for (i, bar) in new_bars.iter_mut().enumerate() {
                        let start = (256.0 * (i as f32 / num_bars as f32).powf(1.8)) as usize;
                        let end = (256.0 * ((i + 1) as f32 / num_bars as f32).powf(1.8)) as usize;
                        let end = end.min(256).max(start + 1);

                        let sum: f32 = magnitudes[start..end].iter().sum();
                        let avg = sum / (end - start) as f32;

                        let boost = 1.0 + (i as f32 / num_bars as f32) * 2.5;
                        *bar = avg * boost * 0.7;
                    }

                    // Aplicamos el filtro de decay: cada barra baja gradualmente
                    // en lugar de caerse de golpe cuando no hay señal
                    let mut st = state_clone.lock().unwrap();
                    if st.visualizer_data.len() != num_bars {
                        st.visualizer_data = vec![0.0; num_bars];
                    }
                    let decay = st.visualizer_decay;
                    for (vd, &nb) in st.visualizer_data.iter_mut().zip(new_bars.iter()) {
                        *vd = (*vd * decay).max(nb);
                    }
                    st.left_level = (st.left_level * decay).max(last_left_peak);
                    st.right_level = (st.right_level * decay).max(last_right_peak);
                } else {
                    let mut st = state_clone.lock().unwrap();
                    if st.status != PlaybackStatus::Playing {
                        st.visualizer_data.fill(0.0);
                        st.left_level = 0.0;
                        st.right_level = 0.0;
                    } else {
                        // Está reproduciendo pero no llegaron samples nuevos —
                        // decaemos suavito pa que no se vea que se congeló
                        let decay = (st.visualizer_decay + 0.15).min(0.95);
                        for val in &mut st.visualizer_data {
                            *val = (*val * decay).max(0.0);
                        }
                        st.left_level = (st.left_level * decay).max(0.0);
                        st.right_level = (st.right_level * decay).max(0.0);
                    }
                }

                let now = Instant::now();
                let delta = now.duration_since(last_tick);
                last_tick = now;

                // Sacamos el send_finished del bloque del mutex pa no mandarlo
                // con el lock tomado — el send puede bloquearse un momento
                let send_finished = {
                    let mut st = state_clone.lock().unwrap();
                    if st.status == PlaybackStatus::Playing {
                        elapsed_millis += delta.as_millis();
                        st.elapsed_secs = (elapsed_millis / 1000) as u64;

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

                // 30ms = ~33 FPS, suficiente pa que el visualizador se vea fluido
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

// Wrapper sobre un Source de rodio que intercepta cada sample pa mandarlo
// al canal del visualizador sin interrumpir la reproducción normal
pub struct VisualizerSource<I>
where
    I: Source<Item = f32>,
{
    input: I,
    sender: Sender<VisualizerFrame>,
    buffer: Vec<f32>,
    channels: u16,
}

impl<I> VisualizerSource<I>
where
    I: Source<Item = f32>,
{
    pub fn new(input: I, sender: Sender<VisualizerFrame>) -> Self {
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

                let mut left_peak = 0.0f32;
                let mut right_peak = 0.0f32;

                // Si es stereo o más canales, mezclamos a mono y medimos picos por lado
                // Si es mono, el pico derecho es igual al izquierdo
                if self.channels >= 2 {
                    for chunk_slice in chunk.chunks_exact(step) {
                        left_peak = left_peak.max(chunk_slice[0].abs());
                        right_peak = right_peak.max(chunk_slice[1].abs());
                        let sum: f32 = chunk_slice.iter().sum();
                        mono.push(sum / self.channels as f32);
                    }
                } else {
                    for &s in &chunk {
                        left_peak = left_peak.max(s.abs());
                        mono.push(s);
                    }
                    right_peak = left_peak;
                }

                let _ = self.sender.send(VisualizerFrame {
                    samples: mono,
                    left_peak,
                    right_peak,
                });
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
            self.buffer.clear();
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

// Busca letra en disco junto al archivo de audio — primero .lrc, luego .txt
fn load_lyrics(path: &std::path::Path) -> Option<String> {
    let lrc_path = path.with_extension("lrc");
    if lrc_path.exists()
        && let Ok(content) = std::fs::read_to_string(&lrc_path) {
            return Some(content);
        }
    let txt_path = path.with_extension("txt");
    if txt_path.exists()
        && let Ok(content) = std::fs::read_to_string(&txt_path) {
            return Some(content);
        }
    None
}

// Consulta lrclib.net pa traer letra; regresa Ok(None) si la rola no está en la base,
// Err si hubo bronca de red o parseo
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

    let resp = agent.get(&url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        ureq::Error::Transport(t) => match t.kind() {
            ureq::ErrorKind::Dns => "DNS lookup failed (offline?)".to_string(),
            ureq::ErrorKind::ConnectionFailed => "Connection refused".to_string(),
            ureq::ErrorKind::Io => "Network I/O error (timed out?)".to_string(),
            _ => "Network error".to_string(),
        },
    })?;

    if resp.status() == 404 {
        return Ok(None);
    }

    let json: serde_json::Value = resp.into_json().map_err(|e| format!("Bad response: {e}"))?;

    // Preferimos la letra plana; si no hay, jalamos la sincronizada
    if let Some(plain) = json.get("plainLyrics").and_then(|v| v.as_str()) {
        if !plain.is_empty() {
            return Ok(Some(plain.to_string()));
        }
    }
    if let Some(synced) = json.get("syncedLyrics").and_then(|v| v.as_str()) {
        if !synced.is_empty() {
            return Ok(Some(synced.to_string()));
        }
    }
    Ok(None)
}
