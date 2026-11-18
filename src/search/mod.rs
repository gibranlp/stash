use crate::models::FileItem;
use crate::preview::LatestLoader;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

struct SearchRequest {
    query: String,
    roots: Vec<PathBuf>,
    revision: u64,
    ready_at: Instant,
}

pub struct SearchState {
    pub query: String,
    pub active: bool,
    pub results: Vec<FileItem>,
    pub selected_index: usize,
    pub search_root: Option<PathBuf>,
    pub loading: bool,
    revision: Arc<AtomicU64>,
    worker: LatestLoader<SearchRequest, Vec<FileItem>>,
}

impl SearchState {
    pub fn new() -> Self {
        let revision = Arc::new(AtomicU64::new(0));
        let current = revision.clone();
        let worker = LatestLoader::new(move |request: &SearchRequest| {
            std::thread::sleep(request.ready_at.saturating_duration_since(Instant::now()));
            Ok(find_matches(&request.roots, &request.query, || {
                current.load(Ordering::Relaxed) != request.revision
            }))
        });
        Self {
            query: String::new(),
            active: false,
            results: Vec::new(),
            selected_index: 0,
            search_root: None,
            loading: false,
            revision,
            worker,
        }
    }

    // Debounce typing and keep recursive filesystem walks off the UI thread.
    pub fn execute(&mut self, search_dirs: &[PathBuf]) {
        let revision = self
            .revision
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        self.results.clear();
        self.selected_index = 0;
        self.loading = !self.query.trim().is_empty();
        if self.loading {
            self.worker.request(Some(SearchRequest {
                query: self.query.clone(),
                roots: search_dirs.to_vec(),
                revision,
                ready_at: Instant::now() + Duration::from_millis(80),
            }));
        } else {
            self.worker.request(None);
        }
    }

    pub fn poll(&mut self) -> bool {
        if !self.active && self.loading {
            self.revision.fetch_add(1, Ordering::Relaxed);
            self.worker.request(None);
            self.loading = false;
        }
        let Some((request, result)) = self.worker.poll() else {
            return false;
        };
        self.loading = false;
        if request.query != self.query {
            return false;
        }
        self.results = result.unwrap_or_default();
        self.selected_index = self
            .selected_index
            .min(self.results.len().saturating_sub(1));
        true
    }
}

impl Drop for SearchState {
    fn drop(&mut self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
}

fn find_matches(
    search_dirs: &[PathBuf],
    query: &str,
    cancelled: impl Fn() -> bool,
) -> Vec<FileItem> {
    let mut matched = Vec::new();
    let query_lower = query.to_lowercase();

    for dir in search_dirs {
        if cancelled() {
            return Vec::new();
        }
        if !dir.exists() {
            continue;
        }

        for entry in WalkDir::new(dir)
            .into_iter()
            // Saltamos los archivos/carpetas ocultos (los que empiezan con punto)
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                e.depth() == 0 || !name.starts_with('.')
            })
            .flatten()
        {
            if cancelled() {
                return Vec::new();
            }
            let path = entry.path();
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let matches_filename = filename.to_lowercase().contains(&query_lower);

            if matches_filename {
                // No incluimos las raíces de búsqueda, solo su contenido
                if search_dirs.iter().any(|d| d == path) {
                    continue;
                }
                let is_dir =
                    entry.file_type().is_dir() || (entry.file_type().is_symlink() && path.is_dir());
                let metadata = entry.metadata().ok();
                let size = if is_dir {
                    0
                } else {
                    metadata.as_ref().map(|m| m.len()).unwrap_or(0)
                };
                let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                matched.push(FileItem {
                    name: filename,
                    path: path.to_path_buf(),
                    size,
                    is_dir,
                    modified,
                    is_selected: false,
                    metadata: None,
                    depth: 0,
                    is_expanded: false,
                });

                if matched.len() >= 100 {
                    break;
                }
            }
        }
        if matched.len() >= 100 {
            break;
        }
    }

    matched
}

pub fn matches_audio_extension(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let ext = ext.to_lowercase();
        if ext == "mp4" && mp4_contains_video_track(path) {
            return false;
        }
        matches!(
            ext.as_str(),
            "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" | "mp4" | "aiff" | "aif"
        )
    } else {
        false
    }
}

fn mp4_contains_video_track(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };

    let len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(_) => return false,
    };

    scan_mp4_boxes_for_video(&mut file, 0, len, 0).unwrap_or(false)
}

