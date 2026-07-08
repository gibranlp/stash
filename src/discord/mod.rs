pub const DEFAULT_APP_ID: u64 = 1524097992859320521;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use serde_json::json;

pub enum DiscordCommand {
    SetActivity {
        title: String,
        artist: String,
        elapsed_secs: u64,
        duration_secs: u64,
    },
    ClearActivity,
}

pub struct DiscordPresence {
    tx: Option<Sender<DiscordCommand>>,
}

impl DiscordPresence {
    pub fn new(client_id: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("discord-rpc".to_string())
            .spawn(move || run_discord_thread(client_id, rx))
            .ok();
        Self { tx: Some(tx) }
    }

    pub fn set_activity(&self, title: &str, artist: &str, elapsed_secs: u64, duration_secs: u64) {
        if let Some(ref tx) = self.tx {
            let _ = tx.send(DiscordCommand::SetActivity {
                title: title.to_string(),
                artist: artist.to_string(),
                elapsed_secs,
                duration_secs,
            });
        }
    }

    pub fn clear_activity(&self) {
        if let Some(ref tx) = self.tx {
            let _ = tx.send(DiscordCommand::ClearActivity);
        }
    }
}

// Hilo que vive aparte y maneja toda la comunicación con Discord.
// Si se cae la conexión, reintenta cada 10 segundos sin tronar el resto de la app.
fn run_discord_thread(client_id: u64, rx: Receiver<DiscordCommand>) {
    let mut ipc = DiscordIpc::new(client_id);
    let mut connected = ipc.connect();
    let mut reconnect_needed = !connected;
    let mut next_reconnect = Instant::now() + Duration::from_secs(10);

    loop {
        if reconnect_needed && Instant::now() >= next_reconnect {
            connected = ipc.connect();
            reconnect_needed = !connected;
            if reconnect_needed {
                next_reconnect = Instant::now() + Duration::from_secs(10);
            }
        }

        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(cmd) => {
                // Jalamos todos los comandos pendientes y nos quedamos solo con el más nuevo,
                // pa' no mandarle a Discord info ya stale si el usuario anduvo skipeando rolas.
                let mut latest = cmd;
                while let Ok(newer) = rx.try_recv() {
                    latest = newer;
                }

                if !connected {
                    continue;
                }

                let ok = match &latest {
                    DiscordCommand::SetActivity { title, artist, elapsed_secs, duration_secs } => {
                        ipc.set_activity(title, artist, *elapsed_secs, *duration_secs)
                    }
                    DiscordCommand::ClearActivity => ipc.clear_activity(),
                };

                if !ok {
                    connected = false;
                    reconnect_needed = true;
                    next_reconnect = Instant::now() + Duration::from_secs(10);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

struct DiscordIpc {
    client_id: u64,
    socket: Option<UnixStream>,
    pid: u32,
    nonce: u32,
}

impl DiscordIpc {
    fn new(client_id: u64) -> Self {
        Self {
            client_id,
            socket: None,
            pid: std::process::id(),
            nonce: 0,
        }
    }

    // Discord puede estar instalado de varias formas en Linux, así que checamos
    // los sockets en el orden más común: instalación normal, snap, flatpak, flatpak canary.
    fn find_socket() -> Option<PathBuf> {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
        for i in 0..10 {
            let p = PathBuf::from(format!("{runtime_dir}/discord-ipc-{i}"));
            if p.exists() {
                return Some(p);
            }
        }
        let p = PathBuf::from(format!("{runtime_dir}/snap.discord/discord-ipc-0"));
        if p.exists() {
            return Some(p);
        }
        let p = PathBuf::from(format!("{runtime_dir}/app/com.discordapp.Discord/discord-ipc-0"));
        if p.exists() {
            return Some(p);
        }
        let p = PathBuf::from(format!("{runtime_dir}/app/com.discordapp.DiscordCanary/discord-ipc-0"));
        if p.exists() {
            return Some(p);
        }
        None
    }

    // Conecta al socket, manda el handshake inicial y espera la respuesta de Discord.
    // Si algo falla en cualquier paso, limpiamos el socket y devolvemos false.
    fn connect(&mut self) -> bool {
        let path = match Self::find_socket() {
            Some(p) => p,
            None => return false,
        };

        let stream = match UnixStream::connect(&path) {
            Ok(s) => s,
            Err(_) => return false,
        };
        stream.set_read_timeout(Some(Duration::from_millis(500))).ok();
        self.socket = Some(stream);

        let handshake = json!({ "v": 1, "client_id": self.client_id.to_string() });
        if !self.send_frame(0, &handshake.to_string()) {
            self.socket = None;
            return false;
        }

        match self.recv_frame() {
            Some(_) => true,
            None => {
                self.socket = None;
                false
            }
        }
    }

    // El protocolo IPC de Discord usa frames con header de 8 bytes:
    // 4 bytes opcode (little-endian) + 4 bytes longitud del payload.
    fn send_frame(&mut self, opcode: u32, json_str: &str) -> bool {
        let socket = match self.socket.as_mut() {
            Some(s) => s,
            None => return false,
        };
        let data = json_str.as_bytes();
        let mut buf = Vec::with_capacity(8 + data.len());
        buf.extend_from_slice(&opcode.to_le_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(data);
        if socket.write_all(&buf).is_err() {
            self.socket = None;
            return false;
        }
        true
    }

    fn recv_frame(&mut self) -> Option<serde_json::Value> {
        let socket = self.socket.as_mut()?;
        let mut header = [0u8; 8];
        socket.read_exact(&mut header).ok()?;
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if length > 65536 {
            return None;
        }
        let mut buf = vec![0u8; length];
        socket.read_exact(&mut buf).ok()?;
        serde_json::from_slice(&buf).ok()
    }

    fn send_command(&mut self, activity_args: serde_json::Value) -> bool {
        self.nonce += 1;
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": activity_args,
            "nonce": self.nonce.to_string()
        });
        if !self.send_frame(1, &payload.to_string()) {
            return false;
        }
        // Drenamos el ACK que manda Discord pa' no llenar el buffer; si no llega, no importa.
        self.recv_frame();
        true
    }

    // Calculamos el timestamp de inicio restándole al tiempo actual los segundos ya reproducidos,
    // así Discord muestra la barra de progreso bien alineada con lo que va sonando.
    fn set_activity(&mut self, title: &str, artist: &str, elapsed_secs: u64, duration_secs: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let start_ts = now.saturating_sub(elapsed_secs);

        let mut timestamps = json!({ "start": start_ts });
        if duration_secs > 0 {
            timestamps["end"] = json!(start_ts + duration_secs);
        }

        let activity = json!({
            "type": 2,
            "details": title,
            "state": artist,
            "timestamps": timestamps
        });

        self.send_command(json!({ "pid": self.pid, "activity": activity }))
    }

    fn clear_activity(&mut self) -> bool {
        self.send_command(json!({ "pid": self.pid, "activity": null }))
    }
}
