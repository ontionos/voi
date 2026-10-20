//! Shared state between the AppKit main thread and the recording worker.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use std::collections::VecDeque;

use voi_audio::Origin;
use voi_core::Stopper;

/// How many settled lines the panel is given to show.
///
/// Enough to scroll back through a good few screens of what was just said, and
/// little enough that building them costs nothing. The transcript on disk is
/// complete regardless — that is what it is for.
const VIEW_LINES: usize = 200;

/// Upper bound on retained lines. Deliberately huge: the panel scrolls, and a
/// long call must not silently lose its beginning. At roughly 80 bytes a line
/// this is a few megabytes — nothing next to the audio.
const HISTORY: usize = 50_000;

/// Bars either side of the centre in the expanded meter, so there are
/// `SPREAD * 2 + 1` of them in all.
///
/// Many and slim. The row has to read as one continuous surface with a shape
/// travelling across it; at a dozen bars it reads as a dozen objects, and the
/// travel becomes a few steps rather than a movement.
pub const SPREAD: usize = 60;

/// History samples between one ring and the next.
///
/// One, so doubling the number of bars does not also double how long the wave
/// takes to cross them: the row still holds half a second.
const PER_RING: usize = 1;
/// How often the level is written to the history, in hertz.
///
/// A fixed rate, deliberately: the delay to the ends is a number of samples, so
/// pinning the sample rate is what keeps the wave travelling at the same speed
/// whatever the display is doing.
const HISTORY_HZ: f32 = 120.0;
/// A level meter that radiates from its centre.
///
/// The bars do not each track the signal. The middle one does, and every other
/// bar shows what the middle one showed a moment ago — one ring further out is
/// one moment further back. So a peak appears in the centre and *travels* to
/// both ends, symmetrically, while the bars themselves stay where they are: the
/// row is the last half-second of the conversation, mirrored.
///
/// This is a delay line rather than a chain of followers. A chain smooths as it
/// passes the level along, and by the ends there is nothing left to see but a
/// slow swell; a delay carries the shape intact, so quiet moments arrive at the
/// ends as gaps and loud ones as peaks.
///
/// One meter serves both sizes: the collapsed widget is three of these bars,
/// the expanded panel is all of them.
#[derive(Debug, Clone)]
pub struct Meter {
    /// The signal itself, with a quick attack and a slower decay — which is
    /// what reads as responsive rather than jittery.
    centre: f32,
    /// What the centre held at each past sample, newest first.
    history: VecDeque<f32>,
    /// Time run up since the last history sample was taken.
    owed: f32,
    /// What counts as loud at the moment: the loudest the input has been in
    /// the last few seconds, and never less than a floor.
    reference: f32,
    /// 0.0–1.0 ramp for the record button's glow, one cycle per ~2.4s.
    pub pulse: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            centre: 0.0,
            history: std::iter::repeat_n(0.0, SPREAD * PER_RING + 1).collect(),
            owed: 0.0,
            reference: 0.0,
            pulse: 0.0,
        }
    }
}

/// Seconds for one breath of the record dot's halo.
const PULSE_PERIOD: f32 = 3.6;


impl Meter {
    /// Advance the meter by `dt` seconds toward `target` (0.0–1.0).
    ///
    /// Rates are time constants, not per-frame fractions, so the animation
    /// looks identical whatever the refresh rate. A per-frame fraction tuned at
    /// 20 fps runs three times too fast at 60.
    pub fn tick(&mut self, target: f32, dt: f32) {
        /// Seconds to close most of the gap while rising, and while falling.
        /// The fall is short: at a quarter of a second it bridges the gaps
        /// between syllables, and continuous speech holds the row at the top.
        const ATTACK_TAU: f32 = 0.055;
        const DECAY_TAU: f32 = 0.09;
        /// How quickly the reference forgets a loud passage.
        const RANGE_TAU: f32 = 3.0;
        /// The quietest the reference is allowed to get, so a silent room is
        /// not amplified into a performance.
        const FLOOR: f32 = 0.34;
        /// How much the difference between a syllable and the gap after it is
        /// stretched. Above one, quiet moments fall further than loud ones.
        const CONTRAST: f32 = 1.7;
        /// Below this the input is a room rather than a voice.
        const GATE: f32 = 0.26;
        /// How much of what is under the gate still shows.
        const LEAK: f32 = 0.11;

        // Silence has to read as silence. Auto-ranging divides by what counts
        // as loud, so without a gate the hum of a quiet room is scaled up into
        // a performance — the meter ends up showing the fridge.
        let raw = target.clamp(0.0, 1.0);
        let voiced = ((raw - GATE) / (1.0 - GATE)).max(0.0);

        // Auto-ranging. What a meter has to show is how loud this moment is
        // against the last few seconds — not against full scale, which anyone
        // talking normally sits near the top of the whole time. The reference
        // rises instantly to a new peak and forgets it slowly, so the row keeps
        // its shape whether you are speaking up or muttering.
        if voiced > self.reference {
            self.reference = voiced;
        } else {
            let settle = voiced.max(FLOOR);
            self.reference += (settle - self.reference) * (1.0 - (-dt / RANGE_TAU).exp());
        }
        let shaped = (voiced / self.reference.max(FLOOR)).clamp(0.0, 1.0).powf(CONTRAST);

        // And a floor under all of it, from the ungated signal: a snap of the
        // fingers or a quiet room still stirs the row a little. Applied here
        // rather than before the gate, where the ranging and the contrast
        // curve between them crushed it back to nothing — which is why a
        // supposedly leaky gate still looked shut.
        let want = shaped.max(LEAK * (raw / GATE).clamp(0.0, 1.0));

        let tau = if want > self.centre { ATTACK_TAU } else { DECAY_TAU };
        self.centre += (want - self.centre) * (1.0 - (-dt / tau).exp());

        // Bounded by the length of the history: after a pause — the app asleep,
        // a menu held open — there is no point replaying more of it than the
        // row can show.
        let step = 1.0 / HISTORY_HZ;
        self.owed += dt;
        let mut taken = 0;
        while self.owed >= step && taken < self.history.len() {
            self.owed -= step;
            taken += 1;
            self.history.push_front(self.centre);
            self.history.pop_back();
        }
        if self.owed >= step {
            self.owed = 0.0;
        }
    }

