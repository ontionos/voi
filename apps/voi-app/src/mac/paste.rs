//! Putting the dictated text where the cursor is.
//!
//! Two steps, and the first one always happens: the text goes on the clipboard,
//! then a ⌘V is synthesised for whatever is frontmost. Typing the characters
//! one by one would be slower, would fight autocorrect and input methods, and
//! would arrive in the wrong order in a slow text field; the clipboard is what
//! every other dictation tool uses for the same reasons.
//!
//! Synthesising a keystroke is the one thing here that needs Accessibility
//! permission — macOS will not let an app press keys for you otherwise. Without
//! it the text is still on the clipboard, and the panel says so: a ⌘V away
//! rather than nothing at all.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_core_graphics::{
    CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

/// Virtual keycode for V, on any layout: these are physical keys, not letters.
const KEY_V: u16 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivered {
    /// On the clipboard and pasted where the cursor was.
    Pasted,
    /// On the clipboard only — pressing keys needs Accessibility.
    Copied,
}

// Declared here rather than pulled in as a crate: two functions and a constant,
// from a framework AppKit already links.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const AnyObject) -> bool;
    /// The one option: put the request on screen rather than answering
    /// silently. A Core Foundation string, which is an `NSString` — the two
    /// are the same object seen from either language.
    static kAXTrustedCheckOptionPrompt: *const NSString;
}

/// Whether macOS will let this app press keys.
pub fn may_press_keys() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Ask for it, with the system's own dialog.
///
/// Accessibility cannot be granted by an app, only by a person, and macOS will
/// not show the request unless it is asked for — so an app that merely *checks*
/// leaves you to find the right pane of System Settings by yourself, which is
/// no way to be told that a feature needs something. This puts the standard
/// alert up, with the button that opens the right pane already in it.
///
/// Asking when it is already granted shows nothing, so this is safe to call at
/// every launch. macOS only shows the dialog once per app anyway; after that
/// the answer is remembered and this is a silent check.
pub fn ask_to_press_keys() -> bool {
    unsafe {
        let key: &NSString = &*kAXTrustedCheckOptionPrompt;
        let yes = NSNumber::numberWithBool(true);
        let options = NSDictionary::from_slices(&[key], &[yes.as_ref() as &AnyObject]);
        AXIsProcessTrustedWithOptions(Retained::as_ptr(&options).cast())
    }
}

/// Copy, then paste if allowed. Returns which of the two happened.
///
/// The permission is asked for here — at the first moment it is actually
/// needed, with the words already on the clipboard — rather than at launch.
/// Asked at launch it is a dialog about a feature nobody has used yet, and it
/// comes back on every launch until it is granted; asked here it arrives with
/// an obvious reason, once, and only if pressing ⌘V was the thing that failed.
pub fn deliver(text: &str) -> Delivered {
    let board = NSPasteboard::generalPasteboard();
    unsafe {
        board.clearContents();
        board.setString_forType(&NSString::from_str(text), NSPasteboardTypeString);
    }

    if !may_press_keys() {
        // Puts the system's own dialog up, with the button that opens the
        // right pane of System Settings. Silent when it has been asked before
        // in this run.
        ask_to_press_keys();
        return Delivered::Copied;
    }
    press_paste();
    Delivered::Pasted
}

/// ⌘V, as though somebody had pressed it.
fn press_paste() {
    // A source in "private" state so the synthesised keys do not get mixed
    // into the real keyboard's state — the Command we are pretending to hold
    // must not confuse anything watching the actual one, this app included.
    let source = CGEventSource::new(CGEventSourceStateID::Private);
    let source = source.as_deref();

    for down in [true, false] {
        let Some(event) = CGEvent::new_keyboard_event(source, KEY_V, down) else {
            continue;
        };
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }
}
