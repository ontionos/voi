//! The floating HUD on Windows: `mac/overlay.rs`, ported.
//!
//! The glass is a window of its own, exactly the glass's size — 44 × 38 at
//! rest, 78 × 38 recording, 408 × 174 open — with the blur behind it and the
//! corners rounded by the compositor (see `glass`), so frame and body are one
//! thing. Its top-right corner stays where it is, 16 points from the screen's
//! top-right corner until dragged: it grows down and to the left, as the
//! Mac's does, and the dot never moves. It fades in and out as a whole. The
//! "Record?" bubble under it is a small window of its own.
//!
//! Hover and presses are read from the system each frame, the way the Mac
//! samples `NSEvent.mouseLocation`.

use std::time::Duration;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, ViewportBuilder,
    ViewportId,
};

use super::draw::{self, rgba, Sweep, RED};
use super::glass;
use crate::state::Shared;

const HEADER_H: f32 = 38.0;
/// Glass to the screen's edge.
const MARGIN: f32 = 16.0;
const DOT_TO_EDGE: f32 = 12.0;
const HIT_SIZE: f32 = 20.0;
const DOT_RADIUS: f32 = 4.5;
const BARS_TO_DOT: f32 = 9.0;
pub const GLASS_IDLE: Vec2 = Vec2::new(2.0 * (DOT_TO_EDGE + HIT_SIZE / 2.0), HEADER_H);
const GLASS_TINY: Vec2 = Vec2::new(78.0, 38.0);
const GLASS_EXPANDED: Vec2 = Vec2::new(408.0, 174.0);
const PROMPT_W: f32 = 86.0;
const PROMPT_H: f32 = 26.0;
const PROMPT_TAIL: f32 = 7.0;
const PROMPT_GAP: f32 = 6.0;
const TRANSCRIPT_SIZE: f32 = 14.0;
/// One line of transcript, as the Mac's font measures it.
const LINE: f32 = 17.0;
/// How many characters wide the fade-in at the reveal head is.
const FADE: f32 = 5.0;
const COPY_SIZE: f32 = 24.0;
const COPY_INSET: f32 = 11.0;
const COPY_GLYPH: f32 = 13.0;
/// The animation's fixed step: the Mac's 60 frames a second.
const DT: f32 = 1.0 / 60.0;

const TITLE: &str = "Voi HUD";
const PROMPT_TITLE: &str = "Voi Prompt";

#[derive(Default)]
pub struct Hud {
    /// The glass's top-right corner on the screen, in points.
    anchor: Option<Pos2>,
    saved: Option<Pos2>,
    open: f32,
    tint: f32,
    appear: f32,
    prompt: f32,
    dot_hover: f32,
    dot_press: f32,
    copy_hover: f32,
    copy_press: f32,
    copy_flash: f32,
    reveal: f32,
    expanded: bool,
    active: bool,
    owed: f32,
    sweep: Sweep,
    hwnd: isize,
    prompt_hwnd: isize,
    /// While the glass is dragged: where the window was when it began, in
    /// points, and the pointer's offset from its corner, in pixels.
    drag: Option<(Rect, (i32, i32))>,
    /// Where Voi last put the window. It is placed only when that changes —
    /// never during a drag, which Windows runs, and never back to where it
    /// already is: a place sent every frame fought the drag and shook.
    placed: Option<Rect>,
    /// It was hidden until this frame.
    resting: bool,
    pub copy_clicked: bool,
    pub dot_clicked: bool,
    pub context_menu: bool,
}

/// `x += (target − x)·(1 − e^(−dt/τ))`, the Mac's easing, at its 60 fps.
fn ease(value: &mut f32, target: f32, tau: f32) {
    *value += (target - *value) * (1.0 - (-DT / tau).exp());
}

impl Hud {
    pub fn new() -> Self {
        Self::default()
    }

    /// The glass's size now: one of the Mac's three.
    fn size(&self) -> Vec2 {
        if self.expanded {
            GLASS_EXPANDED
        } else if self.active {
            GLASS_TINY
        } else {
            GLASS_IDLE
        }
    }

