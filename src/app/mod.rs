use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::audio::AudioEngine;
use crate::browser::{BrowserState, PaneType};
use crate::collections::Collections;
use crate::config::{AppConfig, resolve_path};
use crate::events::Event;
use crate::models::{PlaybackStatus, RepeatMode, VisualizerMode};
use crate::queue::PlaybackQueue;
use crate::search::{SearchState, matches_audio_extension, matches_image_extension, matches_text_extension};
use ratatui::widgets::ListState;
use lofty::prelude::*;
use lofty::probe::Probe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppScreen {
    Browser,
    Queue,
    Collections,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    CreateCollection,
    AddToCollectionList,
    CopyPath,
    MovePath,
    ConfirmDelete,
    Rename,
}

#[derive(Debug, Clone)]
pub struct FileOperationProgress {
    pub op_type: String, // "Copying" or "Moving"
    pub current_file: String,
    pub completed_files: usize,
    pub total_files: usize,
    pub finished: bool,
    pub error: Option<String>,
    pub canceled: bool,
}

pub struct App {
    pub browser: BrowserState,
    pub audio: AudioEngine,
    pub queue: PlaybackQueue,
    pub collections: Collections,
    pub search: SearchState,
    pub screen: AppScreen,
    pub input_mode: InputMode,
    pub input_value: String,
    pub show_help: bool,
    pub should_quit: bool,
    
    // UI Navigation indices
    pub selected_collection_index: usize,
    pub active_collection_file_index: usize,
    pub selected_add_collection_index: usize,
    pub queue_selected_index: usize,
    
    // Pane focus inside collections screen
    pub collections_focused_pane: PaneType, // Directories = left (colls list), Files = right (coll files list)
    pub file_progress: Arc<Mutex<Option<FileOperationProgress>>>,
    
    // Persistent UI scrolling states
    pub dirs_list_state: ListState,
    pub files_list_state: ListState,
    pub queue_list_state: ListState,
    pub colls_list_state: ListState,
    pub coll_files_list_state: ListState,
    pub add_coll_list_state: ListState,
    
    // Media Controls Integration (MPRIS / Playerctl)
    pub media_controls: Option<souvlaki::MediaControls>,
    pub last_media_status: Option<PlaybackStatus>,
    pub last_media_track: Option<PathBuf>,
    pub last_media_elapsed: u64,
    pub rename_target: Option<PathBuf>,
    pub image_zoom: f64,
    pub image_offset_x: i32,
    pub image_offset_y: i32,
    pub last_previewed_file: Option<PathBuf>,
    pub current_image_data: Option<(PathBuf, image::DynamicImage)>,
    pub text_scroll_offset: usize,
    pub current_text_data: Option<(PathBuf, Vec<String>)>,
    pub config: AppConfig,
    pub last_operation_dest: String,
}

