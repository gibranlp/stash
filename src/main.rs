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
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, Show);
        original_hook(panic_info);
    }));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Parse command line arguments for starting directory
    let args: Vec<String> = std::env::args().collect();
    let initial_path = args.get(1).map(PathBuf::from);

    // Create channel
    let (tx, rx) = channel();

    // Spawn event handler
    events::spawn_event_handler(tx.clone());

    // Create app state
    let mut app = App::new(tx, initial_path);

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
        LeaveAlternateScreen,
        Show
    )?;

    Ok(())
}
