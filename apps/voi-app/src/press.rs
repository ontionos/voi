//! What a press of the dictation key means: start, stop, or nothing yet.
//!
//! Shared by both front ends. The Mac reads whether its key is down from the
//! modifier flags sixty times a second; Windows hears it go down and up from
//! its keyboard hook. Either way the same rule decides what that means, so a
//! hold, a brush and a click behave the same on both.

use std::time::{Duration, Instant};

use voi_core::prefs::DictationMode;

/// And how short a press has to be to count as a click, in toggle mode.
const CLICK: Duration = Duration::from_millis(450);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Start,
    Stop,
}

#[derive(Default)]
pub struct Hotkey {
    /// When the key went down, if it is down.
    since: Option<Instant>,
    /// Whether this hold has already started something, so it starts once.
    armed: bool,
}

impl Hotkey {
    /// Work out what the key means, given how long it has been in its current
    /// state and what is already running. Called once a frame.
    pub fn poll(
        &mut self,
        down: bool,
        now: Instant,
        mode: DictationMode,
        running: bool,
    ) -> Option<Trigger> {
        match (down, self.since) {
            // Pressed just now. Held to talk starts here, on the frame the key
            // went down: there used to be a fifth of a second's wait first, so
            // brushing the key did nothing, and it was the slowest part of the
            // window coming up. The key is kept for this alone — pressed with
            // another, it is not ours — so a brush costs a flash and nothing
            // typed, and every real press is answered at once.
            (true, None) => {
                self.since = Some(now);
                self.armed = matches!(mode, DictationMode::PushToTalk) && !running;
                self.armed.then_some(Trigger::Start)
            }
            // Still down.
            (true, Some(_)) => None,
            // Let go.
            (false, Some(since)) => {
                let held = now.saturating_duration_since(since);
                self.since = None;
                let started = self.armed;
                self.armed = false;
                match mode {
                    // Held to talk, released to stop. Only if this hold is the
                    // one that started it — otherwise a stray press while
                    // toggled on would end a dictation it never began.
                    DictationMode::PushToTalk if running && started => Some(Trigger::Stop),
                    // A click, either way round. A long hold in toggle mode is
                    // somebody using the key for something else.
                    DictationMode::Toggle if held < CLICK => {
                        Some(if running { Trigger::Stop } else { Trigger::Start })
                    }
                    _ => None,
                }
            }
            (false, None) => None,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    /// Held: it starts the moment the key goes down, and stops on release.
    #[test]
    fn push_to_talk_runs_for_as_long_as_the_key_is_held() {
        let start = Instant::now();
        let mut key = Hotkey::default();
        let mode = DictationMode::PushToTalk;

        assert_eq!(key.poll(true, start, mode, false), Some(Trigger::Start), "on the first frame");
        // And only once, however many frames the hold lasts.
        assert_eq!(key.poll(true, at(start, 300), mode, true), None);
        assert_eq!(key.poll(true, at(start, 4_000), mode, true), None);
        assert_eq!(key.poll(false, at(start, 4_100), mode, true), Some(Trigger::Stop));
    }

    /// A tap is not a dictation.
    /// A brush is a press like any other: it starts, and its release stops it.
    #[test]
    fn a_brush_of_the_key_starts_and_stops() {
        let start = Instant::now();
        let mut key = Hotkey::default();
        let mode = DictationMode::PushToTalk;

        assert_eq!(key.poll(true, start, mode, false), Some(Trigger::Start));
        assert_eq!(key.poll(false, at(start, 30), mode, true), Some(Trigger::Stop));
    }

    /// A press while a dictation is already going — one toggled on, say —
    /// neither starts a second nor stops the first.
    #[test]
    fn a_press_while_running_is_not_a_start() {
        let start = Instant::now();
        let mut key = Hotkey::default();
        let mode = DictationMode::PushToTalk;

        assert_eq!(key.poll(true, start, mode, true), None);
        assert_eq!(key.poll(false, at(start, 500), mode, true), None);
    }

    /// Toggled: click on, click off, and a long hold is somebody else's
    /// shortcut.
    #[test]
    fn toggle_starts_and_stops_on_a_click() {
        let start = Instant::now();
        let mut key = Hotkey::default();
        let mode = DictationMode::Toggle;

        key.poll(true, start, mode, false);
        assert_eq!(key.poll(false, at(start, 120), mode, false), Some(Trigger::Start));

        // Held down for a while: not a click, so nothing happens on release.
        key.poll(true, at(start, 1_000), mode, true);
        assert_eq!(key.poll(false, at(start, 2_000), mode, true), None);

        // A second click ends it.
        key.poll(true, at(start, 3_000), mode, true);
        assert_eq!(key.poll(false, at(start, 3_100), mode, true), Some(Trigger::Stop));
    }

    /// Holding the key in toggle mode must not start anything by itself.
    #[test]
    fn holding_does_nothing_when_toggling() {
        let start = Instant::now();
        let mut key = Hotkey::default();
        let mode = DictationMode::Toggle;

        key.poll(true, start, mode, false);
        assert_eq!(key.poll(true, at(start, 3_000), mode, false), None);
    }
}
