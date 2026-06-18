use std::path::PathBuf;
use rand::seq::SliceRandom;
use rand::thread_rng;

pub struct PlaybackQueue {
    pub items: Vec<PathBuf>,
    pub current_index: Option<usize>,
    pub shuffle_indices: Vec<usize>,
}

impl PlaybackQueue {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            current_index: None,
            shuffle_indices: Vec::new(),
        }
    }

    pub fn add(&mut self, path: PathBuf) {
        if !self.items.contains(&path) {
            self.items.push(path);
        }
    }

    pub fn add_many(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            self.add(path);
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
            // Adjust current index
            if let Some(curr) = self.current_index {
                if curr == index {
                    if self.items.is_empty() {
                        self.current_index = None;
                    } else {
                        self.current_index = Some(curr.min(self.items.len() - 1));
                    }
                } else if curr > index {
                    self.current_index = Some(curr - 1);
                }
            }
            // Rebuild shuffle indices if active
            if !self.shuffle_indices.is_empty() {
                self.regenerate_shuffle_indices();
            }
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.current_index = None;
        self.shuffle_indices.clear();
    }

    pub fn move_up(&mut self, index: usize) {
        if index > 0 && index < self.items.len() {
            self.items.swap(index, index - 1);
            if let Some(curr) = self.current_index {
                if curr == index {
                    self.current_index = Some(index - 1);
                } else if curr == index - 1 {
                    self.current_index = Some(index);
                }
            }
        }
    }

    pub fn move_down(&mut self, index: usize) {
        if index + 1 < self.items.len() {
            self.items.swap(index, index + 1);
            if let Some(curr) = self.current_index {
                if curr == index {
                    self.current_index = Some(index + 1);
                } else if curr == index + 1 {
                    self.current_index = Some(index);
                }
            }
        }
    }

    pub fn current_track(&self) -> Option<PathBuf> {
        self.current_index.and_then(|idx| self.items.get(idx).cloned())
    }

    pub fn next(&mut self, shuffle: bool) -> Option<PathBuf> {
        if self.items.is_empty() {
            return None;
        }

        if shuffle {
            if self.shuffle_indices.is_empty() || self.shuffle_indices.len() != self.items.len() {
                self.regenerate_shuffle_indices();
            }

            if let Some(curr) = self.current_index {
                if let Some(pos) = self.shuffle_indices.iter().position(|&x| x == curr) {
                    let next_pos = pos + 1;
                    if next_pos < self.shuffle_indices.len() {
                        let idx = self.shuffle_indices[next_pos];
                        self.current_index = Some(idx);
                        return Some(self.items[idx].clone());
                    }
                }
            }

            // Default to first shuffled
            if !self.shuffle_indices.is_empty() {
                let idx = self.shuffle_indices[0];
                self.current_index = Some(idx);
                return Some(self.items[idx].clone());
            }
        } else {
            if let Some(curr) = self.current_index {
                let next_idx = curr + 1;
                if next_idx < self.items.len() {
                    self.current_index = Some(next_idx);
                    return Some(self.items[next_idx].clone());
                }
            } else {
                self.current_index = Some(0);
                return Some(self.items[0].clone());
            }
        }
        None
    }

    pub fn prev(&mut self, shuffle: bool) -> Option<PathBuf> {
        if self.items.is_empty() {
            return None;
        }

        if shuffle {
            if self.shuffle_indices.is_empty() || self.shuffle_indices.len() != self.items.len() {
                self.regenerate_shuffle_indices();
            }

            if let Some(curr) = self.current_index {
                if let Some(pos) = self.shuffle_indices.iter().position(|&x| x == curr) {
                    if pos > 0 {
                        let prev_pos = pos - 1;
                        let idx = self.shuffle_indices[prev_pos];
                        self.current_index = Some(idx);
                        return Some(self.items[idx].clone());
                    }
                }
            }
            if !self.shuffle_indices.is_empty() {
                let idx = self.shuffle_indices[self.shuffle_indices.len() - 1];
                self.current_index = Some(idx);
                return Some(self.items[idx].clone());
            }
        } else {
            if let Some(curr) = self.current_index {
                if curr > 0 {
                    let prev_idx = curr - 1;
                    self.current_index = Some(prev_idx);
                    return Some(self.items[prev_idx].clone());
                }
            } else {
                self.current_index = Some(0);
                return Some(self.items[0].clone());
            }
        }
        None
    }

    pub fn shuffle_items(&mut self) {
        self.regenerate_shuffle_indices();
    }

    fn regenerate_shuffle_indices(&mut self) {
        let mut indices: Vec<usize> = (0..self.items.len()).collect();
        indices.shuffle(&mut thread_rng());
        if let Some(curr) = self.current_index {
            if let Some(pos) = indices.iter().position(|&x| x == curr) {
                indices.remove(pos);
            }
            indices.insert(0, curr);
        }
        self.shuffle_indices = indices;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_add_clear() {
        let mut queue = PlaybackQueue::new();
        queue.add(PathBuf::from("song1.mp3"));
        queue.add(PathBuf::from("song2.mp3"));
        queue.add(PathBuf::from("song1.mp3")); // duplicate
        assert_eq!(queue.items.len(), 2);

        queue.clear();
        assert_eq!(queue.items.len(), 0);
        assert_eq!(queue.current_index, None);
    }

    #[test]
    fn test_queue_navigation() {
        let mut queue = PlaybackQueue::new();
        queue.add(PathBuf::from("song1.mp3"));
        queue.add(PathBuf::from("song2.mp3"));
        queue.add(PathBuf::from("song3.mp3"));

        // Normal linear flow
        assert_eq!(queue.next(false), Some(PathBuf::from("song1.mp3")));
        assert_eq!(queue.current_index, Some(0));

        assert_eq!(queue.next(false), Some(PathBuf::from("song2.mp3")));
        assert_eq!(queue.current_index, Some(1));

        assert_eq!(queue.prev(false), Some(PathBuf::from("song1.mp3")));
        assert_eq!(queue.current_index, Some(0));

        // Skip to end and try next
        queue.current_index = Some(2);
        assert_eq!(queue.next(false), None);
    }

    #[test]
    fn test_queue_reordering() {
        let mut queue = PlaybackQueue::new();
        queue.add(PathBuf::from("song1.mp3"));
        queue.add(PathBuf::from("song2.mp3"));
        queue.add(PathBuf::from("song3.mp3"));

        queue.current_index = Some(1); // playing song2.mp3
        queue.move_up(1); // swap song2 and song1
        assert_eq!(queue.items[0], PathBuf::from("song2.mp3"));
        assert_eq!(queue.items[1], PathBuf::from("song1.mp3"));
        assert_eq!(queue.current_index, Some(0)); // check track index moved

        queue.move_down(0); // swap song2 and song1 back
        assert_eq!(queue.items[0], PathBuf::from("song1.mp3"));
        assert_eq!(queue.items[1], PathBuf::from("song2.mp3"));
        assert_eq!(queue.current_index, Some(1));

        queue.remove(1); // remove song2.mp3
        assert_eq!(queue.items.len(), 2);
        assert_eq!(queue.items[1], PathBuf::from("song3.mp3"));
    }
}
