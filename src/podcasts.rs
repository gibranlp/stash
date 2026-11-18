//! Podcast discovery, RSS subscriptions, and atomic episode downloads.
use crate::preview::LatestLoader;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Show {
    pub title: String,
    pub author: String,
    pub url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Episode {
    pub title: String,
    pub url: String,
    pub date: String,
    pub description: String,
    pub mime: String,
    pub duration: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Feed {
    pub show: Show,
    pub description: String,
    pub episodes: Vec<Episode>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    Search,
    Url,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PodcastPane {
    Discover,
    Subscriptions,
    Episodes,
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum EpisodeSort {
    #[default]
    Newest,
    Oldest,
    Title,
    TitleDescending,
}
impl EpisodeSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "Newest first",
            Self::Oldest => "Oldest first",
            Self::Title => "Title A–Z",
            Self::TitleDescending => "Title Z–A",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Newest => Self::Oldest,
            Self::Oldest => Self::Title,
            Self::Title => Self::TitleDescending,
            Self::TitleDescending => Self::Newest,
        }
    }
}

#[derive(Clone)]
enum Job {
    Search(String),
    Feed(String),
    Download {
        show: Show,
        episode: Episode,
        play: bool,
    },
}
enum ResultData {
    Search(Vec<Show>),
    Feed(Feed),
    Download(PathBuf, bool),
}

pub struct PodcastState {
    pub root: PathBuf,
    pub subscriptions: Vec<Show>,
    pub shows: Vec<Show>,
    pub search_results: bool,
    pub show_index: usize,
    pub episode_index: usize,
    pub focus: PodcastPane,
    pub subscription_index: usize,
    pub subscription_list_state: ratatui::widgets::ListState,
    pub episode_sort: EpisodeSort,
    notice: Option<(String, Instant)>,
    pub feed: Option<Feed>,
    pub prompt: Option<Prompt>,
    pub input: String,
    pub message: String,
    pub busy: bool,
    pub play_ready: Option<PathBuf>,
    pub show_list_state: ratatui::widgets::ListState,
    pub episode_list_state: ratatui::widgets::ListState,
    pub downloaded: std::collections::HashSet<String>,
    subscribe_after_load: bool,
    allow_autoplay: bool,
    progress: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    worker: LatestLoader<Job, ResultData>,
}

impl PodcastState {
    pub fn new() -> Self {
        Self::at(
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Podcasts"),
        )
    }
    pub(crate) fn at(root: PathBuf) -> Self {
        let subscriptions: Vec<Show> = std::fs::read(root.join("subscriptions.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let worker_root = root.clone();
        let progress = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let (done, size) = (progress.clone(), total.clone());
        let worker = LatestLoader::new(move |job| match job {
            Job::Search(query) => search(query).map(ResultData::Search),
            Job::Feed(url) => load_feed(&worker_root, url).map(ResultData::Feed),
            Job::Download {
                show,
                episode,
                play,
            } => download(&worker_root, show, episode, &done, &size)
                .map(|path| ResultData::Download(path, *play)),
        });
        Self {
            root,
            shows: Vec::new(),
            subscriptions,
            search_results: false,
            show_index: 0,
            episode_index: 0,
            focus: PodcastPane::Subscriptions,
            subscription_index: 0,
            subscription_list_state: Default::default(),
            episode_sort: EpisodeSort::default(),
            notice: None,
            feed: None,
            prompt: None,
            input: String::new(),
            message: "Search with / or add a feed URL with a".into(),
            busy: false,
            play_ready: None,
            show_list_state: Default::default(),
            episode_list_state: Default::default(),
            downloaded: Default::default(),
            subscribe_after_load: false,
            allow_autoplay: false,
            progress,
            total,
            worker,
        }
    }
    pub fn open(&mut self) {
        if let Err(e) = std::fs::create_dir_all(&self.root) {
            self.message = format!("Cannot create {}: {e}", self.root.display());
        } else if self.feed.is_none() && !self.subscriptions.is_empty() && !self.busy {
            self.open_selected();
        }
    }
    fn start(&mut self, job: Job, message: &str) {
        if self.busy {
            self.message = "Please wait for the current operation to finish".into();
            return;
        }
        self.notice = None;
        if let Job::Download { play, .. } = &job {
            self.allow_autoplay = *play;
        }
        self.busy = true;
        self.progress.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        self.message = message.into();
        self.worker.request(Some(job));
    }
    pub fn poll(&mut self) {
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            self.notice = None;
        }
        let Some((_, result)) = self.worker.poll() else {
            return;
        };
        self.busy = false;
        match result {
            Err(e) => {
                self.subscribe_after_load = false;
                self.message = format!("Podcast error: {e}");
            }
            Ok(ResultData::Search(shows)) => {
                self.shows = shows;
                self.search_results = true;
                self.show_index = 0;
                self.focus = PodcastPane::Discover;
                self.message = format!(
                    "{} podcasts found — Enter opens episodes; f subscribes",
                    self.shows.len()
                );
            }
            Ok(ResultData::Feed(feed)) => {
                self.message = format!(
                    "{} episodes — Enter downloads and plays; d downloads",
                    feed.episodes.len()
                );
                self.downloaded = feed
                    .episodes
                    .iter()
                    .filter(|e| episode_path(&self.root, &feed.show, e).is_file())
                    .map(|e| e.url.clone())
                    .collect();
                let subscribe = self.subscribe_after_load && !self.subscribed(&feed.show);
                self.subscribe_after_load = false;
                self.feed = Some(feed);
                self.episode_index = 0;
                self.sort_episodes();
                self.episode_index = 0;
                self.focus = PodcastPane::Episodes;
                if subscribe {
                    self.toggle_subscription();
                }
            }
            Ok(ResultData::Download(path, play)) => {
                self.notice = Some((
                    format!("Saved {}", path.display()),
                    Instant::now() + Duration::from_secs(4),
                ));
                self.message = "Ready".into();
                if let Some(feed) = &self.feed {
                    for episode in &feed.episodes {
                        if episode_path(&self.root, &feed.show, episode) == path {
                            self.downloaded.insert(episode.url.clone());
                        }
                    }
                }
                if play && self.allow_autoplay {
                    self.play_ready = Some(path);
                }
                self.allow_autoplay = false;
            }
        }
    }
    pub fn progress_text(&self) -> String {
        if !self.busy
            && let Some((message, until)) = &self.notice
            && Instant::now() < *until
        {
            return message.clone();
        }
        let done = self.progress.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        if done == 0 {
            self.message.clone()
        } else if self.busy && total > 0 {
            format!(
                "Downloading: {:.1} / {:.1} MB",
                done as f64 / 1e6,
                total as f64 / 1e6
            )
        } else if self.busy {
            format!("Downloading: {:.1} MB", done as f64 / 1e6)
        } else {
            self.message.clone()
        }
    }
    fn selected_show(&self) -> Option<Show> {
        match self.focus {
            PodcastPane::Discover => self.shows.get(self.show_index).cloned(),
            PodcastPane::Subscriptions => self.subscriptions.get(self.subscription_index).cloned(),
            PodcastPane::Episodes => self.feed.as_ref().map(|f| f.show.clone()),
        }
    }
    fn open_selected(&mut self) {
        if let Some(show) = self.selected_show() {
            self.start(Job::Feed(show.url), "Loading episodes...");
        }
    }
    fn sort_episodes(&mut self) {
        let Some(feed) = &mut self.feed else { return };
        let selected = feed.episodes.get(self.episode_index).map(|e| e.url.clone());
        match self.episode_sort {
            EpisodeSort::Newest | EpisodeSort::Oldest => {
                let newest = self.episode_sort == EpisodeSort::Newest;
                feed.episodes.sort_by_cached_key(|e| {
                    let time = chrono::DateTime::parse_from_rfc2822(&e.date)
                        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&e.date))
                        .ok()
                        .map(|t| t.timestamp());
                    (
                        time.is_none(),
                        time.map(|t| if newest { -t } else { t }).unwrap_or(0),
                    )
                });
            }
            EpisodeSort::Title => feed.episodes.sort_by_cached_key(|e| e.title.to_lowercase()),
            EpisodeSort::TitleDescending => feed
                .episodes
                .sort_by_cached_key(|e| std::cmp::Reverse(e.title.to_lowercase())),
        }
        self.episode_index = selected
            .and_then(|url| feed.episodes.iter().position(|e| e.url == url))
            .unwrap_or(0);
    }
    pub fn subscribed(&self, show: &Show) -> bool {
        self.subscriptions.iter().any(|s| s.url == show.url)
    }
    fn toggle_subscription(&mut self) {
        let show = self.selected_show();
        let Some(show) = show else { return };
        let mut subscriptions = self.subscriptions.clone();
        let removed = if let Some(i) = subscriptions.iter().position(|s| s.url == show.url) {
            subscriptions.remove(i);
            true
        } else {
            subscriptions.push(show.clone());
            false
        };
        match save_json(&self.root.join("subscriptions.json"), &subscriptions) {
            Ok(()) => {
                self.subscriptions = subscriptions;
                self.message = format!(
                    "{} {}",
                    if removed {
                        "Unsubscribed from"
                    } else {
                        "Subscribed to"
                    },
                    show.title
                );
                self.subscription_index = self
                    .subscription_index
                    .min(self.subscriptions.len().saturating_sub(1));
            }
            Err(e) => self.message = format!("Cannot save subscriptions: {e}"),
        }
    }
    pub fn local_episode(&self, episode: &Episode) -> Option<PathBuf> {
        let path = episode_path(&self.root, &self.feed.as_ref()?.show, episode);
        path.is_file().then_some(path)
    }
    pub fn cancel_pending_playback(&mut self) {
        self.allow_autoplay = false;
        self.play_ready = None;
    }
    pub fn play_selected(&mut self) {
        if self.focus == PodcastPane::Episodes {
            self.get_episode(true);
        } else {
            self.message = "Select an episode with Tab, then press Enter or Space to play".into();
        }
    }
    pub fn step_episode(&mut self, delta: isize, current: Option<&Path>) {
        if self.busy {
            self.message = "Wait for the current operation before changing episodes".into();
            return;
        }
        let Some(feed) = &self.feed else { return };
        if feed.episodes.is_empty() {
            return;
        }
        let from = current
            .and_then(|path| {
                feed.episodes
                    .iter()
                    .position(|episode| episode_path(&self.root, &feed.show, episode) == path)
            })
            .unwrap_or(self.episode_index);
        let next = from as isize + delta;
        if next < 0 || next >= feed.episodes.len() as isize {
            self.message = "No more episodes in this direction".into();
            return;
        }
        self.episode_index = next as usize;
        self.focus = PodcastPane::Episodes;
        self.get_episode(true);
    }
    fn get_episode(&mut self, play: bool) {
        if self.busy {
            self.message = "Wait for the current operation before playing another episode".into();
            return;
        }
        let Some(feed) = &self.feed else { return };
        let Some(episode) = feed.episodes.get(self.episode_index) else {
            return;
        };
        if let Some(path) = self.local_episode(episode) {
            self.notice = Some((
                format!("Already downloaded: {}", path.display()),
                Instant::now() + Duration::from_secs(4),
            ));
            self.message = "Ready".into();
            if play {
                self.play_ready = Some(path);
            }
        } else {
            self.start(
                Job::Download {
                    show: feed.show.clone(),
                    episode: episode.clone(),
                    play,
                },
                "Downloading episode...",
            );
        }
    }
    pub fn paste(&mut self, text: &str) {
        self.input.extend(
            text.chars()
                .filter(|c| !c.is_control())
                .take(4096usize.saturating_sub(self.input.len())),
        );
    }
    pub fn edit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.input.clear();
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear()
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if self.input.len() < 4096 {
                    self.input.push(c);
                }
            }
            KeyCode::Enter => {
                let query = self.input.trim().to_string();
                if query.is_empty() {
                    return;
                }
                let prompt = self.prompt.take();
                self.input.clear();
                if prompt == Some(Prompt::Url) {
                    match http_url(&query) {
                        Ok(url) => {
                            if !self.busy {
                                self.subscribe_after_load = true;
                            }
                            self.start(Job::Feed(url.to_string()), "Loading podcast feed...");
                        }
                        Err(e) => self.message = e,
                    }
                } else {
                    self.start(Job::Search(query), "Searching podcasts...");
                }
            }
            _ => {}
        }
    }
    /// Returns false for the shared volume, playback, help and quit keys.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        self.notice = None;
        let (len, index) = match self.focus {
            PodcastPane::Discover => (self.shows.len(), &mut self.show_index),
            PodcastPane::Subscriptions => (self.subscriptions.len(), &mut self.subscription_index),
            PodcastPane::Episodes => (
                self.feed.as_ref().map_or(0, |f| f.episodes.len()),
                &mut self.episode_index,
            ),
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *index = (*index + 1).min(len.saturating_sub(1)),
            KeyCode::PageUp => *index = index.saturating_sub(10),
            KeyCode::PageDown => *index = (*index + 10).min(len.saturating_sub(1)),
            KeyCode::Home => *index = 0,
            KeyCode::End => *index = len.saturating_sub(1),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    PodcastPane::Discover => PodcastPane::Subscriptions,
                    PodcastPane::Subscriptions => PodcastPane::Episodes,
                    PodcastPane::Episodes => PodcastPane::Discover,
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.focus = if self.focus == PodcastPane::Episodes {
                    PodcastPane::Subscriptions
                } else {
                    PodcastPane::Discover
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.focus = if self.focus == PodcastPane::Discover {
                    PodcastPane::Subscriptions
                } else {
                    PodcastPane::Episodes
                }
            }
            KeyCode::Enter if self.focus == PodcastPane::Episodes => self.get_episode(true),
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char('d') if self.focus == PodcastPane::Episodes => self.get_episode(false),
            KeyCode::Char('f') => self.toggle_subscription(),
            KeyCode::Char('R') => {
                if self.focus == PodcastPane::Episodes
                    && let Some(feed) = &self.feed
                {
                    self.start(Job::Feed(feed.show.url.clone()), "Refreshing episodes...");
                } else {
                    self.open_selected();
                }
            }
            KeyCode::Char('F') | KeyCode::Char('S') | KeyCode::Esc => {
                self.focus = PodcastPane::Subscriptions
            }
            KeyCode::Char('o') if self.focus == PodcastPane::Episodes => {
                self.episode_sort = self.episode_sort.next();
                self.sort_episodes();
                self.message = format!("Episodes sorted: {}", self.episode_sort.label());
            }
            KeyCode::Char('/') => {
                self.prompt = Some(Prompt::Search);
                self.input.clear();
            }
            KeyCode::Char('a') => {
                self.prompt = Some(Prompt::Url);
                self.input.clear();
            }
            _ => return false,
        }
        true
    }
}

