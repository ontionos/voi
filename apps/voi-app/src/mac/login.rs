//! Launch at login, through the system's own Login Items.
//!
//! Nothing about it is kept in `settings.json`. The same switch exists in
//! System Settings ▸ General ▸ Login Items, and a copy of the answer in our own
//! file would be wrong the moment it was flipped there. So the answer is asked
//! of macOS every time, and setting it is telling macOS.
//!
//! `SMAppService.mainAppService` registers the bundle this binary runs from,
//! which is why it only means something for the installed `/Applications/Voi.app`.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::NSError;

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// What macOS says about Voi as a login item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Off,
    On,
    /// Registered, but switched off in System Settings until you allow it.
    NeedsApproval,
    /// No Login Items service on this Mac at all (before macOS 13).
    Unavailable,
}

impl Status {
    /// Whether the switch should show on. Waiting for approval counts: it was
    /// asked for, and macOS is the one holding it back.
    pub fn wanted(self) -> bool {
        matches!(self, Status::On | Status::NeedsApproval)
    }
}

fn service() -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"SMAppService")?;
    unsafe { msg_send![class, mainAppService] }
}

pub fn status() -> Status {
    let Some(service) = service() else {
        return Status::Unavailable;
    };
    // SMAppServiceStatus: NotRegistered, Enabled, RequiresApproval, NotFound.
    // NotFound is also what an app that never registered gets — the installed
    // Voi answered it before its first switch-on — so it reads as off. A copy
    // that really cannot be registered says so when the switch is flipped.
    let raw: isize = unsafe { msg_send![&service, status] };
    match raw {
        1 => Status::On,
        2 => Status::NeedsApproval,
        _ => Status::Off,
    }
}

/// Whether this binary runs from inside an app bundle.
///
/// macOS does not check: asked from a bare `target/debug/voi-app`, it
/// registered that path as a login item, and the next login would have
/// started a development build beside the installed Voi.
fn in_a_bundle() -> bool {
    std::env::current_exe()
        .is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"))
}

/// Turn it on or off, and say what macOS made of that.
pub fn set(on: bool) -> Result<Status, String> {
    if on && !in_a_bundle() {
        return Err("only the installed Voi.app can launch at login, not a bare binary".into());
    }
    let service = service().ok_or("this Mac has no Login Items service")?;
    let done: Result<(), Retained<NSError>> = unsafe {
        if on {
            msg_send![&service, registerAndReturnError: _]
        } else {
            msg_send![&service, unregisterAndReturnError: _]
        }
    };
    done.map_err(|e| e.localizedDescription().to_string())?;
    let now = status();
    tracing::info!(asked = on, ?now, "launch at login");
    Ok(now)
}
