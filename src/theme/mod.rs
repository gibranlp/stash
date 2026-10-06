// STASH pulls its colors from SpectrumOS's theme engine (`spectrum-theme`) when it's
// present, instead of hardcoding its own palette. SpectrumOS's own way to get colors
// (see ~/SpectrumOS2 ADR 0004, "Semantic palette and atomic generations") is:
//
//   - Extractors (wallpaper analysis, matugen, or a fixed palette) produce a small
//     versioned schema: a background, two surfaces, a foreground, a muted tone, two
//     accents, and success/warning/error roles, each a `#rrggbb` hex color.
//   - That schema is rendered to `$XDG_CACHE_HOME/spectrumos/theme/current/palette.json`
//     (one file among many per-app renderings) and activated by atomically swapping the
//     `current` symlink to a new "generation" directory — so a reader either sees a
//     complete palette or the previous one, never a half-written one.
//
// This module reads that same file directly and maps its ten roles onto ratatui
// `Color`s. When SpectrumOS isn't installed (or the palette can't be read), STASH
// falls back to the plain colors it always shipped with — the file's absence changes
// nothing.
use ratatui::style::Color;
use serde::Deserialize;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::events::Event;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Color,
    // STASH never gives a widget its own background fill (everything draws on the
    // terminal's default background), so these two roles of the schema have nowhere
    // to be read yet. Keep them for a full, valid `Theme` and any future widget that
    // wants one.
    #[allow(dead_code)]
    pub surface: Color,
    #[allow(dead_code)]
    pub surface_alt: Color,
    pub foreground: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
}

impl Theme {
    /// STASH's original look, used whenever a SpectrumOS palette isn't available.
    pub const FALLBACK: Theme = Theme {
        background: Color::Black,
        surface: Color::Black,
        surface_alt: Color::DarkGray,
        foreground: Color::White,
        muted: Color::DarkGray,
        accent: Color::Cyan,
        accent_alt: Color::Magenta,
        success: Color::Green,
        warning: Color::Yellow,
        error: Color::Red,
    };

    /// `$XDG_CACHE_HOME/spectrumos/theme/current/palette.json` — matches
    /// `spectrum-theme`'s own `default_output_root()`, so STASH reads exactly the file
    /// every other Spectrum-aware app reads.
    pub fn palette_path() -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("spectrumos/theme/current/palette.json")
    }

    pub fn load() -> Theme {
        Self::read(&Self::palette_path()).unwrap_or(Self::FALLBACK)
    }

    fn read(path: &std::path::Path) -> Option<Theme> {
        let content = std::fs::read_to_string(path).ok()?;
        let parsed: PaletteFile = serde_json::from_str(&content).ok()?;
        if parsed.schema_version != 1 {
            return None;
        }
        let role = |name: &str| parse_hex(parsed.roles.get(name)?);
        Some(Theme {
            background: role("background")?,
            surface: role("surface")?,
            surface_alt: role("surface_alt")?,
            foreground: role("foreground")?,
            muted: role("muted")?,
            accent: role("accent")?,
            accent_alt: role("accent_alt")?,
            success: role("success")?,
            warning: role("warning")?,
            error: role("error")?,
        })
    }

    /// Watches SpectrumOS's `theme` cache directory so a wallpaper or theme change
    /// made while STASH is already running (the `current` symlink getting swapped to a
    /// new generation) sends `Event::ThemeChanged` and gets picked up live. Mirrors
    /// `library::start_library_watcher`. Returns `None` (no live-reload, no error) when
    /// SpectrumOS's theme directory doesn't exist at all.
    pub fn start_watcher(tx: std::sync::mpsc::Sender<Event>) -> Option<notify::RecommendedWatcher> {
        use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};

        let theme_dir = dirs::cache_dir()?.join("spectrumos/theme");
        if !theme_dir.is_dir() {
            return None;
        }

        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    use notify::EventKind::*;
                    if matches!(event.kind, Any | Create(_) | Modify(_) | Remove(_)) {
                        let _ = tx.send(Event::ThemeChanged);
                    }
                }
            },
            NotifyConfig::default(),
        )
        .ok()?;

        // Non-recursive: `current` is a single symlink swapped atomically by
        // spectrum-theme, so watching this directory (not following it) is enough to
        // catch every swap without also watching every retained past generation.
        watcher.watch(&theme_dir, RecursiveMode::NonRecursive).ok()?;
        Some(watcher)
    }
}

#[derive(Deserialize)]
struct PaletteFile {
    schema_version: u32,
    roles: HashMap<String, String>,
}

fn parse_hex(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

thread_local! {
    // The UI thread's snapshot of the active theme for the frame currently being
    // drawn. `ui::render` sets this once per frame from `App::theme` (the value that
    // actually gets reloaded on `Event::ThemeChanged`); every rendering helper below
    // it in the call stack reads it with `current()` without needing `App` threaded
    // through, since most of them (popups, the help screen, ...) don't take it.
    static CURRENT: Cell<Theme> = const { Cell::new(Theme::FALLBACK) };
}

pub fn set_current(theme: Theme) {
    CURRENT.with(|c| c.set(theme));
}

pub fn current() -> Theme {
    CURRENT.with(|c| c.get())
}
