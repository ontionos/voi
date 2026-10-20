//! Settings on Windows: the Mac's Settings (`mac/settings.rs`) — the same
//! tabs, rows, controls, texts, defaults and saving — set in the same glass as
//! the HUD and the dictation window: blurred, glowing at the edge, no title
//! bar. The four tabs sit inside the glass where the Mac's toolbar is; the
//! Mac's red close circle sits in its top-right corner, and Esc or Ctrl+W
//! close it too; dragging anywhere but on a control moves it.
//!
//! Positions are the Mac's, in points, measured from the top-left of the
//! content area under the tabs: `PAD`, `GROUP`, `INSET`, `ROW`, `FIELD` and
//! the rest are its constants, and every row, header and hint advances the
//! same cursor the Mac's `Page` does.
//!
//! As on the Mac there is no Save button. Twice a second whatever the
//! controls hold is compared with what was last written, and if it differs it
//! is written, read back, and the footer says what was saved.

use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
    ViewportBuilder, ViewportId,
};
use voi_core::hotkey::{Hotkey, Verdict};
use voi_core::prefs::{self, DictationKey, DictationMode, Prefs, Spot, DEFAULT_MARGIN};

use super::draw::{self, rgba};
use super::{hook, login};

pub const TITLE: &str = "Voi Settings";
const VIEWPORT: &str = "voi-settings";

// The Mac's layout constants (mac/settings.rs).
const WIDTH: f32 = 560.0;
const HEIGHT: f32 = 500.0;
const PAD: f32 = 20.0;
const GROUP: f32 = WIDTH - 2.0 * PAD;
const INSET: f32 = 16.0;
const INSET_RIGHT: f32 = 14.0;
const ROW: f32 = 44.0;
const FIELD: f32 = 290.0;
const FOOTER: f32 = 44.0;
/// Where the Mac's toolbar is: the tabs, inside the glass.
const TABS_H: f32 = 60.0;
/// Windows rounds the window's corners itself (see `glass`), to this.
const CORNER: f32 = super::glass::CORNER;
/// The window is the glass.
const GLOW: f32 = 0.0;

const GLASS: Vec2 = Vec2::new(WIDTH, TABS_H + HEIGHT);

/// The colours of the Mac's dark appearance, as drawn on glass.
/// Soniox's sign-up page, from its own documentation.
const SIGN_UP: &str = "https://console.soniox.com/signup/";

fn text() -> Color32 {
    rgba(0.98, 0.98, 0.98, 1.0)
}
fn secondary() -> Color32 {
    rgba(0.76, 0.76, 0.76, 1.0)
}
fn tertiary() -> Color32 {
    rgba(0.50, 0.50, 0.50, 1.0)
}
fn accent() -> Color32 {
    Color32::from_rgb(10, 132, 255)
}
fn hairline() -> Color32 {
    rgba(1.0, 1.0, 1.0, 0.11)
}

const TABS: [(&str, &str, &str); 4] = [
    ("General", "\u{E713}", "icons"),
    ("Audio", "\u{E720}", "icons"),
    ("Dictation", "\u{2318}", "symbol"),
    ("Dictionary", "\u{E82D}", "icons"),
];

/// The Mac's dictation keys as Windows has them: the same keys, in the same
/// places, under Windows' names. Fn never reaches Windows, so it is not here.
const KEYS: [(DictationKey, &str); 4] = [
    (DictationKey::RightCommand, "Right Ctrl"),
    (DictationKey::LeftCommand, "Left Ctrl"),
    (DictationKey::RightOption, "Right Alt"),
    (DictationKey::LeftOption, "Left Alt"),
];

pub fn key_label(key: DictationKey) -> &'static str {
    match key {
        DictationKey::RightControl => "Right Ctrl",
        other => KEYS.iter().find(|(k, _)| *k == other).map_or("Right Ctrl", |(_, l)| *l),
    }
}

pub struct Settings {
    tab: usize,
    /// What the controls hold; written when it differs from `last`.
    draft: Prefs,
    last: Prefs,
    languages: String,
    terms: String,
    key: String,
    key_seen: Option<String>,
    key_focused: bool,
    login: login::Status,
    microphones: (Vec<String>, Option<String>),
    /// The footer's message: it stays until the next one, as on the Mac.
    status: String,
    hotkey_note: Option<(Color32, String)>,
    frames: u64,
    hwnd: isize,
    /// Whether the window has been given its first place.
    placed: bool,
    /// While the glass is dragged: the pointer's offset from the window's
    /// corner, in pixels.
    drag: Option<(i32, i32)>,
    opened: Instant,
}

/// What the window asks of the app this frame.
pub struct Outcome {
    /// Prefs just written and read back, to take into force.
    pub saved: Option<Prefs>,
    pub close: bool,
}

