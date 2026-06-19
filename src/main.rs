mod app;
mod audio;
mod browser;
mod collections;
mod config;
mod events;
mod models;
mod queue;
mod search;
mod ui;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    cursor::{Hide, Show},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use app::App;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup panic hook to restore terminal on crash
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = execute!(io::stdout(), crossterm::event::DisableBracketedPaste);
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, Show);
        original_hook(panic_info);
    }));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide, crossterm::event::EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Query terminal capabilities after enabling raw mode but before spawning events thread
    let picker_res = ratatui_image::picker::Picker::from_termios();
    if let Ok(ref p) = picker_res {
        let _ = std::fs::write("debug_picker.log", format!("Picker successfully initialized. Protocol: {:?}", p.protocol_type));
    } else if let Err(ref e) = picker_res {
        let _ = std::fs::write("debug_picker.log", format!("Picker failed: {:?}", e));
    }

    let is_kitty = std::env::var("KITTY_WINDOW_ID").is_ok() || std::env::var("TERM").map(|t| t.contains("kitty")).unwrap_or(false);
    let is_sixel = std::env::var("TERM").map(|t| t.contains("foot") || t.contains("xterm") || t.contains("sixel")).unwrap_or(false);

    let picker = match picker_res {
        Ok(mut p) => {
            if is_kitty {
                p.protocol_type = ratatui_image::picker::ProtocolType::Kitty;
            } else if is_sixel {
                p.protocol_type = ratatui_image::picker::ProtocolType::Sixel;
            }
            Some(p)
        }
        Err(_) => {
            let mut p = ratatui_image::picker::Picker::new((8, 16));
            if is_kitty {
                p.protocol_type = ratatui_image::picker::ProtocolType::Kitty;
            } else if is_sixel {
                p.protocol_type = ratatui_image::picker::ProtocolType::Sixel;
            }
            Some(p)
        }
    };

    // Parse command line arguments for starting directory
    let args: Vec<String> = std::env::args().collect();
    let initial_path = args.get(1).map(PathBuf::from);

    // Create channel
    let (tx, rx) = channel();

    // Spawn event handler
    events::spawn_event_handler(tx.clone());

    // Create app state
    let mut app = App::new(tx, initial_path, picker);

    // Main loop
    while !app.should_quit {
        terminal.draw(|f| ui::render(f, &mut app))?;

        if let Ok(event) = rx.recv() {
            app.handle_event(event);
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::event::DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    )?;

    Ok(())
}
