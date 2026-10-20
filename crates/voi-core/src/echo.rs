//! Keeping the speakers out of the microphone.
//!
//! A headset puts its microphone a few centimetres from the speakers it is
//! wearing, and some of what they play gets in; laptop speakers put in a lot
//! more. A speech model hears faint speech perfectly well, so without this the
//! other side's words come back a second time, under "You".
//!
//! This is the plain version of an echo canceller, deliberately. A real one
//! subtracts the echo and leaves your voice standing over it; this only
//! decides, a hundredth of a second at a time, whether the microphone is saying
//! anything of its own. It can, because what the speakers play is already
//! known exactly — the system stream, captured on its way out — and the echo is
//! that same sound arriving a fixed moment later at a fixed fraction of its
//! level. So the gate learns two numbers:
//!
//! - **the delay**: how long after the system stream the echo reaches the
//!   microphone — the lag at which the microphone's loudness rises and falls
//!   most closely with the system's;
//! - **the loss**: how far below the system stream the echo sits once lined up
//!   by that delay.
//!
//! With those, every moment has an expected echo level, and the microphone is
//! let through only when it is well above it. Otherwise it sends silence in its
//! place — silence rather than nothing, because the transcript is ordered by
//! when things were said, and a stream with the quiet parts cut out would move
//! every later word earlier.
//!
//! The first version learned only the loss, against whatever had played in the
//! last moment, and believed any quieter reading at once. The quietest reading
//! is the microphone's own hiss under a loud word, fifty-odd decibels down and
//! far below any echo, so the real echo always looked like you and the gate
//! stayed open: it silenced 0% of call after call. The loss is now a low
//! quartile of what is heard while the system is loud, which a stray quiet
//! moment cannot drag down and your talking over them cannot drag up.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// One step of the timeline both streams are placed on.
const STEP: Duration = Duration::from_millis(10);

/// Samples per step at the pipeline's 16 kHz.
const STEP_SAMPLES: usize = 160;

/// How much of either stream is remembered, in steps: enough for the delay
/// search to look back over a few seconds of conversation.
const REMEMBER: usize = 800;

/// The longest delay looked for, in steps. Wired headphones and built-in
/// speakers put the echo tens of milliseconds behind; Bluetooth, a few hundred.
/// The shortest is set by [`LOOKAHEAD`], and may be below zero.
const MAX_DELAY: i64 = 40;

/// How long the microphone is held before it is judged, in steps.
///
/// The system stream does not always reach us before its own echo does. On a
/// real call the echo came in more than 30 ms ahead of it — the tap and the
/// thread that drains it add their own delay — and a gate that could only look
/// 30 ms ahead found nothing to line up with, decided the microphone heard no
/// speakers, and let 84% of the other side through a second time. Holding the
/// microphone back half a second lets the delay search look that far the other
/// way; the words under "You" only appear half a second later, which nobody
/// sees next to a transcriber that takes longer than that anyway.
const LOOKAHEAD: i64 = 50;

/// Below this nothing worth echoing is playing, and the microphone is left
/// alone entirely. dBFS.
const PLAYING: f32 = -55.0;

/// How far above the expected echo the microphone must be to count as you, in
/// dB. Lined up by the delay, the echo stays within a few decibels of where it
/// is expected; ten leaves room for a real room, and a voice speaking into a
/// microphone clears it easily.
const MARGIN: f32 = 10.0;

/// Once you are speaking, how far above the echo the microphone must stay to
/// keep the gate open, in dB. Speech is not loud all the way through —
/// consonants, the ends of words and the breath between them dip — and without
/// this only the stressed syllables would get through; the echo itself does
/// not reach this far.
const STILL_YOU: f32 = 4.0;

/// How long the gate stays open after the last clearly-you moment, in steps.
const HOLD: i64 = 30;

/// How far either way of the learned delay a sound may land, in steps: two
/// clocks that do not tick quite together.
const SLACK: i64 = 2;

/// How fast an echo is expected to die away after the sound stops, in dB per
/// step — a room's reverberation — and for how many steps.
const TAIL: f32 = 1.5;
const TAIL_STEPS: i64 = 20;

