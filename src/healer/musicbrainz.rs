use super::{HealMatch, MatchSource, TagSnapshot};

const MB_BASE: &str = "https://musicbrainz.org/ws/2";
const USER_AGENT: &str = "stash-music-player/0.4 (https://github.com/gibranlp/stash)";

pub fn search_recording(title: &str, artist: &str) -> Vec<HealMatch> {
    let query = build_query(title, artist);
    let url = format!("{}/recording?query={}&fmt=json&limit=5", MB_BASE, urlencoding::encode(&query));

    let result = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .call();

    std::thread::sleep(std::time::Duration::from_millis(1100));

    let resp = match result {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let json: serde_json::Value = match resp.into_json() {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };

    parse_recording_response(&json, title, artist)
}

fn build_query(title: &str, artist: &str) -> String {
    if artist.is_empty() {
        format!("recording:{}", title)
    } else {
        format!("recording:{} AND artist:{}", title, artist)
    }
}

fn parse_recording_response(json: &serde_json::Value, orig_title: &str, orig_artist: &str) -> Vec<HealMatch> {
    let recordings = match json.get("recordings").and_then(|r| r.as_array()) {
        Some(r) => r,
        None => return Vec::new(),
    };

    recordings.iter().filter_map(|rec| {
        let title = rec.get("title")?.as_str()?.to_string();

        let artist = rec.get("artist-credit")
            .and_then(|ac| ac.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("artist"))
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();

        let release = rec.get("releases")
            .and_then(|r| r.as_array())
            .and_then(|r| r.first());

        let album = release.and_then(|r| r.get("title")).and_then(|t| t.as_str()).map(|s| s.to_string());
        let year  = release.and_then(|r| r.get("date")).and_then(|d| d.as_str())
            .and_then(|d| d.split('-').next()).and_then(|y| y.parse::<u32>().ok());
        let track = release.and_then(|r| r.get("media")).and_then(|m| m.as_array())
            .and_then(|m| m.first()).and_then(|m| m.get("track")).and_then(|t| t.as_array())
            .and_then(|t| t.first()).and_then(|t| t.get("number")).and_then(|n| n.as_str())
            .and_then(|n| n.parse::<u32>().ok());

        let score_raw = rec.get("score").and_then(|s| s.as_u64()).unwrap_or(0) as u8;
        let title_sim = similarity(&title.to_lowercase(), &orig_title.to_lowercase());
        let artist_sim = if orig_artist.is_empty() { 0.5 } else {
            similarity(&artist.to_lowercase(), &orig_artist.to_lowercase())
        };
        let confidence = ((score_raw as f32 * 0.6 + title_sim * 25.0 + artist_sim * 15.0) as u8).min(99);
        let note = format!("{} match from MusicBrainz", rec.get("score").and_then(|s| s.as_u64()).unwrap_or(0));

        Some(HealMatch {
            source: MatchSource::MusicBrainz,
            confidence,
            tags: TagSnapshot {
                title: Some(title),
                artist: if artist.is_empty() { None } else { Some(artist) },
                album,
                year,
                track,
                ..Default::default()
            },
            note,
        })
    }).collect()
}

fn similarity(a: &str, b: &str) -> f32 {
    if a == b { return 1.0; }
    if a.is_empty() || b.is_empty() { return 0.0; }
    let a_words: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let b_words: std::collections::HashSet<&str> = b.split_whitespace().collect();
    let intersection = a_words.intersection(&b_words).count();
    let union = a_words.union(&b_words).count();
    if union == 0 { return 0.0; }
    intersection as f32 / union as f32
}