    /// The full row, left end to right end, with the centre in the middle and
    /// a floor so the meter never looks dead.
    ///
    /// Levels only. How the ends of the row fade out is the drawing's business:
    /// it depends on how many bars are actually on screen, and there are far
    /// fewer of them when the panel is collapsed.
    pub fn wave(&self) -> Vec<f32> {
        (0..=SPREAD * 2)
            .map(|i| {
                let ring = i.abs_diff(SPREAD);
                // Barely a floor. Silence has to look like silence — the row
                // is still there, but as a line, not as a level.
                self.history[ring * PER_RING].max(0.004)
            })
            .collect()
    }

    /// Advance the decorative phases by `dt` seconds. Separate from
    /// [`tick`](Self::tick) because they run whether or not audio is arriving.
    pub fn tick_phases(&mut self, dt: f32) {
        self.pulse = (self.pulse + dt / PULSE_PERIOD) % 1.0;
    }

    /// The level right now, and what the ends of the row are showing.
    ///
    /// Enough to tell whether the meter has moved since the last frame: the
    /// centre covers everything arriving, the tail everything on its way out.
    pub fn probe(&self) -> (f32, f32) {
        (self.centre, *self.history.back().unwrap_or(&0.0))
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    #[default]
    Idle,
    Recording,
    /// Stop requested; Soniox is still flushing its final tokens.
    Stopping,
}

#[derive(Default)]
pub struct State {
    /// A newer build out, with this system's file (`crate::updates`).
    pub update: Option<voi_core::update::Update>,
    pub mode: Mode,
    /// Set the moment recording is requested, so elapsed time is honest even
    /// before the worker has finished opening devices.
    pub started_at: Option<Instant>,
    pub segments: usize,
    pub session_dir: Option<PathBuf>,
    pub last_line: String,
    /// Most recent problem worth showing in the menu, if any.
    pub error: Option<String>,
    pub stopper: Option<Stopper>,

