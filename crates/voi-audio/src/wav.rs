//! Incremental WAV writing.
//!
//! Audio goes to disk as it arrives so that a long call never accumulates in
//! memory, and so that a crash or a network failure still leaves a usable
//! recording behind.

use anyhow::{Context, Result};
use std::io::BufWriter;
use std::path::Path;

pub struct WavSink {
    writer: hound::WavWriter<BufWriter<std::fs::File>>,
    frames: u64,
    /// Samples written since the header was last brought up to date.
    since_flush: u64,
    flush_every: u64,
}

impl WavSink {
    pub fn create(path: impl AsRef<Path>, sample_rate: u32, channels: u16) -> Result<Self> {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let writer = hound::WavWriter::create(path, spec)
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self {
            writer,
            frames: 0,
            since_flush: 0,
            // Refresh the header every ~5s of audio. Without this, a process
            // killed mid-recording leaves a file whose header still claims zero
            // frames — the samples are all on disk but nothing will play it.
            flush_every: sample_rate as u64 * channels.max(1) as u64 * 5,
        })
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<()> {
        for &s in samples {
            self.writer
                .write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        self.frames += samples.len() as u64;
        self.since_flush += samples.len() as u64;

        if self.since_flush >= self.flush_every {
            self.since_flush = 0;
            self.flush()?;
        }
        Ok(())
    }

    /// Bring the header up to date so the file is playable even if we are
    /// killed before [`finalize`](Self::finalize) runs.
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush().context("failed to flush wav")
    }

    pub fn finalize(self) -> Result<u64> {
        let frames = self.frames;
        self.writer.finalize().context("failed to finalize wav")?;
        Ok(frames)
    }
}

/// Patch a WAV header whose size fields were never written.
///
/// A process killed before the header is updated leaves every sample on disk
/// behind a header claiming zero bytes of audio, so nothing will open the file.
/// The samples are intact; only the two length fields are wrong.
///
/// Returns the number of recovered frames, or `None` if the header was already
/// correct.
pub fn repair_header(path: impl AsRef<Path>) -> Result<Option<u64>> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let path = path.as_ref();
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;

    let len = f.metadata()?.len();
    anyhow::ensure!(len >= 44, "{} is too short to be a WAV", path.display());

    let mut hdr = [0u8; 44];
    f.read_exact(&mut hdr)?;
    anyhow::ensure!(
        &hdr[0..4] == b"RIFF" && &hdr[8..12] == b"WAVE" && &hdr[36..40] == b"data",
        "{} does not have a canonical 44-byte WAV header",
        path.display()
    );

    let declared = u32::from_le_bytes([hdr[40], hdr[41], hdr[42], hdr[43]]) as u64;
    let actual = len - 44;
    if declared == actual {
        return Ok(None);
    }

    f.seek(SeekFrom::Start(4))?;
    f.write_all(&((len - 8) as u32).to_le_bytes())?;
    f.seek(SeekFrom::Start(40))?;
    f.write_all(&(actual as u32).to_le_bytes())?;
    f.flush()?;

    let bits = u16::from_le_bytes([hdr[34], hdr[35]]).max(8) as u64;
    Ok(Some(actual / (bits / 8)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this guards against: a recording was terminated before
    /// `finalize`, so 33 MB of real audio sat on disk behind a header claiming
    /// zero frames, and nothing would open it.
    #[test]
    fn a_file_is_playable_before_finalize() {
        let dir = std::env::temp_dir().join(format!("voi-wav-flush-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.wav");

        {
            let mut sink = WavSink::create(&path, 100, 1).unwrap();
            // flush_every is 500 samples at this rate; write past it.
            sink.write(&vec![0.25; 600]).unwrap();
            // Deliberately drop without finalizing, as a kill would.
        }

        let r = hound::WavReader::open(&path).expect("must be readable without finalize");
        assert_eq!(r.len(), 600, "header must reflect the samples already written");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repair_recovers_a_file_with_a_zeroed_header() {
        use std::io::{Seek, SeekFrom, Write};

        let dir = std::env::temp_dir().join(format!("voi-wav-repair-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.wav");

        let mut sink = WavSink::create(&path, 16000, 1).unwrap();
        sink.write(&vec![0.5; 1000]).unwrap();
        sink.finalize().unwrap();

        // Zero the two length fields, exactly as an unfinalized file has them.
        {
            let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.seek(SeekFrom::Start(4)).unwrap();
            f.write_all(&0u32.to_le_bytes()).unwrap();
            f.seek(SeekFrom::Start(40)).unwrap();
            f.write_all(&0u32.to_le_bytes()).unwrap();
        }
        assert!(hound::WavReader::open(&path).map(|r| r.len()).unwrap_or(0) == 0);

        let recovered = repair_header(&path).unwrap();
        assert_eq!(recovered, Some(1000));

        let r = hound::WavReader::open(&path).unwrap();
        assert_eq!(r.len(), 1000, "audio must be readable after repair");
        assert_eq!(repair_header(&path).unwrap(), None, "repair must be idempotent");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn writes_a_readable_file() {
        let dir = std::env::temp_dir().join(format!("voi-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");

        let mut sink = WavSink::create(&path, 16000, 1).unwrap();
        sink.write(&[0.0, 0.5, -0.5]).unwrap();
        let n = sink.finalize().unwrap();
        assert_eq!(n, 3);

        let r = hound::WavReader::open(&path).unwrap();
        assert_eq!(r.spec().sample_rate, 16000);
        assert_eq!(r.spec().channels, 1);
        assert_eq!(r.len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }
}
