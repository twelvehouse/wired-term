//! The bell's system sound and, while the window is in the background, the taskbar flash.

use std::sync::{Mutex, PoisonError};

use super::*;

/// App-wide, like Tabby's single audio element that does not restart while it plays.
const SOUND_GAP: Duration = Duration::from_millis(300);

static LAST_SOUND: Mutex<Option<Instant>> = Mutex::new(None);

fn is_sound_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= SOUND_GAP)
}

impl Workspace {
    pub(super) fn ring_bell(&self, window: &Window, cx: &App) {
        if Settings::get(cx).config().terminal.bell_sound {
            let now = Instant::now();
            let mut last = LAST_SOUND.lock().unwrap_or_else(PoisonError::into_inner);
            if is_sound_due(*last, now) {
                *last = Some(now);
                wired_platform::window::beep();
            }
        }
        if !window.is_window_active()
            && let Some(id) = self.window_id
        {
            wired_platform::window::flash_taskbar(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_waits_for_the_previous_one() {
        let start = Instant::now();
        assert!(is_sound_due(None, start));
        assert!(!is_sound_due(Some(start), start + SOUND_GAP / 2));
        assert!(is_sound_due(Some(start), start + SOUND_GAP));
    }
}