    /// Latest loudness of each side, 0.0–1.0: the microphone and the system's
    /// output. The meter shows the louder of the two — it is a meter for the
    /// conversation, not for you, and a widget that sits still while the other
    /// person is talking looks like one that has stopped working.
    pub level_mic: f32,
    pub level_system: f32,
    pub meter: Meter,
    /// Finalized lines with the side that said them, oldest first.
    pub lines: VecDeque<(Origin, String)>,
    /// Finalized words of the segment still being built, per side. Settled —
    /// they render like any other finished text.
    pub open_mic: String,
    pub open_system: String,
    /// When each side began the line it is on, on the audio's clock, for
    /// ordering the two live lines against each other.
    pub since_mic: Option<u64>,
    pub since_system: Option<u64>,
    /// The part Soniox may still rewrite, per side.
    pub tail_mic: String,
    pub tail_system: String,
    /// Whether the overlay shows the transcript or just the meter.
    pub expanded: bool,
    /// Where the panel currently is between collapsed (0.0) and expanded (1.0).
    /// Written by the overlay each frame; the drawing code reads it so it can
    /// paint the in-between sizes.
    pub open: f64,
    /// Size of the record dot relative to its resting size: grows a little
    /// under the pointer, dips when pressed.
    pub dot_scale: f64,
    /// How far the controls have travelled from idle to recording, 0.0–1.0.
    ///
    /// A separate value from [`Self::mode`] because colour has to catch up
    /// smoothly. Switching straight to red the instant recording starts is the
    /// one thing that makes the whole widget feel cheap.
    pub tint: f64,
    /// How far the "Record?" prompt has faded in, 0.0–1.0. Eased, so it
    /// appears when another app takes the microphone rather than snapping into
    /// place the instant a poll notices.
    pub prompt: f64,
    /// How far the widget has arrived on screen, 0.0–1.0. It grows and fades
    /// into place rather than appearing whole.
    pub appear: f64,
    /// How far the copy control has turned into its confirmation, 0.0–1.0.
    /// Eased, so the clipboard goes out and the tick comes in as a fade rather
    /// than a switch.
    pub copy_flash: f64,
    /// How much larger the copy control is than at rest — under the pointer it
    /// grows a little, and dips when pressed. Zero at rest, so the default is
    /// the resting size.
    pub copy_grow: f64,
    /// Kept off screen because you asked for it. Outranks everything else —
    /// pinning it, a call starting, a recording running. Recording still works
    /// and the menu bar still says so; the widget simply is not there.
    pub hidden: bool,
    /// Kept on screen because you asked for it, rather than because a call is
    /// happening.
    pub overlay_pinned: bool,
    /// Some other app is taking microphone input, so a conversation is likely
    /// starting.
    pub mic_in_use: bool,
    /// What decides [`Self::mic_in_use`]: knows when a dictation of ours is
    /// running, so the system picking the microphone back up after one is not
    /// taken for a call starting.
    pub mic_watch: voi_core::mic_watch::MicWatch,
    /// What a file transcription is doing right now, if one is running. Just
    /// the words — the spinner beside them is the view's business.
    pub job: Option<String>,
    /// The last transcript written, so it can be opened without hunting for it.
    pub last_transcript: Option<std::path::PathBuf>,
    /// When the transcript was last copied, for the confirmation flash.
    pub copied_at: Option<Instant>,
    /// Bumped whenever the transcript changes. The panel rebuilds its text
    /// only when this moves, rather than on every one of sixty frames a
    /// second — most of which arrive with nothing new to show.
    pub revision: u64,
}

impl State {
    pub fn push_line(&mut self, origin: Origin, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.revision += 1;
        self.lines.push_back((origin, text));
        while self.lines.len() > HISTORY {
            self.lines.pop_front();
        }
    }

    pub fn set_pending(
        &mut self,
        origin: Origin,
        settled: String,
        tail: String,
        start_ms: Option<u64>,
    ) {
        // When this side began the line it is on now, on the audio's own clock.
        // The two live lines are shown in that order — which is the order they
        // were said in, not the order the two connections happened to deliver.
        match origin {
            Origin::Mic => self.since_mic = start_ms,
            Origin::System => self.since_system = start_ms,
        }
        let changed = match origin {
            Origin::Mic => self.open_mic != settled || self.tail_mic != tail,
            Origin::System => self.open_system != settled || self.tail_system != tail,
        };
        if changed {
            self.revision += 1;
        }
        match origin {
            Origin::Mic => {
                self.open_mic = settled;
                self.tail_mic = tail;
            }
            Origin::System => {
                self.open_system = settled;
                self.tail_system = tail;
            }
        }
    }

    pub fn clear_pending(&mut self) {
        self.revision += 1;
        self.since_mic = None;
        self.since_system = None;
        self.open_mic.clear();
        self.open_system.clear();
        self.tail_mic.clear();
        self.tail_system.clear();
    }

    /// The transcript as a sequence of runs, each flagged live or settled.
    ///
    /// Runs rather than two strings because a single line mixes both: the words
    /// Soniox has finalized are settled even while the rest of the same
    /// sentence is still being revised. Splitting by colour instead of by line
    /// is what keeps a sentence on one line.
    ///
    /// Every line is prefixed with the speaker rather than only the ones where
    /// it changes: the panel scrolls, so any line can be the first one visible.
    pub fn display_runs(&self) -> Vec<(String, bool)> {
        self.runs(VIEW_LINES)
    }

