mod app;
mod audio;
mod browser;
mod collections;
mod config;
mod discord;
mod events;
mod healer;
mod library;
mod models;
mod queue;
mod search;
mod stats;
mod ui;
mod updater;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::Duration;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    cursor::{Hide, Show},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use app::App;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Ojo con esto: si truena el programa, hay que dejar el terminal como estaba
    // o se queda cagado sin echo ni nada
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = execute!(io::stdout(), crossterm::event::DisableBracketedPaste);
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, Show);
        original_hook(panic_info);
    }));

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

    // Kitty graphics protocol: Kitty terminal and WezTerm both support it.
    let is_kitty = std::env::var("KITTY_WINDOW_ID").is_ok()
        || term.contains("kitty")
        || term_program == "WezTerm";

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

    // Loop principal: esperamos maximo 50ms por un evento, procesamos todos los que haya
    // acumulados, y luego pintamos. Así no se traba con rafagas de teclas
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

        match rx.recv_timeout(Duration::from_millis(16)) {
            Ok(event) => {
                app.handle_event(event);
                while let Ok(e) = rx.try_recv() {
                    app.handle_event(e);
                }
            }
            Err(_) => {}
        }
        terminal.draw(|f| ui::render(f, &mut app))?;
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::event::DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    )?;

    Ok(())
}
