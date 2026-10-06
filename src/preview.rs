use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

type PreviewResult<K, T> = (K, Result<T, String>);

pub type PreviewLoader<T> = LatestLoader<PathBuf, T>;

struct WorkerState<K, T> {
    generation: u64,
    pending: Option<K>,
    result: Option<PreviewResult<K, T>>,
    shutdown: bool,
}

/// One worker per preview kind; navigation replaces queued work instead of spawning threads.
pub struct LatestLoader<K, T> {
    shared: Arc<(Mutex<WorkerState<K, T>>, Condvar)>,
}

impl<K: Send + 'static, T: Send + 'static> LatestLoader<K, T> {
    pub fn new(load: impl Fn(&K) -> Result<T, String> + Send + 'static) -> Self {
        let shared = Arc::new((
            Mutex::new(WorkerState {
                generation: 0,
                pending: None,
                result: None,
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let worker = Arc::clone(&shared);
        thread::spawn(move || {
            loop {
                let (lock, wake) = &*worker;
                let mut state = lock.lock().unwrap();
                while state.pending.is_none() && !state.shutdown {
                    state = wake.wait(state).unwrap();
                }
                if state.shutdown {
                    break;
                }
                let path = state.pending.take().unwrap();
                let generation = state.generation;
                drop(state);
                let result = load(&path);
                let mut state = lock.lock().unwrap();
                if state.generation == generation && !state.shutdown {
                    state.result = Some((path, result));
                }
            }
        });
        Self { shared }
    }

    pub fn request(&self, path: Option<K>) {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.pending = path;
        state.result = None;
        wake.notify_one();
    }

    pub fn poll(&self) -> Option<PreviewResult<K, T>> {
        self.shared.0.lock().unwrap().result.take()
    }
}

impl<K, T> Drop for LatestLoader<K, T> {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap().shutdown = true;
        wake.notify_one();
    }
}

pub fn load_image(path: &Path) -> Result<image::DynamicImage, String> {
    let mut reader = image::io::Reader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?;
    Ok(img.thumbnail(1024, 1024))
}

pub fn load_text(path: &Path) -> Result<Vec<String>, String> {
    const MAX_BYTES: u64 = 256 * 1024;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let truncated = bytes.len() as u64 > MAX_BYTES;
    bytes.truncate(MAX_BYTES as usize);
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<_> = text.lines().take(5000).map(str::to_string).collect();
    if truncated || text.lines().count() > 5000 {
        lines.push("[Preview truncated]".to_string());
    }
    Ok(lines)
}

pub type GraphicsProtocol = Box<dyn ratatui_image::protocol::ResizeProtocol>;

pub struct GraphicsRequest {
    pub path: PathBuf,
    pub width: u16,
    pub height: u16,
    pub image: Arc<image::DynamicImage>,
    pub picker: ratatui_image::picker::Picker,
    pub kitty_id: u8,
}

pub fn prepare_graphics(request: &GraphicsRequest) -> Result<GraphicsProtocol, String> {
    use ratatui_image::picker::ProtocolType;
    use ratatui_image::protocol::{kitty::KittyState, sixel::SixelState};
    // Each pane owns a fixed ID, so navigation cannot overwrite the other pane's art.
    let font = request.picker.font_size;
    let image = request.image.resize(
        u32::from(request.width) * u32::from(font.0),
        u32::from(request.height) * u32::from(font.1),
        image::imageops::FilterType::Triangle,
    );
    let source = ratatui_image::protocol::ImageSource::new(image, font);
    let mut protocol: GraphicsProtocol = match request.picker.protocol_type {
        ProtocolType::Kitty => Box::new(KittyState::new(source, request.kitty_id)),
        ProtocolType::Sixel => Box::new(SixelState::new(source)),
        ProtocolType::Halfblocks => return Err("Graphics protocol unavailable".to_string()),
    };
    protocol.resize_encode(
        &ratatui_image::Resize::Fit,
        request.picker.background_color,
        ratatui::layout::Rect::new(0, 0, request.width, request.height),
    );
    Ok(protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    #[test]
    fn graphics_are_encoded_before_rendering() {
        use ratatui::{buffer::Buffer, layout::Rect};
        use ratatui_image::picker::{Picker, ProtocolType};
        let area = Rect::new(0, 0, 20, 10);
        for protocol_type in [ProtocolType::Kitty, ProtocolType::Sixel] {
            let mut picker = Picker::new((8, 16));
            picker.protocol_type = protocol_type;
            let request = GraphicsRequest {
                path: "fixture.png".into(),
                width: area.width,
                height: area.height,
                image: Arc::new(image::DynamicImage::new_rgb8(32, 32)),
                picker,
                kitty_id: 2,
            };
            let mut protocol = prepare_graphics(&request).unwrap();
            let mut buffer = Buffer::empty(area);
            protocol.render(area, &mut buffer);
            let first = buffer.get(0, 0).symbol();
            if protocol_type == ProtocolType::Kitty {
                assert!(first.contains("i=2,a=T"));
                let mut next = Buffer::empty(area);
                protocol.render(area, &mut next);
                assert!(!next.get(0, 0).symbol().contains("a=T"));
            } else {
                assert!(first.contains("\x1bP"));
            }
        }
    }

    #[test]
    fn replaces_pending_work_and_rejects_stale_results() {
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = channel();
        let loader = PreviewLoader::new(move |path| {
            started_tx.send(path.to_path_buf()).unwrap();
            release_rx.recv().unwrap();
            Ok(path.to_path_buf())
        });
        loader.request(Some("first".into()));
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            PathBuf::from("first")
        );
        loader.request(Some("skipped".into()));
        loader.request(Some("latest".into()));
        release_tx.send(()).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            PathBuf::from("latest")
        );
        assert!(loader.poll().is_none());
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some((path, result)) = loader.poll() {
                assert_eq!(path, PathBuf::from("latest"));
                assert_eq!(result.unwrap(), path);
                break;
            }
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
    }

    #[test]
    fn invalid_image_finishes_with_error() {
        let path = std::env::temp_dir().join(format!("stash-invalid-{}.png", std::process::id()));
        std::fs::write(&path, b"invalid").unwrap();
        assert!(load_image(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn large_text_is_bounded() {
        let path = std::env::temp_dir().join(format!("stash-large-{}.txt", std::process::id()));
        std::fs::write(&path, "a\n".repeat(200_000)).unwrap();
        let lines = load_text(&path).unwrap();
        assert_eq!(lines.len(), 5001);
        assert_eq!(lines.last().unwrap(), "[Preview truncated]");
        std::fs::remove_file(path).unwrap();
    }
}