/// How closely the microphone must follow the system to count as hearing it,
/// and below what it plainly does not.
const MATCHED: f64 = 0.6;
const UNMATCHED: f64 = 0.3;

/// Delay searches in a row that must find nothing before the gate decides the
/// microphone does not hear the speakers at all — ten seconds of the other side
/// talking. Longer than anyone talks over them, since that also hides the echo
/// from the search, and letting it through afterwards is the failure this
/// exists to prevent.
const GIVE_UP: usize = 20;

/// Loudness of a block of samples, in dBFS.
pub fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }
    let mean = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    10.0 * (mean + 1e-12).log10()
}

/// Splits a block into roughly step-sized parts, each tagged with the step it
/// ends in, the whole block ending in step `end`.
fn steps(len: usize, end: i64) -> impl Iterator<Item = (std::ops::Range<usize>, i64)> {
    let parts = ((len + STEP_SAMPLES / 2) / STEP_SAMPLES).max(1);
    (0..parts).map(move |i| (len * i / parts..len * (i + 1) / parts, end - (parts - 1 - i) as i64))
}

/// Loudness by step. A slot remembers which step it was written for, so an old
/// value is never mistaken for a new one after a gap.
struct Ring {
    slots: Vec<(i64, f32)>,
}

impl Ring {
    fn new() -> Self {
        Self { slots: vec![(i64::MIN, -120.0); REMEMBER] }
    }

    fn slot(&mut self, step: i64) -> &mut (i64, f32) {
        &mut self.slots[step.rem_euclid(REMEMBER as i64) as usize]
    }

    fn put(&mut self, step: i64, db: f32) {
        let slot = self.slot(step);
        // Two blocks can end in the same step; the louder is what was heard.
        *slot = if slot.0 == step { (step, slot.1.max(db)) } else { (step, db) };
    }

    fn get(&self, step: i64) -> Option<f32> {
        let (at, db) = self.slots[step.rem_euclid(REMEMBER as i64) as usize];
        (at == step).then_some(db)
    }

    /// Loudness at a step, or silence if nothing was heard for it.
    fn level(&self, step: i64) -> f32 {
        self.get(step).unwrap_or(-120.0)
    }
}

/// What the system has been playing, step by step, handed from its pipeline to
/// the microphone's.
pub struct Playing {
    epoch: Instant,
    ring: Mutex<Ring>,
}

impl Playing {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { epoch: Instant::now(), ring: Mutex::new(Ring::new()) })
    }

    fn step(&self, at: Instant) -> i64 {
        (at.saturating_duration_since(self.epoch).as_micros() / STEP.as_micros()) as i64
    }
}

/// The system side: says what it played, and when.
pub struct Source {
    playing: Arc<Playing>,
}

impl Source {
    pub fn new(playing: Arc<Playing>) -> Self {
        Self { playing }
    }

    /// `samples` is a block that finished arriving at `now`.
    pub fn heard(&mut self, samples: &[f32], now: Instant) {
        let end = self.playing.step(now);
        let mut ring = self.playing.ring.lock().unwrap_or_else(PoisonError::into_inner);
        for (range, step) in steps(samples.len(), end) {
            ring.put(step, rms_db(&samples[range]));
        }
    }
}

/// What the gate has been doing, for the log.
pub struct Report {
    /// The share of the microphone silenced while something was playing.
    pub silenced: f32,
    /// How far behind the system stream the echo arrives.
    pub delay_ms: i64,
    /// How far below the system stream the echo sits, in dB.
    pub loss_db: f32,
    /// Whether the microphone hears the speakers at all.
    pub echoing: bool,
    /// How closely the microphone followed the system at the best delay, the
    /// last time it was looked for: near 1 is a plain echo, near 0 none.
    pub matched: f64,
}

