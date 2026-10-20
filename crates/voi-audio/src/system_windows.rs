//! System audio on Windows: WASAPI loopback of the render device.
//!
//! cpal opens a loopback client when an *input* stream is built on an
//! *output* device, which is everything Windows needs to hand over the mix
//! going to the speakers — no driver, no virtual cable, no permission prompt.
//!
//! One difference from a Core Audio tap has to be made up for here. A tap
//! delivers zeros while nothing plays; loopback delivers nothing at all, and
//! the stream simply stops. Soniox times words by the audio it was sent, so
//! a quiet minute that never arrived would put everything said after it a
//! minute early against the microphone's clock — and the two sides would be
//! interleaved in the wrong order. A filler thread therefore tops the stream
//! up with silence whenever it falls behind the wall clock; see [`owed`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{AudioChunk, AudioStream};

pub struct SystemSource;

/// How far behind the wall clock the stream may fall before silence is added.
/// Comfortably more than one WASAPI period, so a late packet is not doubled.
const SLACK: Duration = Duration::from_millis(120);
/// How often the filler looks.
const TICK: Duration = Duration::from_millis(40);

/// Frames of silence owed: how far `delivered` is behind `expected`, less the
/// slack, or nothing while it is within it.
pub(crate) fn owed(expected: u64, delivered: u64, slack: u64) -> u64 {
    expected.saturating_sub(delivered).saturating_sub(slack)
}

struct Guard {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn name_of(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

impl SystemSource {
    /// Capture what the default output device is playing.
    ///
    /// `VOI_LOOPBACK_DEVICE` names another output device instead. Tests use it
    /// to play speech into an output nobody listens to and record it back,
    /// with nothing coming out of the speakers.
    pub fn start() -> Result<AudioStream> {
        let host = cpal::default_host();
        let wanted = std::env::var("VOI_LOOPBACK_DEVICE").ok().filter(|s| !s.is_empty());
        let device = match wanted {
            Some(name) => host
                .output_devices()?
                .find(|d| name_of(d).is_some_and(|n| n == name))
                .with_context(|| format!("no output device called {name:?}"))?,
            None => host.default_output_device().context("no default output device")?,
        };
        let config = device
            .default_output_config()
            .context("failed to read the output device's format")?;
        let sample_rate = config.sample_rate();
        let channels = config.channels();
        let format = config.sample_format();
        info!(device = name_of(&device).as_deref().unwrap_or("?"), sample_rate, channels, ?format, "opening system audio loopback");

        let (tx, rx) = mpsc::channel::<AudioChunk>(64);
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        let delivered = Arc::new(AtomicU64::new(0));

        let thread = std::thread::Builder::new()
            .name("voi-loopback".into())
            .spawn(move || {
                let stream = build(&device, &config.into(), format, tx.clone(), delivered.clone(), sample_rate, channels);
                let stream = match stream.and_then(|s| s.play().map(|_| s).map_err(Into::into)) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));

                // The stream lives on this thread; topping it up does too, so
                // dropping the guard ends both at once.
                let started = Instant::now();
                let slack = (SLACK.as_secs_f64() * sample_rate as f64) as u64;
                loop {
                    match stop_rx.recv_timeout(TICK) {
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                    let expected = (started.elapsed().as_secs_f64() * sample_rate as f64) as u64;
                    let frames = owed(expected, delivered.load(Ordering::Relaxed), slack);
                    if frames == 0 {
                        continue;
                    }
                    delivered.fetch_add(frames, Ordering::Relaxed);
                    let samples = vec![0.0; frames as usize * channels as usize];
                    let _ = tx.try_send(AudioChunk { samples, sample_rate, channels });
                }
                drop(stream);
            })
            .context("failed to spawn loopback thread")?;

        ready_rx.recv().context("loopback thread died during startup")??;
        let guard = Guard { stop: Some(stop_tx), thread: Some(thread) };
        Ok(AudioStream::new(rx, sample_rate, channels, Box::new(guard)))
    }
}

fn build(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    tx: mpsc::Sender<AudioChunk>,
    delivered: Arc<AtomicU64>,
    sample_rate: u32,
    channels: u16,
) -> Result<cpal::Stream> {
    let err_fn = |e| warn!(error = %e, "system audio stream error");
    macro_rules! stream_for {
        ($t:ty) => {{
            let tx = tx.clone();
            let delivered = delivered.clone();
            device.build_input_stream(
                config.clone(),
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> =
                        data.iter().map(|s| cpal::Sample::to_float_sample(*s)).collect();
                    delivered.fetch_add((samples.len() / channels.max(1) as usize) as u64, Ordering::Relaxed);
                    let _ = tx.try_send(AudioChunk { samples, sample_rate, channels });
                },
                err_fn,
                None,
            )?
        }};
    }
    Ok(match format {
        cpal::SampleFormat::F32 => stream_for!(f32),
        cpal::SampleFormat::I16 => stream_for!(i16),
        cpal::SampleFormat::I32 => stream_for!(i32),
        other => anyhow::bail!("unsupported output sample format: {other:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::owed;

    #[test]
    fn silence_is_owed_only_past_the_slack() {
        assert_eq!(owed(48_000, 48_000, 5_760), 0, "caught up");
        assert_eq!(owed(48_000, 44_000, 5_760), 0, "behind, but within a period or two");
        assert_eq!(owed(96_000, 48_000, 5_760), 48_000 - 5_760, "a quiet second is filled");
        assert_eq!(owed(1_000, 48_000, 5_760), 0, "ahead of the clock owes nothing");
    }
}