    /// The glass on the screen, in points.
    fn screen_glass(&self, anchor: Pos2) -> Rect {
        let size = self.size();
        Rect::from_min_size(Pos2::new(anchor.x - size.x, anchor.y), size)
    }

    fn header(glass: Rect) -> Rect {
        Rect::from_min_size(glass.min, Vec2::new(glass.width(), HEADER_H))
    }

    fn dot_hit(glass: Rect) -> Rect {
        let header = Self::header(glass);
        Rect::from_min_size(
            Pos2::new(glass.right() - DOT_TO_EDGE - HIT_SIZE, header.center().y - HIT_SIZE / 2.0),
            Vec2::splat(HIT_SIZE),
        )
    }

    fn copy_hit(glass: Rect) -> Rect {
        let centre = Pos2::new(glass.right() - COPY_INSET - COPY_GLYPH / 2.0, glass.bottom() - COPY_INSET - COPY_GLYPH / 2.0);
        Rect::from_center_size(centre, Vec2::splat(COPY_SIZE))
    }

    /// The first place: the glass's top-right 16 points from the top-right of
    /// the usable part of the screen.
    fn first_place(work: Rect) -> Pos2 {
        Pos2::new(work.right() - MARGIN, work.top() + MARGIN)
    }

    /// Run a frame: animate, and show the glass and the bubble as windows.
    pub fn show(&mut self, ctx: &egui::Context, shared: &Shared, work: Rect, pointer: Pos2, button_down: bool) {
        // Hidden, Voi draws a few frames a second; that wait is not the
        // HUD's time, or it would appear all at once instead of fading in.
        let dt = if std::mem::take(&mut self.resting) { 0.0 } else { ctx.input(|i| i.stable_dt).min(2.0 * DT) };
        let ppp = ctx.pixels_per_point();

        let mut anchor = *self.anchor.get_or_insert_with(|| {
            voi_core::load_overlay_position()
                .map(|(x, y)| Pos2::new(x as f32, y as f32))
                .unwrap_or_else(|| Self::first_place(work))
        });
        // On no screen — a monitor gone, a remote session at another size, a
        // place saved for another display — it goes back to its first place.
        let at_rest = Rect::from_min_size(Pos2::new(anchor.x - GLASS_IDLE.x, anchor.y), GLASS_IDLE);
        if !super::on_a_screen(super::screen_rect(Pos2::ZERO, at_rest, ppp)) {
            anchor = Self::first_place(work);
            self.anchor = Some(anchor);
        }

        let (visible, active, prompt_wanted) = {
            let s = shared.lock().unwrap();
            (s.overlay_visible(), s.is_active(), s.wants_prompt())
        };
        self.active = active;

        let glass = self.screen_glass(anchor);
        let over_glass = self.appear > 0.02 && glass.contains(pointer);
        let over_dot = Self::dot_hit(glass).contains(pointer);
        let over_copy = self.open > 0.55 && Self::copy_hit(glass).contains(pointer);

        // A drag is not a hover: while a button is held, nothing opens or
        // closes. When it ends, the glass is where Windows left it.
        //
        // Voi moves the glass itself, with the pointer: Windows' own drag
        // does not move a window that never takes the focus, as the HUD
        // must not.
        if let Some((from, grab)) = self.drag {
            let (x, y) = super::cursor();
            super::move_window(self.hwnd, x - grab.0, y - grab.1);
            if !button_down {
                self.drag = None;
                if let Some(to) = super::window_rect(self.hwnd, ppp) {
                    anchor += to.min - from.min;
                    tracing::info!(by = ?(to.min - from.min), "hud dragged");
                    self.anchor = Some(anchor);
                    self.placed = Some(self.screen_glass(anchor));
                }
            }
        }
        if !button_down {
            self.expanded = visible && over_glass;
        }
        shared.lock().unwrap().expanded = self.expanded;

        self.owed += dt;
        while self.owed >= DT {
            self.owed -= DT;
            self.step(shared, visible, active, prompt_wanted, over_dot, over_copy, button_down);
        }
        if !visible && self.appear < 0.02 {
            // Its window goes, and is placed afresh when it comes back.
            let (anchor, saved, sweep) = (self.anchor, self.saved, std::mem::take(&mut self.sweep));
            *self = Self { anchor, saved, sweep, resting: true, ..Self::default() };
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        }

        if visible && self.saved.is_none_or(|s| (s - anchor).length() >= 1.0) {
            if let Err(e) = voi_core::save_overlay_position(anchor.x as f64, anchor.y as f64) {
                tracing::warn!(error = %e, "could not save the overlay position");
            }
            self.saved = Some(anchor);
        }

        let glass = self.screen_glass(anchor);
        let mut builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size(glass.size())
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_has_shadow(false)
            // Made hidden, shown once it is glass (see `glass::no_transitions`).
            .with_visible(self.hwnd != 0);
        let moved = self.placed.is_none_or(|p| p.size() != glass.size() || (p.min - glass.min).length() >= 0.5);
        if self.drag.is_none() && moved {
            builder = builder.with_position(glass.min);
            self.placed = Some(glass);
        }
        ctx.show_viewport_immediate(ViewportId::from_hash_of(TITLE), builder, |ui, _| {
            let local = Rect::from_min_size(Pos2::ZERO, glass.size());
            self.paint(ui, local, shared);
            self.interact(ui, local);
        });
        if self.hwnd == 0 {
            self.hwnd = super::find_window(TITLE);
            glass::apply(self.hwnd);
            glass::non_activating(self.hwnd);
        }
        glass::popup(self.hwnd);
        glass::fade(self.hwnd, self.appear);

        self.show_prompt(ctx, glass);

        let fps = if visible { 60.0 } else { 30.0 };
        ctx.request_repaint_after(Duration::from_secs_f32(1.0 / fps));
    }

