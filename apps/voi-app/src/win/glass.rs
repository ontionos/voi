//! Making a window glass: the desktop blurred behind it, Windows' own rounded
//! corners, no frame — what `NSVisualEffectView` and a rounded mask do on the
//! Mac.
//!
//! Applied to the window that draws the glass, so blur and contents move and
//! resize as one: no second window to lag a frame behind, and the corners are
//! the ones the compositor rounds, anti-aliased. The blur is DWM's plain blur
//! behind the window, which keeps up while the window moves; the dark wash
//! over it is drawn by Voi, as the Mac's material is dark.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

/// The radius Windows 11 rounds a window's corners to, in points.
pub const CORNER: f32 = 8.0;

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attribute: u32,
    data: *mut core::ffi::c_void,
    size: usize,
}

const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
const WCA_ACCENT_POLICY: u32 = 19;
const DWMWA_TRANSITIONS_FORCEDISABLED: u32 = 3;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWCP_ROUND: u32 = 2;
const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `SetWindowCompositionAttribute`: undocumented but long-standing — it is
/// what the shell itself uses — and so not in the import library.
unsafe fn set_composition(hwnd: HWND, data: *mut CompositionData) {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    type Set = unsafe extern "system" fn(HWND, *mut CompositionData) -> i32;
    let user32 = LoadLibraryW(wide("user32.dll").as_ptr());
    if user32.is_null() {
        return;
    }
    if let Some(f) = GetProcAddress(user32, c"SetWindowCompositionAttribute".as_ptr().cast()) {
        let set: Set = std::mem::transmute(f);
        set(hwnd, data);
    }
}

fn attribute(hwnd: HWND, which: u32, value: u32) {
    unsafe { DwmSetWindowAttribute(hwnd, which, (&value as *const u32).cast(), std::mem::size_of::<u32>() as u32) };
}

/// Blur behind `hwnd`, rounded corners, no border.
pub fn apply(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let hwnd = hwnd as HWND;
    let mut accent = AccentPolicy { state: ACCENT_ENABLE_BLURBEHIND, flags: 0, gradient: 0, animation: 0 };
    let mut data = CompositionData {
        attribute: WCA_ACCENT_POLICY,
        data: (&mut accent as *mut AccentPolicy).cast(),
        size: std::mem::size_of::<AccentPolicy>(),
    };
    unsafe { set_composition(hwnd, &mut data) };
    attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
    attribute(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
    no_transitions(hwnd as isize);
}

/// No Windows open or close animation: it slid the glass in from its right
/// over Voi's own coming out of the pointer. It has to be set before the
/// window is first shown, so every Voi window is made hidden, given this,
/// and shown a frame later.
pub fn no_transitions(hwnd: isize) {
    if hwnd != 0 {
        attribute(hwnd as HWND, DWMWA_TRANSITIONS_FORCEDISABLED, 1);
    }
}

/// No system menu, so Windows draws no caption buttons. winit gives every
/// window, borderless or not, the system-menu style and only hides the frame,
/// and Windows 11 then drew its own small close cross on the glass's
/// top-right corner. The caption style itself stays: winit sizes the window
/// by it, and without it Windows framed the glass with a light strip.
///
/// winit sets its styles again whenever its own state changes, so the window
/// is also subclassed to drop the system menu from every style change before
/// Windows applies it — mending it a frame later let the cross flash.
/// Cheap to call every frame. Returns whether it had to change anything.
pub fn popup(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER,
    };
    if hwnd == 0 {
        return false;
    }
    let hwnd = hwnd as HWND;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        if style & BUTTONS == 0 {
            return false;
        }
        SetWindowSubclass(hwnd, Some(no_buttons), SUBCLASS_ID, 0);
        SetWindowLongPtrW(hwnd, GWL_STYLE, (style & !BUTTONS) as isize);
        let keep = SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, keep);
    }
    true
}

/// The styles Windows draws caption buttons for.
const BUTTONS: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WS_SYSMENU
    | windows_sys::Win32::UI::WindowsAndMessaging::WS_MINIMIZEBOX
    | windows_sys::Win32::UI::WindowsAndMessaging::WS_MAXIMIZEBOX;
const SUBCLASS_ID: usize = 0x766F69; // "voi"

unsafe extern "system" fn no_buttons(
    hwnd: HWND,
    msg: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_STYLE, STYLESTRUCT, WM_STYLECHANGING};
    if msg == WM_STYLECHANGING && wparam as i32 == GWL_STYLE && lparam != 0 {
        let change = &mut *(lparam as *mut STYLESTRUCT);
        change.styleNew &= !BUTTONS;
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// The whole window at `alpha` (0–1): the Mac fades its panels in and out,
/// blur included, and so does this.
pub fn fade(hwnd: isize, alpha: f32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA, WS_EX_LAYERED,
    };
    if hwnd == 0 {
        return;
    }
    let hwnd = hwnd as HWND;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if style & WS_EX_LAYERED == 0 {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style | WS_EX_LAYERED) as isize);
        }
        SetLayeredWindowAttributes(hwnd, 0, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8, LWA_ALPHA);
    }
}

/// Never take focus, never sit in the taskbar or Alt+Tab: for the HUD, the
/// bubble and the dictation glass, which float over whatever has the keyboard.
pub fn non_activating(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    if hwnd == 0 {
        return;
    }
    unsafe {
        let hwnd = hwnd as HWND;
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let style = (style | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize);
    }
}
