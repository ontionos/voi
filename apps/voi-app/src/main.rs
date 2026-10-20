//! Voi, the floating transcription HUD.
//!
//! One app, two front ends. The recording engine, the transcript and the
//! settings file are the shared crates; what is drawn and how the system is
//! asked for things is not, and lives in a module per platform:
//!
//! - [`mac`] — the menu-bar app and overlay, in AppKit.
//! - [`win`] — the tray icon, HUD and settings window, in egui over Win32.
//!
//! [`state`] and [`engine`] are what both front ends share: the live
//! transcript, the meter and the recording mode, which the worker writes and
//! the front end paints from, and the starting and stopping that drives it.

// No console window behind the HUD. The CLI is a separate binary and keeps one.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
mod engine;
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
mod press;
// Some of what the Mac's overlay animates the Windows HUD keeps for itself.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod state;
mod updates;

fn main() {
    #[cfg(target_os = "macos")]
    mac::main();
    #[cfg(windows)]
    win::main();
    #[cfg(not(any(target_os = "macos", windows)))]
    eprintln!("Voi's app runs on macOS and Windows; use the `voi` command line here.");
}
