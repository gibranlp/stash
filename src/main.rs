mod app;
mod artwork;
mod audio;
mod automount;
mod browser;
mod collections;
mod config;
mod discord;
mod events;
mod healer;
mod library;
mod models;
mod queue;
mod preview;
mod radio;
mod podcasts;
mod search;
mod shortcuts;
mod stats;
mod theme;
mod ui;
mod updater;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, BeginSynchronizedUpdate, EndSynchronizedUpdate},
    cursor::{Hide, Show},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use app::App;

fn supports_kitty_graphics(term: &str, term_program: &str, kitty_window: bool) -> bool {
    kitty_window
        || term.contains("kitty")
        || term == "xterm-ghostty"
        || matches!(term_program, "WezTerm" | "ghostty")
}


fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Ojo con esto: si truena el programa, hay que dejar el terminal como estaba
    // o se queda cagado sin echo ni nada
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = execute!(io::stdout(), EndSynchronizedUpdate, crossterm::event::DisableBracketedPaste);
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, Show);
        original_hook(panic_info);
    }));

    updater::remove_previous_executable();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide, crossterm::event::EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Query terminal cell size for image sizing. from_termios() is Unix-only (uses termios).
    // On Windows we fall back to Picker::new with a safe default cell size.
    #[cfg(unix)]
    let picker_res = ratatui_image::picker::Picker::from_termios();
    #[cfg(not(unix))]
    let picker_res: Result<ratatui_image::picker::Picker, &str> = Err("termios unavailable on this platform");

    // Detect terminal capabilities from environment variables.
    // IMPORTANT: do NOT match plain "xterm" — xterm-256color is reported by almost every
    // terminal (macOS Terminal.app, GNOME Terminal, etc.) regardless of Sixel support.
    let term = std::env::var("TERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();

    // Kitty, WezTerm and Ghostty support the Kitty graphics protocol.
    let is_kitty = supports_kitty_graphics(
        &term,
        &term_program,
        std::env::var("KITTY_WINDOW_ID").is_ok(),
    );

    // Sixel: only terminals known to support it. foot, mlterm, yaft, Windows Terminal
    // (WT_SESSION), and iTerm2 (TERM_PROGRAM or ITERM_SESSION_ID).
    let is_sixel = term.contains("foot")
        || term.contains("sixel")
        || term.contains("mlterm")
        || term.contains("yaft")
        || term_program == "iTerm.app"
        || std::env::var("ITERM_SESSION_ID").is_ok()
        || std::env::var("WT_SESSION").is_ok();

    let picker = {
        let mut p = match picker_res {
            Ok(p) => p,
            // Windows / unknown: use a safe default cell size (8×16 pixels).
            Err(_) => ratatui_image::picker::Picker::new((8, 16)),
        };
        if is_kitty {
            p.protocol_type = ratatui_image::picker::ProtocolType::Kitty;
        } else if is_sixel {
            p.protocol_type = ratatui_image::picker::ProtocolType::Sixel;
        }
        // Else: keep Halfblocks — renders everywhere with no terminal-specific support.
        Some(p)
    };

    let args: Vec<String> = std::env::args().collect();
    let initial_path = args.get(1).map(PathBuf::from);

    let (tx, rx) = channel();

    events::spawn_event_handler(tx.clone());

    let mut app = App::new(tx, initial_path, picker);

    // Present one complete animation frame per 60 Hz deadline. The worker
    // prepares the following frame in parallel; input never changes the cadence.
    let mut next_player_frame = Instant::now();
    while !app.should_quit {
        // souvlaki's macOS backend (MPRemoteCommandCenter/MPNowPlayingInfoCenter) talks to
        // mediaremoted/nowplayingd over an XPC connection scheduled on this thread's run
        // loop. Since we never run a Cocoa/CFRunLoop (no NSApplication, no winit), incoming
        // media-key commands and Now Playing registration silently never get delivered.
        // Pumping it briefly each tick is enough — no window required.
        #[cfg(target_os = "macos")]
        core_foundation::runloop::CFRunLoop::run_in_mode(
            unsafe { core_foundation::runloop::kCFRunLoopDefaultMode },
            Duration::from_millis(10),
            true,
        );

        let animated_player = app.screen == app::AppScreen::Queue
            && app.audio.shared_state.lock().unwrap().status == models::PlaybackStatus::Playing;
        let frame_interval = crate::preview::VINYL_FRAME_INTERVAL;
        let wait = if animated_player {
            next_player_frame.saturating_duration_since(Instant::now())
        } else {
            Duration::from_millis(33)
        };
        if let Ok(event) = rx.recv_timeout(wait) {
            app.handle_event(event);
            for e in rx.try_iter().take(255) {
                app.handle_event(e);
            }
        } else if app.audio.shared_state.lock().unwrap().status != models::PlaybackStatus::Playing {
            continue;
        }
        if animated_player {
            let now = Instant::now();
            if now < next_player_frame { continue; }
            next_player_frame += frame_interval;
            if next_player_frame <= now {
                next_player_frame = now + frame_interval;
            }
        } else {
            next_player_frame = Instant::now();
        }
        // Commit image uploads and text changes together. Otherwise a terminal
        // can display a partly replaced image while processing a large frame.
        execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
        let draw_result = terminal.draw(|f| ui::render(f, &mut app)).map(|_| ());
        let end_result = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
        draw_result?;
        end_result?;
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::event::DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    )?;

    if let Some(release) = app.pending_update.take() {
        app.audio.stop();
        drop(app);
        updater::install_and_restart(&release)?;
    }

    Ok(())
}

#[cfg(test)]
mod terminal_tests {
    use super::supports_kitty_graphics;

    #[test]
    fn detects_graphics_terminals_without_enabling_generic_xterm() {
        for (term, program, window) in [
            ("xterm-ghostty", "", false),
            ("xterm-256color", "ghostty", false),
            ("xterm-kitty", "", false),
            ("xterm-256color", "WezTerm", false),
            ("xterm-256color", "", true),
        ] {
            assert!(supports_kitty_graphics(term, program, window));
        }
        assert!(!supports_kitty_graphics("xterm-256color", "", false));
        assert!(!supports_kitty_graphics("xterm-256color", "Apple_Terminal", false));
    }
}