    /// The last `keep` settled lines, then whatever is still being said.
    ///
    /// The panel asks for a screenful; the copy button asks for everything.
    /// Formatting every line was the panel's own habit and it is called for
    /// each frame — five hours into a call that is a hundred and sixty
    /// kilobytes of strings built sixty times a second, and a profile put
    /// eighty per cent of the frame in here. Long before that the frame stops
    /// fitting in its sixteen milliseconds, the widget goes still and stops
    /// answering, and from the outside that is an app that died.
    fn runs(&self, keep: usize) -> Vec<(String, bool)> {
        let mut runs: Vec<(String, bool)> = Vec::new();
        let from = self.lines.len().saturating_sub(keep);
        for (origin, text) in self.lines.iter().skip(from) {
            runs.push((format!("{}: {text}\n", origin.label()), false));
        }

        // Whichever line was begun first goes above the other, so the live
        // text reads in the same order as everything settled above it. Pinning
        // your own line to the bottom regardless — which is what this did —
        // means that when the other side started talking first, their line
        // grows *above* yours: text arriving in the middle of the page.
        let mut live = [
            (Origin::System, &self.open_system, &self.tail_system, self.since_system),
            (Origin::Mic, &self.open_mic, &self.tail_mic, self.since_mic),
        ];
        live.sort_by_key(|(_, _, _, since)| *since);

        for (origin, open, tail, _) in live {
            if open.trim().is_empty() && tail.trim().is_empty() {
                continue;
            }
            // Trim exactly as a closed segment is trimmed. Soniox tokens carry
            // a leading space, so without this the live line reads "You:  text"
            // and the settled one "You: text" — a one character shift that
            // re-wraps the paragraph the moment a phrase finishes, which looks
            // like the whole transcript jumping.
            let (open, tail) = if open.trim().is_empty() {
                ("", tail.trim_start())
            } else {
                (open.trim_start(), tail.as_str())
            };

            runs.push((format!("{}: ", origin.label()), false));
            if !open.is_empty() {
                runs.push((open.to_string(), false));
            }
            if !tail.is_empty() {
                runs.push((tail.to_string(), true));
            }
            runs.push(("\n".to_string(), false));
        }

        // No newline on the end. An empty last line is still a line as far as
        // layout is concerned: it makes the text a line taller than it looks,
        // and scrolling to the bottom then leaves the last line you can read
        // sitting a whole line short of the bottom of the panel.
        while runs.last().is_some_and(|(text, _)| text.ends_with('\n')) {
            let Some((text, _)) = runs.last_mut() else { break };
            text.pop();
            if text.is_empty() {
                runs.pop();
            }
        }
        runs
    }

    /// Everything, for the Copy button — the whole call, not the part of it
    /// the panel happens to be showing. Built on demand, never per frame.
    pub fn transcript_text(&self) -> String {
        let runs = self.runs(usize::MAX);
        if runs.is_empty() {
            return if self.is_active() {
                "Listening…".to_string()
            } else {
                "Not recording".to_string()
            };
        }
        runs.into_iter().map(|(text, _)| text).collect::<String>().trim_end().to_string()
    }

    /// How strongly to show the copy confirmation, 1.0 fading to 0.0.
    pub fn copied_strength(&self) -> f64 {
        const HOLD: f64 = 1.1;
        const FADE: f64 = 0.6;
        let Some(at) = self.copied_at else { return 0.0 };
        let age = at.elapsed().as_secs_f64();
        if age < HOLD {
            1.0
        } else {
            (1.0 - (age - HOLD) / FADE).clamp(0.0, 1.0)
        }
    }

    /// Whether the overlay belongs on screen.
    ///
    /// It hides by default and appears when something starts listening, so it
    /// is not permanent furniture — but a recording in progress always shows,
    /// and pinning it from the menu overrides everything.
    /// Whether to ask whether this should be recorded.
    ///
    /// Something else has the microphone — a call has started — and nothing is
    /// being recorded yet. Not while the panel is open: the pointer is already
    /// on the record dot, so the question answers itself.
    pub fn wants_prompt(&self) -> bool {
        self.mic_in_use && matches!(self.mode, Mode::Idle) && !self.expanded
    }

    pub fn overlay_visible(&self) -> bool {
        !self.hidden && (self.overlay_pinned || self.mic_in_use || self.is_active())
    }

    pub fn is_active(&self) -> bool {
        self.mode != Mode::Idle
    }

    /// One-line summary for the menu header.
    ///
    /// Whether it is recording is the only thing worth saying here. A segment
    /// count answers a question nobody asked, and a clock could only tick once
    /// a second, which read as a stutter.
    pub fn summary(&self) -> String {
        // A file being transcribed takes precedence: it is the thing that is
        // taking time and the thing worth reporting.
        if let Some(job) = &self.job {
            return job.clone();
        }
        match self.mode {
            Mode::Idle => "Idle".to_string(),
            Mode::Stopping => "Stopping — saving transcript…".to_string(),
            Mode::Recording => "Recording".to_string(),
        }
    }

    /// What the toggle item should say right now.
    pub fn toggle_title(&self) -> &'static str {
        match self.mode {
            Mode::Idle => "Start Recording",
            Mode::Recording => "Stop Recording",
            Mode::Stopping => "Stopping…",
        }
    }

