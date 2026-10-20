//! Downsampling to 16 kHz mono.
//!
//! Speech recognition gains nothing above 16 kHz, and the difference on disk is
//! large: 48 kHz 16-bit mono is 96 KB/s, so two streams cost ~690 MB per hour.
//! At 16 kHz that drops to ~230 MB, and the 32-bit size field in a WAV header
//! stops being a practical ceiling until well past a day of recording.
//!
//! This reverses an earlier decision to send every stream at its native rate.
//! That kept the pipeline simpler, but it is not worth three times the disk and
//! bandwidth for audio nobody listens to at full fidelity.

/// Target rate for everything written to disk and sent for transcription.
pub const TARGET_RATE: u32 = 16_000;

/// Linear-interpolating downsampler that keeps its phase across chunks.
///
/// Carrying `pos` and `last` between calls is the whole point: resetting per
/// chunk would drop or duplicate a fraction of a sample every 100 ms, which
/// accumulates into audible clicks and a drifting timeline.
pub struct Resampler {
    from_rate: u32,
    /// Position in the input stream, in input samples, of the next output sample.
    pos: f64,
    /// Final sample of the previous chunk, so interpolation can span the join.
    last: f32,
    primed: bool,
}

impl Resampler {
    pub fn new(from_rate: u32) -> Self {
        Self { from_rate: from_rate.max(1), pos: 0.0, last: 0.0, primed: false }
    }

    pub fn from_rate(&self) -> u32 {
        self.from_rate
    }

    /// Resample one chunk of mono input.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        if self.from_rate == TARGET_RATE {
            return input.to_vec();
        }

        let step = self.from_rate as f64 / TARGET_RATE as f64;
        let mut out = Vec::with_capacity((input.len() as f64 / step) as usize + 1);

        // `pos` is relative to the start of this chunk, and may be negative when
        // the previous chunk ended mid-interval — that sample is `last`.
        while self.pos < input.len() as f64 {
            let i = self.pos.floor();
            let frac = (self.pos - i) as f32;
            let i = i as isize;

            let a = if i < 0 {
                if self.primed {
                    self.last
                } else {
                    input[0]
                }
            } else {
                input[i as usize]
            };
            let b = input[(i + 1).clamp(0, input.len() as isize - 1) as usize];

            out.push(a + (b - a) * frac);
            self.pos += step;
        }

        self.pos -= input.len() as f64;
        self.last = input[input.len() - 1];
        self.primed = true;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_rate_passes_through_untouched() {
        let mut r = Resampler::new(TARGET_RATE);
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn three_to_one_produces_a_third_of_the_samples() {
        let mut r = Resampler::new(48_000);
        let input = vec![0.5f32; 4800]; // 100ms at 48kHz
        let out = r.process(&input);
        assert!(
            (out.len() as i64 - 1600).abs() <= 1,
            "expected ~1600 samples, got {}",
            out.len()
        );
    }

    /// Phase must carry across chunks; resetting it per chunk drifts the
    /// timeline and clicks at every join.
    #[test]
    fn output_length_stays_correct_across_many_chunks() {
        let mut r = Resampler::new(44_100);
        let chunk = vec![0.25f32; 441]; // 10ms
        let mut total = 0;
        for _ in 0..100 {
            total += r.process(&chunk).len();
        }
        // 100 × 10ms at 16kHz = 16000 samples.
        assert!(
            (total as i64 - 16_000).abs() <= 2,
            "drifted to {total} samples over a second"
        );
    }

    #[test]
    fn a_constant_signal_stays_constant() {
        let mut r = Resampler::new(48_000);
        r.process(&vec![1.0f32; 480]);
        let out = r.process(&vec![1.0f32; 480]);
        assert!(
            out.iter().all(|s| (*s - 1.0).abs() < 1e-6),
            "interpolation must not ripple on a flat signal"
        );
    }

    #[test]
    fn empty_input_is_harmless() {
        let mut r = Resampler::new(48_000);
        assert!(r.process(&[]).is_empty());
    }
}
