//! Replays a WAV file as if it were live capture.
//!
//! This is what makes the rest of the stack testable without a microphone, a
//! permission prompt, or a real call.

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::{AudioChunk, AudioStream};

pub struct FileSource;

impl FileSource {
    /// Stream `path` in `chunk_ms` blocks.
    ///
    /// When `realtime` is set, chunks are paced to wall-clock so downstream
    /// latency behaves as it would on a live call. Otherwise they are delivered
    /// as fast as the consumer accepts them, which is what tests want.
    pub fn start(path: impl AsRef<std::path::Path>, chunk_ms: u64, realtime: bool) -> Result<AudioStream> {
        let path = path.as_ref().to_path_buf();
        let reader = hound::WavReader::open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        let spec = reader.spec();
        let sample_rate = spec.sample_rate;
        let channels = spec.channels;

        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .into_samples::<f32>()
                .collect::<Result<_, _>>()
                .context("failed reading float samples")?,
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / max))
                    .collect::<Result<_, _>>()
                    .context("failed reading integer samples")?
            }
        };

        let frames_per_chunk = (sample_rate as u64 * chunk_ms / 1000) as usize;
        let samples_per_chunk = frames_per_chunk * channels as usize;

        let (tx, rx) = mpsc::channel::<AudioChunk>(64);
        tokio::spawn(async move {
            for block in samples.chunks(samples_per_chunk.max(1)) {
                let chunk = AudioChunk { samples: block.to_vec(), sample_rate, channels };
                if tx.send(chunk).await.is_err() {
                    return; // consumer gone
                }
                if realtime {
                    tokio::time::sleep(std::time::Duration::from_millis(chunk_ms)).await;
                }
            }
        });

        Ok(AudioStream::new(rx, sample_rate, channels, Box::new(())))
    }
}
