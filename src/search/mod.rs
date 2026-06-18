use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use crate::models::FileItem;

pub struct SearchState {
    pub query: String,
    pub active: bool,
    pub results: Vec<FileItem>,
    pub selected_index: usize,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            active: false,
            results: Vec::new(),
            selected_index: 0,
        }
    }

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
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    !name.starts_with('.')
                })
                .flatten()
            {
                let path = entry.path();
                if path.is_file() {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let matches_filename = filename.to_lowercase().contains(&query_lower);

                    if matches_filename {
                        let metadata = entry.metadata().ok();
                        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                        let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                        matched.push(FileItem {
                            name: filename,
                            path: path.to_path_buf(),
                            size,
                            is_dir: false,
                            modified,
                            is_selected: false,
                            metadata: None,
                        });

                        // Cap results at 100 to maintain instant rendering responsiveness
                        if matched.len() >= 100 {
                            break;
                        }
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
        matches!(ext.to_lowercase().as_str(), "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac")
    } else {
        false
    }
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

