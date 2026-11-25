use std::{io::{BufRead, BufReader, Read, Write}, os::unix::{fs::PermissionsExt, net::{UnixListener, UnixStream}}, path::PathBuf, time::Duration};
use serde_json::{Value, json};
use crate::{app::App, events::Event, library::ScanState};

pub struct Server { listener: UnixListener, path: PathBuf }

impl Server {
    pub fn bind() -> std::io::Result<Self> {
        let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or(std::io::ErrorKind::NotFound)?).join("stash-player.sock");
        if path.exists() { std::fs::remove_file(&path)?; }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, path })
    }

    pub fn poll(&self, app: &mut App) {
        for _ in 0..8 {
            let Ok((mut stream, _)) = self.listener.accept() else { break; };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
            let result = read_request(&mut stream).and_then(|request| handle(app, &request));
            let reply = match result { Ok(value) => value, Err(error) => json!({"error":error}) };
            let _ = writeln!(stream, "{reply}");
            if app.should_quit { break; }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.path); }
}

fn read_request(stream: &mut UnixStream) -> Result<Value, String> {
    let mut line = String::new();
    BufReader::new(stream.take(4*1024*1024)).read_line(&mut line).map_err(|error| error.to_string())?;
    serde_json::from_str(&line).map_err(|error| error.to_string())
}

fn matching_paths(app: &App, query: &str) -> Vec<PathBuf> {
    let query = query.to_lowercase();
    app.library.tracks.iter().filter(|track| query.is_empty() ||
        format!("{} {} {} {}", track.title.as_deref().unwrap_or(""), track.artist.as_deref().unwrap_or(""),
            track.album.as_deref().unwrap_or(""), track.path.display()).to_lowercase().contains(&query))
        .map(|track| track.path.clone()).collect()
}

fn handle(app: &mut App, request: &Value) -> Result<Value, String> {
    if request["action"].as_str().is_some_and(|action| action == "state" || action.starts_with("native-")) {
        native_action(app, request)?;
        return Ok(native_snapshot(app, request));
    }
    if app.library.scan_state == ScanState::Idle { app.library.start_scan(&app.config.music_folders); }
    app.library.poll_scan();
    let query = request["query"].as_str().unwrap_or("");
    match request["action"].as_str().unwrap_or("snapshot") {
        "snapshot" => {},
        "play" => {
            let path = PathBuf::from(request["path"].as_str().ok_or("Select a track first")?);
            let paths = matching_paths(app, query);
            let index = paths.iter().position(|item| item == &path).ok_or("Track is no longer in this library view")?;
            app.queue.clear();
            app.queue.add_many(paths);
            app.queue.current_index = Some(index);
            app.audio.play(path);
        },
        "toggle" => app.handle_event(Event::MediaPlayPause),
        "pause" => app.handle_event(Event::MediaPause),
        "next" => app.handle_event(Event::MediaNext),
        "previous" => app.handle_event(Event::MediaPrev),
        "volume" => { app.audio.set_volume(request["value"].as_u64().unwrap_or(75).min(100) as u32); },
        "rescan" => app.library.start_scan(&app.config.music_folders),
        "add_folder" => {
            let folder = crate::config::resolve_path(request["path"].as_str().ok_or("Enter a folder path")?);
            if !folder.is_dir() { return Err("Music folder does not exist".to_string()); }
            app.config.add_music_folder(&folder);
            app.config.save().map_err(|error| error.to_string())?;
            app.library.start_scan(&app.config.music_folders);
        },
        _ => return Err("Unknown Stash request".to_string()),
    }
    let paths = matching_paths(app, query);
    let offset = request["offset"].as_u64().unwrap_or(0) as usize;
    let matching: std::collections::HashSet<&PathBuf> = paths.iter().collect();
    let tracks: Vec<Value> = app.library.tracks.iter().filter(|track| matching.contains(&track.path)).skip(offset).take(64)
        .map(|track| json!({"path":track.path,"title":track.title.clone().unwrap_or_else(||track.path.file_stem().unwrap_or_default().to_string_lossy().into_owned()),"artist":track.artist,"album":track.album})).collect();
    let state = app.audio.shared_state.lock().unwrap();
    Ok(json!({"pid":std::process::id(),"tracks":tracks,"total":paths.len(),"offset":offset,"scanning":app.library.scan_state==ScanState::Scanning,
        "folders":app.config.music_folders,"status":state.status,"volume":state.volume,
        "title":state.metadata.as_ref().and_then(|metadata|metadata.title.as_ref()),
        "artist":state.metadata.as_ref().and_then(|metadata|metadata.artist.as_ref()),
        "current_path":state.current_track,"elapsed":state.elapsed_secs,"duration":state.duration_secs}))
}

