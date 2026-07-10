use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrackStats {
    pub play_count: u32,
    pub skip_count: u32,
    pub total_listen_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub start_ts: u64,
    pub duration_secs: u64,
    pub tracks_played: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListenStats {
    pub tracks: HashMap<PathBuf, TrackStats>,
    pub sessions: Vec<Session>,
}

impl ListenStats {
    pub fn load() -> Self {
        if let Some(path) = Self::stats_file_path()
            && path.exists()
                && let Ok(content) = fs::read_to_string(&path)
                    && let Ok(stats) = serde_json::from_str::<ListenStats>(&content) {
                        return stats;
                    }
        Self::default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(path) = Self::stats_file_path() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let content = serde_json::to_string_pretty(self)?;
            fs::write(path, content)?;
        }
        Ok(())
    }

    fn stats_file_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("stash").join("stats.json"))
    }

    pub fn record_play(&mut self, path: &PathBuf, listen_secs: u64) {
        let entry = self.tracks.entry(path.clone()).or_default();
        entry.play_count += 1;
        entry.total_listen_secs += listen_secs;
    }

    pub fn record_skip(&mut self, path: &PathBuf, listen_secs: u64) {
        let entry = self.tracks.entry(path.clone()).or_default();
        entry.skip_count += 1;
        entry.total_listen_secs += listen_secs;
    }

    pub fn record_session(&mut self, duration_secs: u64, tracks_played: u32, start_ts: u64) {
        if duration_secs > 0 || tracks_played > 0 {
            self.sessions.push(Session { start_ts, duration_secs, tracks_played });
        }
    }

    pub fn total_listen_secs(&self) -> u64 {
        self.tracks.values().map(|s| s.total_listen_secs).sum()
    }

    pub fn total_plays(&self) -> u32 {
        self.tracks.values().map(|s| s.play_count).sum()
    }

    pub fn total_skips(&self) -> u32 {
        self.tracks.values().map(|s| s.skip_count).sum()
    }

    pub fn avg_session_secs(&self) -> u64 {
        if self.sessions.is_empty() {
            return 0;
        }
        let total: u64 = self.sessions.iter().map(|s| s.duration_secs).sum();
        total / self.sessions.len() as u64
    }

    pub fn longest_session_secs(&self) -> u64 {
        self.sessions.iter().map(|s| s.duration_secs).max().unwrap_or(0)
    }

    pub fn most_played_paths(&self, n: usize) -> Vec<(PathBuf, u32)> {
        let mut entries: Vec<_> = self.tracks.iter()
            .filter(|(_, s)| s.play_count > 0)
            .map(|(p, s)| (p.clone(), s.play_count))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.into_iter().take(n).collect()
    }

    pub fn most_skipped_paths(&self, n: usize) -> Vec<(PathBuf, u32)> {
        let mut entries: Vec<_> = self.tracks.iter()
            .filter(|(_, s)| s.skip_count > 0)
            .map(|(p, s)| (p.clone(), s.skip_count))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.into_iter().take(n).collect()
    }
}

pub fn unix_ts_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Default)]
pub struct StatsTracking {
    pub current_track: Option<PathBuf>,
    pub last_elapsed: u64,
    pub last_duration: u64,
    pub natural_finish: bool,
    pub session_start_ts: u64,
    pub session_tracks: u32,
    pub session_listen_secs: u64,
}
