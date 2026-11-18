use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
    pub fn new(mut load: impl FnMut(&K) -> Result<T, String> + Send + 'static) -> Self {
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

pub const VINYL_FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const VINYL_PHASES: u16 = 60_000;
const VINYL_PERIOD: Duration = Duration::from_secs(20);

/// Animation time is independent of audio progress, which advances in chunks
/// and can jump when seeking. Pausing freezes the angle without resetting it.
#[derive(Default)]
pub struct VinylClock {
    last_update: Option<Instant>,
    elapsed: Duration,
    playing: bool,
}

impl VinylClock {
    pub fn update(&mut self, now: Instant, playing: bool) {
        if self.playing
            && let Some(last) = self.last_update
        {
            self.elapsed += now.saturating_duration_since(last);
            self.elapsed =
                Duration::from_nanos((self.elapsed.as_nanos() % VINYL_PERIOD.as_nanos()) as u64);
        }
        self.last_update = Some(now);
        self.playing = playing;
    }

    pub fn phase(&self, ahead: Duration) -> u16 {
        let elapsed = self.elapsed + if self.playing { ahead } else { Duration::ZERO };
        ((elapsed.as_nanos() % VINYL_PERIOD.as_nanos()) * u128::from(VINYL_PHASES)
            / VINYL_PERIOD.as_nanos()) as u16
    }
}

/// Square center crop, circular sleeve art, dark vinyl rim and spindle hole.
/// A high precision phase describes a 20-second revolution, independent of frame rate.
pub fn vinyl_frame(image: &image::DynamicImage, side: u32, phase: u16) -> image::DynamicImage {
    let side = side.max(1);
    let source = image.resize_to_fill(side, side, image::imageops::FilterType::Triangle);
    rotate_vinyl(&source, phase)
}

fn rotate_vinyl(source: &image::DynamicImage, phase: u16) -> image::DynamicImage {
    use image::{GenericImageView, Rgba};
    let side = source.width();
    let angle = f32::from(phase) * std::f32::consts::TAU / f32::from(VINYL_PHASES);
    let (sin, cos) = angle.sin_cos();
    let radius = side as f32 / 2.0;
    let output = image::RgbaImage::from_fn(side, side, |x, y| {
        let dx = (x as f32 + 0.5 - radius) / radius;
        let dy = (y as f32 + 0.5 - radius) / radius;
        let distance = dx.hypot(dy);
        if distance > 1.0 || distance < 0.045 {
            return Rgba([0, 0, 0, 0]);
        }
        let edge = ((1.0 - distance) * radius).clamp(0.0, 1.0);
        if distance > 0.93 {
            let groove = if (distance * radius).sin() > 0.0 {
                28
            } else {
                16
            };
            return Rgba([groove, groove, groove, (255.0 * edge) as u8]);
        }
        let sx = ((cos * dx + sin * dy) * radius + radius - 0.5).clamp(0.0, side as f32 - 1.0);
        let sy = ((-sin * dx + cos * dy) * radius + radius - 0.5).clamp(0.0, side as f32 - 1.0);
        // Bilinear sampling avoids the shimmer caused by whole-pixel jumps
        // as fine details in the album art rotate between adjacent pixels.
        let x0 = sx.floor() as u32;
        let y0 = sy.floor() as u32;
        let x1 = (x0 + 1).min(side - 1);
        let y1 = (y0 + 1).min(side - 1);
        let tx = sx - x0 as f32;
        let ty = sy - y0 as f32;
        let samples = [
            source.get_pixel(x0, y0),
            source.get_pixel(x1, y0),
            source.get_pixel(x0, y1),
            source.get_pixel(x1, y1),
        ];
        let mut pixel = [0; 4];
        for channel in 0..4 {
            let top = samples[0][channel] as f32 * (1.0 - tx) + samples[1][channel] as f32 * tx;
            let bottom = samples[2][channel] as f32 * (1.0 - tx) + samples[3][channel] as f32 * tx;
            pixel[channel] = (top * (1.0 - ty) + bottom * ty).round() as u8;
        }
        Rgba(pixel)
    });
    image::DynamicImage::ImageRgba8(output)
}

pub type GraphicsProtocol = Box<dyn ratatui_image::protocol::ResizeProtocol>;

pub struct GraphicsRequest {
    pub path: PathBuf,
    pub width: u16,
    pub height: u16,
    pub image: Arc<image::DynamicImage>,
    pub picker: ratatui_image::picker::Picker,
    pub kitty_id: u8,
    pub vinyl_phase: Option<u16>,
}

/// Keep one resized source, not a bank of animation frames. Resizing the full
/// album cover for every frame was both costly and a source of uneven cadence.
pub fn graphics_encoder() -> impl FnMut(&GraphicsRequest) -> Result<GraphicsProtocol, String> + Send
{
    let mut cached: Option<(Arc<image::DynamicImage>, u32, image::DynamicImage)> = None;
    move |request| {
        let image = if let Some(phase) = request.vinyl_phase {
            let font = request.picker.font_size;
            let side = (u32::from(request.width) * u32::from(font.0))
                .min(u32::from(request.height) * u32::from(font.1))
                .clamp(1, 512);
            if !cached.as_ref().is_some_and(|(original, size, _)| {
                Arc::ptr_eq(original, &request.image) && *size == side
            }) {
                cached = Some((
                    Arc::clone(&request.image),
                    side,
                    request
                        .image
                        .resize_to_fill(side, side, image::imageops::FilterType::Triangle),
                ));
            }
            let frame = rotate_vinyl(&cached.as_ref().unwrap().2, phase);
            // The current Kitty encoder sends RGB, dropping alpha. Composite
            // explicitly so transparent corners and the hole don't become black.
            composite_background(
                &frame,
                request
                    .picker
                    .background_color
                    .unwrap_or(image::Rgb([0, 0, 0])),
            )
        } else {
            cached = None;
            let font = request.picker.font_size;
            request.image.resize(
                u32::from(request.width) * u32::from(font.0),
                u32::from(request.height) * u32::from(font.1),
                image::imageops::FilterType::Triangle,
            )
        };
        encode_graphics(request, image)
    }
}

fn composite_background(frame: &image::DynamicImage, color: image::Rgb<u8>) -> image::DynamicImage {
    let mut background = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        frame.width(),
        frame.height(),
        color,
    ));
    image::imageops::overlay(&mut background, frame, 0, 0);
    background
}

