use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use lofty::prelude::*;
use lofty::probe::Probe;

use super::{HealerFile, HealMatch, HealStatus, MatchSource, MetaIssue, TagSnapshot};
use super::filename;

pub fn scan_files(
    paths: Vec<PathBuf>,
    progress: Arc<Mutex<(usize, usize, String)>>,
    slot: Arc<Mutex<Option<Vec<HealerFile>>>>,
) {
    std::thread::spawn(move || {
        let total = paths.len();
        let mut results = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                *progress.lock().unwrap() = (i, total, name);
            }
            results.push(scan_single(path));
        }
        *progress.lock().unwrap() = (total, total, String::new());
        *slot.lock().unwrap() = Some(results);
    });
}

fn scan_single(path: &Path) -> HealerFile {
    let mut snap = TagSnapshot::default();
    if let Ok(tagged) = Probe::open(path).and_then(|p| p.read()) {
        if let Some(tag) = tagged.primary_tag().or(tagged.first_tag()) {
            snap.title        = tag.title().map(|s| s.to_string());
            snap.artist       = tag.artist().map(|s| s.to_string());
            snap.album        = tag.album().map(|s| s.to_string());
            snap.album_artist = tag.get_string(&lofty::tag::ItemKey::AlbumArtist).map(|s| s.to_string());
            snap.track        = tag.track();
            snap.disc         = tag.disk();
            snap.year         = tag.year();
            snap.genre        = tag.genre().map(|s| s.to_string());
        }
    }
    let issues = detect_issues(&snap);
    let mut matches: Vec<HealMatch> = Vec::new();
    if !issues.is_empty() {
        if let Some(parsed) = filename::parse(path) {
            let confidence = filename_confidence(&parsed, &snap, &issues);
            matches.push(HealMatch {
                source: MatchSource::Filename,
                confidence,
                note: format!("Parsed from filename: {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                tags: parsed,
            });
        }
    }
    HealerFile { path: path.to_path_buf(), issues, original: snap, matches, status: HealStatus::Pending }
}

fn detect_issues(snap: &TagSnapshot) -> Vec<MetaIssue> {
    let mut issues = Vec::new();
    let is_blank = |o: &Option<String>| o.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true);
    if is_blank(&snap.title)  { issues.push(MetaIssue::MissingTitle); }
    if is_blank(&snap.artist) { issues.push(MetaIssue::MissingArtist); }
    if is_blank(&snap.album)  { issues.push(MetaIssue::MissingAlbum); }
    if snap.year.is_none()    { issues.push(MetaIssue::MissingYear); }
    if snap.track.is_none()   { issues.push(MetaIssue::MissingTrackNumber); }
    issues
}

fn filename_confidence(parsed: &TagSnapshot, current: &TagSnapshot, issues: &[MetaIssue]) -> u8 {
    let mut score: u8 = 50;
    if parsed.title.is_some()  { score = score.saturating_add(10); }
    if parsed.artist.is_some() { score = score.saturating_add(10); }
    if parsed.album.is_some()  { score = score.saturating_add(5); }
    if parsed.track.is_some()  { score = score.saturating_add(8); }
    if let (Some(pt), Some(ct)) = (&parsed.title, &current.title) {
        if pt.to_lowercase().contains(&ct.to_lowercase()) || ct.to_lowercase().contains(&pt.to_lowercase()) {
            score = score.saturating_add(10);
        }
    }
    let fillable = issues.iter().filter(|i| match i {
        MetaIssue::MissingTitle  => parsed.title.is_some(),
        MetaIssue::MissingArtist => parsed.artist.is_some(),
        MetaIssue::MissingAlbum  => parsed.album.is_some(),
        MetaIssue::MissingYear   => parsed.year.is_some(),
        MetaIssue::MissingTrackNumber => parsed.track.is_some(),
    }).count();
    if fillable == issues.len() { score = score.saturating_add(5); }
    score.min(85)
}

pub fn apply_match(path: &PathBuf, m: &HealMatch, backup: &mut super::backup::BackupStore, current: &TagSnapshot) -> Result<(), String> {
    backup.record(path, current);
    let _ = backup.save();
    let mut tagged_file = Probe::open(path).map_err(|e| e.to_string())?.read().map_err(|e| e.to_string())?;
    let has_tag = tagged_file.primary_tag().is_some() || tagged_file.first_tag().is_some();
    if !has_tag {
        let tag_type = tagged_file.file_type().primary_tag_type();
        tagged_file.insert_tag(lofty::tag::Tag::new(tag_type));
    }
    let modified = if let Some(tag) = tagged_file.primary_tag_mut() {
        apply_tags(tag, &m.tags);
        true
    } else if let Some(tag) = tagged_file.first_tag_mut() {
        apply_tags(tag, &m.tags);
        true
    } else {
        false
    };
    if !modified { return Err("No writable tag found".to_string()); }
    tagged_file.save_to_path(path, lofty::config::WriteOptions::default()).map_err(|e| e.to_string())
}

fn apply_tags(tag: &mut lofty::tag::Tag, t: &TagSnapshot) {
    if let Some(ref v) = t.title        { tag.set_title(v.clone()); }
    if let Some(ref v) = t.artist       { tag.set_artist(v.clone()); }
    if let Some(ref v) = t.album        { tag.set_album(v.clone()); }
    if let Some(ref v) = t.genre        { tag.set_genre(v.clone()); }
    if let Some(v) = t.track            { tag.set_track(v); }
    if let Some(v) = t.disc             { tag.set_disk(v); }
    if let Some(v) = t.year             { tag.set_year(v); }
    if let Some(ref v) = t.album_artist {
        tag.insert_text(lofty::tag::ItemKey::AlbumArtist, v.clone());
    }
}