/// The microphone side: decides, step by step, whether it is you or the
/// speakers.
pub struct Gate {
    playing: Arc<Playing>,
    /// Microphone audio not yet judged — see [`LOOKAHEAD`].
    waiting: VecDeque<(i64, Vec<f32>)>,
    /// The microphone's loudness by step, when it was plainly not you, for the
    /// delay search.
    heard: Ring,
    /// How many steps behind the system stream the echo arrives.
    delay: i64,
    /// How far below the system stream the echo sits, in dB — negative, and
    /// learned.
    loss: f32,
    /// The loudest the system has been lately. The loss is learned only near
    /// it, where an echo stands clear of the microphone's own hiss.
    peak: f32,
    /// Whether the microphone hears the speakers. Assumed until the delay
    /// search shows otherwise; once it does, the gate stands aside.
    echoing: bool,
    unmatched: usize,
    matched: f64,
    open_until: i64,
    since_search: usize,
    judged: usize,
    silenced: usize,
}

impl Gate {
    pub fn new(playing: Arc<Playing>) -> Self {
        Self {
            playing,
            waiting: VecDeque::new(),
            heard: Ring::new(),
            delay: 3,
            // Starts at the top and walks down to the real level within a
            // couple of seconds of the other side talking. Too high only means
            // your voice has to be loud to get through over them for those
            // seconds; too low was the old bug.
            loss: 0.0,
            peak: -120.0,
            echoing: true,
            unmatched: 0,
            matched: 0.0,
            open_until: i64::MIN,
            since_search: 0,
            judged: 0,
            silenced: 0,
        }
    }

    /// Takes a block of microphone audio that finished arriving at `now`, and
    /// returns the audio that is ready — [`LOOKAHEAD`] behind, with whatever is
    /// only the speakers coming back replaced by silence.
    pub fn filter(&mut self, samples: &[f32], now: Instant) -> Vec<f32> {
        let playing = Arc::clone(&self.playing);
        let end = playing.step(now);
        for (range, step) in steps(samples.len(), end) {
            self.waiting.push_back((step, samples[range].to_vec()));
        }

        let ring = playing.ring.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out = Vec::with_capacity(samples.len());
        while let Some((step, _)) = self.waiting.front() {
            if *step > end - LOOKAHEAD {
                break;
            }
            let (step, mut part) = self.waiting.pop_front().expect("front was just seen");
            if !self.judge(&ring, step, rms_db(&part)) {
                part.fill(0.0);
            }
            out.extend_from_slice(&part);
            self.since_search += 1;
        }
        if self.since_search >= 50 {
            self.since_search = 0;
            self.search(&ring, end - LOOKAHEAD);
        }
        out
    }

    /// Whatever is still waiting, for when the recording stops — judged
    /// against what is known of the system by then.
    pub fn flush(&mut self) -> Vec<f32> {
        let playing = Arc::clone(&self.playing);
        let ring = playing.ring.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out = Vec::new();
        while let Some((step, mut part)) = self.waiting.pop_front() {
            if !self.judge(&ring, step, rms_db(&part)) {
                part.fill(0.0);
            }
            out.extend_from_slice(&part);
        }
        out
    }

    fn judge(&mut self, ring: &Ring, step: i64, mic: f32) -> bool {
        let at = step - self.delay;
        // What the echo is lined up with, give or take the slack...
        let lined = (at - SLACK..=at + SLACK).map(|s| ring.level(s)).fold(-120.0f32, f32::max);
        // ...and what could still be ringing from before it.
        let ringing = (1..=TAIL_STEPS)
            .map(|k| ring.level(at - SLACK - k) - TAIL * k as f32)
            .fold(lined, f32::max);
        self.peak = lined.max(self.peak - 0.05);
        if ringing < PLAYING {
            return true;
        }

        // A low quartile rather than a median: you talking over them lifts
        // what is heard, and a quartile stays with the echo as long as you are
        // not talking through most of what they say. A muted microphone says
        // nothing about the echo at all.
        if lined > PLAYING && lined > self.peak - 15.0 && mic > -100.0 {
            self.loss += if mic - lined > self.loss { 0.1 } else { -0.3 };
        }

        let echo = ringing + self.loss;
        let clear = mic > echo + MARGIN;
        if clear {
            self.open_until = step + HOLD;
        } else if mic > -100.0 {
            // Only what is plainly not you goes into the delay search: your
            // own voice follows nothing the system plays.
            self.heard.put(step, mic);
        }
        let pass = !self.echoing || clear || (step < self.open_until && mic > echo + STILL_YOU);
        self.judged += 1;
        if !pass {
            self.silenced += 1;
        }
        pass
    }