    #[allow(clippy::too_many_arguments)]
    fn step(&mut self, shared: &Shared, visible: bool, active: bool, prompt_wanted: bool, over_dot: bool, over_copy: bool, button_down: bool) {
        ease(&mut self.appear, if visible { 1.0 } else { 0.0 }, 0.14);
        let target = if self.expanded { 1.0 } else { 0.0 };
        if (self.open - target).abs() > 0.0005 {
            ease(&mut self.open, target, 0.12);
            if (self.open - target).abs() < 0.005 {
                self.open = target;
            }
        }
        ease(&mut self.tint, if active { 1.0 } else { 0.0 }, 0.45);
        ease(&mut self.dot_hover, if over_dot { 1.0 } else { 0.0 }, 0.09);
        let press = over_dot && button_down;
        ease(&mut self.dot_press, if press { 1.0 } else { 0.0 }, if press { 0.025 } else { 0.09 });
        ease(&mut self.copy_hover, if over_copy { 1.0 } else { 0.0 }, if over_copy { 0.09 } else { 0.16 });
        let press = over_copy && button_down;
        ease(&mut self.copy_press, if press { 1.0 } else { 0.0 }, if press { 0.025 } else { 0.09 });
        let copied = shared.lock().unwrap().copied_strength() as f32;
        let tau = if copied > self.copy_flash { 0.07 } else { 0.16 };
        ease(&mut self.copy_flash, copied, tau);
        ease(&mut self.prompt, if prompt_wanted && visible { 1.0 } else { 0.0 }, 0.30);
        let mut s = shared.lock().unwrap();
        let target = if s.is_active() { s.level_mic.max(s.level_system) } else { 0.0 };
        s.meter.tick(target, DT);
        s.meter.tick_phases(DT);
        drop(s);
        let size = self.size();
        let track = Rect::from_min_size(Pos2::ZERO, size).shrink(0.5);
        let reference = 2.0 * (GLASS_TINY.x + GLASS_TINY.y - 4.0 * glass::CORNER) + std::f32::consts::TAU * glass::CORNER;
        self.sweep.tick(track, glass::CORNER, reference, active, DT);
    }