impl Settings {
    pub fn new(prefs: &Prefs) -> Self {
        let key = voi_core::find_api_key().ok();
        let path = prefs::path().map(|p| p.display().to_string()).unwrap_or_default();
        Self {
            tab: 0,
            draft: prefs.clone(),
            last: prefs.clone(),
            languages: prefs::join_list(&prefs.languages),
            terms: prefs::join_list(&prefs.terms),
            key: key.clone().unwrap_or_default(),
            key_seen: key,
            key_focused: false,
            login: login::status(),
            microphones: voi_audio::inputs(),
            // On the Mac the footer opens on the settings file's path, or the
            // Accessibility note; Windows needs no such permission.
            status: path,
            hotkey_note: None,
            frames: 0,
            hwnd: 0,
            placed: false,
            drag: None,
            opened: Instant::now(),
        }
    }

    /// Draw the window and run its autosave. Called every frame while open.
    /// `at`: where it was last closed in this run, as the Mac keeps its
    /// window; centred the first time.
    pub fn show(&mut self, ctx: &egui::Context, captured: Option<Hotkey>, work: Rect, at: Option<Pos2>) -> Outcome {
        if let Some(hotkey) = captured {
            self.take_hotkey(hotkey);
        }
        let window = GLASS + Vec2::splat(2.0 * GLOW);
        let mut close = false;
        let mut builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_icon(super::app_icon())
            .with_inner_size(window)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_has_shadow(false)
            // Made hidden, shown once it is glass (see `glass::no_transitions`).
            .with_visible(self.hwnd != 0);
        // Opens in the middle of the screen, as the Mac's does, and is placed
        // only then: from there on the window is where Windows has it. Asking
        // for a place every frame fought every drag, and the window shook.
        if !self.placed {
            self.placed = true;
            builder = builder.with_position(at.unwrap_or(work.center() - window / 2.0));
        }
        // Voi moves the window itself, with the pointer. Windows' own drag
        // keeps the button's release from the window, so the next drag
        // never began, and Settings stopped moving.
        if let Some((dx, dy)) = self.drag {
            if super::left_button_down() {
                let (x, y) = super::cursor();
                super::move_window(self.hwnd, x - dx, y - dy);
                ctx.request_repaint();
            } else {
                self.drag = None;
            }
        }
        ctx.show_viewport_immediate(ViewportId::from_hash_of(VIEWPORT), builder, |ui, _| {
            close |= self.draw(ui);
        });

        if self.hwnd == 0 {
            self.hwnd = super::find_window(TITLE);
            super::glass::apply(self.hwnd);
        }
        super::glass::popup(self.hwnd);

        self.frames += 1;
        let saved = if self.frames % 30 == 0 { self.autosave() } else { None };
        Outcome { saved, close }
    }

    /// Where the window is now, to open there again.
    pub fn place(&self, pixels_per_point: f32) -> Option<Pos2> {
        super::window_rect(self.hwnd, pixels_per_point).map(|r| r.min)
    }

    /// The window's whole contents, drawn into `ui` from its top-left: the
    /// glass, the tabs, the page, the footer. Returns whether to close.
    pub fn draw(&mut self, ui: &mut egui::Ui) -> bool {
        let glass = Rect::from_min_size(Pos2::new(GLOW, GLOW), GLASS);
        // Dragging anywhere but on a control moves the window — registered
        // before every control, so the controls, on top of it, get theirs.
        let drag = ui.interact(glass, ui.scope_id().with("drag"), Sense::drag());
        if drag.drag_started() {
            if let Some((x, y)) = super::window_origin(self.hwnd) {
                let (cx, cy) = super::cursor();
                self.drag = Some((cx - x, cy - y));
            }
        }
        self.paint_glass(ui, glass);
        self.tabs(ui, glass);
        let close = self.close_button(ui, glass);
        let content = Rect::from_min_size(glass.min + Vec2::new(0.0, TABS_H), Vec2::new(WIDTH, HEIGHT));
        let mut page = Page { ui, origin: content.min, y: PAD };
        match self.tab {
            0 => self.general(&mut page),
            1 => self.audio(&mut page),
            2 => self.dictation(&mut page),
            _ => self.dictionary(&mut page),
        }
        self.footer(page.ui, content);
        // The circle, Esc, or Ctrl+W (the Mac's ⌘W) close it.
        close
            || page.ui.ctx().input(|i| {
                i.key_pressed(Key::Escape) || (i.modifiers.command && i.key_pressed(Key::W)) || i.viewport().close_requested()
            })
    }

    /// For screenshots: which tab, and the window's size.
    pub fn select(&mut self, tab: usize) {
        self.tab = tab;
    }

    pub fn window_size() -> Vec2 {
        GLASS + Vec2::splat(2.0 * GLOW)
    }

