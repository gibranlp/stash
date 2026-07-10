use std::path::Path;
use std::process::Command;
use super::{HealMatch, MatchSource, TagSnapshot};

pub struct Fingerprint {
    pub duration: u32,
    pub fingerprint: String,
}

pub fn compute(path: &Path) -> Option<Fingerprint> {
    let output = Command::new("fpcalc").arg("-json").arg(path).output().ok()?;
    if !output.status.success() { return None; }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let duration = json.get("duration")?.as_f64()? as u32;
    let fingerprint = json.get("fingerprint")?.as_str()?.to_string();
    Some(Fingerprint { duration, fingerprint })
}

pub fn lookup(fp: &Fingerprint, api_key: &str) -> Vec<HealMatch> {
    if api_key.is_empty() { return Vec::new(); }
    let url = format!(
        "https://api.acoustid.org/v2/lookup?client={}&fingerprint={}&duration={}&meta=recordings+releases",
        api_key, fp.fingerprint, fp.duration
    );
    let resp = match ureq::get(&url).set("User-Agent", "stash-music-player/0.4").call() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    std::thread::sleep(std::time::Duration::from_millis(350));
    let json: serde_json::Value = match resp.into_json() {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };
    parse_acoustid_response(&json)
}

fn parse_acoustid_response(json: &serde_json::Value) -> Vec<HealMatch> {
    let results = match json.get("results").and_then(|r| r.as_array()) {
        Some(r) => r,
        None => return Vec::new(),
    };
    results.iter().filter_map(|result| {
        let score = (result.get("score")?.as_f64()? * 100.0) as u8;
        let recording = result.get("recordings")?.as_array()?.first()?;
        let title = recording.get("title")?.as_str()?.to_string();
        let artist = recording.get("artists")
            .and_then(|a| a.as_array()).and_then(|a| a.first())
            .and_then(|a| a.get("name")).and_then(|n| n.as_str())
            .unwrap_or("").to_string();
        let release = recording.get("releases").and_then(|r| r.as_array()).and_then(|r| r.first());
        let album = release.and_then(|r| r.get("title")).and_then(|t| t.as_str()).map(|s| s.to_string());
        let year = release.and_then(|r| r.get("date")).and_then(|d| d.get("year")).and_then(|y| y.as_u64()).map(|y| y as u32);
        Some(HealMatch {
            source: MatchSource::AcoustID,
            confidence: score.min(98),
            tags: TagSnapshot {
                title: Some(title),
                artist: if artist.is_empty() { None } else { Some(artist) },
                album,
                year,
                ..Default::default()
            },
            note: format!("AcoustID score: {:.0}%", result.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0) * 100.0),
        })
    }).collect()
}
