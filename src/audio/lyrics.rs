// Local sidecar lyrics take priority over embedded tags and online results.
pub(super) fn load_lyrics(path: &std::path::Path) -> Option<String> {
    for extension in ["lrc", "LRC", "txt", "TXT"] {
        let lyrics_path = path.with_extension(extension);
        if let Ok(content) = std::fs::read_to_string(&lyrics_path)
            && let Some(content) = non_empty_lyrics(&content)
        {
            return Some(content);
        }
    }
    None
}

pub(super) fn clean_filename_title(filename: &str) -> String {
    let trimmed = filename.trim();
    let without_track_number = trimmed
        .find(|c: char| !c.is_ascii_digit() && !matches!(c, ' ' | '-' | '_' | '.'))
        .filter(|&index| index > 0 && index <= 5)
        .map(|index| &trimmed[index..])
        .unwrap_or(trimmed);
    without_track_number
        .trim_start_matches([' ', '-', '_', '.'])
        .trim()
        .to_string()
}

pub(super) fn non_empty_lyrics(lyrics: &str) -> Option<String> {
    let lyrics = lyrics.trim_start_matches('\u{feff}').trim();
    (!lyrics.is_empty()).then(|| lyrics.to_string())
}

fn lyrics_from_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("syncedLyrics")
        .and_then(|value| value.as_str())
        .and_then(non_empty_lyrics)
        .or_else(|| {
            value
                .get("plainLyrics")
                .and_then(|value| value.as_str())
                .and_then(non_empty_lyrics)
        })
}