    /// The glass: a dark wash over the blur behind. No light runs round it:
    /// on the Mac that only happens while something is listening.
    fn paint_glass(&mut self, ui: &mut egui::Ui, glass: Rect) {
        let painter = ui.painter();
        draw::shot_backdrop(painter, glass, CORNER, 1.0);
        painter.rect_filled(glass, CornerRadius::same(CORNER as u8), rgba(0.11, 0.11, 0.12, 0.62));
        draw::hairline(painter, glass, CORNER, 0.10);
    }

    /// The Mac's close button, its red circle with the cross, in the glass's
    /// top-right corner. Returns whether it was
    /// clicked.
    fn close_button(&mut self, ui: &mut egui::Ui, glass: Rect) -> bool {
        const DIAMETER: f32 = 12.0;
        let centre = Pos2::new(glass.right() - 20.0, glass.top() + 20.0);
        let resp = ui.interact(Rect::from_center_size(centre, Vec2::splat(20.0)), ui.scope_id().with("close"), Sense::click());
        let painter = ui.painter();
        painter.circle_filled(centre, DIAMETER / 2.0, rgba(1.0, 0.373, 0.341, 1.0));
        painter.circle_stroke(centre, DIAMETER / 2.0 - 0.25, Stroke::new(0.5, rgba(0.0, 0.0, 0.0, 0.18)));
        // The cross is always there, darker under the pointer.
        let s = Stroke::new(1.1, rgba(0.30, 0.0, 0.0, if resp.hovered() { 0.95 } else { 0.70 }));
        let d = 2.6;
        painter.line_segment([centre + Vec2::new(-d, -d), centre + Vec2::new(d, d)], s);
        painter.line_segment([centre + Vec2::new(-d, d), centre + Vec2::new(d, -d)], s);
        resp.clicked()
    }

    /// The tabs, icon over name, centred along the top of the glass.
    fn tabs(&mut self, ui: &mut egui::Ui, glass: Rect) {
        let label = FontId::proportional(13.0);
        let widths: Vec<f32> = TABS
            .iter()
            .map(|(name, _, _)| {
                let w = ui.painter().layout_no_wrap(name.to_string(), label.clone(), Color32::WHITE).size().x;
                (w + 16.0).max(55.0)
            })
            .collect();
        let gap = 4.0;
        let total: f32 = widths.iter().sum::<f32>() + gap * (TABS.len() as f32 - 1.0);
        let mut x = glass.center().x - total / 2.0;
        for (i, ((name, glyph, family), w)) in TABS.iter().zip(widths).enumerate() {
            let item = Rect::from_min_size(Pos2::new(x, glass.top() + 4.0), Vec2::new(w, 48.0));
            let resp = ui.interact(item, ui.scope_id().with(("tab", i)), Sense::click());
            let selected = self.tab == i;
            if selected {
                ui.painter().rect_filled(item, CornerRadius::same(8), rgba(1.0, 1.0, 1.0, 0.10));
            } else if resp.hovered() {
                ui.painter().rect_filled(item, CornerRadius::same(8), rgba(1.0, 1.0, 1.0, 0.05));
            }
            let colour = if selected { text() } else { rgba(0.60, 0.60, 0.62, 1.0) };
            ui.painter().text(
                Pos2::new(item.center().x, glass.top() + 20.0),
                Align2::CENTER_CENTER,
                *glyph,
                FontId::new(20.0, FontFamily::Name((*family).into())),
                colour,
            );
            ui.painter().text(Pos2::new(item.center().x, glass.top() + 40.0), Align2::CENTER_CENTER, *name, label.clone(), colour);
            if resp.clicked() {
                self.tab = i;
                if i == 1 {
                    self.microphones = voi_audio::inputs();
                }
            }
            x += w + gap;
        }
    }