    /// Clicks: the dot records; the rest of the glass drags; a right click on
    /// the header opens its menu.
    fn interact(&mut self, ui: &mut egui::Ui, glass: Rect) {
        let resp = ui.interact(glass, ui.scope_id().with("glass"), Sense::click_and_drag());
        let at = resp.interact_pointer_pos().unwrap_or(Pos2::ZERO);
        let on_dot = Self::dot_hit(glass).contains(at);
        let on_copy = self.open > 0.55 && Self::copy_hit(glass).contains(at);
        if resp.drag_started() && !on_dot && !on_copy {
            let ppp = ui.ctx().pixels_per_point();
            if let Some(from) = super::window_rect(self.hwnd, ppp) {
                // The spot that was grabbed, in pixels from the window's
                // corner: it stays under the pointer.
                let down = ui.ctx().input(|i| i.pointer.press_origin()).unwrap_or(at);
                self.drag = Some((from, ((down.x * ppp).round() as i32, (down.y * ppp).round() as i32)));
            }
        }
        if resp.clicked() {
            if on_dot {
                self.dot_clicked = true;
            } else if on_copy {
                self.copy_clicked = true;
            }
        }
        if resp.secondary_clicked() && Self::header(glass).contains(at) {
            self.context_menu = true;
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        }
    }

    /// The bubble under the idle widget while another app has the mic.
    fn show_prompt(&mut self, ctx: &egui::Context, glass: Rect) {
        let p = self.prompt * self.appear;
        if p <= 0.01 {
            self.prompt_hwnd = 0;
            return;
        }
        let dot = Self::dot_hit(glass).center();
        let size = Vec2::new(PROMPT_W, PROMPT_H + PROMPT_TAIL);
        let at = Pos2::new(dot.x - PROMPT_W / 2.0, glass.bottom() + PROMPT_GAP);
        let builder = ViewportBuilder::default()
            .with_title(PROMPT_TITLE)
            .with_inner_size(size)
            .with_position(at)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_has_shadow(false)
            .with_visible(self.prompt_hwnd != 0);
        let mut clicked = false;
        ctx.show_viewport_immediate(ViewportId::from_hash_of(PROMPT_TITLE), builder, |ui, _| {
            let painter = ui.painter().clone();
            let bubble = Rect::from_min_size(Pos2::new(0.0, PROMPT_TAIL), Vec2::new(PROMPT_W, PROMPT_H));
            let apex = Pos2::new(PROMPT_W / 2.0, 0.0);
            let fill = rgba(0.13, 0.13, 0.15, 0.94);
            painter.rect_filled(bubble, CornerRadius::same(7), fill);
            painter.rect_stroke(bubble, CornerRadius::same(7), Stroke::new(1.0, rgba(1.0, 1.0, 1.0, 0.12)), StrokeKind::Inside);
            let tail = vec![Pos2::new(apex.x - PROMPT_TAIL, PROMPT_TAIL + 0.5), apex, Pos2::new(apex.x + PROMPT_TAIL, PROMPT_TAIL + 0.5)];
            painter.add(Shape::convex_polygon(tail.clone(), fill, Stroke::NONE));
            painter.add(Shape::line(tail, Stroke::new(1.0, rgba(1.0, 1.0, 1.0, 0.12))));
            painter.text(bubble.center(), Align2::CENTER_CENTER, "Record?", FontId::proportional(11.5), rgba(0.94, 0.95, 0.97, 1.0));
            let resp = ui.interact(bubble, ui.scope_id().with("bubble"), Sense::click());
            clicked = resp.clicked();
        });
        if clicked {
            self.dot_clicked = true;
        }
        if self.prompt_hwnd == 0 {
            self.prompt_hwnd = super::find_window(PROMPT_TITLE);
            super::no_frame(self.prompt_hwnd);
            glass::no_transitions(self.prompt_hwnd);
            glass::non_activating(self.prompt_hwnd);
        }
        glass::popup(self.prompt_hwnd);
        glass::fade(self.prompt_hwnd, p);
    }

    /// For `--screenshots`: the glass drawn into `ui` where the Mac's panel
    /// has it in its 458 × 263 window, so the two can be laid over each other.
    pub fn paint_for_shot(&mut self, ui: &mut egui::Ui, shared: &Shared, open: bool) {
        let (active, visible) = {
            let s = shared.lock().unwrap();
            (s.is_active(), s.overlay_visible())
        };
        self.active = active;
        self.expanded = open;
        self.step(shared, visible, active, false, false, false, false);
        let size = self.size();
        let glass = Rect::from_min_size(Pos2::new(433.0 - size.x, 25.0), size);
        draw::shot_backdrop(ui.painter(), glass, glass::CORNER, 1.0);
        self.paint(ui, glass, shared);
    }

