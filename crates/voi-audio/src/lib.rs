//! Audio capture for Voi.
//!
//! Every source produces the same thing: a channel of [`AudioChunk`]s carrying
//! interleaved `f32` samples at a known rate. Streams stay independent all the
//! way to the transcription service — that separation is what makes speaker
//! attribution reliable. Rate conversion to 16 kHz lives in [`resample`] and is
//! applied per stream, never across them.

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;

mod file;
mod mic;
pub mod permissions;
pub mod resample;
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
mod supervise;
pub mod watch;
pub mod wav;

#[cfg_attr(target_os = "macos", path = "system_macos.rs")]
#[cfg_attr(windows, path = "system_windows.rs")]
#[cfg_attr(not(any(target_os = "macos", windows)), path = "system_stub.rs")]
mod system;

pub use file::FileSource;
pub use mic::{inputs, MicSource};
pub use resample::{Resampler, TARGET_RATE};
pub use system::SystemSource;

/// A block of captured audio. Mono after [`AudioChunk::to_mono`], interleaved before it.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioChunk {
    /// Downmix to mono by averaging channels. A no-op when already mono.
    pub fn to_mono(&self) -> Vec<f32> {
        if self.channels <= 1 {
            return self.samples.clone();
        }
        let n = self.channels as usize;
        self.samples
            .chunks(n)
            .map(|frame| frame.iter().sum::<f32>() / n as f32)
            .collect()
    }
}

/// Convert mono `f32` samples to little-endian `s16` bytes.
///
/// This halves the bytes on the wire versus sending `pcm_f32le`, and is the only
/// format conversion in the capture path.
pub fn f32_to_s16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// How many [`AudioStream`]s exist right now, across every source.
static OPEN: AtomicUsize = AtomicUsize::new(0);

/// How many captures this process is holding open, by its own account.
///
/// The other half of [`watch::we_are_listening`]: when this is zero and Core
/// Audio still says we are taking input, a stream was dropped without closing
/// its device — which is exactly how cpal 0.15 kept the microphone light on
/// for as long as Voi ran.
pub fn open_streams() -> usize {
    OPEN.load(Ordering::SeqCst)
}

/// A running capture. Dropping this stops the underlying device.
///
/// `rx` is deliberately **private**. When it was public, moving a stream into an
/// `async move` block and calling `stream.rx.recv()` captured only that one
/// field — Rust 2021 closures capture disjoint fields, not whole structs — so
/// `_guard` was dropped at the end of the enclosing function. The device shut
/// down instantly and the channel closed, which looked exactly like a capture
/// backend that produced nothing. Going through `recv(&mut self)` forces the
/// whole struct to be captured, so the guard cannot be separated from the
/// receiver by accident.
pub struct AudioStream {
    rx: mpsc::Receiver<AudioChunk>,
    pub sample_rate: u32,
    pub channels: u16,
    /// Keeps the platform device/thread alive for as long as the stream lives.
    _guard: Box<dyn Send>,
}

impl AudioStream {
    pub(crate) fn new(
        rx: mpsc::Receiver<AudioChunk>,
        sample_rate: u32,
        channels: u16,
        guard: Box<dyn Send>,
    ) -> Self {
        OPEN.fetch_add(1, Ordering::SeqCst);
        Self { rx, sample_rate, channels, _guard: guard }
    }

    /// Next block of captured audio, or `None` once capture has stopped.
    pub async fn recv(&mut self) -> Option<AudioChunk> {
        self.rx.recv().await
    }
}

impl Drop for AudioStream {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Which side of the conversation a stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// The local microphone — the user, by construction.
    Mic,
    /// System output — everyone else on the call.
    System,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Mic => "You",
            Origin::System => "Them",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_passthrough_is_identity() {
        let c = AudioChunk { samples: vec![0.1, -0.2, 0.3], sample_rate: 16000, channels: 1 };
        assert_eq!(c.to_mono(), vec![0.1, -0.2, 0.3]);
    }

    #[test]
    fn stereo_downmixes_by_averaging() {
        let c = AudioChunk { samples: vec![1.0, 0.0, -1.0, 1.0], sample_rate: 48000, channels: 2 };
        assert_eq!(c.to_mono(), vec![0.5, 0.0]);
    }

    /// Regression test for the disjoint-capture bug described on [`AudioStream`].
    ///
    /// Moving a stream into a spawned task must keep the device guard alive. When
    /// `rx` was a public field, `async move { stream.rx.recv() }` captured only
    /// the receiver and dropped the guard immediately.
    #[tokio::test]
    async fn moving_a_stream_into_a_task_keeps_the_guard_alive() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<AudioChunk>(4);
        let stream = AudioStream::new(rx, 16000, 1, Box::new(DropFlag(dropped.clone())));

        let handle = tokio::spawn(async move {
            let mut stream = stream;
            stream.recv().await.map(|c| c.samples.len())
        });

        // If the guard had been dropped with the enclosing scope, this would
        // already be true while the task is still waiting for audio.
        tokio::task::yield_now().await;
        assert!(
            !dropped.load(Ordering::SeqCst),
            "device guard was dropped while the stream was still live"
        );

        tx.send(AudioChunk { samples: vec![0.0; 8], sample_rate: 16000, channels: 1 })
            .await
            .unwrap();
        assert_eq!(handle.await.unwrap(), Some(8));
        assert!(dropped.load(Ordering::SeqCst), "guard should drop with the stream");
    }

    #[test]
    fn s16_conversion_round_trips_and_clamps() {
        let bytes = f32_to_s16le(&[0.0, 1.0, -1.0, 2.0, -2.0]);
        assert_eq!(bytes.len(), 10);
        let vals: Vec<i16> = bytes
            .chunks(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(vals[0], 0);
        assert_eq!(vals[1], i16::MAX);
        assert_eq!(vals[2], -i16::MAX);
        // Out-of-range input must clamp rather than wrap to the opposite sign.
        assert_eq!(vals[3], i16::MAX);
        assert_eq!(vals[4], -i16::MAX);
    }
}
