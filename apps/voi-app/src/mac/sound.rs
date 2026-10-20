//! The two notes dictation makes: one when it starts listening, one when it
//! stops.
//!
//! Worth the file it takes. While dictating you are looking at whatever you are
//! writing into — a chat box, a terminal, somebody else's window — and not at
//! the little pane in the corner. A sound is the only feedback that reaches you
//! where you are actually looking, and it is how you know the key registered
//! without breaking off to check.
//!
//! Both are loaded once and kept. `NSSound` reads and decodes the file when it
//! is created, so building one per press would put a file read in the path of
//! the thing whose whole job is to feel instant.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_app_kit::NSSound;
use objc2_foundation::{NSBundle, NSString};

#[derive(Clone, Copy)]
pub enum Note {
    Start,
    End,
}

impl Note {
    fn file(self) -> &'static str {
        match self {
            Note::Start => "start",
            Note::End => "end",
        }
    }
}

thread_local! {
    /// Main-thread only, which is where every caller already is. AppKit objects
    /// are not `Send`, so this cannot be a `static` — and it does not need to
    /// be: the frame loop that plays these runs on one thread for ever.
    static LOADED: RefCell<Option<[Option<Retained<NSSound>>; 2]>> = const { RefCell::new(None) };
}

/// Play one, if the setting allows it and the file is there.
///
/// Silently does nothing when it cannot: a missing sound is not worth
/// interrupting a dictation over, and the reason is in the log once.
pub fn play(note: Note, allowed: bool) {
    if !allowed {
        return;
    }
    LOADED.with(|cell| {
        let mut slot = cell.borrow_mut();
        let sounds = slot.get_or_insert_with(|| [load(Note::Start), load(Note::End)]);
        let index = match note {
            Note::Start => 0,
            Note::End => 1,
        };
        if let Some(sound) = sounds[index].as_ref() {
            // Stopped first: pressing the key twice in quick succession should
            // restart the note rather than be ignored, which is what `play` on
            // an already-playing sound does.
            sound.stop();
            sound.play();
        }
    });
}

fn load(note: Note) -> Option<Retained<NSSound>> {
    let bundle = NSBundle::mainBundle();
    let path = bundle.pathForResource_ofType(
        Some(&NSString::from_str(note.file())),
        Some(&NSString::from_str("mp3")),
    );
    let Some(path) = path else {
        // Running the binary directly rather than from Voi.app: there is no
        // Resources folder to find these in, and that is fine.
        tracing::info!(sound = note.file(), "no sound in the bundle");
        return None;
    };
    let sound = NSSound::initWithContentsOfFile_byReference(NSSound::alloc(), &path, true);
    if sound.is_none() {
        tracing::warn!(sound = note.file(), "sound would not load");
    }
    sound
}