    fn general(&mut self, page: &mut Page) {
        page.header("Soniox");
        page.begin_group();
        let mid = page.row("API key", ROW, true);
        let field = page.right(Vec2::new(FIELD, 22.0), mid);
        let resp = text_field(page.ui, field, &mut self.key, "Paste your key", true);
        self.key_focused = resp.has_focus();
        let mid = page.row("Languages", ROW, false);
        let field = page.right(Vec2::new(FIELD, 22.0), mid);
        let resp = token_field(page.ui, field, &mut self.languages);
        if resp.changed() {
            self.draft.languages = prefs::parse_list(&self.languages).iter().map(|l| prefs::language_code(l)).collect();
        }
        page.end_group();
        // A first start has no key: where to get one, as a link.
        if self.key.trim().is_empty() {
            page.link("No key yet? Get one at ", "console.soniox.com/signup", SIGN_UP);
        }
        page.hint("Get a key at console.soniox.com — it is stored on this computer only. Languages are the ones that may turn up in your audio, as names or codes: English, uk, ru.");

        page.header("Startup");
        page.begin_group();
        let mid = page.row("Launch at login", ROW, true);
        let at = page.right(Vec2::new(52.0, 22.0), mid);
        let mut on = self.login.wanted();
        if switch(page.ui, at, &mut on, "launch") {
            self.set_login(on);
        }
        page.end_group();
        page.hint("Voi starts in the notification area when you sign in, so the dictation key works from the first minute. The same switch is in Task Manager ▸ Startup apps.");

        // The one row the Mac does not have: the record hotkey, off unless
        // set.
        page.header("Recording");
        page.begin_group();
        let mid = page.row("Hotkey", ROW, true);
        let box_ = page.right(Vec2::new(200.0, 26.0), mid);
        let capturing = hook::capturing();
        let shown = if capturing {
            "Press the key or keys…".to_string()
        } else {
            self.draft.record_hotkey.map_or("Off".to_string(), |h| h.to_string())
        };
        let resp = popup_like(page.ui, box_, &shown, false, "hotkey");
        if resp.double_clicked() {
            hook::capture(false);
            self.draft.record_hotkey = None;
            self.hotkey_note = None;
        } else if resp.clicked() {
            hook::capture(!capturing);
            self.hotkey_note = None;
        }
        if self.draft.record_hotkey.is_some() {
            let reset = Rect::from_min_size(Pos2::new(box_.left() - 26.0, box_.top() + 3.0), Vec2::splat(20.0));
            if reset_button(page.ui, reset, "Off", "hotkey-reset") {
                hook::capture(false);
                self.draft.record_hotkey = None;
            }
        }
        page.end_group();
        let note = match &self.hotkey_note {
            Some((_, note)) => note.clone(),
            None => "Starts and stops a recording from anywhere; while it is set, no other program receives it. Click the box, then press the key or keys.".to_string(),
        };
        page.hint(&note);
    }

    fn audio(&mut self, page: &mut Page) {
        page.header("Input");
        page.begin_group();
        let mid = page.row("Microphone", ROW, true);
        let at = page.right(Vec2::new(FIELD, 26.0), mid);
        let (names, default) = self.microphones.clone();
        let system = match &default {
            Some(name) => format!("System default ({name})"),
            None => "System default".to_string(),
        };
        let mut items: Vec<(Option<String>, String)> = vec![(None, system)];
        items.extend(names.iter().map(|n| (Some(n.clone()), n.clone())));
        if let Some(chosen) = self.draft.microphone.clone().filter(|c| !names.contains(c)) {
            items.push((Some(chosen.clone()), format!("{chosen} (not connected)")));
        }
        let shown = items.iter().find(|(v, _)| *v == self.draft.microphone).map_or(items[0].1.clone(), |(_, l)| l.clone());
        if let Some(pick) = popup(page.ui, at, &shown, &items.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>(), "microphone") {
            self.draft.microphone = items[pick].0.clone();
        }
        let mid = page.row("Start and stop sounds", ROW, false);
        let at = page.right(Vec2::new(52.0, 22.0), mid);
        switch(page.ui, at, &mut self.draft.sounds, "sounds");
        page.end_group();
        page.hint("Used for dictation and for your side of a call. Bluetooth headphones switch from music to headset mode whenever their microphone opens, which interrupts what is playing and takes a second or two — choose the computer's own microphone to keep them for listening only. Dictation sounds tell you it heard the key while you look at what you are typing into.");
    }

    fn dictation(&mut self, page: &mut Page) {
        page.header("Shortcut");
        page.begin_group();
        let mid = page.row("Key", ROW, true);
        let at = page.right(Vec2::new(200.0, 26.0), mid);
        let labels: Vec<String> = KEYS.iter().map(|(_, l)| l.to_string()).collect();
        if let Some(pick) = popup(page.ui, at, key_label(self.draft.dictation_key), &labels, "key") {
            self.draft.dictation_key = KEYS[pick].0;
        }
        let mid = page.row("Mode", ROW, false);
        let at = page.right(Vec2::new(200.0, 26.0), mid);
        let modes = ["Hold to talk".to_string(), "Click on, click off".to_string()];
        let shown = match self.draft.dictation_mode {
            DictationMode::PushToTalk => &modes[0],
            DictationMode::Toggle => &modes[1],
        };
        if let Some(pick) = popup(page.ui, at, shown, &modes, "mode") {
            self.draft.dictation_mode = if pick == 0 { DictationMode::PushToTalk } else { DictationMode::Toggle };
        }
        page.end_group();
        page.hint("Speak while it listens; it is typed where the cursor is when it stops. Let go with the pointer on the window to throw it away instead.");

        page.header("Window");
        page.begin_group();
        let mid = page.row("Follow pointer", ROW, true);
        let at = page.right(Vec2::new(52.0, 22.0), mid);
        switch(page.ui, at, &mut self.draft.dictation_follow_pointer, "follow");
        let mid = page.row("Position on screen", 70.0 + 20.0, false);
        let at = page.right(Vec2::new(112.0, 70.0), mid);
        spot_grid(page.ui, at, &mut self.draft.dictation_spot);
        let mid = page.row("Distance from the edge", ROW, false);
        let at = page.right(Vec2::new(200.0, 22.0), mid);
        slider(page.ui, at, &mut self.draft.dictation_margin, 6.0..=220.0, "margin");
        if (self.draft.dictation_margin - DEFAULT_MARGIN).abs() >= 0.5 {
            let reset = Rect::from_min_size(Pos2::new(at.left() - 26.0, at.top() + 1.0), Vec2::splat(20.0));
            if reset_button(page.ui, reset, &format!("Back to {:.0} pt", DEFAULT_MARGIN), "margin-reset") {
                self.draft.dictation_margin = DEFAULT_MARGIN;
            }
        }
        page.end_group();
        page.hint("Above the pointer and to its right; left of it or under it where the screen runs out. Never on it — the pointer on the window throws a dictation away.");
    }

