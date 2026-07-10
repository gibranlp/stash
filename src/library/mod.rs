use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::config::{ParseOptions, ParsingMode, WriteOptions};
use serde::{Deserialize, Serialize};

use crate::collections::Collections;
use crate::config::resolve_path;
use crate::search::matches_audio_extension;
use crate::stats::ListenStats;

// ── Library cache ─────────────────────────────────────────────────────────────
// Persists tag data keyed by path. Each entry includes the file's mtime so we
// can skip re-reading tags for files that haven't changed.

#[derive(Serialize, Deserialize)]
struct CachedTrack {
    mtime:        u64,
    title:        Option<String>,
    artist:       Option<String>,
    album:        Option<String>,
    track:        Option<u32>,
    year:         Option<u32>,
    genre:        Option<String>,
    duration_secs: Option<u64>,
}

type CacheMap = HashMap<PathBuf, CachedTrack>;

fn cache_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("stash").join("library_cache.json"))
}

fn load_cache() -> CacheMap {
    cache_path()
        .and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_cache(cache: &CacheMap) {
    if let Some(p) = cache_path() {
        if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); }
        if let Ok(s) = serde_json::to_string(cache) {
            let _ = std::fs::write(p, s);
        }
    }
}

fn file_mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub const SMART_PLAYLISTS: &[&str] = &[
    "Most Played",
    "Top 100",
    "Most Skipped",
    "Favorite Genre",
    "By Artist",
    "By Album",
    "Stats",
];

pub fn is_smart_playlist(name: &str) -> bool {
    SMART_PLAYLISTS.contains(&name)
}

pub const TAG_FIELD_NAMES: [&str; 6] = ["Title", "Artist", "Album", "Track #", "Year", "Genre"];

