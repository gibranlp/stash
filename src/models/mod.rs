use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AudioMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<u64>,
    pub track: Option<u32>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub codec: Option<String>,
    pub lyrics: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct FileItem {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
    pub modified: Option<SystemTime>,
    pub is_selected: bool,
    pub metadata: Option<AudioMetadata>,
    // depth e is_expanded sirven para manejar árboles de directorios anidados en el browser
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub is_expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

// LyricsState maneja el ciclo de vida de las letras: primero carga local, luego jala de red si no hay
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LyricsState {
    Loading,
    Fetching,
    Found(String),
    NotFound,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum VisualizerMode {
    #[default]
    Spectrum,
    Waveform,
    SignalLevels,
}
