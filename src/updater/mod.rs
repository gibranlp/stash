use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const REPO: &str = "gibranlp/stash";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    url: String,
    size: u64,
    digest: Option<String>,
    checksums_url: Option<String>,
    asset: String,
}

#[derive(Debug, Clone)]
pub enum UpdateProgress {
    Idle,
    Checking,
    Available {
        release: Release,
    },
    Downloading {
        version: String,
        downloaded: u64,
        total: u64,
    },
    Replacing,
    Done {
        version: String,
    },
    Error(String),
}

pub type UpdateSlot = Arc<Mutex<UpdateProgress>>;

pub fn new_slot() -> UpdateSlot {
    Arc::new(Mutex::new(UpdateProgress::Idle))
}

pub fn check_interval(state: &UpdateProgress) -> Duration {
    if matches!(state, UpdateProgress::Error(_)) {
        Duration::from_secs(15 * 60)
    } else {
        Duration::from_secs(6 * 60 * 60)
    }
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
    fn parse(s: &str) -> Option<semver::Version> {
        // Release tags historically used both v0.x.y and stash-v0.x.y.
        let version = s
            .strip_prefix("stash-")
            .unwrap_or(s)
            .trim_start_matches('v');
        let (core, suffix) = version
            .find(['-', '+'])
            .map_or((version, ""), |i| version.split_at(i));
        let mut core = core.to_string();
        while core.matches('.').count() < 2 {
            core.push_str(".0");
        }
        semver::Version::parse(&format!("{core}{suffix}")).ok()
    }
    match (parse(current), parse(remote)) {
        (Some(current), Some(remote)) => remote.cmp_precedence(&current).is_gt(),
        _ => false,
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
    state: String,
}

fn release_from_json(json: &str, current: &str, asset: &str) -> anyhow::Result<Option<Release>> {
    let release: GithubRelease =
        serde_json::from_str(json).context("Invalid GitHub release response")?;
    if release.draft || release.prerelease || !is_newer(current, &release.tag_name) {
        return Ok(None);
    }
    let binary = release
        .assets
        .iter()
        .find(|a| a.name == asset && a.state == "uploaded")
        .with_context(|| {
            format!(
                "Release {} has no uploaded {asset} asset yet",
                release.tag_name
            )
        })?;
    if binary.size == 0 || binary.size > MAX_DOWNLOAD {
        bail!("Invalid release asset size");
    }
    let digest = binary
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|d| valid_digest(d))
        .map(str::to_ascii_lowercase);
    let checksums_url = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS" && a.state == "uploaded")
        .map(|a| a.browser_download_url.clone());
    if digest.is_none() && checksums_url.is_none() {
        bail!("Release has no SHA-256 checksum");
    }
    for url in std::iter::once(&binary.browser_download_url).chain(checksums_url.iter()) {
        let parsed = url::Url::parse(url)?;
        if parsed.scheme() != "https"
            || parsed.host_str() != Some("github.com")
            || !parsed
                .path()
                .starts_with(&format!("/{REPO}/releases/download/"))
        {
            bail!("Unexpected GitHub release download URL");
        }
    }
    Ok(Some(Release {
        version: release.tag_name,
        url: binary.browser_download_url.clone(),
        size: binary.size,
        digest,
        checksums_url,
        asset: asset.to_string(),
    }))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(&format!("stash/{CURRENT_VERSION}"))
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .timeout(Duration::from_secs(180))
        .build()
}

// Claim the operation before spawning: repeated keypresses/ticks cannot start
// concurrent downloads. After replacement current_exe may refer to an unlinked
// inode on Linux, so a completed update is terminal until Stash restarts.
fn begin_check(slot: &UpdateSlot) -> bool {
    let mut state = slot.lock().unwrap();
    if matches!(
        *state,
        UpdateProgress::Checking
            | UpdateProgress::Downloading { .. }
            | UpdateProgress::Replacing
            | UpdateProgress::Done { .. }
    ) {
        return false;
    }
    *state = UpdateProgress::Checking;
    true
}

