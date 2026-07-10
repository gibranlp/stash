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
    pub files: Vec<FileItem>,
    pub file_index: usize,
    pub focused_pane: PaneType,
    pub selected_paths: HashSet<PathBuf>,
    pub show_hidden: bool,
    pub expanded_paths: HashSet<PathBuf>,
    pub shift_start: Option<usize>,
}

impl BrowserState {
    pub fn new(initial_dir: PathBuf, show_hidden: bool) -> Self {
        let mut state = Self {
            current_dir: initial_dir,
            files: Vec::new(),
            file_index: 0,
            focused_pane: PaneType::Files,
            selected_paths: HashSet::new(),
            show_hidden,
            expanded_paths: HashSet::new(),
            shift_start: None,
        };
        state.refresh();
        state
    }

    // Le pegamos un refresh al árbol: reconstruye la lista de archivos
    // y ajusta el índice pa que no se salga de rango
    pub fn refresh(&mut self) {
        let mut new_files = Vec::new();
        build_tree(
            &self.current_dir,
            0,
            self.show_hidden,
            &self.expanded_paths,
            &self.selected_paths,
            &mut new_files,
        );

        // On macOS, /Volumes/ may contain a symlink for the root system volume — strip it.
        #[cfg(target_os = "macos")]
        if self.current_dir == Path::new("/Volumes") {
            new_files.retain(|f| !f.path.is_symlink());
        }

        // When browsing /media, also surface drives from /run/media/*/
        // (udisks2 mounts there on Arch/systemd systems)
        if self.current_dir == Path::new("/media") {
            let mut injected: Vec<(String, PathBuf)> = Vec::new();
            if let Ok(user_dirs) = fs::read_dir("/run/media") {
                for user_entry in user_dirs.flatten() {
                    let user_dir = user_entry.path();
                    if !user_dir.is_dir() { continue; }
                    if let Ok(drives) = fs::read_dir(&user_dir) {
                        for drive_entry in drives.flatten() {
                            let drive_path = drive_entry.path();
                            if drive_path.is_dir()
                                && !new_files.iter().any(|f| f.path == drive_path)
                            {
                                let name = drive_entry.file_name().to_string_lossy().into_owned();
                                injected.push((name, drive_path));
                            }
                        }
                    }
                }
            }
            injected.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
            for (name, drive_path) in injected {
                let is_selected = self.selected_paths.contains(&drive_path);
                let is_expanded = self.expanded_paths.contains(&drive_path);
                new_files.push(crate::models::FileItem {
                    name,
                    path: drive_path.clone(),
                    size: 0,
                    is_dir: true,
                    modified: None,
                    is_selected,
                    metadata: None,
                    depth: 0,
                    is_expanded,
                });
                if is_expanded {
                    build_tree(
                        &drive_path,
                        1,
                        self.show_hidden,
                        &self.expanded_paths,
                        &self.selected_paths,
                        &mut new_files,
                    );
                }
            }
        }

        self.files = new_files;

        if self.files.is_empty() {
            self.file_index = 0;
        } else if self.file_index >= self.files.len() {
            self.file_index = self.files.len() - 1;
        }
    }

    pub fn move_up(&mut self) {
        if !self.files.is_empty() && self.file_index > 0 {
            self.file_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.files.is_empty() && self.file_index + 1 < self.files.len() {
            self.file_index += 1;
        }
    }

    // Subimos al directorio padre; expandimos el que dejamos
    // pa que el usuario lo siga viendo en el árbol y no se pierda
    pub fn go_to_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            let old_dir = self.current_dir.clone();
            self.current_dir = parent.to_path_buf();
            self.expanded_paths.insert(old_dir.clone());
            self.refresh();

            // Dejamos el cursor apuntando al folder del que salimos
            if let Some(pos) = self.files.iter().position(|f| f.path == old_dir) {
                self.file_index = pos;
            } else {
                self.file_index = 0;
            }
        }
    }

    pub fn toggle_select_highlighted(&mut self) {
        if !self.files.is_empty() && self.file_index < self.files.len() {
            let file = &mut self.files[self.file_index];
            file.is_selected = !file.is_selected;
            if file.is_selected {
                self.selected_paths.insert(file.path.clone());
            } else {
                self.selected_paths.remove(&file.path.clone());
            }
        }
    }


    // Selección por rango tipo shift+click: el estado (seleccionar/deseleccionar)
    // se decide según cómo esté el item donde está parado el cursor ahorita
    pub fn toggle_range_selection(&mut self, start: usize, end: usize) {
        let min_idx = start.min(end);
        let max_idx = start.max(end).min(self.files.len().saturating_sub(1));

        if min_idx >= self.files.len() {
            return;
        }

        let target_state = if self.file_index < self.files.len() {
            !self.files[self.file_index].is_selected
        } else {
            true
        };

        for idx in min_idx..=max_idx {
            let file = &self.files[idx];
            let path = file.path.clone();
            if file.is_dir {
                if target_state {
                    self.selected_paths.insert(path.clone());
                } else {
                    self.selected_paths.remove(&path);
                    self.selected_paths.retain(|p| !p.starts_with(&path));
                }
            } else {
                if target_state {
                    self.selected_paths.insert(path);
                } else {
                    self.selected_paths.remove(&path);
                }
            }
        }
        self.refresh();
    }

    // Toggle de tres estados para carpetas:
    //   [ ] → [*]  selecciona la carpeta + todos los archivos de adentro
    //   [*] → [-]  quita la carpeta del selected pero deja los archivos
    //   [-] → [ ]  limpia todo lo que esté seleccionado adentro
    pub fn toggle_folder_select(&mut self, idx: usize) {
        if idx >= self.files.len() || !self.files[idx].is_dir {
            return;
        }
        let folder_path = self.files[idx].path.clone();
        let already_selected = self.selected_paths.contains(&folder_path)
            || self.selected_paths.iter().any(|p| p != &folder_path && p.starts_with(&folder_path));

        if already_selected {
            // Deselect folder and any individually-selected children
            self.selected_paths.remove(&folder_path);
            self.selected_paths.retain(|p| !p.starts_with(&folder_path));
            self.files[idx].is_selected = false;
            for file in &mut self.files {
                if file.path.starts_with(&folder_path) {
                    file.is_selected = false;
                }
            }
        } else {
            // Select only the folder itself — operations expand contents recursively as needed
            self.selected_paths.insert(folder_path.clone());
            self.files[idx].is_selected = true;
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
        self.file_index = self.file_index.saturating_sub(step);
    }

    pub fn page_down(&mut self) {
        let step = 10;
        if !self.files.is_empty() {
            self.file_index = (self.file_index + step).min(self.files.len().saturating_sub(1));
        }
    }
}

