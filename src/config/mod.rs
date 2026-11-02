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
    #[serde(default = "auto_update_default")]
    pub auto_update: bool,
    // Mount USB sticks and external drives as soon as they are plugged in.
    #[serde(default = "auto_mount_default")]
    pub auto_mount: bool,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: RepeatMode,
    #[serde(default)]
    pub visualizer_mode: VisualizerMode,
    #[serde(default = "default_visualizer_decay")]
    pub visualizer_decay: f32,
    // Optional Discord application ID override; None uses the built-in Stash application.
    #[serde(default)]
    pub discord_app_id: Option<u64>,
    #[serde(default)]
    pub acoustid_api_key: Option<String>,
    // Radio station slugs the user starred.
    #[serde(default)]
    pub radio_favorites: Vec<String>,
}

fn default_visualizer_decay() -> f32 {
    0.88
}

fn auto_update_default() -> bool { true }

fn auto_mount_default() -> bool { true }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            music_folders: vec!["~/Music".to_string()],
            default_volume: 75,
            theme: "default".to_string(),
            show_hidden: false,
            auto_resume: true,
            auto_update: true,
            auto_mount: true,
            shuffle: false,
            repeat: RepeatMode::Off,
            visualizer_mode: VisualizerMode::Spectrum,
            visualizer_decay: 0.88,
            discord_app_id: None,
            acoustid_api_key: None,
            radio_favorites: Vec::new(),
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

#[cfg(test)]
mod update_tests {
    use super::AppConfig;

    #[test]
    fn old_configs_enable_automatic_updates_and_opt_out_is_preserved() {
        let mut old = serde_json::to_value(AppConfig::default()).unwrap();
        old.as_object_mut().unwrap().remove("auto_update");
        assert!(serde_json::from_value::<AppConfig>(old.clone()).unwrap().auto_update);
        old["auto_update"] = false.into();
        assert!(!serde_json::from_value::<AppConfig>(old).unwrap().auto_update);
    }
}
