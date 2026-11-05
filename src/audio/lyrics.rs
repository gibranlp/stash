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
                && difference.is_some_and(|d| d <= 5.0)
            {
                score += 15;
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
            let mut lookup_error = LookupError::new(friendly_request_error(error));
            lookup_error.stop = stop;
            lookup_error.retry_after = retry_after;
            return Err(lookup_error);
        }
    };

    response.into_json().map(Some).map_err(|_| {
        LookupError::new("The lyrics service sent an unexpected response. Please try again later.")
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LookupError {
    pub(super) message: String,
    // Stop querying this service (e.g. rate limited) but keep trying the others.
    stop: bool,
    // Stop the whole lookup: the track changed or the overall deadline passed.
    abort: bool,
    pub(super) retry_after: Option<std::time::Duration>,
}

impl LookupError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stop: false,
            abort: false,
            retry_after: None,
        }
    }

    fn stop(mut self) -> Self {
        self.stop = true;
        self
    }

    fn abort(mut self) -> Self {
        self.stop = true;
        self.abort = true;
        self
    }

    fn retry_after(mut self, delay: std::time::Duration) -> Self {
        self.retry_after = Some(delay);
        self
    }
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

// True when the text carries LRC line timestamps such as [01:23.45].
pub(super) fn has_synced_timestamps(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim_start()
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .is_some_and(|(tag, _)| {
                tag.starts_with(|c: char| c.is_ascii_digit())
                    && tag.contains(':')
                    && tag.chars().all(|c| c.is_ascii_digit() || matches!(c, ':' | '.'))
            })
    })
}

type Request<'a> = dyn FnMut(&str) -> Result<Option<serde_json::Value>, LookupError> + 'a;

// The request callback makes fallback behavior testable without a real lyrics service.
fn lookup_with(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration: Option<u64>,
    request: &mut Request<'_>,
) -> Result<Option<String>, LookupError> {
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
    // LRCLIB often stores several uploads of one song; the exact match may be plain
    // while another upload is synced, so keep searching before settling for plain text.
    let mut plain = None;
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
                    if has_synced_timestamps(&lyrics) {
                        return Ok(Some(lyrics));
                    }
                    plain.get_or_insert(lyrics);
                }
            }
            Ok(None) => {}
            Err(error) => {
                let stop = error.stop;
                last_error = Some(error);
                if stop {
                    break;
                }
            }
        }
    }
    match (plain, last_error) {
        (Some(plain), _) => Ok(Some(plain)),
        (None, Some(error)) => Err(error),
        (None, None) => Ok(None),
    }
}

// NetEase credits ("作词 : Name") are timed lines that aren't lyrics.
fn clean_netease_lyrics(text: &str) -> Option<String> {
    if text.contains("纯音乐，请欣赏") {
        return None;
    }
    let lines: Vec<_> = text
        .lines()
        .filter(|line| {
            let mut words = line.trim();
            while let Some(rest) = words.strip_prefix('[') {
                match rest.split_once(']') {
                    Some((_, after)) => words = after.trim(),
                    None => break,
                }
            }
            !words.split_once([':', '：']).is_some_and(|(label, _)| {
                label.trim().chars().count() <= 6
                    && label.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
            })
        })
        .collect();
    let lyrics = non_empty_lyrics(&lines.join("\n"))?;
    (has_synced_timestamps(&lyrics) && unsync_lyrics(&lyrics).is_some()).then_some(lyrics)
}