pub fn spawn_check(slot: UpdateSlot, auto_update: bool) {
    if !begin_check(&slot) {
        return;
    }
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Option<Release>> {
            let asset =
                asset_name().context("Automatic updates are unavailable for this platform")?;
            fetch_release(CURRENT_VERSION, asset)
        })();
        match result {
            Ok(Some(release)) => {
                *slot.lock().unwrap() = UpdateProgress::Available {
                    release: release.clone(),
                };
                if auto_update {
                    spawn_download(release, slot);
                }
            }
            Ok(None) => *slot.lock().unwrap() = UpdateProgress::Idle,
            Err(error) => *slot.lock().unwrap() = UpdateProgress::Error(format!("{error:#}")),
        }
    });
}

fn fetch_release(current: &str, asset: &str) -> anyhow::Result<Option<Release>> {
    let response = agent()
        .get(&format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("Could not check GitHub for updates")?;
    let mut json = String::new();
    response
        .into_reader()
        .take(2 * 1024 * 1024)
        .read_to_string(&mut json)?;
    release_from_json(&json, current, asset)
}

pub fn spawn_download(release: Release, slot: UpdateSlot) {
    {
        let mut state = slot.lock().unwrap();
        if !matches!(&*state, UpdateProgress::Available { release: available } if available.version == release.version)
        {
            return;
        }
        *state = UpdateProgress::Downloading {
            version: release.version.clone(),
            downloaded: 0,
            total: release.size,
        };
    }
    std::thread::spawn(move || {
        let result = download(&release, &slot);
        *slot.lock().unwrap() = match result {
            Ok(()) => UpdateProgress::Done {
                version: release.version,
            },
            Err(error) => UpdateProgress::Error(format!("{error:#}")),
        };
    });
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit())
}

fn checksum_for_asset(manifest: &str, asset: &str) -> anyhow::Result<String> {
    for line in manifest.lines() {
        let mut fields = line.split_whitespace();
        if let (Some(hash), Some(name)) = (fields.next(), fields.next())
            && name.trim_start_matches('*') == asset
            && valid_digest(hash)
        {
            return Ok(hash.to_ascii_lowercase());
        }
    }
    bail!("SHA256SUMS has no valid checksum for {asset}")
}

fn download(release: &Release, slot: &UpdateSlot) -> anyhow::Result<()> {
    let current_exe =
        std::env::current_exe().context("Cannot locate the installed Stash executable")?;
    let current_exe = current_exe.canonicalize().context(
        "The running Stash executable was moved or replaced; restart Stash before updating",
    )?;
    download_to(release, slot, &current_exe)
}

fn download_to(release: &Release, slot: &UpdateSlot, current_exe: &Path) -> anyhow::Result<()> {
    // Create the temporary file alongside the executable before downloading.
    // This detects directory permissions early and keeps the final rename atomic.
    let mut staged = tempfile::NamedTempFile::new_in(current_exe.parent().context("Executable has no directory")?)
        .context("Stash's installation directory is not writable; update it using its package manager or reinstall to ~/.local/bin")?;
    let agent = agent();
    let digest = match &release.digest {
        Some(digest) => digest.clone(),
        None => {
            let url = release
                .checksums_url
                .as_ref()
                .context("Missing checksum URL")?;
            let response = agent
                .get(url)
                .call()
                .context("Could not download SHA256SUMS")?;
            let mut manifest = String::new();
            response
                .into_reader()
                .take(64 * 1024)
                .read_to_string(&mut manifest)?;
            checksum_for_asset(&manifest, &release.asset)?
        }
    };
    let response = agent
        .get(&release.url)
        .call()
        .context("Could not download the update")?;
    if let Some(length) = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        && length != release.size
    {
        bail!("Update size does not match GitHub's release metadata");
    }
    stage_download(response.into_reader(), &mut staged, release, &digest, slot)?;
    *slot.lock().unwrap() = UpdateProgress::Replacing;
    replace(staged, current_exe)?;
    Ok(())
}

