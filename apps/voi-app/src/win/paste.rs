//! Putting the dictated text where the cursor is: `mac/paste.rs` on Windows.
//!
//! The same two steps: the text goes on the clipboard, then Ctrl+V is pressed
//! for whatever window has the keyboard. Windows asks no permission for the
//! second step, so this pastes every time the clipboard takes the text; the
//! Mac's "Copied — press ⌘V" case remains for a clipboard that refuses it.
//! (A window running as administrator silently ignores keys pressed by an
//! ordinary program, and Windows does not say so; the text is still on the
//! clipboard there.)

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
};

/// `CF_UNICODETEXT`.
const UNICODE_TEXT: u32 = 13;

/// The window the clipboard is opened for. Opened for no window, Windows
/// empties it to no owner and then refuses to put anything on it.
static OWNER: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn set_owner(hwnd: isize) {
    OWNER.store(hwnd, std::sync::atomic::Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivered {
    /// On the clipboard and pasted where the cursor was.
    Pasted,
    /// The clipboard would not take it; nothing was pressed.
    Copied,
}

pub fn deliver(text: &str) -> Delivered {
    if !copy(text) {
        tracing::warn!("the clipboard would not take the dictation");
        return Delivered::Copied;
    }
    press_paste();
    Delivered::Pasted
}

/// The text on the clipboard, as Unicode. Retried briefly: another program
/// holding the clipboard open is the usual reason it fails, and it is short.
pub fn copy(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    for _ in 0..10 {
        unsafe {
            let owner = OWNER.load(std::sync::atomic::Ordering::SeqCst) as windows_sys::Win32::Foundation::HWND;
            if OpenClipboard(owner) == 0 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            EmptyClipboard();
            let bytes = wide.len() * 2;
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes);
            let ok = if memory.is_null() {
                false
            } else {
                let target = GlobalLock(memory) as *mut u16;
                std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
                GlobalUnlock(memory);
                // The clipboard owns the memory once it takes it.
                if SetClipboardData(UNICODE_TEXT, memory as HANDLE).is_null() {
                    GlobalFree(memory);
                    false
                } else {
                    true
                }
            };
            CloseClipboard();
            return ok;
        }
    }
    false
}

/// Ctrl+V, as though somebody had pressed it. Marked as Voi's own, so the
/// keyboard hook does not take it for the dictation key or the hotkey.
fn press_paste() {
    let key = |vk: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: super::hook::OURS,
            },
        },
    };
    let v = b'V' as u16;
    let inputs = [key(VK_CONTROL, false), key(v, false), key(v, true), key(VK_CONTROL, true)];
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}
