//! The dictation window on Windows: hold the key, speak, let go, and it is
//! typed — `mac/voice.rs`, ported.
//!
//! The same glass, sizes, timings and texts as the Mac's. It is drawn as its
//! own always-on-top window that never takes focus and lets clicks through,
//! so the text lands in whatever had the cursor; whether the pointer is on it
//! (to throw a dictation away) is asked of the system each frame, as the Mac
//! does.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Vec2, ViewportBuilder, ViewportId};
use voi_core::dictation::{flush_deadline, Dictation, DictationEvent};
use voi_core::prefs::Prefs;

use super::draw::{self, rgba, Sweep, RED};
use super::paste::{deliver, Delivered};
use crate::state::Meter;

/// The glass. One size, always.
const GLASS: Vec2 = Vec2::new(368.0, 93.0);
/// Transparent room around the glass for the edge light to spill into.
const PAD: f32 = draw::REACH + 5.0;
/// Windows rounds the window's corners itself (see `glass`), to this.
const CORNER: f32 = super::glass::CORNER;
/// Text inset from the glass's sides.
const MARGIN: f32 = 18.0;
const TEXT_SIZE: f32 = 13.0;
/// One line of text, as the Mac's font measures it.
const LINE: f32 = 16.0;
/// The strip at the bottom that the meter lives in.
const METER_H: f32 = 22.0;
/// How far the glass travels towards its edge as it comes and goes.
const TRAVEL: f32 = 30.0;
/// The parting swell after the key comes up.
const PARTING: Duration = Duration::from_millis(1_500);
/// How long the window stays once the text is in: not at all.
const DELIVERED: Duration = Duration::ZERO;
/// Unless there is something to read instead.
const TO_READ: Duration = Duration::from_millis(2_500);
/// Per-character reveal (see the Mac's comments for why each is what it is).
const ARRIVING: Duration = Duration::from_millis(50);
const WHITENING_TIME: Duration = Duration::from_millis(50);
const LETTER: Duration = Duration::from_millis(3);
const WHITENING: Duration = Duration::from_millis(2);
const TRAIL: Duration = Duration::from_millis(50);
const FLUSH: Duration = Duration::from_millis(5);
/// The animation's fixed step: the Mac's 60 frames a second.
const DT: f32 = 1.0 / 60.0;

pub const VIEWPORT: &str = "voi-dictation";

