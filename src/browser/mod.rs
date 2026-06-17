use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use crate::models::FileItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneType {
    Directories,
    Files,
}

pub struct BrowserState {
    pub current_dir: PathBuf,
    pub directories: Vec<PathBuf>,
    pub files: Vec<FileItem>,
    pub dir_index: usize,
    pub file_index: usize,
    pub focused_pane: PaneType,
    pub selected_paths: HashSet<PathBuf>,
    pub show_hidden: bool,
}

impl BrowserState {
    pub fn new(initial_dir: PathBuf, show_hidden: bool) -> Self {
        let mut state = Self {
            current_dir: initial_dir,
            directories: Vec::new(),
            files: Vec::new(),
            dir_index: 0,
            file_index: 0,
            focused_pane: PaneType::Directories,
            selected_paths: HashSet::new(),
            show_hidden,
        };
        state.refresh();
        state
    }

    pub fn refresh(&mut self) {
        let (dirs, mut fls) = load_directory(&self.current_dir, self.show_hidden);
        
        // Sync selections
        for file in &mut fls {
            if self.selected_paths.contains(&file.path) {
                file.is_selected = true;
            }
        }

        self.directories = dirs;
        self.files = fls;

        // Ensure indices are within bounds
        if self.directories.is_empty() {
            self.dir_index = 0;
        } else if self.dir_index >= self.directories.len() {
            self.dir_index = self.directories.len() - 1;
        }

        if self.files.is_empty() {
            self.file_index = 0;
        } else if self.file_index >= self.files.len() {
            self.file_index = self.files.len() - 1;
        }
    }

    pub fn move_up(&mut self) {
        match self.focused_pane {
            PaneType::Directories => {
                if !self.directories.is_empty() && self.dir_index > 0 {
                    self.dir_index -= 1;
                }
            }
            PaneType::Files => {
                if !self.files.is_empty() && self.file_index > 0 {
                    self.file_index -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.focused_pane {
            PaneType::Directories => {
                if !self.directories.is_empty() && self.dir_index + 1 < self.directories.len() {
                    self.dir_index += 1;
                }
            }
            PaneType::Files => {
                if !self.files.is_empty() && self.file_index + 1 < self.files.len() {
                    self.file_index += 1;
                }
            }
        }
    }

    pub fn open_selected_dir(&mut self) {
        if self.focused_pane == PaneType::Directories && !self.directories.is_empty() {
            let next_dir = self.directories[self.dir_index].clone();
            
            // Normalize path if it ends with . or ..
            let resolved = if next_dir.ends_with(".") {
                self.current_dir.clone()
            } else if next_dir.ends_with("..") {
                self.current_dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| self.current_dir.clone())
            } else {
                next_dir
            };

            self.current_dir = resolved;
            self.dir_index = 0;
            self.file_index = 0;
            self.refresh();
        }
    }

    pub fn go_to_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            let old_dir = self.current_dir.clone();
            self.current_dir = parent.to_path_buf();
            self.refresh();
            
            // Try to set directory index to the folder we just exited
            if let Some(pos) = self.directories.iter().position(|d| d == &old_dir) {
                self.dir_index = pos;
            } else {
                self.dir_index = 0;
            }
            self.file_index = 0;
            self.focused_pane = PaneType::Directories;
        }
    }

    pub fn toggle_select_highlighted(&mut self) {
        if self.focused_pane == PaneType::Files && !self.files.is_empty() {
            let file = &mut self.files[self.file_index];
            file.is_selected = !file.is_selected;
            if file.is_selected {
                self.selected_paths.insert(file.path.clone());
            } else {
                self.selected_paths.remove(&file.path);
            }
        }
    }

    pub fn clear_selections(&mut self) {
        self.selected_paths.clear();
        for file in &mut self.files {
            file.is_selected = false;
        }
    }


    pub fn delete_selected(&mut self) -> anyhow::Result<()> {
        for path in &self.selected_paths {
            if path.is_file() {
                fs::remove_file(path)?;
            }
        }
        self.clear_selections();
        self.refresh();
        Ok(())
    }

    pub fn page_up(&mut self) {
        let step = 10;
        match self.focused_pane {
            PaneType::Directories => {
                if !self.directories.is_empty() {
                    self.dir_index = self.dir_index.saturating_sub(step);
                }
            }
            PaneType::Files => {
                if !self.files.is_empty() {
                    self.file_index = self.file_index.saturating_sub(step);
                }
            }
        }
    }

    pub fn page_down(&mut self) {
        let step = 10;
        match self.focused_pane {
            PaneType::Directories => {
                if !self.directories.is_empty() {
                    self.dir_index = (self.dir_index + step).min(self.directories.len().saturating_sub(1));
                }
            }
            PaneType::Files => {
                if !self.files.is_empty() {
                    self.file_index = (self.file_index + step).min(self.files.len().saturating_sub(1));
                }
            }
        }
    }
}

pub fn load_directory(path: &Path, show_hidden: bool) -> (Vec<PathBuf>, Vec<FileItem>) {
    let mut directories = Vec::new();
    let mut files = Vec::new();

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();

            if !show_hidden && name.starts_with('.') {
                continue;
            }

            if entry_path.is_dir() {
                directories.push(entry_path);
            } else {
                let metadata = entry.metadata().ok();
                let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                files.push(FileItem {
                    name,
                    path: entry_path,
                    size,
                    is_dir: false,
                    modified,
                    is_selected: false,
                    metadata: None,
                });
            }
        }
    }

    directories.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
            .cmp(&b.file_name().unwrap_or_default().to_string_lossy().to_lowercase())
    });

    // Prepend . and .. at the very top of the sorted directory list
    let mut sorted_dirs = vec![path.join(".")];
    if path.parent().is_some() {
        sorted_dirs.push(path.join(".."));
    }
    sorted_dirs.extend(directories);

    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    (sorted_dirs, files)
}
