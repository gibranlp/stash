use std::thread;
use std::time::Duration;
use crossterm::event::{self, Event as CrossEvent, KeyEvent};

#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Paste(String),
    Tick,
    AudioFinished,
    MediaPlayPause,
    MediaNext,
    MediaPrev,
    MediaSeek(souvlaki::SeekDirection, std::time::Duration),
    MediaSetPosition(std::time::Duration),
    LibraryChanged,
}

// Aquí arrancamos dos hilos: uno que jala eventos del teclado/paste y otro
// que manda un Tick cada 50ms pa que la UI se mantenga viva
pub fn spawn_event_handler(tx: std::sync::mpsc::Sender<Event>) {
    let input_tx = tx.clone();
    thread::spawn(move || {
        loop {
            if let Ok(has_event) = event::poll(Duration::from_millis(50))
                && has_event {
                    match event::read() {
                        Ok(CrossEvent::Key(key_event)) => {
                            let _ = input_tx.send(Event::Key(key_event));
                        }
                        Ok(CrossEvent::Paste(content)) => {
                            let _ = input_tx.send(Event::Paste(content));
                        }
                        _ => {}
                    }
                }
        }
    });

    let tick_tx = tx;
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(50));
            let _ = tick_tx.send(Event::Tick);
        }
    });
}
