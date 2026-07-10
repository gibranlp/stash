use std::path::Path;
use super::TagSnapshot;

pub fn parse(path: &Path) -> Option<TagSnapshot> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let parent_name = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
    let grandparent_name = path.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());

    let mut result = try_patterns(&stem);
    if let Some(ref mut t) = result {
        if t.album.is_none() {
            if let Some(ref parent) = parent_name {
                if !is_generic_dir(parent) {
                    t.album = Some(parent.clone());
                }
            }
        }
        if t.artist.is_none() {
            if let Some(ref gp) = grandparent_name {
                if !is_generic_dir(gp) {
                    t.artist = Some(gp.clone());
                }
            }
        }
    }
    result
}

fn is_generic_dir(name: &str) -> bool {
    let lower = name.to_lowercase();
    matches!(lower.as_str(), "music" | "downloads" | "audio" | "mp3" | "flac" | "songs")
}

fn try_patterns(stem: &str) -> Option<TagSnapshot> {
    if let Some(t) = pattern_track_artist_title(stem) { return Some(t); }
    if let Some(t) = pattern_artist_title(stem) { return Some(t); }
    if let Some(t) = pattern_track_title(stem) { return Some(t); }
    Some(TagSnapshot { title: Some(clean_title(stem)), ..Default::default() })
}

fn strip_leading_track(s: &str) -> Option<(u32, &str)> {
    let s = s.trim();
    let digits_end = s.find(|c: char| !c.is_ascii_digit())?;
    if digits_end == 0 || digits_end > 3 { return None; }
    let num: u32 = s[..digits_end].parse().ok()?;
    let rest = s[digits_end..].trim_start_matches(|c: char| !c.is_alphanumeric() && c != '(').trim();
    Some((num, rest))
}

fn pattern_track_artist_title(s: &str) -> Option<TagSnapshot> {
    let (track, rest) = strip_leading_track(s)?;
    let parts: Vec<&str> = rest.splitn(2, " - ").collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        return Some(TagSnapshot {
            title:  Some(clean_title(parts[1])),
            artist: Some(parts[0].trim().to_string()),
            track:  Some(track),
            ..Default::default()
        });
    }
    None
}

fn pattern_artist_title(s: &str) -> Option<TagSnapshot> {
    let parts: Vec<&str> = s.splitn(2, " - ").collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        if parts[0].trim().parse::<u32>().is_ok() { return None; }
        return Some(TagSnapshot {
            title:  Some(clean_title(parts[1])),
            artist: Some(parts[0].trim().to_string()),
            ..Default::default()
        });
    }
    None
}

fn pattern_track_title(s: &str) -> Option<TagSnapshot> {
    let (track, rest) = strip_leading_track(s)?;
    if !rest.is_empty() {
        return Some(TagSnapshot {
            title: Some(clean_title(rest)),
            track: Some(track),
            ..Default::default()
        });
    }
    None
}

fn clean_title(s: &str) -> String {
    s.trim().to_string()
}
