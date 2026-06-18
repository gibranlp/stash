use std::thread;
use std::time::Duration;
use crossterm::event::{self, Event as CrossEvent, KeyEvent};

#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Tick,
    AudioFinished,
    MediaPlayPause,
    MediaNext,
    MediaPrev,
}

pub fn spawn_event_handler(tx: std::sync::mpsc::Sender<Event>) {
    // Input thread
    let input_tx = tx.clone();
    thread::spawn(move || {
        loop {
            if let Ok(has_event) = event::poll(Duration::from_millis(50)) {
                if has_event {
                    if let Ok(CrossEvent::Key(key_event)) = event::read() {
                        let _ = input_tx.send(Event::Key(key_event));
                    }
                }
            }
        }
    });

    // Tick thread
    let tick_tx = tx;
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(50));
            let _ = tick_tx.send(Event::Tick);
        }
    });
}
pub type EventSender = std::sync::mpsc::Sender<Event>;