pub fn run(server: Server) -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new(tx, None, None);
    app.library.start_scan(&app.config.music_folders);
    while !app.should_quit {
        server.poll(&mut app);
        if app.should_quit { break; }
        for event in rx.try_iter().take(256) { app.handle_event(event); }
        app.handle_event(Event::Tick);
        std::thread::sleep(Duration::from_millis(33));
    }
    Ok(())
}

pub fn client_request(request: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("No desktop runtime directory")?).join("stash-player.sock");
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    BufReader::new(stream.take(8*1024*1024)).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

static REMOTE_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn remote_mode() -> bool { REMOTE_MODE.load(std::sync::atomic::Ordering::Relaxed) }
pub fn set_remote_mode() { REMOTE_MODE.store(true, std::sync::atomic::Ordering::Relaxed); }

fn native_action(app: &mut App, request: &Value) -> Result<(), String> {
    use crate::audio::AudioCommand;
    match request["action"].as_str().unwrap_or("state") {
        "state" => {},
        "native-play" => {
            let path = PathBuf::from(request["path"].as_str().ok_or("Missing audio path")?);
            if !path.is_file() || !crate::search::matches_audio_extension(&path) { return Err("Select an existing audio file".into()); }
            app.queue.add(path.clone());
            app.queue.current_index = app.queue.items.iter().position(|item| item == &path);
            app.audio.play(path);
        },
        "native-queue" => {
            let paths: Vec<PathBuf> = serde_json::from_value(request["paths"].clone()).map_err(|error| error.to_string())?;
            if paths.iter().any(|path| !crate::search::matches_audio_extension(path)) { return Err("Queue contains a non-audio path".into()); }
            let shuffle_indices: Vec<usize> = serde_json::from_value(request["shuffle_indices"].clone()).unwrap_or_default();
            let mut sorted = shuffle_indices.clone();
            sorted.sort_unstable();
            if !shuffle_indices.is_empty() && sorted != (0..paths.len()).collect::<Vec<_>>() {
                return Err("Invalid shuffle order".into());
            }
            app.queue.clear(); app.queue.add_many(paths);
            app.queue.shuffle_indices = shuffle_indices;
            app.queue.current_index = request["index"].as_u64().map(|index| index as usize).filter(|index| *index < app.queue.items.len());
        },
        "native-pause" => app.handle_event(Event::MediaPause),
        "native-resume" => app.handle_event(Event::MediaPlay),
        "native-stop" => app.audio.stop(),
        "native-shutdown" => app.quit(),
        "native-volume" => {
            let volume=request["value"].as_u64().unwrap_or(75).min(100) as u32;
            app.audio.set_volume(volume);app.config.default_volume=volume;
            app.config.save().map_err(|error|error.to_string())?;
        },
        "native-seek" => app.audio.seek(Duration::from_millis(request["position"].as_u64().unwrap_or(0))),
        "native-radio" => {
            let station = crate::radio::STATIONS.iter().find(|station| Some(station.slug) == request["slug"].as_str()).ok_or("Unknown radio station")?;
            let _ = app.audio.command_tx.send(AudioCommand::PlayStream(station.clone()));
        },
        "native-settings" => {
            let mut state = app.audio.shared_state.lock().unwrap();
            state.repeat = serde_json::from_value(request["repeat"].clone()).map_err(|error|error.to_string())?;
            state.shuffle = request["shuffle"].as_bool().unwrap_or(false);
            state.visualizer_decay = request["decay"].as_f64().unwrap_or(0.88).clamp(0.0,1.0) as f32;
            app.config.repeat=state.repeat; app.config.shuffle=state.shuffle; app.config.visualizer_decay=state.visualizer_decay;
            drop(state);
            if let Ok(folders) = serde_json::from_value::<Vec<String>>(request["folders"].clone()) {
                if folders != app.config.music_folders { app.config.music_folders=folders; app.library.start_scan(&app.config.music_folders); }
            }
            app.config.save().map_err(|error|error.to_string())?;
        },
        _ => return Err("Unknown native player action".into()),
    }
    if app.config.shuffle && app.queue.shuffle_indices.len() != app.queue.items.len() {
        app.queue.shuffle_items();
    }
    Ok(())
}