    fn paint(&mut self, ui: &mut egui::Ui, glass: Rect, shared: &Shared) {
        let painter = ui.painter().clone();
        let header = Self::header(glass);
        let dot_hit = Self::dot_hit(glass);
        let dot = dot_hit.center();
        let corner = CornerRadius::same(glass::CORNER as u8);

        // A dark wash over the blur behind: the Mac's material is dark.
        painter.rect_filled(glass, corner, rgba(0.11, 0.11, 0.12, 0.58));
        self.sweep.paint(&painter, glass.shrink(0.5), glass::CORNER);
        draw::hairline(&painter, glass, glass::CORNER, 0.10);
        if self.open > 0.55 {
            let line = Rect::from_min_size(Pos2::new(glass.left() + 10.0, header.bottom() - 1.0), Vec2::new(glass.width() - 20.0, 1.0));
            painter.rect_filled(line, CornerRadius::ZERO, rgba(1.0, 1.0, 1.0, 0.10));
        }

        let (pulse, wave) = {
            let s = shared.lock().unwrap();
            (s.meter.pulse, s.meter.wave())
        };
        if self.tint > 0.01 {
            let left = glass.left() + (0.10 * glass.width()).min(17.5);
            let right = dot_hit.left() - BARS_TO_DOT;
            draw::meter(&painter, left, right, header.center().y, 22.0, &wave, self.tint, self.tint);
        }

        let phase = 0.5 - 0.5 * (pulse * std::f32::consts::TAU).cos();
        let scale = 1.0 + 0.20 * self.dot_hover - 0.40 * self.dot_press;
        let breath = 1.0 + 0.40 * phase * (1.0 - self.tint) + 0.13 * phase * self.tint;
        if self.active {
            for i in (0..10).rev() {
                let t = i as f32 / 9.0;
                let r = DOT_RADIUS * scale + 1.0 + t * (4.0 + 7.0 * phase);
                let alpha = 0.26 * (1.0 - t).powf(1.6) * (0.32 + 0.68 * phase);
                painter.circle_filled(dot, r, rgba(RED.0, RED.1, RED.2, alpha));
            }
        }
        painter.circle_filled(dot, DOT_RADIUS * scale * breath, rgba(RED.0, RED.1, RED.2, 1.0));

        if self.open > 0.55 {
            let fade = ((self.open - 0.55) / 0.45).clamp(0.0, 1.0);
            self.paint_transcript(ui, glass, shared, fade);
            self.paint_copy(&painter, glass, fade);
        }
    }