pub struct Voice {
    session: Option<Dictation>,
    finishing: Option<Instant>,
    began: Instant,
    connected: Option<Instant>,
    heard: bool,
    released: Option<Instant>,
    /// The window's top-left, in points.
    home: Pos2,
    /// Which way it comes from, y up as on the Mac.
    from: (f32, f32),
    beside_pointer: bool,
    /// Whether, as it goes, it has turned towards where the pointer is now.
    aimed_home: bool,
    confirmed: Option<(Instant, Duration)>,
    sounds: bool,
    over: bool,
    hover: f32,
    button: bool,
    open: f32,
    speed: f32,
    wanted: f32,
    owed: f32,
    // What is drawn.
    meter: Meter,
    meter_target: f32,
    settled: String,
    tail: String,
    note: Option<String>,
    born: Vec<Instant>,
    settled_at: Vec<Instant>,
    listening: bool,
    sweep: Sweep,
    /// Whether the window's frame has been taken off since it was made.
    framed: bool,
    /// The window was just made, or a dictation just began: the time since
    /// the last frame is not animation time.
    just_made: bool,
    hwnd: isize,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            session: None,
            finishing: None,
            began: Instant::now(),
            connected: None,
            heard: false,
            released: None,
            home: Pos2::ZERO,
            from: (0.0, -1.0),
            beside_pointer: false,
            aimed_home: false,
            confirmed: None,
            sounds: false,
            over: false,
            hover: 0.0,
            button: false,
            open: 0.0,
            speed: 0.0,
            wanted: 0.0,
            owed: 0.0,
            meter: Meter::default(),
            meter_target: 0.0,
            settled: String::new(),
            tail: String::new(),
            note: None,
            born: Vec::new(),
            settled_at: Vec::new(),
            listening: false,
            sweep: Sweep::default(),
            framed: false,
            just_made: false,
            hwnd: 0,
        }
    }

    pub fn listening(&self) -> bool {
        self.session.is_some()
    }

    /// Whether there is anything to draw.
    pub fn visible(&self) -> bool {
        self.open > 0.001 || self.wanted > 0.0
    }

    pub fn start(&mut self, api_key: &str, prefs: &Prefs, points_per_pixel: f32) {
        if self.session.is_some() {
            return;
        }
        self.settled.clear();
        self.tail.clear();
        self.note = None;
        self.born.clear();
        self.settled_at.clear();
        self.listening = true;
        self.finishing = None;
        self.began = Instant::now();
        self.connected = None;
        self.heard = false;
        self.released = None;
        self.confirmed = None;
        self.sounds = prefs.sounds;
        self.session = Some(Dictation::start(
            api_key,
            prefs.languages.clone(),
            prefs.terms.clone(),
            prefs.microphone.clone(),
        ));
        self.place(prefs, points_per_pixel);
        self.wanted = 1.0;
        // Idle, Voi draws a few frames a second; the time since the last one
        // is not the glass's, or it would arrive in a single frame.
        self.just_made = true;
        self.owed = 0.0;
    }

    /// The key came up: stop listening, and let the last words arrive.
    pub fn finish(&mut self) {
        let Some(session) = &self.session else { return };
        session.finish();
        let now = Instant::now();
        let mut next = now;
        for born in self.born.iter_mut().filter(|at| **at > now) {
            *born = next;
            next += FLUSH;
        }
        let mut next = now;
        for settled in self.settled_at.iter_mut().filter(|at| **at > now) {
            *settled = next;
            next += FLUSH;
        }
        self.finishing = Some(now);
        self.released = Some(now);
        self.listening = false;
    }

    /// Letting go with the pointer on the glass throws it away.
    pub fn hovered(&self) -> bool {
        self.session.is_some() && self.over
    }

    pub fn cancel(&mut self) {
        if self.session.take().is_none() {
            return;
        }
        self.finishing = None;
        self.confirmed = None;
        self.listening = false;
        self.wanted = 0.0;
        super::sound::play(super::sound::Note::End, self.sounds);
        tracing::info!("dictation cancelled");
    }

    /// Where it goes: on the chosen spot of the screen the pointer is on, or
    /// beside the pointer.
    fn place(&mut self, prefs: &Prefs, points_per_pixel: f32) {
        let surround = PAD + TRAVEL;
        let (pointer, work) = pointer_and_work_area(points_per_pixel);
        let (gw, gh) = (GLASS.x as f64, GLASS.y as f64);
        // The Mac's placement counts y upward from the bottom of the work
        // area; so does this, and it is turned the right way up at the end.
        let up = |y_down: f32| (work.bottom() - y_down) as f64;
        let origin = (work.left() as f64, 0.0);
        let screen = (work.width() as f64, work.height() as f64);
        let down = |x: f64, y_up: f64| Pos2::new(x as f32, work.bottom() - y_up as f32 - GLASS.y);
        if prefs.dictation_follow_pointer {
            let p = (pointer.x as f64, up(pointer.y));
            let (x, y) = voi_core::prefs::beside_pointer(p, origin, screen, (gw, gh));
            let (_, (dx, dy)) = voi_core::prefs::facing_corner(p, (x, y), (gw, gh));
            self.home = down(x, y) - Vec2::splat(surround);
            self.beside_pointer = true;
            self.from = (((dx as f32) / TRAVEL).clamp(-1.0, 1.0), ((dy as f32) / TRAVEL).clamp(-1.0, 1.0));
            self.aimed_home = false;
            return;
        }
        let (x, y) = prefs.dictation_spot.place(origin, screen, (gw, gh), prefs.dictation_margin);
        self.home = down(x, y) - Vec2::splat(surround);
        let (sx, sy) = prefs.dictation_spot.slide();
        self.from = (sx as f32, sy as f32);
        self.beside_pointer = false;
    }

    /// Slide towards `pointer` from the glass's corner nearest it, as
    /// `place` does when it comes.
    fn aim_at(&mut self, pointer: Pos2, points_per_pixel: f32) {
        let (_, work) = pointer_and_work_area(points_per_pixel);
        let surround = PAD + TRAVEL;
        let up = |y_down: f32| (work.bottom() - y_down) as f64;
        let glass = (self.home.x + surround) as f64;
        let glass_up = up(self.home.y + surround + GLASS.y);
        let (_, (dx, dy)) =
            voi_core::prefs::facing_corner((pointer.x as f64, up(pointer.y)), (glass, glass_up), (GLASS.x as f64, GLASS.y as f64));
        self.from = (((dx as f32) / TRAVEL).clamp(-1.0, 1.0), ((dy as f32) / TRAVEL).clamp(-1.0, 1.0));
    }

    /// The glass inside the window, at the current slide.
    fn glass(&self, offset: Vec2) -> Rect {
        let window = window_size();
        let min = Pos2::new((window.x - GLASS.x) / 2.0 + offset.x, (window.y - GLASS.y) / 2.0 - offset.y);
        Rect::from_min_size(min, GLASS)
    }

    fn offset(&self) -> Vec2 {
        let away = (1.0 - self.open) * TRAVEL;
        Vec2::new(self.from.0 * away, self.from.1 * away)
    }

    /// Once a frame: read the session, decide when it is over, move.
    pub fn refresh(&mut self, dt: f32, points_per_pixel: f32) {
        let (pointer, _) = pointer_and_work_area(points_per_pixel);
        let offset = if self.beside_pointer { Vec2::ZERO } else { self.offset() };
        let glass = self.glass(offset).translate(self.home.to_vec2());
        self.over = self.visible() && glass.contains(pointer);
        let down = left_button_down();
        let pressed = down && !self.button;
        self.button = down;
        if pressed && self.hovered() {
            self.cancel();
        }

        self.drain();
        if let Some(since) = self.finishing {
            if self.session.is_none() {
                self.finishing = None;
            } else if Instant::now() > flush_deadline(self.began, since, self.connected) {
                match self.connected {
                    Some(at) => tracing::warn!(
                        connected_after_ms = at.duration_since(self.began).as_millis() as u64,
                        "dictation did not finish in time; using what arrived"
                    ),
                    None => tracing::error!(
                        held_ms = since.duration_since(self.began).as_millis() as u64,
                        "dictation never reached soniox; what was said is lost"
                    ),
                }
                let text = voi_core::dictation::assembled(&self.settled, &self.tail);
                self.complete(text);
            }
        }
        if let Some((since, stay)) = self.confirmed {
            if since.elapsed() >= stay {
                self.confirmed = None;
                self.wanted = 0.0;
            }
        }

        // Beside the pointer it goes back into the pointer, where it is now:
        // it may have moved while you spoke.
        if self.beside_pointer && self.wanted == 0.0 && !self.aimed_home {
            self.aimed_home = true;
            self.aim_at(pointer, points_per_pixel);
        }

        // Fixed steps of a sixtieth, the Mac's frame: the spring and the
        // meter run at the same pace whatever this display refreshes at.
        // Making the window holds a frame up for a fifth of a second; spent
        // at once, that time would finish the coming in a single frame.
        let dt = if std::mem::take(&mut self.just_made) { 0.0 } else { dt };
        // At most two steps a frame: a frame held up — the first dictation's
        // setting up, Windows busy — slows the glass rather than skipping it
        // to the end, which made it simply appear.
        self.owed += dt.min(2.0 * DT);
        while self.owed >= DT {
            self.owed -= DT;
            self.step();
        }
        self.stamp();
    }

    fn step(&mut self) {
        // Windows takes a few frames to make the window. Until it is on the
        // screen the glass waits where it comes from, or it would arrive
        // unseen and simply be there: the coming out of the pointer would be
        // lost, and only the going seen.
        if self.wanted > self.open && !self.framed {
            self.open = 0.0;
            self.speed = 0.0;
        }
        let (stiffness, damping) = if self.beside_pointer { (1100.0, 52.0) } else { (420.0, 26.0) };
        self.speed += ((self.wanted - self.open) * stiffness - self.speed * damping) * DT;
        self.open += self.speed * DT;
        if (self.open - self.wanted).abs() < 0.002 && self.speed.abs() < 0.05 {
            self.open = self.wanted;
            self.speed = 0.0;
        }
        let parting = self.released.map_or(0.0, |at| {
            let done = at.elapsed().as_secs_f32() / PARTING.as_secs_f32();
            (1.0 - done).clamp(0.0, 1.0).powf(1.6)
        });
        let wanted = if self.over && self.session.is_some() { 1.0 } else { 0.0 };
        let step = DT / 0.10;
        self.hover += (wanted - self.hover).clamp(-step, step);
        let target = if self.listening { self.meter_target } else { parting };
        self.meter.tick(target, DT);
        self.meter.tick_phases(DT);
        let track = self.glass(Vec2::ZERO).shrink(0.5);
        let reference = Rect::from_min_size(Pos2::ZERO, GLASS);
        let reference = 2.0 * (reference.width() + reference.height() - 4.0 * CORNER) + std::f32::consts::TAU * CORNER;
        self.sweep.tick(track, CORNER, reference, self.listening(), DT);
    }

    /// When each character arrives and when it turns white.
    fn stamp(&mut self) {
        let now = Instant::now();
        let settled_len = self.settled.chars().count();
        let length = settled_len + self.tail.chars().count();
        let stagger = |backlog: usize, ideal: Duration| ideal.min(TRAIL / backlog.max(1) as u32);
        let gap = stagger(length.saturating_sub(self.born.len()), LETTER);
        let mut next = self.born.last().map_or(now, |last| (*last + gap).max(now));
        while self.born.len() < length {
            self.born.push(next);
            next += gap;
        }
        let gap = stagger(settled_len.saturating_sub(self.settled_at.len()), WHITENING);
        let mut next = self.settled_at.last().map_or(now, |last| (*last + gap).max(now));
        while self.settled_at.len() < settled_len {
            self.settled_at.push(next);
            next += gap;
        }
    }

    fn drain(&mut self) {
        let Some(session) = &mut self.session else { return };
        let mut done = None;
        let mut failed = None;
        while let Ok(event) = session.events.try_recv() {
            match event {
                DictationEvent::Level(level) => self.meter_target = level,
                DictationEvent::Connected => {
                    self.connected.get_or_insert_with(Instant::now);
                }
                DictationEvent::Text { settled, tail } => {
                    if !self.heard && !(settled.trim().is_empty() && tail.trim().is_empty()) {
                        self.heard = true;
                        tracing::info!(
                            after_key_ms = self.began.elapsed().as_millis() as u64,
                            after_connect_ms = self.connected.map(|at| at.elapsed().as_millis() as u64),
                            "first words"
                        );
                    }
                    self.settled = settled;
                    self.tail = tail;
                }
                DictationEvent::Done(text) => done = Some(text),
                DictationEvent::Warning(message) => tracing::warn!(%message, "dictation"),
                DictationEvent::Failed(message) => failed = Some(message),
            }
        }
        if let Some(message) = failed {
            self.session = None;
            self.finishing = None;
            self.listening = false;
            self.note = Some(message);
            self.confirmed = Some((Instant::now(), TO_READ));
            return;
        }
        if let Some(text) = done {
            self.complete(text);
        }
    }

    fn complete(&mut self, text: String) {
        self.session = None;
        self.finishing = None;
        self.listening = false;
        let (note, stay) = if text.trim().is_empty() {
            (String::new(), DELIVERED)
        } else {
            let delivered = deliver(&text);
            tracing::info!(chars = text.chars().count(), ?delivered, "dictation delivered");
            match delivered {
                Delivered::Pasted => (text, DELIVERED),
                Delivered::Copied => (format!("Copied — press Ctrl+V to paste.\n\n{text}"), TO_READ),
            }
        };
        self.note = Some(note);
        self.confirmed = Some((Instant::now(), stay));
    }

    /// For screenshots: words as though heard, fully arrived and settled, the
    /// glass fully up, and whether the pointer is on it. Then `tick_for_shot`
    /// for the frames, and `paint_into` the window to read it back from.
    pub fn stage(&mut self, settled: &str, tail: &str, listening: bool, hover: bool) {
        let long_ago = Instant::now() - Duration::from_secs(5);
        self.settled = settled.to_string();
        self.tail = tail.to_string();
        self.note = None;
        self.born = vec![long_ago; settled.chars().count() + tail.chars().count()];
        self.settled_at = vec![long_ago; settled.chars().count()];
        self.listening = listening;
        self.meter_target = if listening { 0.6 } else { 0.0 };
        self.open = 1.0;
        self.wanted = 1.0;
        self.speed = 0.0;
        self.over = hover;
        self.hover = if hover { 1.0 } else { 0.0 };
    }

    /// For `VOI_TEST_STAGE`: placed as a dictation would be, without one.
    pub fn start_staged(&mut self, prefs: &Prefs, points_per_pixel: f32) {
        self.place(prefs, points_per_pixel);
    }

    pub fn tick_for_shot(&mut self) {
        self.step();
    }

    pub fn paint_into(&self, painter: &egui::Painter) {
        self.paint(painter, self.glass(self.offset()), (self.open / 0.35).clamp(0.0, 1.0));
    }

    pub fn window_size() -> Vec2 {
        window_size()
    }

    /// Draw it, as a window of its own the size of the glass. The window
    /// itself slides and fades, blur and all.
    ///
    /// The window is made once, when Voi starts, and kept, clear and letting
    /// clicks through while there is nothing to show. Made afresh for each
    /// dictation it sometimes flashed white before its first frame, and the
    /// making held the glass up by a fifth of a second.
    pub fn show(&mut self, ctx: &egui::Context) {
        let glass = self.glass(self.offset()).translate(self.home.to_vec2());
        let builder = ViewportBuilder::default()
            .with_title("Voi Dictation")
            .with_inner_size(GLASS)
            .with_position(glass.min)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_mouse_passthrough(true)
            .with_has_shadow(false)
            // Made hidden, shown once it is glass (see `glass::no_transitions`).
            .with_visible(self.framed);
        ctx.show_viewport_immediate(ViewportId::from_hash_of(VIEWPORT), builder, |ui, _| {
            self.paint(ui.painter(), Rect::from_min_size(Pos2::ZERO, GLASS), 1.0);
        });
        if !self.framed {
            let hwnd = super::find_window("Voi Dictation");
            if hwnd != 0 {
                super::glass::apply(hwnd);
                super::glass::non_activating(hwnd);
                self.framed = true;
                self.just_made = true;
                self.hwnd = hwnd;
            }
        }
        super::glass::popup(self.hwnd);
        // The Mac fades the whole panel in over the first third of the way.
        super::glass::fade(self.hwnd, (self.open / 0.35).clamp(0.0, 1.0));
        if self.visible() {
            ctx.request_repaint();
        }
    }

    /// The glass at `glass`; `window_alpha` is for a picture with no window
    /// to fade (the screenshots), 1 when the window fades itself.
    fn paint(&self, painter: &egui::Painter, glass: Rect, window_alpha: f32) {
        let open = self.open.clamp(0.0, 1.0);
        if open <= 0.01 {
            return;
        }
        let fade = |c: Color32| c.gamma_multiply(window_alpha);

        // A dark wash over the blur behind: the Mac's material is dark.
        draw::shot_backdrop(painter, glass, CORNER, window_alpha);
        painter.rect_filled(glass, CornerRadius::same(CORNER as u8), fade(rgba(0.11, 0.11, 0.12, 0.58)));
        // The edge light, above the glass.
        self.sweep.paint(painter, glass.shrink(0.5), CORNER);
        draw::hairline(painter, glass, CORNER, 0.10 * open * window_alpha);

        // The words, hanging from the bottom of their box.
        let shown = self.note.clone().unwrap_or_else(|| format!("{}{}", self.settled, self.tail));
        if !shown.trim().is_empty() {
            let job = match &self.note {
                Some(note) => styled(note, open * window_alpha, 1.0),
                None => self.revealed(open * window_alpha),
            };
            let width = GLASS.x - 2.0 * MARGIN;
            let bottom = glass.bottom() - (METER_H + 6.0);
            let room = (glass.bottom() - glass.top()) - 12.0 - (METER_H + 6.0);
            let lines = (room / LINE).floor().max(1.0);
            let window = Rect::from_min_max(Pos2::new(glass.left() + MARGIN, bottom - lines * LINE), Pos2::new(glass.left() + MARGIN + width, bottom));
            let mut job = job;
            // A trailing space may run past the edge, as AppKit lets it;
            // egui would otherwise break the line a word early.
            job.wrap.max_width = width + TEXT_SIZE * draw::MONO_SCALE * 0.6;
            let galley = painter.layout_job(job);
            // Laid in a box at least the window's height whose bottom is the
            // window's: a short text starts at the top of the window, a long
            // one has its beginning pushed up out of sight (the Mac's way).
            let top = bottom - galley.size().y.max(lines * LINE);
            painter.with_clip_rect(window).galley(Pos2::new(window.left(), top), galley, Color32::WHITE);
        }

        // The meter along the bottom, red while listening.
        let tint = if self.listening { 1.0 } else { 0.0 };
        draw::meter(painter, glass.left(), glass.right(), glass.bottom() - (METER_H / 2.0 + 2.0), 18.0, &self.meter.wave(), tint, open * window_alpha);

        // Last, over everything: the pointer here means throwing it away.
        if self.hover > 0.001 {
            let shade = self.hover * open * window_alpha;
            let r = CornerRadius::same(CORNER as u8);
            painter.rect_filled(glass, r, rgba(0.0, 0.0, 0.0, 0.40 * shade));
            painter.rect_filled(glass, r, rgba(RED.0, RED.1, RED.2, 0.20 * shade));
            painter.rect_stroke(glass.shrink(0.75), r, egui::Stroke::new(1.5, rgba(RED.0, RED.1, RED.2, 0.75 * shade)), egui::StrokeKind::Middle);
            painter.text(glass.center(), Align2::CENTER_CENTER, "Cancel", FontId::proportional(TEXT_SIZE), rgba(1.0, 0.44, 0.40, shade));
        }
    }

    /// The text with its newest characters still arriving.
    fn revealed(&self, alpha: f32) -> egui::text::LayoutJob {
        const STEPS: f32 = 40.0;
        let now = Instant::now();
        let age = |at: Option<&Instant>, over: Duration| match at {
            Some(at) if *at <= now => (now.duration_since(*at).as_secs_f32() / over.as_secs_f32()).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let mut runs: Vec<(String, f32, f32)> = Vec::new();
        for (i, ch) in self.settled.chars().chain(self.tail.chars()).enumerate() {
            let showing = age(self.born.get(i), ARRIVING);
            if showing <= 0.0 {
                break;
            }
            let certain = age(self.settled_at.get(i), WHITENING_TIME);
            let (showing, certain) = ((showing * STEPS).round() / STEPS, (certain * STEPS).round() / STEPS);
            match runs.last_mut() {
                Some((run, had, was)) if *had == showing && *was == certain => run.push(ch),
                _ => runs.push((ch.to_string(), showing, certain)),
            }
        }
        let mut job = egui::text::LayoutJob::default();
        for (run, showing, certain) in runs {
            append(&mut job, &run, alpha * showing, certain);
        }
        job
    }
}

