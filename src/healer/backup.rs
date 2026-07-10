use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use super::TagSnapshot;

#[derive(Serialize, Deserialize, Default)]
pub struct BackupStore {
    pub entries: HashMap<PathBuf, TagBackup>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TagBackup {
    pub title:        Option<String>,
    pub artist:       Option<String>,
    pub album:        Option<String>,
    pub album_artist: Option<String>,
    pub track:        Option<u32>,
    pub disc:         Option<u32>,
    pub year:         Option<u32>,
    pub genre:        Option<String>,
}

impl BackupStore {
    pub fn load() -> Self {
        backup_path()
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(path) = backup_path() {
            if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
            fs::write(path, serde_json::to_string_pretty(&self.entries)?)?;
        }
        Ok(())
    }

    pub fn record(&mut self, path: &PathBuf, snap: &TagSnapshot) {
        self.entries.entry(path.clone()).or_insert_with(|| TagBackup {
            title:        snap.title.clone(),
            artist:       snap.artist.clone(),
            album:        snap.album.clone(),
            album_artist: snap.album_artist.clone(),
            track:        snap.track,
            disc:         snap.disc,
            year:         snap.year,
            genre:        snap.genre.clone(),
        });
    }

}

fn backup_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("stash").join("healer_backup.json"))
}
