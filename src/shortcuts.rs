//! Shared playback bindings for Player, Radio and Podcasts.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackAction {
    Toggle,
    Stop,
    Next,
    Previous,
    Volume(i32),
    Seek(i64),
    Repeat,
    Shuffle,
    Visualizer,
    Decay(f32),
}

pub fn playback_action(key: KeyEvent) -> Option<PlaybackAction> {
    use PlaybackAction::*;
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    Some(match key.code {
        KeyCode::Char(' ' | 'p' | 'P') => Toggle,
        KeyCode::Char('s') => Stop,
        KeyCode::Char('n') => Next,
        KeyCode::Char('b') => Previous,
        KeyCode::Char('+' | '=') => Volume(5),
        KeyCode::Char('-') => Volume(-5),
        KeyCode::Char('H') => Seek(-5),
        KeyCode::Char('L') => Seek(5),
        KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => Seek(-5),
        KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => Seek(5),
        KeyCode::Char('r') => Repeat,
        KeyCode::Char('z') => Shuffle,
        KeyCode::Char('v') => Visualizer,
        KeyCode::Char('[' | '<' | ',') => Decay(-0.02),
        KeyCode::Char(']' | '>' | '.') => Decay(0.02),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playback_bindings_do_not_steal_navigation_or_screen_actions() {
        for (key, action) in [
            ('s', PlaybackAction::Stop),
            ('p', PlaybackAction::Toggle),
            ('n', PlaybackAction::Next),
            ('b', PlaybackAction::Previous),
            ('r', PlaybackAction::Repeat),
            ('[', PlaybackAction::Decay(-0.02)),
            (']', PlaybackAction::Decay(0.02)),
        ] {
            assert_eq!(
                playback_action(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                Some(action)
            );
        }
        for key in [
            KeyCode::Char('f'),
            KeyCode::Char('R'),
            KeyCode::Char('o'),
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Tab,
        ] {
            assert_eq!(
                playback_action(KeyEvent::new(key, KeyModifiers::NONE)),
                None
            );
        }
        assert_eq!(
            playback_action(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
            Some(PlaybackAction::Seek(5))
        );
        assert_eq!(
            playback_action(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            None
        );
    }
}
