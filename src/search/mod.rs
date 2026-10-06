use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use crate::models::FileItem;

pub struct SearchState {
    pub query: String,
    pub active: bool,
    pub results: Vec<FileItem>,
    pub selected_index: usize,
    pub search_root: Option<PathBuf>,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            active: false,
            results: Vec::new(),
            selected_index: 0,
            search_root: None,
        }
    }

    // Aquí jalamos el WalkDir por todos los directorios configurados y filtramos por query.
    // Ojo: le ponemos tope de 100 resultados para que no se tarde un chingo renderizando.
    pub fn execute(&mut self, search_dirs: &[PathBuf]) {
        if self.query.trim().is_empty() {
            self.results.clear();
            self.selected_index = 0;
            return;
        }

        let mut matched = Vec::new();
        let query_lower = self.query.to_lowercase();

        for dir in search_dirs {
            if !dir.exists() {
                continue;
            }

            for entry in WalkDir::new(dir)
                .into_iter()
                // Saltamos los archivos/carpetas ocultos (los que empiezan con punto)
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    !name.starts_with('.')
                })
                .flatten()
            {
                let path = entry.path();
                let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let matches_filename = filename.to_lowercase().contains(&query_lower);

                if matches_filename {
                    // No incluimos las raíces de búsqueda, solo su contenido
                    if search_dirs.iter().any(|d| d == path) {
                        continue;
                    }
                    let is_dir = path.is_dir();
                    let metadata = entry.metadata().ok();
                    let size = if is_dir { 0 } else { metadata.as_ref().map(|m| m.len()).unwrap_or(0) };
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

        self.results = matched;
        if self.results.is_empty() {
            self.selected_index = 0;
        } else if self.selected_index >= self.results.len() {
            self.selected_index = self.results.len() - 1;
        }
    }
}

pub fn matches_audio_extension(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let ext = ext.to_lowercase();
        if ext == "mp4" && mp4_contains_video_track(path) {
            return false;
        }
        matches!(
            ext.as_str(),
            "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "mp4" | "aiff" | "aif"
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
            && scan_mp4_boxes_for_video(file, payload_start, payload_end, depth + 1)? {
                return Ok(true);
            }

        pos += box_size;
    }

    Ok(false)
}

pub fn matches_image_extension(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        matches!(ext.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp")
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