pub fn prepare_graphics(request: &GraphicsRequest) -> Result<GraphicsProtocol, String> {
    graphics_encoder()(request)
}

fn encode_graphics(
    request: &GraphicsRequest,
    image: image::DynamicImage,
) -> Result<GraphicsProtocol, String> {
    use ratatui_image::picker::ProtocolType;
    use ratatui_image::protocol::{kitty::KittyState, sixel::SixelState};
    let font = request.picker.font_size;
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
    fn vinyl_clock_moves_every_frame_and_resumes_without_a_jump() {
        let mut clock = VinylClock::default();
        let start = Instant::now();
        clock.update(start, true);
        let mut previous = 0;
        for frame in 1..=600 {
            let now = start + VINYL_FRAME_INTERVAL * frame;
            clock.update(now, true);
            let phase = clock.phase(Duration::ZERO);
            assert_eq!(phase - previous, 50, "Uneven rotation at frame {frame}");
            assert_eq!(clock.phase(VINYL_FRAME_INTERVAL), phase + 50);
            previous = phase;
        }
        let pause_at = start + VINYL_FRAME_INTERVAL * 600;
        clock.update(pause_at, false);
        let paused = clock.phase(Duration::ZERO);
        clock.update(pause_at + Duration::from_secs(10), false);
        assert_eq!(clock.phase(VINYL_FRAME_INTERVAL), paused);
        clock.update(pause_at + Duration::from_secs(20), true);
        assert_eq!(clock.phase(Duration::ZERO), paused);
        clock.update(
            pause_at + Duration::from_secs(20) + VINYL_FRAME_INTERVAL,
            true,
        );
        assert_eq!(clock.phase(Duration::ZERO), paused + 50);
    }

    #[test]
    fn vinyl_is_round_with_a_hole_and_rotating_art() {
        let source =
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(100, 100, |x, _| {
                if x < 50 {
                    image::Rgba([255, 0, 0, 255])
                } else {
                    image::Rgba([0, 0, 255, 255])
                }
            }));
        let frame = vinyl_frame(&source, 100, 0).to_rgba8();
        assert_eq!(frame.get_pixel(0, 0)[3], 0);
        assert_eq!(frame.get_pixel(50, 50)[3], 0);
        assert_eq!(*frame.get_pixel(25, 50), image::Rgba([255, 0, 0, 255]));
        assert_eq!(*frame.get_pixel(75, 50), image::Rgba([0, 0, 255, 255]));
        let rotated = vinyl_frame(&source, 100, 30_000).to_rgba8();
        assert_eq!(*rotated.get_pixel(25, 50), image::Rgba([0, 0, 255, 255]));
        assert_eq!(frame, vinyl_frame(&source, 100, 60_000).to_rgba8());
        let between_pixels = vinyl_frame(&source, 100, 3).to_rgba8();
        assert!(
            between_pixels
                .pixels()
                .any(|p| p[0] > 0 && p[0] < 255 && p[2] > 0)
        );
    }

    #[test]
    fn transparent_vinyl_pixels_match_the_background() {
        let source = image::DynamicImage::new_rgb8(64, 64);
        let frame = vinyl_frame(&source, 100, 0);
        let color = image::Rgb([31, 42, 53]);
        let composited = composite_background(&frame, color).to_rgb8();
        assert_eq!(*composited.get_pixel(0, 0), color);
        assert_eq!(*composited.get_pixel(50, 50), color);
        assert_ne!(*composited.get_pixel(25, 50), color);
    }

    #[test]
    #[ignore = "manual animation timing and resident-memory probe"]
    fn vinyl_resource_probe() {
        let mut picker = ratatui_image::picker::Picker::new((8, 16));
        picker.protocol_type = ratatui_image::picker::ProtocolType::Kitty;
        picker.background_color = Some(image::Rgb([31, 42, 53]));
        let mut request = GraphicsRequest {
            path: "probe.png".into(),
            width: 24,
            height: 12,
            image: Arc::new(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
                1024,
                1024,
                |x, y| image::Rgb([x as u8, y as u8, (x ^ y) as u8]),
            ))),
            picker,
            kitty_id: 2,
            vinyl_phase: Some(0),
        };
        let mut encode = graphics_encoder();
        let rss = || {
            std::fs::read_to_string("/proc/self/status")
                .unwrap_or_default()
                .lines()
                .filter(|s| s.starts_with("VmRSS:") || s.starts_with("VmHWM:"))
                .collect::<Vec<_>>()
                .join("; ")
        };
        eprintln!("source loaded: {}", rss());
        let mut slots = [None, None];
        let started = Instant::now();
        for frame in 0..600 {
            request.vinyl_phase = Some(frame * 50 % VINYL_PHASES);
            request.kitty_id = 2 + (frame % 2) as u8;
            slots[frame as usize % 2] = Some(encode(&request).unwrap());
            if frame == 59 {
                eprintln!("warm: {}", rss());
            }
        }
        eprintln!("600 frames: {:?}; final: {}", started.elapsed(), rss());
    }

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
                vinyl_phase: Some(0),
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