    /// Finds the delay at which the microphone's loudness follows the system's
    /// most closely over the last few seconds, and whether it follows at all.
    fn search(&mut self, ring: &Ring, end: i64) {
        let from = end - (REMEMBER as i64 - MAX_DELAY - SLACK - LOOKAHEAD);
        let mut best = (f64::MIN, self.delay);
        for delay in -(LOOKAHEAD - SLACK)..=MAX_DELAY {
            let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0);
            for step in from..=end {
                let Some(y) = self.heard.get(step) else { continue };
                let x = ring.level(step - delay);
                if x < PLAYING {
                    continue;
                }
                let (x, y) = (x as f64, y as f64);
                n += 1.0;
                sx += x;
                sy += y;
                sxx += x * x;
                syy += y * y;
                sxy += x * y;
            }
            // A second of the other side talking, at least, or there is
            // nothing to go on yet.
            if n < 100.0 {
                return;
            }
            let cov = sxy / n - (sx / n) * (sy / n);
            let var = (sxx / n - (sx / n).powi(2)) * (syy / n - (sy / n).powi(2));
            let r = cov / var.max(1e-9).sqrt();
            if r > best.0 {
                best = (r, delay);
            }
        }
        self.matched = best.0;
        if best.0 > MATCHED {
            self.delay = best.1;
            self.echoing = true;
            self.unmatched = 0;
        } else if best.0 < UNMATCHED {
            self.unmatched += 1;
            if self.unmatched >= GIVE_UP {
                self.echoing = false;
            }
        }
    }

    /// What the gate has done since the last call. `None` when nothing was
    /// playing in between.
    pub fn report(&mut self) -> Option<Report> {
        if self.judged == 0 {
            return None;
        }
        let report = Report {
            silenced: self.silenced as f32 / self.judged as f32,
            delay_ms: self.delay * STEP.as_millis() as i64,
            loss_db: self.loss,
            echoing: self.echoing,
            matched: self.matched,
        };
        self.judged = 0;
        self.silenced = 0;
        Some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: usize = 16_000;

    /// Something with the rhythm of speech: bursts of noise a syllable long,
    /// at uneven loudness, with pauses between — a new pattern for each seed.
    fn speech(seconds: f32, level_db: f32, seed: u64) -> Vec<f32> {
        let mut state = seed;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 40) as f32 / (1u64 << 24) as f32
        };
        let amp = 10f32.powf(level_db / 20.0) * 1.7;
        let mut out = Vec::new();
        while out.len() < (seconds * RATE as f32) as usize {
            let len = RATE / 20 + (next() * (RATE / 5) as f32) as usize;
            let gain = if next() < 0.25 { 0.0 } else { 0.2 + next() };
            out.extend((0..len).map(|_| (next() * 2.0 - 1.0) * amp * gain));
        }
        out.truncate((seconds * RATE as f32) as usize);
        out
    }

    /// A microphone's own hiss.
    fn hiss(seconds: f32, level_db: f32) -> Vec<f32> {
        let mut state = 77u64;
        let amp = 10f32.powf(level_db / 20.0) * 1.7;
        (0..(seconds * RATE as f32) as usize)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((state >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn delayed(signal: &[f32], ms: usize, db: f32) -> Vec<f32> {
        let shift = ms * RATE / 1000;
        let gain = 10f32.powf(db / 20.0);
        (0..signal.len()).map(|i| if i < shift { 0.0 } else { signal[i - shift] * gain }).collect()
    }

    fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    /// Plays `system` and `mic` through a source and a gate in 10.7 ms blocks,
    /// as a 48 kHz device with 512-frame buffers delivers them, and returns
    /// what the gate let out, lined up with the input.
    fn run(system: &[f32], mic: &[f32]) -> (Vec<f32>, Gate) {
        run_late(system, mic, Duration::ZERO)
    }

    /// As [`run`], with the system stream reaching the gate `late`.
    fn run_late(system: &[f32], mic: &[f32], late: Duration) -> (Vec<f32>, Gate) {
        let playing = Playing::new();
        let mut source = Source::new(playing.clone());
        let mut gate = Gate::new(playing);
        let start = Instant::now() + Duration::from_millis(1);
        let mut out = Vec::new();
        let (mut at, mut k) = (0, 0);
        while at < mic.len() {
            k += 1;
            let to = (k * 512 / 3).min(mic.len());
            let now = start + Duration::from_secs_f64(to as f64 / RATE as f64);
            source.heard(&system[at..to], now + late);
            out.extend(gate.filter(&mic[at..to], now + Duration::from_millis(3)));
            at = to;
        }
        out.extend(gate.flush());
        (out, gate)
    }

    /// The share of the steps from `from` seconds on, where `wanted` is
    /// loud, that the gate let through.
    fn passed(out: &[f32], wanted: &[f32], from: f32) -> f32 {
        let (mut loud, mut through) = (0, 0);
        for (o, w) in out.chunks(STEP_SAMPLES).zip(wanted.chunks(STEP_SAMPLES)).skip((from * 100.0) as usize) {
            if rms_db(w) > -60.0 {
                loud += 1;
                through += o.iter().any(|s| *s != 0.0) as usize;
            }
        }
        through as f32 / loud.max(1) as f32
    }

    #[test]
    fn the_echo_is_held_back_and_its_delay_found() {
        let them = speech(20.0, -20.0, 1);
        let echo = delayed(&them, 60, -30.0);
        let (out, mut gate) = run(&them, &add(&echo, &hiss(20.0, -70.0)));
        assert!(passed(&out, &echo, 5.0) < 0.03, "echo got through: {}", passed(&out, &echo, 5.0));
        let report = gate.report().unwrap();
        assert!((50..=70).contains(&report.delay_ms), "delay {}", report.delay_ms);
        assert!((report.loss_db + 30.0).abs() < 4.0, "loss {}", report.loss_db);
    }

    /// What happened on a real call: the system stream arrived well after its
    /// own echo, and the gate could not look far enough ahead to line them up.
    #[test]
    fn an_echo_that_arrives_before_its_source_is_still_held_back() {
        let them = speech(20.0, -20.0, 1);
        let echo = delayed(&them, 20, -25.0);
        let (out, mut gate) = run_late(&them, &add(&echo, &hiss(20.0, -70.0)), Duration::from_millis(250));
        assert!(passed(&out, &echo, 5.0) < 0.03, "echo got through: {}", passed(&out, &echo, 5.0));
        let report = gate.report().unwrap();
        assert!(report.echoing);
        assert!((-240..=-220).contains(&report.delay_ms), "delay {}", report.delay_ms);
    }

    #[test]
    fn a_voice_over_the_echo_gets_through() {
        let them = speech(20.0, -20.0, 1);
        let you = speech(20.0, -24.0, 2);
        let you = you.iter().enumerate().map(|(i, s)| if i < 10 * RATE { 0.0 } else { *s }).collect::<Vec<_>>();
        let (out, _) = run(&them, &add(&delayed(&them, 60, -30.0), &you));
        assert!(passed(&out, &you, 10.0) > 0.9, "voice cut: {}", passed(&out, &you, 10.0));
    }

    #[test]
    fn nothing_playing_leaves_the_microphone_alone() {
        let you = speech(5.0, -30.0, 2);
        let (out, _) = run(&vec![0.0; you.len()], &you);
        assert_eq!(out, you);
    }

    #[test]
    fn a_microphone_that_hears_no_echo_is_left_alone() {
        let them = speech(30.0, -20.0, 1);
        let you = add(&speech(30.0, -35.0, 2), &hiss(30.0, -70.0));
        let (out, mut gate) = run(&them, &you);
        assert!(!gate.report().unwrap().echoing);
        assert!(passed(&out, &you, 20.0) > 0.99, "voice cut: {}", passed(&out, &you, 20.0));
    }

    #[test]
    fn the_output_is_the_input_length() {
        let them = speech(3.0, -20.0, 1);
        let (out, _) = run(&them, &delayed(&them, 30, -20.0));
        assert_eq!(out.len(), them.len());
    }
}