    fn dictionary(&mut self, page: &mut Page) {
        page.header("Words to expect");
        page.hint("Names, jargon and product names that come up in your calls. Commas or new lines between them. A hint, not a filter: nothing is forced to match.");
        page.y -= 8.0;
        let top = page.y;
        let bottom = HEIGHT - (FOOTER + 26.0);
        let group = Rect::from_min_max(page.at(PAD, top), page.at(PAD + GROUP, bottom));
        group_box(page.ui, group);
        let inner = group.shrink(6.0);
        let mut child = page.ui.new_child(egui::UiBuilder::new().max_rect(inner));
        egui::ScrollArea::vertical().auto_shrink([false, false]).scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR | egui::scroll_area::ScrollSource::MOUSE_WHEEL).show(&mut child, |ui| {
            let edit = egui::TextEdit::multiline(&mut self.terms)
                .frame(egui::Frame::NONE)
                .font(FontId::proportional(13.0))
                .text_color(text())
                .margin(egui::Margin::symmetric(4, 6))
                .desired_width(f32::INFINITY)
                .desired_rows(16);
            if ui.add(edit).changed() {
                self.draft.terms = prefs::parse_list(&self.terms);
            }
        });
        let used: usize = self.draft.terms.iter().map(|t| t.chars().count() + 2).sum();
        let n = self.draft.terms.len();
        let label = format!("{n} term{} · {used} of about {} characters", if n == 1 { "" } else { "s" }, prefs::MAX_CONTEXT_CHARS);
        let colour = if used > prefs::MAX_CONTEXT_CHARS { Color32::from_rgb(255, 69, 58) } else { secondary() };
        let y = HEIGHT - (FOOTER + 26.0) + 6.0;
        page.ui.painter().text(page.at(PAD + GROUP - 4.0, y + 7.0), Align2::RIGHT_CENTER, label, FontId::proportional(11.0), colour);
    }

    fn footer(&self, ui: &mut egui::Ui, content: Rect) {
        let status = Rect::from_min_size(content.min + Vec2::new(24.0, HEIGHT - 10.0 - (FOOTER - 16.0)), Vec2::new(GROUP - 190.0, FOOTER - 16.0));
        let galley = ui.painter().layout(self.status.clone(), FontId::proportional(11.0), secondary(), status.width());
        ui.painter().with_clip_rect(status).galley(status.min, galley, secondary());
        ui.painter().text(
            content.min + Vec2::new(PAD + GROUP - 4.0, HEIGHT - 10.0 - 7.0),
            Align2::RIGHT_CENTER,
            super::version_line(),
            FontId::proportional(10.0),
            tertiary(),
        );
    }

    fn set_login(&mut self, on: bool) {
        self.status = match login::set(on) {
            Ok(status) => {
                self.login = status;
                match status {
                    login::Status::On => "Voi will launch at login.".into(),
                    login::Status::Off => "Voi will not launch at login.".into(),
                    login::Status::Disabled => "Allow Voi in Task Manager ▸ Startup apps to finish turning on launch at login.".into(),
                }
            }
            Err(e) => format!("Could not change launch at login: {e}."),
        };
        self.login = login::status();
    }

    fn take_hotkey(&mut self, hotkey: Hotkey) {
        match hotkey.verdict() {
            Verdict::Refuse(why) => self.hotkey_note = Some((Color32::from_rgb(255, 69, 58), format!("Not set: {why}"))),
            Verdict::Warn(why) => {
                self.draft.record_hotkey = Some(hotkey);
                self.hotkey_note = Some((Color32::from_rgb(255, 159, 10), why));
            }
            Verdict::Ok => {
                self.draft.record_hotkey = Some(hotkey);
                self.hotkey_note = None;
            }
        }
    }

    /// The Mac's autosave poll: the key once the cursor has left it, then
    /// everything else, with a receipt in the footer.
    fn autosave(&mut self) -> Option<Prefs> {
        if !self.key_focused {
            let typed = self.key.trim().to_string();
            if !typed.is_empty() && Some(&typed) != self.key_seen.as_ref() {
                match voi_core::save_api_key(&typed) {
                    Ok(()) => {
                        self.key_seen = Some(typed);
                        self.status = "Key saved.".into();
                    }
                    Err(e) => self.status = format!("Could not save the key: {e}"),
                }
                return None;
            }
        }
        if self.draft == self.last {
            return None;
        }
        let count: usize = self.draft.terms.iter().map(|t| t.chars().count() + 2).sum();
        if count > prefs::MAX_CONTEXT_CHARS {
            self.status = format!("The dictionary is {count} characters; Soniox takes about {}. Not saved.", prefs::MAX_CONTEXT_CHARS);
            return None;
        }
        match prefs::save(&self.draft) {
            Ok(()) => {
                let back = prefs::load();
                self.status = confirmation(&back);
                self.last = back.clone();
                self.draft = back.clone();
                Some(back)
            }
            Err(e) => {
                self.status = format!("Could not save: {e}");
                None
            }
        }
    }

    #[allow(dead_code)]
    pub fn age(&self) -> std::time::Duration {
        self.opened.elapsed()
    }
}

