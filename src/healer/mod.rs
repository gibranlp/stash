pub mod backup;
pub mod filename;
pub mod fingerprint;
pub mod musicbrainz;
pub mod pipeline;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use ratatui::widgets::ListState;

#[derive(Default, Clone, Debug)]
pub struct TagSnapshot {
    pub title:        Option<String>,
    pub artist:       Option<String>,
    pub album:        Option<String>,
    pub album_artist: Option<String>,
    pub track:        Option<u32>,
    pub disc:         Option<u32>,
    pub year:         Option<u32>,
    pub genre:        Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaIssue {
    MissingTitle,
    MissingArtist,
    MissingAlbum,
    MissingYear,
    MissingTrackNumber,
}

impl MetaIssue {
    pub fn label(self) -> &'static str {
        match self {
            Self::MissingTitle       => "Missing Title",
            Self::MissingArtist      => "Missing Artist",
            Self::MissingAlbum       => "Missing Album",
            Self::MissingYear        => "Missing Year",
            Self::MissingTrackNumber => "Missing Track#",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchSource {
    Filename,
    MusicBrainz,
    AcoustID,
    Manual,
}

impl MatchSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Filename     => "Filename",
            Self::MusicBrainz  => "MusicBrainz",
            Self::AcoustID     => "AcoustID",
            Self::Manual       => "Manual",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HealMatch {
    pub source:     MatchSource,
    pub confidence: u8,
    pub tags:       TagSnapshot,
    pub note:       String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealStatus {
    Pending,
    Skipped,
    NoMatch,
}

#[derive(Debug, Clone)]
pub struct HealerFile {
    pub path:     PathBuf,
    pub issues:   Vec<MetaIssue>,
    pub original: TagSnapshot,
    pub matches:  Vec<HealMatch>,
    pub status:   HealStatus,
}

#[derive(Debug, Default, Clone)]
pub struct HealthReport {
    pub total:           usize,
    pub healthy:         usize,
    pub missing_title:   usize,
    pub missing_artist:  usize,
    pub missing_album:   usize,
    pub missing_year:    usize,
    pub missing_track:   usize,
    pub has_matches:     usize,
    pub no_match:        usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealerScreen {
    Menu,
    Scanning,
    Report,
    FileList,
    Preview,
    Editor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealScanState {
    Idle,
    Scanning,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealLookupState {
    Idle,
    Searching,
    Done,
    #[allow(dead_code)] // reserved for network error signaling
    Failed,
}

pub struct HealerState {
    pub screen:         HealerScreen,
    pub files:          Vec<HealerFile>,
    pub list_idx:       usize,
    pub list_state:     ListState,
    pub scan_state:     HealScanState,
    pub pending_scan:   Option<Arc<Mutex<Option<Vec<HealerFile>>>>>,
    pub scan_progress:  Arc<Mutex<(usize, usize, String)>>,
    pub report:         Option<HealthReport>,
    pub lookup_state:   HealLookupState,
    pub pending_lookup: Option<Arc<Mutex<Option<Vec<HealMatch>>>>>,
    pub match_idx:      usize,
    pub edit_fields:    [String; 8],
    pub edit_field_idx: usize,
    pub edit_cursor:    usize,
    pub edit_typing:    bool,
    pub edit_original:  Option<[String; 8]>,
    pub search_query:   String,
    pub search_active:  bool,
}

impl HealerState {
    pub fn new() -> Self {
        Self {
            screen:         HealerScreen::Menu,
            files:          Vec::new(),
            list_idx:       0,
            list_state:     ListState::default(),
            scan_state:     HealScanState::Idle,
            pending_scan:   None,
            scan_progress:  Arc::new(Mutex::new((0, 0, String::new()))),
            report:         None,
            lookup_state:   HealLookupState::Idle,
            pending_lookup: None,
            match_idx:      0,
            edit_fields:    Default::default(),
            edit_field_idx: 0,
            edit_cursor:    0,
            edit_typing:    false,
            edit_original:  None,
            search_query:   String::new(),
            search_active:  false,
        }
    }

    pub fn poll_scan(&mut self) -> bool {
        let result = self.pending_scan.as_ref().and_then(|s| s.lock().unwrap().take());
        if let Some(files) = result {
            let report = build_report(&files);
            // Only keep files that actually have issues; report totals are already computed above
            self.files = files.into_iter().filter(|f| !f.issues.is_empty()).collect();
            self.report = Some(report);
            self.scan_state = HealScanState::Done;
            self.pending_scan = None;
            self.screen = HealerScreen::Report;
            true
        } else {
            false
        }
    }

    pub fn poll_lookup(&mut self) -> bool {
        let result = self.pending_lookup.as_ref().and_then(|s| s.lock().unwrap().take());
        if let Some(matches) = result {
            if let Some(file) = self.files.get_mut(self.list_idx) {
                let existing_sources: Vec<MatchSource> = file.matches.iter().map(|m| m.source).collect();
                for m in &matches {
                    if !existing_sources.contains(&m.source) {
                        file.matches.push(m.clone());
                    }
                }
                file.matches.sort_by(|a, b| b.confidence.cmp(&a.confidence));
                if matches.is_empty() && file.matches.is_empty() {
                    file.status = HealStatus::NoMatch;
                }
            }
            self.lookup_state = HealLookupState::Done;
            self.pending_lookup = None;
            true
        } else {
            false
        }
    }

    pub fn filtered_indices(&self) -> Vec<usize> {
        if self.search_query.is_empty() {
            return (0..self.files.len()).collect();
        }
        let q = self.search_query.to_lowercase();
        self.files.iter().enumerate()
            .filter(|(_, f)| {
                let name = f.path.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                name.contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn current_file_index(&self) -> Option<usize> {
        self.filtered_indices().get(self.list_idx).copied()
    }

    pub fn current_file(&self) -> Option<&HealerFile> {
        self.current_file_index().and_then(|i| self.files.get(i))
    }

    pub fn current_file_mut(&mut self) -> Option<&mut HealerFile> {
        let i = self.current_file_index()?;
        self.files.get_mut(i)
    }

    pub fn current_match(&self) -> Option<&HealMatch> {
        self.current_file().and_then(|f| f.matches.get(self.match_idx))
    }

    pub fn load_editor_from_match(&mut self) {
        if let Some(m) = self.current_match() {
            let t = &m.tags;
            let fields = [
                t.title.clone().unwrap_or_default(),
                t.artist.clone().unwrap_or_default(),
                t.album.clone().unwrap_or_default(),
                t.album_artist.clone().unwrap_or_default(),
                t.track.map(|n| n.to_string()).unwrap_or_default(),
                t.disc.map(|n| n.to_string()).unwrap_or_default(),
                t.year.map(|n| n.to_string()).unwrap_or_default(),
                t.genre.clone().unwrap_or_default(),
            ];
            self.edit_original = Some(fields.clone());
            self.edit_fields = fields;
            self.edit_field_idx = 0;
            self.edit_cursor = self.edit_fields[0].chars().count();
            self.edit_typing = false;
        }
    }

    pub fn load_editor_from_original(&mut self) {
        let idx = self.current_file_index();
        if let Some(f) = idx.and_then(|i| self.files.get(i)) {
            let t = &f.original;
            let fields = [
                t.title.clone().unwrap_or_default(),
                t.artist.clone().unwrap_or_default(),
                t.album.clone().unwrap_or_default(),
                t.album_artist.clone().unwrap_or_default(),
                t.track.map(|n| n.to_string()).unwrap_or_default(),
                t.disc.map(|n| n.to_string()).unwrap_or_default(),
                t.year.map(|n| n.to_string()).unwrap_or_default(),
                t.genre.clone().unwrap_or_default(),
            ];
            self.edit_original = Some(fields.clone());
            self.edit_fields = fields;
            self.edit_field_idx = 0;
            self.edit_cursor = self.edit_fields[0].chars().count();
            self.edit_typing = false;
        }
    }

    pub fn editor_as_match(&self) -> HealMatch {
        HealMatch {
            source: MatchSource::Manual,
            confidence: 100,
            tags: TagSnapshot {
                title:        non_empty(&self.edit_fields[0]),
                artist:       non_empty(&self.edit_fields[1]),
                album:        non_empty(&self.edit_fields[2]),
                album_artist: non_empty(&self.edit_fields[3]),
                track:        self.edit_fields[4].parse().ok(),
                disc:         self.edit_fields[5].parse().ok(),
                year:         self.edit_fields[6].parse().ok(),
                genre:        non_empty(&self.edit_fields[7]),
            },
            note: "Manually edited".to_string(),
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() { None } else { Some(s.to_string()) }
}

pub fn build_report(files: &[HealerFile]) -> HealthReport {
    let mut r = HealthReport { total: files.len(), ..Default::default() };
    for f in files {
        if f.issues.is_empty() {
            r.healthy += 1;
        }
        for &issue in &f.issues {
            match issue {
                MetaIssue::MissingTitle       => r.missing_title   += 1,
                MetaIssue::MissingArtist      => r.missing_artist  += 1,
                MetaIssue::MissingAlbum       => r.missing_album   += 1,
                MetaIssue::MissingYear        => r.missing_year    += 1,
                MetaIssue::MissingTrackNumber => r.missing_track   += 1,
            }
        }
        if !f.issues.is_empty() {
            if f.matches.is_empty() { r.no_match += 1; } else { r.has_matches += 1; }
        }
    }
    r
}

pub const EDIT_FIELD_NAMES: [&str; 8] = [
    "Title", "Artist", "Album", "Album Artist",
    "Track", "Disc", "Year", "Genre",
];
