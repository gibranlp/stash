use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::audio::AudioEngine;
use crate::browser::{BrowserState, PaneType};
use crate::collections::Collections;
use crate::config::{AppConfig, resolve_path};
use crate::events::Event;
use crate::models::{PlaybackStatus, RepeatMode};
use crate::queue::PlaybackQueue;
use crate::search::{SearchState, matches_audio_extension};
use ratatui::widgets::ListState;

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
}

#[derive(Debug, Clone)]
pub struct FileOperationProgress {
    pub op_type: String, // "Copying" or "Moving"
    pub current_file: String,
    pub completed_files: usize,
    pub total_files: usize,
    pub finished: bool,
    pub error: Option<String>,
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
}

impl App {
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
        let audio = AudioEngine::new(event_tx.clone(), config.default_volume);
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
                        if state.finished {
                            should_refresh = true;
                        }
                    }
                }
                if should_refresh {
                    *self.file_progress.lock().unwrap() = None;
                    self.browser.clear_selections();
                    self.browser.refresh();
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

        // Ignore input if a file operation is active
        if self.is_file_operation_active() {
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
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
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
                if self.screen == AppScreen::Collections {
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
            KeyCode::Up | KeyCode::Char('k') => self.navigate_up(),
            KeyCode::Down | KeyCode::Char('j') => self.navigate_down(),
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
                if self.screen == AppScreen::Browser && self.browser.focused_pane == PaneType::Files {
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
                self.audio.set_volume(current_vol.saturating_add(5).min(100));
            }
            KeyCode::Char('-') => {
                let current_vol = {
                    let state = self.audio.shared_state.lock().unwrap();
                    state.volume
                };
                self.audio.set_volume(current_vol.saturating_sub(5));
            }
            KeyCode::Char('r') => {
                let mut state = self.audio.shared_state.lock().unwrap();
                state.repeat = match state.repeat {
                    RepeatMode::Off => RepeatMode::All,
                    RepeatMode::All => RepeatMode::One,
                    RepeatMode::One => RepeatMode::Off,
                };
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
            }
            
            // Queue & Collections triggers
            KeyCode::Char('q') => {
                // Add selected files to queue
                if !self.browser.selected_paths.is_empty() {
                    let paths: Vec<PathBuf> = self.browser.selected_paths.iter().cloned().collect();
                    self.queue.add_many(paths);
                    self.browser.clear_selections();
                } else if self.screen == AppScreen::Browser && self.browser.focused_pane == PaneType::Files && !self.browser.files.is_empty() {
                    // If no selections, add highlighted file
                    let path = self.browser.files[self.browser.file_index].path.clone();
                    if matches_audio_extension(&path) {
                        self.queue.add(path);
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
                if !self.browser.selected_paths.is_empty() {
                    self.input_mode = InputMode::CopyPath;
                    self.input_value = "~/.config/stash/favorites".to_string();
                }
            }
            KeyCode::Char('y') => {
                if !self.browser.selected_paths.is_empty() {
                    self.input_mode = InputMode::MovePath;
                    self.input_value = "~/.config/stash/favorites".to_string();
                }
            }
            KeyCode::Char('d') => {
                if !self.browser.selected_paths.is_empty() {
                    self.input_mode = InputMode::ConfirmDelete;
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
                self.start_file_operation(resolved, false);
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
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
                self.start_file_operation(resolved, true);
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
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
                }
            }
        }
    }

    fn navigate_left(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.browser.focused_pane == PaneType::Files {
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
                self.browser.focused_pane = PaneType::Files;
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
                                }
                            }
                        }
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
                        let m_meta = souvlaki::MediaMetadata {
                            title,
                            artist,
                            album,
                            duration: Some(std::time::Duration::from_secs(duration)),
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

        let total_files = paths.iter().filter(|p| p.is_file()).count();
        if total_files == 0 {
            return;
        }

        let progress = Arc::clone(&self.file_progress);
        let op_type = if is_move { "Moving".to_string() } else { "Copying".to_string() };

        {
            let mut p = progress.lock().unwrap();
            *p = Some(FileOperationProgress {
                op_type: op_type.clone(),
                current_file: String::new(),
                completed_files: 0,
                total_files,
                finished: false,
                error: None,
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
                if path.is_file() {
                    if let Some(filename) = path.file_name() {
                        let filename_str = filename.to_string_lossy().into_owned();
                        {
                            let mut p = progress.lock().unwrap();
                            if let Some(ref mut state) = *p {
                                state.current_file = filename_str;
                            }
                        }

                        let dest_path = dest_dir.join(filename);
                        let res = if is_move {
                            std::fs::rename(&path, &dest_path).or_else(|_| {
                                // Fallback for cross-device moves: copy then delete original
                                std::fs::copy(&path, &dest_path).and_then(|_| std::fs::remove_file(&path))
                            })
                        } else {
                            std::fs::copy(&path, &dest_path).map(|_| ())
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
            }

            {
                let mut p = progress.lock().unwrap();
                if let Some(ref mut state) = *p {
                    state.finished = true;
                }
            }
        });
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
}