// Remove search noise while preserving meaningful parts such as "(I Can't Get No)".
fn clean_search_title(value: &str) -> String {
    let value = value.trim().replace('_', " ");
    let mut output = String::new();
    let mut rest = value.as_str();
    while let Some(index) = rest.find(['(', '[']) {
        output.push_str(&rest[..index]);
        let close = if rest.as_bytes()[index] == b'(' {
            ')'
        } else {
            ']'
        };
        let Some(end) = rest[index..].find(close).map(|offset| offset + index) else {
            output.push_str(&rest[index..]);
            rest = "";
            break;
        };
        let group = &rest[index + 1..end];
        if !is_search_noise(group) {
            output.push_str(&rest[index..=end]);
        }
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    if let Some((base, suffix)) = output.rsplit_once(" - ")
        && is_search_noise(suffix)
    {
        output = base.to_string();
    }
    let lower = output.to_ascii_lowercase();
    for marker in [" feat.", " feat ", " ft.", " ft ", " featuring "] {
        if let Some(index) = lower.find(marker) {
            output.truncate(index);
            break;
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_search_noise(value: &str) -> bool {
    let lower = value.trim().to_lowercase();
    lower.starts_with("feat")
        || lower.starts_with("ft.")
        || lower.starts_with("ft ")
        || lower.contains("remaster")
        || lower.contains("radio edit")
        || lower.contains("radio version")
        || lower.contains("album version")
        || lower.contains("single version")
        || lower.contains("original mix")
        || lower.contains("official video")
        || lower.contains("official audio")
        || matches!(
            lower.as_str(),
            "lyrics" | "lyric video" | "explicit" | "hq" | "hd"
        )
}

fn normalized_track_name(value: &str) -> String {
    clean_search_title(value)
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn primary_artist(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let end = [
        ",",
        ";",
        " feat.",
        " feat ",
        " ft.",
        " ft ",
        " featuring ",
        " & ",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .min()
    .unwrap_or(value.len());
    value[..end].trim().to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Query {
    title: String,
    artist: Option<String>,
}

fn queries(title: &str, artist: Option<&str>) -> Vec<Query> {
    let artist = artist.map(str::trim).filter(|a| !a.is_empty());
    let title = title.trim().to_string();
    let mut result = vec![Query {
        title: title.clone(),
        artist: artist.map(str::to_string),
    }];
    let mut resolved_title = title.clone();
    let mut resolved_artist = artist.map(str::to_string);
    for separator in [" - ", " – ", " — "] {
        if let Some((prefix, song)) = title.split_once(separator)
            && !prefix.trim().is_empty()
            && !song.trim().is_empty()
            && (artist.is_none()
                || artist
                    .is_some_and(|a| normalized_track_name(a) == normalized_track_name(prefix)))
        {
            resolved_title = song.trim().to_string();
            resolved_artist = Some(prefix.trim().to_string());
            break;
        }
    }
    let resolved = Query {
        title: resolved_title.clone(),
        artist: resolved_artist.clone(),
    };
    let cleaned = Query {
        title: clean_search_title(&resolved_title),
        artist: resolved_artist
            .as_deref()
            .map(primary_artist)
            .filter(|a| !a.is_empty()),
    };
    for query in [resolved, cleaned] {
        if !query.title.is_empty() && !result.contains(&query) {
            result.push(query);
        }
    }
    result
}

fn title_similarity(expected: &str, candidate: &str) -> Option<i32> {
    if expected == candidate {
        return Some(100);
    }
    // Allow a small typo in longer titles, never arbitrary substring matches like Home/Homecoming.
    let a: Vec<_> = expected.chars().collect();
    let b: Vec<_> = candidate.chars().collect();
    if a.len().min(b.len()) < 8 || a.len().max(b.len()) > 256 {
        return None;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(old + 1)
                .min(previous + usize::from(ca != cb));
            previous = old;
        }
    }
    let distance = row[b.len()];
    (distance <= 2 && distance * 10 <= a.len().max(b.len())).then_some(80)
}

fn best_search_result(
    results: &serde_json::Value,
    title: &str,
    artist: Option<&str>,
    duration: Option<u64>,
) -> Option<String> {
    let duration = duration.filter(|duration| *duration > 0);
    let expected_title = normalized_track_name(title);
    let expected_artist = artist
        .filter(|a| !a.trim().is_empty())
        .map(|a| normalized_track_name(&primary_artist(a)));
    if expected_title.is_empty() {
        return None;
    }
    results
        .as_array()?
        .iter()
        .filter_map(|result| {
            let candidate_title = normalized_track_name(result.get("trackName")?.as_str()?);
            let mut score = title_similarity(&expected_title, &candidate_title)?;
            if let Some(expected) = &expected_artist {
                let candidate =
                    normalized_track_name(&primary_artist(result.get("artistName")?.as_str()?));
                // A matching title alone is insufficient when we know the artist.
                if expected != &candidate {
                    return None;
                }
                score += 50;
            }
            let difference = duration
                .zip(result.get("duration").and_then(|v| v.as_f64()))
                .map(|(a, b)| (a as f64 - b).abs());
            score += match difference {
                Some(d) if d <= 3.0 => 30,
                Some(d) if d <= 10.0 => 10,
                Some(d) if d > 30.0 => -30,
                _ => 0,
            };
            if expected_artist.is_none() && !difference.is_some_and(|d| d <= 10.0) {
                return None;
            }
            // A different edit can share words but not timestamps. Use plain lyrics in that case.
            let lyrics = if difference.is_some_and(|d| d > 5.0) {
                result
                    .get("plainLyrics")
                    .and_then(|v| v.as_str())
                    .and_then(non_empty_lyrics)
                    .or_else(|| lyrics_from_value(result).and_then(|text| unsync_lyrics(&text)))?
            } else {
                lyrics_from_value(result)?
            };
            if result
                .get("syncedLyrics")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.trim().is_empty())
                && difference.is_some_and(|d| d <= 3.0)
            {
                score += 5;
            }
            Some((score, lyrics))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, lyrics)| lyrics)
}

fn unsync_lyrics(text: &str) -> Option<String> {
    let lines: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let mut line = line.trim();
            while line.starts_with('[') {
                let end = line.find(']')?;
                if !line[1..end].starts_with(|c: char| c.is_ascii_digit()) {
                    return None;
                }
                line = line[end + 1..].trim();
            }
            (!line.is_empty()).then_some(line)
        })
        .collect();
    non_empty_lyrics(&lines.join("\n"))
}

fn friendly_request_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(429, _) => {
            "The lyrics service is busy right now. Please try again in a moment.".to_string()
        }
        ureq::Error::Status(code, _) if code >= 500 => {
            "The lyrics service is temporarily unavailable. Please try again later.".to_string()
        }
        ureq::Error::Status(_, _) => {
            "The lyrics service couldn't complete this request.".to_string()
        }
        ureq::Error::Transport(transport) => match transport.kind() {
            ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed => {
                "You're offline, so online lyrics couldn't be loaded.".to_string()
            }
            ureq::ErrorKind::Io => {
                "The lyrics lookup timed out. Please try again in a moment.".to_string()
            }
            _ => "Online lyrics couldn't be reached right now.".to_string(),
        },
    }
}

fn get_json(agent: &ureq::Agent, url: &str) -> Result<Option<serde_json::Value>, LookupError> {
    let response = match agent
        .get(url)
        .set(
            "User-Agent",
            concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/gibranlp/stash)"
            ),
        )
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(400 | 404, _)) => return Ok(None),
        Err(error) => {
            let stop = matches!(error, ureq::Error::Status(429, _));
            let retry_after = if let ureq::Error::Status(429, response) = &error {
                Some(std::time::Duration::from_secs(
                    response
                        .header("Retry-After")
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(60)
                        .min(86400),
                ))
            } else {
                None
            };
            return Err(LookupError {
                message: friendly_request_error(error),
                stop,
                retry_after,
            });
        }
    };

    response.into_json().map(Some).map_err(|_| LookupError {
        message: "The lyrics service sent an unexpected response. Please try again later."
            .to_string(),
        stop: false,
        retry_after: None,
    })
}

