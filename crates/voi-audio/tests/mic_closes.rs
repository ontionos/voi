//! Letting go of the microphone has to actually let go of it.
//!
//! cpal 0.15 leaked every stream opened on a device picked by name: the
//! disconnect listener held the stream and the stream held the listener, so
//! dropping it stopped nothing and the microphone light stayed on for as long
//! as Voi ran. This asks Core Audio — not our own bookkeeping — whether this
//! process is still taking input after the stream is gone.
//!
//! It opens a real microphone, so it does not run with plain `cargo test`:
//!
//! ```sh
//! cargo test -p voi-audio --test mic_closes -- --ignored
//! ```
//!
//! The built-in microphone is used unless `VOI_TEST_MIC` names another.
//! Never a Bluetooth headset by default: opening AirPods' microphone switches
//! them out of music mode, and a test must not do that to whoever is listening.

use std::time::{Duration, Instant};

use voi_audio::{inputs, open_streams, watch::we_are_listening, MicSource};

/// Wait up to `limit` for `want` to become true.
fn within(limit: Duration, want: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if want() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    want()
}

#[test]
#[ignore = "opens a real microphone"]
fn a_microphone_chosen_by_name_closes_when_dropped() {
    let (names, _) = inputs();
    let name = std::env::var("VOI_TEST_MIC")
        .ok()
        .or_else(|| names.iter().find(|n| n.contains("MacBook")).cloned())
        .expect("no built-in microphone; set VOI_TEST_MIC to one from inputs()");
    assert!(names.contains(&name), "{name:?} is not among {names:?}");

    assert!(!we_are_listening(), "already listening before the test opened anything");

    for round in 1..=3 {
        let stream = MicSource::start_named(Some(&name)).expect("microphone would not open");
        assert!(
            within(Duration::from_secs(2), we_are_listening),
            "round {round}: Core Audio never saw the microphone open"
        );
        assert_eq!(open_streams(), 1, "round {round}: the open stream was not counted");
        drop(stream);
        assert_eq!(open_streams(), 0, "round {round}: the dropped stream is still counted");
        assert!(
            within(Duration::from_secs(1), || !we_are_listening()),
            "round {round}: the microphone stayed open after the stream was dropped"
        );
    }
}
