use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::audio::AudioEngine;
use crate::browser::{BrowserState, PaneType};
use crate::collections::Collections;
use crate::config::{AppConfig, resolve_path};
use crate::discord::DiscordPresence;
use crate::events::Event;
use crate::library::{LibraryState, LibraryPanel, LibraryTrack, ScanState, TagEditorState, BulkTagEditorState, write_tags, write_bulk_tag_to_path, is_smart_playlist, start_library_watcher};
use crate::models::{PlaybackStatus, RepeatMode, VisualizerMode};
use crate::queue::PlaybackQueue;
use crate::search::{SearchState, matches_audio_extension, matches_image_extension, matches_text_extension};
use crate::stats::{ListenStats, StatsTracking, unix_ts_secs};
use crate::healer::{HealerState, HealerScreen, HealScanState, HealLookupState, HealStatus};
use crate::healer::backup::BackupStore;
use crate::healer::pipeline;
use crate::healer::musicbrainz;
use crate::healer::fingerprint;
use ratatui::widgets::ListState;
use lofty::prelude::*;
use lofty::probe::Probe;
use notify::RecommendedWatcher;

type LoadingImageSlot = Arc<Mutex<Option<(PathBuf, Option<image::DynamicImage>)>>>;
type LoadingTextSlot  = Arc<Mutex<Option<(PathBuf, Option<Vec<String>>)>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppScreen {
    Browser,
    Queue,
    Library,
    Healer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictAction {
    Replace,
    Skip,
    ReplaceAll,
    SkipAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestBrowserFocus {
    Quick,
    Dirs,
}

pub struct DestBrowserState {
    pub current_dir: PathBuf,
    pub dirs: Vec<PathBuf>,
    pub dir_index: usize,
    pub dir_scroll: usize,
    pub quick_paths: Vec<(String, PathBuf)>,
    pub quick_index: usize,
    pub focus: DestBrowserFocus,
    pub is_move: bool,
    pub loading: bool,
    pub pending_dirs: Option<Arc<Mutex<Option<Vec<PathBuf>>>>>,
}

impl DestBrowserState {
    pub fn new(start_dir: PathBuf, quick_paths: Vec<(String, PathBuf)>, is_move: bool) -> Self {
        Self {
            current_dir: start_dir,
            dirs: Vec::new(),
            dir_index: 0,
            dir_scroll: 0,
            quick_paths,
            quick_index: 0,
            focus: DestBrowserFocus::Quick,
            is_move,
            loading: false,
            pending_dirs: None,
        }
    }

    fn list_dirs(path: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir()
                    && let Some(name) = p.file_name().and_then(|n| n.to_str())
                        && !name.starts_with('.') {
                            dirs.push(p);
                        }
            }
        }
        dirs.sort_by(|a, b| {
            let a_name = a.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            let b_name = b.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            a_name.cmp(&b_name)
        });
        dirs
    }

    // Navega a un directorio en background para no trancar el hilo principal
    pub fn navigate_to(&mut self, dir: PathBuf) {
        self.current_dir = dir.clone();
        self.dir_index = 1; // 0 = "Copy here" virtual entry, 1 = first real subdir
        self.dir_scroll = 0;
        self.loading = true;
        let slot: Arc<Mutex<Option<Vec<PathBuf>>>> = Arc::new(Mutex::new(None));
        self.pending_dirs = Some(slot.clone());
        thread::spawn(move || {
            let result = Self::list_dirs(&dir);
            *slot.lock().unwrap() = Some(result);
        });
    }

    // Jala el resultado del hilo de carga; regresa true si ya llegaron los directorios
    pub fn poll_pending(&mut self) -> bool {
        let result = self.pending_dirs.as_ref().and_then(|slot| {
            slot.lock().unwrap().take()
        });
        if let Some(dirs) = result {
            self.dirs = dirs;
            self.loading = false;
            self.pending_dirs = None;
            // dir_index 0 = "Copy here", 1..=dirs.len() = actual subdirs
            if self.dir_index > self.dirs.len() {
                self.dir_index = self.dirs.len(); // clamp to last real dir
            }
            if self.dirs.is_empty() {
                self.dir_index = 0; // no subdirs — default to "Copy here"
            }
            true
        } else {
            false
        }
    }

    pub fn enter_highlighted(&mut self) {
        // dir_index 0 = "Copy here" virtual entry; real dirs are at 1..=dirs.len()
        if self.dir_index > 0 && !self.dirs.is_empty() {
            let target = self.dirs[self.dir_index - 1].clone();
            self.navigate_to(target);
        }
    }

    pub fn go_up(&mut self) {
        if let Some(parent) = self.current_dir.parent().map(|p| p.to_path_buf()) {
            let old_dir = self.current_dir.clone();
            self.navigate_to(parent.clone());
            let _ = old_dir;
        }
    }

    pub fn move_up(&mut self) {
        if self.dir_index > 0 {
            self.dir_index -= 1;
        }
        self.clamp_scroll(10);
    }

    pub fn move_down(&mut self) {
        // Allow moving through 0 (Copy here) up to dirs.len() (last real subdir)
        if self.dir_index < self.dirs.len() {
            self.dir_index += 1;
        }
        self.clamp_scroll(10);
    }

    pub fn clamp_scroll(&mut self, visible: usize) {
        if self.dir_index < self.dir_scroll {
            self.dir_scroll = self.dir_index;
        } else if self.dir_index >= self.dir_scroll + visible {
            self.dir_scroll = self.dir_index + 1 - visible;
        }
    }
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
    CreateFolder,
    TagEdit,
    BulkTagEdit,
    ConfirmDeletePlaylist,
    ManageMusicFolders,
}

