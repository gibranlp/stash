mod app;
mod audio;
mod browser;
mod collections;
mod config;
mod discord;
mod events;
mod models;
mod queue;
mod search;
mod ui;

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

    // Jalamos el picker para imágenes — tiene que ser antes de spawnear el hilo de eventos
    // porque from_termios() necsita interactuar directo con la terminal
    #[cfg(unix)]
    let picker_res = ratatui_image::picker::Picker::from_termios();
    #[cfg(not(unix))]
    let picker_res: Result<ratatui_image::picker::Picker, &str> = Err("Querying terminal is not supported on this platform");
    if let Ok(ref p) = picker_res {
        let _ = std::fs::write("debug_picker.log", format!("Picker successfully initialized. Protocol: {:?}", p.protocol_type));
    } else if let Err(ref e) = picker_res {
        let _ = std::fs::write("debug_picker.log", format!("Picker failed: {:?}", e));
    }

    // Forzamos el protocolo correcto según las variables de entorno,
    // porque el autodetect a veces se equivoca con kitty o sixel
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
            // Si falla el autodetect, armamos uno a fuerza con tamaño de celda default
            let mut p = ratatui_image::picker::Picker::new((8, 16));
            if is_kitty {
                p.protocol_type = ratatui_image::picker::ProtocolType::Kitty;
            } else if is_sixel {
                p.protocol_type = ratatui_image::picker::ProtocolType::Sixel;
            }
            Some(p)
        }
    };

    let args: Vec<String> = std::env::args().collect();
    let initial_path = args.get(1).map(PathBuf::from);

    let (tx, rx) = channel();

    events::spawn_event_handler(tx.clone());

    let mut app = App::new(tx, initial_path, picker);

    // Loop principal: esperamos maximo 50ms por un evento, procesamos todos los que haya
    // acumulados, y luego pintamos. Así no se traba con rafagas de teclas
    while !app.should_quit {
        match rx.recv_timeout(Duration::from_millis(50)) {
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