impl App {
    pub fn open_file_with_default_app(path: &Path) {
        let mut cmd = if let Ok(sudo_user) = std::env::var("SUDO_USER") {
            if !sudo_user.is_empty() {
                let mut c = std::process::Command::new("sudo");
                c.arg("-u").arg(&sudo_user).arg("xdg-open").arg(path);
                // Forward GUI and audio environments
                for var in &["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"] {
                    if let Ok(val) = std::env::var(var) {
                        c.env(var, val);
                    }
                }
                c
            } else {
                let mut c = std::process::Command::new("xdg-open");
                c.arg(path);
                c
            }
        } else {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(path);
            c
        };

        let _ = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    pub fn new(event_tx: std::sync::mpsc::Sender<Event>, initial_path: Option<PathBuf>) -> Self {
        let config = AppConfig::load();
        
        // Resolve initial directory: command-line argument, config music_folders, or current dir
        let starting_dir = if let Some(ref path) = initial_path {
            let resolved = resolve_path(&path.to_string_lossy());
            if resolved.exists() {
                resolved
            } else if !config.music_folders.is_empty() {
                let p = resolve_path(&config.music_folders[0]);
                if p.exists() { p } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) }
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            }
        } else if !config.music_folders.is_empty() {
            let path = resolve_path(&config.music_folders[0]);
            if path.exists() {
                path
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            }
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        let browser = BrowserState::new(starting_dir, config.show_hidden);
        let audio = AudioEngine::new(event_tx.clone(), config.default_volume, config.repeat, config.shuffle, config.visualizer_decay);
        let queue = PlaybackQueue::new();
        let collections = Collections::load();
        let search = SearchState::new();

        let media_controls = {
            #[cfg(not(target_os = "windows"))]
            let hwnd = None;
            #[cfg(target_os = "windows")]
            let hwnd = None;

            let platform_config = souvlaki::PlatformConfig {
                dbus_name: "stash",
                display_name: "STASH Music Player",
                hwnd,
            };

            if let Ok(mut controls) = souvlaki::MediaControls::new(platform_config) {
                let tx_clone = event_tx.clone();
                let _ = controls.attach(move |event| {
                    match event {
                        souvlaki::MediaControlEvent::Play | souvlaki::MediaControlEvent::Pause | souvlaki::MediaControlEvent::Toggle => {
                            let _ = tx_clone.send(Event::MediaPlayPause);
                        }
                        souvlaki::MediaControlEvent::Next => {
                            let _ = tx_clone.send(Event::MediaNext);
                        }
                        souvlaki::MediaControlEvent::Previous => {
                            let _ = tx_clone.send(Event::MediaPrev);
                        }
                        souvlaki::MediaControlEvent::Seek(direction) => {
                            let _ = tx_clone.send(Event::MediaSeek(direction, std::time::Duration::from_secs(5)));
                        }
                        souvlaki::MediaControlEvent::SeekBy(direction, duration) => {
                            let _ = tx_clone.send(Event::MediaSeek(direction, duration));
                        }
                        souvlaki::MediaControlEvent::SetPosition(position) => {
                            let _ = tx_clone.send(Event::MediaSetPosition(position.0));
                        }
                        _ => {}
                    }
                });
                Some(controls)
            } else {
                None
            }
        };

        Self {
            browser,
            audio,
            queue,
            collections,
            search,
            screen: AppScreen::Browser,
            input_mode: InputMode::Normal,
            input_value: String::new(),
            show_help: false,
            should_quit: false,
            selected_collection_index: 0,
            active_collection_file_index: 0,
            selected_add_collection_index: 0,
            queue_selected_index: 0,
            collections_focused_pane: PaneType::Directories,
            file_progress: Arc::new(Mutex::new(None)),
            dirs_list_state: ListState::default(),
            files_list_state: ListState::default(),
            queue_list_state: ListState::default(),
            colls_list_state: ListState::default(),
            coll_files_list_state: ListState::default(),
            add_coll_list_state: ListState::default(),
            media_controls,
            last_media_status: None,
            last_media_track: None,
            last_media_elapsed: 0,
            rename_target: None,
            image_zoom: 1.0,
            image_offset_x: 0,
            image_offset_y: 0,
            last_previewed_file: None,
            current_image_data: None,
            text_scroll_offset: 0,
            current_text_data: None,
            config,
            last_operation_dest: String::new(),
        }
    }

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Tick => {
                let mut should_refresh = false;
                {
                    let p = self.file_progress.lock().unwrap();
                    if let Some(ref state) = *p {
                        if state.finished && state.error.is_none() && !state.canceled {
                            should_refresh = true;
                        }
                    }
                }
                if should_refresh {
                    *self.file_progress.lock().unwrap() = None;
                    self.browser.clear_selections();
                    self.browser.refresh();
                }
                let current_preview_path = if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                    let path = &self.browser.files[self.browser.file_index].path;
                    if matches_image_extension(path) || matches_text_extension(path) {
                        Some(path.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };

                if current_preview_path != self.last_previewed_file {
                    self.last_previewed_file = current_preview_path;
                    self.image_zoom = 1.0;
                    self.image_offset_x = 0;
                    self.image_offset_y = 0;
                    self.text_scroll_offset = 0;
                }

                self.update_media_controls();
            }
            Event::AudioFinished => {
                self.handle_audio_finished();
            }
            Event::MediaPlayPause => {
                self.toggle_playback();
            }
            Event::MediaNext => {
                let shuffle = {
                    let state = self.audio.shared_state.lock().unwrap();
                    state.shuffle
                };
                if let Some(next_path) = self.queue.next(shuffle) {
                    self.audio.play(next_path);
                    self.sync_queue_selection();
                }
            }
            Event::MediaPrev => {
                let shuffle = {
                    let state = self.audio.shared_state.lock().unwrap();
                    state.shuffle
                };
                if let Some(prev_path) = self.queue.prev(shuffle) {
                    self.audio.play(prev_path);
                    self.sync_queue_selection();
                }
            }
            Event::MediaSeek(direction, duration) => {
                let seconds = duration.as_secs() as i64;
                let sign = match direction {
                    souvlaki::SeekDirection::Forward => 1,
                    souvlaki::SeekDirection::Backward => -1,
                };
                self.seek_relative(seconds * sign);
            }
            Event::MediaSetPosition(duration) => {
                self.audio.seek(duration);
            }
        }
    }

    fn handle_audio_finished(&mut self) {
        let (repeat, shuffle) = {
            let state = self.audio.shared_state.lock().unwrap();
            (state.repeat, state.shuffle)
        };

        match repeat {
            RepeatMode::One => {
                // Play current track again
                if let Some(path) = self.queue.current_track() {
                    self.audio.play(path);
                }
            }
            RepeatMode::All => {
                // Move to next track, if we reach the end, loop back to the beginning!
                if let Some(next_path) = self.queue.next(shuffle) {
                    self.audio.play(next_path);
                    self.sync_queue_selection();
                } else {
                    // Reached the end of the queue, loop back to the beginning of the queue!
                    if !self.queue.items.is_empty() {
                        self.queue.current_index = None;
                        if let Some(next_path) = self.queue.next(shuffle) {
                            self.audio.play(next_path);
                            self.sync_queue_selection();
                        }
                    }
                }
            }
            RepeatMode::Off => {
                // Move to next track in queue, stop if we reach the end
                if let Some(next_path) = self.queue.next(shuffle) {
                    self.audio.play(next_path);
                    self.sync_queue_selection();
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Global exit hook
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // Intercept keys if a file operation is active
        if self.is_file_operation_active() {
            let mut dismiss = false;
            {
                let mut p = self.file_progress.lock().unwrap();
                if let Some(ref mut state) = *p {
                    if state.finished || state.error.is_some() || state.canceled {
                        if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
                            dismiss = true;
                        }
                    } else if key.code == KeyCode::Esc {
                        state.canceled = true;
                        state.current_file = "Canceling...".to_string();
                    }
                }
            }
            if dismiss {
                *self.file_progress.lock().unwrap() = None;
                self.browser.clear_selections();
                self.browser.refresh();
            }
            return;
        }

        if self.show_help {
            self.show_help = false;
            return;
        }

        match self.input_mode {
            InputMode::Normal => self.handle_normal_key(key),
            InputMode::Search => self.handle_search_key(key),
            InputMode::CreateCollection => self.handle_create_collection_key(key),
            InputMode::AddToCollectionList => self.handle_add_to_collection_list_key(key),
            InputMode::CopyPath => self.handle_copy_path_key(key),
            InputMode::MovePath => self.handle_move_path_key(key),
            InputMode::ConfirmDelete => self.handle_confirm_delete_key(key),
            InputMode::Rename => self.handle_rename_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        if self.screen == AppScreen::Browser && self.browser.focused_pane == PaneType::Preview {
            let is_txt = if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                matches_text_extension(&self.browser.files[self.browser.file_index].path)
            } else {
                false
            };

            if is_txt {
                let file_path = self.browser.files[self.browser.file_index].path.clone();
                let needs_load = match &self.current_text_data {
                    Some((path, _)) => *path != file_path,
                    None => true,
                };
                if needs_load {
                    if let Ok(content) = std::fs::read_to_string(&file_path) {
                        let parsed_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                        self.current_text_data = Some((file_path, parsed_lines));
                    }
                }

                match key.code {
                    KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') => {
                        self.browser.focused_pane = PaneType::Files;
                        return;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.text_scroll_offset = self.text_scroll_offset.saturating_sub(1);
                        return;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some((_, ref lines)) = self.current_text_data {
                            if self.text_scroll_offset + 1 < lines.len() {
                                self.text_scroll_offset += 1;
                            }
                        }
                        return;
                    }
                    KeyCode::PageUp => {
                        self.text_scroll_offset = self.text_scroll_offset.saturating_sub(10);
                        return;
                    }
                    KeyCode::PageDown => {
                        if let Some((_, ref lines)) = self.current_text_data {
                            self.text_scroll_offset = (self.text_scroll_offset + 10).min(lines.len().saturating_sub(1));
                        }
                        return;
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') => {
                        self.browser.focused_pane = PaneType::Files;
                        return;
                    }
                    KeyCode::Up => {
                        self.image_offset_y = self.image_offset_y.saturating_sub(10);
                        return;
                    }
                    KeyCode::Down => {
                        self.image_offset_y = self.image_offset_y.saturating_add(10);
                        return;
                    }
                    KeyCode::Left => {
                        self.image_offset_x = self.image_offset_x.saturating_sub(10);
                        return;
                    }
                    KeyCode::Right => {
                        self.image_offset_x = self.image_offset_x.saturating_add(10);
                        return;
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        self.image_zoom = (self.image_zoom + 0.25).min(10.0);
                        return;
                    }
                    KeyCode::Char('-') => {
                        self.image_zoom = (self.image_zoom - 0.25).max(1.0);
                        return;
                    }
                    _ => {}
                }
            }
        }

        match key.code {
            KeyCode::Char('?') => {
                self.show_help = true;
            }
            KeyCode::Char('Q') => {
                if self.screen == AppScreen::Queue {
                    self.screen = AppScreen::Browser;
                    self.search.active = false;
                    self.search.query.clear();
                } else {
                    self.screen = AppScreen::Queue;
                    self.search.active = false;
                    self.search.query.clear();
                    self.queue_selected_index = 0;
                    self.sync_queue_selection();
                }
            }
            KeyCode::Char('C') => {
                if self.screen == AppScreen::Queue {
                    self.queue.clear();
                    self.audio.stop();
                    self.queue_selected_index = 0;
                } else if self.screen == AppScreen::Collections {
                    self.screen = AppScreen::Browser;
                } else {
                    self.screen = AppScreen::Collections;
                    self.search.active = false;
                    self.search.query.clear();
                    self.selected_collection_index = 0;
                    self.active_collection_file_index = 0;
                    self.collections_focused_pane = PaneType::Directories;
                }
            }
            KeyCode::Esc => {
                self.screen = AppScreen::Browser;
                self.search.active = false;
                self.search.query.clear();
            }
            KeyCode::Up => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && self.screen == AppScreen::Queue {
                    self.move_highlighted_queue_item(true);
                } else {
                    self.navigate_up();
                }
            }
            KeyCode::Char('k') => self.navigate_up(),
            KeyCode::Char('K') => {
                if self.screen == AppScreen::Queue {
                    self.move_highlighted_queue_item(true);
                }
            }
            KeyCode::Down => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && self.screen == AppScreen::Queue {
                    self.move_highlighted_queue_item(false);
                } else {
                    self.navigate_down();
                }
            }
            KeyCode::Char('j') => self.navigate_down(),
            KeyCode::Char('J') => {
                if self.screen == AppScreen::Queue {
                    self.move_highlighted_queue_item(false);
                }
            }
            KeyCode::PageUp => self.navigate_page_up(),
            KeyCode::PageDown => self.navigate_page_down(),
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.seek_relative(-5);
                } else {
                    self.navigate_left();
                }
            }
            KeyCode::Char('h') => self.navigate_left(),
            KeyCode::Char('H') => self.seek_relative(-5),
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.seek_relative(5);
                } else {
                    self.navigate_right();
                }
            }
            KeyCode::Char('l') => self.navigate_right(),
            KeyCode::Char('L') => self.seek_relative(5),
            KeyCode::Enter => self.activate_item(),
            KeyCode::Backspace => self.navigate_back(),
            
            // Selection & Editing
            KeyCode::Char(' ') => {
                if self.screen == AppScreen::Browser && (self.browser.focused_pane == PaneType::Files || self.browser.focused_pane == PaneType::Directories) {
                    self.browser.toggle_select_highlighted();
                } else {
                    // Space acts as pause/resume on other screens/panes
                    self.toggle_playback();
                }
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                self.toggle_playback();
            }
            KeyCode::Char('n') => {
                let shuffle = {
                    let state = self.audio.shared_state.lock().unwrap();
                    state.shuffle
                };
                if let Some(next_path) = self.queue.next(shuffle) {
                    self.audio.play(next_path);
                    self.sync_queue_selection();
                }
            }
            KeyCode::Char('b') => {
                let shuffle = {
                    let state = self.audio.shared_state.lock().unwrap();
                    state.shuffle
                };
                if let Some(prev_path) = self.queue.prev(shuffle) {
                    self.audio.play(prev_path);
                    self.sync_queue_selection();
                }
            }
            KeyCode::Char('s') => {
                self.audio.stop();
                self.queue.current_index = None;
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let current_vol = {
                    let state = self.audio.shared_state.lock().unwrap();
                    state.volume
                };
                let new_vol = current_vol.saturating_add(5).min(100);
                self.audio.set_volume(new_vol);
                self.config.default_volume = new_vol;
                let _ = self.config.save();
            }
            KeyCode::Char('-') => {
                let current_vol = {
                    let state = self.audio.shared_state.lock().unwrap();
                    state.volume
                };
                let new_vol = current_vol.saturating_sub(5);
                self.audio.set_volume(new_vol);
                self.config.default_volume = new_vol;
                let _ = self.config.save();
            }
            KeyCode::Char('r') => {
                let new_repeat = {
                    let mut state = self.audio.shared_state.lock().unwrap();
                    state.repeat = match state.repeat {
                        RepeatMode::Off => RepeatMode::All,
                        RepeatMode::All => RepeatMode::One,
                        RepeatMode::One => RepeatMode::Off,
                    };
                    state.repeat
                };
                self.config.repeat = new_repeat;
                let _ = self.config.save();
            }
            KeyCode::Char('z') => {
                let next_shuffle = {
                    let mut state = self.audio.shared_state.lock().unwrap();
                    state.shuffle = !state.shuffle;
                    state.shuffle
                };
                if next_shuffle {
                    self.queue.shuffle_items();
                }
                self.sync_queue_selection();
                self.config.shuffle = next_shuffle;
                let _ = self.config.save();
            }
            
            // Queue & Collections triggers
            KeyCode::Char('q') => {
                // Add selected files to queue
                if !self.browser.selected_paths.is_empty() {
                    let paths: Vec<PathBuf> = self.browser.selected_paths.iter().cloned().collect();
                    self.queue.add_many(paths);
                    self.browser.clear_selections();
                } else if self.screen == AppScreen::Browser {
                    match self.browser.focused_pane {
                        PaneType::Directories => {
                            if !self.browser.directories.is_empty() {
                                let dir_path = self.browser.directories[self.browser.dir_index].clone();
                                if !dir_path.ends_with("..") {
                                    let walk_path = if dir_path.ends_with(".") {
                                        self.browser.current_dir.clone()
                                    } else {
                                        dir_path
                                    };
                                    let mut paths = Vec::new();
                                    for entry in walkdir::WalkDir::new(&walk_path)
                                        .into_iter()
                                        .filter_entry(|e| {
                                            let name = e.file_name().to_string_lossy();
                                            !name.starts_with('.')
                                        })
                                        .flatten()
                                    {
                                        let p = entry.path();
                                        if p.is_file() && matches_audio_extension(p) {
                                            paths.push(p.to_path_buf());
                                        }
                                    }
                                    if !paths.is_empty() {
                                        paths.sort();
                                        self.queue.add_many(paths);
                                        
                                        // Shuffle queue if shuffle is active
                                        let shuffle = {
                                            let state = self.audio.shared_state.lock().unwrap();
                                            state.shuffle
                                        };
                                        if shuffle {
                                            self.queue.shuffle_items();
                                        }
                                        self.sync_queue_selection();
                                    }
                                }
                            }
                        }
                        PaneType::Files => {
                            if !self.browser.files.is_empty() {
                                let path = self.browser.files[self.browser.file_index].path.clone();
                                if matches_audio_extension(&path) {
                                    self.queue.add(path);
                                }
                            }
                        }
                        PaneType::Preview => {}
                    }
                }
            }
            KeyCode::Char('c') => {
                self.input_mode = InputMode::CreateCollection;
                self.input_value.clear();
            }
            KeyCode::Char('a') => {
                if !self.browser.selected_paths.is_empty() {
                    self.input_mode = InputMode::AddToCollectionList;
                    self.selected_add_collection_index = 0;
                }
            }
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Search;
                self.search.active = true;
                self.search.query.clear();
            }
            KeyCode::Char('v') => {
                if self.screen == AppScreen::Queue {
                    self.config.visualizer_mode = match self.config.visualizer_mode {
                        VisualizerMode::Spectrum => VisualizerMode::Waveform,
                        VisualizerMode::Waveform => VisualizerMode::SignalLevels,
                        VisualizerMode::SignalLevels => VisualizerMode::Spectrum,
                    };
                    let _ = self.config.save();
                } else {
                    if !self.browser.selected_paths.is_empty() {
                        self.input_mode = InputMode::CopyPath;
                        self.input_value = self.last_operation_dest.clone();
                    }
                }
            }
            KeyCode::Char('y') => {
                if !self.browser.selected_paths.is_empty() {
                    self.input_mode = InputMode::MovePath;
                    self.input_value = self.last_operation_dest.clone();
                }
            }
            KeyCode::Char('d') => {
                if self.screen == AppScreen::Queue {
                    self.delete_highlighted_queue_item();
                } else {
                    if !self.browser.selected_paths.is_empty() {
                        self.input_mode = InputMode::ConfirmDelete;
                    }
                }
            }
            KeyCode::Char('x') => {
                if self.screen == AppScreen::Queue {
                    self.delete_highlighted_queue_item();
                }
            }
            KeyCode::Char('[') => {
                if self.screen == AppScreen::Queue {
                    let mut current_decay = self.config.visualizer_decay;
                    current_decay = (current_decay - 0.05).max(0.10);
                    self.config.visualizer_decay = current_decay;
                    if let Ok(mut state) = self.audio.shared_state.lock() {
                        state.visualizer_decay = current_decay;
                    }
                    let _ = self.config.save();
                }
            }
            KeyCode::Char(']') => {
                if self.screen == AppScreen::Queue {
                    let mut current_decay = self.config.visualizer_decay;
                    current_decay = (current_decay + 0.05).min(0.95);
                    self.config.visualizer_decay = current_decay;
                    if let Ok(mut state) = self.audio.shared_state.lock() {
                        state.visualizer_decay = current_decay;
                    }
                    let _ = self.config.save();
                }
            }
            KeyCode::F(2) => {
                if self.screen == AppScreen::Browser {
                    match self.browser.focused_pane {
                        PaneType::Directories => {
                            if !self.browser.directories.is_empty() && self.browser.dir_index < self.browser.directories.len() {
                                // Prevent renaming "." and ".."
                                let is_current_or_parent = self.browser.dir_index == 0 || 
                                    (self.browser.dir_index == 1 && self.browser.directories.len() > 1 && self.browser.directories[1].ends_with(".."));
                                if !is_current_or_parent {
                                    let target_path = self.browser.directories[self.browser.dir_index].clone();
                                    let name = target_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                    self.input_value = name;
                                    self.rename_target = Some(target_path);
                                    self.input_mode = InputMode::Rename;
                                }
                            }
                        }
                        PaneType::Files => {
                            if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                                let target_path = self.browser.files[self.browser.file_index].path.clone();
                                let name = target_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                self.input_value = name;
                                self.rename_target = Some(target_path);
                                self.input_mode = InputMode::Rename;
                            }
                        }
                        PaneType::Preview => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                // If query is empty, cancel search state
                if self.search.query.trim().is_empty() {
                    self.search.active = false;
                }
            }
            KeyCode::Backspace => {
                self.search.query.pop();
                if self.screen == AppScreen::Browser {
                    self.search.execute(&[self.browser.current_dir.clone()]);
                } else if self.screen == AppScreen::Queue {
                    self.queue_selected_index = 0;
                }
            }
            KeyCode::Char(c) => {
                self.search.query.push(c);
                if self.screen == AppScreen::Browser {
                    self.search.execute(&[self.browser.current_dir.clone()]);
                } else if self.screen == AppScreen::Queue {
                    self.queue_selected_index = 0;
                }
            }
            _ => {}
        }
    }

    fn handle_create_collection_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                if !self.input_value.trim().is_empty() {
                    self.collections.create_collection(&self.input_value);
                }
                self.input_mode = InputMode::Normal;
                self.input_value.clear();
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.input_value.clear();
            }
            KeyCode::Backspace => {
                self.input_value.pop();
            }
            KeyCode::Char(c) => {
                self.input_value.push(c);
            }
            _ => {}
        }
    }

    fn handle_rename_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                if let Some(target) = self.rename_target.take() {
                    let new_name = self.input_value.trim();
                    if !new_name.is_empty() {
                        let mut new_path = target.clone();
                        new_path.set_file_name(new_name);
                        
                        if let Err(_e) = std::fs::rename(&target, &new_path) {
                            // Fail silently or handle error
                        } else {
                            if self.browser.selected_paths.contains(&target) {
                                self.browser.selected_paths.remove(&target);
                                self.browser.selected_paths.insert(new_path.clone());
                            }
                            
                            self.browser.refresh();
                            
                            if self.browser.focused_pane == PaneType::Directories {
                                if let Some(pos) = self.browser.directories.iter().position(|d| d == &new_path) {
                                    self.browser.dir_index = pos;
                                    self.browser.refresh();
                                }
                            } else {
                                if let Some(pos) = self.browser.files.iter().position(|f| f.path == new_path) {
                                    self.browser.file_index = pos;
                                }
                            }
                        }
                    }
                }
                self.input_mode = InputMode::Normal;
                self.input_value.clear();
            }
            KeyCode::Esc => {
                self.rename_target = None;
                self.input_mode = InputMode::Normal;
                self.input_value.clear();
            }
            KeyCode::Backspace => {
                self.input_value.pop();
            }
            KeyCode::Char(c) => {
                self.input_value.push(c);
            }
            _ => {}
        }
    }

    fn handle_add_to_collection_list_key(&mut self, key: KeyEvent) {
        let coll_names: Vec<&String> = self.collections.collections.keys().collect();
        match key.code {
            KeyCode::Up => {
                if !coll_names.is_empty() && self.selected_add_collection_index > 0 {
                    self.selected_add_collection_index -= 1;
                }
            }
            KeyCode::Down => {
                if !coll_names.is_empty() && self.selected_add_collection_index + 1 < coll_names.len() {
                    self.selected_add_collection_index += 1;
                }
            }
            KeyCode::Enter => {
                if !coll_names.is_empty() && self.selected_add_collection_index < coll_names.len() {
                    let name = coll_names[self.selected_add_collection_index].clone();
                    let paths: Vec<PathBuf> = self.browser.selected_paths.iter().cloned().collect();
                    self.collections.add_to_collection(&name, paths);
                    self.browser.clear_selections();
                }
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_copy_path_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let resolved = resolve_path(&self.input_value);
                self.last_operation_dest = self.input_value.clone();
                self.start_file_operation(resolved, false);
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Tab => {
                if let Some(completed) = Self::autocomplete_path(&self.input_value) {
                    self.input_value = completed;
                }
            }
            KeyCode::Backspace => {
                self.input_value.pop();
            }
            KeyCode::Char(c) => {
                self.input_value.push(c);
            }
            _ => {}
        }
    }

    fn handle_move_path_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let resolved = resolve_path(&self.input_value);
                self.last_operation_dest = self.input_value.clone();
                self.start_file_operation(resolved, true);
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Tab => {
                if let Some(completed) = Self::autocomplete_path(&self.input_value) {
                    self.input_value = completed;
                }
            }
            KeyCode::Backspace => {
                self.input_value.pop();
            }
            KeyCode::Char(c) => {
                self.input_value.push(c);
            }
            _ => {}
        }
    }

    fn handle_confirm_delete_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let _ = self.browser.delete_selected();
                self.browser.refresh();
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn toggle_playback(&self) {
        let status = {
            let state = self.audio.shared_state.lock().unwrap();
            state.status
        };
        match status {
            PlaybackStatus::Playing => self.audio.pause(),
            PlaybackStatus::Paused => self.audio.resume(),
            PlaybackStatus::Stopped => {
                // Play currently selected/highlighted track if any in queue
                if let Some(track) = self.queue.current_track() {
                    self.audio.play(track);
                } else if self.screen == AppScreen::Browser && self.browser.focused_pane == PaneType::Files && !self.browser.files.is_empty() {
                    let path = self.browser.files[self.browser.file_index].path.clone();
                    if matches_audio_extension(&path) {
                        self.audio.play(path);
                    }
                }
            }
        }
    }

    fn navigate_up(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.search.active {
                    if !self.search.results.is_empty() && self.search.selected_index > 0 {
                        self.search.selected_index -= 1;
                    }
                } else {
                    self.browser.move_up();
                }
            }
            AppScreen::Queue => {
                if self.queue_items_len() > 0 && self.queue_selected_index > 0 {
                    self.queue_selected_index -= 1;
                }
            }
            AppScreen::Collections => {
                match self.collections_focused_pane {
                    PaneType::Directories => {
                        if !self.collections.collections.is_empty() && self.selected_collection_index > 0 {
                            self.selected_collection_index -= 1;
                            self.active_collection_file_index = 0;
                        }
                    }
                    PaneType::Files => {
                        let coll_names: Vec<&String> = self.collections.collections.keys().collect();
                        if let Some(&name) = coll_names.get(self.selected_collection_index) {
                            if let Some(files) = self.collections.collections.get(name) {
                                if !files.is_empty() && self.active_collection_file_index > 0 {
                                    self.active_collection_file_index -= 1;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn navigate_down(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.search.active {
                    if !self.search.results.is_empty() && self.search.selected_index + 1 < self.search.results.len() {
                        self.search.selected_index += 1;
                    }
                } else {
                    self.browser.move_down();
                }
            }
            AppScreen::Queue => {
                let len = self.queue_items_len();
                if len > 0 && self.queue_selected_index + 1 < len {
                    self.queue_selected_index += 1;
                }
            }
            AppScreen::Collections => {
                match self.collections_focused_pane {
                    PaneType::Directories => {
                        if !self.collections.collections.is_empty() && self.selected_collection_index + 1 < self.collections.collections.len() {
                            self.selected_collection_index += 1;
                            self.active_collection_file_index = 0;
                        }
                    }
                    PaneType::Files => {
                        let coll_names: Vec<&String> = self.collections.collections.keys().collect();
                        if let Some(&name) = coll_names.get(self.selected_collection_index) {
                            if let Some(files) = self.collections.collections.get(name) {
                                if !files.is_empty() && self.active_collection_file_index + 1 < files.len() {
                                    self.active_collection_file_index += 1;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn navigate_left(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.browser.focused_pane == PaneType::Preview {
                    self.browser.focused_pane = PaneType::Files;
                } else if self.browser.focused_pane == PaneType::Files {
                    self.browser.focused_pane = PaneType::Directories;
                } else {
                    self.browser.go_to_parent();
                }
            }
            AppScreen::Collections => {
                self.collections_focused_pane = PaneType::Directories;
            }
            _ => {}
        }
    }

    fn navigate_right(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.browser.focused_pane == PaneType::Files {
                    if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                        let path = &self.browser.files[self.browser.file_index].path;
                        if matches_image_extension(path) || matches_text_extension(path) {
                            self.browser.focused_pane = PaneType::Preview;
                        }
                    }
                } else if self.browser.focused_pane == PaneType::Directories {
                    self.browser.focused_pane = PaneType::Files;
                }
            }
            AppScreen::Collections => {
                self.collections_focused_pane = PaneType::Files;
            }
            _ => {}
        }
    }

    fn navigate_back(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.search.active {
                    // Turn off search results view
                    self.search.active = false;
                    self.search.query.clear();
                } else {
                    self.browser.go_to_parent();
                }
            }
            AppScreen::Queue => {
                if self.search.active {
                    self.search.active = false;
                    self.search.query.clear();
                    self.sync_queue_selection();
                } else {
                    self.screen = AppScreen::Browser;
                }
            }
            AppScreen::Collections => {
                self.screen = AppScreen::Browser;
            }
        }
    }

    fn activate_item(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.search.active {
                    if !self.search.results.is_empty() {
                        let path = self.search.results[self.search.selected_index].path.clone();
                        if matches_audio_extension(&path) {
                            // Stop current, clear queue, add this track, play
                            self.queue.clear();
                            self.queue.add(path.clone());
                            self.queue.current_index = Some(0);
                            self.audio.play(path);
                            self.sync_queue_selection();
                        } else {
                            Self::open_file_with_default_app(&path);
                        }
                    }
                } else {
                    match self.browser.focused_pane {
                        PaneType::Directories => {
                            self.browser.open_selected_dir();
                        }
                        PaneType::Files => {
                            if !self.browser.files.is_empty() {
                                let path = self.browser.files[self.browser.file_index].path.clone();
                                if matches_audio_extension(&path) {
                                    // Set up queue with currently visible files (if audio) to acts as list
                                    self.queue.clear();
                                    
                                    let mut added_idx = 0;
                                    for (idx, f) in self.browser.files.iter().enumerate() {
                                        if matches_audio_extension(&f.path) {
                                            self.queue.add(f.path.clone());
                                            if idx == self.browser.file_index {
                                                self.queue.current_index = Some(added_idx);
                                            }
                                            added_idx += 1;
                                        }
                                    }
                                    
                                    self.audio.play(path);
                                    self.sync_queue_selection();
                                } else {
                                    Self::open_file_with_default_app(&path);
                                }
                            }
                        }
                        PaneType::Preview => {}
                    }
                }
            }
            AppScreen::Queue => {
                let filtered = self.get_filtered_queue_indices();
                if let Some(&(_, original_idx)) = filtered.get(self.queue_selected_index) {
                    if original_idx < self.queue.items.len() {
                        let path = self.queue.items[original_idx].clone();
                        self.queue.current_index = Some(original_idx);
                        self.audio.play(path);
                        self.sync_queue_selection();
                    }
                }
            }
            AppScreen::Collections => {
                if self.collections_focused_pane == PaneType::Files {
                    let coll_names: Vec<&String> = self.collections.collections.keys().collect();
                    if let Some(&name) = coll_names.get(self.selected_collection_index) {
                        if let Some(files) = self.collections.collections.get(name) {
                            if !files.is_empty() && self.active_collection_file_index < files.len() {
                                let path = files[self.active_collection_file_index].clone();
                                
                                // Load this collection into playback queue
                                self.queue.clear();
                                self.queue.add_many(files.clone());
                                self.queue.current_index = Some(self.active_collection_file_index);
                                
                                self.audio.play(path);
                                self.sync_queue_selection();
                            }
                        }
                    }
                }
            }
        }
    }

    fn seek_relative(&self, seconds: i64) {
        let (elapsed, duration) = {
            let state = self.audio.shared_state.lock().unwrap();
            (state.elapsed_secs, state.duration_secs)
        };
        if duration > 0 {
            let new_pos = if seconds < 0 {
                elapsed.saturating_sub(seconds.unsigned_abs())
            } else {
                elapsed.saturating_add(seconds as u64).min(duration)
            };
            self.audio.seek(std::time::Duration::from_secs(new_pos));
        }
    }

    pub fn sync_queue_selection(&mut self) {
        if let Some(idx) = self.queue.current_index {
            let filtered = self.get_filtered_queue_indices();
            if let Some(pos) = filtered.iter().position(|&(_, orig_idx)| orig_idx == idx) {
                self.queue_selected_index = pos;
            }
        }
    }

    fn update_media_controls(&mut self) {
        if let Some(ref mut controls) = self.media_controls {
            let (status, current_track, metadata, elapsed, duration) = {
                let state = self.audio.shared_state.lock().unwrap();
                (state.status, state.current_track.clone(), state.metadata.clone(), state.elapsed_secs, state.duration_secs)
            };

            let status_changed = Some(status) != self.last_media_status;
            let track_changed = current_track != self.last_media_track;
            let elapsed_changed = elapsed != self.last_media_elapsed;

            if status_changed || track_changed || elapsed_changed {
                if track_changed {
                    if let Some(meta) = metadata {
                        let title = meta.title.as_deref();
                        let artist = meta.artist.as_deref();
                        let album = meta.album.as_deref();
                        let cover_url = current_track.as_ref().and_then(|p| Self::find_cover_art(p));
                        let m_meta = souvlaki::MediaMetadata {
                            title,
                            artist,
                            album,
                            duration: Some(std::time::Duration::from_secs(duration)),
                            cover_url: cover_url.as_deref(),
                            ..Default::default()
                        };
                        let _ = controls.set_metadata(m_meta);
                    } else {
                        let _ = controls.set_metadata(souvlaki::MediaMetadata::default());
                    }
                    self.last_media_track = current_track;
                }

                if status_changed || elapsed_changed {
                    let progress = if duration > 0 {
                        Some(souvlaki::MediaPosition(std::time::Duration::from_secs(elapsed)))
                    } else {
                        None
                    };

                    let playback = match status {
                        PlaybackStatus::Playing => souvlaki::MediaPlayback::Playing { progress },
                        PlaybackStatus::Paused => souvlaki::MediaPlayback::Paused { progress },
                        PlaybackStatus::Stopped => souvlaki::MediaPlayback::Stopped,
                    };
                    let _ = controls.set_playback(playback);

                    self.last_media_status = Some(status);
                    self.last_media_elapsed = elapsed;
                }
            }
        }
    }

    fn find_cover_art(track_path: &Path) -> Option<String> {
        // 1. Look for local cover image in the same directory as the track
        if let Some(parent) = track_path.parent() {
            let common_names = ["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "front.png", "Cover.jpg", "Cover.png", "Folder.jpg", "Folder.png"];
            for name in &common_names {
                let img_path = parent.join(name);
                if img_path.exists() && img_path.is_file() {
                    return Some(format!("file://{}", img_path.to_string_lossy()));
                }
            }
            
            // case-insensitive check
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                            let lower = filename.to_lowercase();
                            if lower == "cover.jpg" || lower == "cover.png" || lower == "folder.jpg" || lower == "folder.png" || lower == "front.jpg" || lower == "front.png" {
                                return Some(format!("file://{}", path.to_string_lossy()));
                            }
                        }
                    }
                }
            }
        }

        // 2. Try to extract embedded cover art from the audio file itself using lofty
        if let Ok(tagged_file) = Probe::open(track_path).and_then(|p| p.read()) {
            if let Some(tag) = tagged_file.primary_tag() {
                if let Some(picture) = tag.pictures().first() {
                    let data = picture.data();
                    if let Some(cache_dir) = dirs::cache_dir() {
                        let art_cache_dir = cache_dir.join("stash").join("album_art");
                        if std::fs::create_dir_all(&art_cache_dir).is_ok() {
                            use std::collections::hash_map::DefaultHasher;
                            use std::hash::{Hash, Hasher};
                            
                            let mut hasher = DefaultHasher::new();
                            track_path.hash(&mut hasher);
                            let hash = hasher.finish();
                            
                            let mime_str = picture.mime_type().map(|m| m.as_str()).unwrap_or("image/jpeg");
                            let ext = if mime_str.contains("png") { "png" } else { "jpg" };
                            
                            let cached_path = art_cache_dir.join(format!("{}.{}", hash, ext));
                            
                            // Only write if it doesn't exist
                            if cached_path.exists() || std::fs::write(&cached_path, data).is_ok() {
                                return Some(format!("file://{}", cached_path.to_string_lossy()));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    pub fn get_filtered_queue_indices(&self) -> Vec<(usize, usize)> {
        let is_shuffle = {
            if let Ok(state) = self.audio.shared_state.lock() {
                state.shuffle
            } else {
                false
            }
        };

        let base_indices: Vec<usize> = if is_shuffle && !self.queue.shuffle_indices.is_empty() {
            self.queue.shuffle_indices.clone()
        } else {
            (0..self.queue.items.len()).collect()
        };

        let query_lower = self.search.query.to_lowercase();
        base_indices
            .into_iter()
            .enumerate()
            .filter(|(_, orig_idx)| {
                if self.search.active && !query_lower.is_empty() {
                    if let Some(path) = self.queue.items.get(*orig_idx) {
                        let filename = path.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                        filename.contains(&query_lower)
                    } else {
                        false
                    }
                } else {
                    true
                }
            })
            .collect()
    }

    pub fn queue_items_len(&self) -> usize {
        self.get_filtered_queue_indices().len()
    }

    pub fn is_file_operation_active(&self) -> bool {
        self.file_progress.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    fn start_file_operation(&mut self, dest_dir: PathBuf, is_move: bool) {
        let paths: Vec<PathBuf> = self.browser.selected_paths.iter().cloned().collect();
        if paths.is_empty() {
            return;
        }

        let total_items = paths.len();
        let progress = Arc::clone(&self.file_progress);
        let op_type = if is_move { "Moving".to_string() } else { "Copying".to_string() };

        {
            let mut p = progress.lock().unwrap();
            *p = Some(FileOperationProgress {
                op_type: op_type.clone(),
                current_file: String::new(),
                completed_files: 0,
                total_files: total_items,
                finished: false,
                error: None,
                canceled: false,
            });
        }

        thread::spawn(move || {
            if !dest_dir.exists() {
                if let Err(e) = std::fs::create_dir_all(&dest_dir) {
                    let mut p = progress.lock().unwrap();
                    if let Some(ref mut state) = *p {
                        state.error = Some(e.to_string());
                        state.finished = true;
                    }
                    return;
                }
            }

            let mut completed = 0;
            for path in paths {
                // Check if operation was canceled by the user
                {
                    let p = progress.lock().unwrap();
                    if let Some(ref state) = *p {
                        if state.canceled {
                            break;
                        }
                    }
                }

                if let Some(name) = path.file_name() {
                    let name_str = name.to_string_lossy().into_owned();
                    {
                        let mut p = progress.lock().unwrap();
                        if let Some(ref mut state) = *p {
                            state.current_file = name_str;
                        }
                    }

                    let dest_path = dest_dir.join(name);
                    let res = if path.is_file() {
                        if is_move {
                            std::fs::rename(&path, &dest_path).or_else(|_| {
                                std::fs::copy(&path, &dest_path).and_then(|_| std::fs::remove_file(&path))
                            })
                        } else {
                            std::fs::copy(&path, &dest_path).map(|_| ())
                        }
                    } else if path.is_dir() {
                        if is_move {
                            std::fs::rename(&path, &dest_path).or_else(|_| {
                                Self::copy_dir_recursive(&path, &dest_path, &progress).and_then(|_| std::fs::remove_dir_all(&path))
                            })
                        } else {
                            Self::copy_dir_recursive(&path, &dest_path, &progress)
                        }
                    } else {
                        Ok(())
                    };

                    if let Err(e) = res {
                        let mut p = progress.lock().unwrap();
                        if let Some(ref mut state) = *p {
                            state.error = Some(format!("Error on {}: {}", path.display(), e));
                            state.finished = true;
                        }
                        return;
                    }

                    completed += 1;
                    {
                        let mut p = progress.lock().unwrap();
                        if let Some(ref mut state) = *p {
                            state.completed_files = completed;
                        }
                    }
                }
            }

            {
                let mut p = progress.lock().unwrap();
                if let Some(ref mut state) = *p {
                    state.finished = true;
                }
            }
        });
    }

    pub fn copy_dir_recursive(src: &Path, dst: &Path, progress: &Arc<Mutex<Option<FileOperationProgress>>>) -> std::io::Result<()> {
        // Check cancellation
        {
            if let Some(ref state) = *progress.lock().unwrap() {
                if state.canceled {
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Operation canceled by user"));
                }
            }
        }

        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let entry_path = entry.path();
            let dest_path = dst.join(entry.file_name());

            // Check cancellation before processing next entry
            {
                if let Some(ref state) = *progress.lock().unwrap() {
                    if state.canceled {
                        return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Operation canceled by user"));
                    }
                }
            }

            if entry_path.is_dir() {
                Self::copy_dir_recursive(&entry_path, &dest_path, progress)?;
            } else {
                std::fs::copy(&entry_path, &dest_path)?;
            }
        }
        Ok(())
    }

    pub fn autocomplete_path(input: &str) -> Option<String> {
        let path = Path::new(input);
        
        let (search_dir, prefix) = if input.ends_with('/') || input.ends_with('\\') || input.is_empty() {
            let dir = if input.is_empty() {
                Path::new(".")
            } else {
                path
            };
            (dir, "")
        } else {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            (parent, file_name)
        };

        let search_dir_resolved = if search_dir.to_string_lossy().starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                let stripped = search_dir.strip_prefix("~").unwrap_or(Path::new(""));
                home.join(stripped)
            } else {
                search_dir.to_path_buf()
            }
        } else {
            search_dir.to_path_buf()
        };

        if let Ok(entries) = std::fs::read_dir(&search_dir_resolved) {
            let mut matches = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(prefix) {
                    matches.push((name, entry.path().is_dir()));
                }
            }

            matches.sort_by(|a, b| a.0.cmp(&b.0));

            if !matches.is_empty() {
                let longest_common = Self::longest_common_prefix(&matches.iter().map(|m| m.0.as_str()).collect::<Vec<&str>>());
                if longest_common.len() > prefix.len() {
                    let parent_str = search_dir.to_string_lossy();
                    let parent_str = if parent_str == "." && !input.starts_with('.') {
                        "".to_string()
                    } else if parent_str.ends_with('/') || parent_str.ends_with('\\') {
                        parent_str.into_owned()
                    } else if parent_str.is_empty() {
                        "".to_string()
                    } else {
                        format!("{}{}", parent_str, std::path::MAIN_SEPARATOR)
                    };
                    
                    let mut completed = format!("{}{}", parent_str, longest_common);
                    if matches.len() == 1 && matches[0].1 {
                        completed.push(std::path::MAIN_SEPARATOR);
                    }
                    return Some(completed);
                } else if matches.len() == 1 {
                    let parent_str = search_dir.to_string_lossy();
                    let parent_str = if parent_str == "." && !input.starts_with('.') {
                        "".to_string()
                    } else if parent_str.ends_with('/') || parent_str.ends_with('\\') {
                        parent_str.into_owned()
                    } else if parent_str.is_empty() {
                        "".to_string()
                    } else {
                        format!("{}{}", parent_str, std::path::MAIN_SEPARATOR)
                    };
                    
                    let mut completed = format!("{}{}", parent_str, matches[0].0);
                    if matches[0].1 {
                        completed.push(std::path::MAIN_SEPARATOR);
                    }
                    return Some(completed);
                }
            }
        }
        None
    }

    fn longest_common_prefix(strs: &[&str]) -> String {
        if strs.is_empty() {
            return String::new();
        }
        let first = strs[0];
        let mut length = first.len();
        for &s in &strs[1..] {
            length = length.min(s.len());
            let mut i = 0;
            for (c1, c2) in first.chars().zip(s.chars()) {
                if c1 != c2 {
                    break;
                }
                i += 1;
            }
            length = length.min(i);
        }
        first.chars().take(length).collect()
    }

    fn navigate_page_up(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                self.browser.page_up();
            }
            AppScreen::Queue => {
                if self.queue_items_len() > 0 {
                    self.queue_selected_index = self.queue_selected_index.saturating_sub(10);
                }
            }
            AppScreen::Collections => {
                match self.collections_focused_pane {
                    PaneType::Directories => {
                        let count = self.collections.collections.len();
                        if count > 0 {
                            self.selected_collection_index = self.selected_collection_index.saturating_sub(10);
                        }
                    }
                    PaneType::Files => {
                        let coll_names: Vec<&String> = self.collections.collections.keys().collect();
                        if let Some(&name) = coll_names.get(self.selected_collection_index) {
                            if let Some(files) = self.collections.collections.get(name) {
                                if !files.is_empty() {
                                    self.active_collection_file_index = self.active_collection_file_index.saturating_sub(10);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn navigate_page_down(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                self.browser.page_down();
            }
            AppScreen::Queue => {
                let len = self.queue_items_len();
                if len > 0 {
                    self.queue_selected_index = (self.queue_selected_index + 10).min(len.saturating_sub(1));
                }
            }
            AppScreen::Collections => {
                match self.collections_focused_pane {
                    PaneType::Directories => {
                        let count = self.collections.collections.len();
                        if count > 0 {
                            self.selected_collection_index = (self.selected_collection_index + 10).min(count.saturating_sub(1));
                        }
                    }
                    PaneType::Files => {
                        let coll_names: Vec<&String> = self.collections.collections.keys().collect();
                        if let Some(&name) = coll_names.get(self.selected_collection_index) {
                            if let Some(files) = self.collections.collections.get(name) {
                                let len = files.len();
                                if len > 0 {
                                    self.active_collection_file_index = (self.active_collection_file_index + 10).min(len.saturating_sub(1));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn remove_queue_item(&mut self, index: usize) {
        if index < self.queue.items.len() {
            self.queue.items.remove(index);
            
            // Adjust current_index if necessary
            if let Some(curr) = self.queue.current_index {
                if curr == index {
                    self.audio.stop();
                    self.queue.current_index = None;
                } else if curr > index {
                    self.queue.current_index = Some(curr - 1);
                }
            }
            
            // Adjust shuffle indices
            self.queue.shuffle_items();
            self.sync_queue_selection();
        }
    }

    pub fn delete_highlighted_queue_item(&mut self) {
        let filtered = self.get_filtered_queue_indices();
        if let Some(&(_, original_idx)) = filtered.get(self.queue_selected_index) {
            self.remove_queue_item(original_idx);
            
            // Bound check queue_selected_index
            let new_len = self.get_filtered_queue_indices().len();
            if new_len == 0 {
                self.queue_selected_index = 0;
            } else if self.queue_selected_index >= new_len {
                self.queue_selected_index = new_len - 1;
            }
        }
    }

    pub fn move_highlighted_queue_item(&mut self, up: bool) {
        let filtered = self.get_filtered_queue_indices();
        if let Some(&(_, original_idx)) = filtered.get(self.queue_selected_index) {
            if up {
                if original_idx > 0 {
                    self.queue.items.swap(original_idx, original_idx - 1);
                    // Update current_index
                    if self.queue.current_index == Some(original_idx) {
                        self.queue.current_index = Some(original_idx - 1);
                    } else if self.queue.current_index == Some(original_idx - 1) {
                        self.queue.current_index = Some(original_idx);
                    }
                    // Adjust selected index
                    self.queue_selected_index = self.queue_selected_index.saturating_sub(1);
                    self.queue.shuffle_items();
                }
            } else {
                if original_idx + 1 < self.queue.items.len() {
                    self.queue.items.swap(original_idx, original_idx + 1);
                    // Update current_index
                    if self.queue.current_index == Some(original_idx) {
                        self.queue.current_index = Some(original_idx + 1);
                    } else if self.queue.current_index == Some(original_idx + 1) {
                        self.queue.current_index = Some(original_idx);
                    }
                    // Adjust selected index
                    let max_len = self.get_filtered_queue_indices().len();
                    self.queue_selected_index = (self.queue_selected_index + 1).min(max_len.saturating_sub(1));
                    self.queue.shuffle_items();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn test_queue_search_filtering() {
        let (tx, _rx) = channel();
        let mut app = App::new(tx, None);

        // Clear queue and add dummy tracks
        app.queue.clear();
        app.queue.add(PathBuf::from("dance_electronic.mp3"));
        app.queue.add(PathBuf::from("ambient_chill.wav"));
        app.queue.add(PathBuf::from("rock_classic.flac"));

        // By default, no search active, filtered indices should show all 3 items
        let indices = app.get_filtered_queue_indices();
        assert_eq!(indices.len(), 3);
        assert_eq!(indices[0], (0, 0));
        assert_eq!(indices[1], (1, 1));
        assert_eq!(indices[2], (2, 2));

        // Activate search and set query
        app.search.active = true;
        app.search.query = "classic".to_string();

        let indices = app.get_filtered_queue_indices();
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0].0, 2); // base_display_idx is 2
        assert_eq!(indices[0].1, 2); // original index is 2
        assert_eq!(app.queue_items_len(), 1);

        // Test lowercase matching
        app.search.query = "CHILL".to_string();
        let indices = app.get_filtered_queue_indices();
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0].0, 1);
        assert_eq!(indices[0].1, 1);
        assert_eq!(app.queue_items_len(), 1);

        // Test non-matching query
        app.search.query = "pop".to_string();
        assert_eq!(app.queue_items_len(), 0);

        // Test shuffle interaction
        app.search.query = "electronic".to_string();
        app.queue.shuffle_indices = vec![2, 0, 1]; // shuffled order
        {
            let mut state = app.audio.shared_state.lock().unwrap();
            state.shuffle = true;
        }

        // Under shuffle, the visual order is shuffle_indices.
        // Index 0 in visual order is 2 (rock_classic.flac)
        // Index 1 in visual order is 0 (dance_electronic.mp3)
        // Index 2 in visual order is 1 (ambient_chill.wav)
        // Since query is "electronic", it matches index 0 (dance_electronic.mp3).
        // Therefore, it should match the 2nd visual item (base_idx = 1) whose original index is 0.
        let indices = app.get_filtered_queue_indices();
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0], (1, 0)); // (base_idx 1, orig_idx 0)
    }

    #[test]
    fn test_rename() {
        let (tx, _rx) = channel();
        let test_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_stash_rename");
        
        if test_root.exists() {
            let _ = std::fs::remove_dir_all(&test_root);
        }
        std::fs::create_dir_all(&test_root).unwrap();
        
        let file_path = test_root.join("test_file.mp3");
        std::fs::File::create(&file_path).unwrap();

        let mut app = App::new(tx, Some(test_root.clone()));

        // Focus on files pane
        app.browser.focused_pane = PaneType::Files;
        app.browser.file_index = 0;

        // Trigger rename with F2
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        
        assert_eq!(app.input_mode, InputMode::Rename);
        assert_eq!(app.input_value, "test_file.mp3");
        assert_eq!(app.rename_target, Some(file_path.clone()));

        // Simulate typing new name
        app.input_value = "renamed_file.mp3".to_string();

        // Press Enter to confirm rename
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.input_value.is_empty());
        assert_eq!(app.rename_target, None);

        // Verify filesystem change
        assert!(!file_path.exists());
        let new_file_path = test_root.join("renamed_file.mp3");
        assert!(new_file_path.exists());

        // Verify browser updated files list
        assert_eq!(app.browser.files.len(), 1);
        assert_eq!(app.browser.files[0].name, "renamed_file.mp3");

        // Clean up
        let _ = std::fs::remove_dir_all(&test_root);
    }

    #[test]
    fn test_find_cover_art() {
        let test_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_stash_find_cover_art");
        
        if test_root.exists() {
            let _ = std::fs::remove_dir_all(&test_root);
        }
        std::fs::create_dir_all(&test_root).unwrap();

        let track_path = test_root.join("song.mp3");
        std::fs::File::create(&track_path).unwrap();

        let cover_path = test_root.join("cover.jpg");
        std::fs::File::create(&cover_path).unwrap();

        let res = App::find_cover_art(&track_path);
        assert!(res.is_some());
        let expected_url = format!("file://{}", cover_path.to_string_lossy());
        assert_eq!(res.unwrap(), expected_url);

        let _ = std::fs::remove_dir_all(&test_root);
    }

    #[test]
    fn test_image_preview_navigation() {
        let (tx, _rx) = channel();
        let test_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_stash_image_preview");
        
        if test_root.exists() {
            let _ = std::fs::remove_dir_all(&test_root);
        }
        std::fs::create_dir_all(&test_root).unwrap();

        let image_path = test_root.join("test_image.png");
        std::fs::File::create(&image_path).unwrap();

        let mut app = App::new(tx, Some(test_root.clone()));

        // Make sure it loads the files list
        assert_eq!(app.browser.files.len(), 1);
        assert_eq!(app.browser.files[0].name, "test_image.png");

        // Focus on files pane
        app.browser.focused_pane = PaneType::Files;
        app.browser.file_index = 0;

        // Verify matches_image_extension is true
        assert!(matches_image_extension(&image_path));

        // Test navigate_right when highlighted file is an image -> focus should move to Preview pane
        app.navigate_right();
        assert_eq!(app.browser.focused_pane, PaneType::Preview);

        // Under Preview focus, simulate panning up / down / left / right
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.image_offset_y, -10);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.image_offset_y, 0);

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.image_offset_x, -10);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.image_offset_x, 0);

        // Simulate zoom in / out
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(app.image_zoom, 1.25);

        app.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        assert_eq!(app.image_zoom, 1.0);

        // Navigate left to focus back to Files pane
        app.navigate_left();
        assert_eq!(app.browser.focused_pane, PaneType::Files);

        let _ = std::fs::remove_dir_all(&test_root);
    }

    #[test]
    fn test_text_preview_navigation() {
        let (tx, _rx) = channel();
        let test_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_stash_text_preview");
        
        if test_root.exists() {
            let _ = std::fs::remove_dir_all(&test_root);
        }
        std::fs::create_dir_all(&test_root).unwrap();

        let text_path = test_root.join("test_code.rs");
        std::fs::write(&text_path, "fn main() {\n    println!(\"Hello\");\n}\n// Line 4\n// Line 5\n// Line 6\n// Line 7\n// Line 8\n// Line 9\n// Line 10\n// Line 11\n// Line 12\n").unwrap();

        let mut app = App::new(tx, Some(test_root.clone()));

        // Make sure it loads the files list
        assert_eq!(app.browser.files.len(), 1);
        assert_eq!(app.browser.files[0].name, "test_code.rs");

        // Focus on files pane
        app.browser.focused_pane = PaneType::Files;
        app.browser.file_index = 0;

        // Verify matches_text_extension is true
        assert!(matches_text_extension(&text_path));

        // Test navigate_right when highlighted file is a text file -> focus should move to Preview pane
        app.navigate_right();
        assert_eq!(app.browser.focused_pane, PaneType::Preview);

        // Under Preview focus, simulate scrolling down / up / page down / page up
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 1);

        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 2);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 1);

        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 0);

        // PageDown/PageUp scrolling
        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 10);

        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 0);

        // Navigate left to focus back to Files pane
        app.navigate_left();
        assert_eq!(app.browser.focused_pane, PaneType::Files);

        let _ = std::fs::remove_dir_all(&test_root);
    }

    #[test]
    fn test_autocomplete_and_folder_operations() {
        let (tx, _rx) = channel();
        let test_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_stash_autocomplete");
        
        if test_root.exists() {
            let _ = std::fs::remove_dir_all(&test_root);
        }
        std::fs::create_dir_all(&test_root).unwrap();

        let src_dir = test_root.join("src_folder");
        let dest_dir = test_root.join("dest_folder");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("file.txt"), "hello").unwrap();

        // 1. Autocomplete test
        let completed = App::autocomplete_path(&test_root.join("src_fo").to_string_lossy());
        assert!(completed.is_some());
        let val = completed.unwrap();
        assert!(val.contains("src_folder"));

        // 2. Folder copying test
        let mut app = App::new(tx, Some(test_root.clone()));
        app.browser.selected_paths.insert(src_dir.clone());

        app.start_file_operation(dest_dir.clone(), false);

        // Wait a tiny bit for the worker thread to finish
        std::thread::sleep(std::time::Duration::from_millis(100));

        let copied_folder = dest_dir.join("src_folder");
        assert!(copied_folder.exists());
        assert!(copied_folder.join("file.txt").exists());

        let _ = std::fs::remove_dir_all(&test_root);
    }

    #[test]
    fn test_copy_move_last_operation_dest() {
        let (tx, _rx) = channel();
        let test_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_stash_last_dest");
        
        if test_root.exists() {
            let _ = std::fs::remove_dir_all(&test_root);
        }
        std::fs::create_dir_all(&test_root).unwrap();
        
        let file_path = test_root.join("some_file.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let mut app = App::new(tx, Some(test_root.clone()));
        app.browser.focused_pane = PaneType::Files;
        app.browser.file_index = 0;
        
        // select files
        app.browser.selected_paths.insert(file_path.clone());
        
        // Assert starts empty
        assert_eq!(app.last_operation_dest, "");
        
        // press 'v' (copy)
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(app.input_mode, InputMode::CopyPath);
        assert_eq!(app.input_value, "");
        
        // simulate typing a destination path (we can use target/test_stash_last_dest/dest_v)
        let dest_v = test_root.join("dest_v");
        app.input_value = dest_v.to_string_lossy().into_owned();
        
        // press Enter to confirm
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        
        // should remember last operation dest
        assert_eq!(app.last_operation_dest, dest_v.to_string_lossy());
        
        // Wait a tiny bit for the worker thread to finish
        std::thread::sleep(std::time::Duration::from_millis(100));
        
        // Dismiss progress
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        
        // now select again and press 'y' (move)
        app.browser.selected_paths.insert(file_path.clone());
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.input_mode, InputMode::MovePath);
        
        // input_value should have pre-populated with last_operation_dest
        assert_eq!(app.input_value, dest_v.to_string_lossy());
        
        // change it to target/test_stash_last_dest/dest_y
        let dest_y = test_root.join("dest_y");
        app.input_value = dest_y.to_string_lossy().into_owned();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        
        assert_eq!(app.last_operation_dest, dest_y.to_string_lossy());
        
        let _ = std::fs::remove_dir_all(&test_root);
    }
}

