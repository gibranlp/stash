use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::models::{RepeatMode, VisualizerMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub music_folders: Vec<String>,
    pub default_volume: u32,
    pub theme: String,
    pub show_hidden: bool,
    pub auto_resume: bool,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: RepeatMode,
    #[serde(default)]
    pub visualizer_mode: VisualizerMode,
    #[serde(default = "default_visualizer_decay")]
    pub visualizer_decay: f32,
    // ID de la app de Discord para el Rich Presence — si no lo pones, no hay status
    #[serde(default)]
    pub discord_app_id: Option<u64>,
    #[serde(default)]
    pub acoustid_api_key: Option<String>,
}

fn default_visualizer_decay() -> f32 {
    0.88
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            music_folders: vec!["~/Music".to_string()],
            default_volume: 75,
            theme: "default".to_string(),
            show_hidden: false,
            auto_resume: true,
            shuffle: false,
            repeat: RepeatMode::Off,
            visualizer_mode: VisualizerMode::Spectrum,
            visualizer_decay: 0.88,
            discord_app_id: None,
            acoustid_api_key: None,
        }
    }
}

impl AppConfig {
    // Intentamos cargar el config del disco; si algo falla o no existe, jalamos el default y lo guardamos
    pub fn load() -> Self {
        if let Some(path) = Self::config_file_path()
            && path.exists()
                && let Ok(content) = fs::read_to_string(&path)
                    && let Ok(config) = serde_json::from_str::<AppConfig>(&content) {
                        return config;
                    }
        let default_config = Self::default();
        let _ = default_config.save();
        default_config
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(path) = Self::config_file_path() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let content = serde_json::to_string_pretty(self)?;
            fs::write(path, content)?;
        }
        Ok(())
    }

    pub fn config_file_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("stash").join("config.json"))
    }

    pub fn add_music_folder(&mut self, path: &PathBuf) -> bool {
        let canonical = path.to_string_lossy().into_owned();
        let already = self.music_folders.iter().any(|f| resolve_path(f) == *path);
        if !already {
            self.music_folders.push(canonical);
            let _ = self.save();
            true
        } else {
            false
        }
    }

    pub fn remove_music_folder_at(&mut self, index: usize) {
        if index < self.music_folders.len() {
            self.music_folders.remove(index);
            let _ = self.save();
        }
    }
}

// Expande el "~/" al home del usuario — sin esto las rutas del config quedan chuecas
pub fn resolve_path(path_str: &str) -> PathBuf {
    if path_str.starts_with("~/")
        && let Some(home) = dirs::home_dir() {
            return home.join(&path_str[2..]);
        }
    PathBuf::from(path_str)
}