    /// Text beside the status-bar icon.
    ///
    /// Always empty: the overlay carries the elapsed time, and a clock ticking
    /// in the menu bar only makes the item wider and says nothing new.
    pub fn clock_title(&self) -> String {
        String::new()
    }

    /// How solid the status-bar dot should be.
    ///
    /// Always a dot, never a glyph that changes shape. A warning triangle in
    /// place of the icon made the menu bar jump about and said less than the
    /// overlay already does.
    pub fn dot_alpha(&self) -> f64 {
        match self.mode {
            // Solid throughout. The colour says which state it is in — white
            // waiting, red recording — so dimming it as well only makes the
            // menu-bar icon look switched off.
            Mode::Idle => 1.0,
            Mode::Stopping => 0.85,
            Mode::Recording => 1.0,
        }
    }
}

pub type Shared = Arc<Mutex<State>>;

/// The states both apps draw for the side-by-side comparison
/// (`--screenshots`), in order, with the same words and the same number of
/// frames, so the two pictures of each can be laid over each other.
pub mod parity {
    use super::{Mode, Shared};
    use voi_audio::Origin;

    /// Frames each state is run for before it is drawn: two seconds at 60.
    pub const FRAMES: u32 = 120;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Shot {
        HudIdle,
        HudPrompt,
        HudRecording,
        HudOpen,
        DictationListening,
        DictationCancel,
    }

    pub const SHOTS: [(Shot, &str); 6] = [
        (Shot::HudIdle, "hud-idle"),
        (Shot::HudPrompt, "hud-prompt"),
        (Shot::HudRecording, "hud-recording"),
        (Shot::HudOpen, "hud-open"),
        (Shot::DictationListening, "dictation-listening"),
        (Shot::DictationCancel, "dictation-cancel"),
    ];

    /// The dictation's words: one line settled, the rest still arriving.
    /// A voice's loudness at `frame` (of 60 a second): syllables a few frames
    /// long, louder words, and the short gaps between them — what the meter
    /// shows with someone speaking. Fixed, so the pictures are too.
    pub fn speech(frame: u32) -> f32 {
        let t = frame as f32 / 60.0;
        // Fixed noise: the same every run.
        let grain = ((t * 91.7).sin() * 43_758.5).fract().abs();
        if (t * 1.3).fract() > 0.8 {
            // A breath between phrases.
            return 0.2;
        }
        let words = (t * std::f32::consts::TAU * 1.9).sin().abs();
        (0.5 + 0.45 * (0.6 * words + 0.4 * grain)).clamp(0.0, 1.0)
    }

    /// What people dictate.
    pub const DICTATED: (&str, &str) = ("Remind me tomorrow to send the contract, ", "and book a room for Friday at ten.");