    fn paint_transcript(&mut self, ui: &mut egui::Ui, glass: Rect, shared: &Shared, fade: f32) {
        let space = GLASS_EXPANDED.y - HEADER_H;
        let lines = ((space - 2.0 * 7.0) / LINE).floor();
        let margin = (space - lines * LINE) / 2.0;
        let content = Rect::from_min_max(
            Pos2::new(glass.left() + margin, glass.top() + HEADER_H + margin),
            Pos2::new(glass.right() - margin, glass.bottom() - margin),
        );
        let runs = shared.lock().unwrap().display_runs();
        let live: usize = runs.iter().filter(|(_, p)| *p).map(|(t, _)| t.chars().count()).sum();
        let target = live as f32 + FADE;
        if self.reveal > target {
            self.reveal = target;
        } else if (target - self.reveal).abs() > 0.05 {
            self.reveal += (target - self.reveal) * 0.08;
        } else {
            self.reveal = target;
        }
        let mut job = egui::text::LayoutJob::default();
        let mut n = 0usize;
        for (text, provisional) in runs {
            if !provisional {
                append(&mut job, &text, rgba(0.95, 0.96, 0.98, fade));
                continue;
            }
            for ch in text.chars() {
                let alpha = (((self.reveal - n as f32) / FADE).clamp(0.0, 1.0) * 10.0).round() / 10.0;
                n += 1;
                if alpha <= 0.0 {
                    break;
                }
                let mut buf = [0u8; 4];
                append(&mut job, ch.encode_utf8(&mut buf), rgba(0.74, 0.77, 0.81, alpha * fade));
            }
        }
        // A trailing space may run past the edge, as AppKit lets it.
        job.wrap.max_width = GLASS_EXPANDED.x - 2.0 * margin + TRANSCRIPT_SIZE * draw::MONO_SCALE * 0.6;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content));
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut child, |ui| {
                ui.label(job);
            });
    }

    fn paint_copy(&self, painter: &egui::Painter, glass: Rect, fade: f32) {
        let centre = Self::copy_hit(glass).center();
        let grow = 0.16 * self.copy_hover - 0.30 * self.copy_press;
        let side = COPY_GLYPH * (1.0 + grow);
        let alpha = fade * (1.0 - self.copy_flash) * 0.55;
        if alpha > 0.001 {
            let colour = rgba(0.70, 0.73, 0.78, alpha);
            let page = side * 0.72;
            let width = 0.10 * side;
            let radius = CornerRadius::same((0.16 * side).round() as u8);
            let origin = centre - Vec2::splat(side / 2.0);
            let front = Rect::from_min_size(Pos2::new(origin.x, origin.y + side - page), Vec2::splat(page));
            let back = front.translate(Vec2::new(0.28 * side, -0.28 * side));
            let gap = front.expand(width);
            painter
                .with_clip_rect(Rect::from_min_max(Pos2::new(gap.right(), back.top() - 2.0), back.max + Vec2::splat(2.0)))
                .rect_stroke(back, radius, Stroke::new(width, colour), StrokeKind::Middle);
            painter
                .with_clip_rect(Rect::from_min_max(Pos2::new(back.left() - 2.0, back.top() - 2.0), Pos2::new(back.right() + 2.0, gap.top())))
                .rect_stroke(back, radius, Stroke::new(width, colour), StrokeKind::Middle);
            painter.rect_stroke(front, radius, Stroke::new(width, colour), StrokeKind::Middle);
        }
        let alpha = fade * self.copy_flash;
        if alpha > 0.001 {
            let origin = centre - Vec2::splat(COPY_GLYPH / 2.0);
            let at = |x: f32, y: f32| Pos2::new(origin.x + x * COPY_GLYPH, origin.y + (1.0 - y) * COPY_GLYPH);
            painter.add(Shape::line(
                vec![at(0.12, 0.54), at(0.38, 0.24), at(0.88, 0.78)],
                Stroke::new(0.14 * COPY_GLYPH, rgba(0.32, 0.80, 0.44, alpha)),
            ));
        }
    }
}

fn append(job: &mut egui::text::LayoutJob, text: &str, color: Color32) {
    let mut format = egui::TextFormat::simple(FontId::monospace(TRANSCRIPT_SIZE * draw::MONO_SCALE), color);
    format.line_height = Some(LINE);
    job.append(text, 0.0, format);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dot stays where it is whatever the glass's size.
    #[test]
    fn the_glass_grows_from_its_top_right_corner() {
        let anchor = Pos2::new(1904.0, 16.0);
        let mut hud = Hud::new();
        let idle = hud.screen_glass(anchor);
        hud.active = true;
        let tiny = hud.screen_glass(anchor);
        hud.expanded = true;
        let open = hud.screen_glass(anchor);
        for g in [idle, tiny, open] {
            assert_eq!(g.right_top(), anchor);
            assert_eq!(Hud::dot_hit(g).center(), Pos2::new(anchor.x - 22.0, anchor.y + 19.0));
        }
        assert_eq!(idle.size(), Vec2::new(44.0, 38.0));
        assert_eq!(open.size(), GLASS_EXPANDED);
        let copy = Hud::copy_hit(open).center();
        assert_eq!(open.right_bottom() - copy, Vec2::splat(17.5));
    }

    #[test]
    fn it_first_appears_sixteen_points_from_the_top_right_corner() {
        let work = Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1040.0));
        assert_eq!(Hud::first_place(work), Pos2::new(1904.0, 16.0));
    }
}