#[derive(Debug, Clone)]
pub struct LibraryTrack {
    pub path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub duration_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryPanel {
    Playlists,
    Tracks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibrarySort {
    #[default]
    Default,   // scan / filesystem order
    Title,
    Artist,    // artist → album → track number
    Album,     // album → track number
    Year,      // year → album
    Duration,
}

impl LibrarySort {
    pub fn label(self) -> &'static str {
        match self {
            LibrarySort::Default  => "Default",
            LibrarySort::Title    => "Title",
            LibrarySort::Artist   => "Artist",
            LibrarySort::Album    => "Album",
            LibrarySort::Year     => "Year",
            LibrarySort::Duration => "Duration",
        }
    }

    pub fn next(self) -> Self {
        match self {
            LibrarySort::Default  => LibrarySort::Title,
            LibrarySort::Title    => LibrarySort::Artist,
            LibrarySort::Artist   => LibrarySort::Album,
            LibrarySort::Album    => LibrarySort::Year,
            LibrarySort::Year     => LibrarySort::Duration,
            LibrarySort::Duration => LibrarySort::Default,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanState {
    Idle,
    Scanning,
    Done,
}

pub struct TagEditorState {
    pub path: PathBuf,
    pub fields: [String; 6],  // Title, Artist, Album, Track, Year, Genre
    pub active_field: usize,
    pub cursor_pos: usize,
    pub dirty: bool,
    pub save_result: Option<Result<(), String>>,
}

pub struct LibraryState {
    pub tracks: Vec<LibraryTrack>,
    pub focused_panel: LibraryPanel,
    pub playlist_index: usize,
    pub track_index: usize,
    pub tag_editor: Option<TagEditorState>,
    pub scan_state: ScanState,
    pub pending_scan: Option<Arc<Mutex<Option<Vec<LibraryTrack>>>>>,
    pub sort: LibrarySort,
}

impl LibraryState {
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            focused_panel: LibraryPanel::Playlists,
            playlist_index: 0,
            track_index: 0,
            tag_editor: None,
            scan_state: ScanState::Idle,
            pending_scan: None,
            sort: LibrarySort::Default,
        }
    }

    pub fn start_scan(&mut self, music_folders: &[String]) {
        let folders: Vec<PathBuf> = music_folders
            .iter()
            .map(|s| resolve_path(s))
            .filter(|p| p.is_dir())
            .collect();

        if folders.is_empty() {
            self.scan_state = ScanState::Done;
            return;
        }

        self.scan_state = ScanState::Scanning;
        let slot: Arc<Mutex<Option<Vec<LibraryTrack>>>> = Arc::new(Mutex::new(None));
        self.pending_scan = Some(slot.clone());

        thread::spawn(move || {
            let mut cache = load_cache();
            let mut new_cache: CacheMap = HashMap::new();
            let mut found: Vec<LibraryTrack> = Vec::new();

            for folder in &folders {
                for entry in walkdir::WalkDir::new(folder)
                    .follow_links(false)
                    .into_iter()
                    .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .flatten()
                {
                    let path = entry.path();
                    if !path.is_file() || !matches_audio_extension(path) {
                        continue;
                    }
                    let path_buf = path.to_path_buf();
                    let mtime = file_mtime(path);

                    // Use cached entry if the file hasn't changed
                    if let Some(cached) = cache.remove(&path_buf) {
                        if cached.mtime == mtime {
                            found.push(LibraryTrack {
                                path:         path_buf.clone(),
                                title:        cached.title.clone(),
                                artist:       cached.artist.clone(),
                                album:        cached.album.clone(),
                                track:        cached.track,
                                year:         cached.year,
                                genre:        cached.genre.clone(),
                                duration_secs: cached.duration_secs,
                            });
                            new_cache.insert(path_buf, cached);
                            continue;
                        }
                    }

                    // Cache miss or stale — read tags from disk
                    let track = scan_single_track(path_buf.clone());
                    new_cache.insert(path_buf, CachedTrack {
                        mtime,
                        title:        track.title.clone(),
                        artist:       track.artist.clone(),
                        album:        track.album.clone(),
                        track:        track.track,
                        year:         track.year,
                        genre:        track.genre.clone(),
                        duration_secs: track.duration_secs,
                    });
                    found.push(track);
                }
            }

            save_cache(&new_cache);

            found.sort_unstable_by(|a, b| {
                a.artist.as_deref().unwrap_or("").to_lowercase()
                    .cmp(&b.artist.as_deref().unwrap_or("").to_lowercase())
                    .then_with(|| {
                        a.album.as_deref().unwrap_or("").to_lowercase()
                            .cmp(&b.album.as_deref().unwrap_or("").to_lowercase())
                    })
                    .then_with(|| a.track.unwrap_or(u32::MAX).cmp(&b.track.unwrap_or(u32::MAX)))
                    .then_with(|| {
                        a.title.as_deref().unwrap_or("").to_lowercase()
                            .cmp(&b.title.as_deref().unwrap_or("").to_lowercase())
                    })
            });
            *slot.lock().unwrap() = Some(found);
        });
    }

    pub fn poll_scan(&mut self) -> bool {
        let result = self
            .pending_scan
            .as_ref()
            .and_then(|slot| slot.lock().unwrap().take());
        if let Some(tracks) = result {
            self.tracks = tracks;
            self.scan_state = ScanState::Done;
            self.pending_scan = None;
            self.track_index = 0;
            true
        } else {
            false
        }
    }

    // Returns "All Tracks" at index 0, then user playlists, then smart playlists.
    pub fn playlist_names(collections: &Collections) -> Vec<String> {
        let mut names: Vec<String> = collections.collections.keys().cloned().collect();
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        let mut result = vec!["All Tracks".to_string()];
        result.extend(names);
        for smart in SMART_PLAYLISTS {
            result.push(smart.to_string());
        }
        result
    }

    // Returns tracks visible in the tracks pane, filtered by playlist and query.
    // Pass an empty string for query when no filter is active.
    pub fn visible_tracks<'a>(&'a self, collections: &'a Collections, query: &str, stats: &ListenStats) -> Vec<&'a LibraryTrack> {
        let names = Self::playlist_names(collections);
        let playlist_name = names.get(self.playlist_index).cloned();

        let base: Vec<&LibraryTrack> = if self.playlist_index == 0 {
            self.tracks.iter().collect()
        } else if let Some(ref playlist_name) = playlist_name {
            match playlist_name.as_str() {
                "Most Played" | "Top 100" => {
                    let limit = if playlist_name == "Top 100" { 100 } else { usize::MAX };
                    let mut result: Vec<&LibraryTrack> = self.tracks.iter()
                        .filter(|t| stats.tracks.get(&t.path).map(|s| s.play_count > 0).unwrap_or(false))
                        .collect();
                    result.sort_by(|a, b| {
                        let pa = stats.tracks.get(&a.path).map(|s| s.play_count).unwrap_or(0);
                        let pb = stats.tracks.get(&b.path).map(|s| s.play_count).unwrap_or(0);
                        pb.cmp(&pa)
                    });
                    result.truncate(limit);
                    result
                }
                "Most Skipped" => {
                    let mut result: Vec<&LibraryTrack> = self.tracks.iter()
                        .filter(|t| stats.tracks.get(&t.path).map(|s| s.skip_count > 0).unwrap_or(false))
                        .collect();
                    result.sort_by(|a, b| {
                        let sa = stats.tracks.get(&a.path).map(|s| s.skip_count).unwrap_or(0);
                        let sb = stats.tracks.get(&b.path).map(|s| s.skip_count).unwrap_or(0);
                        sb.cmp(&sa)
                    });
                    result
                }
                "Favorite Genre" => {
                    if let Some(fav) = favorite_genre(stats, &self.tracks) {
                        self.tracks.iter()
                            .filter(|t| t.genre.as_deref().map(|g| g.eq_ignore_ascii_case(&fav)).unwrap_or(false))
                            .collect()
                    } else {
                        Vec::new()
                    }
                }
                "By Artist" => {
                    let mut result: Vec<&LibraryTrack> = self.tracks.iter().collect();
                    result.sort_by(|a, b| {
                        a.artist.as_deref().unwrap_or("").to_lowercase()
                            .cmp(&b.artist.as_deref().unwrap_or("").to_lowercase())
                            .then(a.album.as_deref().unwrap_or("").to_lowercase()
                                .cmp(&b.album.as_deref().unwrap_or("").to_lowercase()))
                            .then(a.track.unwrap_or(u32::MAX).cmp(&b.track.unwrap_or(u32::MAX)))
                    });
                    result
                }
                "By Album" => {
                    let mut result: Vec<&LibraryTrack> = self.tracks.iter().collect();
                    result.sort_by(|a, b| {
                        a.album.as_deref().unwrap_or("").to_lowercase()
                            .cmp(&b.album.as_deref().unwrap_or("").to_lowercase())
                            .then(a.track.unwrap_or(u32::MAX).cmp(&b.track.unwrap_or(u32::MAX)))
                    });
                    result
                }
                "Stats" => Vec::new(),
                _ => {
                    if let Some(paths) = collections.collections.get(playlist_name.as_str()) {
                        let path_set: std::collections::HashSet<&PathBuf> = paths.iter().collect();
                        self.tracks.iter().filter(|t| path_set.contains(&t.path)).collect()
                    } else {
                        Vec::new()
                    }
                }
            }
        } else {
            Vec::new()
        };

        let mut result = if query.is_empty() {
            base
        } else {
            let q = query.to_lowercase();
            base.into_iter()
                .filter(|t| {
                    let title = t.title.as_deref().unwrap_or("").to_lowercase();
                    let artist = t.artist.as_deref().unwrap_or("").to_lowercase();
                    let album = t.album.as_deref().unwrap_or("").to_lowercase();
                    let fname = t.path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    title.contains(&q)
                        || artist.contains(&q)
                        || album.contains(&q)
                        || fname.contains(&q)
                })
                .collect()
        };

        // Apply user sort; skip for smart playlists that own their sort order
        let keep_smart_sort = matches!(
            playlist_name.as_deref().unwrap_or(""),
            "Most Played" | "Top 100" | "Most Skipped"
        );
        let _ = &playlist_name; // used above
        if !keep_smart_sort && self.sort != LibrarySort::Default {
            result.sort_by(|a, b| {
                let str_cmp = |x: Option<&str>, y: Option<&str>| {
                    x.unwrap_or("").to_lowercase().cmp(&y.unwrap_or("").to_lowercase())
                };
                match self.sort {
                    LibrarySort::Title => str_cmp(a.title.as_deref(), b.title.as_deref()),
                    LibrarySort::Artist => str_cmp(a.artist.as_deref(), b.artist.as_deref())
                        .then(str_cmp(a.album.as_deref(), b.album.as_deref()))
                        .then(a.track.unwrap_or(u32::MAX).cmp(&b.track.unwrap_or(u32::MAX))),
                    LibrarySort::Album => str_cmp(a.album.as_deref(), b.album.as_deref())
                        .then(a.track.unwrap_or(u32::MAX).cmp(&b.track.unwrap_or(u32::MAX))),
                    LibrarySort::Year => a.year.unwrap_or(0).cmp(&b.year.unwrap_or(0))
                        .then(str_cmp(a.album.as_deref(), b.album.as_deref()))
                        .then(a.track.unwrap_or(u32::MAX).cmp(&b.track.unwrap_or(u32::MAX))),
                    LibrarySort::Duration => a.duration_secs.unwrap_or(0).cmp(&b.duration_secs.unwrap_or(0)),
                    LibrarySort::Default => std::cmp::Ordering::Equal,
                }
            });
        }

        result
    }

    pub fn clamp_track_index(&mut self, collections: &Collections, query: &str, stats: &ListenStats) {
        let len = self.visible_tracks(collections, query, stats).len();
        if len == 0 {
            self.track_index = 0;
        } else if self.track_index >= len {
            self.track_index = len - 1;
        }
    }

    pub fn clamp_playlist_index(&mut self, collections: &Collections) {
        let len = Self::playlist_names(collections).len();
        if len == 0 {
            self.playlist_index = 0;
        } else if self.playlist_index >= len {
            self.playlist_index = len - 1;
        }
    }
}

pub fn favorite_genre(stats: &ListenStats, tracks: &[LibraryTrack]) -> Option<String> {
    let mut genre_secs: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for track in tracks {
        if let Some(genre) = &track.genre {
            let secs = stats.tracks.get(&track.path).map(|s| s.total_listen_secs).unwrap_or(0);
            if secs > 0 {
                *genre_secs.entry(genre.clone()).or_default() += secs;
            }
        }
    }
    genre_secs.into_iter().max_by_key(|(_, v)| *v).map(|(g, _)| g)
}

fn scan_single_track(path: PathBuf) -> LibraryTrack {
    let mut track = LibraryTrack {
        path: path.clone(),
        title: None,
        artist: None,
        album: None,
        track: None,
        year: None,
        genre: None,
        duration_secs: None,
    };

    if let Ok(tagged_file) = Probe::open(&path).and_then(|p| p.read()) {
        if let Some(tag) = tagged_file.primary_tag().or(tagged_file.first_tag()) {
            track.title = tag.title().map(|s| s.to_string());
            track.artist = tag.artist().map(|s| s.to_string());
            track.album = tag.album().map(|s| s.to_string());
            track.genre = tag.genre().map(|s| s.to_string());
            track.track = tag.track();
            track.year = tag.year();
        }
        let dur = tagged_file.properties().duration().as_secs();
        if dur > 0 {
            track.duration_secs = Some(dur);
        }
    }

    track
}

pub fn write_tags(editor: &mut TagEditorState) {
    let result = (|| -> anyhow::Result<()> {
        let parse_opts = ParseOptions::new().parsing_mode(ParsingMode::Relaxed);
        let mut tagged_file = Probe::open(&editor.path)
            .map_err(|e| anyhow::anyhow!("Cannot open: {}", e))?
            .options(parse_opts)
            .read()
            .map_err(|e| anyhow::anyhow!("Cannot read: {}", e))?;

        // If no tag exists, insert one appropriate for the file type.
        let has_tag = tagged_file.primary_tag().is_some() || tagged_file.first_tag().is_some();
        if !has_tag {
            let tag_type = tagged_file.file_type().primary_tag_type();
            tagged_file.insert_tag(lofty::tag::Tag::new(tag_type));
        }

        let modified = 'edit: {
            if let Some(tag) = tagged_file.primary_tag_mut() {
                apply_tag_fields(tag, editor);
                break 'edit true;
            }
            if let Some(tag) = tagged_file.first_tag_mut() {
                apply_tag_fields(tag, editor);
                break 'edit true;
            }
            false
        };

        if !modified {
            return Err(anyhow::anyhow!("No writable tag found in file"));
        }

        tagged_file
            .save_to_path(&editor.path, WriteOptions::default())
            .map_err(|e| anyhow::anyhow!("Cannot save: {}", e))?;

        Ok(())
    })();

    editor.save_result = Some(result.map_err(|e| e.to_string()));
    editor.dirty = false;
}

fn apply_tag_fields(tag: &mut lofty::tag::Tag, editor: &TagEditorState) {
    let title = &editor.fields[0];
    let artist = &editor.fields[1];
    let album = &editor.fields[2];
    let track_str = &editor.fields[3];
    let year_str = &editor.fields[4];
    let genre = &editor.fields[5];

    if title.is_empty() {
        tag.remove_title();
    } else {
        tag.set_title(title.clone());
    }
    if artist.is_empty() {
        tag.remove_artist();
    } else {
        tag.set_artist(artist.clone());
    }
    if album.is_empty() {
        tag.remove_album();
    } else {
        tag.set_album(album.clone());
    }
    if genre.is_empty() {
        tag.remove_genre();
    } else {
        tag.set_genre(genre.clone());
    }
    if track_str.is_empty() {
        tag.remove_track();
    } else if let Ok(n) = track_str.parse::<u32>() {
        tag.set_track(n);
    }
    if year_str.is_empty() {
        tag.remove_year();
    } else if let Ok(n) = year_str.parse::<u32>() {
        tag.set_year(n);
    }
}
