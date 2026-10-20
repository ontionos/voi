//! Launch at login, through the Windows startup list.
//!
//! The per-user `Run` key is what Task Manager's Startup tab and Settings ▸
//! Apps ▸ Startup both show and switch, so — as on the Mac — the answer is
//! asked of the system every time rather than copied into `settings.json`,
//! where it would be wrong the moment somebody flipped it there. A value that
//! Startup has switched off is recorded beside it, under `StartupApproved`.

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
use winreg::RegKey;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const NAME: &str = "Voi";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Off,
    On,
    /// In the list, but switched off in Task Manager or Settings.
    Disabled,
}

impl Status {
    pub fn wanted(self) -> bool {
        self == Status::On
    }
}

fn command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(format!("\"{}\"", exe.display()))
}

pub fn status() -> Status {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(run) = hkcu.open_subkey_with_flags(RUN, KEY_READ) else { return Status::Off };
    if run.get_value::<String, _>(NAME).is_err() {
        return Status::Off;
    }
    // The first byte of the approval record is 2 (or 6) while enabled and 3
    // once switched off; no record at all means it was never switched off.
    let disabled = hkcu
        .open_subkey_with_flags(APPROVED, KEY_READ)
        .and_then(|k| k.get_raw_value(NAME))
        .is_ok_and(|v| v.bytes.first().is_some_and(|b| b & 1 == 1));
    if disabled {
        Status::Disabled
    } else {
        Status::On
    }
}

pub fn set(on: bool) -> Result<Status, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu.create_subkey(RUN).map_err(|e| e.to_string())?;
    if on {
        run.set_value(NAME, &command()?).map_err(|e| e.to_string())?;
        // Switching it on here means on, so forget an earlier "off" from
        // Task Manager rather than leave it overruling this.
        if let Ok(approved) = hkcu.open_subkey_with_flags(APPROVED, KEY_SET_VALUE) {
            let _ = approved.delete_value(NAME);
        }
    } else {
        match run.delete_value(NAME) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(status())
}
