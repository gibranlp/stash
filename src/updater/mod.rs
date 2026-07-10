use std::io::Read;
use std::sync::{Arc, Mutex};

const REPO: &str = "gibranlp/stash";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub enum UpdateProgress {
    Idle,
    Checking,
    Available { version: String, url: String },
    Downloading { version: String, downloaded: u64, total: u64 },
    Replacing,
    Done { version: String },
    Error(String),
}

pub type UpdateSlot = Arc<Mutex<UpdateProgress>>;

pub fn new_slot() -> UpdateSlot {
    Arc::new(Mutex::new(UpdateProgress::Idle))
}

fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("stash-linux-x86_64"),
        ("macos", "x86_64") => Some("stash-macos-x86_64"),
        ("macos", "aarch64") => Some("stash-macos-arm64"),
        ("windows", "x86_64") => Some("stash-windows-x86_64.exe"),
        _ => None,
    }
}

fn is_newer(current: &str, remote: &str) -> bool {
    let remote = remote.trim_start_matches('v');
    let parse_ver = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|p| p.parse().ok()).collect()
    };
    parse_ver(remote) > parse_ver(current)
}

pub fn spawn_check(slot: UpdateSlot) {
    std::thread::spawn(move || {
        *slot.lock().unwrap() = UpdateProgress::Checking;

        let api_url = format!("https://api.github.com/repos/{}/releases/latest", REPO);
        let resp = ureq::get(&api_url)
            .set("User-Agent", &format!("stash/{}", CURRENT_VERSION))
            .call();

        let next = match resp {
            Ok(r) => match r.into_json::<serde_json::Value>() {
                Ok(json) => {
                    let tag = json["tag_name"].as_str().unwrap_or("").to_string();
                    if tag.is_empty() || !is_newer(CURRENT_VERSION, &tag) {
                        UpdateProgress::Idle
                    } else if let Some(asset) = asset_name() {
                        let dl_url = format!(
                            "https://github.com/{}/releases/download/{}/{}",
                            REPO, tag, asset
                        );
                        UpdateProgress::Available { version: tag, url: dl_url }
                    } else {
                        UpdateProgress::Idle
                    }
                }
                Err(_) => UpdateProgress::Idle,
            },
            Err(_) => UpdateProgress::Idle,
        };

        *slot.lock().unwrap() = next;
    });
}

pub fn spawn_download(version: String, url: String, slot: UpdateSlot) {
    std::thread::spawn(move || {
        let ver = version.clone();
        let result = (|| -> anyhow::Result<()> {
            let resp = ureq::get(&url)
                .set("User-Agent", &format!("stash/{}", CURRENT_VERSION))
                .call()
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            let total: u64 = resp
                .header("Content-Length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);

            let mut reader = resp.into_reader();
            let mut buf: Vec<u8> = Vec::with_capacity(total as usize + 1024);
            let mut chunk = [0u8; 65536];
            let mut downloaded: u64 = 0;

            loop {
                let n = reader.read(&mut chunk)?;
                if n == 0 { break; }
                buf.extend_from_slice(&chunk[..n]);
                downloaded += n as u64;
                *slot.lock().unwrap() = UpdateProgress::Downloading {
                    version: version.clone(),
                    downloaded,
                    total: total.max(downloaded),
                };
            }

            *slot.lock().unwrap() = UpdateProgress::Replacing;

            let current_exe = std::env::current_exe()?;
            let tmp = current_exe.with_extension("update_tmp");

            {
                use std::io::Write;
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(&buf)?;
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            }

            std::fs::rename(&tmp, &current_exe).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                anyhow::anyhow!(
                    "Could not replace binary: {}. On Windows, run install.ps1 instead.",
                    e
                )
            })?;

            Ok(())
        })();

        *slot.lock().unwrap() = match result {
            Ok(()) => UpdateProgress::Done { version: ver },
            Err(e) => UpdateProgress::Error(e.to_string()),
        };
    });
}