#[derive(Debug, Clone)]
pub struct FileOperationProgress {
    pub op_type: String,
    pub current_file: String,
    pub completed_files: usize,
    pub total_files: usize,
    pub bytes_copied: u64,
    pub total_bytes: u64,
    pub finished: bool,
    pub error: Option<String>,
    pub canceled: bool,
    pub conflict_file: Option<String>,
    pub conflict_src_size: u64,
    pub conflict_dest_size: u64,
    pub conflict_action: Option<ConflictAction>,
    pub replace_all: bool,
    pub skip_all: bool,
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
    pub input_cursor: usize,
    pub show_help: bool,
    pub help_scroll: u16,
    pub should_quit: bool,
    pub selected_add_collection_index: usize,
    pub queue_selected_index: usize,
    pub file_progress: Arc<Mutex<Option<FileOperationProgress>>>,
    pub files_list_state: ListState,
    pub queue_list_state: ListState,
    pub media_controls: Option<souvlaki::MediaControls>,
    pub last_media_status: Option<PlaybackStatus>,
    pub last_media_track: Option<PathBuf>,
    pub last_media_elapsed: u64,
    pub discord: DiscordPresence,
    pub last_discord_track: Option<PathBuf>,
    pub last_discord_status: Option<PlaybackStatus>,
    pub rename_target: Option<PathBuf>,
    pub last_previewed_file: Option<PathBuf>,
    pub current_image_data: Option<(PathBuf, image::DynamicImage)>,
    pub loading_image: LoadingImageSlot,
    pub current_image_protocol: Option<(PathBuf, u16, u16, Box<dyn ratatui_image::protocol::ResizeProtocol>)>,
    pub current_image_lines: Option<(PathBuf, u16, u16, Vec<ratatui::text::Line<'static>>)>,
    pub current_cover_path: Option<PathBuf>,
    pub current_cover_data: Option<(PathBuf, image::DynamicImage)>,
    pub loading_cover: LoadingImageSlot,
    pub current_cover_protocol: Option<(PathBuf, u16, u16, Box<dyn ratatui_image::protocol::ResizeProtocol>)>,
    pub current_cover_lines: Option<(PathBuf, u16, u16, Vec<ratatui::text::Line<'static>>)>,
    pub last_cover_file: Option<PathBuf>,
    pub text_scroll_offset: usize,
    pub current_text_data: Option<(PathBuf, Vec<String>)>,
    pub loading_text: LoadingTextSlot,
    pub lyrics_scroll_offset: usize,
    pub lyrics_focused: bool,
    pub config: AppConfig,
    pub last_operation_dest: String,
    pub picker: Option<ratatui_image::picker::Picker>,
    pub external_drives: Vec<PathBuf>,
    pub drive_scan_ticks: u16,
    pub dest_browser: Option<DestBrowserState>,
    pub conflict_condvar: Arc<Condvar>,
    pub library: LibraryState,
    pub pending_delete_playlist: Option<String>,
    pub library_pending_add_paths: Vec<PathBuf>,
    pub add_coll_creating: bool, // true = "New playlist" sub-input is active in the popup
    pub update: crate::updater::UpdateSlot,
    pub manage_folders_index: usize,
    pub notification: Option<(String, u8)>,  // (message, ticks_remaining at 50ms each)
    pub library_track_list_state: ListState,
    pub stats: ListenStats,
    pub stats_tracking: StatsTracking,
    pub healer: HealerState,
    pub healer_backup: BackupStore,
    pub m_hold_start: Option<std::time::Instant>,
    pub m_last_press: Option<std::time::Instant>,
    pub m_select_all_triggered: bool,
    pub m_clear_all_triggered: bool,
    pub library_rescan_after: Option<std::time::Instant>,
    _library_watcher: Option<RecommendedWatcher>,
    event_tx: std::sync::mpsc::Sender<Event>,
}

// Escanea interfaces USB buscando dispositivos MTP (clase 06 = Still Image / MTP)
pub fn scan_usb_mtp_devices() -> Vec<(u32, u32, String)> {
    let mut devices: Vec<(u32, u32, String)> = Vec::new();
    let sys_usb = std::path::Path::new("/sys/bus/usb/devices");
    let Ok(entries) = std::fs::read_dir(sys_usb) else {
        return devices;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.contains(':') {
            continue;
        }
        let iface_path = entry.path();
        let class = std::fs::read_to_string(iface_path.join("bInterfaceClass"))
            .unwrap_or_default();
        if class.trim() != "06" {
            continue;
        }
        let dev_name = name.split(':').next().unwrap_or(&name);
        let dev_path = sys_usb.join(dev_name);
        let bus = std::fs::read_to_string(dev_path.join("busnum"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let dev = std::fs::read_to_string(dev_path.join("devnum"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let (Some(bus), Some(dev)) = (bus, dev) else {
            continue;
        };
        if devices.iter().any(|(b, d, _)| *b == bus && *d == dev) {
            continue;
        }
        let product = std::fs::read_to_string(dev_path.join("product"))
            .unwrap_or_else(|_| "Unknown Device".to_string())
            .trim()
            .to_string();
        devices.push((bus, dev, product));
    }
    devices
}

// Decodifica el nombre de display de una ruta MTP tipo gvfs (viene URL-encoded con host=...)
pub fn mtp_display_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let mut decoded = String::new();
    let mut chars = raw.chars().peekable();
    loop {
        match chars.next() {
            None => break,
            Some('%') => {
                let h1 = chars.next();
                let h2 = chars.next();
                if let (Some(h1), Some(h2)) = (h1, h2) {
                    let hex = format!("{}{}", h1, h2);
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        decoded.push(byte as char);
                    } else {
                        decoded.push('%');
                        decoded.push(h1);
                        decoded.push(h2);
                    }
                }
            }
            Some(c) => decoded.push(c),
        }
    }
    // El nombre legible viene después del ']' en rutas tipo mtp://[usb:001,002]/Nombre
    if let Some(pos) = decoded.find(']') {
        let after = decoded[pos + 1..].trim_start_matches(['/', ',']).trim();
        if !after.is_empty() {
            return after.to_string();
        }
    }
    decoded
}

pub fn scan_external_drives() -> Vec<PathBuf> {
    let mut drives: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        // On macOS, all volumes (external drives, USB sticks, disk images) mount under /Volumes/.
        // The root system volume shows up as a symlink there — skip it.
        if let Ok(entries) = std::fs::read_dir("/Volumes") {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !path.is_symlink() && !drives.contains(&path) {
                    drives.push(path);
                }
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
            for line in mounts.lines() {
                let mut parts = line.split_whitespace();
                let _device = parts.next();
                let mount_point = match parts.next() {
                    Some(p) => p,
                    None => continue,
                };
                let is_external = mount_point.starts_with("/media/")
                    || mount_point.starts_with("/run/media/")
                    || (mount_point.starts_with("/mnt/") && mount_point.len() > 5);
                if is_external {
                    let path = PathBuf::from(mount_point);
                    if path.is_dir() && !drives.contains(&path) {
                        drives.push(path);
                    }
                }
            }
        }
    }

    drives.sort();
    drives
}

impl App {
    // Ojo: si estamos corriendo con sudo, hay que abrir el archivo como el usuario original
    // para que la app gráfica aparezca en el entorno correcto
    pub fn open_file_with_default_app(path: &Path) {
        let mut cmd = if let Ok(sudo_user) = std::env::var("SUDO_USER") {
            if !sudo_user.is_empty() {
                let mut c = std::process::Command::new("sudo");
                c.arg("-u").arg(&sudo_user).arg("xdg-open").arg(path);
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

    pub fn new(event_tx: std::sync::mpsc::Sender<Event>, initial_path: Option<PathBuf>, picker: Option<ratatui_image::picker::Picker>) -> Self {
        let config = AppConfig::load();

        let starting_dir = if let Some(ref path) = initial_path {
            let resolved = resolve_path(&path.to_string_lossy());
            if resolved.exists() {
                resolved
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

        let discord = DiscordPresence::new(
            config.discord_app_id.unwrap_or(crate::discord::DEFAULT_APP_ID),
        );

        // Inicializa MPRIS para que los controles de media del sistema operativo jalen
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

            match souvlaki::MediaControls::new(platform_config) {
                Ok(mut controls) => {
                    let tx_clone = event_tx.clone();
                    if let Err(e) = controls.attach(move |event| {
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("debug_mpris.log") {
                            use std::io::Write;
                            let _ = writeln!(f, "Received MPRIS event: {:?}", event);
                        }
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
                    }) {
                        let _ = std::fs::write("debug_mpris.log", format!("Attach failed: {:?}", e));
                    } else {
                        let _ = std::fs::write("debug_mpris.log", "MPRIS successfully initialized and attached.");
                    }
                    Some(controls)
                }
                Err(e) => {
                    let _ = std::fs::write("debug_mpris.log", format!("MPRIS Initialization failed: {:?}", e));
                    None
                }
            }
        };

        let library_watcher = start_library_watcher(&config.music_folders, event_tx.clone());

        Self {
            browser,
            audio,
            queue,
            collections,
            search,
            screen: AppScreen::Browser,
            input_mode: InputMode::Normal,
            input_value: String::new(),
            input_cursor: 0,
            show_help: false,
            help_scroll: 0,
            should_quit: false,
            selected_add_collection_index: 0,
            queue_selected_index: 0,
            file_progress: Arc::new(Mutex::new(None)),
            files_list_state: ListState::default(),
            queue_list_state: ListState::default(),
            media_controls,
            last_media_status: None,
            last_media_track: None,
            last_media_elapsed: 0,
            discord,
            last_discord_track: None,
            last_discord_status: None,
            rename_target: None,
            last_previewed_file: None,
            current_image_data: None,
            loading_image: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_image_protocol: None,
            current_image_lines: None,
            current_cover_path: None,
            current_cover_data: None,
            loading_cover: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_cover_protocol: None,
            current_cover_lines: None,
            last_cover_file: None,
            text_scroll_offset: 0,
            current_text_data: None,
            loading_text: Arc::new(Mutex::new(None)),
            lyrics_scroll_offset: 0,
            lyrics_focused: false,
            config,
            last_operation_dest: String::new(),
            picker,
            external_drives: scan_external_drives(),
            drive_scan_ticks: 0,
            dest_browser: None,
            conflict_condvar: Arc::new(Condvar::new()),
            library: LibraryState::new(),
            pending_delete_playlist: None,
            library_pending_add_paths: Vec::new(),
            add_coll_creating: false,
            update: {
                let slot = crate::updater::new_slot();
                crate::updater::spawn_check(slot.clone());
                slot
            },
            manage_folders_index: 0,
            notification: None,
            library_track_list_state: ListState::default(),
            stats: ListenStats::load(),
            stats_tracking: StatsTracking::default(),
            healer: HealerState::new(),
            healer_backup: BackupStore::load(),
            m_hold_start: None,
            m_last_press: None,
            m_select_all_triggered: false,
            m_clear_all_triggered: false,
            library_rescan_after: None,
            _library_watcher: library_watcher,
            event_tx,
        }
    }

    fn restart_library_watcher(&mut self) {
        self._library_watcher = start_library_watcher(&self.config.music_folders, self.event_tx.clone());
    }

    fn jump_to_external_drives(&mut self) {
        self.external_drives = scan_external_drives();
        if self.external_drives.len() == 1 {
            self.browser.current_dir = self.external_drives[0].clone();
            self.browser.refresh();
            self.browser.file_index = 0;
        } else if !self.external_drives.is_empty() {
            let first = &self.external_drives[0];
            if let Some(parent) = first.parent() {
                self.browser.current_dir = parent.to_path_buf();
                self.browser.refresh();
                self.browser.file_index = 0;
            }
        }
    }

    fn gvfs_mtp_paths_list() -> Vec<PathBuf> {
        let uid_str = std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "1000".to_string());
        let gvfs_dir = PathBuf::from(format!("/run/user/{}/gvfs", uid_str));
        let mut result = Vec::new();
        if gvfs_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&gvfs_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if name.starts_with("mtp:") || name.starts_with("gphoto2:") {
                        result.push(p);
                    }
                }
            }
        result
    }

    fn build_quick_paths(&self) -> Vec<(String, PathBuf)> {
        let mut paths: Vec<(String, PathBuf)> = Vec::new();
        for drive in scan_external_drives() {
            let name = drive
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| drive.to_string_lossy().into_owned());
            paths.push((name, drive));
        }
        for mtp_path in Self::gvfs_mtp_paths_list() {
            let name = mtp_display_name(&mtp_path);
            paths.push((name, mtp_path));
        }
        paths
    }

    pub fn open_dest_browser(&mut self, is_move: bool) {
        let quick_paths = self.build_quick_paths();
        let start_dir = if !self.last_operation_dest.is_empty() {
            let p = PathBuf::from(&self.last_operation_dest);
            if p.is_dir() { p } else { self.browser.current_dir.clone() }
        } else {
            self.browser.current_dir.clone()
        };
        let mut db = DestBrowserState::new(start_dir.clone(), quick_paths, is_move);
        db.navigate_to(start_dir);
        self.dest_browser = Some(db);
        self.input_mode = if is_move { InputMode::MovePath } else { InputMode::CopyPath };
    }

    // Si no hay dispositivos montados, intenta montarlos con gio antes de navegar
    fn jump_to_mtp(&mut self) {
        let uid = std::env::var("UID")
            .unwrap_or_else(|_| {
                if let Ok(output) = std::process::Command::new("id").arg("-u").output() {
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                } else {
                    "1000".to_string()
                }
            });

        let gvfs_path = std::path::PathBuf::from(format!("/run/user/{}/gvfs", uid));
        if !gvfs_path.exists() {
            return;
        }

        let mut mtp_paths = Self::gvfs_mtp_paths_list();

        if mtp_paths.is_empty() {
            for (bus, dev, _) in scan_usb_mtp_devices() {
                let uri = format!("mtp://[usb:{:03},{:03}]/", bus, dev);
                let _ = std::process::Command::new("gio")
                    .arg("mount")
                    .arg(&uri)
                    .output();
            }
            mtp_paths = Self::gvfs_mtp_paths_list();
        }

        if !mtp_paths.is_empty() {
            self.browser.current_dir = mtp_paths[0].clone();
        } else {
            self.browser.current_dir = gvfs_path;
        }
        self.browser.refresh();
        self.browser.file_index = 0;
    }

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Paste(content) => self.handle_paste(content),
            Event::Tick => {
                // Detect 'm' key release: no 'm' press for 300ms means the key was let go
                if let Some(last) = self.m_last_press {
                    if last.elapsed() >= std::time::Duration::from_millis(300) {
                        if !self.m_select_all_triggered && self.m_hold_start.is_some() {
                            // Short tap — toggle the current track
                            let filter_query = if self.search.active { self.search.query.clone() } else { String::new() };
                            let visible: Vec<PathBuf> = self.library
                                .visible_tracks(&self.collections, &filter_query, &self.stats)
                                .iter().map(|t| t.path.clone()).collect();
                            if let Some(path) = visible.get(self.library.track_index).cloned() {
                                if self.library.selected_tracks.contains(&path) {
                                    self.library.selected_tracks.remove(&path);
                                } else {
                                    self.library.selected_tracks.insert(path);
                                }
                            }
                        }
                        self.m_hold_start = None;
                        self.m_last_press = None;
                        self.m_select_all_triggered = false;
                        self.m_clear_all_triggered = false;
                    }
                }
                self.drive_scan_ticks += 1;
                if self.drive_scan_ticks >= 20 {
                    self.drive_scan_ticks = 0;
                    let fresh = scan_external_drives();
                    if fresh != self.external_drives {
                        self.external_drives = fresh;
                        // Refresh browser if it's showing a media directory
                        let cur = &self.browser.current_dir;
                        if cur == Path::new("/media")
                            || cur.starts_with("/run/media")
                            || cur == Path::new("/Volumes")
                        {
                            self.browser.refresh();
                        }
                    }
                }
                if let Some((_, ref mut ticks)) = self.notification {
                    if *ticks == 0 {
                        self.notification = None;
                    } else {
                        *ticks -= 1;
                    }
                }
                if let Some(ref mut db) = self.dest_browser {
                    db.poll_pending();
                }
                // Si la operación terminó limpia sin errores, refresca el browser automáticamente
                let mut should_refresh = false;
                {
                    let p = self.file_progress.lock().unwrap();
                    if let Some(ref state) = *p
                        && state.finished && state.error.is_none() && !state.canceled {
                            should_refresh = true;
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

                // Si cambió el archivo seleccionado, limpiamos cache de preview y arrancamos carga nueva
                if current_preview_path != self.last_previewed_file {
                    self.current_image_data = None;
                    self.current_image_protocol = None;
                    self.current_image_lines = None;
                    self.current_text_data = None;
                    self.last_previewed_file = current_preview_path.clone();
                    self.text_scroll_offset = 0;

                    if let Some(ref new_path) = self.last_previewed_file
                        && matches_text_extension(new_path) {
                            let mut lock = self.loading_text.lock().unwrap();
                            *lock = Some((new_path.clone(), None));
                            let loading_clone = self.loading_text.clone();
                            let path_clone = new_path.clone();
                            thread::spawn(move || {
                                let lines = std::fs::read_to_string(&path_clone)
                                    .map(|c| c.lines().map(|l| l.to_string()).collect::<Vec<_>>())
                                    .unwrap_or_default();
                                let mut lock = loading_clone.lock().unwrap();
                                if let Some((ref cur, _)) = *lock
                                    && *cur == path_clone {
                                        *lock = Some((path_clone, Some(lines)));
                                    }
                            });
                        }

                    if let Some(ref new_path) = self.last_previewed_file
                        && matches_image_extension(new_path) {
                            let mut lock = self.loading_image.lock().unwrap();
                            *lock = Some((new_path.clone(), None));
                            let loading_clone = self.loading_image.clone();
                            let path_clone = new_path.clone();
                            thread::spawn(move || {
                                if let Ok(img) = image::open(&path_clone) {
                                    // Reducimos imágenes enormes para no comernos la RAM
                                    let img = {
                                        let max_dim = 1024u32;
                                        if img.width() > max_dim || img.height() > max_dim {
                                            img.resize(max_dim, max_dim, image::imageops::FilterType::CatmullRom)
                                        } else {
                                            img
                                        }
                                    };
                                    let mut lock = loading_clone.lock().unwrap();
                                    if let Some((ref cur_path, _)) = *lock
                                        && cur_path == &path_clone {
                                            *lock = Some((path_clone, Some(img)));
                                        }
                                }
                            });
                        }
                }

                // Promueve imagen cargada en background al estado activo
                let completed = {
                    let mut lock = self.loading_image.lock().unwrap();
                    if matches!(&*lock, Some((_, Some(_)))) { lock.take() } else { None }
                };
                if let Some((loaded_path, Some(loaded_img))) = completed {
                    self.current_image_data = Some((loaded_path, loaded_img));
                    self.current_image_protocol = None;
                    self.current_image_lines = None;
                }

                let completed_text = {
                    let mut lock = self.loading_text.lock().unwrap();
                    if matches!(&*lock, Some((_, Some(_)))) { lock.take() } else { None }
                };
                if let Some((loaded_path, Some(lines))) = completed_text {
                    self.current_text_data = Some((loaded_path, lines));
                }

                // Mismo patrón para la carátula del track en reproducción
                if self.current_cover_path != self.last_cover_file {
                    self.current_cover_data = None;
                    self.current_cover_protocol = None;
                    self.current_cover_lines = None;
                    self.last_cover_file = self.current_cover_path.clone();

                    if let Some(ref new_path) = self.last_cover_file {
                        let mut lock = self.loading_cover.lock().unwrap();
                        *lock = Some((new_path.clone(), None));
                        let loading_clone = self.loading_cover.clone();
                        let path_clone = new_path.clone();
                        thread::spawn(move || {
                            if let Ok(img) = image::open(&path_clone) {
                                let img = {
                                    let max_dim = 1024u32;
                                    if img.width() > max_dim || img.height() > max_dim {
                                        img.resize(max_dim, max_dim, image::imageops::FilterType::CatmullRom)
                                    } else {
                                        img
                                    }
                                };
                                let mut lock = loading_clone.lock().unwrap();
                                if let Some((ref cur_path, _)) = *lock
                                    && cur_path == &path_clone {
                                        *lock = Some((path_clone, Some(img)));
                                    }
                            }
                        });
                    }
                }

                let completed_cover = {
                    let mut lock = self.loading_cover.lock().unwrap();
                    if matches!(&*lock, Some((_, Some(_)))) { lock.take() } else { None }
                };
                if let Some((loaded_path, Some(loaded_img))) = completed_cover {
                    self.current_cover_data = Some((loaded_path, loaded_img));
                    self.current_cover_protocol = None;
                    self.current_cover_lines = None;
                }

                self.update_media_controls();

                if self.library.scan_state == ScanState::Scanning {
                    self.library.poll_scan();
                }
                if let Some(deadline) = self.library_rescan_after {
                    if std::time::Instant::now() >= deadline {
                        self.library_rescan_after = None;
                        if self.library.scan_state != ScanState::Scanning {
                            self.library.start_scan(&self.config.music_folders);
                        }
                    }
                }
                if self.healer.scan_state == HealScanState::Scanning {
                    self.healer.poll_scan();
                }
                if self.healer.lookup_state == HealLookupState::Searching {
                    self.healer.poll_lookup();
                }

                // Detect track changes to record play/skip stats
                let (new_track, new_elapsed, new_duration) = {
                    let state = self.audio.shared_state.lock().unwrap();
                    (state.current_track.clone(), state.elapsed_secs, state.duration_secs)
                };
                if new_track != self.stats_tracking.current_track {
                    if let Some(ref old_path) = self.stats_tracking.current_track.clone() {
                        let elapsed = self.stats_tracking.last_elapsed;
                        let duration = self.stats_tracking.last_duration;
                        if self.stats_tracking.natural_finish || (duration > 0 && elapsed * 10 >= duration * 8) {
                            self.stats.record_play(old_path, elapsed);
                        } else if elapsed >= 5 {
                            self.stats.record_skip(old_path, elapsed);
                        }
                        self.stats_tracking.session_listen_secs += elapsed;
                        let _ = self.stats.save();
                    }
                    if new_track.is_some() {
                        if self.stats_tracking.session_start_ts == 0 {
                            self.stats_tracking.session_start_ts = unix_ts_secs();
                        }
                        self.stats_tracking.session_tracks += 1;
                    }
                    self.stats_tracking.current_track = new_track;
                    self.stats_tracking.natural_finish = false;
                }
                self.stats_tracking.last_elapsed = new_elapsed;
                self.stats_tracking.last_duration = new_duration;
            }
            Event::AudioFinished => {
                self.handle_audio_finished();
            }
            Event::MediaPlayPause => {
                self.toggle_playback();
            }
            Event::MediaNext => {
                if self.is_queue_search_active() {
                    if let Some(next_path) = self.next_search_result() {
                        self.audio.play(next_path);
                        self.sync_queue_selection();
                    }
                } else {
                    let shuffle = {
                        let state = self.audio.shared_state.lock().unwrap();
                        state.shuffle
                    };
                    if let Some(next_path) = self.queue.next(shuffle) {
                        self.audio.play(next_path);
                        self.sync_queue_selection();
                    }
                }
            }
            Event::MediaPrev => {
                if self.is_queue_search_active() {
                    if let Some(prev_path) = self.prev_search_result() {
                        self.audio.play(prev_path);
                        self.sync_queue_selection();
                    }
                } else {
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
            Event::LibraryChanged => {
                // Debounce: wait 2s after the last change before triggering a rescan
                self.library_rescan_after = Some(
                    std::time::Instant::now() + std::time::Duration::from_secs(2)
                );
            }
        }
    }

    fn handle_audio_finished(&mut self) {
        self.stats_tracking.natural_finish = true;

        let (repeat, shuffle) = {
            let state = self.audio.shared_state.lock().unwrap();
            (state.repeat, state.shuffle)
        };

        let search_active = self.is_queue_search_active();

        match repeat {
            RepeatMode::One => {
                if let Some(path) = self.queue.current_track() {
                    self.audio.play(path);
                }
            }
            RepeatMode::All => {
                if search_active {
                    if let Some(next_path) = self.next_search_result() {
                        self.audio.play(next_path);
                        self.sync_queue_selection();
                    } else {
                        self.queue.current_index = None;
                        if let Some(next_path) = self.next_search_result() {
                            self.audio.play(next_path);
                            self.sync_queue_selection();
                        }
                    }
                } else if let Some(next_path) = self.queue.next(shuffle) {
                    self.audio.play(next_path);
                    self.sync_queue_selection();
                } else {
                    // Llegamos al final de la cola — damos vuelta al inicio
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
                if search_active {
                    if let Some(next_path) = self.next_search_result() {
                        self.audio.play(next_path);
                        self.sync_queue_selection();
                    }
                } else if let Some(next_path) = self.queue.next(shuffle) {
                    self.audio.play(next_path);
                    self.sync_queue_selection();
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.save_session_on_quit();
            self.should_quit = true;
            return;
        }

        if self.input_mode == InputMode::Normal
            && key.code == KeyCode::Char('q') {
                self.save_session_on_quit();
                self.should_quit = true;
                return;
            }

        // Mientras hay una operación de archivo activa, solo procesamos teclas de conflicto/cancelar
        if self.is_file_operation_active() {
            let mut dismiss = false;
            let mut notify_conflict = false;
            {
                let mut p = self.file_progress.lock().unwrap();
                if let Some(ref mut state) = *p {
                    if state.conflict_file.is_some() {
                        let action = match key.code {
                            KeyCode::Char('r') => Some(ConflictAction::Replace),
                            KeyCode::Char('R') => Some(ConflictAction::ReplaceAll),
                            KeyCode::Char('s') | KeyCode::Esc => Some(ConflictAction::Skip),
                            KeyCode::Char('S') => Some(ConflictAction::SkipAll),
                            _ => None,
                        };
                        if let Some(action) = action {
                            state.conflict_action = Some(action);
                            notify_conflict = true;
                        }
                    } else if state.finished || state.error.is_some() || state.canceled {
                        if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
                            dismiss = true;
                        }
                    } else if key.code == KeyCode::Esc {
                        state.canceled = true;
                        state.current_file = "Canceling...".to_string();
                    }
                }
            }
            if notify_conflict {
                self.conflict_condvar.notify_one();
            }
            if dismiss {
                *self.file_progress.lock().unwrap() = None;
                self.browser.clear_selections();
                self.browser.refresh();
            }
            return;
        }

        if self.show_help {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1);
                }
                KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(10);
                }
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(10);
                }
                _ => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }

        match self.input_mode {
            InputMode::Normal => self.handle_normal_key(key),
            InputMode::Search => self.handle_search_key(key),
            InputMode::CreateCollection => self.handle_create_collection_key(key),
            InputMode::AddToCollectionList => self.handle_add_to_collection_list_key(key),
            InputMode::CopyPath => self.handle_dest_browser_key(key),
            InputMode::MovePath => self.handle_dest_browser_key(key),
            InputMode::ConfirmDelete => self.handle_confirm_delete_key(key),
            InputMode::Rename => self.handle_rename_key(key),
            InputMode::CreateFolder => self.handle_create_folder_key(key),
            InputMode::TagEdit => self.handle_tag_edit_key(key),
            InputMode::BulkTagEdit => self.handle_bulk_tag_edit_key(key),
            InputMode::ConfirmDeletePlaylist => self.handle_confirm_delete_playlist_key(key),
            InputMode::ManageMusicFolders => self.handle_manage_folders_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        // U triggers update from any screen when an update is available
        if key.code == KeyCode::Char('U') && key.modifiers.is_empty() {
            let state = self.update.lock().unwrap().clone();
            if let crate::updater::UpdateProgress::Available { version, url } = state {
                crate::updater::spawn_download(version, url, self.update.clone());
                return;
            }
        }

        // Number keys switch screens from anywhere
        if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Char('1') => {
                    self.screen = AppScreen::Browser;
                    self.search.active = false;
                    self.search.query.clear();
                    return;
                }
                KeyCode::Char('2') => {
                    self.screen = AppScreen::Queue;
                    self.search.active = false;
                    self.search.query.clear();
                    self.queue_selected_index = 0;
                    self.sync_queue_selection();
                    return;
                }
                KeyCode::Char('3') => {
                    self.screen = AppScreen::Library;
                    self.search.active = false;
                    self.search.query.clear();
                    if self.library.scan_state == ScanState::Idle {
                        self.library.start_scan(&self.config.music_folders);
                    }
                    return;
                }
                KeyCode::Char('4') => {
                    self.screen = AppScreen::Healer;
                    self.search.active = false;
                    self.search.query.clear();
                    return;
                }
                _ => {}
            }
        }

        if self.screen == AppScreen::Library {
            self.handle_library_normal_key(key);
            return;
        }

        if self.screen == AppScreen::Healer {
            self.handle_healer_key(key);
            return;
        }

        // Cuando el foco está en el panel de preview de texto, las teclas de scroll se quedan aquí
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
                if needs_load
                    && let Ok(content) = std::fs::read_to_string(&file_path) {
                        let parsed_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                        self.current_text_data = Some((file_path, parsed_lines));
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
                        if let Some((_, ref lines)) = self.current_text_data
                            && self.text_scroll_offset + 1 < lines.len() {
                                self.text_scroll_offset += 1;
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
                    _ => {}
                }
            }
        }

        match key.code {
            KeyCode::Tab => {
                if self.screen == AppScreen::Browser {
                    let has_prev = self.has_preview();
                    self.browser.focused_pane = match self.browser.focused_pane {
                        PaneType::Directories => PaneType::Files,
                        PaneType::Files => {
                            if has_prev {
                                PaneType::Preview
                            } else {
                                PaneType::Directories
                            }
                        }
                        PaneType::Preview => PaneType::Directories,
                    };
                } else if self.screen == AppScreen::Queue {
                    self.lyrics_focused = !self.lyrics_focused;
                }
            }
            KeyCode::BackTab => {
                if self.screen == AppScreen::Browser {
                    let has_prev = self.has_preview();
                    self.browser.focused_pane = match self.browser.focused_pane {
                        PaneType::Directories => {
                            if has_prev {
                                PaneType::Preview
                            } else {
                                PaneType::Files
                            }
                        }
                        PaneType::Files => PaneType::Directories,
                        PaneType::Preview => PaneType::Files,
                    };
                }
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                self.help_scroll = 0;
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
                }
            }
            KeyCode::Esc => {
                self.screen = AppScreen::Browser;
                self.search.active = false;
                self.search.query.clear();
            }
            KeyCode::Up => {
                if self.screen == AppScreen::Queue && self.lyrics_focused {
                    self.lyrics_scroll_offset = self.lyrics_scroll_offset.saturating_sub(1);
                } else if key.modifiers.contains(KeyModifiers::CONTROL) && self.screen == AppScreen::Queue {
                    self.move_highlighted_queue_item(true);
                } else if key.modifiers.contains(KeyModifiers::SHIFT) && self.screen == AppScreen::Browser {
                    self.browser_shift_navigate(true);
                } else {
                    if self.screen == AppScreen::Browser {
                        self.browser.shift_start = None;
                    }
                    self.navigate_up();
                }
            }
            KeyCode::Char('k') => {
                if self.screen == AppScreen::Queue && self.lyrics_focused {
                    self.lyrics_scroll_offset = self.lyrics_scroll_offset.saturating_sub(1);
                } else {
                    if self.screen == AppScreen::Browser {
                        self.browser.shift_start = None;
                    }
                    self.navigate_up();
                }
            }
            KeyCode::Char('K') => {
                if self.screen == AppScreen::Queue {
                    self.move_highlighted_queue_item(true);
                } else if self.screen == AppScreen::Browser {
                    self.browser_shift_navigate(true);
                }
            }
            KeyCode::Down => {
                if self.screen == AppScreen::Queue && self.lyrics_focused {
                    self.lyrics_scroll_offset += 1;
                } else if key.modifiers.contains(KeyModifiers::CONTROL) && self.screen == AppScreen::Queue {
                    self.move_highlighted_queue_item(false);
                } else if key.modifiers.contains(KeyModifiers::SHIFT) && self.screen == AppScreen::Browser {
                    self.browser_shift_navigate(false);
                } else {
                    if self.screen == AppScreen::Browser {
                        self.browser.shift_start = None;
                    }
                    self.navigate_down();
                }
            }
            KeyCode::Char('j') => {
                if self.screen == AppScreen::Queue && self.lyrics_focused {
                    self.lyrics_scroll_offset += 1;
                } else {
                    if self.screen == AppScreen::Browser {
                        self.browser.shift_start = None;
                    }
                    self.navigate_down();
                }
            }
            KeyCode::Char('J') => {
                if self.screen == AppScreen::Queue {
                    self.move_highlighted_queue_item(false);
                } else if self.screen == AppScreen::Browser {
                    self.browser_shift_navigate(false);
                }
            }
            KeyCode::PageUp => {
                if self.screen == AppScreen::Queue && self.lyrics_focused {
                    self.lyrics_scroll_offset = self.lyrics_scroll_offset.saturating_sub(10);
                } else {
                    if self.screen == AppScreen::Browser {
                        self.browser.shift_start = None;
                    }
                    self.navigate_page_up();
                }
            }
            KeyCode::PageDown => {
                if self.screen == AppScreen::Queue && self.lyrics_focused {
                    self.lyrics_scroll_offset += 10;
                } else {
                    if self.screen == AppScreen::Browser {
                        self.browser.shift_start = None;
                    }
                    self.navigate_page_down();
                }
            }
            KeyCode::Home => {
                if self.screen == AppScreen::Browser {
                    self.browser.shift_start = None;
                }
                self.navigate_home();
            }
            KeyCode::End => {
                if self.screen == AppScreen::Browser {
                    self.browser.shift_start = None;
                }
                self.navigate_end();
            }
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.seek_relative(-5);
                } else {
                    if self.screen == AppScreen::Browser {
                        self.browser.shift_start = None;
                    }
                    self.navigate_left();
                }
            }
            KeyCode::Char('h') => {
                if self.screen == AppScreen::Browser {
                    self.browser.show_hidden = !self.browser.show_hidden;
                    self.browser.refresh();
                }
            }
            KeyCode::Char('H') => self.seek_relative(-5),
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.seek_relative(5);
                } else {
                    if self.screen == AppScreen::Browser {
                        self.browser.shift_start = None;
                    }
                    self.navigate_right();
                }
            }
            KeyCode::Char('l') => {
                if self.screen == AppScreen::Browser {
                    self.browser.shift_start = None;
                }
                self.navigate_right();
            }
            KeyCode::Char('L') => self.seek_relative(5),
            KeyCode::Enter => {
                if self.screen == AppScreen::Browser {
                    self.browser.shift_start = None;
                }
                self.activate_item();
            }
            KeyCode::Backspace => {
                if self.screen == AppScreen::Browser {
                    self.browser.shift_start = None;
                }
                self.navigate_back();
            }
            KeyCode::Char(' ') => {
                if self.screen == AppScreen::Browser && (self.browser.focused_pane == PaneType::Files || self.browser.focused_pane == PaneType::Directories) {
                    // Espacio con shift_start activo hace selección de rango
                    if let Some(start) = self.browser.shift_start {
                        self.browser.toggle_range_selection(start, self.browser.file_index);
                        self.browser.shift_start = None;
                    } else {
                        let is_dir = !self.browser.files.is_empty()
                            && self.browser.file_index < self.browser.files.len()
                            && self.browser.files[self.browser.file_index].is_dir;
                        if is_dir {
                            self.browser.toggle_folder_select(self.browser.file_index);
                        } else {
                            self.browser.toggle_select_highlighted();
                        }
                    }
                } else {
                    self.toggle_playback();
                }
            }
            KeyCode::Char('*') => {
                if self.screen == AppScreen::Browser {
                    let all_selected = !self.browser.files.is_empty()
                        && self.browser.files.iter().all(|f| f.is_selected);
                    if all_selected {
                        self.browser.clear_selections();
                    } else {
                        let paths: Vec<PathBuf> = self.browser.files.iter()
                            .map(|f| f.path.clone())
                            .collect();
                        for path in paths {
                            self.browser.selected_paths.insert(path);
                        }
                        for file in &mut self.browser.files {
                            file.is_selected = true;
                        }
                    }
                }
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                self.toggle_playback();
            }
            KeyCode::Char('n') => {
                if self.screen == AppScreen::Browser {
                    self.input_mode = InputMode::CreateFolder;
                    self.input_clear();
                } else if self.is_queue_search_active() {
                    if let Some(next_path) = self.next_search_result() {
                        self.audio.play(next_path);
                        self.sync_queue_selection();
                    }
                } else {
                    let shuffle = {
                        let state = self.audio.shared_state.lock().unwrap();
                        state.shuffle
                    };
                    if let Some(next_path) = self.queue.next(shuffle) {
                        self.audio.play(next_path);
                        self.sync_queue_selection();
                    }
                }
            }
            KeyCode::Char('b') => {
                if self.is_queue_search_active() {
                    if let Some(prev_path) = self.prev_search_result() {
                        self.audio.play(prev_path);
                        self.sync_queue_selection();
                    }
                } else {
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
            KeyCode::Char('q') => {
                if !self.browser.selected_paths.is_empty() {
                    let mut audio_paths: Vec<PathBuf> = Vec::new();
                    for p in self.browser.selected_paths.iter() {
                        if p.is_dir() {
                            for entry in walkdir::WalkDir::new(p)
                                .into_iter()
                                .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
                                .flatten()
                            {
                                if entry.file_type().is_file() && matches_audio_extension(entry.path()) {
                                    audio_paths.push(entry.path().to_path_buf());
                                }
                            }
                        } else if matches_audio_extension(p) {
                            audio_paths.push(p.clone());
                        }
                    }
                    audio_paths.sort();
                    self.queue.add_many(audio_paths);
                    self.browser.clear_selections();
                } else if self.screen == AppScreen::Browser {
                    match self.browser.focused_pane {
                        PaneType::Directories => {}
                        PaneType::Files => {
                            if !self.browser.files.is_empty() {
                                let item = &self.browser.files[self.browser.file_index];
                                if item.is_dir {
                                    // Si es carpeta, metemos todos los audios que encuentre adentro
                                    let mut paths = Vec::new();
                                    for entry in walkdir::WalkDir::new(&item.path)
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
                                        self.sync_queue_selection();
                                    }
                                } else {
                                    let path = item.path.clone();
                                    if matches_audio_extension(&path) {
                                        self.queue.add(path);
                                    }
                                }
                            }
                        }
                        PaneType::Preview => {}
                    }
                }
            }

            KeyCode::Char('a') => {
                if self.screen == AppScreen::Browser {
                    if !self.browser.selected_paths.is_empty() {
                        // Expand any selected folders to audio file paths
                        let mut audio_paths: Vec<PathBuf> = Vec::new();
                        for p in self.browser.selected_paths.iter() {
                            if p.is_dir() {
                                for entry in walkdir::WalkDir::new(p)
                                    .into_iter()
                                    .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
                                    .flatten()
                                {
                                    if entry.file_type().is_file() && matches_audio_extension(entry.path()) {
                                        audio_paths.push(entry.path().to_path_buf());
                                    }
                                }
                            } else if matches_audio_extension(p) {
                                audio_paths.push(p.clone());
                            }
                        }
                        audio_paths.sort();
                        self.library_pending_add_paths = audio_paths;
                        self.selected_add_collection_index = 0;
                        self.input_mode = InputMode::AddToCollectionList;
                    } else if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                        let (is_dir, item_path) = {
                            let item = &self.browser.files[self.browser.file_index];
                            (item.is_dir, item.path.clone())
                        };
                        if is_dir {
                            let in_library = self.config.music_folders.iter().any(|f| {
                                item_path.starts_with(resolve_path(f))
                            });
                            if !in_library {
                                self.notification = Some(("Folder not in Library — press 'm' to add it first".to_string(), 80));
                            } else {
                                let mut paths: Vec<PathBuf> = Vec::new();
                                for entry in walkdir::WalkDir::new(&item_path)
                                    .into_iter()
                                    .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
                                    .flatten()
                                {
                                    let p = entry.path().to_path_buf();
                                    if p.is_file() && matches_audio_extension(&p) {
                                        paths.push(p);
                                    }
                                }
                                if !paths.is_empty() {
                                    paths.sort();
                                    self.library_pending_add_paths = paths;
                                    self.selected_add_collection_index = 0;
                                    self.input_mode = InputMode::AddToCollectionList;
                                }
                            }
                        } else if matches_audio_extension(&item_path) {
                            self.library_pending_add_paths = vec![item_path];
                            self.selected_add_collection_index = 0;
                            self.input_mode = InputMode::AddToCollectionList;
                        }
                    }
                }
            }

            KeyCode::Char('/') => {
                self.input_mode = InputMode::Search;
                self.search.active = true;
                self.search.query.clear();
                let root = if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                    let item = &self.browser.files[self.browser.file_index];
                    if item.is_dir {
                        item.path.clone()
                    } else {
                        item.path.parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| self.browser.current_dir.clone())
                    }
                } else {
                    self.browser.current_dir.clone()
                };
                self.search.search_root = Some(root);
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
                        self.open_dest_browser(false);
                    }
                }
            }
            KeyCode::Char('y') => {
                if !self.browser.selected_paths.is_empty() {
                    self.open_dest_browser(true);
                }
            }
            KeyCode::Char('Y') => {
                if self.screen == AppScreen::Browser {
                    let paths = self.get_active_paths();
                    if !paths.is_empty() {
                        let _ = Self::copy_paths_to_clipboard(&paths);
                    }
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
            KeyCode::Char('<') | KeyCode::Char(',') => {
                if self.screen == AppScreen::Queue {
                    let mut current_decay = self.config.visualizer_decay;
                    current_decay = (current_decay - 0.02).max(0.50);
                    self.config.visualizer_decay = current_decay;
                    if let Ok(mut state) = self.audio.shared_state.lock() {
                        state.visualizer_decay = current_decay;
                    }
                    let _ = self.config.save();
                }
            }
            KeyCode::Char('>') | KeyCode::Char('.') => {
                if self.screen == AppScreen::Queue {
                    let mut current_decay = self.config.visualizer_decay;
                    current_decay = (current_decay + 0.02).min(0.99);
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
                        PaneType::Directories => {}
                        PaneType::Files => {
                            if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                                let target_path = self.browser.files[self.browser.file_index].path.clone();
                                let name = target_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                self.input_set(name);
                                self.rename_target = Some(target_path);
                                self.input_mode = InputMode::Rename;
                            }
                        }
                        PaneType::Preview => {}
                    }
                }
            }
            KeyCode::Char('m') => {
                if self.screen == AppScreen::Browser {
                    if let Some(item) = self.browser.files.get(self.browser.file_index) {
                        if item.is_dir {
                            let path = item.path.clone();
                            if self.config.add_music_folder(&path) {
                                self.library.start_scan(&self.config.music_folders);
                                self.notification = Some(("Added to Library — scanning now".to_string(), 50));
                            } else {
                                self.notification = Some(("Already in Library".to_string(), 50));
                            }
                        }
                    }
                }
            }
            KeyCode::Char('M') => {
                if self.screen == AppScreen::Browser {
                    self.jump_to_mtp();
                }
            }
            KeyCode::Char('E')
                if self.screen == AppScreen::Browser => {
                    self.jump_to_external_drives();
                }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                if self.search.query.trim().is_empty() {
                    self.search.active = false;
                }
            }
            KeyCode::Backspace => {
                self.search.query.pop();
                if self.screen == AppScreen::Browser {
                    let root = self.search.search_root.clone().unwrap_or_else(|| self.browser.current_dir.clone());
                    self.search.execute(&[root]);
                } else if self.screen == AppScreen::Queue {
                    self.queue_selected_index = 0;
                } else if self.screen == AppScreen::Library {
                    self.library.track_index = 0;
                }
            }
            KeyCode::Char(c) => {
                self.search.query.push(c);
                if self.screen == AppScreen::Browser {
                    let root = self.search.search_root.clone().unwrap_or_else(|| self.browser.current_dir.clone());
                    self.search.execute(&[root]);
                } else if self.screen == AppScreen::Queue {
                    self.queue_selected_index = 0;
                } else if self.screen == AppScreen::Library {
                    self.library.track_index = 0;
                }
            }
            _ => {}
        }
    }

    fn handle_create_folder_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let folder_name = self.input_value.trim().to_string();
                if !folder_name.is_empty() {
                    let new_dir_path = self.browser.current_dir.join(folder_name);
                    let _ = std::fs::create_dir_all(new_dir_path);
                    self.browser.refresh();
                }
                self.input_mode = InputMode::Normal;
                self.input_clear();
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.input_clear();
            }
            KeyCode::Backspace => {
                self.input_delete_before_cursor();
            }
            KeyCode::Delete => {
                self.input_delete_after_cursor();
            }
            KeyCode::Left => {
                self.input_move_left();
            }
            KeyCode::Right => {
                self.input_move_right();
            }
            KeyCode::Home => {
                self.input_cursor = 0;
            }
            KeyCode::End => {
                self.input_cursor = self.input_value.chars().count();
            }
            KeyCode::Char(c) => {
                self.input_insert_char(c);
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
                self.input_clear();
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.input_clear();
            }
            KeyCode::Backspace => {
                self.input_delete_before_cursor();
            }
            KeyCode::Delete => {
                self.input_delete_after_cursor();
            }
            KeyCode::Left => {
                self.input_move_left();
            }
            KeyCode::Right => {
                self.input_move_right();
            }
            KeyCode::Home => {
                self.input_cursor = 0;
            }
            KeyCode::End => {
                self.input_cursor = self.input_value.chars().count();
            }
            KeyCode::Char(c) => {
                self.input_insert_char(c);
            }
            _ => {}
        }
    }

    // Inserta un carácter respetando la posición del cursor (trabaja en char-indices, no bytes)
    fn input_insert_char(&mut self, c: char) {
        let byte_pos = self.input_value.char_indices()
            .nth(self.input_cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.input_value.len());
        self.input_value.insert(byte_pos, c);
        self.input_cursor += 1;
    }

    fn input_delete_before_cursor(&mut self) {
        if self.input_cursor > 0 {
            let byte_pos = self.input_value.char_indices()
                .nth(self.input_cursor - 1)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input_value.remove(byte_pos);
            self.input_cursor -= 1;
        }
    }

    fn input_delete_after_cursor(&mut self) {
        if self.input_cursor < self.input_value.chars().count() {
            let byte_pos = self.input_value.char_indices()
                .nth(self.input_cursor)
                .map(|(i, _)| i)
                .unwrap_or(self.input_value.len());
            self.input_value.remove(byte_pos);
        }
    }

    fn input_move_left(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor -= 1;
        }
    }

    fn input_move_right(&mut self) {
        if self.input_cursor < self.input_value.chars().count() {
            self.input_cursor += 1;
        }
    }

    fn input_clear(&mut self) {
        self.input_value.clear();
        self.input_cursor = 0;
    }

    fn input_set(&mut self, value: String) {
        self.input_cursor = value.chars().count();
        self.input_value = value;
    }

    fn handle_rename_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                if let Some(target) = self.rename_target.take() {
                    let new_name = self.input_value.trim().to_string();
                    if !new_name.is_empty() {
                        let mut new_path = target.clone();
                        new_path.set_file_name(&new_name);

                        if let Err(_e) = std::fs::rename(&target, &new_path) {
                        } else {
                            // Si el archivo renombrado estaba seleccionado, actualizamos la selección
                            if self.browser.selected_paths.contains(&target) {
                                self.browser.selected_paths.remove(&target);
                                self.browser.selected_paths.insert(new_path.clone());
                            }

                            self.browser.refresh();

                            if let Some(pos) = self.browser.files.iter().position(|f| f.path == new_path) {
                                self.browser.file_index = pos;
                            }
                        }
                    }
                }
                self.input_mode = InputMode::Normal;
                self.input_clear();
            }
            KeyCode::Esc => {
                self.rename_target = None;
                self.input_mode = InputMode::Normal;
                self.input_clear();
            }
            KeyCode::Backspace => {
                self.input_delete_before_cursor();
            }
            KeyCode::Delete => {
                self.input_delete_after_cursor();
            }
            KeyCode::Left => {
                self.input_move_left();
            }
            KeyCode::Right => {
                self.input_move_right();
            }
            KeyCode::Home => {
                self.input_cursor = 0;
            }
            KeyCode::End => {
                self.input_cursor = self.input_value.chars().count();
            }
            KeyCode::Char(c) => {
                self.input_insert_char(c);
            }
            _ => {}
        }
    }

    // Ensures any selected folders (or their parents) are indexed in the library.
    // Called right before clearing selections after a playlist add.
    fn ensure_selections_in_library(&mut self) {
        // Prefer the original folder selections; fall back to parent dirs of audio files.
        let mut folders: Vec<PathBuf> = self.browser.selected_paths.iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect();

        if folders.is_empty() {
            let mut seen = std::collections::HashSet::new();
            for p in &self.library_pending_add_paths {
                if let Some(parent) = p.parent() {
                    let parent = parent.to_path_buf();
                    if seen.insert(parent.clone()) {
                        folders.push(parent);
                    }
                }
            }
        }

        let mut added = false;
        for folder in folders {
            if self.config.add_music_folder(&folder) {
                added = true;
            }
        }
        if added {
            self.restart_library_watcher();
            self.library.start_scan(&self.config.music_folders);
            self.notification = Some(("Added to library — scanning now".to_string(), 60));
        }
    }

    fn handle_add_to_collection_list_key(&mut self, key: KeyEvent) {
        // While the "New playlist" inline input is active, handle text editing
        if self.add_coll_creating {
            match key.code {
                KeyCode::Enter => {
                    let name = self.input_value.trim().to_string();
                    if !name.is_empty() {
                        self.collections.create_collection(&name);
                        self.ensure_selections_in_library();
                        let paths = std::mem::take(&mut self.library_pending_add_paths);
                        if !paths.is_empty() {
                            self.collections.add_to_collection(&name, paths);
                        }
                        self.browser.clear_selections();
                    }
                    self.add_coll_creating = false;
                    self.input_clear();
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Esc => {
                    self.add_coll_creating = false;
                    self.input_clear();
                }
                KeyCode::Backspace => { self.input_delete_before_cursor(); }
                KeyCode::Delete    => { self.input_delete_after_cursor(); }
                KeyCode::Left      => { self.input_move_left(); }
                KeyCode::Right     => { self.input_move_right(); }
                KeyCode::Char(c)   => { self.input_insert_char(c); }
                _ => {}
            }
            return;
        }

        // Index 0 = virtual "New playlist" entry; 1..=N = existing collections
        let coll_names: Vec<String> = {
            let mut v: Vec<String> = self.collections.collections.keys().cloned().collect();
            v.sort();
            v
        };
        let total = coll_names.len() + 1; // +1 for "New playlist"

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_add_collection_index > 0 {
                    self.selected_add_collection_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_add_collection_index + 1 < total {
                    self.selected_add_collection_index += 1;
                }
            }
            KeyCode::Enter => {
                if self.selected_add_collection_index == 0 {
                    // Activate inline "New playlist" input
                    self.add_coll_creating = true;
                    self.input_clear();
                } else {
                    let idx = self.selected_add_collection_index - 1;
                    if idx < coll_names.len() {
                        let name = coll_names[idx].clone();
                        self.ensure_selections_in_library();
                        let paths = std::mem::take(&mut self.library_pending_add_paths);
                        if !paths.is_empty() {
                            self.collections.add_to_collection(&name, paths);
                        }
                        self.browser.clear_selections();
                    }
                    self.input_mode = InputMode::Normal;
                }
            }
            KeyCode::Esc => {
                self.library_pending_add_paths.clear();
                self.add_coll_creating = false;
                self.input_clear();
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_dest_browser_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.dest_browser = None;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Tab => {
                if let Some(ref mut db) = self.dest_browser {
                    db.focus = match db.focus {
                        DestBrowserFocus::Quick => DestBrowserFocus::Dirs,
                        DestBrowserFocus::Dirs => DestBrowserFocus::Quick,
                    };
                }
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                if let Some(ref db) = self.dest_browser {
                    let dest = db.current_dir.clone();
                    let is_move = db.is_move;
                    self.dest_browser = None;
                    self.last_operation_dest = dest.to_string_lossy().into_owned();
                    self.input_mode = InputMode::Normal;
                    self.start_file_operation(dest, is_move);
                }
            }
            _ => {
                let is_quick = self.dest_browser
                    .as_ref()
                    .map(|db| db.focus == DestBrowserFocus::Quick)
                    .unwrap_or(false);
                if is_quick {
                    self.dest_browser_quick_key(key);
                } else {
                    self.dest_browser_dirs_key(key);
                }
            }
        }
    }

    fn dest_browser_quick_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(ref mut db) = self.dest_browser
                    && db.quick_index > 0 { db.quick_index -= 1; }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(ref mut db) = self.dest_browser {
                    let n = db.quick_paths.len();
                    if db.quick_index + 1 < n { db.quick_index += 1; }
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                let dir = self.dest_browser
                    .as_ref()
                    .and_then(|db| db.quick_paths.get(db.quick_index))
                    .map(|(_, p)| p.clone());
                if let (Some(dir), Some(ref mut db)) = (dir, self.dest_browser.as_mut()) {
                    db.navigate_to(dir);
                    db.focus = DestBrowserFocus::Dirs;
                }
            }
            _ => {}
        }
    }

    fn dest_browser_dirs_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(ref mut db) = self.dest_browser { db.move_up(); }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(ref mut db) = self.dest_browser { db.move_down(); }
            }
            KeyCode::Enter => {
                let copy_here = self.dest_browser.as_ref().map(|db| db.dir_index == 0).unwrap_or(false);
                if copy_here {
                    if let Some(ref db) = self.dest_browser {
                        let dest = db.current_dir.clone();
                        let is_move = db.is_move;
                        self.dest_browser = None;
                        self.last_operation_dest = dest.to_string_lossy().into_owned();
                        self.input_mode = InputMode::Normal;
                        self.start_file_operation(dest, is_move);
                    }
                } else if let Some(ref mut db) = self.dest_browser {
                    if !db.dirs.is_empty() && !db.loading {
                        db.enter_highlighted();
                    }
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(ref mut db) = self.dest_browser
                    && db.dir_index > 0 && !db.dirs.is_empty() && !db.loading {
                        db.enter_highlighted();
                    }
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                if let Some(ref mut db) = self.dest_browser { db.go_up(); }
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

    // Inicia selección de rango con Shift+flecha; guarda el punto de inicio la primera vez
    fn browser_shift_navigate(&mut self, up: bool) {
        if self.browser.shift_start.is_none() {
            self.browser.shift_start = Some(self.browser.file_index);
        }
        if up {
            self.browser.move_up();
        } else {
            self.browser.move_down();
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
            AppScreen::Library => {}
            AppScreen::Healer => {}
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
            AppScreen::Library => {}
            AppScreen::Healer => {}
        }
    }

    fn navigate_left(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                match self.browser.focused_pane {
                    PaneType::Preview => {
                        self.browser.focused_pane = PaneType::Files;
                    }
                    PaneType::Files | PaneType::Directories => {
                        if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                            let item = self.browser.files[self.browser.file_index].clone();
                            if item.is_dir && item.is_expanded {
                                self.browser.expanded_paths.remove(&item.path);
                                self.browser.refresh();
                            } else {
                                // Si el item tiene profundidad, saltamos al padre visible en la lista
                                let mut found_parent = false;
                                if item.depth > 0 {
                                    for i in (0..self.browser.file_index).rev() {
                                        if self.browser.files[i].is_dir && self.browser.files[i].depth < item.depth {
                                            self.browser.file_index = i;
                                            found_parent = true;
                                            break;
                                        }
                                    }
                                }
                                if !found_parent {
                                    self.browser.go_to_parent();
                                }
                            }
                        } else {
                            self.browser.go_to_parent();
                        }
                    }
                }
                self.browser.refresh();
            }
            _ => {}
        }
    }

    fn navigate_right(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                match self.browser.focused_pane {
                    PaneType::Directories => {
                        self.browser.focused_pane = PaneType::Files;
                    }
                    PaneType::Files => {
                        if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                            let item = self.browser.files[self.browser.file_index].clone();
                            if item.is_dir {
                                if !item.is_expanded {
                                    self.browser.expanded_paths.insert(item.path.clone());
                                    self.browser.refresh();
                                } else if self.browser.file_index + 1 < self.browser.files.len() {
                                    self.browser.file_index += 1;
                                }
                            } else if self.has_preview() {
                                self.browser.focused_pane = PaneType::Preview;
                            }
                        }
                    }
                    PaneType::Preview => {}
                }
                self.browser.refresh();
            }
            _ => {}
        }
    }

    fn navigate_back(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.search.active {
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
            AppScreen::Library => {}
            AppScreen::Healer => {}
        }
    }

    fn activate_item(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if self.search.active {
                    if !self.search.results.is_empty() {
                        let path = self.search.results[self.search.selected_index].path.clone();
                        if matches_audio_extension(&path) {
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
                    if !self.browser.files.is_empty() {
                        let path = self.browser.files[self.browser.file_index].path.clone();
                        if self.browser.files[self.browser.file_index].is_dir {
                            let is_expanded = self.browser.files[self.browser.file_index].is_expanded;
                            if is_expanded {
                                self.browser.expanded_paths.remove(&path);
                            } else {
                                self.browser.expanded_paths.insert(path);
                            }
                            self.browser.refresh();
                            return;
                        }
                        if matches_audio_extension(&path) {
                            // Al dar Enter en un audio, cargamos todos los audios visibles como cola
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
            }
            AppScreen::Queue => {
                let filtered = self.get_filtered_queue_indices();
                if let Some(&(_, original_idx)) = filtered.get(self.queue_selected_index)
                    && original_idx < self.queue.items.len() {
                        let path = self.queue.items[original_idx].clone();
                        self.queue.current_index = Some(original_idx);
                        self.audio.play(path);
                        self.sync_queue_selection();
                    }
            }
            AppScreen::Library => {}
            AppScreen::Healer => {}
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

    // Sincroniza queue_selected_index con el track que está sonando actualmente
    pub fn sync_queue_selection(&mut self) {
        if let Some(idx) = self.queue.current_index {
            let filtered = self.get_filtered_queue_indices();
            if let Some(pos) = filtered.iter().position(|&(_, orig_idx)| orig_idx == idx) {
                self.queue_selected_index = pos;
            }
        }
    }

    // Actualiza los metadatos y estado de reproducción en MPRIS (systray / playerctl)
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
                    self.lyrics_scroll_offset = 0;
                    let cover_url = current_track.as_ref().and_then(|p| Self::find_cover_art(p));
                    self.current_cover_path = cover_url.as_ref()
                        .and_then(|url| url.strip_prefix("file://"))
                        .map(PathBuf::from);

                    if let Some(meta) = metadata {
                        let title = meta.title.as_deref();
                        let artist = meta.artist.as_deref();
                        let album = meta.album.as_deref();
                        let m_meta = souvlaki::MediaMetadata {
                            title,
                            artist,
                            album,
                            duration: Some(std::time::Duration::from_secs(duration)),
                            cover_url: cover_url.as_deref(),
                        };
                        let _ = controls.set_metadata(m_meta);
                    } else {
                        let title = current_track.as_ref()
                            .and_then(|p| p.file_name())
                            .map(|s| s.to_string_lossy().into_owned());
                        let m_meta = souvlaki::MediaMetadata {
                            title: title.as_deref(),
                            duration: Some(std::time::Duration::from_secs(duration)),
                            ..Default::default()
                        };
                        let _ = controls.set_metadata(m_meta);
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
                    if let Err(e) = controls.set_playback(playback)
                        && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("debug_mpris.log") {
                            use std::io::Write;
                            let _ = writeln!(f, "set_playback error: {:?}", e);
                        }

                    self.last_media_status = Some(status);
                    self.last_media_elapsed = elapsed;
                }
            }
        }
        self.update_discord_presence();
    }

    fn update_discord_presence(&mut self) {
        let (status, current_track, metadata, elapsed, duration) = {
            let state = self.audio.shared_state.lock().unwrap();
            (state.status, state.current_track.clone(), state.metadata.clone(), state.elapsed_secs, state.duration_secs)
        };

        let status_changed = Some(status) != self.last_discord_status;
        let track_changed = current_track != self.last_discord_track;

        if !status_changed && !track_changed {
            return;
        }

        match status {
            PlaybackStatus::Stopped => {
                self.discord.clear_activity();
            }
            PlaybackStatus::Playing | PlaybackStatus::Paused => {
                let title = metadata.as_ref()
                    .and_then(|m| m.title.clone())
                    .or_else(|| current_track.as_ref()
                        .and_then(|p| p.file_stem())
                        .map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "Unknown".to_string());
                let artist = metadata.as_ref()
                    .and_then(|m| m.artist.clone())
                    .unwrap_or_default();
                self.discord.set_activity(&title, &artist, elapsed, duration);
            }
        }

        self.last_discord_status = Some(status);
        self.last_discord_track = current_track;
    }

    // Busca carátula: primero en disco junto al track, luego embebida con lofty y la cachea
    pub fn find_cover_art(track_path: &Path) -> Option<String> {
        if let Some(parent) = track_path.parent() {
            let common_names = ["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "front.png", "Cover.jpg", "Cover.png", "Folder.jpg", "Folder.png"];
            for name in &common_names {
                let img_path = parent.join(name);
                if img_path.exists() && img_path.is_file() {
                    return Some(format!("file://{}", img_path.to_string_lossy()));
                }
            }

            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file()
                        && let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                            let lower = filename.to_lowercase();
                            if lower == "cover.jpg" || lower == "cover.png" || lower == "folder.jpg" || lower == "folder.png" || lower == "front.jpg" || lower == "front.png" {
                                return Some(format!("file://{}", path.to_string_lossy()));
                            }
                        }
                }
            }
        }

        // Si no hay imagen en disco, intentamos extraerla del tag del archivo y cacheamos en disco
        if let Ok(tagged_file) = Probe::open(track_path).and_then(|p| p.read())
            && let Some(tag) = tagged_file.primary_tag()
                && let Some(picture) = tag.pictures().first() {
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

                            if cached_path.exists() || std::fs::write(&cached_path, data).is_ok() {
                                return Some(format!("file://{}", cached_path.to_string_lossy()));
                            }
                        }
                    }
                }

        None
    }

    // Regresa los índices de la cola filtrados por búsqueda y respetando el orden shuffle
    // Cada tupla es (índice_visual_base, índice_original_en_items)
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

    fn is_queue_search_active(&self) -> bool {
        self.search.active && !self.search.query.is_empty()
    }

    fn next_search_result(&mut self) -> Option<PathBuf> {
        let filtered = self.get_filtered_queue_indices();
        if filtered.is_empty() {
            return None;
        }
        if let Some(curr) = self.queue.current_index {
            if let Some(pos) = filtered.iter().position(|(_, orig)| *orig == curr) {
                let next_pos = pos + 1;
                if next_pos < filtered.len() {
                    let next_orig = filtered[next_pos].1;
                    self.queue.current_index = Some(next_orig);
                    return Some(self.queue.items[next_orig].clone());
                }
                return None;
            }
        }
        let first_orig = filtered[0].1;
        self.queue.current_index = Some(first_orig);
        Some(self.queue.items[first_orig].clone())
    }

    fn prev_search_result(&mut self) -> Option<PathBuf> {
        let filtered = self.get_filtered_queue_indices();
        if filtered.is_empty() {
            return None;
        }
        if let Some(curr) = self.queue.current_index {
            if let Some(pos) = filtered.iter().position(|(_, orig)| *orig == curr) {
                if pos > 0 {
                    let prev_orig = filtered[pos - 1].1;
                    self.queue.current_index = Some(prev_orig);
                    return Some(self.queue.items[prev_orig].clone());
                }
                return None;
            }
        }
        let last_orig = filtered[filtered.len() - 1].1;
        self.queue.current_index = Some(last_orig);
        Some(self.queue.items[last_orig].clone())
    }

    pub fn is_file_operation_active(&self) -> bool {
        self.file_progress.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    fn save_session_on_quit(&mut self) {
        if self.stats_tracking.session_start_ts > 0 {
            let duration = self.stats_tracking.session_listen_secs;
            let tracks = self.stats_tracking.session_tracks;
            let start = self.stats_tracking.session_start_ts;
            self.stats.record_session(duration, tracks, start);
            let _ = self.stats.save();
        }
    }

    pub fn has_preview(&self) -> bool {
        if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
            let path = &self.browser.files[self.browser.file_index].path;
            matches_image_extension(path) || matches_text_extension(path)
        } else {
            false
        }
    }

    fn get_active_paths(&self) -> Vec<PathBuf> {
        if !self.browser.selected_paths.is_empty() {
            self.browser.selected_paths.iter().cloned().collect()
        } else {
            if !self.browser.files.is_empty() && self.browser.file_index < self.browser.files.len() {
                vec![self.browser.files[self.browser.file_index].path.clone()]
            } else {
                vec![]
            }
        }
    }

    fn url_decode(s: &str) -> Result<String, std::string::FromUtf8Error> {
        let mut bytes = Vec::new();
        let mut chars = s.as_bytes().iter().peekable();
        while let Some(&b) = chars.next() {
            if b == b'%'
                && let (Some(&h), Some(&l)) = (chars.next(), chars.next())
                    && let Ok(hex_str) = std::str::from_utf8(&[h, l])
                        && let Ok(val) = u8::from_str_radix(hex_str, 16) {
                            bytes.push(val);
                            continue;
                        }
            bytes.push(b);
        }
        String::from_utf8(bytes)
    }

    // Parsea texto de drag & drop que puede venir como file:// URLs, rutas crudas, con comillas o con espacios escapados
    fn parse_dropped_paths(text: &str) -> Vec<PathBuf> {
        let mut paths = Vec::new();

        for line in text.lines() {
            let cleaned = line.trim().to_string();
            if cleaned.is_empty() {
                continue;
            }

            let mut candidate = cleaned.clone();

            if (candidate.starts_with('\'') && candidate.ends_with('\'')) ||
               (candidate.starts_with('"') && candidate.ends_with('"')) {
                candidate = candidate[1..candidate.len()-1].to_string();
            }

            if candidate.starts_with("file://")
                && let Some(stripped) = candidate.strip_prefix("file://") {
                    candidate = stripped.to_string();
                }
            if candidate.contains('%')
                && let Ok(decoded) = Self::url_decode(&candidate) {
                    candidate = decoded;
                }

            let candidate_path = PathBuf::from(&candidate);
            if candidate_path.exists() {
                paths.push(candidate_path);
                continue;
            }

            // Si el path completo no existe, puede ser que la línea tenga múltiples rutas separadas por espacios
            let mut current = String::new();
            let mut in_single_quote = false;
            let mut in_double_quote = false;
            let mut escaped = false;
            let chars = line.chars().peekable();

            for c in chars {
                if escaped {
                    current.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '\'' && !in_double_quote {
                    in_single_quote = !in_single_quote;
                } else if c == '"' && !in_single_quote {
                    in_double_quote = !in_double_quote;
                } else if (c == ' ' || c == '\t') && !in_single_quote && !in_double_quote {
                    if !current.is_empty() {
                        let mut p_val = current.clone();
                        if p_val.starts_with("file://")
                            && let Some(stripped) = p_val.strip_prefix("file://") {
                                p_val = stripped.to_string();
                            }
                        if p_val.contains('%')
                            && let Ok(decoded) = Self::url_decode(&p_val) {
                                p_val = decoded;
                            }
                        let pb = PathBuf::from(p_val);
                        if pb.exists() {
                            paths.push(pb);
                        }
                        current.clear();
                    }
                } else {
                    current.push(c);
                }
            }
            if !current.is_empty() {
                let mut p_val = current;
                if p_val.starts_with("file://")
                    && let Some(stripped) = p_val.strip_prefix("file://") {
                        p_val = stripped.to_string();
                    }
                if p_val.contains('%')
                    && let Ok(decoded) = Self::url_decode(&p_val) {
                        p_val = decoded;
                    }
                let pb = PathBuf::from(p_val);
                if pb.exists() {
                    paths.push(pb);
                }
            }
        }

        paths
    }

    // Copia las rutas al portapapeles: primero via OSC 52 (funciona en cualquier terminal),
    // luego intenta wl-copy (Wayland) o xclip (X11) para que file managers también puedan pegar
    fn copy_paths_to_clipboard(paths: &[PathBuf]) -> Result<(), String> {
        if paths.is_empty() {
            return Err("No paths selected".to_string());
        }

        let mut plain_text = String::new();
        let mut uri_list = String::new();
        for path in paths {
            if let Ok(abs_path) = std::fs::canonicalize(path) {
                let path_str = abs_path.to_string_lossy().to_string();
                if !plain_text.is_empty() {
                    plain_text.push(' ');
                }
                plain_text.push_str(&path_str);

                uri_list.push_str("file://");
                uri_list.push_str(&path_str);
                uri_list.push('\n');
            }
        }

        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(plain_text.as_bytes());
        let osc52 = format!("\x1b]52;c;{}\x07", b64);
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(osc52.as_bytes());
        let _ = stdout.flush();

        let use_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
        if use_wayland {
            let child = std::process::Command::new("wl-copy")
                .args(["-t", "text/uri-list"])
                .stdin(std::process::Stdio::piped())
                .spawn();

            if let Ok(mut c) = child
                && c.stdin.as_mut().unwrap().write_all(uri_list.as_bytes()).is_ok() {
                    let _ = c.wait();
                }
        } else {
            let child = std::process::Command::new("xclip")
                .args(["-selection", "clipboard", "-t", "text/uri-list"])
                .stdin(std::process::Stdio::piped())
                .spawn();
            if let Ok(mut c) = child
                && c.stdin.as_mut().unwrap().write_all(uri_list.as_bytes()).is_ok() {
                    let _ = c.wait();
                }
        }

        Ok(())
    }

    fn start_drop_operation(&mut self, source_paths: Vec<PathBuf>, dest_dir: PathBuf) {
        if source_paths.is_empty() {
            return;
        }

        let total_items = source_paths.len();
        let total_bytes = Self::get_total_size(&source_paths);
        let progress = Arc::clone(&self.file_progress);
        let op_type = "Dropping".to_string();

        {
            let mut p = progress.lock().unwrap();
            *p = Some(FileOperationProgress {
                op_type: op_type.clone(),
                current_file: String::new(),
                completed_files: 0,
                total_files: total_items,
                bytes_copied: 0,
                total_bytes,
                finished: false,
                error: None,
                canceled: false,
                conflict_file: None,
                conflict_src_size: 0,
                conflict_dest_size: 0,
                conflict_action: None,
                replace_all: false,
                skip_all: false,
            });
        }

        thread::spawn(move || {
            if !dest_dir.exists()
                && let Err(e) = std::fs::create_dir_all(&dest_dir) {
                    let mut p = progress.lock().unwrap();
                    if let Some(ref mut state) = *p {
                        state.error = Some(e.to_string());
                        state.finished = true;
                    }
                    return;
                }

            let mut completed = 0;
            for path in source_paths {
                {
                    let p = progress.lock().unwrap();
                    if let Some(ref state) = *p
                        && state.canceled {
                            break;
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
                        Self::copy_file_with_progress(&path, &dest_path, &progress)
                    } else if path.is_dir() {
                        Self::copy_dir_recursive(&path, &dest_path, &progress)
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
                }

                completed += 1;
                {
                    let mut p = progress.lock().unwrap();
                    if let Some(ref mut state) = *p {
                        state.completed_files = completed;
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

    fn handle_paste(&mut self, content: String) {
        match self.input_mode {
            InputMode::CreateCollection
            | InputMode::AddToCollectionList
            | InputMode::Search
            | InputMode::Rename
            | InputMode::CreateFolder => {
                self.input_value.push_str(&content);
            }
            InputMode::CopyPath
            | InputMode::MovePath => {
            }
            InputMode::Normal
            | InputMode::ConfirmDelete
            | InputMode::TagEdit
            | InputMode::BulkTagEdit
            | InputMode::ConfirmDeletePlaylist
            | InputMode::ManageMusicFolders => {
                let paths = Self::parse_dropped_paths(&content);
                if !paths.is_empty() {
                    let dest_dir = self.browser.current_dir.clone();
                    self.start_drop_operation(paths, dest_dir);
                }
            }
        }
    }

    // Lanza la operación de copia/mover en un hilo separado con soporte de resolución de conflictos
    // El hilo worker se bloquea en el Condvar cuando hay un conflicto hasta que el UI thread responda
    fn start_file_operation(&mut self, dest_dir: PathBuf, is_move: bool) {
        // Deduplicate: if a directory is selected, don't also process its children
        // individually — copy_dir_recursive handles them. Without this, selecting a folder
        // and its contents (via toggle_folder_select) would copy everything twice.
        // Pre-collect dirs with N stat() calls; the filter then only does string prefix checks.
        let paths: Vec<PathBuf> = {
            let all: Vec<PathBuf> = self.browser.selected_paths.iter().cloned().collect();
            let selected_dirs: Vec<PathBuf> = all.iter().filter(|p| p.is_dir()).cloned().collect();
            if selected_dirs.is_empty() {
                all
            } else {
                all.into_iter()
                    .filter(|p| !selected_dirs.iter().any(|d| d != p && p.starts_with(d)))
                    .collect()
            }
        };
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
                bytes_copied: 0,
                total_bytes: 0, // computed in background thread to avoid blocking UI
                finished: false,
                error: None,
                canceled: false,
                conflict_file: None,
                conflict_src_size: 0,
                conflict_dest_size: 0,
                conflict_action: None,
                replace_all: false,
                skip_all: false,
            });
        }

        let condvar = Arc::clone(&self.conflict_condvar);

        thread::spawn(move || {
            if !dest_dir.exists()
                && let Err(e) = std::fs::create_dir_all(&dest_dir) {
                    let mut p = progress.lock().unwrap();
                    if let Some(ref mut state) = *p {
                        state.error = Some(e.to_string());
                        state.finished = true;
                    }
                    return;
                }

            // Compute total size here so the main thread is never blocked
            let total_bytes = Self::get_total_size(&paths);
            {
                let mut p = progress.lock().unwrap();
                if let Some(ref mut state) = *p {
                    state.total_bytes = total_bytes;
                }
            }

            let mut completed = 0;
            for path in paths {
                {
                    let p = progress.lock().unwrap();
                    if let Some(ref state) = *p
                        && state.canceled {
                            break;
                        }
                }

                if let Some(name) = path.file_name() {
                    let name_str = name.to_string_lossy().into_owned();
                    {
                        let mut p = progress.lock().unwrap();
                        if let Some(ref mut state) = *p {
                            state.current_file = name_str.clone();
                        }
                    }

                    let dest_path = dest_dir.join(name);

                    // Manejo de conflictos: si ya existe el destino, preguntamos al usuario
                    let mut skip_file = false;
                    if dest_path.exists() {
                        let (already_skip_all, already_replace_all) = {
                            let p = progress.lock().unwrap();
                            p.as_ref().map(|s| (s.skip_all, s.replace_all)).unwrap_or((false, false))
                        };
                        if already_skip_all {
                            skip_file = true;
                        } else if !already_replace_all {
                            let src_size = path.metadata().map(|m| m.len()).unwrap_or(0);
                            let dest_size = dest_path.metadata().map(|m| m.len()).unwrap_or(0);
                            {
                                let mut p = progress.lock().unwrap();
                                if let Some(ref mut state) = *p {
                                    state.conflict_file = Some(name_str.clone());
                                    state.conflict_src_size = src_size;
                                    state.conflict_dest_size = dest_size;
                                    state.conflict_action = None;
                                }
                            }
                            // Aquí nos quedamos dormidos esperando que el UI ponga la acción
                            let action = {
                                let guard = progress.lock().unwrap();
                                let mut guard = condvar
                                    .wait_while(guard, |p| {
                                        p.as_ref()
                                            .map(|s| s.conflict_action.is_none() && s.conflict_file.is_some())
                                            .unwrap_or(false)
                                    })
                                    .unwrap();
                                let action = guard.as_ref().and_then(|s| s.conflict_action.clone());
                                if let Some(ref mut state) = *guard {
                                    state.conflict_file = None;
                                    state.conflict_action = None;
                                    match &action {
                                        Some(ConflictAction::SkipAll) => state.skip_all = true,
                                        Some(ConflictAction::ReplaceAll) => state.replace_all = true,
                                        _ => {}
                                    }
                                }
                                action
                            };
                            match action {
                                Some(ConflictAction::Skip) | Some(ConflictAction::SkipAll) => {
                                    skip_file = true;
                                }
                                _ => {}
                            }
                        }
                    }

                    if skip_file {
                        completed += 1;
                        let mut p = progress.lock().unwrap();
                        if let Some(ref mut state) = *p {
                            state.completed_files = completed;
                        }
                        continue;
                    }
                    let file_size = Self::get_total_size(std::slice::from_ref(&path));
                    let res = if path.is_file() {
                        if is_move {
                            // Intentamos rename primero (atómico en mismo FS), si falla copiamos y borramos
                            std::fs::rename(&path, &dest_path).map(|_| {
                                let mut p = progress.lock().unwrap();
                                if let Some(ref mut state) = *p {
                                    state.bytes_copied += file_size;
                                }
                            }).or_else(|_| {
                                Self::copy_file_with_progress(&path, &dest_path, &progress).and_then(|_| {
                                    std::fs::remove_file(&path)
                                })
                            })
                        } else {
                            Self::copy_file_with_progress(&path, &dest_path, &progress)
                        }
                    } else if path.is_dir() {
                        if is_move {
                            std::fs::rename(&path, &dest_path).map(|_| {
                                let mut p = progress.lock().unwrap();
                                if let Some(ref mut state) = *p {
                                    state.bytes_copied += file_size;
                                }
                            }).or_else(|_| {
                                Self::copy_dir_recursive(&path, &dest_path, &progress).and_then(|_| {
                                    std::fs::remove_dir_all(&path)
                                })
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
        {
            if let Some(ref state) = *progress.lock().unwrap()
                && state.canceled {
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Operation canceled by user"));
                }
        }

        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let entry_path = entry.path();
            let dest_path = dst.join(entry.file_name());

            {
                if let Some(ref state) = *progress.lock().unwrap()
                    && state.canceled {
                        return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Operation canceled by user"));
                    }
            }

            if entry_path.is_dir() {
                Self::copy_dir_recursive(&entry_path, &dest_path, progress)?;
            } else {
                Self::copy_file_with_progress(&entry_path, &dest_path, progress)?;
            }
        }
        Ok(())
    }

    // Copia en bloques de 64KB actualizando el progreso por chunk para que la barra no se trabe
    fn copy_file_with_progress(
        src: &Path,
        dest: &Path,
        progress: &Arc<Mutex<Option<FileOperationProgress>>>,
    ) -> std::io::Result<()> {
        use std::fs::File;
        use std::io::{Read, Write};

        let mut src_file = File::open(src)?;
        let mut dest_file = File::create(dest)?;
        let mut buffer = vec![0; 64 * 1024];

        loop {
            {
                let p = progress.lock().unwrap();
                if let Some(ref state) = *p
                    && state.canceled {
                        return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Operation canceled by user"));
                    }
            }

            let bytes_read = src_file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            dest_file.write_all(&buffer[..bytes_read])?;

            {
                let mut p = progress.lock().unwrap();
                if let Some(ref mut state) = *p {
                    state.bytes_copied += bytes_read as u64;
                }
            }
        }
        Ok(())
    }

    fn get_total_size(paths: &[PathBuf]) -> u64 {
        let mut total = 0;
        for path in paths {
            if path.is_file() {
                if let Ok(meta) = std::fs::metadata(path) {
                    total += meta.len();
                }
            } else if path.is_dir() {
                total += Self::get_dir_size(path);
            }
        }
        total
    }

    fn get_dir_size(dir: &Path) -> u64 {
        let mut total = 0;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(meta) = std::fs::metadata(&path) {
                        total += meta.len();
                    }
                } else if path.is_dir() {
                    total += Self::get_dir_size(&path);
                }
            }
        }
        total
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
            AppScreen::Library => {}
            AppScreen::Healer => {}
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
            AppScreen::Library => {}
            AppScreen::Healer => {}
        }
    }

    fn navigate_home(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if !self.browser.files.is_empty() {
                    self.browser.file_index = 0;
                }
            }
            AppScreen::Queue => {
                let filtered = self.get_filtered_queue_indices();
                if !filtered.is_empty() {
                    self.queue_selected_index = 0;
                    self.sync_queue_selection();
                }
            }
            AppScreen::Library => {}
            AppScreen::Healer => {}
        }
    }

    fn navigate_end(&mut self) {
        match self.screen {
            AppScreen::Browser => {
                if !self.browser.files.is_empty() {
                    self.browser.file_index = self.browser.files.len() - 1;
                }
            }
            AppScreen::Queue => {
                let filtered = self.get_filtered_queue_indices();
                if !filtered.is_empty() {
                    self.queue_selected_index = filtered.len() - 1;
                    self.sync_queue_selection();
                }
            }
            AppScreen::Library => {}
            AppScreen::Healer => {}
        }
    }

    pub fn remove_queue_item(&mut self, index: usize) {
        if index < self.queue.items.len() {
            self.queue.items.remove(index);

            // Ajustamos current_index si apuntaba al item borrado o a uno posterior
            if let Some(curr) = self.queue.current_index {
                if curr == index {
                    self.audio.stop();
                    self.queue.current_index = None;
                } else if curr > index {
                    self.queue.current_index = Some(curr - 1);
                }
            }

            self.queue.shuffle_items();
            self.sync_queue_selection();
        }
    }

    pub fn delete_highlighted_queue_item(&mut self) {
        let filtered = self.get_filtered_queue_indices();
        if let Some(&(_, original_idx)) = filtered.get(self.queue_selected_index) {
            self.remove_queue_item(original_idx);

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
                    // Ojo: hay que actualizar current_index si el swap afectó el track en reproducción
                    if self.queue.current_index == Some(original_idx) {
                        self.queue.current_index = Some(original_idx - 1);
                    } else if self.queue.current_index == Some(original_idx - 1) {
                        self.queue.current_index = Some(original_idx);
                    }
                    self.queue_selected_index = self.queue_selected_index.saturating_sub(1);
                    self.queue.shuffle_items();
                }
            } else {
                if original_idx + 1 < self.queue.items.len() {
                    self.queue.items.swap(original_idx, original_idx + 1);
                    if self.queue.current_index == Some(original_idx) {
                        self.queue.current_index = Some(original_idx + 1);
                    } else if self.queue.current_index == Some(original_idx + 1) {
                        self.queue.current_index = Some(original_idx);
                    }
                    let max_len = self.get_filtered_queue_indices().len();
                    self.queue_selected_index = (self.queue_selected_index + 1).min(max_len.saturating_sub(1));
                    self.queue.shuffle_items();
                }
            }
        }
    }

    fn handle_library_normal_key(&mut self, key: KeyEvent) {
        // Bulk tag editor intercept
        if self.library.bulk_tag_editor.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.library.bulk_tag_editor = None;
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    if let Some(ref mut ed) = self.library.bulk_tag_editor {
                        ed.active_field = (ed.active_field + 1) % 5;
                        ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                    }
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    if let Some(ref mut ed) = self.library.bulk_tag_editor {
                        ed.active_field = if ed.active_field == 0 { 4 } else { ed.active_field - 1 };
                        ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                    }
                }
                KeyCode::Enter => {
                    self.input_mode = InputMode::BulkTagEdit;
                }
                KeyCode::Char('w') => {
                    self.do_write_bulk_tags();
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.do_write_bulk_tags();
                }
                _ => {}
            }
            return;
        }

        // Single tag editor takes priority when open — intercept navigation and editing keys
        if self.library.tag_editor.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.library.tag_editor = None;
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    if let Some(ref mut ed) = self.library.tag_editor {
                        ed.active_field = (ed.active_field + 1) % 6;
                        ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                    }
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    if let Some(ref mut ed) = self.library.tag_editor {
                        ed.active_field = if ed.active_field == 0 { 5 } else { ed.active_field - 1 };
                        ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                    }
                }
                KeyCode::Enter => {
                    self.input_mode = InputMode::TagEdit;
                }
                KeyCode::Char('w') => {
                    self.do_write_tags();
                }
                KeyCode::Char('n') => {
                    self.navigate_tag_editor(1);
                }
                KeyCode::Char('p') => {
                    self.navigate_tag_editor(-1);
                }
                _ => {}
            }
            return;
        }

        let filter_query = if self.search.active { self.search.query.clone() } else { String::new() };

        match key.code {
            KeyCode::Char('1') => {
                self.screen = AppScreen::Browser;
                self.search.active = false;
                self.search.query.clear();
            }
            KeyCode::Char('2') => {
                self.screen = AppScreen::Queue;
                self.search.active = false;
                self.search.query.clear();
                self.queue_selected_index = 0;
                self.sync_queue_selection();
            }
            KeyCode::Char('3') => {}
            KeyCode::Char('?') => { self.show_help = true; self.help_scroll = 0; }
            KeyCode::Tab | KeyCode::BackTab => {
                self.library.focused_panel = match self.library.focused_panel {
                    LibraryPanel::Playlists => LibraryPanel::Tracks,
                    LibraryPanel::Tracks => LibraryPanel::Playlists,
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                match self.library.focused_panel {
                    LibraryPanel::Playlists => {
                        if self.library.playlist_index > 0 {
                            self.library.playlist_index -= 1;
                            self.library.track_index = 0;
                        }
                    }
                    LibraryPanel::Tracks => {
                        if self.library.track_index > 0 {
                            self.library.track_index -= 1;
                        }
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                match self.library.focused_panel {
                    LibraryPanel::Playlists => {
                        let count = LibraryState::playlist_names(&self.collections).len();
                        if self.library.playlist_index + 1 < count {
                            self.library.playlist_index += 1;
                            self.library.track_index = 0;
                        }
                    }
                    LibraryPanel::Tracks => {
                        let count = self.library.visible_tracks(&self.collections, &filter_query, &self.stats).len();
                        if self.library.track_index + 1 < count {
                            self.library.track_index += 1;
                        }
                    }
                }
            }
            KeyCode::Home => {
                match self.library.focused_panel {
                    LibraryPanel::Playlists => self.library.playlist_index = 0,
                    LibraryPanel::Tracks => self.library.track_index = 0,
                }
            }
            KeyCode::End => {
                match self.library.focused_panel {
                    LibraryPanel::Playlists => {
                        let count = LibraryState::playlist_names(&self.collections).len();
                        if count > 0 { self.library.playlist_index = count - 1; }
                    }
                    LibraryPanel::Tracks => {
                        let count = self.library.visible_tracks(&self.collections, &filter_query, &self.stats).len();
                        if count > 0 { self.library.track_index = count - 1; }
                    }
                }
            }
            KeyCode::PageUp => {
                match self.library.focused_panel {
                    LibraryPanel::Playlists => {
                        self.library.playlist_index = self.library.playlist_index.saturating_sub(10);
                    }
                    LibraryPanel::Tracks => {
                        self.library.track_index = self.library.track_index.saturating_sub(10);
                    }
                }
            }
            KeyCode::PageDown => {
                match self.library.focused_panel {
                    LibraryPanel::Playlists => {
                        let count = LibraryState::playlist_names(&self.collections).len();
                        self.library.playlist_index = (self.library.playlist_index + 10).min(count.saturating_sub(1));
                    }
                    LibraryPanel::Tracks => {
                        let count = self.library.visible_tracks(&self.collections, &filter_query, &self.stats).len();
                        self.library.track_index = (self.library.track_index + 10).min(count.saturating_sub(1));
                    }
                }
            }
            KeyCode::Enter => {
                if self.library.focused_panel == LibraryPanel::Tracks {
                    let visible_paths: Vec<PathBuf> = self.library
                        .visible_tracks(&self.collections, &filter_query, &self.stats)
                        .iter().map(|t| t.path.clone()).collect();
                    let idx = self.library.track_index;
                    if let Some(path) = visible_paths.get(idx).cloned() {
                        self.queue.clear();
                        let audio_paths: Vec<PathBuf> = visible_paths.iter()
                            .filter(|p| matches_audio_extension(p))
                            .cloned().collect();
                        let play_idx = audio_paths.iter().position(|p| p == &path).unwrap_or(0);
                        for p in &audio_paths {
                            self.queue.add(p.clone());
                        }
                        self.queue.current_index = Some(play_idx);
                        self.audio.play(path);
                        self.sync_queue_selection();
                    }
                }
            }
            KeyCode::Char(' ') => {
                if self.library.focused_panel == LibraryPanel::Tracks {
                    let visible_paths: Vec<PathBuf> = self.library
                        .visible_tracks(&self.collections, &filter_query, &self.stats)
                        .iter().map(|t| t.path.clone()).collect();
                    if let Some(path) = visible_paths.get(self.library.track_index).cloned() {
                        if matches_audio_extension(&path) {
                            self.queue.add(path);
                        }
                    }
                } else {
                    self.toggle_playback();
                }
            }
            KeyCode::Char('e') => {
                if self.library.focused_panel == LibraryPanel::Tracks {
                    if !self.library.selected_tracks.is_empty() {
                        self.open_bulk_tag_editor();
                    } else {
                        self.open_tag_editor_for_selected();
                    }
                }
            }
            KeyCode::Char('m') => {
                if self.library.focused_panel == LibraryPanel::Tracks {
                    let now = std::time::Instant::now();
                    if self.m_hold_start.is_none() {
                        // First press — start hold timer, don't act yet
                        self.m_hold_start = Some(now);
                        self.m_last_press = Some(now);
                        self.m_select_all_triggered = false;
                        self.m_clear_all_triggered = false;
                    } else {
                        self.m_last_press = Some(now);
                        let elapsed = now.duration_since(self.m_hold_start.unwrap());
                        if self.m_select_all_triggered && !self.m_clear_all_triggered
                            && elapsed >= std::time::Duration::from_secs(3)
                        {
                            // Held 3s — clear all selection
                            self.library.selected_tracks.clear();
                            self.m_clear_all_triggered = true;
                        } else if !self.m_select_all_triggered
                            && elapsed >= std::time::Duration::from_secs(2)
                        {
                            // Held 2s — select all visible tracks
                            let filter_query = if self.search.active { self.search.query.clone() } else { String::new() };
                            let paths: Vec<PathBuf> = self.library
                                .visible_tracks(&self.collections, &filter_query, &self.stats)
                                .iter().map(|t| t.path.clone()).collect();
                            for p in paths {
                                self.library.selected_tracks.insert(p);
                            }
                            self.m_select_all_triggered = true;
                        }
                    }
                }
            }
            KeyCode::Char('E') => {
                if self.library.focused_panel == LibraryPanel::Tracks
                    && !self.library.selected_tracks.is_empty()
                {
                    self.open_bulk_tag_editor();
                }
            }
            KeyCode::Char('a') => {
                if self.library.focused_panel == LibraryPanel::Tracks {
                    let visible_paths: Vec<PathBuf> = self.library
                        .visible_tracks(&self.collections, &filter_query, &self.stats)
                        .iter().map(|t| t.path.clone()).collect();
                    if let Some(path) = visible_paths.get(self.library.track_index).cloned() {
                        self.library_pending_add_paths = vec![path];
                        self.selected_add_collection_index = 0;
                        self.input_mode = InputMode::AddToCollectionList;
                    }
                }
            }
            KeyCode::Char('N') => {
                if self.library.focused_panel == LibraryPanel::Playlists {
                    self.input_mode = InputMode::CreateCollection;
                    self.input_clear();
                }
            }
            KeyCode::Char('D') => {
                if self.library.focused_panel == LibraryPanel::Playlists {
                    let names = LibraryState::playlist_names(&self.collections);
                    if self.library.playlist_index > 0 {
                        if let Some(name) = names.get(self.library.playlist_index).cloned() {
                            if !is_smart_playlist(&name) {
                                self.pending_delete_playlist = Some(name);
                                self.input_mode = InputMode::ConfirmDeletePlaylist;
                            }
                        }
                    }
                }
            }
            KeyCode::Char('x') => {
                if self.library.focused_panel == LibraryPanel::Tracks {
                    self.library_remove_from_playlist();
                }
            }
            KeyCode::Char('s') => {
                self.library.sort = self.library.sort.next();
                self.library.track_index = 0;
            }
            KeyCode::Char('R') => {
                if self.library.scan_state != ScanState::Scanning {
                    self.library.start_scan(&self.config.music_folders);
                    self.notification = Some(("Library scan started".to_string(), 40));
                }
            }
            KeyCode::Char('F') => {
                self.manage_folders_index = 0;
                self.input_mode = InputMode::ManageMusicFolders;
            }
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Search;
                self.search.active = true;
                self.search.query.clear();
            }
            KeyCode::Esc | KeyCode::Backspace => {
                if self.search.active {
                    self.search.active = false;
                    self.search.query.clear();
                    let q = String::new();
                    self.library.clamp_track_index(&self.collections, &q, &self.stats);
                }
            }
            KeyCode::Char('p') | KeyCode::Char('P') => { self.toggle_playback(); }
            KeyCode::Char('n') => {
                let shuffle = { let s = self.audio.shared_state.lock().unwrap(); s.shuffle };
                if self.is_queue_search_active() {
                    if let Some(p) = self.next_search_result() { self.audio.play(p); self.sync_queue_selection(); }
                } else if let Some(p) = self.queue.next(shuffle) { self.audio.play(p); self.sync_queue_selection(); }
            }
            KeyCode::Char('b') => {
                let shuffle = { let s = self.audio.shared_state.lock().unwrap(); s.shuffle };
                if self.is_queue_search_active() {
                    if let Some(p) = self.prev_search_result() { self.audio.play(p); self.sync_queue_selection(); }
                } else if let Some(p) = self.queue.prev(shuffle) { self.audio.play(p); self.sync_queue_selection(); }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let v = { let s = self.audio.shared_state.lock().unwrap(); s.volume };
                let nv = v.saturating_add(5).min(100);
                self.audio.set_volume(nv);
                self.config.default_volume = nv;
                let _ = self.config.save();
            }
            KeyCode::Char('-') => {
                let v = { let s = self.audio.shared_state.lock().unwrap(); s.volume };
                let nv = v.saturating_sub(5);
                self.audio.set_volume(nv);
                self.config.default_volume = nv;
                let _ = self.config.save();
            }
            KeyCode::Char('r') => {
                let nr = {
                    let mut s = self.audio.shared_state.lock().unwrap();
                    s.repeat = match s.repeat {
                        RepeatMode::Off => RepeatMode::All,
                        RepeatMode::All => RepeatMode::One,
                        RepeatMode::One => RepeatMode::Off,
                    };
                    s.repeat
                };
                self.config.repeat = nr;
                let _ = self.config.save();
            }
            KeyCode::Char('z') => {
                let ns = {
                    let mut s = self.audio.shared_state.lock().unwrap();
                    s.shuffle = !s.shuffle;
                    s.shuffle
                };
                if ns { self.queue.shuffle_items(); }
                self.sync_queue_selection();
                self.config.shuffle = ns;
                let _ = self.config.save();
            }
            KeyCode::Char('H') => self.seek_relative(-5),
            KeyCode::Char('L') => self.seek_relative(5),
            _ => {}
        }
    }

    fn open_tag_editor_for_selected(&mut self) {
        let filter_query = if self.search.active { self.search.query.clone() } else { String::new() };
        let visible: Vec<LibraryTrack> = self.library
            .visible_tracks(&self.collections, &filter_query, &self.stats)
            .iter().map(|t| (*t).clone()).collect();
        let track_list: Vec<PathBuf> = visible.iter().map(|t| t.path.clone()).collect();
        let idx = self.library.track_index;
        if let Some(track) = visible.get(idx) {
            let editor = TagEditorState {
                path: track.path.clone(),
                fields: [
                    track.title.clone().unwrap_or_default(),
                    track.artist.clone().unwrap_or_default(),
                    track.album.clone().unwrap_or_default(),
                    track.track.map(|n| n.to_string()).unwrap_or_default(),
                    track.year.map(|n| n.to_string()).unwrap_or_default(),
                    track.genre.clone().unwrap_or_default(),
                ],
                active_field: 0,
                cursor_pos: 0,
                dirty: false,
                save_result: None,
                track_list,
                track_list_idx: idx,
            };
            self.library.tag_editor = Some(editor);
        }
    }

    fn navigate_tag_editor(&mut self, delta: i64) {
        // Save dirty state first
        if self.library.tag_editor.as_ref().map(|e| e.dirty).unwrap_or(false) {
            self.do_write_tags();
        }

        let (track_list, current_idx, active_field) = match self.library.tag_editor {
            Some(ref ed) => (ed.track_list.clone(), ed.track_list_idx, ed.active_field),
            None => return,
        };

        let new_idx = if delta > 0 {
            current_idx + 1
        } else {
            current_idx.saturating_sub(1)
        };

        if new_idx == current_idx || new_idx >= track_list.len() { return; }

        let path = track_list[new_idx].clone();
        if let Some(track) = self.library.tracks.iter().find(|t| t.path == path).cloned() {
            let editor = TagEditorState {
                path: track.path.clone(),
                fields: [
                    track.title.clone().unwrap_or_default(),
                    track.artist.clone().unwrap_or_default(),
                    track.album.clone().unwrap_or_default(),
                    track.track.map(|n| n.to_string()).unwrap_or_default(),
                    track.year.map(|n| n.to_string()).unwrap_or_default(),
                    track.genre.clone().unwrap_or_default(),
                ],
                active_field,
                cursor_pos: 0,
                dirty: false,
                save_result: None,
                track_list,
                track_list_idx: new_idx,
            };
            self.library.tag_editor = Some(editor);
            self.library.track_index = new_idx;
        }
    }

    fn open_bulk_tag_editor(&mut self) {
        let filter_query = if self.search.active { self.search.query.clone() } else { String::new() };
        let visible: Vec<LibraryTrack> = self.library
            .visible_tracks(&self.collections, &filter_query, &self.stats)
            .iter().map(|t| (*t).clone()).collect();

        // Collect selected paths that are in the visible list
        let paths: Vec<PathBuf> = visible.iter()
            .filter(|t| self.library.selected_tracks.contains(&t.path))
            .map(|t| t.path.clone())
            .collect();

        if paths.is_empty() { return; }

        self.library.bulk_tag_editor = Some(BulkTagEditorState {
            paths,
            fields: Default::default(),
            active_field: 0,
            cursor_pos: 0,
            dirty: false,
            save_result: None,
        });
    }

    fn do_write_bulk_tags(&mut self) {
        let editor = match self.library.bulk_tag_editor.take() {
            Some(e) => e,
            None => return,
        };

        let mut ok_count = 0usize;
        let mut first_error: Option<String> = None;

        for path in &editor.paths {
            match write_bulk_tag_to_path(path, &editor.fields) {
                Ok(()) => {
                    ok_count += 1;
                    // Update in-memory track
                    if let Some(t) = self.library.tracks.iter_mut().find(|t| &t.path == path) {
                        if !editor.fields[0].is_empty() { t.artist = Some(editor.fields[0].clone()); }
                        if !editor.fields[1].is_empty() { t.album = Some(editor.fields[1].clone()); }
                        if !editor.fields[2].is_empty() { t.track = editor.fields[2].parse::<u32>().ok(); }
                        if !editor.fields[3].is_empty() { t.year = editor.fields[3].parse::<u32>().ok(); }
                        if !editor.fields[4].is_empty() { t.genre = Some(editor.fields[4].clone()); }
                    }
                }
                Err(e) => {
                    if first_error.is_none() {
                        let fname = path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("?");
                        first_error = Some(format!("{}: {}", fname, e));
                    }
                }
            }
        }

        let err_count = editor.paths.len() - ok_count;
        let msg = if first_error.is_none() {
            format!("✓ Saved {} tracks", ok_count)
        } else if ok_count > 0 {
            format!("Saved {}/{} — error: {}", ok_count, editor.paths.len(), first_error.unwrap())
        } else {
            format!("Failed: {}", first_error.unwrap())
        };

        if err_count == 0 {
            self.library.selected_tracks.clear();
        }

        let mut new_editor = editor;
        new_editor.save_result = Some(msg);
        self.library.bulk_tag_editor = Some(new_editor);
    }

    fn do_write_tags(&mut self) {
        if let Some(ref mut editor) = self.library.tag_editor {
            write_tags(editor);
        }
        // Update in-memory track on success
        if let Some(ref editor) = self.library.tag_editor {
            if matches!(&editor.save_result, Some(Ok(()))) {
                let path = editor.path.clone();
                let new_title = editor.fields[0].clone();
                let new_artist = editor.fields[1].clone();
                let new_album = editor.fields[2].clone();
                let new_track = editor.fields[3].parse::<u32>().ok();
                let new_year = editor.fields[4].parse::<u32>().ok();
                let new_genre = editor.fields[5].clone();
                if let Some(t) = self.library.tracks.iter_mut().find(|t| t.path == path) {
                    t.title = if new_title.is_empty() { None } else { Some(new_title) };
                    t.artist = if new_artist.is_empty() { None } else { Some(new_artist) };
                    t.album = if new_album.is_empty() { None } else { Some(new_album) };
                    t.track = new_track;
                    t.year = new_year;
                    t.genre = if new_genre.is_empty() { None } else { Some(new_genre) };
                }
            }
        }
    }

    fn library_remove_from_playlist(&mut self) {
        let filter_query = if self.search.active { self.search.query.clone() } else { String::new() };
        let names = LibraryState::playlist_names(&self.collections);
        if self.library.playlist_index == 0 {
            return;
        }
        if let Some(playlist_name) = names.get(self.library.playlist_index).cloned() {
            if is_smart_playlist(&playlist_name) {
                return;
            }
            let visible_paths: Vec<PathBuf> = self.library
                .visible_tracks(&self.collections, &filter_query, &self.stats)
                .iter().map(|t| t.path.clone()).collect();
            if let Some(path) = visible_paths.get(self.library.track_index).cloned() {
                self.collections.remove_from_collection(&playlist_name, &path);
                let q = filter_query;
                self.library.clamp_track_index(&self.collections, &q, &self.stats);
            }
        }
    }

    fn handle_bulk_tag_edit_key(&mut self, key: KeyEvent) {
        if self.library.bulk_tag_editor.is_none() {
            self.input_mode = InputMode::Normal;
            return;
        }
        // Ctrl+S saves from anywhere in edit mode
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.input_mode = InputMode::Normal;
            self.do_write_bulk_tags();
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Tab => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor {
                    ed.active_field = (ed.active_field + 1) % 5;
                    ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                }
                // Stay in BulkTagEdit mode so typing continues on the next field
            }
            KeyCode::BackTab => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor {
                    ed.active_field = if ed.active_field == 0 { 4 } else { ed.active_field - 1 };
                    ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                }
                // Stay in BulkTagEdit mode
            }
            KeyCode::Left => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor {
                    if ed.cursor_pos > 0 { ed.cursor_pos -= 1; }
                }
            }
            KeyCode::Right => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor {
                    let len = ed.fields[ed.active_field].chars().count();
                    if ed.cursor_pos < len { ed.cursor_pos += 1; }
                }
            }
            KeyCode::Home => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor { ed.cursor_pos = 0; }
            }
            KeyCode::End => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor {
                    ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor {
                    let pos = ed.cursor_pos;
                    if pos > 0 {
                        let field = &mut ed.fields[ed.active_field];
                        let byte_pos = field.char_indices().nth(pos - 1).map(|(i, _)| i).unwrap_or(0);
                        field.remove(byte_pos);
                        ed.cursor_pos -= 1;
                        ed.dirty = true;
                    }
                }
            }
            KeyCode::Delete => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor {
                    let pos = ed.cursor_pos;
                    let field = &mut ed.fields[ed.active_field];
                    let len = field.chars().count();
                    if pos < len {
                        let byte_pos = field.char_indices().nth(pos).map(|(i, _)| i).unwrap_or(field.len());
                        field.remove(byte_pos);
                        ed.dirty = true;
                    }
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut ed) = self.library.bulk_tag_editor {
                    let pos = ed.cursor_pos;
                    let field = &mut ed.fields[ed.active_field];
                    let byte_pos = field.char_indices().nth(pos).map(|(i, _)| i).unwrap_or(field.len());
                    field.insert(byte_pos, c);
                    ed.cursor_pos += 1;
                    ed.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn handle_tag_edit_key(&mut self, key: KeyEvent) {
        if self.library.tag_editor.is_none() {
            self.input_mode = InputMode::Normal;
            return;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Tab => {
                if let Some(ref mut ed) = self.library.tag_editor {
                    ed.active_field = (ed.active_field + 1) % 6;
                    ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                }
            }
            KeyCode::BackTab => {
                if let Some(ref mut ed) = self.library.tag_editor {
                    ed.active_field = if ed.active_field == 0 { 5 } else { ed.active_field - 1 };
                    ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                }
            }
            KeyCode::Left => {
                if let Some(ref mut ed) = self.library.tag_editor {
                    if ed.cursor_pos > 0 { ed.cursor_pos -= 1; }
                }
            }
            KeyCode::Right => {
                if let Some(ref mut ed) = self.library.tag_editor {
                    let len = ed.fields[ed.active_field].chars().count();
                    if ed.cursor_pos < len { ed.cursor_pos += 1; }
                }
            }
            KeyCode::Home => {
                if let Some(ref mut ed) = self.library.tag_editor { ed.cursor_pos = 0; }
            }
            KeyCode::End => {
                if let Some(ref mut ed) = self.library.tag_editor {
                    ed.cursor_pos = ed.fields[ed.active_field].chars().count();
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut ed) = self.library.tag_editor {
                    let pos = ed.cursor_pos;
                    if pos > 0 {
                        let field = &mut ed.fields[ed.active_field];
                        let byte_pos = field.char_indices().nth(pos - 1).map(|(i, _)| i).unwrap_or(0);
                        field.remove(byte_pos);
                        ed.cursor_pos -= 1;
                        ed.dirty = true;
                    }
                }
            }
            KeyCode::Delete => {
                if let Some(ref mut ed) = self.library.tag_editor {
                    let pos = ed.cursor_pos;
                    let field = &mut ed.fields[ed.active_field];
                    let len = field.chars().count();
                    if pos < len {
                        let byte_pos = field.char_indices().nth(pos).map(|(i, _)| i).unwrap_or(field.len());
                        field.remove(byte_pos);
                        ed.dirty = true;
                    }
                }
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input_mode = InputMode::Normal;
                self.navigate_tag_editor(1);
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input_mode = InputMode::Normal;
                self.navigate_tag_editor(-1);
            }
            KeyCode::Char(c) => {
                if let Some(ref mut ed) = self.library.tag_editor {
                    let pos = ed.cursor_pos;
                    let field = &mut ed.fields[ed.active_field];
                    let byte_pos = field.char_indices().nth(pos).map(|(i, _)| i).unwrap_or(field.len());
                    field.insert(byte_pos, c);
                    ed.cursor_pos += 1;
                    ed.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn handle_confirm_delete_playlist_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Some(name) = self.pending_delete_playlist.take() {
                    self.collections.delete_collection(&name);
                    self.library.clamp_playlist_index(&self.collections);
                    self.library.track_index = 0;
                }
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending_delete_playlist = None;
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_manage_folders_key(&mut self, key: KeyEvent) {
        let len = self.config.music_folders.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.manage_folders_index > 0 {
                    self.manage_folders_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0 && self.manage_folders_index + 1 < len {
                    self.manage_folders_index += 1;
                }
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                if len > 0 && self.manage_folders_index < len {
                    self.config.remove_music_folder_at(self.manage_folders_index);
                    if self.manage_folders_index >= self.config.music_folders.len()
                        && self.manage_folders_index > 0
                    {
                        self.manage_folders_index -= 1;
                    }
                    self.restart_library_watcher();
                    self.library.start_scan(&self.config.music_folders);
                }
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    // ── Library Healer ────────────────────────────────────────────────────────

    fn handle_healer_key(&mut self, key: KeyEvent) {
        match self.healer.screen {
            HealerScreen::Menu     => self.handle_healer_menu_key(key),
            HealerScreen::Scanning => self.handle_healer_scanning_key(key),
            HealerScreen::Report   => self.handle_healer_report_key(key),
            HealerScreen::FileList => self.handle_healer_list_key(key),
            HealerScreen::Preview  => self.handle_healer_preview_key(key),
            HealerScreen::Editor   => self.handle_healer_editor_key(key),
        }
    }

    fn handle_healer_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('1') | KeyCode::Esc => {
                self.screen = AppScreen::Browser;
            }
            KeyCode::Char('2') => {
                self.screen = AppScreen::Queue;
                self.queue_selected_index = 0;
                self.sync_queue_selection();
            }
            KeyCode::Char('3') => {
                self.screen = AppScreen::Library;
                if self.library.scan_state == ScanState::Idle {
                    self.library.start_scan(&self.config.music_folders);
                }
            }
            KeyCode::Char('s') | KeyCode::Enter => {
                if self.healer.scan_state != HealScanState::Scanning {
                    self.healer_start_scan_library();
                }
            }
            KeyCode::Char('r') => {
                if self.healer.report.is_some() {
                    self.healer.screen = HealerScreen::Report;
                }
            }
            KeyCode::Char('f') => {
                if !self.healer.files.is_empty() {
                    self.healer.list_idx = 0;
                    self.healer.screen = HealerScreen::FileList;
                }
            }
            _ => {}
        }
    }

    fn handle_healer_scanning_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.healer.screen = HealerScreen::Menu;
        }
    }

    fn handle_healer_report_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.healer.screen = HealerScreen::Menu;
            }
            KeyCode::Char('1') => {
                self.screen = AppScreen::Browser;
            }
            KeyCode::Char('2') => {
                self.screen = AppScreen::Queue;
                self.queue_selected_index = 0;
                self.sync_queue_selection();
            }
            KeyCode::Char('3') => {
                self.screen = AppScreen::Library;
            }
            KeyCode::Enter | KeyCode::Char('f') => {
                if !self.healer.files.is_empty() {
                    self.healer.list_idx = 0;
                    self.healer.list_state.select(Some(0));
                    self.healer.screen = HealerScreen::FileList;
                }
            }
            KeyCode::Char('s') => {
                self.healer_start_scan_library();
            }
            _ => {}
        }
    }

    fn handle_healer_list_key(&mut self, key: KeyEvent) {
        // When search is active, route input to the search query
        if self.healer.search_active {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.healer.search_active = false;
                }
                KeyCode::Backspace => {
                    self.healer.search_query.pop();
                    self.healer.list_idx = 0;
                    self.healer.list_state.select(Some(0));
                }
                KeyCode::Char(c) => {
                    self.healer.search_query.push(c);
                    self.healer.list_idx = 0;
                    self.healer.list_state.select(Some(0));
                }
                _ => {}
            }
            return;
        }

        let file_count = self.healer.filtered_indices().len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if !self.healer.search_query.is_empty() {
                    self.healer.search_query.clear();
                    self.healer.list_idx = 0;
                    self.healer.list_state.select(Some(0));
                } else {
                    self.healer.screen = HealerScreen::Report;
                }
            }
            KeyCode::Char('/') => {
                self.healer.search_active = true;
            }
            KeyCode::Char('1') => { self.screen = AppScreen::Browser; }
            KeyCode::Char('2') => {
                self.screen = AppScreen::Queue;
                self.queue_selected_index = 0;
                self.sync_queue_selection();
            }
            KeyCode::Char('3') => { self.screen = AppScreen::Library; }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.healer.list_idx > 0 {
                    self.healer.list_idx -= 1;
                    self.healer.list_state.select(Some(self.healer.list_idx));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if file_count > 0 && self.healer.list_idx + 1 < file_count {
                    self.healer.list_idx += 1;
                    self.healer.list_state.select(Some(self.healer.list_idx));
                }
            }
            KeyCode::PageUp => {
                self.healer.list_idx = self.healer.list_idx.saturating_sub(10);
                self.healer.list_state.select(Some(self.healer.list_idx));
            }
            KeyCode::PageDown => {
                if file_count > 0 {
                    self.healer.list_idx = (self.healer.list_idx + 10).min(file_count - 1);
                    self.healer.list_state.select(Some(self.healer.list_idx));
                }
            }
            KeyCode::Enter => {
                self.healer.match_idx = 0;
                self.healer.screen = HealerScreen::Preview;
            }
            KeyCode::Char('l') => {
                self.healer_start_lookup();
            }
            KeyCode::Char('e') => {
                self.healer.load_editor_from_original();
                self.healer.screen = HealerScreen::Editor;
            }
            KeyCode::Char('s') => {
                if let Some(f) = self.healer.current_file_mut() {
                    f.status = HealStatus::Skipped;
                }
            }
            _ => {}
        }
    }

    fn handle_healer_preview_key(&mut self, key: KeyEvent) {
        let match_count = self.healer.current_file().map(|f| f.matches.len()).unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.healer.screen = HealerScreen::FileList;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.healer.match_idx > 0 { self.healer.match_idx -= 1; }
            }
            KeyCode::Right | KeyCode::Char('m') => {
                if match_count > 0 && self.healer.match_idx + 1 < match_count {
                    self.healer.match_idx += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char('a') => {
                self.healer_apply_match();
            }
            KeyCode::Char('e') => {
                self.healer.load_editor_from_match();
                self.healer.screen = HealerScreen::Editor;
            }
            KeyCode::Char('s') => {
                if let Some(f) = self.healer.current_file_mut() {
                    f.status = HealStatus::Skipped;
                }
                self.healer.screen = HealerScreen::FileList;
            }
            KeyCode::Char('L') => {
                self.healer_start_lookup();
            }
            _ => {}
        }
    }

    fn handle_healer_editor_key(&mut self, key: KeyEvent) {
        if self.healer.edit_typing {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.healer.edit_typing = false;
                }
                KeyCode::Char(c) => {
                    let idx = self.healer.edit_field_idx;
                    let cursor = self.healer.edit_cursor;
                    self.healer.edit_fields[idx].insert(cursor, c);
                    self.healer.edit_cursor += 1;
                }
                KeyCode::Backspace => {
                    let idx = self.healer.edit_field_idx;
                    let cursor = self.healer.edit_cursor;
                    if cursor > 0 {
                        self.healer.edit_fields[idx].remove(cursor - 1);
                        self.healer.edit_cursor -= 1;
                    }
                }
                KeyCode::Left  => { if self.healer.edit_cursor > 0 { self.healer.edit_cursor -= 1; } }
                KeyCode::Right => {
                    let len = self.healer.edit_fields[self.healer.edit_field_idx].chars().count();
                    if self.healer.edit_cursor < len { self.healer.edit_cursor += 1; }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc => { self.healer.screen = HealerScreen::Preview; }
            KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
                self.healer.edit_field_idx = (self.healer.edit_field_idx + 1) % 8;
                self.healer.edit_cursor = self.healer.edit_fields[self.healer.edit_field_idx].chars().count();
            }
            KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
                self.healer.edit_field_idx = if self.healer.edit_field_idx == 0 { 7 } else { self.healer.edit_field_idx - 1 };
                self.healer.edit_cursor = self.healer.edit_fields[self.healer.edit_field_idx].chars().count();
            }
            KeyCode::Enter => { self.healer.edit_typing = true; }
            KeyCode::Char('w') | KeyCode::Char('s') => {
                let m = self.healer.editor_as_match();
                self.healer_apply_match_data(m);
            }
            _ => {}
        }
    }

    fn healer_start_scan_library(&mut self) {
        let dirs: Vec<std::path::PathBuf> = self.config.music_folders.iter()
            .map(|f| crate::config::resolve_path(f))
            .collect();
        let mut paths = Vec::new();
        for dir in dirs {
            if !dir.is_dir() { continue; }
            for entry in walkdir::WalkDir::new(&dir)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .flatten()
            {
                let p = entry.path();
                if p.is_file() && crate::search::matches_audio_extension(p) {
                    paths.push(p.to_path_buf());
                }
            }
        }
        self.healer_start_scan_paths(paths);
    }

    fn healer_start_scan_paths(&mut self, paths: Vec<std::path::PathBuf>) {
        let slot: std::sync::Arc<std::sync::Mutex<Option<Vec<crate::healer::HealerFile>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        self.healer.pending_scan = Some(slot.clone());
        self.healer.scan_state = HealScanState::Scanning;
        self.healer.screen = HealerScreen::Scanning;
        let progress = self.healer.scan_progress.clone();
        pipeline::scan_files(paths, progress, slot);
    }

    fn healer_start_lookup(&mut self) {
        if self.healer.lookup_state == HealLookupState::Searching { return; }
        let file = match self.healer.current_file() { Some(f) => f, None => return };
        let title   = file.original.title.clone().unwrap_or_default();
        let artist  = file.original.artist.clone().unwrap_or_default();
        let path    = file.path.clone();
        let api_key = self.config.acoustid_api_key.clone().unwrap_or_default();

        let slot: std::sync::Arc<std::sync::Mutex<Option<Vec<crate::healer::HealMatch>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        self.healer.pending_lookup = Some(slot.clone());
        self.healer.lookup_state = HealLookupState::Searching;

        std::thread::spawn(move || {
            let mut all_matches = Vec::new();
            let mb = musicbrainz::search_recording(&title, &artist);
            all_matches.extend(mb);
            if !api_key.is_empty() {
                if let Some(fp) = fingerprint::compute(&path) {
                    all_matches.extend(fingerprint::lookup(&fp, &api_key));
                }
            }
            all_matches.sort_by(|a, b| b.confidence.cmp(&a.confidence));
            *slot.lock().unwrap() = Some(all_matches);
        });
    }

    fn healer_apply_match(&mut self) {
        let (path, m, original) = match self.healer.current_file() {
            Some(f) => {
                let m = match f.matches.get(self.healer.match_idx) {
                    Some(m) => m.clone(),
                    None => return,
                };
                (f.path.clone(), m, f.original.clone())
            }
            None => return,
        };
        self.healer_apply_match_at(path, m, original);
    }

    fn healer_apply_match_data(&mut self, m: crate::healer::HealMatch) {
        let (path, original) = match self.healer.current_file() {
            Some(f) => (f.path.clone(), f.original.clone()),
            None => return,
        };
        self.healer_apply_match_at(path, m, original);
    }

    fn healer_apply_match_at(&mut self, path: std::path::PathBuf, m: crate::healer::HealMatch, original: crate::healer::TagSnapshot) {
        match pipeline::apply_match(&path, &m, &mut self.healer_backup, &original) {
            Ok(()) => {
                // Patch the in-memory library track so the UI reflects the new tags immediately
                if let Some(track) = self.library.tracks.iter_mut().find(|t| t.path == path) {
                    if let Some(ref v) = m.tags.title  { track.title  = Some(v.clone()); }
                    if let Some(ref v) = m.tags.artist { track.artist = Some(v.clone()); }
                    if let Some(ref v) = m.tags.album  { track.album  = Some(v.clone()); }
                    if let Some(ref v) = m.tags.genre  { track.genre  = Some(v.clone()); }
                    if let Some(v) = m.tags.year       { track.year   = Some(v); }
                    if let Some(v) = m.tags.track      { track.track  = Some(v); }
                }
                // Remove the now-fixed file from the healer list
                if let Some(actual_idx) = self.healer.current_file_index() {
                    self.healer.files.remove(actual_idx);
                }
                let new_visible = self.healer.filtered_indices().len();
                if new_visible == 0 {
                    self.healer.list_idx = 0;
                    self.healer.list_state.select(None);
                } else {
                    if self.healer.list_idx >= new_visible {
                        self.healer.list_idx = new_visible - 1;
                    }
                    self.healer.list_state.select(Some(self.healer.list_idx));
                }
                self.notification = Some(("Tags applied successfully".to_string(), 40));
                self.healer.screen = HealerScreen::FileList;
            }
            Err(e) => {
                self.notification = Some((format!("Healer: {}", e), 80));
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
        let mut app = App::new(tx, None, None);

        app.queue.clear();
        app.queue.add(PathBuf::from("dance_electronic.mp3"));
        app.queue.add(PathBuf::from("ambient_chill.wav"));
        app.queue.add(PathBuf::from("rock_classic.flac"));

        let indices = app.get_filtered_queue_indices();
        assert_eq!(indices.len(), 3);
        assert_eq!(indices[0], (0, 0));
        assert_eq!(indices[1], (1, 1));
        assert_eq!(indices[2], (2, 2));

        app.search.active = true;
        app.search.query = "classic".to_string();

        let indices = app.get_filtered_queue_indices();
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0].0, 2);
        assert_eq!(indices[0].1, 2);
        assert_eq!(app.queue_items_len(), 1);

        app.search.query = "CHILL".to_string();
        let indices = app.get_filtered_queue_indices();
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0].0, 1);
        assert_eq!(indices[0].1, 1);
        assert_eq!(app.queue_items_len(), 1);

        app.search.query = "pop".to_string();
        assert_eq!(app.queue_items_len(), 0);

        app.search.query = "electronic".to_string();
        app.queue.shuffle_indices = vec![2, 0, 1];
        {
            let mut state = app.audio.shared_state.lock().unwrap();
            state.shuffle = true;
        }

        let indices = app.get_filtered_queue_indices();
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0], (1, 0));
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

        let mut app = App::new(tx, Some(test_root.clone()), None);

        app.browser.focused_pane = PaneType::Files;
        app.browser.file_index = 0;

        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));

        assert_eq!(app.input_mode, InputMode::Rename);
        assert_eq!(app.input_value, "test_file.mp3");
        assert_eq!(app.rename_target, Some(file_path.clone()));

        app.input_value = "renamed_file.mp3".to_string();

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.input_value.is_empty());
        assert_eq!(app.rename_target, None);

        assert!(!file_path.exists());
        let new_file_path = test_root.join("renamed_file.mp3");
        assert!(new_file_path.exists());

        assert_eq!(app.browser.files.len(), 1);
        assert_eq!(app.browser.files[0].name, "renamed_file.mp3");

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

        let mut app = App::new(tx, Some(test_root.clone()), None);

        assert_eq!(app.browser.files.len(), 1);
        assert_eq!(app.browser.files[0].name, "test_image.png");

        app.browser.focused_pane = PaneType::Files;
        app.browser.file_index = 0;

        assert!(matches_image_extension(&image_path));

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.browser.focused_pane, PaneType::Preview);

        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
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

        let mut app = App::new(tx, Some(test_root.clone()), None);

        assert_eq!(app.browser.files.len(), 1);
        assert_eq!(app.browser.files[0].name, "test_code.rs");

        app.browser.focused_pane = PaneType::Files;
        app.browser.file_index = 0;

        assert!(matches_text_extension(&text_path));
        assert!(matches_text_extension(std::path::Path::new("app.desktop")));

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.browser.focused_pane, PaneType::Preview);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 1);

        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 2);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 1);

        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 0);

        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 10);

        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.text_scroll_offset, 0);

        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
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

        let completed = App::autocomplete_path(&test_root.join("src_fo").to_string_lossy());
        assert!(completed.is_some());
        let val = completed.unwrap();
        assert!(val.contains("src_folder"));

        let mut app = App::new(tx, Some(test_root.clone()), None);
        app.browser.selected_paths.insert(src_dir.clone());

        app.start_file_operation(dest_dir.clone(), false);

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

        let mut app = App::new(tx, Some(test_root.clone()), None);
        app.browser.focused_pane = PaneType::Files;
        app.browser.file_index = 0;

        app.browser.selected_paths.insert(file_path.clone());

        assert_eq!(app.last_operation_dest, "");

        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(app.input_mode, InputMode::CopyPath);
        assert!(app.dest_browser.is_some());

        let dest_v = test_root.join("dest_v");
        app.dest_browser.as_mut().unwrap().current_dir = dest_v.clone();

        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

        assert_eq!(app.last_operation_dest, dest_v.to_string_lossy());

        std::thread::sleep(std::time::Duration::from_millis(100));

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        app.browser.selected_paths.insert(file_path.clone());
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.input_mode, InputMode::MovePath);

        assert_eq!(
            app.dest_browser.as_ref().unwrap().current_dir,
            PathBuf::from(&app.last_operation_dest)
        );

        let dest_y = test_root.join("dest_y");
        app.dest_browser.as_mut().unwrap().current_dir = dest_y.clone();
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

        assert_eq!(app.last_operation_dest, dest_y.to_string_lossy());

        let _ = std::fs::remove_dir_all(&test_root);
    }
}