fn queue_version(app: &App) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    app.queue.items.hash(&mut hash); app.queue.current_index.hash(&mut hash); app.queue.shuffle_indices.hash(&mut hash); hash.finish()
}

fn native_snapshot(app: &App, request: &Value) -> Value {
    use crate::models::LyricsState;
    let state=app.audio.shared_state.lock().unwrap();
    let radio=state.radio.as_ref().map(|radio|json!({"slug":radio.station.slug,"connecting":radio.connecting,"session":radio.session,
        "songs":radio.songs.iter().map(|song|json!({"title":song.title,"artist":song.artist,"album":song.album})).collect::<Vec<_>>()}));
    let lyrics=match &state.lyrics_state { LyricsState::Found(text)=>Some(text.clone()), _=>None };
    let version=queue_version(app);
    let mut result=json!({"protocol":2,"pid":std::process::id(),"current_path":state.current_track,"metadata":state.metadata,"status":state.status,
        "volume":state.volume,"elapsed":state.elapsed_secs,"elapsed_millis":state.elapsed_millis,"duration":state.duration_secs,
        "repeat":state.repeat,"shuffle":state.shuffle,"decay":state.visualizer_decay,"radio":radio,"lyrics":lyrics,
        "visualizer_data":state.visualizer_data,"visualizer_peaks":state.visualizer_peaks,"left_level":state.left_level,"right_level":state.right_level,
        "device_error":state.device_error,"queue_version":version,"folders":app.config.music_folders});
    if request["include_queue"].as_bool()==Some(true) && request["queue_version"].as_u64()!=Some(version) {
        result["queue"]=json!(app.queue.items);result["queue_index"]=json!(app.queue.current_index);result["shuffle_indices"]=json!(app.queue.shuffle_indices);
    }
    result
}