/// The receipt: what is now in force, in one line (mac/settings.rs).
fn confirmation(p: &Prefs) -> String {
    let languages = if p.languages.is_empty() { "any language".to_string() } else { prefs::join_list(&p.languages) };
    let n = p.terms.len();
    let mic = p.microphone.clone().unwrap_or_else(|| "default microphone".into());
    let placement = if p.dictation_follow_pointer { "beside the pointer".to_string() } else { p.dictation_spot.label().to_lowercase() };
    format!(
        "Saved: {languages}, {n} term{}, {mic}, {} {placement}, {:.0}pt in, sound {}.",
        if n == 1 { "" } else { "s" },
        key_label(p.dictation_key),
        p.dictation_margin,
        if p.sounds { "on" } else { "off" }
    )
}

/// The Mac's `Page`: a cursor moving down the content area.
struct Page<'a> {
    ui: &'a mut egui::Ui,
    origin: Pos2,
    y: f32,
}

impl Page<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.origin + Vec2::new(x, y)
    }

    fn header(&mut self, title: &str) {
        let at = self.at(PAD + 4.0, self.y);
        self.ui.painter().text(at, Align2::LEFT_TOP, title, FontId::new(13.0, FontFamily::Name("semibold".into())), text());
        self.y += 17.0 + 8.0;
    }

    fn begin_group(&mut self) {
        self.ui.data_mut(|d| d.insert_temp(egui::Id::unique("group-top"), self.y));
    }

    /// A row: the separator above it unless it is the first, the label, and
    /// the row's middle for the control.
    fn row(&mut self, label: &str, height: f32, first: bool) -> f32 {
        if !first {
            let a = self.at(PAD + INSET, self.y);
            self.ui.painter().line_segment([a, a + Vec2::new(GROUP - INSET, 0.0)], Stroke::new(1.0, hairline()));
        }
        let mid = self.y + height / 2.0;
        self.ui.painter().text(self.at(PAD + INSET, mid), Align2::LEFT_CENTER, label, FontId::proportional(13.0), text());
        self.y += height;
        mid
    }

    /// A control of `size`, right-aligned in the row whose middle is `mid`.
    fn right(&self, size: Vec2, mid: f32) -> Rect {
        let right = PAD + GROUP - INSET_RIGHT;
        Rect::from_min_size(self.at(right - size.x, mid - size.y / 2.0), size)
    }

    fn end_group(&mut self) {
        let top: f32 = self.ui.data(|d| d.get_temp(egui::Id::unique("group-top"))).unwrap_or(self.y);
        let rect = Rect::from_min_max(self.at(PAD, top), self.at(PAD + GROUP, self.y));
        group_box(self.ui, rect);
        self.y += 6.0;
    }

    /// `lead` and then `label` as a link to `url`, opened in the browser.
    fn link(&mut self, lead: &str, label: &str, url: &str) {
        let font = FontId::proportional(12.0);
        let painter = self.ui.painter().clone();
        let at = self.at(PAD + 4.0, self.y);
        let lead = painter.layout_no_wrap(lead.to_string(), font.clone(), secondary());
        let label_at = at + Vec2::new(lead.size().x, 0.0);
        let label = painter.layout_no_wrap(label.to_string(), font, accent());
        let rect = Rect::from_min_size(label_at, label.size());
        let resp = self.ui.interact(rect, self.ui.scope_id().with(("link", url)), Sense::click());
        let height = lead.size().y.max(label.size().y);
        painter.galley(at, lead, secondary());
        painter.galley(label_at, label, accent());
        if resp.hovered() {
            self.ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, accent()));
        }
        if resp.clicked() {
            let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
        }
        self.y += height + 6.0;
    }

    fn hint(&mut self, text_: &str) {
        let width = GROUP - 8.0;
        let galley = self.ui.painter().layout(text_.to_string(), FontId::proportional(11.0), secondary(), width);
        let height = galley.size().y.ceil();
        self.ui.painter().galley(self.at(PAD + 4.0, self.y), galley, secondary());
        self.y += height + 18.0;
    }
}

