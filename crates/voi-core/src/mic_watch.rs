//! Deciding whether someone else has the microphone — a call starting — from
//! the list of processes taking input.
//!
//! Our own capture never counts. Neither does the system's speech service
//! (`corespeechd`, which listens for "Hey Siri") picking the built-in
//! microphone back up the moment a dictation of ours lets go of it: Core Audio
//! reports it as one more process taking input, and counted as "another app"
//! it put the "Record?" prompt up after every dictation. It is excused only
//! around a dictation — from the moment one starts until a few seconds after
//! it ends, and for as long as it then keeps listening. Any other app, at any
//! time, still counts, and so does the speech service when no dictation is
//! anywhere near: someone invoking Siri is a microphone in use like any other.
//!
//! Pure bookkeeping, no Core Audio: the platform's own watcher supplies the
//! list, so the same rule serves every platform.

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// How long after a dictation ends the speech service may still take the
/// microphone back on its account. It does so within a couple of seconds of
/// our stream closing, and the stream itself can take a second or two to
/// close after the words arrive — a Bluetooth microphone longer.
pub const GRACE: Duration = Duration::from_secs(5);

/// Bundle ids, matched as case-insensitive prefixes, of the system services
/// that react to our own use of the microphone rather than to anything the
/// user started.
const COMPANIONS: &[&str] = &[
    "com.apple.corespeech",
    "com.apple.assistantd",
    "com.apple.speechrecognitioncore",
];

fn is_companion(bundle: &str) -> bool {
    let bundle = bundle.to_ascii_lowercase();
    COMPANIONS.iter().any(|c| bundle.starts_with(c))
}

/// Whether a dictation of ours is running, when the last one ended, and
/// which companion processes are excused because of it.
#[derive(Debug, Default)]
pub struct MicWatch {
    dictating: bool,
    ended: Option<Instant>,
    /// Companions that started listening while they were excused. Kept off
    /// the count until they stop, so one that holds the microphone a little
    /// past [`GRACE`] does not bring the prompt up late.
    excused: HashSet<i32>,
}

impl MicWatch {
    /// A dictation started or ended. Repeating the current state is harmless.
    pub fn set_dictating(&mut self, on: bool, now: Instant) {
        if self.dictating && !on {
            self.ended = Some(now);
        }
        self.dictating = on;
    }

    fn excusing(&self, now: Instant) -> bool {
        self.dictating || self.ended.is_some_and(|t| now.saturating_duration_since(t) < GRACE)
    }

    /// Whether any of `listeners` — `(pid, bundle id)` of every process
    /// taking input — is someone other than us. `me` is this process.
    pub fn someone_else(&mut self, listeners: &[(i32, String)], me: i32, now: Instant) -> bool {
        let excusing = self.excusing(now);
        self.excused.retain(|pid| listeners.iter().any(|(p, _)| p == pid));
        let mut someone = false;
        for (pid, bundle) in listeners {
            if *pid == me || self.excused.contains(pid) {
                continue;
            }
            if excusing && is_companion(bundle) {
                self.excused.insert(*pid);
                continue;
            }
            someone = true;
        }
        someone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: i32 = 100;

    fn us() -> (i32, String) {
        (ME, "is.mana.voi".into())
    }
    fn siri() -> (i32, String) {
        (819, "com.apple.CoreSpeech".into())
    }
    fn zoom() -> (i32, String) {
        (4242, "us.zoom.xos".into())
    }

    /// What every dictation looked like: the system speech service grabs the
    /// microphone as ours opens and again as ours closes.
    #[test]
    fn a_dictation_of_ours_never_asks() {
        let mut w = MicWatch::default();
        let t = Instant::now();
        w.set_dictating(true, t);
        assert!(!w.someone_else(&[us()], ME, t));
        assert!(!w.someone_else(&[us(), siri()], ME, t + Duration::from_millis(500)));
        w.set_dictating(false, t + Duration::from_secs(3));
        assert!(!w.someone_else(&[siri()], ME, t + Duration::from_secs(5)));
        // Still holding it past the grace: it was excused when it started.
        assert!(!w.someone_else(&[siri()], ME, t + Duration::from_secs(10)));
        assert!(!w.someone_else(&[], ME, t + Duration::from_secs(11)));
    }

    #[test]
    fn another_app_asks() {
        let mut w = MicWatch::default();
        assert!(w.someone_else(&[zoom()], ME, Instant::now()));
    }

    #[test]
    fn another_app_during_a_dictation_asks() {
        let mut w = MicWatch::default();
        let t = Instant::now();
        w.set_dictating(true, t);
        assert!(w.someone_else(&[us(), zoom()], ME, t + Duration::from_secs(1)));
    }

    /// The grace is for the speech service only: a call that starts the
    /// moment a dictation ends is still a call.
    #[test]
    fn another_app_right_after_a_dictation_asks() {
        let mut w = MicWatch::default();
        let t = Instant::now();
        w.set_dictating(true, t);
        w.set_dictating(false, t + Duration::from_secs(2));
        assert!(!w.someone_else(&[siri()], ME, t + Duration::from_secs(3)));
        assert!(w.someone_else(&[siri(), zoom()], ME, t + Duration::from_secs(4)));
        assert!(w.someone_else(&[zoom()], ME, t + Duration::from_secs(5)));
    }

    /// Away from any dictation, the speech service listening is someone
    /// invoking Siri, and counts like any other app — as it always has.
    #[test]
    fn the_speech_service_on_its_own_still_counts() {
        let mut w = MicWatch::default();
        let t = Instant::now();
        assert!(w.someone_else(&[siri()], ME, t));

        w.set_dictating(true, t);
        w.set_dictating(false, t + Duration::from_secs(1));
        assert!(!w.someone_else(&[], ME, t + Duration::from_secs(2)));
        assert!(w.someone_else(&[siri()], ME, t + Duration::from_secs(30)));
    }
}