fn stage_download(
    mut reader: impl Read,
    staged: &mut tempfile::NamedTempFile,
    release: &Release,
    expected_digest: &str,
    slot: &UpdateSlot,
) -> anyhow::Result<()> {
    let mut hasher = Sha256::new();
    let mut chunk = [0; 65536];
    let mut downloaded = 0u64;
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        downloaded += count as u64;
        if downloaded > release.size || downloaded > MAX_DOWNLOAD {
            bail!("Update download exceeds expected size");
        }
        hasher.update(&chunk[..count]);
        staged.write_all(&chunk[..count])?;
        *slot.lock().unwrap() = UpdateProgress::Downloading {
            version: release.version.clone(),
            downloaded,
            total: release.size,
        };
    }
    if downloaded == 0 || downloaded != release.size {
        bail!("Incomplete update download");
    }
    if format!("{:x}", hasher.finalize()) != expected_digest {
        bail!("Update SHA-256 checksum mismatch; installed Stash was left untouched");
    }
    staged.as_file().sync_all()?;
    Ok(())
}

fn replace(staged: tempfile::NamedTempFile, current_exe: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    staged.persist(current_exe).map_err(|e| {
        // PersistError retains the temporary file; dropping it removes it.
        anyhow::anyhow!("Could not replace Stash: {}. Check installation-directory permissions. On Windows, close Stash and run install.ps1.", e.error)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn release(bytes: &[u8]) -> Release {
        Release {
            version: "v9.0.0".into(),
            url: format!("https://github.com/{REPO}/releases/download/v9.0.0/stash-linux-x86_64"),
            size: bytes.len() as u64,
            digest: Some(format!("{:x}", Sha256::digest(bytes))),
            checksums_url: None,
            asset: "stash-linux-x86_64".into(),
        }
    }

    fn metadata() -> serde_json::Value {
        serde_json::json!({
            "tag_name": "v9.0.0", "draft": false, "prerelease": false,
            "assets": [{ "name": "stash-linux-x86_64", "size": 3, "state": "uploaded",
                "digest": format!("sha256:{:x}", Sha256::digest(b"new")),
                "browser_download_url": format!("https://github.com/{REPO}/releases/download/v9.0.0/stash-linux-x86_64") }]
        })
    }

    #[test]
    fn compares_versions_and_historical_prefixes() {
        assert!(is_newer("0.7.1", "v0.10.0"));
        assert!(is_newer("0.7.1", "stash-v0.7.2"));
        assert!(!is_newer("0.7", "v0.7.0"));
        assert!(!is_newer("0.7.1", "v0.7.1"));
        assert!(!is_newer("0.7.1", "v0.7.0"));
        assert!(!is_newer("0.7.1", "v0.7.1-beta.1"));
        assert!(!is_newer("0.7.1+local", "v0.7.1+github"));
        assert!(!is_newer("0.7.1", "release tomorrow"));
    }

    #[test]
    fn selects_uploaded_asset_and_uses_its_download_url() {
        let json = metadata();
        let selected = release_from_json(&json.to_string(), "0.7.1", "stash-linux-x86_64")
            .unwrap()
            .unwrap();
        assert_eq!(selected.url, json["assets"][0]["browser_download_url"]);
        assert_eq!(selected.size, 3);
        assert_eq!(selected.digest, release(b"new").digest);
        assert!(
            release_from_json(&json.to_string(), "9.0.0", "stash-linux-x86_64")
                .unwrap()
                .is_none()
        );
        assert!(release_from_json(&json.to_string(), "0.7.1", "stash-macos-arm64").is_err());
    }

    #[test]
    fn skips_drafts_and_prereleases() {
        for flag in ["draft", "prerelease"] {
            let mut json = metadata();
            json[flag] = true.into();
            assert!(
                release_from_json(&json.to_string(), "0.7.1", "stash-linux-x86_64")
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn incomplete_or_untrusted_release_is_an_error() {
        assert!(release_from_json("not JSON", "0.7.1", "stash-linux-x86_64").is_err());
        for (field, value) in [
            ("state", serde_json::json!("new")),
            ("size", serde_json::json!(0)),
            ("size", serde_json::json!(MAX_DOWNLOAD + 1)),
            ("digest", serde_json::Value::Null),
            (
                "browser_download_url",
                serde_json::json!("https://example.com/stash"),
            ),
        ] {
            let mut json = metadata();
            json["assets"][0][field] = value;
            assert!(
                release_from_json(&json.to_string(), "0.7.1", "stash-linux-x86_64").is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn older_releases_can_use_sha256sums() {
        let mut json = metadata();
        json["assets"][0]["digest"] = serde_json::Value::Null;
        json["assets"].as_array_mut().unwrap().push(serde_json::json!({
            "name": "SHA256SUMS", "size": 100, "state": "uploaded", "digest": null,
            "browser_download_url": format!("https://github.com/{REPO}/releases/download/v9.0.0/SHA256SUMS")
        }));
        let selected = release_from_json(&json.to_string(), "0.7.1", "stash-linux-x86_64")
            .unwrap()
            .unwrap();
        assert!(selected.digest.is_none() && selected.checksums_url.is_some());
        let hash = release(b"new").digest.unwrap();
        assert_eq!(
            checksum_for_asset(&format!("{hash}  stash-linux-x86_64\n"), &selected.asset).unwrap(),
            hash
        );
        assert_eq!(
            checksum_for_asset(&format!("{hash} *stash-linux-x86_64\r\n"), &selected.asset)
                .unwrap(),
            hash
        );
        assert!(checksum_for_asset(&format!("{hash}  another-app\n"), &selected.asset).is_err());
    }

    #[test]
    fn prevents_overlapping_updates_and_replacement_until_restart() {
        let slot = new_slot();
        assert!(begin_check(&slot));
        assert!(!begin_check(&slot));
        for state in [
            UpdateProgress::Downloading {
                version: "v9".into(),
                downloaded: 0,
                total: 10,
            },
            UpdateProgress::Replacing,
            UpdateProgress::Done {
                version: "v9".into(),
            },
        ] {
            *slot.lock().unwrap() = state;
            assert!(!begin_check(&slot));
        }
        *slot.lock().unwrap() = UpdateProgress::Error("offline".into());
        assert!(begin_check(&slot));
        assert!(
            check_interval(&UpdateProgress::Error("offline".into()))
                < check_interval(&UpdateProgress::Idle)
        );
    }

    #[test]
    fn invalid_downloads_preserve_installed_binary_and_clean_up() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("stash");
        std::fs::write(&installed, b"original executable").unwrap();
        for bytes in [b"".as_slice(), b"ne", b"new plus unexpected bytes", b"bad"] {
            let mut staged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
            let staged_path = staged.path().to_path_buf();
            let descriptor = release(b"new");
            assert!(
                stage_download(
                    Cursor::new(bytes),
                    &mut staged,
                    &descriptor,
                    descriptor.digest.as_deref().unwrap(),
                    &new_slot()
                )
                .is_err()
            );
            drop(staged);
            assert!(!staged_path.exists());
            assert_eq!(std::fs::read(&installed).unwrap(), b"original executable");
        }
    }

    #[cfg(unix)]
    #[test]
    fn atomically_replaces_a_running_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("stash");
        std::fs::copy("/bin/sleep", &installed).unwrap();
        let mut running = std::process::Command::new(&installed)
            .arg("5")
            .spawn()
            .unwrap();
        let bytes = std::fs::read("/bin/true").unwrap();
        let descriptor = release(&bytes);
        let mut staged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        stage_download(
            Cursor::new(bytes),
            &mut staged,
            &descriptor,
            descriptor.digest.as_deref().unwrap(),
            &new_slot(),
        )
        .unwrap();
        replace(staged, &installed).unwrap();
        assert!(running.try_wait().unwrap().is_none());
        running.kill().unwrap();
        running.wait().unwrap();
        assert!(
            std::process::Command::new(&installed)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            std::fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    #[ignore = "requires access to GitHub"]
    fn live_github_check() {
        // Test downloading and replacement in an isolated temp directory.
        // Never replace or execute the user's installed binary.
        let selected = fetch_release("0.0.0", asset_name().unwrap())
            .unwrap()
            .unwrap();
        assert!(is_newer("0.0.0", &selected.version));
        assert!(selected.size > 0);
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("stash");
        std::fs::write(&installed, b"old executable").unwrap();
        download_to(&selected, &new_slot(), &installed).unwrap();
        assert_eq!(std::fs::metadata(&installed).unwrap().len(), selected.size);
        let hash = format!("{:x}", Sha256::digest(std::fs::read(&installed).unwrap()));
        assert_eq!(Some(hash), selected.digest);
        println!(
            "GitHub release: {} ({} bytes)",
            selected.version, selected.size
        );
    }
}
