//! The dictation key: right Command, watched rather than intercepted.
//!
//! Asked for sixty times a second rather than hooked. Intercepting keystrokes
//! means an event tap, an event tap means Accessibility before the feature
//! works at all, and a tap can swallow a keystroke by mistake. Asking can do
//! neither.
//!
//! What is asked matters, though. Reading the *keyboard* — which key is down —
//! is gated behind Input Monitoring, and an app that has not been granted it
//! is simply told "no key is down", for ever, with no error to notice. Reading
//! the *modifier flags* is not the same question: those are the state the
//! window server already publishes, which every app in the world reads to
//! check for an option-click, and they carry a bit that says which of the two
//! Command keys it was. See [`Hotkey::pressed`].
//!
//! Right Command by default, and any of a handful by choice. Bare Command is
//! held for ⌘C, ⌘V, ⌘Tab, ⌘Space — the window would be up and down all day.
//! The right-hand one is pressed by itself or not at all, because every
//! shortcut worth the name is a left-hand job.

use objc2_app_kit::NSEvent;
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID};
use voi_core::prefs::DictationKey;

pub use crate::press::{Hotkey, Trigger};

impl Hotkey {
    /// Is the chosen key down at this instant?
    ///
    /// Asked of the modifier flags, twice, because they are the one place this
    /// can be read without permission. `CGEventSourceKeyState` — which asks
    /// which *key* is down — is gated behind Input Monitoring, and an app that
    /// has not been granted it is told "no key is down" for ever, with no
    /// error to notice.
    ///
    /// Modifier flags are not the keyboard. They are the state of the
    /// shift-and-command keys as the window server already publishes it, which
    /// any app may read — that is how every app in the world checks for an
    /// option-click. Underneath the bits everyone uses is an older set saying
    /// which side the key was on, and that is what [`DictationKey::mask`]
    /// picks out.
    pub fn pressed(key: DictationKey) -> bool {
        let mask = key.mask();
        if NSEvent::modifierFlags_class().0 & mask != 0 {
            return true;
        }
        CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState).0 as usize & mask
            != 0
    }
}