fn apply_audio(state: &mut crate::audio::AudioSharedState, snapshot: &Value) {
    use crate::models::LyricsState;
    if snapshot.get("status").is_none() { return; }
    state.current_track=serde_json::from_value(snapshot["current_path"].clone()).unwrap_or(None);
    state.metadata=serde_json::from_value(snapshot["metadata"].clone()).unwrap_or(None);
    state.status=serde_json::from_value(snapshot["status"].clone()).unwrap_or(crate::models::PlaybackStatus::Stopped);
    state.volume=snapshot["volume"].as_u64().unwrap_or(75) as u32;
    state.elapsed_secs=snapshot["elapsed"].as_u64().unwrap_or(0);state.elapsed_millis=snapshot["elapsed_millis"].as_u64().unwrap_or(0);
    state.duration_secs=snapshot["duration"].as_u64().unwrap_or(0);
    state.repeat=serde_json::from_value(snapshot["repeat"].clone()).unwrap_or_default();state.shuffle=snapshot["shuffle"].as_bool().unwrap_or(false);
    state.visualizer_decay=snapshot["decay"].as_f64().unwrap_or(0.88) as f32;
    state.visualizer_data=serde_json::from_value(snapshot["visualizer_data"].clone()).unwrap_or_else(|_|vec![0.0;160]);
    state.visualizer_peaks=serde_json::from_value(snapshot["visualizer_peaks"].clone()).unwrap_or_else(|_|vec![0.0;160]);
    state.left_level=snapshot["left_level"].as_f64().unwrap_or(0.0) as f32;state.right_level=snapshot["right_level"].as_f64().unwrap_or(0.0) as f32;
    state.device_error=snapshot["device_error"].as_str().map(str::to_string);
    state.lyrics_state=snapshot["lyrics"].as_str().map(|text|LyricsState::Found(text.to_string())).unwrap_or(LyricsState::NotFound);
    state.radio=snapshot["radio"]["slug"].as_str().and_then(|slug|crate::radio::STATIONS.iter().find(|station|station.slug==slug)).map(|station|crate::audio::RadioState {
        station:station.clone(),connecting:snapshot["radio"]["connecting"].as_bool().unwrap_or(false),session:snapshot["radio"]["session"].as_u64().unwrap_or(0),
        songs:snapshot["radio"]["songs"].as_array().map(|songs|songs.iter().map(|song|crate::radio::RadioSong {
            title:song["title"].as_str().unwrap_or("").into(),artist:song["artist"].as_str().unwrap_or("").into(),album:song["album"].as_str().unwrap_or("").into(),
        }).collect()).unwrap_or_default(),
    });
}

pub fn remote_audio(rx: std::sync::mpsc::Receiver<crate::audio::AudioCommand>, state: std::sync::Arc<std::sync::Mutex<crate::audio::AudioSharedState>>) {
    use crate::audio::AudioCommand;
    std::thread::spawn(move || loop {
        let request=match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(AudioCommand::Shutdown) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected)=>break,
            Ok(AudioCommand::Play(path))=>json!({"action":"native-play","path":path}),
            Ok(AudioCommand::PlayStream(station))=>json!({"action":"native-radio","slug":station.slug}),
            Ok(AudioCommand::Pause)=>json!({"action":"native-pause"}),
            Ok(AudioCommand::Resume)=>json!({"action":"native-resume"}),
            Ok(AudioCommand::Stop)=>json!({"action":"native-stop"}),
            Ok(AudioCommand::SetVolume(volume))=>json!({"action":"native-volume","value":volume}),
            Ok(AudioCommand::Seek(position))=>json!({"action":"native-seek","position":position.as_millis() as u64}),
            Ok(AudioCommand::StreamReady(_, _)) | Err(std::sync::mpsc::RecvTimeoutError::Timeout)=>json!({"action":"state"}),
        };
        match client_request(&request) {
            Ok(snapshot)=>{
                let mut state=state.lock().unwrap();
                if let Some(error)=snapshot["error"].as_str() { state.device_error=Some(error.into()); }
                else { apply_audio(&mut state,&snapshot); }
            },
            Err(error)=>state.lock().unwrap().device_error=Some(format!("Shared Stash player: {error}")),
        }
    });
}