/// The Mac's group: radius 10, a half-point border, a faint fill — under the
/// rows, so drawn behind them.
fn group_box(ui: &mut egui::Ui, rect: Rect) {
    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("groups"));
    let painter = ui.ctx().layer_painter(layer).with_clip_rect(ui.clip_rect());
    painter.rect_filled(rect, CornerRadius::same(10), rgba(1.0, 1.0, 1.0, 0.04));
    painter.rect_stroke(rect, CornerRadius::same(10), Stroke::new(0.5, rgba(1.0, 1.0, 1.0, 0.13)), StrokeKind::Inside);
}

fn text_field(ui: &mut egui::Ui, rect: Rect, value: &mut String, placeholder: &str, password: bool) -> egui::Response {
    ui.painter().rect_filled(rect, CornerRadius::same(6), rgba(0.0, 0.0, 0.0, 0.22));
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, rgba(1.0, 1.0, 1.0, 0.12)), StrokeKind::Inside);
    let edit = egui::TextEdit::singleline(value)
        .frame(egui::Frame::NONE)
        .password(password)
        .font(FontId::proportional(13.0))
        .text_color(text())
        .hint_text(egui::RichText::new(placeholder).color(rgba(0.55, 0.55, 0.57, 1.0)))
        .margin(egui::Margin::symmetric(6, 3))
        .desired_width(rect.width());
    ui.put(rect, edit)
}

/// The Mac's token field: the languages as chips; plain text while edited.
fn token_field(ui: &mut egui::Ui, rect: Rect, value: &mut String) -> egui::Response {
    let id = egui::Id::unique("languages-field");
    let editing = ui.memory(|m| m.has_focus(id));
    if editing {
        ui.painter().rect_filled(rect, CornerRadius::same(6), rgba(0.0, 0.0, 0.0, 0.22));
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, accent()), StrokeKind::Inside);
        let edit = egui::TextEdit::singleline(value)
            .id(id)
            .frame(egui::Frame::NONE)
            .font(FontId::proportional(13.0))
            .text_color(text())
            .margin(egui::Margin::symmetric(6, 3))
            .desired_width(rect.width());
        return ui.put(rect, edit);
    }
    ui.painter().rect_filled(rect, CornerRadius::same(6), rgba(0.0, 0.0, 0.0, 0.22));
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, rgba(1.0, 1.0, 1.0, 0.12)), StrokeKind::Inside);
    let mut x = rect.left() + 10.0;
    for token in prefs::parse_list(value) {
        let galley = ui.painter().layout_no_wrap(token.clone(), FontId::proportional(13.0), text());
        let chip = Rect::from_min_size(Pos2::new(x, rect.top() + 4.0), Vec2::new(galley.size().x + 28.0, rect.height() - 8.0));
        ui.painter().rect_filled(chip, CornerRadius::same(3), Color32::from_rgb(40, 64, 95));
        ui.painter().galley(chip.center() - galley.size() / 2.0, galley, text());
        x = chip.right() + 6.0;
    }
    let resp = ui.interact(rect, id, Sense::click());
    if resp.clicked() {
        ui.memory_mut(|m| m.request_focus(id));
    }
    resp
}

/// The Mac's switch: a capsule track, a capsule knob, accent blue when on.
fn switch(ui: &mut egui::Ui, rect: Rect, on: &mut bool, id: &str) -> bool {
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let t = ui.ctx().animate_bool_with_time(ui.scope_id().with((id, "anim")), *on, 0.15);
    let track = if *on { accent() } else { rgba(1.0, 1.0, 1.0, 0.13) };
    ui.painter().rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), track);
    let knob_w = 32.0;
    let x = rect.left() + 2.0 + t * (rect.width() - knob_w - 4.0);
    let knob = Rect::from_min_size(Pos2::new(x, rect.top() + 2.0), Vec2::new(knob_w, rect.height() - 4.0));
    ui.painter().rect_filled(knob, CornerRadius::same((knob.height() / 2.0) as u8), rgba(0.88, 0.88, 0.88, 1.0));
    resp.clicked()
}