// Construye el árbol de archivos de forma recursiva. Los directorios van primero,
// luego los archivos, todo ordenado sin importar mayúsculas. Si una carpeta está
// en expanded_paths, se mete a construir su contenido también (recursión).
pub fn build_tree(
    path: &Path,
    depth: usize,
    show_hidden: bool,
    expanded_paths: &HashSet<PathBuf>,
    selected_paths: &HashSet<PathBuf>,
    out_files: &mut Vec<FileItem>,
) {
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
                directories.push((name, entry_path));
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
                    depth,
                    is_expanded: false,
                });
            }
        }
    }

    directories.sort_by_key(|a| a.0.to_lowercase());
    files.sort_by_key(|a| a.name.to_lowercase());

    for (name, entry_path) in directories {
        let is_expanded = expanded_paths.contains(&entry_path);
        let is_selected = selected_paths.contains(&entry_path);

        out_files.push(FileItem {
            name,
            path: entry_path.clone(),
            size: 0,
            is_dir: true,
            modified: None,
            is_selected,
            metadata: None,
            depth,
            is_expanded,
        });

        if is_expanded {
            build_tree(
                &entry_path,
                depth + 1,
                show_hidden,
                expanded_paths,
                selected_paths,
                out_files,
            );
        }
    }

    for mut file in files {
        if selected_paths.contains(&file.path) {
            file.is_selected = true;
        }
        out_files.push(file);
    }
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
            .join("test_stash_dir_preview_clean");

        if test_root.exists() {
            let _ = fs::remove_dir_all(&test_root);
        }

        let subdir_a = test_root.join("subdir_a");
        let subdir_b = test_root.join("subdir_b");
        create_dir_all(&subdir_a).unwrap();
        create_dir_all(&subdir_b).unwrap();

        File::create(subdir_a.join("file1.mp3")).unwrap();
        File::create(subdir_b.join("file2.wav")).unwrap();
        File::create(test_root.join("root_file.mp3")).unwrap();

        let mut browser = BrowserState::new(test_root.clone(), false);

        assert_eq!(browser.current_dir, test_root);
        assert_eq!(browser.files.len(), 3);
        assert_eq!(browser.files[0].name, "subdir_a");
        assert!(browser.files[0].is_dir);
        assert_eq!(browser.files[0].depth, 0);

        browser.file_index = 0;
        browser.toggle_expand_highlighted();

        assert_eq!(browser.files.len(), 4);
        assert_eq!(browser.files[1].name, "file1.mp3");
        assert_eq!(browser.files[1].depth, 1);

        let _ = fs::remove_dir_all(&test_root);
    }

    #[test]
    fn test_range_selection() {
        let test_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_stash_range_selection");

        if test_root.exists() {
            let _ = fs::remove_dir_all(&test_root);
        }
        create_dir_all(&test_root).unwrap();

        File::create(test_root.join("a.txt")).unwrap();
        File::create(test_root.join("b.txt")).unwrap();
        File::create(test_root.join("c.txt")).unwrap();

        let mut browser = BrowserState::new(test_root.clone(), false);
        assert_eq!(browser.files.len(), 3);

        browser.shift_start = Some(0);
        browser.file_index = 2;

        browser.toggle_range_selection(0, 2);

        assert!(browser.files[0].is_selected);
        assert!(browser.files[1].is_selected);
        assert!(browser.files[2].is_selected);

        browser.toggle_range_selection(0, 2);
        assert!(!browser.files[0].is_selected);
        assert!(!browser.files[1].is_selected);
        assert!(!browser.files[2].is_selected);

        let _ = fs::remove_dir_all(&test_root);
    }
}