/// One run of text, grey while the server may still rewrite it, white once
/// settled.
fn append(job: &mut egui::text::LayoutJob, text: &str, alpha: f32, certain: f32) {
    let mix = |unsure: f32, sure: f32| unsure + (sure - unsure) * certain;
    let color = rgba(mix(0.62, 0.96), mix(0.66, 0.97), mix(0.72, 0.99), alpha);
    let mut format = egui::TextFormat::simple(FontId::monospace(TEXT_SIZE * draw::MONO_SCALE), color);
    format.line_height = Some(LINE);
    job.append(text, 0.0, format);
}

fn styled(text: &str, alpha: f32, certain: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    append(&mut job, text, alpha, certain);
    job
}

fn window_size() -> Vec2 {
    GLASS + Vec2::splat(2.0 * (PAD + TRAVEL))
}

use super::{left_button_down, pointer_and_work_area};

#[cfg(test)]
mod tests {
    use super::*;

    /// Three lines of text fit above the meter, as on the Mac.
    #[test]
    fn three_lines_fit_above_the_meter() {
        let room = GLASS.y - 12.0 - (METER_H + 6.0);
        assert_eq!((room / LINE).floor(), 3.0);
        assert_eq!(window_size(), Vec2::new(478.0, 203.0));
    }
}