// Second source: NetEase Cloud Music has broad synced-lyrics coverage and needs no key.
fn netease_with(
    title: &str,
    artist: Option<&str>,
    duration: Option<u64>,
    request: &mut Request<'_>,
) -> Result<Option<String>, LookupError> {
    let query = queries(title, artist).pop().unwrap();
    let expected_title = normalized_track_name(&query.title);
    if expected_title.is_empty() {
        return Ok(None);
    }
    let expected_artist = query.artist.as_deref().map(normalized_track_name);
    let terms = match &query.artist {
        Some(artist) => format!("{artist} {}", query.title),
        None => query.title.clone(),
    };
    let url = format!(
        "https://music.163.com/api/search/get?s={}&type=1&limit=10",
        urlencoding::encode(&terms)
    );
    let Some(results) = request(&url)? else {
        return Ok(None);
    };
    let duration = duration.filter(|duration| *duration > 0);
    let mut candidates: Vec<(i32, u64)> = results
        .pointer("/result/songs")
        .and_then(|songs| songs.as_array())
        .into_iter()
        .flatten()
        .filter_map(|song| {
            let name = normalized_track_name(song.get("name")?.as_str()?);
            let mut score = title_similarity(&expected_title, &name)?;
            if let Some(expected) = &expected_artist {
                let matches = song
                    .get("artists")?
                    .as_array()?
                    .iter()
                    .filter_map(|artist| artist.get("name")?.as_str())
                    .any(|name| normalized_track_name(&primary_artist(name)) == *expected);
                if !matches {
                    return None;
                }
                score += 50;
            }
            let difference = duration
                .zip(song.get("duration").and_then(|v| v.as_f64()))
                .map(|(expected, ms)| (expected as f64 - ms / 1000.0).abs());
            // Another edit's timestamps would drift, and we only take synced lyrics here.
            if difference.is_some_and(|d| d > 5.0)
                || (expected_artist.is_none() && !difference.is_some_and(|d| d <= 3.0))
            {
                return None;
            }
            if difference.is_some_and(|d| d <= 2.0) {
                score += 30;
            }
            Some((score, song.get("id")?.as_u64()?))
        })
        .collect();
    candidates.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    for (_, id) in candidates.into_iter().take(2) {
        let url = format!("https://music.163.com/api/song/lyric?id={id}&lv=1");
        if let Some(value) = request(&url)?
            && let Some(lyrics) = value
                .pointer("/lrc/lyric")
                .and_then(|v| v.as_str())
                .and_then(clean_netease_lyrics)
        {
            return Ok(Some(lyrics));
        }
    }
    Ok(None)
}

// Last resort for plain lyrics when neither synced source knows the song.
fn lyrics_ovh_with(
    title: &str,
    artist: Option<&str>,
    request: &mut Request<'_>,
) -> Result<Option<String>, LookupError> {
    let query = queries(title, artist).pop().unwrap();
    let Some(artist) = query.artist.filter(|_| !query.title.is_empty()) else {
        return Ok(None);
    };
    let url = format!(
        "https://api.lyrics.ovh/v1/{}/{}",
        urlencoding::encode(&artist),
        urlencoding::encode(&query.title)
    );
    let Some(value) = request(&url)? else {
        return Ok(None);
    };
    let Some(text) = value.get("lyrics").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let text = text.replace("\r\n", "\n");
    let text = match text.split_once('\n') {
        Some((first, rest)) if first.starts_with("Paroles de la chanson") => rest,
        _ => text.as_str(),
    };
    Ok(non_empty_lyrics(text))
}

// Try every source, preferring synced lyrics. With `synced_only`, plain results are
// discarded because the caller already has plain lyrics from the file itself.
fn lookup_all(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration: Option<u64>,
    synced_only: bool,
    request: &mut Request<'_>,
) -> Result<Option<String>, LookupError> {
    let mut plain = None;
    let mut error: Option<LookupError> = None;
    // Each source either finishes the lookup or records its plain result / error.
    let record = |result: Result<Option<String>, LookupError>,
                      plain: &mut Option<String>,
                      error: &mut Option<LookupError>|
     -> Option<String> {
        match result {
            Ok(Some(lyrics)) if has_synced_timestamps(&lyrics) => return Some(lyrics),
            Ok(Some(lyrics)) => {
                plain.get_or_insert(lyrics);
            }
            Ok(None) => {}
            Err(e) => {
                error.get_or_insert(e);
            }
        }
        None
    };
    let aborted = |error: &Option<LookupError>| error.as_ref().is_some_and(|e| e.abort);

    let lrclib = lookup_with(title, artist, album, duration, request);
    if let Some(synced) = record(lrclib, &mut plain, &mut error) {
        return Ok(Some(synced));
    }
    if !aborted(&error) {
        let netease = netease_with(title, artist, duration, request);
        if let Some(synced) = record(netease, &mut plain, &mut error) {
            return Ok(Some(synced));
        }
    }
    if synced_only {
        return error.map_or(Ok(None), Err);
    }
    if plain.is_none() && !aborted(&error) {
        let ovh = lyrics_ovh_with(title, artist, request);
        record(ovh, &mut plain, &mut error);
    }
    match (plain, error) {
        (Some(plain), _) => Ok(Some(plain)),
        (None, Some(error)) => Err(error),
        (None, None) => Ok(None),
    }
}

