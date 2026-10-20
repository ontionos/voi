//! The two notes dictation makes: `mac/sound.rs` on Windows.
//!
//! The same recordings (`assets/sounds`), as WAV because that is what the
//! system's own player takes, built into the program so there is no file to
//! lose. Played asynchronously; a new note replaces one still playing, which
//! is what the Mac's stop-then-play does.

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

#[derive(Clone, Copy)]
pub enum Note {
    Start,
    End,
}

static START: &[u8] = include_bytes!("../../../../assets/sounds/start.wav");
static END: &[u8] = include_bytes!("../../../../assets/sounds/end.wav");

/// Play one, if the setting allows it.
pub fn play(note: Note, allowed: bool) {
    if !allowed {
        return;
    }
    let bytes = match note {
        Note::Start => START,
        Note::End => END,
    };
    unsafe {
        PlaySoundW(bytes.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}
