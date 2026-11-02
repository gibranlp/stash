//! Mounts hot-plugged drives (USB sticks, external disks) through udisks2 so
//! they show up in Stash without opening a file manager. Linux only: macOS
//! mounts removable volumes under /Volumes by itself.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct AutoMounter {
    busy: Arc<AtomicBool>,
    // Devices already tried, so a drive that refuses to mount (encrypted,
    // unsupported filesystem, unmounted on purpose) isn't retried every scan.
    attempted: Arc<Mutex<HashSet<String>>>,
    // Mount points that appeared since the last `take_mounted` call.
    mounted: Arc<Mutex<Vec<String>>>,
}

impl AutoMounter {
    /// Mounts any unmounted hot-plugged filesystem in the background.
    pub fn scan(&self) {
        if !cfg!(target_os = "linux") || self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let busy = self.busy.clone();
        let attempted = self.attempted.clone();
        let mounted = self.mounted.clone();
        std::thread::spawn(move || {
            if let Some(devices) = list_devices() {
                let present: HashSet<&str> = devices.iter().map(|d| d.path.as_str()).collect();
                // Forget removed devices so re-inserting one mounts it again.
                attempted.lock().unwrap().retain(|p| present.contains(p.as_str()));
                for device in devices.iter().filter(|d| d.wants_mount()) {
                    if !attempted.lock().unwrap().insert(device.path.clone()) {
                        continue;
                    }
                    if let Some(point) = mount(&device.path) {
                        mounted.lock().unwrap().push(point);
                    }
                }
            }
            busy.store(false, Ordering::Release);
        });
    }

    pub fn take_mounted(&self) -> Vec<String> {
        std::mem::take(&mut *self.mounted.lock().unwrap())
    }
}

#[derive(Debug, PartialEq)]
struct Device {
    path: String,
    fstype: Option<String>,
    mounted: bool,
    hotplug: bool,
}

impl Device {
    fn wants_mount(&self) -> bool {
        const SKIP: &[&str] = &["swap", "crypto_LUKS", "LVM2_member", "linux_raid_member", "iso9660_boot"];
        self.hotplug
            && !self.mounted
            && self.fstype.as_deref().is_some_and(|fs| !SKIP.contains(&fs))
    }
}

fn list_devices() -> Option<Vec<Device>> {
    let output = std::process::Command::new("lsblk")
        .args(["-J", "-o", "PATH,FSTYPE,MOUNTPOINT,HOTPLUG"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    parse_lsblk(&String::from_utf8_lossy(&output.stdout))
}

fn parse_lsblk(json: &str) -> Option<Vec<Device>> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut devices = Vec::new();
    fn walk(nodes: &[serde_json::Value], devices: &mut Vec<Device>) {
        for node in nodes {
            let text = |key: &str| node[key].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            // Older lsblk prints booleans as "1"/"0" strings.
            let hotplug = node["hotplug"].as_bool().unwrap_or_else(|| node["hotplug"].as_str() == Some("1"));
            if let Some(path) = text("path") {
                devices.push(Device {
                    path,
                    fstype: text("fstype"),
                    mounted: text("mountpoint").is_some(),
                    hotplug,
                });
            }
            if let Some(children) = node["children"].as_array() {
                walk(children, devices);
            }
        }
    }
    walk(value["blockdevices"].as_array()?, &mut devices);
    Some(devices)
}

/// Returns the new mount point. `--no-user-interaction` keeps polkit from
/// prompting for a password over the TUI; desktop sessions are allowed to
/// mount removable drives without one.
fn mount(device: &str) -> Option<String> {
    let output = std::process::Command::new("udisksctl")
        .args(["mount", "--no-user-interaction", "-b", device])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    // "Mounted /dev/sdb1 at /run/media/user/STICK"
    let text = String::from_utf8_lossy(&output.stdout);
    let point = text.trim().split_once(" at ")?.1.trim_end_matches('.');
    Some(point.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounts_only_unmounted_hotplug_filesystems() {
        let json = r#"{"blockdevices":[
            {"path":"/dev/sda","fstype":null,"mountpoint":null,"hotplug":false,"children":[
                {"path":"/dev/sda1","fstype":"ext4","mountpoint":null,"hotplug":false}]},
            {"path":"/dev/sdb","fstype":null,"mountpoint":null,"hotplug":true,"children":[
                {"path":"/dev/sdb1","fstype":"vfat","mountpoint":null,"hotplug":true},
                {"path":"/dev/sdb2","fstype":"exfat","mountpoint":"/run/media/u/X","hotplug":true},
                {"path":"/dev/sdb3","fstype":"crypto_LUKS","mountpoint":null,"hotplug":true}]},
            {"path":"/dev/sdc1","fstype":"ntfs","mountpoint":"","hotplug":"1"}]}"#;
        let wanted: Vec<String> = parse_lsblk(json)
            .unwrap()
            .into_iter()
            .filter(Device::wants_mount)
            .map(|d| d.path)
            .collect();
        assert_eq!(wanted, ["/dev/sdb1", "/dev/sdc1"]);
    }
}