#[derive(Debug)]
struct LookupError {
    message: String,
    stop: bool,
    retry_after: Option<std::time::Duration>,
}

fn query_url(endpoint: &str, query: &Query) -> String {
    let mut url = format!(
        "https://lrclib.net/api/{endpoint}?track_name={}",
        urlencoding::encode(&query.title)
    );
    if let Some(artist) = &query.artist {
        url.push_str(&format!("&artist_name={}", urlencoding::encode(artist)));
    }
    url
}

// The request callback makes fallback behavior testable without a real lyrics service.
fn lookup_with(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration: Option<u64>,
    mut request: impl FnMut(&str) -> Result<Option<serde_json::Value>, LookupError>,
) -> Result<Option<String>, String> {
    let variants = queries(title, artist);
    let mut urls = Vec::new();
    let original = &variants[0];
    if original.artist.is_some() {
        let mut exact = query_url("get", original);
        if let Some(album) = album.filter(|a| !a.trim().is_empty()) {
            exact.push_str(&format!("&album_name={}", urlencoding::encode(album)));
        }
        if let Some(duration) = duration.filter(|d| (1..=3600).contains(d)) {
            exact.push_str(&format!("&duration={duration}"));
        }
        urls.push((exact, original.clone(), true));
    }
    for query in &variants {
        urls.push((query_url("search", query), query.clone(), false));
    }
    let clean = variants.last().unwrap();
    if let Some(artist) = &clean.artist {
        // Broad search handles punctuation and artist collaborations without requiring all tags.
        let words = format!("{} {}", artist, clean.title);
        urls.push((
            format!(
                "https://lrclib.net/api/search?q={}",
                urlencoding::encode(&words)
            ),
            clean.clone(),
            false,
        ));
        let title_only = Query {
            title: clean.title.clone(),
            artist: None,
        };
        urls.push((query_url("search", &title_only), clean.clone(), false));
    }
    let mut last_error = None;
    let mut visited = std::collections::HashSet::new();
    for (url, query, exact) in urls {
        if !visited.insert(url.clone()) {
            continue;
        }
        match request(&url) {
            Ok(Some(value)) => {
                let results = if exact {
                    serde_json::json!([value])
                } else {
                    value
                };
                if let Some(lyrics) =
                    best_search_result(&results, &query.title, query.artist.as_deref(), duration)
                {
                    return Ok(Some(lyrics));
                }
            }
            Ok(None) => {}
            Err(error) => {
                if error.stop {
                    return Err(error.message);
                }
                last_error = Some(error.message);
            }
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

fn cache_key(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration: Option<u64>,
) -> String {
    serde_json::json!([title, artist, album, duration]).to_string()
}

fn cache_path(key: &str) -> Option<std::path::PathBuf> {
    // Deterministic filename; the original key is also checked when reading the entry.
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    dirs::cache_dir().map(|root| {
        root.join("stash/lyrics-v2")
            .join(format!("{hash:016x}.json"))
    })
}

fn read_cache(path: &std::path::Path, key: &str) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if record["key"] != key {
        return None;
    }
    record["lyrics"].as_str().and_then(non_empty_lyrics)
}

fn write_cache(path: &std::path::Path, key: &str, lyrics: &str) {
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let record = serde_json::json!({"key": key, "lyrics": lyrics});
    // Cache failures must never turn successful lyrics into an error.
    let _ = std::fs::write(path, record.to_string());
}

pub(super) fn fetch_lyrics_online(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration: Option<u64>,
    still_current: impl Fn() -> bool,
) -> Result<Option<String>, String> {
    // Serialize lookups so rapid track changes cannot hammer the public API.
    static REQUEST_LOCK: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let key = cache_key(title, artist, album, duration);
    let path = cache_path(&key);
    if let Some(lyrics) = path.as_deref().and_then(|path| read_cache(path, &key)) {
        return Ok(Some(lyrics));
    }
    let mut next_request = REQUEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if !still_current() {
        return Ok(None);
    }
    if next_request.is_some_and(|next| {
        next > std::time::Instant::now() + std::time::Duration::from_millis(500)
    }) {
        return Err(
            "The lyrics service is busy right now. Please try again in a moment.".to_string(),
        );
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(4))
        .timeout_read(std::time::Duration::from_secs(6))
        .timeout(std::time::Duration::from_secs(8))
        .build();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let result = lookup_with(title, artist, album, duration, |url| {
        if !still_current() {
            return Err(LookupError {
                message: "Lyrics lookup cancelled.".to_string(),
                stop: true,
                retry_after: None,
            });
        }
        if std::time::Instant::now() >= deadline {
            return Err(LookupError {
                message: "The lyrics lookup timed out. Please try again in a moment.".to_string(),
                stop: true,
                retry_after: None,
            });
        }
        if let Some(delay) =
            next_request.and_then(|next| next.checked_duration_since(std::time::Instant::now()))
        {
            std::thread::sleep(delay);
        }
        let response = get_json(&agent, url);
        let delay = response
            .as_ref()
            .err()
            .and_then(|error| error.retry_after)
            .unwrap_or(std::time::Duration::from_millis(250));
        *next_request = Some(std::time::Instant::now() + delay);
        response
    })?;
    if let (Some(path), Some(lyrics)) = (path.as_deref(), result.as_deref()) {
        write_cache(path, &key, lyrics);
    }
    Ok(result)
}

#[cfg(test)]
mod lyrics_tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "requires network access to LRCLIB"]
    fn live_lookup_matches_cleaned_collaboration_and_filename() {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(8))
            .build();
        for (title, artist, duration) in [
            ("Foster The People - Pumped Up Kicks", None, 240),
            (
                "Reality (feat. Janieck Devy) - Radio Edit",
                Some("Lost Frequencies, Janieck Devy"),
                158,
            ),
        ] {
            let result = lookup_with(title, artist, None, Some(duration), |url| {
                std::thread::sleep(std::time::Duration::from_millis(250));
                get_json(&agent, url)
            })
            .unwrap();
            assert!(result.is_some(), "no matching lyrics for {title}");
        }
    }

    #[test]
    fn cleans_track_number_from_filename_title() {
        assert_eq!(clean_filename_title("01 - A Great Song"), "A Great Song");
        assert_eq!(clean_filename_title("12_song title"), "song title");
        assert_eq!(clean_filename_title("1985"), "1985");
    }

    #[test]
    fn ignores_empty_local_or_remote_lyrics() {
        assert_eq!(non_empty_lyrics(" \n\t"), None);
        assert_eq!(
            non_empty_lyrics("\u{feff}Words\n"),
            Some("Words".to_string())
        );
        assert_eq!(lyrics_from_value(&json!({ "plainLyrics": "" })), None);
    }

    #[test]
    fn cleans_collaborations_and_infers_artist_from_filename() {
        let variants = queries(
            "Lost Frequencies, Janieck Devy - Reality (feat. Janieck Devy) - Radio Edit",
            None,
        );
        assert_eq!(
            variants.last().unwrap(),
            &Query {
                title: "Reality".into(),
                artist: Some("Lost Frequencies".into())
            }
        );
        assert_eq!(
            clean_search_title("Song (2011 Remastered) [Official Video]"),
            "Song"
        );
        assert_eq!(
            clean_search_title("(I Can't Get No) Satisfaction"),
            "(I Can't Get No) Satisfaction"
        );
        assert_eq!(clean_search_title("99 Luftballons"), "99 Luftballons");
        assert_eq!(
            clean_search_title("Song (Live at Wembley)"),
            "Song (Live at Wembley)"
        );
        assert_eq!(primary_artist("İbrahim, Guest"), "İbrahim");
    }

    #[test]
    fn rejects_different_artists_and_substring_titles() {
        let results = json!([
            {"trackName": "Homecoming", "artistName": "The Band", "duration": 180, "plainLyrics": "unrelated title"},
            {"trackName": "Home", "artistName": "Another Band", "duration": 180, "plainLyrics": "unrelated artist"}
        ]);
        assert_eq!(
            best_search_result(&results, "Home", Some("The Band"), Some(180)),
            None
        );
    }

    #[test]
    fn handles_small_typos_but_requires_duration_for_unknown_artists() {
        let results = json!([{ "trackName": "Californication", "artistName": "Red Hot Chili Peppers", "duration": 320, "plainLyrics": "matched" }]);
        assert_eq!(
            best_search_result(
                &results,
                "Californicaton",
                Some("Red Hot Chili Peppers"),
                Some(320)
            ),
            Some("matched".into())
        );
        assert_eq!(
            best_search_result(&results, "Californication", None, None),
            None
        );
        assert_eq!(
            best_search_result(&results, "Californication", None, Some(321)),
            Some("matched".into())
        );
    }

    #[test]
    fn different_edit_uses_plain_lyrics_instead_of_wrong_timestamps() {
        let results = json!([{ "trackName": "Song", "artistName": "Artist", "duration": 200, "syncedLyrics": "[00:01.00]Words", "plainLyrics": "Words" }]);
        assert_eq!(
            best_search_result(&results, "Song", Some("Artist"), Some(180)),
            Some("Words".into())
        );
        assert_eq!(
            best_search_result(&results, "Song", Some("Artist"), Some(200)),
            Some("[00:01.00]Words".into())
        );
        assert_eq!(
            unsync_lyrics("[ar:Artist]\n[00:01.00][00:02.00]Words\n[00:03.00]More"),
            Some("Words\nMore".into())
        );
    }

    #[test]
    fn fallback_search_recovers_from_exact_endpoint_error() {
        let mut calls = 0;
        let result = lookup_with("Song", Some("Artist"), None, Some(180), |url| {
            calls += 1;
            if url.contains("/get?") {
                Err(LookupError {
                    message: "temporarily unavailable".into(),
                    stop: false,
                    retry_after: None,
                })
            } else {
                Ok(Some(
                    json!([{ "trackName": "Song", "artistName": "Artist", "duration": 180, "plainLyrics": "matched" }]),
                ))
            }
        });
        assert_eq!(result.unwrap(), Some("matched".into()));
        assert_eq!(calls, 2);
    }

    #[test]
    fn cleaned_query_finds_featured_artist_track_after_misses() {
        let mut calls = Vec::new();
        let result = lookup_with(
            "Reality (feat. Janieck Devy) - Radio Edit",
            Some("Lost Frequencies, Janieck Devy"),
            None,
            Some(158),
            |url| {
                calls.push(url.to_string());
                if url.contains("track_name=Reality&artist_name=Lost%20Frequencies") {
                    Ok(Some(
                        json!([{ "trackName": "Reality", "artistName": "Lost Frequencies", "duration": 158, "plainLyrics": "matched" }]),
                    ))
                } else {
                    Ok(None)
                }
            },
        );
        assert_eq!(result.unwrap(), Some("matched".into()));
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn title_only_fallback_still_validates_artist() {
        let result = lookup_with("Song", Some("Artist"), None, Some(180), |url| {
            if url.ends_with("track_name=Song") {
                Ok(Some(json!([
                    { "trackName": "Song", "artistName": "Wrong Artist", "duration": 180, "plainLyrics": "wrong" },
                    { "trackName": "Song", "artistName": "Artist", "duration": 180, "plainLyrics": "right" }
                ])))
            } else {
                Ok(None)
            }
        });
        assert_eq!(result.unwrap(), Some("right".into()));
    }

    #[test]
    fn rate_limit_stops_further_requests_and_invalid_duration_is_omitted() {
        let mut calls = 0;
        let result = lookup_with("Song", Some("Artist"), None, Some(0), |url| {
            calls += 1;
            assert!(!url.contains("duration="));
            Err(LookupError {
                message: "busy".into(),
                stop: true,
                retry_after: Some(std::time::Duration::from_secs(60)),
            })
        });
        assert_eq!(result, Err("busy".into()));
        assert_eq!(calls, 1);
    }

    #[test]
    fn cache_reuses_lyrics_and_rejects_wrong_keys_or_corruption() {
        let root = std::env::temp_dir().join(format!("stash-lyrics-test-{}", std::process::id()));
        let path = root.join("entry.json");
        let key = cache_key("Song", Some("Artist"), None, Some(180));
        write_cache(&path, &key, "[00:01.00]Words");
        assert_eq!(read_cache(&path, &key), Some("[00:01.00]Words".into()));
        assert_eq!(read_cache(&path, "other key"), None);
        std::fs::write(&path, "broken json").unwrap();
        assert_eq!(read_cache(&path, &key), None);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn search_fallback_prefers_matching_artist_and_duration() {
        let results = json!([
            {
                "trackName": "Home",
                "artistName": "Someone Else",
                "duration": 180.0,
                "plainLyrics": "wrong"
            },
            {
                "trackName": "Home (Remastered)",
                "artistName": "The Band",
                "duration": 201.5,
                "plainLyrics": "right"
            }
        ]);

        assert_eq!(
            best_search_result(&results, "Home", Some("The Band"), Some(202)),
            Some("right".to_string())
        );
    }
}
