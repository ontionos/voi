//! The recording hotkey on Windows: a low-level keyboard hook.
//!
//! `RegisterHotKey` was the other way, and it is the wrong one here on three
//! counts. It cannot take a key on its own without a modifier the way a hook
//! can (Right Ctrl, Pause), it loses to whichever program registered the same
//! combination first, and it says nothing about the key's release — so the
//! foreground program still receives that. A `WH_KEYBOARD_LL` hook sees every
//! key press before anything else is decided about it, hotkeys included, and
//! returning non-zero from it means no window anywhere receives that press.
//!
//! The hook runs on a thread of its own with its own message loop, because
//! Windows calls a low-level hook on the thread that installed it, and only
//! while that thread is pumping messages. It must answer within a fraction of
//! a second or Windows quietly removes it, so the callback only decides —
//! through [`voi_core::hotkey::Matcher`] — and hands anything slower to
//! [`Events`], which another thread acts on.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use voi_core::hotkey::{vk, Action, Hotkey, KeyEvent, Matcher};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    WM_APP, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// The hotkey in force, packed; zero for none. Read by the hook on every key
/// press, so it is an atomic rather than anything that can block.
static BOUND: AtomicU32 = AtomicU32::new(0);
/// Settings is waiting for the next key to be pressed.
static CAPTURE: AtomicBool = AtomicBool::new(false);
static MATCHER: Mutex<Option<Matcher>> = Mutex::new(None);
static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();
static THREAD: AtomicU32 = AtomicU32::new(0);

/// Marks input Voi injects (the mask key, the dictation's Ctrl+V), so the
/// hook does not act on its own keys.
pub const OURS: usize = 0x564F_4931; // "VOI1"
/// The dictation key's virtual-key code; zero for none. Watched, never taken:
/// on the Mac the dictation key still reaches the app in front, and so here.
static DICTATION: AtomicU32 = AtomicU32::new(0);
/// Whether it is down, so its auto-repeats are not reported as presses.
static DICTATION_DOWN: AtomicBool = AtomicBool::new(false);
/// Asks the hook thread to press the mask key, after the hook has returned.
const WM_MASK: u32 = WM_APP + 1;

/// What the hook hands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The hotkey went down: start or stop.
    Pressed,
    /// Settings asked for a key, and this one was pressed.
    Captured(Hotkey),
    /// The dictation key went down or came up, at this moment.
    Dictation { down: bool, at: std::time::Instant },
}

/// Which key dictation listens for, by the Mac's name for it.
///
/// The Mac's keys are its modifiers by side; Windows has the same keys in the
/// same places under other names — ⌘ where Ctrl is, ⌥ where Alt is (gap G1 in
/// docs/windows-parity.md). Fn never reaches Windows at all: the keyboard keeps
/// it to itself.
pub fn dictation_vk(key: voi_core::prefs::DictationKey) -> Option<u16> {
    use voi_core::prefs::DictationKey as K;
    match key {
        K::RightCommand | K::RightControl => Some(vk::RCONTROL),
        K::LeftCommand => Some(vk::LCONTROL),
        K::RightOption => Some(vk::RMENU),
        K::LeftOption => Some(vk::LMENU),
        K::Fn => None,
    }
}

/// Listen for this key for dictation, or for none.
pub fn bind_dictation(key: Option<u16>) {
    DICTATION.store(key.map_or(0, u32::from), Ordering::SeqCst);
    DICTATION_DOWN.store(false, Ordering::SeqCst);
}

/// Set the hotkey in force, or none.
pub fn bind(hotkey: Option<Hotkey>) {
    BOUND.store(hotkey.map_or(0, |h| h.packed()), Ordering::SeqCst);
    tracing::info!(hotkey = %hotkey.map_or("none".into(), |h| h.to_string()), "record hotkey bound");
}

pub fn bound() -> Option<Hotkey> {
    Hotkey::unpack(BOUND.load(Ordering::SeqCst))
}

/// Start or stop waiting for a key to be chosen in Settings. While waiting,
/// the hotkey in force rests, so pressing it again can choose it again.
pub fn capture(on: bool) {
    if CAPTURE.swap(on, Ordering::SeqCst) != on {
        tracing::info!(on, "waiting for a hotkey to be pressed");
    }
}

pub fn capturing() -> bool {
    CAPTURE.load(Ordering::SeqCst)
}

/// Install the hook on a thread of its own. Returns the events it reports.
pub fn install() -> Receiver<Event> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = EVENTS.set(tx);
    *MATCHER.lock().unwrap() = Some(Matcher::new());
    std::thread::Builder::new()
        .name("voi-keyboard-hook".into())
        .spawn(run)
        .expect("failed to start the keyboard hook thread");
    rx
}

fn run() {
    unsafe {
        THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), GetModuleHandleW(std::ptr::null()), 0);
        if hook.is_null() {
            tracing::error!("could not install the keyboard hook; the record hotkey will not work");
            return;
        }
        tracing::info!("keyboard hook installed");
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_MASK {
                press_mask();
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnhookWindowsHookEx(hook);
    }
}

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }
    let info = &*(lparam as *const KBDLLHOOKSTRUCT);
    let down = match wparam as u32 {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam),
    };
    // Our own mask key, and the Ctrl that AltGr fakes on layouts that have
    // one (it carries scan code 0x21D): neither is anything anyone pressed.
    let fake_ctrl = info.vkCode as u16 == vk::LCONTROL && info.scanCode == 0x21D;
    if info.dwExtraInfo == OURS || fake_ctrl {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }

    let dictation = DICTATION.load(Ordering::Relaxed);
    if dictation != 0 && info.vkCode == dictation && DICTATION_DOWN.swap(down, Ordering::SeqCst) != down {
        if let Some(tx) = EVENTS.get() {
            let _ = tx.send(Event::Dictation { down, at: std::time::Instant::now() });
        }
    }

    let event = KeyEvent { key: info.vkCode as u16, down };
    let action = match MATCHER.lock() {
        Ok(mut guard) => match guard.as_mut() {
            Some(m) => m.event(event, &[bound()], capturing()),
            None => Action::Pass,
        },
        Err(_) => Action::Pass,
    };

    let mask = match action {
        Action::Pass => return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam),
        Action::Swallow { mask } => mask,
        Action::Released { .. } => false,
        Action::Trigger { mask, .. } => {
            if let Some(tx) = EVENTS.get() {
                let _ = tx.send(Event::Pressed);
            }
            mask
        }
        Action::Captured(hotkey) => {
            CAPTURE.store(false, Ordering::SeqCst);
            if let Some(tx) = EVENTS.get() {
                let _ = tx.send(Event::Captured(hotkey));
            }
            false
        }
    };
    if mask {
        PostThreadMessageW(THREAD.load(Ordering::SeqCst), WM_MASK, 0, 0);
    }
    1
}

/// Tap the unassigned key, so a held Alt or Win is not let go of "alone".
fn press_mask() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk::MASK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: OURS },
        },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
}