fn http_url(value: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(value.trim()).map_err(|e| format!("Invalid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Use an http:// or https:// podcast RSS feed URL".into());
    }
    Ok(url)
}
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent(concat!("Stash/", env!("CARGO_PKG_VERSION")))
        .build()
}
fn read_bounded(response: ureq::Response) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Podcast feed is larger than 8 MB".into());
    }
    Ok(bytes)
}
fn search(query: &str) -> Result<Vec<Show>, String> {
    let response = agent()
        .get("https://itunes.apple.com/search")
        .query("media", "podcast")
        .query("entity", "podcast")
        .query("limit", "40")
        .query("term", query)
        .call()
        .map_err(|e| e.to_string())?;
    parse_search(&read_bounded(response)?)
}
fn parse_search(bytes: &[u8]) -> Result<Vec<Show>, String> {
    let json: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let results = json["results"]
        .as_array()
        .ok_or("Invalid podcast search response")?;
    let mut shows = Vec::new();
    for result in results {
        let Some(url) = result["feedUrl"].as_str().and_then(|u| http_url(u).ok()) else {
            continue;
        };
        if shows.iter().any(|s: &Show| s.url == url.as_str()) {
            continue;
        }
        shows.push(Show {
            title: result["collectionName"]
                .as_str()
                .unwrap_or("Untitled podcast")
                .into(),
            author: result["artistName"].as_str().unwrap_or("").into(),
            url: url.to_string(),
        });
    }
    Ok(shows)
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn load_feed(root: &Path, url: &str) -> Result<Feed, String> {
    let url = http_url(url)?;
    let cache = root
        .join(".feeds")
        .join(format!("{}.json", digest(url.as_str())));
    let fetched = (|| {
        let response = agent()
            .get(url.as_str())
            .call()
            .map_err(|e| e.to_string())?;
        let base = http_url(response.get_url())?;
        parse_feed(&read_bounded(response)?, &base, url.as_str())
    })();
    match fetched {
        Ok(feed) => {
            let _ = save_json(&cache, &feed);
            Ok(feed)
        }
        Err(e) => std::fs::read(cache)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or(e),
    }
}
fn parse_feed(bytes: &[u8], base: &url::Url, original_url: &str) -> Result<Feed, String> {
    let channel =
        rss::Channel::read_from(bytes).map_err(|e| format!("Cannot read podcast RSS feed: {e}"))?;
    let show = Show {
        title: channel.title().to_string(),
        author: channel
            .itunes_ext()
            .and_then(|e| e.author())
            .unwrap_or("")
            .to_string(),
        url: original_url.into(),
    };
    let mut episodes = Vec::new();
    for item in channel.items().iter().take(2000) {
        let Some(enclosure) = item.enclosure() else {
            continue;
        };
        let Ok(url) = base.join(enclosure.url()) else {
            continue;
        };
        if http_url(url.as_str()).is_err() {
            continue;
        }
        if !enclosure.mime_type().starts_with("audio/")
            && !matches!(
                url.path()
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_lowercase()
                    .as_str(),
                "mp3" | "m4a" | "ogg" | "opus" | "flac" | "wav" | "aac"
            )
        {
            continue;
        }
        if episodes.iter().any(|e: &Episode| e.url == url.as_str()) {
            continue;
        }
        episodes.push(Episode {
            title: item.title().unwrap_or("Untitled episode").into(),
            url: url.to_string(),
            date: item.pub_date().unwrap_or("").into(),
            description: plain_text(item.description().unwrap_or("")),
            mime: enclosure.mime_type().into(),
            duration: item
                .itunes_ext()
                .and_then(|e| e.duration())
                .unwrap_or("")
                .into(),
        });
    }
    Ok(Feed {
        show,
        description: plain_text(channel.description()),
        episodes,
    })
}
fn plain_text(text: &str) -> String {
    let mut inside = false;
    text.chars()
        .filter_map(|c| match c {
            '<' => {
                inside = true;
                Some(' ')
            }
            '>' => {
                inside = false;
                None
            }
            _ if inside => None,
            _ => Some(c),
        })
        .collect::<String>()
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn safe_name(value: &str) -> String {
    let name: String = value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(50)
        .collect();
    if name.trim().is_empty() {
        "Podcast".into()
    } else {
        name.trim().to_string()
    }
}
fn episode_path(root: &Path, show: &Show, episode: &Episode) -> PathBuf {
    let extension = match episode.mime.split(';').next().unwrap_or("") {
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/aac" => "aac",
        _ => "mp3",
    };
    root.join(format!(
        "{}-{}",
        safe_name(&show.title),
        &digest(&show.url)[..8]
    ))
    .join(format!(
        "{}-{}.{}",
        safe_name(&episode.title),
        &digest(&episode.url)[..12],
        extension
    ))
}
fn save_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("No storage directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut temp, value).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
fn download(
    root: &Path,
    show: &Show,
    episode: &Episode,
    progress: &AtomicU64,
    total: &AtomicU64,
) -> Result<PathBuf, String> {
    let path = episode_path(root, show, episode);
    if path.is_file() {
        return Ok(path);
    }
    let url = http_url(&episode.url)?;
    let response = agent()
        .get(url.as_str())
        .call()
        .map_err(|e| e.to_string())?;
    if response
        .header("Content-Type")
        .is_some_and(|t| t.starts_with("text/") || t.contains("json"))
    {
        return Err("Episode URL returned a page instead of audio".into());
    }
    let expected = response
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok());
    total.store(expected.unwrap_or(0), Ordering::Relaxed);
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    let mut reader = response.into_reader();
    let mut buffer = [0; 64 * 1024];
    let mut written = 0;
    loop {
        let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        written += count as u64;
        if written > 2 * 1024 * 1024 * 1024 {
            return Err("Episode exceeds the 2 GB download limit".into());
        }
        temp.write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
        progress.store(written, Ordering::Relaxed);
    }
    if written == 0 || expected.is_some_and(|n| n != written) {
        return Err("Incomplete episode download — please retry".into());
    }
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist_noclobber(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel>
    <title>Test &amp; Show</title><link>https://example.com</link><description><![CDATA[<p>Show description</p>]]></description>
    <itunes:author>Author</itunes:author>
    <item><title>Episode One</title><description><![CDATA[<p>Hello &amp; welcome</p>]]></description><pubDate>Tue, 17 Nov 2026 12:00:00 GMT</pubDate>
    <itunes:duration>32:15</itunes:duration><enclosure url="/episode.mp3" type="audio/mpeg" length="100"/></item>
    <item><title>Duplicate</title><enclosure url="/episode.mp3" type="audio/mpeg" length="100"/></item>
    <item><title>Video</title><enclosure url="https://example.com/video.mp4" type="video/mp4" length="100"/></item>
    <item><title>Local</title><enclosure url="file:///etc/passwd" type="audio/mpeg" length="100"/></item>
    <item><title>No audio</title></item>
    </channel></rss>"#;

    fn fixture() -> Feed {
        parse_feed(
            RSS.as_bytes(),
            &http_url("https://example.com/feed").unwrap(),
            "https://example.com/feed",
        )
        .unwrap()
    }
    #[test]
    fn sorting_uses_real_dates_and_keeps_the_selected_episode() {
        let root = tempfile::tempdir().unwrap();
        let mut state = PodcastState::at(root.path().to_path_buf());
        let mut feed = fixture();
        let template = feed.episodes[0].clone();
        feed.episodes = [
            ("Alpha", "Tue, 17 Nov 2026 14:00:00 +0000"),
            ("Zulu", "Tue, 17 Nov 2026 10:00:00 -0500"),
            ("Beta", "Mon, 16 Nov 2026 23:00:00 -0800"),
            ("Undated", "unknown"),
        ]
        .into_iter()
        .map(|(title, date)| Episode {
            title: title.into(),
            date: date.into(),
            url: format!("https://example.com/{title}.mp3"),
            ..template.clone()
        })
        .collect();
        state.feed = Some(feed);
        state.focus = PodcastPane::Episodes;
        let titles = |s: &PodcastState| {
            s.feed
                .as_ref()
                .unwrap()
                .episodes
                .iter()
                .map(|e| e.title.clone())
                .collect::<Vec<_>>()
        };
        let selected = |s: &PodcastState| {
            s.feed.as_ref().unwrap().episodes[s.episode_index]
                .title
                .clone()
        };
        state.sort_episodes();
        assert_eq!(titles(&state), ["Zulu", "Alpha", "Beta", "Undated"]);
        assert_eq!(selected(&state), "Alpha");
        state.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert_eq!(titles(&state), ["Beta", "Alpha", "Zulu", "Undated"]);
        assert_eq!(selected(&state), "Alpha");
        state.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert_eq!(titles(&state), ["Alpha", "Beta", "Undated", "Zulu"]);
        state.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert_eq!(titles(&state), ["Zulu", "Undated", "Beta", "Alpha"]);
        assert_eq!(selected(&state), "Alpha");
    }

    #[test]
    fn saved_notice_expires_without_waiting_or_clearing_other_status() {
        let root = tempfile::tempdir().unwrap();
        let mut state = PodcastState::at(root.path().to_path_buf());
        state.message = "Ready".into();
        state.notice = Some((
            "Saved episode.mp3".into(),
            Instant::now() + Duration::from_secs(4),
        ));
        assert_eq!(state.progress_text(), "Saved episode.mp3");
        state.notice.as_mut().unwrap().1 = Instant::now() - Duration::from_millis(1);
        assert_eq!(state.progress_text(), "Ready");
        state.poll();
        assert!(state.notice.is_none());
    }

    #[test]
    fn subscription_pane_navigates_independently_of_search_results() {
        let root = tempfile::tempdir().unwrap();
        let mut state = PodcastState::at(root.path().to_path_buf());
        let show = fixture().show;
        state.shows = vec![show.clone()];
        state.search_results = true;
        state.subscriptions = vec![
            show.clone(),
            Show {
                title: "Second".into(),
                url: "https://example.com/second".into(),
                ..show
            },
        ];
        state.focus = PodcastPane::Discover;
        state.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.focus, PodcastPane::Subscriptions);
        state.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.subscription_index, 1);
        assert_eq!(state.show_index, 0);
        assert_eq!(state.selected_show().unwrap().title, "Second");
        state.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.focus, PodcastPane::Episodes);
        state.key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE));
        assert_eq!(state.focus, PodcastPane::Subscriptions);
        assert!(state.search_results);
        assert_eq!(state.shows.len(), 1);
    }

    #[test]
    fn reads_real_podcast_fields_and_only_valid_audio_enclosures() {
        let feed = fixture();
        assert_eq!(feed.show.title, "Test & Show");
        assert_eq!(feed.show.author, "Author");
        assert_eq!(feed.episodes.len(), 1);
        assert_eq!(feed.episodes[0].url, "https://example.com/episode.mp3");
        assert_eq!(feed.episodes[0].duration, "32:15");
        assert_eq!(feed.episodes[0].description, "Hello & welcome");
        assert!(!feed.description.contains('<'));
        assert!(parse_feed(b"not XML", &http_url("https://example.com").unwrap(), "").is_err());
    }
    #[test]
    fn search_skips_invalid_and_duplicate_feeds() {
        let shows = parse_search(
            br#"{"results":[
            {"collectionName":"A","artistName":"Author","feedUrl":"https://example.com/rss"},
            {"collectionName":"Duplicate","feedUrl":"https://example.com/rss"},
            {"collectionName":"No feed"}, {"feedUrl":"file:///tmp/secret"}] }"#,
        )
        .unwrap();
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].title, "A");
        assert!(http_url("https://example.com/feed").is_ok());
        assert!(http_url("javascript:alert(1)").is_err());
    }
    #[test]
    fn subscriptions_persist_and_url_input_can_be_pasted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Podcasts");
        let mut state = PodcastState::at(root.clone());
        state.open();
        assert!(root.is_dir());
        state.shows = vec![fixture().show];
        state.search_results = true;
        state.focus = PodcastPane::Discover;
        state.key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        let mut restored = PodcastState::at(root.clone());
        assert_eq!(restored.subscriptions.len(), 1);
        restored.key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(PodcastState::at(root).subscriptions.is_empty());
        state.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        state.paste("https://example.com/feed\n");
        assert_eq!(state.input, "https://example.com/feed");
        state.edit_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(state.prompt.is_none());
        assert!(state.input.is_empty());
    }
    #[test]
    fn paths_cannot_escape_storage_and_different_shows_do_not_collide() {
        let feed = fixture();
        let root = Path::new("/tmp/podcasts");
        let mut episode = feed.episodes[0].clone();
        episode.title = "../../outside/episode".into();
        let path = episode_path(root, &feed.show, &episode);
        assert_eq!(path.parent().unwrap().parent().unwrap(), root);
        assert!(!path.to_string_lossy().contains(".."));
        let mut other = feed.show.clone();
        other.url.push_str("?different");
        assert_ne!(path, episode_path(root, &other, &episode));
    }
    fn serve(body: Vec<u8>, truncated: bool) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/audio.mp3", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let received = stream.read(&mut request).unwrap();
            assert!(received > 0);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() + if truncated { 20 } else { 0 }).unwrap();
            stream.write_all(&body).unwrap();
        });
        (url, handle)
    }
    #[test]
    fn stop_cancels_download_autoplay_but_keeps_the_download() {
        let root = tempfile::tempdir().unwrap();
        let (url, server) = serve(
            include_bytes!("../tests/fixtures/artwork.mp3").to_vec(),
            false,
        );
        let mut state = PodcastState::at(root.path().to_path_buf());
        let mut feed = fixture();
        feed.episodes[0].url = url;
        let saved = episode_path(root.path(), &feed.show, &feed.episodes[0]);
        state.feed = Some(feed);
        state.focus = PodcastPane::Episodes;
        state.play_selected();
        state.cancel_pending_playback();
        let started = Instant::now();
        while state.busy && started.elapsed() < Duration::from_secs(5) {
            state.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        server.join().unwrap();
        assert!(saved.is_file());
        assert!(state.play_ready.is_none());
    }

    #[test]
    fn next_and_previous_follow_the_playing_episode_not_the_browsed_selection() {
        let root = tempfile::tempdir().unwrap();
        let mut state = PodcastState::at(root.path().to_path_buf());
        let mut feed = fixture();
        let template = feed.episodes[0].clone();
        feed.episodes = (0..3)
            .map(|i| Episode {
                title: format!("Episode {i}"),
                url: format!("https://example.com/{i}.mp3"),
                ..template.clone()
            })
            .collect();
        let paths: Vec<_> = feed
            .episodes
            .iter()
            .map(|e| episode_path(root.path(), &feed.show, e))
            .collect();
        std::fs::create_dir_all(paths[0].parent().unwrap()).unwrap();
        for path in &paths {
            std::fs::write(path, b"cached").unwrap();
        }
        state.feed = Some(feed);
        state.episode_index = 2;
        state.step_episode(1, Some(&paths[0]));
        assert_eq!(state.episode_index, 1);
        assert_eq!(state.play_ready.take(), Some(paths[1].clone()));
        state.step_episode(-1, Some(&paths[1]));
        assert_eq!(state.play_ready.take(), Some(paths[0].clone()));
        state.cancel_pending_playback();
        state.step_episode(-1, Some(&paths[0]));
        assert!(state.play_ready.is_none());
    }

    #[test]
    fn adding_a_feed_url_subscribes_after_validation() {
        let root = tempfile::tempdir().unwrap();
        let (url, server) = serve(RSS.as_bytes().to_vec(), false);
        let mut state = PodcastState::at(root.path().to_path_buf());
        state.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        state.paste(&url);
        state.edit_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let started = std::time::Instant::now();
        while state.busy && started.elapsed() < Duration::from_secs(5) {
            state.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!state.busy);
        assert_eq!(state.subscriptions.len(), 1, "{}", state.message);
        assert_eq!(state.subscriptions[0].url, url);
        assert_eq!(state.focus, PodcastPane::Episodes);
        assert_eq!(state.feed.as_ref().unwrap().episodes.len(), 1);
        server.join().unwrap();
        assert!(root.path().join("subscriptions.json").is_file());
    }

    #[test]
    fn downloads_atomically_reuses_files_and_retains_audio_bytes() {
        let root = tempfile::tempdir().unwrap();
        let bytes = include_bytes!("../tests/fixtures/artwork.mp3").to_vec();
        let (url, server) = serve(bytes.clone(), false);
        let feed = fixture();
        let mut episode = feed.episodes[0].clone();
        episode.url = url;
        let progress = AtomicU64::new(0);
        let total = AtomicU64::new(0);
        let saved = download(root.path(), &feed.show, &episode, &progress, &total).unwrap();
        server.join().unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        assert_eq!(progress.load(Ordering::Relaxed), bytes.len() as u64);
        // Server is gone: this must reuse the saved episode without a network request.
        assert_eq!(
            download(root.path(), &feed.show, &episode, &progress, &total).unwrap(),
            saved
        );
    }
    #[test]
    fn failed_download_never_leaves_a_finished_episode() {
        let root = tempfile::tempdir().unwrap();
        let (url, server) = serve(vec![1; 100], true);
        let feed = fixture();
        let mut episode = feed.episodes[0].clone();
        episode.url = url;
        assert!(
            download(
                root.path(),
                &feed.show,
                &episode,
                &AtomicU64::new(0),
                &AtomicU64::new(0)
            )
            .is_err()
        );
        server.join().unwrap();
        assert!(!episode_path(root.path(), &feed.show, &episode).exists());
    }
    #[test]
    #[ignore = "requires the public podcast directory and publisher feed"]
    fn live_search_and_feed() {
        let shows = search("Planet Money").unwrap();
        let show = shows.iter().find(|s| s.title == "Planet Money").unwrap();
        let root = tempfile::tempdir().unwrap();
        let feed = load_feed(root.path(), &show.url).unwrap();
        assert!(!feed.episodes.is_empty());
        eprintln!(
            "{}: {} audio episodes",
            feed.show.title,
            feed.episodes.len()
        );
    }

    #[test]
    fn cached_feeds_are_available_offline() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/feed", listener.local_addr().unwrap());
        drop(listener);
        let mut feed = fixture();
        feed.show.url = url.clone();
        let cache = root
            .path()
            .join(".feeds")
            .join(format!("{}.json", digest(&url)));
        save_json(&cache, &feed).unwrap();
        let restored = load_feed(root.path(), &url).unwrap();
        assert_eq!(restored.episodes[0].title, "Episode One");
    }
}
