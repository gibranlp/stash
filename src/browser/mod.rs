use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use crate::models::FileItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneType {
    Directories,
    Files,
    Preview,
}

pub struct BrowserState {
    pub current_dir: PathBuf,
    pub directories: Vec<PathBuf>,
    pub directories_has_subdirs: Vec<bool>,
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
            directories_has_subdirs: Vec::new(),
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

    pub fn get_highlighted_dir(&self) -> PathBuf {
        if self.directories.is_empty() || self.dir_index >= self.directories.len() {
            return self.current_dir.clone();
        }
        let dir = &self.directories[self.dir_index];
        // The first directory in self.directories is always the current directory (".")
        if self.dir_index == 0 {
            self.current_dir.clone()
        } else if dir.ends_with("..") {
            self.current_dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| self.current_dir.clone())
        } else {
            dir.clone()
        }
    }

    pub fn refresh(&mut self) {
        let (dirs, _) = load_directory(&self.current_dir, self.show_hidden);
        self.directories = dirs;

        // Compute if each directory has subdirectories
        self.directories_has_subdirs = self.directories
            .iter()
            .map(|dir| {
                if dir.ends_with(".") || dir.ends_with("..") {
                    return false;
                }
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let entry_path = entry.path();
                        if entry_path.is_dir() {
                            let name = entry.file_name().to_string_lossy().into_owned();
                            if self.show_hidden || !name.starts_with('.') {
                                return true;
                            }
                        }
                    }
                }
                false
            })
            .collect();

        // Ensure indices are within bounds
        if self.directories.is_empty() {
            self.dir_index = 0;
        } else if self.dir_index >= self.directories.len() {
            self.dir_index = self.directories.len() - 1;
        }

        let preview_dir = self.get_highlighted_dir();
        let (_, mut fls) = load_directory(&preview_dir, self.show_hidden);

        // Sync selections
        for file in &mut fls {
            if self.selected_paths.contains(&file.path) {
                file.is_selected = true;
            }
        }

        self.files = fls;

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
                    self.refresh();
                }
            }
            PaneType::Files => {
                if !self.files.is_empty() && self.file_index > 0 {
                    self.file_index -= 1;
                }
            }
            PaneType::Preview => {}
        }
    }

    pub fn move_down(&mut self) {
        match self.focused_pane {
            PaneType::Directories => {
                if !self.directories.is_empty() && self.dir_index + 1 < self.directories.len() {
                    self.dir_index += 1;
                    self.refresh();
                }
            }
            PaneType::Files => {
                if !self.files.is_empty() && self.file_index + 1 < self.files.len() {
                    self.file_index += 1;
                }
            }
            PaneType::Preview => {}
        }
    }

    pub fn open_selected_dir(&mut self) {
        if self.focused_pane == PaneType::Directories && !self.directories.is_empty() {
            let resolved = self.get_highlighted_dir();
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
            
            // First load directories of the parent so we can locate old_dir
            let (dirs, _) = load_directory(&self.current_dir, self.show_hidden);
            self.directories = dirs;

            // Try to set directory index to the folder we just exited
            if let Some(pos) = self.directories.iter().position(|d| d == &old_dir) {
                self.dir_index = pos;
            } else {
                self.dir_index = 0;
            }
            self.file_index = 0;
            self.focused_pane = PaneType::Directories;

            self.refresh();
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
        } else if self.focused_pane == PaneType::Directories && !self.directories.is_empty() {
            let path = self.directories[self.dir_index].clone();
            // Don't allow selecting current (".") or parent ("..") directories
            if !path.ends_with(".") && !path.ends_with("..") {
                if self.selected_paths.contains(&path) {
                    self.selected_paths.remove(&path);
                } else {
                    self.selected_paths.insert(path);
                }
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
            } else if path.is_dir() {
                fs::remove_dir_all(path)?;
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
                    self.refresh();
                }
            }
            PaneType::Files => {
                if !self.files.is_empty() {
                    self.file_index = self.file_index.saturating_sub(step);
                }
            }
            PaneType::Preview => {}
        }
    }

    pub fn page_down(&mut self) {
        let step = 10;
        match self.focused_pane {
            PaneType::Directories => {
                if !self.directories.is_empty() {
                    self.dir_index = (self.dir_index + step).min(self.directories.len().saturating_sub(1));
                    self.refresh();
                }
            }
            PaneType::Files => {
                if !self.files.is_empty() {
                    self.file_index = (self.file_index + step).min(self.files.len().saturating_sub(1));
                }
            }
            PaneType::Preview => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, File};

    #[test]
    fn test_directory_preview() {
        let test_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_stash_dir_preview");
        
        // Clean up any old test files first
        if test_root.exists() {
            let _ = fs::remove_dir_all(&test_root);
        }
        
        // Create directory structure
        let subdir_a = test_root.join("subdir_a");
        let subdir_b = test_root.join("subdir_b");
        create_dir_all(&subdir_a).unwrap();
        create_dir_all(&subdir_b).unwrap();
        
        // Create files
        File::create(subdir_a.join("file1.mp3")).unwrap();
        File::create(subdir_b.join("file2.wav")).unwrap();
        File::create(test_root.join("root_file.mp3")).unwrap();

        // Initialize BrowserState
        let mut browser = BrowserState::new(test_root.clone(), false);

        println!("browser.directories = {:?}", browser.directories);

        // Under normal view (highlighting `.`), files should be those in test_root (i.e. root_file.mp3)
        assert_eq!(browser.current_dir, test_root);
        assert!(!browser.directories.is_empty());
        
        // Let's check which index corresponds to `.`, `subdir_a`, `subdir_b`
        let idx_dot = 0;
        let idx_a = browser.directories.iter().position(|d| d == &subdir_a).unwrap();
        let idx_b = browser.directories.iter().position(|d| d == &subdir_b).unwrap();

        // Highlighting `.` (idx_dot)
        browser.dir_index = idx_dot;
        browser.refresh();
        assert_eq!(browser.files.len(), 1);
        assert_eq!(browser.files[0].name, "root_file.mp3");

        // Highlighting `subdir_a` should show `file1.mp3`
        browser.dir_index = idx_a;
        browser.refresh();
        assert_eq!(browser.files.len(), 1);
        assert_eq!(browser.files[0].name, "file1.mp3");

        // Highlighting `subdir_b` should show `file2.wav`
        browser.dir_index = idx_b;
        browser.refresh();
        assert_eq!(browser.files.len(), 1);
        assert_eq!(browser.files[0].name, "file2.wav");

        // Move selection to subdir_a and enter it
        browser.dir_index = idx_a;
        browser.open_selected_dir();
        assert_eq!(browser.current_dir, subdir_a);
        
        // Inside subdir_a, the default selection is `.` which shows `file1.mp3`
        assert_eq!(browser.files.len(), 1);
        assert_eq!(browser.files[0].name, "file1.mp3");

        // Go back up to parent
        browser.go_to_parent();
        assert_eq!(browser.current_dir, test_root);
        
        // Should have returned to highlighting `subdir_a`
        assert_eq!(browser.dir_index, idx_a);
        
        // And the files pane should show `file1.mp3` since `subdir_a` is highlighted
        assert_eq!(browser.files.len(), 1);
        assert_eq!(browser.files[0].name, "file1.mp3");

        // Clean up
        let _ = fs::remove_dir_all(&test_root);
    }
}
