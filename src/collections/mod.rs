use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Collections {
    pub collections: HashMap<String, Vec<PathBuf>>,
}

impl Collections {
    // Jalamos las colecciones del archivo JSON; si no existe o está chueco, regresamos default
    pub fn load() -> Self {
        if let Some(path) = Self::collections_file_path()
            && path.exists()
                && let Ok(content) = fs::read_to_string(&path)
                    && let Ok(collections) = serde_json::from_str::<HashMap<String, Vec<PathBuf>>>(&content) {
                        return Self { collections };
                    }
        Self::default()
    }

    // Guardamos todo al disco; creamos el directorio si no existe para no tronar
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(path) = Self::collections_file_path() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let content = serde_json::to_string_pretty(&self.collections)?;
            fs::write(path, content)?;
        }
        Ok(())
    }

    pub fn collections_file_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("stash").join("collections.json"))
    }

    // Agrega paths a una colección; si ya están, los ignoramos para no duplicar
    pub fn add_to_collection(&mut self, collection_name: &str, paths: Vec<PathBuf>) {
        let entry = self.collections.entry(collection_name.to_string()).or_default();
        for path in paths {
            if !entry.contains(&path) {
                entry.push(path);
            }
        }
        let _ = self.save();
    }

    pub fn create_collection(&mut self, name: &str) {
        if !name.trim().is_empty() {
            self.collections.entry(name.to_string()).or_default();
            let _ = self.save();
        }
    }

    pub fn delete_collection(&mut self, name: &str) {
        self.collections.remove(name);
        let _ = self.save();
    }

    pub fn remove_from_collection(&mut self, name: &str, path: &PathBuf) {
        if let Some(entry) = self.collections.get_mut(name) {
            entry.retain(|p| p != path);
            let _ = self.save();
        }
    }
}