fn cache_key(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration: Option<u64>,
    synced_only: bool,
) -> String {
    serde_json::json!([title, artist, album, duration, synced_only]).to_string()
}

fn cache_path(key: &str) -> Option<std::path::PathBuf> {
    // Deterministic filename; the original key is also checked when reading the entry.
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    dirs::cache_dir().map(|root| {
        root.join("stash/lyrics-v3")
            .join(format!("{hash:016x}.json"))
    })
}

// Synced lyrics are final. Plain or missing lyrics are re-checked after a week,
// since LRCLIB keeps receiving new synced uploads.
const RECHECK_AFTER_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, PartialEq, Eq)]
struct CacheEntry {
    lyrics: Option<String>,
    fresh: bool,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache(path: &std::path::Path, key: &str) -> Option<CacheEntry> {
    if std::fs::metadata(path).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if record["key"] != key {
        return None;
    }
    let lyrics = record["lyrics"].as_str().and_then(non_empty_lyrics);
    let checked = record["checked"].as_u64().unwrap_or(0);
    let fresh = lyrics.as_deref().is_some_and(has_synced_timestamps)
        || unix_now().saturating_sub(checked) < RECHECK_AFTER_SECS;
    Some(CacheEntry { lyrics, fresh })
}

fn write_cache(path: &std::path::Path, key: &str, lyrics: Option<&str>) {
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let record = serde_json::json!({"key": key, "lyrics": lyrics, "checked": unix_now()});
    // Cache failures must never turn successful lyrics into an error.
    let _ = std::fs::write(path, record.to_string());
}

fn service_index(url: &str) -> usize {
    if url.contains("lrclib.net") {
        0
    } else if url.contains("music.163.com") {
        1
    } else {
        2
    }
}

pub(super) fn fetch_lyrics_online(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration: Option<u64>,
    synced_only: bool,
    still_current: impl Fn() -> bool,
) -> Result<Option<String>, LookupError> {
    // Serialize lookups and pace each service separately so rapid track changes cannot
    // hammer a public API, and one rate-limited service does not stall the others.
    static PACING: std::sync::Mutex<[Option<std::time::Instant>; 3]> =
        std::sync::Mutex::new([None; 3]);
    let key = cache_key(title, artist, album, duration, synced_only);
    let path = cache_path(&key);
    let cached = path.as_deref().and_then(|path| read_cache(path, &key));
    if let Some(entry) = &cached
        && entry.fresh
    {
        return Ok(entry.lyrics.clone());
    }
    let mut pacing = PACING.lock().unwrap_or_else(|poison| poison.into_inner());
    if !still_current() {
        return Ok(None);
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(4))
        .timeout_read(std::time::Duration::from_secs(6))
        .timeout(std::time::Duration::from_secs(8))
        .build();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut request = |url: &str| {
        if !still_current() {
            return Err(LookupError::new("Lyrics lookup cancelled.").abort());
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(LookupError::new(
                "The lyrics lookup timed out. Please try again in a moment.",
            )
            .abort());
        }
        let slot = &mut pacing[service_index(url)];
        if let Some(wait) = slot.and_then(|next| next.checked_duration_since(now)) {
            if wait > std::time::Duration::from_millis(500) {
                return Err(LookupError::new(
                    "The lyrics service is busy right now. Please try again in a moment.",
                )
                .stop()
                .retry_after(wait));
            }
            std::thread::sleep(wait);
        }
        let response = get_json(&agent, url);
        let delay = response
            .as_ref()
            .err()
            .and_then(|error| error.retry_after)
            .unwrap_or(std::time::Duration::from_millis(250));
        *slot = Some(std::time::Instant::now() + delay);
        response
    };
    match lookup_all(title, artist, album, duration, synced_only, &mut request) {
        Ok(result) => {
            if let Some(path) = path.as_deref() {
                write_cache(path, &key, result.as_deref());
            }
            Ok(result)
        }
        // A stale cached copy beats an error while the services are unreachable.
        Err(_) if cached.as_ref().is_some_and(|entry| entry.lyrics.is_some()) => {
            Ok(cached.and_then(|entry| entry.lyrics))
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod lyrics_tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "requires network access to the lyrics services"]
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
            let result = lookup_all(title, artist, None, Some(duration), false, &mut |url| {
                std::thread::sleep(std::time::Duration::from_millis(250));
                get_json(&agent, url)
            })
            .unwrap();
            assert!(result.is_some(), "no matching lyrics for {title}");
        }
        let synced = netease_with("Pumped Up Kicks", Some("Foster The People"), Some(238), &mut |url| {
            std::thread::sleep(std::time::Duration::from_millis(250));
            get_json(&agent, url)
        })
        .unwrap();
        assert!(synced.is_some_and(|lyrics| has_synced_timestamps(&lyrics) && !lyrics.contains("作词")));
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
        let result = lookup_with("Song", Some("Artist"), None, Some(180), &mut |url| {
            calls += 1;
            if url.contains("/get?") {
                Err(LookupError::new("temporarily unavailable"))
            } else {
                Ok(Some(
                    json!([{ "trackName": "Song", "artistName": "Artist", "duration": 180, "syncedLyrics": "[00:01.00]matched" }]),
                ))
            }
        });
        assert_eq!(result.unwrap(), Some("[00:01.00]matched".into()));
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
            &mut |url| {
                calls.push(url.to_string());
                if url.contains("track_name=Reality&artist_name=Lost%20Frequencies") {
                    Ok(Some(
                        json!([{ "trackName": "Reality", "artistName": "Lost Frequencies", "duration": 158, "syncedLyrics": "[00:01.00]matched" }]),
                    ))
                } else {
                    Ok(None)
                }
            },
        );
        assert_eq!(result.unwrap(), Some("[00:01.00]matched".into()));
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn title_only_fallback_still_validates_artist() {
        let result = lookup_with("Song", Some("Artist"), None, Some(180), &mut |url| {
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
        let result = lookup_with("Song", Some("Artist"), None, Some(0), &mut |url| {
            calls += 1;
            assert!(!url.contains("duration="));
            Err(LookupError::new("busy")
                .stop()
                .retry_after(std::time::Duration::from_secs(60)))
        });
        let error = result.unwrap_err();
        assert_eq!(error.message, "busy");
        assert_eq!(error.retry_after, Some(std::time::Duration::from_secs(60)));
        assert_eq!(calls, 1);
    }

    #[test]
    fn cache_reuses_lyrics_and_rejects_wrong_keys_or_corruption() {
        let root = std::env::temp_dir().join(format!("stash-lyrics-test-{}", std::process::id()));
        let path = root.join("entry.json");
        let key = cache_key("Song", Some("Artist"), None, Some(180), false);
        write_cache(&path, &key, Some("[00:01.00]Words"));
        assert_eq!(
            read_cache(&path, &key),
            Some(CacheEntry { lyrics: Some("[00:01.00]Words".into()), fresh: true })
        );
        assert_eq!(read_cache(&path, "other key"), None);
        // Missing lyrics are remembered, but only until the re-check window passes.
        write_cache(&path, &key, None);
        assert_eq!(read_cache(&path, &key), Some(CacheEntry { lyrics: None, fresh: true }));
        let old = json!({"key": key, "lyrics": "Plain", "checked": 0}).to_string();
        std::fs::write(&path, old).unwrap();
        assert_eq!(
            read_cache(&path, &key),
            Some(CacheEntry { lyrics: Some("Plain".into()), fresh: false })
        );
        std::fs::write(&path, "broken json").unwrap();
        assert_eq!(read_cache(&path, &key), None);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn exact_plain_match_keeps_searching_for_synced_upload() {
        let result = lookup_with("Song", Some("Artist"), None, Some(180), &mut |url| {
            if url.contains("/get?") {
                Ok(Some(json!({ "trackName": "Song", "artistName": "Artist", "duration": 180, "plainLyrics": "plain" })))
            } else {
                Ok(Some(json!([{ "trackName": "Song", "artistName": "Artist", "duration": 180, "syncedLyrics": "[00:01.00]synced" }])))
            }
        });
        assert_eq!(result.unwrap(), Some("[00:01.00]synced".into()));
        let result = lookup_with("Song", Some("Artist"), None, Some(180), &mut |url| {
            if url.contains("/get?") {
                Ok(Some(json!({ "trackName": "Song", "artistName": "Artist", "duration": 180, "plainLyrics": "plain" })))
            } else {
                Ok(None)
            }
        });
        assert_eq!(result.unwrap(), Some("plain".into()));
    }

    #[test]
    fn netease_supplies_synced_lyrics_when_lrclib_only_has_plain() {
        let mut urls = Vec::new();
        let result = lookup_all("Song", Some("Artist"), None, Some(180), false, &mut |url| {
            urls.push(url.to_string());
            if url.contains("lrclib.net/api/get") {
                Ok(Some(json!({ "trackName": "Song", "artistName": "Artist", "duration": 180, "plainLyrics": "plain" })))
            } else if url.contains("music.163.com/api/search") {
                Ok(Some(json!({ "result": { "songs": [
                    { "id": 1, "name": "Song", "artists": [{ "name": "Other" }], "duration": 180000 },
                    { "id": 2, "name": "Song", "artists": [{ "name": "Artist" }], "duration": 199000 },
                    { "id": 3, "name": "Song", "artists": [{ "name": "Artist" }], "duration": 180500 }
                ] } })))
            } else if url.contains("song/lyric?id=3") {
                Ok(Some(json!({ "lrc": { "lyric": "[00:00.000] 作词 : Someone\n[00:05.00]Hello" } })))
            } else {
                Ok(None)
            }
        });
        assert_eq!(result.unwrap(), Some("[00:05.00]Hello".into()));
        assert!(!urls.iter().any(|u| u.contains("id=1") || u.contains("id=2")));
        assert!(!urls.iter().any(|u| u.contains("lyrics.ovh")));
    }

    #[test]
    fn lyrics_ovh_is_last_resort_and_errors_without_results_are_reported() {
        let result = lookup_all("Song", Some("Artist"), None, Some(180), false, &mut |url| {
            if url.contains("lyrics.ovh") {
                Ok(Some(json!({ "lyrics": "Paroles de la chanson Song par Artist\r\nWords\r\nMore" })))
            } else {
                Ok(None)
            }
        });
        assert_eq!(result.unwrap(), Some("Words\nMore".into()));

        let result = lookup_all("Song", Some("Artist"), None, Some(180), false, &mut |url| {
            if url.contains("lrclib") {
                Err(LookupError::new("down"))
            } else {
                Ok(None)
            }
        });
        assert_eq!(result.unwrap_err().message, "down");

        // Synced-only lookups never fall back to plain text.
        let result = lookup_all("Song", Some("Artist"), None, Some(180), true, &mut |url| {
            assert!(!url.contains("lyrics.ovh"));
            Ok(Some(json!([{ "trackName": "Song", "artistName": "Artist", "duration": 180, "plainLyrics": "plain" }])))
        });
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn aborted_lookup_stops_every_source() {
        let mut calls = 0;
        let result = lookup_all("Song", Some("Artist"), None, Some(180), false, &mut |_| {
            calls += 1;
            Err(LookupError::new("cancelled").abort())
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn detects_synced_lyrics_and_drops_netease_credits() {
        assert!(has_synced_timestamps("[ar:Artist]\n[01:02.50]Words"));
        assert!(!has_synced_timestamps("[Chorus]\nWords"));
        assert_eq!(clean_netease_lyrics("[00:00.00]纯音乐，请欣赏"), None);
        assert_eq!(clean_netease_lyrics("[00:00.00] 作曲 : Someone"), None);
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
