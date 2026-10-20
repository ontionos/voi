//! Triggering the macOS audio permission prompts up front.
//!
//! macOS raises a TCC prompt the first time a process actually touches a
//! device, not when it asks politely in advance. Waiting until the user presses
//! record means the first recording of the day is interrupted by two dialogs
//! and, if either is declined, silently captures nothing.
//!
//! So we open both sources briefly at launch and drop them again. That is what
//! surfaces the dialogs, and it costs a few hundred milliseconds once per run.
//!
//! Note what this can and cannot tell you: a denied **microphone** fails to
//! open, but a denied **system audio** tap opens successfully and then delivers
//! digital silence. Detecting that needs actual audio, which is why the
//! recorder also watches for an all-zero system stream.

use std::time::Duration;

use crate::{MicSource, SystemSource};

#[derive(Debug, Default)]
pub struct Preflight {
    pub mic: Option<String>,
    pub system: Option<String>,
}

impl Preflight {
    pub fn ok(&self) -> bool {
        self.mic.is_none() && self.system.is_none()
    }
}

/// Open and immediately release both capture sources, raising any permission
/// prompts. Blocks for a few hundred milliseconds; call it off the UI thread.
pub fn request() -> Preflight {
    let mut out = Preflight::default();

    match MicSource::start() {
        Ok(stream) => {
            // Hold it just long enough for Core Audio to have really started,
            // then let the guard drop and close the device.
            std::thread::sleep(Duration::from_millis(250));
            drop(stream);
        }
        Err(e) => out.mic = Some(format!("{e:#}")),
    }

    match SystemSource::start() {
        Ok(stream) => {
            std::thread::sleep(Duration::from_millis(250));
            drop(stream);
        }
        Err(e) => out.system = Some(format!("{e:#}")),
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_preflight_reports_ok() {
        assert!(Preflight::default().ok());
    }

    #[test]
    fn either_failure_makes_it_not_ok() {
        let mic_failed = Preflight { mic: Some("denied".into()), system: None };
        assert!(!mic_failed.ok());
        let system_failed = Preflight { mic: None, system: Some("denied".into()) };
        assert!(!system_failed.ok());
    }
}
