//! Settings has to hear about it when the microphones change.
//!
//! The list in Settings was filled once, when the window opened, so it kept
//! naming the old default for as long as it stayed open. It now redraws when
//! Core Audio says a device came or went, or the default input moved. This
//! makes a device come and go — an empty aggregate device, which nothing plays
//! through and which is not made the default — and checks that the news
//! arrived.
//!
//! It adds a device to the Mac for a moment, so it runs only when asked:
//!
//! ```sh
//! cargo test -p voi-audio --test inputs_changed -- --ignored
//! ```

#![cfg(target_os = "macos")]

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey, kAudioAggregateDeviceUIDKey,
    AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice, AudioObjectID,
};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use voi_audio::watch::inputs_changed;

fn key(k: &std::ffi::CStr) -> Retained<NSString> {
    NSString::from_str(k.to_str().unwrap())
}

/// Wait up to two seconds for Core Audio to say something changed.
fn heard() -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if inputs_changed() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
#[ignore = "adds an audio device to the Mac for a moment"]
fn a_device_coming_and_going_is_heard() {
    // The first call starts listening; anything already pending is dropped.
    inputs_changed();
    std::thread::sleep(Duration::from_millis(200));
    inputs_changed();

    let uid = format!("voi-test-{}", std::process::id());
    let description: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
        &[
            &*key(kAudioAggregateDeviceNameKey),
            &*key(kAudioAggregateDeviceUIDKey),
            &*key(kAudioAggregateDeviceIsPrivateKey),
        ],
        &[
            &*NSString::from_str("Voi Test Device") as &AnyObject,
            &*NSString::from_str(&uid) as &AnyObject,
            &*NSNumber::new_bool(true) as &AnyObject,
        ],
    );
    let mut device: AudioObjectID = 0;
    let status = unsafe {
        let cf = &*(Retained::as_ptr(&description) as *const CFDictionary);
        AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut device))
    };
    assert_eq!(status, 0, "could not create the test device");
    let came = heard();

    let status = unsafe { AudioHardwareDestroyAggregateDevice(device) };
    assert_eq!(status, 0, "could not remove the test device");
    let went = heard();

    assert!(came, "a device appeared and nothing was heard");
    assert!(went, "a device disappeared and nothing was heard");
}
