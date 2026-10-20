//! Microphone capture via cpal. Works unchanged on Windows, so nothing here is
//! behind a platform cfg.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{AudioChunk, AudioStream};

pub struct MicSource;

/// Held by [`AudioStream`]; dropping it tells the capture thread to stop.
struct MicGuard {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for MicGuard {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The aggregate device the system-audio tap builds while a call is being
/// recorded. It takes input, but it is not a microphone.
const OUR_TAP: &str = "Voi System Tap";

/// What the device calls itself — the name Settings shows and saves.
fn name_of(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

/// The microphones this Mac has, by name, and the name of the one the system
/// uses by default. Listing them opens nothing — AirPods stay as they are.
pub fn inputs() -> (Vec<String>, Option<String>) {
    let host = cpal::default_host();
    let names = host
        .input_devices()
        .map(|devices| devices.filter_map(|d| name_of(&d)).filter(|n| n != OUR_TAP).collect())
        .unwrap_or_default();
    let default = host.default_input_device().and_then(|d| name_of(&d));
    (names, default)
}

impl MicSource {
    /// Open the default input device.
    pub fn start() -> Result<AudioStream> {
        Self::start_named(None)
    }

    /// Open the microphone called `name`, or the default one when it is `None`
    /// or not connected right now.
    ///
    /// Not finding it is not an error: a MacBook with its lid shut has no
    /// built-in microphone, and losing your words over that would be worse
    /// than taking them from the default one.
    ///
    /// The `Stream` is created and owned by a dedicated thread that parks
    /// until the guard is dropped; the stream dropping there is what closes
    /// the device and puts the microphone light out.
    pub fn start_named(name: Option<&str>) -> Result<AudioStream> {
        let host = cpal::default_host();
        let chosen = name.and_then(|name| {
            let found = host
                .input_devices()
                .ok()
                .and_then(|mut devices| devices.find(|d| name_of(d).is_some_and(|n| n == name)));
            if found.is_none() {
                warn!(microphone = name, "chosen microphone is not connected; using the default");
            }
            found
        });
        let device = match chosen {
            Some(device) => device,
            None => host
                .default_input_device()
                .context("no default input device — check microphone permission")?,
        };
        let config = device
            .default_input_config()
            .context("failed to read default input config")?;

        let sample_rate = config.sample_rate();
        let channels = config.channels();
        let sample_format = config.sample_format();
        info!(device = name_of(&device).as_deref().unwrap_or("?"), sample_rate, channels, ?sample_format, "opening microphone");

        let (tx, rx) = mpsc::channel::<AudioChunk>(64);
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let thread = std::thread::Builder::new()
            .name("voi-mic".into())
            .spawn(move || {
                let stream = build_stream(&device, &config.into(), sample_format, tx, sample_rate, channels);
                match stream {
                    Ok(s) => {
                        if let Err(e) = s.play() {
                            let _ = ready_tx.send(Err(e.into()));
                            return;
                        }
                        let _ = ready_tx.send(Ok(()));
                        // Park until the guard drops and closes the channel.
                        let _ = stop_rx.recv();
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .context("failed to spawn microphone thread")?;

        ready_rx
            .recv()
            .context("microphone thread died during startup")??;

        let guard = MicGuard { stop: Some(stop_tx), thread: Some(thread) };
        Ok(AudioStream::new(rx, sample_rate, channels, Box::new(guard)))
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    tx: mpsc::Sender<AudioChunk>,
    sample_rate: u32,
    channels: u16,
) -> Result<cpal::Stream> {
    let err_fn = |e| warn!(error = %e, "microphone stream error");

    // The capture callback is realtime — it must never block. `try_send` drops the
    // chunk if the consumer has fallen behind, which is the correct tradeoff for the
    // transcription path. The disk-writing path is fed separately and never drops.
    macro_rules! stream_for {
        ($t:ty) => {{
            let tx = tx.clone();
            device.build_input_stream(
                config.clone(),
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> =
                        data.iter().map(|s| cpal::Sample::to_float_sample(*s)).collect();
                    let _ = tx.try_send(AudioChunk { samples, sample_rate, channels });
                },
                err_fn,
                None,
            )?
        }};
    }

    let stream = match format {
        cpal::SampleFormat::F32 => stream_for!(f32),
        cpal::SampleFormat::I16 => stream_for!(i16),
        cpal::SampleFormat::U16 => stream_for!(u16),
        other => anyhow::bail!("unsupported microphone sample format: {other:?}"),
    };
    Ok(stream)
}
