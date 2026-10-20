//! Does system-audio capture hear what is played? Without anyone hearing it.
//!
//!   loopback_check                      # list the output devices
//!   loopback_check "Device name" a.wav  # play a.wav on that device and capture it back
//!
//! Point it at an output nothing is plugged into — a monitor without
//! speakers, a virtual device — and it proves loopback end to end in silence:
//! the WAV goes out through that device's mix and comes back through
//! `SystemSource`, which is told to listen there by `VOI_LOOPBACK_DEVICE`.

use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn name_of(d: &cpal::Device) -> String {
    d.description().map(|d| d.name().to_string()).unwrap_or_default()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let host = cpal::default_host();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let default = host.default_output_device().map(|d| name_of(&d));
    let Some(wanted) = args.first() else {
        for d in host.output_devices()? {
            let n = name_of(&d);
            println!("{}{n}", if Some(&n) == default.as_ref() { "* " } else { "  " });
        }
        return Ok(());
    };
    anyhow::ensure!(Some(wanted) != default.as_ref(), "refusing to play on the default output: someone may hear it");
    let device = host.output_devices()?.find(|d| &name_of(d) == wanted).ok_or_else(|| anyhow::anyhow!("no such output"))?;

    // The speech, as mono samples, stretched to the device's rate by repetition.
    let mut reader = hound::WavReader::open(&args[1])?;
    let rate = reader.spec().sample_rate;
    let speech: Vec<f32> = reader.samples::<i16>().map(|s| s.unwrap_or(0) as f32 / 32768.0).collect();
    let reader_seconds = speech.len() as f64 / rate as f64;
    let config = device.default_output_config()?;
    let (out_rate, channels) = (config.sample_rate(), config.channels() as usize);
    let step = rate as f64 / out_rate as f64;
    let mut pos = 0f64;
    let stream = device.build_output_stream(
        config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            for frame in data.chunks_mut(channels) {
                let s = speech.get(pos as usize).copied().unwrap_or(0.0);
                pos += step;
                frame.fill(s);
            }
        },
        |e| eprintln!("play error: {e}"),
        None,
    )?;

    std::env::set_var("VOI_LOOPBACK_DEVICE", wanted);
    let mut capture = voi_audio::SystemSource::start()?;
    stream.play()?;
    let (mut peak, mut frames) = (0f32, 0usize);
    let seconds = reader_seconds + 1.0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(seconds);
    while let Ok(Some(chunk)) = tokio::time::timeout_at(deadline, capture.recv()).await {
        frames += chunk.samples.len() / chunk.channels.max(1) as usize;
        peak = chunk.samples.iter().fold(peak, |p, s| p.max(s.abs()));
    }
    println!("played on {wanted:?}; captured {frames} frames at {} Hz, peak {peak:.3}", capture.sample_rate);
    Ok(())
}