pub struct RemoteView {
    queue:Vec<PathBuf>,index:Option<usize>,shuffle_indices:Vec<usize>,settings:Value,version:Option<u64>,last:std::time::Instant,
}
impl RemoteView {
    pub fn attach(app:&mut App)->Result<Self,Box<dyn std::error::Error>> {
        if !app.queue.items.is_empty() { client_request(&json!({"action":"native-queue","paths":app.queue.items,"index":app.queue.current_index,"shuffle_indices":app.queue.shuffle_indices}))?; }
        let mut view=Self { queue:app.queue.items.clone(),index:app.queue.current_index,shuffle_indices:app.queue.shuffle_indices.clone(),settings:Value::Null,version:None,last:std::time::Instant::now()-Duration::from_secs(1) };
        view.refresh(app)?;Ok(view)
    }
    fn settings(app:&App)->Value { json!({"repeat":app.config.repeat,"shuffle":app.config.shuffle,"decay":app.config.visualizer_decay,"folders":app.config.music_folders}) }
    pub fn poll(&mut self, app:&mut App) {
        if self.last.elapsed()<Duration::from_millis(100) { return; }
        if self.queue!=app.queue.items || self.index!=app.queue.current_index || self.shuffle_indices!=app.queue.shuffle_indices {
            if let Err(error)=client_request(&json!({"action":"native-queue","paths":app.queue.items,"index":app.queue.current_index,"shuffle_indices":app.queue.shuffle_indices})) { app.notification=Some((error.to_string(),80)); }
        }
        let settings=Self::settings(app);
        if settings!=self.settings {
            let mut request=settings.clone();request["action"]="native-settings".into();
            if let Err(error)=client_request(&request) { app.notification=Some((error.to_string(),80)); }
        }
        if let Err(error)=self.refresh(app) { app.notification=Some((format!("Shared player: {error}"),80)); }
    }
    fn refresh(&mut self,app:&mut App)->Result<(),Box<dyn std::error::Error>> {
        let snapshot=client_request(&json!({"action":"state","include_queue":true,"queue_version":self.version}))?;
        if let Some(error)=snapshot["error"].as_str() { return Err(error.to_string().into()); }
        if let Some(queue)=snapshot.get("queue") {
            app.queue.items=serde_json::from_value(queue.clone())?;
            app.queue.current_index=serde_json::from_value(snapshot["queue_index"].clone())?;
            app.queue.shuffle_indices=serde_json::from_value(snapshot["shuffle_indices"].clone()).unwrap_or_default();
        }
        app.config.repeat=serde_json::from_value(snapshot["repeat"].clone()).unwrap_or_default();
        app.config.shuffle=snapshot["shuffle"].as_bool().unwrap_or(false);
        app.config.visualizer_decay=snapshot["decay"].as_f64().unwrap_or(0.88) as f32;
        app.config.music_folders=serde_json::from_value(snapshot["folders"].clone()).unwrap_or_default();
        apply_audio(&mut app.audio.shared_state.lock().unwrap(),&snapshot);
        if snapshot.get("queue").is_some() { app.sync_queue_selection(); }
        self.version=snapshot["queue_version"].as_u64();self.queue=app.queue.items.clone();self.index=app.queue.current_index;self.shuffle_indices=app.queue.shuffle_indices.clone();
        self.settings=Self::settings(app);self.last=std::time::Instant::now();Ok(())
    }
}

