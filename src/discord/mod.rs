pub const DEFAULT_APP_ID: u64 = 1524097992859320521;

#[cfg(unix)]
mod imp {
    use serde_json::{Value, json};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const RETRY_INTERVAL: Duration = Duration::from_secs(5);

    pub struct DiscordPresence {
        tx: Option<Sender<Value>>,
        worker: Option<JoinHandle<()>>,
    }

    impl DiscordPresence {
        pub fn new(client_id: u64) -> Self {
            let (tx, rx) = mpsc::channel();
            let worker = thread::Builder::new()
                .name("discord-rpc".to_string())
                .spawn(move || run_discord_thread(client_id, rx, socket_paths(), RETRY_INTERVAL))
                .ok();
            Self {
                tx: Some(tx),
                worker,
            }
        }

        pub fn set_activity(
            &self,
            title: &str,
            artist: &str,
            elapsed_secs: u64,
            duration_secs: u64,
        ) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.send(activity(title, artist, elapsed_secs, duration_secs, now));
        }

        pub fn clear_activity(&self) {
            self.send(Value::Null);
        }

        fn send(&self, activity: Value) {
            if let Some(tx) = &self.tx {
                let _ = tx.send(activity);
            }
        }
    }

    impl Drop for DiscordPresence {
        fn drop(&mut self) {
            self.tx.take();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn activity(title: &str, artist: &str, elapsed: u64, duration: u64, now: u64) -> Value {
        // Store absolute timestamps so reconnecting does not reset the progress bar.
        let start = now.saturating_sub(elapsed);
        let mut timestamps = json!({ "start": start });
        if duration > 0 {
            timestamps["end"] = json!(start.saturating_add(duration));
        }
        let mut result = json!({
            "type": 2,
            "details": truncate(title),
            "timestamps": timestamps
        });
        if !artist.trim().is_empty() {
            result["state"] = json!(truncate(artist));
        }
        result
    }

    fn truncate(value: &str) -> String {
        // Discord limits rich presence text to 128 characters.
        value.chars().take(128).collect()
    }

    fn socket_paths() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for key in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
            if let Some(path) = std::env::var_os(key).filter(|p| !p.is_empty()) {
                roots.push(PathBuf::from(path));
            }
        }
        roots.push(PathBuf::from("/tmp"));
        let mut paths = Vec::new();
        for root in roots {
            for subdir in [
                "",
                "snap.discord",
                "app/com.discordapp.Discord",
                "app/com.discordapp.DiscordCanary",
                "app/dev.vencord.Vesktop",
            ] {
                for index in 0..10 {
                    let path = root.join(subdir).join(format!("discord-ipc-{index}"));
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }
        paths
    }

    fn run_discord_thread(
        client_id: u64,
        rx: Receiver<Value>,
        paths: Vec<PathBuf>,
        interval: Duration,
    ) {
        let mut ipc = DiscordIpc::new(client_id);
        let mut latest = Value::Null;
        loop {
            match rx.recv_timeout(interval) {
                Ok(next) => {
                    latest = next;
                    // Publish only the newest state after rapid skips or pause/resume.
                    while let Ok(next) = rx.try_recv() {
                        latest = next;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    if ipc.socket.is_some() {
                        ipc.set_activity(Value::Null);
                    }
                    break;
                }
            }
            // Keep the latest state while offline and replay it after reconnecting.
            // Periodic acknowledgements also detect a client restart during the same song.
            if ipc.socket.is_none() && !ipc.connect(&paths) {
                continue;
            }
            if !ipc.set_activity(latest.clone()) {
                ipc.socket = None;
            }
        }
    }

    struct DiscordIpc {
        client_id: u64,
        socket: Option<UnixStream>,
        nonce: u32,
    }

    impl DiscordIpc {
        fn new(client_id: u64) -> Self {
            Self {
                client_id,
                socket: None,
                nonce: 0,
            }
        }

        fn connect(&mut self, paths: &[PathBuf]) -> bool {
            // Try all candidates: a stale socket must not hide a working Vesktop socket.
            for path in paths {
                let Ok(stream) = UnixStream::connect(path) else {
                    continue;
                };
                let timeout = Some(Duration::from_millis(500));
                if stream.set_read_timeout(timeout).is_err()
                    || stream.set_write_timeout(timeout).is_err()
                {
                    continue;
                }
                self.socket = Some(stream);
                let handshake = json!({ "v": 1, "client_id": self.client_id.to_string() });
                if self.send_frame(0, &handshake)
                    && self
                        .recv_frame()
                        .is_some_and(|value| value["evt"] == "READY")
                {
                    return true;
                }
                self.socket = None;
            }
            false
        }

        fn send_frame(&mut self, opcode: u32, payload: &Value) -> bool {
            let Some(socket) = self.socket.as_mut() else {
                return false;
            };
            let data = payload.to_string();
            let mut frame = Vec::with_capacity(8 + data.len());
            frame.extend_from_slice(&opcode.to_le_bytes());
            frame.extend_from_slice(&(data.len() as u32).to_le_bytes());
            frame.extend_from_slice(data.as_bytes());
            socket.write_all(&frame).is_ok()
        }

        fn recv_frame(&mut self) -> Option<Value> {
            // Handle IPC ping frames while waiting for READY or a command acknowledgement.
            for _ in 0..8 {
                let socket = self.socket.as_mut()?;
                let mut header = [0u8; 8];
                socket.read_exact(&mut header).ok()?;
                let opcode = u32::from_le_bytes(header[..4].try_into().ok()?);
                let length = u32::from_le_bytes(header[4..].try_into().ok()?) as usize;
                if length > 65536 {
                    return None;
                }
                let mut data = vec![0u8; length];
                socket.read_exact(&mut data).ok()?;
                if opcode == 3 {
                    // Pong must echo the original bytes, even if the ping isn't JSON.
                    header[..4].copy_from_slice(&4u32.to_le_bytes());
                    socket.write_all(&header).ok()?;
                    socket.write_all(&data).ok()?;
                    continue;
                }
                if opcode != 1 {
                    return None;
                }
                return serde_json::from_slice(&data).ok();
            }
            None
        }

        fn set_activity(&mut self, activity: Value) -> bool {
            self.nonce = self.nonce.wrapping_add(1);
            let nonce = self.nonce.to_string();
            let payload = json!({
                "cmd": "SET_ACTIVITY",
                "args": { "pid": std::process::id(), "activity": activity },
                "nonce": nonce
            });
            if !self.send_frame(1, &payload) {
                return false;
            }
            for _ in 0..8 {
                let Some(reply) = self.recv_frame() else {
                    return false;
                };
                if reply["nonce"] == nonce {
                    return reply["evt"] != "ERROR" && reply["cmd"] == "SET_ACTIVITY";
                }
            }
            false
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::discord::DEFAULT_APP_ID;
        use std::os::unix::net::UnixListener;

        fn read(stream: &mut UnixStream) -> (u32, Value) {
            let mut header = [0; 8];
            stream.read_exact(&mut header).unwrap();
            let mut data = vec![0; u32::from_le_bytes(header[4..].try_into().unwrap()) as usize];
            stream.read_exact(&mut data).unwrap();
            (
                u32::from_le_bytes(header[..4].try_into().unwrap()),
                serde_json::from_slice(&data).unwrap(),
            )
        }

        fn reply(stream: &mut UnixStream, value: Value) {
            let data = value.to_string();
            stream.write_all(&1u32.to_le_bytes()).unwrap();
            stream
                .write_all(&(data.len() as u32).to_le_bytes())
                .unwrap();
            stream.write_all(data.as_bytes()).unwrap();
        }

        #[test]
        fn listening_payload_has_track_and_absolute_progress() {
            let value = activity("Song", "Artist", 30, 180, 1000);
            assert_eq!(value["type"], 2);
            assert_eq!(value["details"], "Song");
            assert_eq!(value["state"], "Artist");
            assert_eq!(value["timestamps"], json!({"start": 970, "end": 1150}));
            let unknown = activity(&"é".repeat(150), "", 0, 0, 1000);
            assert_eq!(unknown["details"].as_str().unwrap().chars().count(), 128);
            assert!(unknown.get("state").is_none());
            assert!(unknown["timestamps"].get("end").is_none());
        }

        #[test]
        fn handles_ping_and_rejects_error_acknowledgements() {
            let (client, mut server) = UnixStream::pair().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            server
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let worker = thread::spawn(move || {
                let (_, command) = read(&mut server);
                server.write_all(&3u32.to_le_bytes()).unwrap();
                server.write_all(&4u32.to_le_bytes()).unwrap();
                server.write_all(b"ping").unwrap();
                let mut pong = [0; 12];
                server.read_exact(&mut pong).unwrap();
                assert_eq!(&pong[..4], &4u32.to_le_bytes());
                assert_eq!(&pong[8..], b"ping");
                reply(
                    &mut server,
                    json!({"cmd": "SET_ACTIVITY", "evt": "ERROR", "nonce": command["nonce"]}),
                );
            });
            let mut ipc = DiscordIpc::new(DEFAULT_APP_ID);
            ipc.socket = Some(client);
            assert!(!ipc.set_activity(activity("Song", "Artist", 0, 180, 1000)));
            worker.join().unwrap();
        }

        #[test]
        fn replays_offline_track_after_reconnect_and_clears_on_exit() {
            let root = std::env::temp_dir().join(format!("stash-rpc-test-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("discord-ipc-1");
            let (tx, rx) = mpsc::channel();
            let paths = vec![root.join("discord-ipc-0"), path.clone()];
            let worker = thread::spawn(move || {
                run_discord_thread(DEFAULT_APP_ID, rx, paths, Duration::from_millis(30))
            });
            let expected = activity("Song", "Artist", 30, 180, 1000);
            tx.send(expected.clone()).unwrap();
            thread::sleep(Duration::from_millis(80));
            let listener = UnixListener::bind(&path).unwrap();
            // Two connections, without sending another song command.
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let (opcode, handshake) = read(&mut socket);
                assert_eq!(opcode, 0);
                assert_eq!(handshake["client_id"], DEFAULT_APP_ID.to_string());
                reply(&mut socket, json!({"cmd": "DISPATCH", "evt": "READY"}));
                let (_, command) = read(&mut socket);
                assert_eq!(command["args"]["activity"], expected);
                reply(
                    &mut socket,
                    json!({"cmd": "SET_ACTIVITY", "evt": null, "nonce": command["nonce"]}),
                );
                if attempt == 1 {
                    drop(tx);
                    let (_, clear) = read(&mut socket);
                    assert!(clear["args"]["activity"].is_null());
                    reply(
                        &mut socket,
                        json!({"cmd": "SET_ACTIVITY", "evt": null, "nonce": clear["nonce"]}),
                    );
                    break;
                }
            }
            worker.join().unwrap();
            std::fs::remove_file(path).unwrap();
            std::fs::remove_dir(root).unwrap();
        }
    }
}

#[cfg(unix)]
pub use imp::DiscordPresence;

#[cfg(not(unix))]
pub struct DiscordPresence;

#[cfg(not(unix))]
impl DiscordPresence {
    pub fn new(_client_id: u64) -> Self {
        Self
    }
    pub fn set_activity(
        &self,
        _title: &str,
        _artist: &str,
        _elapsed_secs: u64,
        _duration_secs: u64,
    ) {
    }
    pub fn clear_activity(&self) {}
}