/// The look of the Mac's pop-up button: a rounded field with the choice and
/// the up-down chevrons.
fn popup_like(ui: &mut egui::Ui, rect: Rect, shown: &str, selected: bool, id: &str) -> egui::Response {
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click());
    let fill = if selected || resp.hovered() { rgba(1.0, 1.0, 1.0, 0.13) } else { rgba(1.0, 1.0, 1.0, 0.09) };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    let galley = ui.painter().layout(shown.to_string(), FontId::proportional(13.0), rgba(0.875, 0.875, 0.875, 1.0), rect.width() - 34.0);
    let galley_h = galley.size().y;
    ui.painter().with_clip_rect(rect.shrink2(Vec2::new(12.0, 0.0))).galley(Pos2::new(rect.left() + 12.0, rect.center().y - galley_h / 2.0), galley, Color32::WHITE);
    let c = Pos2::new(rect.right() - 14.0, rect.center().y);
    let s = Stroke::new(1.3, rgba(0.85, 0.85, 0.85, 1.0));
    ui.painter().line_segment([c + Vec2::new(-3.0, -2.0), c + Vec2::new(0.0, -5.0)], s);
    ui.painter().line_segment([c + Vec2::new(0.0, -5.0), c + Vec2::new(3.0, -2.0)], s);
    ui.painter().line_segment([c + Vec2::new(-3.0, 2.0), c + Vec2::new(0.0, 5.0)], s);
    ui.painter().line_segment([c + Vec2::new(0.0, 5.0), c + Vec2::new(3.0, 2.0)], s);
    resp
}

/// A pop-up button with its menu of `items`. Returns the one picked.
fn popup(ui: &mut egui::Ui, rect: Rect, shown: &str, items: &[String], id: &str) -> Option<usize> {
    let resp = popup_like(ui, rect, shown, false, id);
    let mut picked = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(rect.width());
        for (i, item) in items.iter().enumerate() {
            if ui.selectable_label(item == shown, item).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

/// The Mac's slider: a thin track, accent up to the knob, a capsule knob.
fn slider(ui: &mut egui::Ui, rect: Rect, value: &mut f64, range: std::ops::RangeInclusive<f64>, id: &str) {
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    let knob_w = 21.0;
    let track = Rect::from_min_max(Pos2::new(rect.left(), rect.center().y - 2.5), Pos2::new(rect.right(), rect.center().y + 2.5));
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            let t = ((p.x - rect.left() - knob_w / 2.0) / (rect.width() - knob_w)).clamp(0.0, 1.0);
            *value = lo + (hi - lo) * t as f64;
        }
    }
    let t = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
    let x = rect.left() + knob_w / 2.0 + t * (rect.width() - knob_w);
    ui.painter().rect_filled(track, CornerRadius::same(3), rgba(1.0, 1.0, 1.0, 0.13));
    ui.painter().rect_filled(Rect::from_min_max(track.min, Pos2::new(x, track.bottom())), CornerRadius::same(3), accent());
    let knob = Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::new(knob_w, 14.0));
    ui.painter().rect_filled(knob, CornerRadius::same(7), rgba(0.87, 0.87, 0.87, 1.0));
}

/// The Mac's ↺ (arrow.counterclockwise), with its tooltip.
fn reset_button(ui: &mut egui::Ui, rect: Rect, tip: &str, id: &str) -> bool {
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click()).on_hover_text(tip);
    let colour = if resp.hovered() { text() } else { secondary() };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "↺", FontId::new(15.0, FontFamily::Name("symbol".into())), colour);
    resp.clicked()
}

/// The Mac's spot grid (mac/spotgrid.rs): nine dots in a screen-shaped frame.
fn spot_grid(ui: &mut egui::Ui, rect: Rect, spot: &mut Spot) {
    let resp = ui.interact(rect, ui.scope_id().with("spots"), Sense::click());
    let frame = rect.shrink(0.5);
    ui.painter().rect_filled(frame, CornerRadius::same(6), rgba(0.5, 0.5, 0.55, 0.20));
    ui.painter().rect_stroke(frame, CornerRadius::same(6), Stroke::new(1.0, rgba(0.5, 0.5, 0.55, 0.45)), StrokeKind::Middle);
    let inset = 14.0;
    let dot = |i: usize| -> Pos2 {
        let (col, row) = (i % 3, i / 3);
        Pos2::new(
            rect.left() + inset + col as f32 * (rect.width() - 2.0 * inset) / 2.0,
            rect.top() + inset + row as f32 * (rect.height() - 2.0 * inset) / 2.0,
        )
    };
    if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.clicked()) {
        let nearest = (0..9).min_by(|a, b| (dot(*a) - p).length().total_cmp(&(dot(*b) - p).length())).unwrap();
        *spot = Spot::ALL[nearest];
    }
    for (i, s) in Spot::ALL.iter().enumerate() {
        if s == spot {
            ui.painter().circle_filled(dot(i), 4.5, accent());
        } else {
            ui.painter().circle_filled(dot(i), 2.5, rgba(0.55, 0.57, 0.62, 0.85));
        }
    }
}