fn scan_mp4_boxes_for_video(
    file: &mut File,
    start: u64,
    end: u64,
    depth: u8,
) -> std::io::Result<bool> {
    if depth > 8 {
        return Ok(false);
    }

    let mut pos = start;
    while pos + 8 <= end {
        file.seek(SeekFrom::Start(pos))?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header)?;

        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        let box_type = &header[4..8];
        let mut header_len = 8u64;
        let box_size = match size32 {
            0 => end.saturating_sub(pos),
            1 => {
                let mut extended = [0u8; 8];
                file.read_exact(&mut extended)?;
                header_len = 16;
                u64::from_be_bytes(extended)
            }
            n => n,
        };

        if box_size < header_len || pos.saturating_add(box_size) > end {
            break;
        }

        let payload_start = pos + header_len;
        let payload_end = pos + box_size;

        if box_type == b"hdlr" {
            if payload_start + 12 <= payload_end {
                file.seek(SeekFrom::Start(payload_start + 8))?;
                let mut handler = [0u8; 4];
                file.read_exact(&mut handler)?;
                if &handler == b"vide" {
                    return Ok(true);
                }
            }
        } else if matches!(box_type, b"moov" | b"trak" | b"mdia")
            && scan_mp4_boxes_for_video(file, payload_start, payload_end, depth + 1)?
        {
            return Ok(true);
        }

        pos += box_size;
    }

    Ok(false)
}

pub fn matches_image_extension(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        matches!(
            ext.to_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp"
        )
    } else {
        false
    }
}

pub fn matches_text_extension(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        matches!(
            ext.to_lowercase().as_str(),
            "txt"
                | "desktop"
                | "rs"
                | "py"
                | "js"
                | "ts"
                | "json"
                | "toml"
                | "yaml"
                | "yml"
                | "md"
                | "sh"
                | "html"
                | "css"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "go"
                | "java"
                | "kt"
                | "xml"
                | "sql"
        )
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::matches_audio_extension;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_mp4_path(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("stash-{name}-{stamp}.mp4"))
    }

    fn mp4_box(kind: &[u8; 4], payload: Vec<u8>) -> Vec<u8> {
        let size = (payload.len() + 8) as u32;
        let mut data = Vec::with_capacity(size as usize);
        data.extend_from_slice(&size.to_be_bytes());
        data.extend_from_slice(kind);
        data.extend_from_slice(&payload);
        data
    }

    fn handler_box(handler: &[u8; 4]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0, 0, 0, 0]);
        payload.extend_from_slice(&[0, 0, 0, 0]);
        payload.extend_from_slice(handler);
        mp4_box(b"hdlr", payload)
    }

    fn mp4_with_handler(handler: &[u8; 4]) -> Vec<u8> {
        mp4_box(
            b"moov",
            mp4_box(b"trak", mp4_box(b"mdia", handler_box(handler))),
        )
    }

    #[test]
    fn audio_only_mp4_matches_audio_extension() {
        let path = temp_mp4_path("audio");
        fs::write(&path, mp4_with_handler(b"soun")).unwrap();

        assert!(matches_audio_extension(&path));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn video_mp4_does_not_match_audio_extension() {
        let path = temp_mp4_path("video");
        fs::write(&path, mp4_with_handler(b"vide")).unwrap();

        assert!(!matches_audio_extension(&path));

        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod async_tests {
    use super::*;

    #[test]
    fn newer_queries_cancel_old_results_and_empty_queries_clear_them() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("alpha.mp3"), b"").unwrap();
        std::fs::write(root.path().join("beta.mp3"), b"").unwrap();
        let mut search = SearchState::new();
        search.active = true;
        search.query = "alpha".into();
        search.execute(&[root.path().into()]);
        search.query = "beta".into();
        search.execute(&[root.path().into()]);
        assert!(
            search.results.is_empty(),
            "Old matches must never remain selectable"
        );
        let started = Instant::now();
        while search.loading && started.elapsed() < Duration::from_secs(3) {
            search.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!search.loading);
        assert_eq!(search.results.len(), 1);
        assert_eq!(search.results[0].name, "beta.mp3");
        search.query.clear();
        search.execute(&[root.path().into()]);
        assert!(search.results.is_empty());
        assert!(!search.loading);
    }

    #[test]
    fn cancelled_walks_stop_and_hidden_children_stay_excluded() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".hidden")).unwrap();
        std::fs::write(root.path().join(".hidden/match.mp3"), b"").unwrap();
        for i in 0..110 {
            std::fs::write(root.path().join(format!("match-{i}.mp3")), b"").unwrap();
        }
        assert!(find_matches(&[root.path().into()], "match", || true).is_empty());
        let results = find_matches(&[root.path().into()], "match", || false);
        assert_eq!(results.len(), 100);
        assert!(results.iter().all(|r| r.path.parent() == Some(root.path())));
    }
}