pub fn ensure_compatible_service() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::{fs::MetadataExt, process::CommandExt};
    let deadline=std::time::Instant::now()+Duration::from_secs(5);
    let snapshot=loop {
        match client_request(&json!({"action":"state"})) {
            Ok(value)=>break value,
            Err(error) if std::time::Instant::now()>=deadline=>return Err(error),
            Err(_)=>std::thread::sleep(Duration::from_millis(50)),
        }
    };
    if snapshot.get("status").is_some() && snapshot.get("error").is_none() { return Ok(()); }
    if snapshot["error"].as_str()!=Some("Unknown Stash request") {
        return Err(snapshot["error"].as_str().unwrap_or("Shared Stash player is unavailable").to_string().into());
    }
    let legacy=client_request(&json!({"action":"snapshot"}))?;
    let pid=legacy["pid"].as_u64().ok_or("Could not identify the old Stash player")?;
    let process=PathBuf::from(format!("/proc/{pid}"));
    let command=std::fs::read(process.join("cmdline"))?;
    let arguments:Vec<&[u8]>=command.split(|byte|*byte==0).filter(|value|!value.is_empty()).collect();
    if std::fs::metadata(&process)?.uid()!=std::fs::metadata("/proc/self")?.uid()
        || !arguments.iter().any(|argument|*argument==b"--service")
        || arguments.first().and_then(|argument|std::str::from_utf8(argument).ok()).and_then(|argument|std::path::Path::new(argument).file_name()).is_none_or(|name|name!="stash") {
        return Err("Restart the old Stash window once to load the updated shared player.".into());
    }
    if legacy["status"].as_str()!=Some("Stopped") {
        return Err(format!("An older Stash background player is active. Playback was left running. To stop that old player, run `kill {pid}` once, then run `stash` again.").into());
    }
    let latest=client_request(&json!({"action":"snapshot"}))?;
    if latest["pid"].as_u64()!=Some(pid) || latest["status"].as_str()!=Some("Stopped") {
        return Err("Stash playback changed during the update check. Run stash again.".into());
    }
    let status=std::process::Command::new("kill").args(["-TERM",&pid.to_string()]).status()?;
    if !status.success() { return Err("Could not stop the outdated Stash service".into()); }
    let runtime=PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("No desktop runtime directory")?);
    let lock=std::fs::OpenOptions::new().read(true).write(true).open(runtime.join("stash-player.lock"))?;
    let deadline=std::time::Instant::now()+Duration::from_secs(5);
    loop {
        match lock.try_lock() {
            Ok(())=>{lock.unlock()?;break;},
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(50)),
            Err(error)=>return Err(error.into()),
        }
    }
    let mut child=std::process::Command::new(std::env::current_exe()?).arg("--service")
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .process_group(0).spawn()?;
    std::thread::spawn(move||{let _=child.wait();});
    let deadline=std::time::Instant::now()+Duration::from_secs(5);
    while std::time::Instant::now()<deadline {
        if let Ok(value)=client_request(&json!({"action":"state"})) {
            if value.get("status").is_some() && value.get("error").is_none() {
                let result=client_request(&json!({"action":"native-volume","value":legacy["volume"].as_u64().unwrap_or(75)}))?;
                if let Some(error)=result["error"].as_str() {return Err(error.to_string().into());}
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("The Stash service was updated but has not become ready. Run stash again.".into())
}

#[cfg(test)]
mod shuffle_tests {
    use super::*;

    #[test]
    fn native_shutdown_stops_audio_and_exits_the_player() {
        let root = tempfile::tempdir().unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new(tx, Some(root.path().into()), None);
        let state = app.audio.shared_state.clone();
        let (command_tx, commands) = std::sync::mpsc::channel();
        app.audio = crate::audio::AudioEngine { command_tx, shared_state: state };
        native_action(&mut app, &json!({"action":"native-shutdown"})).unwrap();
        assert!(app.should_quit);
        assert!(matches!(commands.try_recv(), Ok(crate::audio::AudioCommand::Stop)));
    }

    #[test]
    fn shared_queue_preserves_shuffle_order_and_versions_order_changes() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new(tx, None, None);
        native_action(&mut app, &json!({"action":"native-queue",
            "paths":["a.mp3","b.mp3","c.mp3"],"index":2,"shuffle_indices":[2,0,1]})).unwrap();
        let version = queue_version(&app);
        let snapshot = native_snapshot(&app, &json!({"include_queue":true}));
        assert_eq!(snapshot["shuffle_indices"], json!([2,0,1]));
        assert_eq!(app.queue.next(true), Some(PathBuf::from("a.mp3")));
        assert_eq!(app.queue.next(true), Some(PathBuf::from("b.mp3")));
        app.queue.current_index = Some(2);
        app.queue.shuffle_indices = vec![2,1,0];
        assert_ne!(queue_version(&app), version);
        assert!(native_action(&mut app, &json!({"action":"native-queue",
            "paths":["a.mp3","b.mp3","c.mp3"],"shuffle_indices":[2,2,0]})).is_err());
        assert_eq!(app.queue.shuffle_indices, vec![2,1,0]);
    }
}