    /// Put `shared` in the state a shot shows.
    pub fn stage(shared: &Shared, shot: Shot) {
        let mut s = shared.lock().unwrap();
        *s = Default::default();
        s.overlay_pinned = true;
        match shot {
            Shot::HudIdle => {}
            Shot::HudPrompt => s.mic_in_use = true,
            Shot::HudRecording | Shot::HudOpen => {
                s.mode = Mode::Recording;
                s.level_mic = 0.55;
                s.push_line(Origin::Mic, "Hi! Can you see my screen?".into());
                s.push_line(Origin::System, "Yes, I can see it. Let's go through the numbers.".into());
                s.push_line(Origin::Mic, "Great, let's start with the October budget.".into());
                s.set_pending(Origin::System, "Sounds good, and then".into(), " the release plan".into(), Some(9_000));
            }
            Shot::DictationListening | Shot::DictationCancel => s.overlay_pinned = false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row is a mirror about its middle. Anything else is a waveform
    /// travelling in one direction, which is the look being avoided.
    #[test]
    fn the_wave_is_symmetric_about_its_centre() {
        let mut m = Meter::default();
        for _ in 0..12 {
            m.tick(1.0, 1.0 / 60.0);
        }
        let wave = m.wave();
        assert_eq!(wave.len(), SPREAD * 2 + 1, "centre plus a ring either side");
        for i in 0..=SPREAD {
            assert_eq!(wave[i], wave[wave.len() - 1 - i], "bar {i} is not mirrored");
        }
        assert!(wave.iter().all(|v| *v > 0.0), "a floor keeps the meter visible");
    }

    /// A peak starts in the middle and arrives at the ends later. Without that
    /// lag the row is one bar drawn sixty times.
    #[test]
    fn a_peak_travels_outward_from_the_centre() {
        let mut m = Meter::default();
        // Rising: the centre is lit and the ends still hold the silence.
        for _ in 0..10 {
            m.tick(1.0, 1.0 / 60.0);
        }
        let rising = m.wave();
        assert!(rising[SPREAD] > rising[0] + 0.5, "the centre must lead: {rising:?}");
        for i in 0..SPREAD {
            assert!(
                rising[i] <= rising[i + 1] + 1e-6,
                "level must fall off toward the ends: {rising:?}"
            );
        }

        // Long enough for it to arrive at the ends.
        for _ in 0..60 {
            m.tick(1.0, 1.0 / 60.0);
        }
        let full = m.wave();
        // Measured half-way out, where the row is still at full height: the
        // last bars are deliberately damped, so they are no test of arrival.
        let out = SPREAD / 2;
        assert!(full[out] > 0.6, "the peak should have travelled: {full:?}");

        // Cut the input: the centre drops first, and what the bars further out
        // show is still the loud passage on its way out.
        for _ in 0..12 {
            m.tick(0.0, 1.0 / 60.0);
        }
        let after = m.wave();
        assert!(after[SPREAD] < full[SPREAD] - 0.1, "the centre must fall first");
        assert!(after[out] > after[SPREAD], "the rest still holds what the centre had");
    }

    /// The wave must cross the row in the same time whatever the frame rate:
    /// the delay is counted in history samples, and those are taken on a clock
    /// of their own.
    #[test]
    fn the_wave_travels_at_the_same_speed_at_any_frame_rate() {
        let mut slow = Meter::default();
        let mut fast = Meter::default();
        for _ in 0..15 {
            slow.tick(1.0, 1.0 / 30.0);
        }
        for _ in 0..30 {
            fast.tick(1.0, 1.0 / 60.0);
        }
        // Half a second of input in both cases, so the front of the wave has
        // travelled the same distance along the row. Its position is what has
        // to match; the exact heights cannot, since a slower loop samples the
        // rise itself more coarsely.
        //
        // The tolerance is one frame of the slower loop: it takes its samples
        // in coarser steps, so its front can be up to that far along. A frame
        // at 30fps is four samples, and a sample either side of the centre is
        // two bars.
        let front = |wave: Vec<f32>| wave.iter().filter(|v| **v > 0.5).count() as i32;
        let (slow, fast) = (front(slow.wave()), front(fast.wave()));
        let slack = 2 * (HISTORY_HZ / 30.0) as i32 / PER_RING as i32;
        assert!(
            (slow - fast).abs() <= slack,
            "the wave front is in a different place: {slow} bars lit vs {fast}"
        );
    }

    #[test]
    fn meter_rises_with_input_and_falls_in_silence() {
        let mut m = Meter::default();
        for _ in 0..20 {
            m.tick(1.0, 1.0 / 60.0);
        }
        let loud = m.wave();
        assert!(loud[SPREAD] > 0.8, "the centre should reach near full on loud input");
        assert!(loud[SPREAD] > loud[0], "the centre must lead, so motion spreads outward");

        for _ in 0..120 {
            m.tick(0.0, 1.0 / 60.0);
        }
        let quiet = m.wave();
        assert!(quiet[SPREAD] < 0.1, "the meter must fall back in silence");
        assert!(quiet.iter().all(|b| *b > 0.0), "the row keeps a visible floor");
    }

    #[test]
    fn decorative_phases_advance_and_wrap() {
        let mut m = Meter::default();
        for _ in 0..60 {
            m.tick_phases(PULSE_PERIOD / 60.0);
        }
        // One full glow cycle brings it back to the start.
        assert!(m.pulse < 0.001 || m.pulse > 0.999, "pulse should have wrapped");
    }

    /// The highlight must cross the screen at the same rate whatever size the
    /// panel is. Measuring in points instead of perimeter fractions is the
    /// whole reason this field is not a 0–1 ramp.
    /// Animation must not change with the refresh rate. The same elapsed time
    /// in different step sizes has to land in the same place.
    #[test]
    fn animation_is_frame_rate_independent() {
        let mut slow = Meter::default();
        let mut fast = Meter::default();
        for _ in 0..20 {
            slow.tick(1.0, 1.0 / 20.0);
            slow.tick_phases(1.0 / 20.0);
        }
        for _ in 0..60 {
            fast.tick(1.0, 1.0 / 60.0);
            fast.tick_phases(1.0 / 60.0);
        }
        assert!(
            (slow.probe().0 - fast.probe().0).abs() < 0.02,
            "meter differs: {} vs {}",
            slow.probe().0,
            fast.probe().0
        );
        assert!((slow.pulse - fast.pulse).abs() < 1e-4);
    }


    #[test]
    fn the_copy_confirmation_holds_then_fades() {
        let mut s = State::default();
        assert_eq!(s.copied_strength(), 0.0, "nothing to confirm before a copy");
        s.copied_at = Some(Instant::now());
        assert_eq!(s.copied_strength(), 1.0);
        s.copied_at = Some(Instant::now() - std::time::Duration::from_secs(5));
        assert_eq!(s.copied_strength(), 0.0, "it must not linger");
    }

    #[test]
    fn each_line_says_who_spoke() {
        let mut s = State { mode: Mode::Recording, ..Default::default() };
        s.push_line(Origin::Mic, "привіт".into());
        s.push_line(Origin::System, "hello".into());
        s.set_pending(Origin::Mic, String::new(), "ще не фінал".into(), Some(0));
        assert_eq!(
            s.transcript_text(),
            "You: привіт\nThem: hello\nYou: ще не фінал"
        );
    }

    /// Two live lines read in the order they were begun. Pinning your own to
    /// the bottom means the other side's grows above it, which on screen is
    /// text being typed into the middle of the transcript.
    #[test]
    fn the_live_line_begun_first_is_shown_first() {
        let mut s = State { mode: Mode::Recording, ..Default::default() };
        s.set_pending(Origin::System, String::new(), "they started".into(), Some(1_000));
        s.set_pending(Origin::Mic, String::new(), "then me".into(), Some(4_000));
        assert_eq!(s.transcript_text(), "Them: they started\nYou: then me");

        // The other way round, once both lines have ended and begun again.
        let mut s = State { mode: Mode::Recording, ..Default::default() };
        s.set_pending(Origin::Mic, String::new(), "I started".into(), Some(1_000));
        s.set_pending(Origin::System, String::new(), "then them".into(), Some(4_000));
        assert_eq!(s.transcript_text(), "You: I started\nThem: then them");

        // And a line that ends forgets when it began, so the next one is
        // ordered by when *it* began.
        s.set_pending(Origin::Mic, String::new(), String::new(), None);
        assert!(s.since_mic.is_none(), "an empty line has no beginning");
    }

    /// The panel builds a screenful; the copy button takes the whole call.
    /// Rebuilding every line of a long transcript sixty times a second is what
    /// made the widget seize up hours into a call.
    #[test]
    fn the_panel_shows_the_end_and_the_copy_takes_everything() {
        let mut s = State { mode: Mode::Recording, ..Default::default() };
        for i in 0..(VIEW_LINES + 50) {
            s.push_line(Origin::Mic, format!("line {i}"));
        }

        let shown = s.display_runs();
        assert_eq!(shown.len(), VIEW_LINES, "the panel is given a fixed amount");
        assert!(shown[0].0.contains("line 50"), "and it is the end of it");

        let all = s.transcript_text();
        assert!(all.contains("line 0"), "copying takes the whole call");
        assert!(all.ends_with(&format!("line {}", VIEW_LINES + 49)));
    }

    /// Every change to the transcript has to be visible as one, or the panel
    /// keeps showing what it built before.
    #[test]
    fn the_revision_moves_whenever_the_words_do() {
        let mut s = State { mode: Mode::Recording, ..Default::default() };
        let start = s.revision;
        s.push_line(Origin::Mic, "said".into());
        assert!(s.revision > start, "a settled line is a change");

        let at = s.revision;
        s.set_pending(Origin::Mic, String::new(), "saying".into(), Some(0));
        assert!(s.revision > at, "so is a word being said");

        let at = s.revision;
        s.set_pending(Origin::Mic, String::new(), "saying".into(), Some(0));
        assert_eq!(s.revision, at, "the same words twice are not");
    }

    /// Both sides can be mid-sentence at once; neither may hide the other.
    #[test]
    fn a_live_tail_from_each_side_is_shown() {
        let mut s = State { mode: Mode::Recording, ..Default::default() };
        s.set_pending(Origin::Mic, String::new(), "я кажу".into(), Some(1_000));
        s.set_pending(Origin::System, String::new(), "they speak".into(), Some(2_000));
        // In the order the two were begun — see the test above.
        assert_eq!(s.transcript_text(), "You: я кажу\nThem: they speak");
    }

    /// Words Soniox has already finalized must look settled straight away,
    /// even though the segment they belong to is still open. Before this they
    /// stayed dim until a pause happened to close the segment, which could be
    /// a whole phrase later.
    #[test]
    fn finalized_words_are_settled_while_the_segment_is_still_open() {
        let mut s = State { mode: Mode::Recording, ..Default::default() };
        s.push_line(Origin::Mic, "готовий рядок".into());
        s.set_pending(Origin::Mic, "вже фінальне".into(), " ще ні".into(), Some(0));

        let runs = s.display_runs();
        let live: Vec<&str> = runs
            .iter()
            .filter(|(_, live)| *live)
            .map(|(t, _)| t.as_str())
            .collect();
        assert_eq!(live, vec![" ще ні"], "only the unstable part may look live");

        let settled: String = runs
            .iter()
            .filter(|(_, live)| !*live)
            .map(|(t, _)| t.as_str())
            .collect();
        assert!(settled.contains("вже фінальне"));
        assert!(settled.contains("готовий рядок"));
    }

    /// A sentence must not be split across lines just because part of it is
    /// still being revised.
    #[test]
    fn settled_and_live_text_share_one_line() {
        let mut s = State { mode: Mode::Recording, ..Default::default() };
        s.set_pending(Origin::Mic, "початок".into(), " кінець".into(), Some(0));
        assert_eq!(s.transcript_text(), "You: початок кінець");
    }

    /// The live line must read character for character like the settled line it
    /// becomes. Soniox prefixes tokens with a space; leaving it in shifted the
    /// text by one character the instant a phrase closed, re-wrapping the
    /// paragraph and making it look like everything jumped.
    #[test]
    fn a_live_line_matches_the_settled_line_it_becomes() {
        let mut live = State { mode: Mode::Recording, ..Default::default() };
        live.set_pending(Origin::Mic, " Привіт".into(), " світ".into(), Some(0));

        let mut settled = State { mode: Mode::Recording, ..Default::default() };
        settled.push_line(Origin::Mic, "Привіт світ".to_string());

        assert_eq!(live.transcript_text(), settled.transcript_text());
    }

    /// The panel scrolls, so nothing may be dropped from the middle of a call.
    #[test]
    fn history_is_not_truncated() {
        let mut s = State::default();
        for i in 0..5_000 {
            s.push_line(Origin::Mic, format!("line {i}"));
        }
        let text = s.transcript_text();
        assert_eq!(text.lines().count(), 5_000);
        assert!(text.starts_with("You: line 0"), "the start of the call must survive");
    }

    #[test]
    fn blank_lines_are_never_stored() {
        let mut s = State::default();
        s.push_line(Origin::Mic, "   ".into());
        assert!(s.lines.is_empty());
    }

    #[test]
    fn toggle_title_tracks_mode() {
        let mut s = State::default();
        assert_eq!(s.toggle_title(), "Start Recording");
        s.mode = Mode::Recording;
        assert_eq!(s.toggle_title(), "Stop Recording");
        s.mode = Mode::Stopping;
        assert_eq!(s.toggle_title(), "Stopping…");
    }

    /// The bug this guards: the menu said "Start Recording" while a recording
    /// was live, so clicking it again started a second one.
    #[test]
    fn a_live_recording_never_offers_to_start_again() {
        let s = State { mode: Mode::Recording, ..Default::default() };
        assert!(s.is_active());
        assert_ne!(s.toggle_title(), "Start Recording");
    }

    /// The menu bar says one thing: whether it is recording.
    /// Hidden by default; shown when a call starts, while recording, or when
    /// asked for explicitly.
    #[test]
    fn the_overlay_appears_only_when_it_should() {
        let mut s = State::default();
        assert!(!s.overlay_visible(), "nothing is happening, so nothing on screen");

        s.mic_in_use = true;
        assert!(s.overlay_visible(), "another app started listening");
        s.mic_in_use = false;
        assert!(!s.overlay_visible());

        s.mode = Mode::Recording;
        assert!(s.overlay_visible(), "a recording must always be visible");
        s.mode = Mode::Idle;

        s.overlay_pinned = true;
        assert!(s.overlay_visible(), "pinning overrides everything");

        s.mode = Mode::Recording;
        s.mic_in_use = true;
        s.hidden = true;
        assert!(
            !s.overlay_visible(),
            "hidden outranks all of it: pinned, mic in use, and recording"
        );
    }

    #[test]
    fn the_status_dot_only_reflects_whether_it_is_recording() {
        let mut s = State::default();
        // Solid at rest as well: the icon says which state it is in by its
        // colour, and a half-faded dot in a menu bar just looks broken.
        assert_eq!(s.dot_alpha(), 1.0, "solid at rest");

        s.mode = Mode::Recording;
        let recording = s.dot_alpha();
        assert_eq!(recording, 1.0, "solid while recording");

        // A capture problem is surfaced in the overlay, not by mutating the
        // menu bar icon into something else.
        s.error = Some("system audio silent".into());
        assert_eq!(s.dot_alpha(), recording);
    }

    #[test]
    fn the_menu_bar_carries_no_text() {
        let s = State::default();
        assert!(s.clock_title().is_empty());
        assert_eq!(s.summary(), "Idle");
        // A running job outranks the recording state in the menu header.
        let mut s = s;
        s.job = Some("Transcribing…".into());
        assert_eq!(s.summary(), "Transcribing…");
    }
}
