//! Voi on Windows: a tray icon, the floating HUD, and a Settings window.
//!
//! Drawn with egui (through eframe and winit) rather than with Win32 controls:
//! the HUD is a custom-drawn translucent shape either way, and Settings needs
//! nothing a native dialog would do better. What has to be Windows itself is
//! done directly — the keyboard hook ([`hook`]), the window styles that keep
//! the HUD from taking focus, the startup entry ([`login`]).
//!
//! Threads: the UI runs on the main thread; the recording runs on the tokio
//! runtime and writes into [`Shared`]; the keyboard hook has its own thread;
//! a watcher polls whether another app is using the microphone. None of them
//! wait on the UI — a hotkey press starts a recording even while the HUD is
//! not on screen.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, ViewportBuilder, ViewportCommand};
use voi_core::hotkey::Hotkey;
use voi_core::prefs::{self, Prefs};

use crate::state::{Mode, Shared};

mod glass;
mod draw;
mod hook;
mod hud;
mod login;
mod paste;
mod settings;
mod shot;
mod sound;
mod tray;
mod voice;

use hud::Hud;
use settings::Settings;
use tray::{Command, Tray};


/// What the command line can ask for besides running.
#[derive(Default)]
struct Args {
    /// Write screenshots of the HUD and Settings here, then quit.
    screenshots: Option<PathBuf>,
    /// Open Settings with the hotkey box already waiting for a key: the
    /// first step of the hotkey round-trip test, without a mouse.
    capture_hotkey: bool,
    settings: bool,
    /// Start with the overlay kept on screen, as the tray's "Keep Overlay
    /// Visible" does.
    pin: bool,
}

pub fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    // `Voi.exe --launch-at-login on|off|status`, as on the Mac.
    if raw.first().map(String::as_str) == Some("--launch-at-login") {
        let result = match raw.get(1).map(String::as_str) {
            Some("on") => login::set(true),
            Some("off") => login::set(false),
            Some("status") | None => Ok(login::status()),
            Some(other) => Err(format!("expected on, off or status, not {other:?}")),
        };
        attach_console();
        match result {
            Ok(status) => println!("{status:?}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let mut args = Args::default();
    let mut it = raw.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshots" => args.screenshots = it.next().map(PathBuf::from),
            "--capture-hotkey" => args.capture_hotkey = true,
            "--settings" => args.settings = true,
            "--pin" => args.pin = true,
            _ => {}
        }
    }

    init_logging();
    tracing::info!(build = %version_line(), "voi app starting");

    if args.screenshots.is_none() && !single_instance() {
        ring_the_running_one();
        tracing::info!("Voi is already running; leaving it be");
        return;
    }

    let saved = prefs::load();
    tracing::info!(
        languages = %prefs::join_list(&saved.languages),
        terms = saved.terms.len(),
        hidden = saved.hidden,
        hotkey = %saved.record_hotkey.map_or("none".into(), |h| h.to_string()),
        "settings loaded"
    );

    let shared: Shared = Default::default();
    shared.lock().unwrap().hidden = saved.hidden;
    shared.lock().unwrap().overlay_pinned = args.pin;

    let options = eframe::NativeOptions {
        // The main window draws nothing: it is one transparent pixel in the
        // corner of the screen that clicks pass through, there to keep the
        // frames coming. Windows paints only windows that are on a screen,
        // and the HUD, the dictation glass and Settings are each a window of
        // their own drawn in this one's frames.
        viewport: ViewportBuilder::default()
            .with_title("Voi")
            .with_icon(app_icon())
            .with_inner_size(egui::Vec2::splat(1.0))
            .with_position(egui::Pos2::ZERO)
            .with_mouse_passthrough(true)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_has_shadow(false)
            .with_visible(true),
        persist_window: false,
        ..Default::default()
    };

    let result = eframe::run_native(
        "Voi",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, shared, saved, args)))),
    );
    if let Err(e) = result {
        tracing::error!(error = %e, "the window could not be created");
    }
}

struct App {
    shared: Shared,
    /// The settings in force: edited by Settings, saved on every change, and
    /// replaced by what was read back from the file after each save.
    prefs: Prefs,
    hud: Hud,
    tray: Option<Tray>,
    settings: Option<Settings>,
    /// Where Settings was closed, to reopen there until Voi quits.
    settings_at: Option<egui::Pos2>,
    /// The main window.
    root: isize,
    /// A key Settings asked for and the hook caught, waiting to be read.
    captured: Arc<Mutex<Option<Hotkey>>>,
    /// The dictation window, and what its key has done since the last frame.
    voice: voice::Voice,
    press: crate::press::Hotkey,
    dictation_key: Arc<Mutex<Vec<(bool, Instant)>>>,
    hwnd: isize,
    quitting: bool,
    shots: Option<shot::Plan>,
    frames: u64,
    /// Frames Settings has been drawn for since it opened.
    settings_frames: u32,
    /// The menu's spinner frame, advanced while a job runs.
    spin: usize,
    /// Whether a dictation was running last frame, for the mic watcher.
    dictating: bool,
    /// Set when Voi is started again while running: open Settings.
    open_request: Arc<Mutex<bool>>,
    /// Read once at start, like the Mac's template image would be.
    light_taskbar: bool,
    test_settings_at: Option<Instant>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, shared: Shared, prefs: Prefs, args: Args) -> Self {
        let ctx = cc.egui_ctx.clone();
        fonts(&ctx);
        ctx.set_visuals(egui::Visuals::dark());

        let hwnd = window_handle(cc);
        paste::set_owner(hwnd);
        drop_raw_keyboard();
        // Never take focus, never sit in Alt+Tab or the taskbar: it is a HUD
        // over the call you are on, not a window you switch to.
        hud_styles(hwnd);

        let tray = match Tray::new(&ctx) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::error!(error = %e, "no tray icon");
                None
            }
        };

        // The hook, and a thread acting on what it reports — so a press
        // starts a recording whatever the UI happens to be doing.
        let captured: Arc<Mutex<Option<Hotkey>>> = Default::default();
        hook::bind(prefs.record_hotkey);
        hook::bind_dictation(hook::dictation_vk(prefs.dictation_key));
        let events = hook::install();
        let dictation_key: Arc<Mutex<Vec<(bool, Instant)>>> = Default::default();
        {
            let shared = shared.clone();
            let captured = captured.clone();
            let dictation_key = dictation_key.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                for event in events {
                    match event {
                        hook::Event::Pressed => {
                            tracing::info!("record hotkey pressed");
                            crate::engine::toggle(&shared);
                        }
                        hook::Event::Captured(key) => {
                            tracing::info!(hotkey = %key, "hotkey captured in settings");
                            *captured.lock().unwrap() = Some(key);
                        }
                        hook::Event::Dictation { down, at } => {
                            dictation_key.lock().unwrap().push((down, at));
                        }
                    }
                    ctx.request_repaint();
                }
            });
        }

        // Another app taking the microphone puts the HUD on screen, as on the
        // Mac — see `voi_audio::watch`. Not while taking screenshots: those
        // show the states they stage, not this computer's.
        draw::SHOT.store(args.screenshots.is_some(), std::sync::atomic::Ordering::Relaxed);
        if args.screenshots.is_none() {
            let repaint = ctx.clone();
            crate::updates::watch(shared.clone(), move || repaint.request_repaint());
            let shared = shared.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                // The same decision as the Mac's (voi_core::mic_watch): our
                // own process never counts, and a dictation of ours is known.
                let listeners = voi_audio::watch::listeners();
                let (changed, listening) = {
                    let mut s = shared.lock().unwrap();
                    let me = std::process::id() as i32;
                    let listening = s.mic_watch.someone_else(&listeners, me, Instant::now());
                    (std::mem::replace(&mut s.mic_in_use, listening) != listening, listening)
                };
                if changed {
                    tracing::info!(listening, "microphone in use by another app");
                    ctx.request_repaint();
                }
                std::thread::sleep(std::time::Duration::from_millis(1200));
            });
        }

        let mut app = Self {
            shared,
            prefs,
            hud: Hud::new(),
            tray,
            settings: None,
            settings_at: None,
            root: hwnd,
            captured,
            voice: voice::Voice::new(),
            press: Default::default(),
            dictation_key,
            hwnd,
            quitting: false,
            shots: None,
            frames: 0,
            settings_frames: 0,
            spin: 0,
            dictating: false,
            open_request: Default::default(),
            light_taskbar: tray::light_taskbar(),
            test_settings_at: std::env::var("VOI_TEST_SETTINGS_AFTER_MS")
                .ok()
                .and_then(|ms| ms.parse().ok())
                .map(|ms| Instant::now() + std::time::Duration::from_millis(ms)),
        };
        if args.settings || args.capture_hotkey {
            app.open_settings();
        }
        if args.capture_hotkey {
            hook::capture(true);
        }
        if let Some(dir) = args.screenshots {
            app.shots = Some(shot::Plan::new(dir));
        } else {
            listen_for_another_start(ctx.clone(), app.open_request.clone());
            // Not for a test running on a folder of its own: the entry is the
            // one the person uses.
            if std::env::var_os("VOI_DATA_DIR").is_none() {
                start_menu_entry();
            }
        }
        app
    }

    fn open_settings(&mut self) {
        if self.settings.is_none() {
            let mut settings = Settings::new(&self.prefs);
            // `VOI_TEST_SETTINGS_TAB=2` opens on that tab, for pictures of it.
            if let Some(tab) = std::env::var("VOI_TEST_SETTINGS_TAB").ok().and_then(|t| t.parse().ok()) {
                settings.select(tab);
            }
            self.settings = Some(settings);
        }
    }

    /// Write the settings and take back what the file now says — the round
    /// trip, on every change, so what is in force is what will load next time.
    fn save(&mut self) {
        match prefs::save(&self.prefs) {
            Ok(()) => {
                let back = prefs::load();
                if back != self.prefs {
                    tracing::error!("settings did not read back as written");
                }
                self.prefs = back;
                hook::bind(self.prefs.record_hotkey);
                hook::bind_dictation(hook::dictation_vk(self.prefs.dictation_key));
                self.shared.lock().unwrap().hidden = self.prefs.hidden;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not save settings");
            }
        }
    }

    fn command(&mut self, ctx: &egui::Context, command: Command) {
        match command {
            Command::Toggle => crate::engine::toggle(&self.shared),
            Command::TranscribeFile => self.transcribe_file(),
            Command::OpenLast => {
                if let Some(path) = voi_core::latest_transcript() {
                    open(&path);
                }
            }
            Command::OpenSessions => {
                if let Ok(root) = voi_core::Session::default_root() {
                    std::fs::create_dir_all(&root).ok();
                    open(&root);
                }
            }
            Command::KeepVisible => {
                let mut s = self.shared.lock().unwrap();
                s.overlay_pinned = !s.overlay_pinned;
            }
            Command::Hide => {
                self.prefs.hidden = !self.prefs.hidden;
                self.save();
            }
            Command::Settings => self.open_settings(),
            // The Mac's opens Privacy ▸ Audio Capture; this is Windows' own
            // page for the same thing (gap G6).
            Command::AudioPermissions => {
                let _ = std::process::Command::new("explorer.exe").arg("ms-settings:privacy-microphone").spawn();
            }
            // The newer build's setup, downloaded by the browser.
            Command::Update => {
                let update = self.shared.lock().unwrap().update.clone();
                if let Some(update) = update {
                    tracing::info!(build = update.build, "downloading the update");
                    let _ = std::process::Command::new("explorer.exe").arg(&update.download).spawn();
                }
            }
            Command::Quit => {
                // Stop cleanly so the transcript is finished, then go once the
                // worker reports idle.
                if self.shared.lock().unwrap().is_active() {
                    crate::engine::stop(&self.shared);
                    self.quitting = true;
                } else {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    fn transcribe_file(&mut self) {
        let key = match voi_core::find_api_key() {
            Ok(k) => k,
            Err(e) => {
                self.shared.lock().unwrap().error = Some(e.to_string());
                self.open_settings();
                return;
            }
        };
        let Ok(root) = voi_core::Session::default_root() else { return };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose a recording to transcribe")
            .add_filter("Audio", &["wav", "mp3", "m4a", "ogg", "opus", "flac", "aac", "webm", "mp4", "wma", "amr"])
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        crate::engine::transcribe(&self.shared, key, root, path, |out| reveal(&out));
    }

    /// What the dictation key just did, by the same rule as the Mac's.
    fn dictate(&mut self, ctx: &egui::Context, down: bool, at: Instant) {
        let trigger = self.press.poll(down, at, self.prefs.dictation_mode, self.voice.listening());
        match trigger {
            Some(crate::press::Trigger::Start) => match voi_core::find_api_key() {
                Ok(key) => {
                    tracing::info!("dictation starting");
                    // Starting a stream spawns tasks, which needs the runtime.
                    let _enter = crate::engine::runtime().enter();
                    self.voice.start(&key, &self.prefs, 1.0 / ctx.pixels_per_point());
                    sound::play(sound::Note::Start, self.prefs.sounds);
                }
                Err(e) => {
                    self.shared.lock().unwrap().error = Some(e.to_string());
                    tracing::warn!(error = %e, "dictation needs a key");
                }
            },
            Some(crate::press::Trigger::Stop) => {
                // Letting go with the pointer on the window throws it away.
                if self.voice.hovered() {
                    self.voice.cancel();
                } else {
                    tracing::info!("dictation stopping");
                    self.voice.finish();
                    sound::play(sound::Note::End, self.prefs.sounds);
                }
            }
            None => {}
        }
        ctx.request_repaint();
    }

    fn tray_view(&self) -> tray::View {
        let s = self.shared.lock().unwrap();
        tray::View {
            summary: s.summary(),
            spin: self.spin,
            toggle_title: s.toggle_title(),
            has_last: voi_core::latest_transcript().is_some(),
            pinned: s.overlay_pinned,
            hidden: s.hidden,
            recording: s.is_active(),
            stopping: s.mode == Mode::Stopping,
            pulse: s.meter.pulse,
            light_taskbar: self.light_taskbar,
            error: s.error.clone(),
            update: s.update.as_ref().map(crate::updates::title),
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(settings) = self.settings.as_mut() else { return };
        let captured = self.captured.lock().unwrap().take();
        let (_, work) = pointer_and_work_area(1.0 / ctx.pixels_per_point());
        let outcome = settings.show(ctx, captured, work, self.settings_at);
        self.settings_frames += 1;
        if self.settings_frames == 3 {
            tracing::info!("settings opened");
        }
        if let Some(saved) = outcome.saved {
            self.prefs = saved;
            hook::bind(self.prefs.record_hotkey);
            hook::bind_dictation(hook::dictation_vk(self.prefs.dictation_key));
            self.shared.lock().unwrap().hidden = self.prefs.hidden;
        }
        if outcome.close {
            tracing::info!("settings closed");
            self.settings_at = settings.place(ctx.pixels_per_point());
            self.settings_frames = 0;
            hook::capture(false);
            self.settings = None;
        }
    }

}

impl App {
    /// A frame of `--screenshots`: the stage being drawn, drawn into this
    /// window, and the picture taken once it has run its frames.
    fn shot_frame(&mut self, ui: &mut egui::Ui) {
        use crate::state::parity::{self, Shot, DICTATED};
        let ctx = ui.ctx().clone();
        let Some(plan) = self.shots.as_ref() else { return };
        let Some(stage) = plan.current() else {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        };
        let entering = plan.entering();
        let away = egui::Pos2::new(-10_000.0, -10_000.0);
        let elsewhere = |ctx: &egui::Context, size: egui::Vec2| {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::Pos2::new(40.0, 40.0)));
        };
        match stage {
            shot::Stage::Parity(which @ (Shot::DictationListening | Shot::DictationCancel)) => {
                if entering {
                    parity::stage(&self.shared, which);
                    elsewhere(&ctx, voice::Voice::window_size());
                    self.voice.stage(DICTATED.0, DICTATED.1, true, which == Shot::DictationCancel);
                }
                self.voice.tick_for_shot();
                self.voice.paint_into(ui.painter());
            }
            shot::Stage::Parity(which) => {
                if entering {
                    parity::stage(&self.shared, which);
                    elsewhere(&ctx, egui::Vec2::new(458.0, 263.0));
                }
                let (_, work) = pointer_and_work_area(1.0 / ctx.pixels_per_point());
                let _ = (work, away);
                self.hud.paint_for_shot(ui, &self.shared, which == Shot::HudOpen);
            }
            shot::Stage::Settings(tab) => {
                if entering {
                    parity::stage(&self.shared, Shot::DictationListening);
                    elsewhere(&ctx, Settings::window_size());
                    let mut settings = Settings::new(&self.prefs);
                    settings.select(tab);
                    self.settings = Some(settings);
                }
                if let Some(settings) = self.settings.as_mut() {
                    settings.draw(ui);
                }
            }
        }
        if let Some(plan) = self.shots.as_mut() {
            plan.step(&ctx);
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(at) = self.test_settings_at {
            if Instant::now() >= at {
                self.test_settings_at = None;
                tracing::info!("TEST opening settings as the tray would");
                self.command(ctx, Command::Settings);
            }
        }
        if std::mem::take(&mut *self.open_request.lock().unwrap()) {
            tracing::info!("started again: opening settings");
            self.open_settings();
        }
        let commands = self.tray.as_ref().map(|t| t.commands()).unwrap_or_default();
        for command in commands {
            self.command(ctx, command);
        }
        let presses: Vec<(bool, Instant)> = std::mem::take(&mut *self.dictation_key.lock().unwrap());
        for (down, at) in presses {
            self.dictate(ctx, down, at);
        }
        if self.quitting && !self.shared.lock().unwrap().is_active() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        self.frames += 1;
        // The spinner turns about eight times a second, and only while a job
        // takes time — as on the Mac.
        if self.frames % 8 == 0 && self.shared.lock().unwrap().job.is_some() {
            self.spin = self.spin.wrapping_add(1);
        }
        // The menu and icon fifteen times a second: the halo breathes, and the
        // text follows the recording's state.
        if self.frames % 4 == 1 {
            let view = self.tray_view();
            if let Some(t) = self.tray.as_mut() {
                t.refresh(&view);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.shots.is_some() {
            self.shot_frame(ui);
            return;
        }
        // `VOI_TEST_STAGE=hud-open|dictation-listening|…` holds the live app
        // in one of the comparison's states, blur window and all, so what is
        // on the screen can be checked against the Mac.
        let staged = std::env::var("VOI_TEST_STAGE").ok().and_then(|name| {
            crate::state::parity::SHOTS.iter().find(|(_, n)| *n == name).map(|(s, _)| *s)
        });
        if let Some(shot) = staged {
            use crate::state::parity::{self, Shot, DICTATED};
            if self.frames < 3 {
                parity::stage(&self.shared, shot);
                if matches!(shot, Shot::DictationListening | Shot::DictationCancel) {
                    let ppp = 1.0 / ctx.pixels_per_point();
                    self.voice.start_staged(&self.prefs, ppp);
                    self.voice.stage(DICTATED.0, DICTATED.1, true, shot == Shot::DictationCancel);
                }
            }
            if matches!(shot, Shot::DictationListening | Shot::DictationCancel) {
                self.voice.tick_for_shot();
                self.voice.show(&ctx);
            }
        }
        keep_root(self.root);
        let ppp = 1.0 / ctx.pixels_per_point();
        let (pointer, work) = pointer_and_work_area(ppp);
        self.hud.show(&ctx, &self.shared, work, pointer, left_button_down());

        if std::mem::take(&mut self.hud.dot_clicked) {
            crate::engine::toggle(&self.shared);
        }
        if std::mem::take(&mut self.hud.copy_clicked) {
            let text = self.shared.lock().unwrap().transcript_text();
            if paste::copy(&text) {
                self.shared.lock().unwrap().copied_at = Some(Instant::now());
            }
        }
        if std::mem::take(&mut self.hud.context_menu) {
            if let Some(t) = self.tray.as_ref() {
                use tray_icon::menu::ContextMenu;
                unsafe { t.hud_menu.show_context_menu_for_hwnd(self.hwnd, None) };
            }
        }

        let dt = ctx.input(|i| i.stable_dt);
        let staged_dictation = matches!(
            staged,
            Some(crate::state::parity::Shot::DictationListening | crate::state::parity::Shot::DictationCancel)
        );
        if !staged_dictation {
            self.voice.refresh(dt, ppp);
            // Told when it changes, as on the Mac.
            let on = self.voice.listening();
            if std::mem::replace(&mut self.dictating, on) != on {
                self.shared.lock().unwrap().mic_watch.set_dictating(on, Instant::now());
            }
            self.voice.show(&ctx);
        }

        self.settings_window(&ctx);

    }

}

/// The HUD's own window, for the styles winit has no names for.
fn window_handle(cc: &eframe::CreationContext<'_>) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

/// The pointer, and the usable part of the screen it is on, in points.
pub(crate) fn pointer_and_work_area(points_per_pixel: f32) -> (egui::Pos2, egui::Rect) {
    use egui::{Pos2, Rect};
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        let monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &mut info);
        let w = info.rcWork;
        let s = points_per_pixel;
        (
            Pos2::new(p.x as f32 * s, p.y as f32 * s),
            Rect::from_min_max(Pos2::new(w.left as f32 * s, w.top as f32 * s), Pos2::new(w.right as f32 * s, w.bottom as f32 * s)),
        )
    }
}

/// Whether any monitor shows any of `rect` (screen pixels).
pub(crate) fn on_a_screen(rect: windows_sys::Win32::Foundation::RECT) -> bool {
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromRect, MONITOR_DEFAULTTONULL};
    unsafe { !MonitorFromRect(&rect, MONITOR_DEFAULTTONULL).is_null() }
}

/// Whether the left button is down, wherever the pointer is.
pub(crate) fn left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0 }
}

/// No frame: Windows 11 draws a hairline border and rounds the corners of
/// every top-level window, borderless or not, and around a transparent
/// window that border is a faint box floating around the glass.
pub(crate) fn no_frame(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    const BORDER_COLOR: u32 = 34; // DWMWA_BORDER_COLOR
    const CORNER_PREFERENCE: u32 = 33; // DWMWA_WINDOW_CORNER_PREFERENCE
    const COLOR_NONE: u32 = 0xFFFF_FFFE;
    const DO_NOT_ROUND: u32 = 1;
    if hwnd == 0 {
        return;
    }
    unsafe {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        let size = std::mem::size_of::<u32>() as u32;
        DwmSetWindowAttribute(hwnd, BORDER_COLOR, (&COLOR_NONE as *const u32).cast(), size);
        DwmSetWindowAttribute(hwnd, CORNER_PREFERENCE, (&DO_NOT_ROUND as *const u32).cast(), size);
    }
}

/// Voi's icon, the Mac's (`assets/voi.ico`, also built into Voi.exe), for
/// the windows Windows shows it on: Alt+Tab and the like.
pub(crate) fn app_icon() -> Arc<egui::IconData> {
    static ICON: std::sync::OnceLock<Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| Arc::new(icon_from_ico(include_bytes!("../../../../assets/voi.ico"), 32).unwrap_or_default())).clone()
}

/// One size out of an .ico of PNG images.
fn icon_from_ico(ico: &[u8], want: u32) -> Option<egui::IconData> {
    let count = u16::from_le_bytes(ico.get(4..6)?.try_into().ok()?) as usize;
    let entry = (0..count).map(|i| ico.get(6 + 16 * i..22 + 16 * i)).find(|e| e.is_some_and(|e| e[0] as u32 == want))??;
    let len = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
    let at = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
    let mut reader = png::Decoder::new(std::io::Cursor::new(ico.get(at..at + len)?)).read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    rgba.truncate(info.buffer_size());
    Some(egui::IconData { rgba, width: info.width, height: info.height })
}

/// A window of this process, by its title.
pub(crate) fn find_window(title: &str) -> isize {
    find_window_impl(title)
}

/// A glass rectangle in a window at `origin` (points), as screen pixels.
pub(crate) fn screen_rect(origin: egui::Pos2, glass: egui::Rect, pixels_per_point: f32) -> windows_sys::Win32::Foundation::RECT {
    let p = |v: f32| (v * pixels_per_point).round() as i32;
    windows_sys::Win32::Foundation::RECT {
        left: p(origin.x + glass.left()),
        top: p(origin.y + glass.top()),
        right: p(origin.x + glass.right()),
        bottom: p(origin.y + glass.bottom()),
    }
}

/// The pointer, in screen pixels.
pub(crate) fn cursor() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// Put a window of ours at a place on the screen, in pixels, at once.
pub(crate) fn move_window(hwnd: isize, x: i32, y: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};
    if hwnd != 0 {
        unsafe { SetWindowPos(hwnd as windows_sys::Win32::Foundation::HWND, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }
}

/// A window of ours' top-left corner on the screen, in pixels.
pub(crate) fn window_origin(hwnd: isize) -> Option<(i32, i32)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    if hwnd == 0 {
        return None;
    }
    let mut r = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
    (unsafe { GetWindowRect(hwnd as windows_sys::Win32::Foundation::HWND, &mut r) } != 0).then_some((r.left, r.top))
}

/// Where a window of ours is on the screen, in points.
pub(crate) fn window_rect(hwnd: isize, pixels_per_point: f32) -> Option<egui::Rect> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    if hwnd == 0 {
        return None;
    }
    let mut r = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if unsafe { GetWindowRect(hwnd as windows_sys::Win32::Foundation::HWND, &mut r) } == 0 {
        return None;
    }
    let pt = |v: i32| v as f32 / pixels_per_point;
    Some(egui::Rect::from_min_max(egui::pos2(pt(r.left), pt(r.top)), egui::pos2(pt(r.right), pt(r.bottom))))
}

/// Only among this process's windows: another Voi running beside this one —
/// a test, a second data folder — has windows with the same titles, and
/// `FindWindowW` would hand back whichever comes first.
fn find_window_impl(title: &str) -> isize {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId};
    struct Search {
        title: Vec<u16>,
        me: u32,
        found: isize,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> i32 {
        let search = &mut *(data as *mut Search);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != search.me {
            return 1;
        }
        let mut buf = [0u16; 128];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        if buf[..n] == search.title[..] {
            search.found = hwnd as isize;
            return 0;
        }
        1
    }
    let mut search = Search { title: title.encode_utf16().collect(), me: unsafe { GetCurrentProcessId() }, found: 0 };
    unsafe { EnumWindows(Some(each), &mut search as *mut Search as LPARAM) };
    search.found
}

/// The main window without caption buttons either (see `glass::popup`).
/// Runs every frame and acts when it must.
fn keep_root(hwnd: isize) {
    glass::popup(hwnd);
}

fn hud_styles(hwnd: isize) {
    no_frame(hwnd);
    keep_root(hwnd);
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    };
    if hwnd == 0 {
        return;
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let style = (style | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST) & !WS_EX_APPWINDOW;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize);
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Take back the raw keyboard input winit signs every window up for.
///
/// A process registered for raw keyboard input stops getting its own
/// low-level keyboard hook called whenever one of its windows is in front —
/// so with Settings open, the hotkey and the "press a key" box both went
/// deaf, while working perfectly with any other window in front. winit only
/// uses raw keyboard input for device events, which egui never reads; its key
/// events come from ordinary window messages and are unaffected.
fn drop_raw_keyboard() {
    use windows_sys::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_REMOVE};
    let keyboard = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x06,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: std::ptr::null_mut(),
    };
    let ok = unsafe { RegisterRawInputDevices(&keyboard, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) };
    if ok == 0 {
        tracing::warn!("could not drop raw keyboard input; the hotkey will not work while a Voi window is in front");
    }
}

/// The faces the Mac's stand in for (gap G3): Segoe UI Variable for SF Pro,
/// a semibold Segoe for the Mac's semibold headers, Cascadia Mono for SF
/// Mono, Windows' own icon font for the toolbar's symbols, and Segoe UI
/// Symbol for ⌘ and ↺. egui's own fonts stay behind them as the last resort.
fn fonts(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    let dir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts");
    let mut load = |name: &str, files: &[&str]| -> bool {
        for file in files {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                defs.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
                return true;
            }
        }
        false
    };
    let ui_face = load("segoe", &["SegUIVar.ttf", "segoeui.ttf"]);
    let semibold = load("segoe-semibold", &["seguisb.ttf", "segoeuib.ttf"]);
    let mono = load("cascadia", &["CascadiaMono.ttf", "consola.ttf"]);
    let icons = load("icons", &["SegoeIcons.ttf", "segmdl2.ttf"]);
    let symbol = load("segoe-symbol", &["seguisym.ttf"]);
    let fallback: Vec<String> = defs.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    let mut family = |name: &str, first: &[(&str, bool)]| {
        let mut list: Vec<String> = first.iter().filter(|(_, ok)| *ok).map(|(n, _)| n.to_string()).collect();
        list.extend(fallback.iter().cloned());
        defs.families.insert(egui::FontFamily::Name(name.into()), list);
    };
    family("semibold", &[("segoe-semibold", semibold), ("segoe-symbol", symbol)]);
    family("icons", &[("icons", icons), ("segoe-symbol", symbol)]);
    family("symbol", &[("segoe-symbol", symbol), ("segoe", ui_face)]);
    if let Some(list) = defs.families.get_mut(&egui::FontFamily::Proportional) {
        let mut first: Vec<String> = Vec::new();
        if ui_face {
            first.push("segoe".into());
        }
        if symbol {
            first.push("segoe-symbol".into());
        }
        for (i, name) in first.into_iter().enumerate() {
            list.insert(i, name);
        }
    }
    if mono {
        if let Some(list) = defs.families.get_mut(&egui::FontFamily::Monospace) {
            list.insert(0, "cascadia".into());
        }
    }
    ctx.set_fonts(defs);
}

/// Only one Voi per settings folder: a second on the same folder would
/// install a second hook for the same keys and fight the first over every
/// press. A test pointed at a folder of its own (`VOI_DATA_DIR`) can run
/// beside the Voi in use.
fn single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = instance_name("VoiApp");
    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        // Leaked on purpose: held for as long as the process lives.
        !handle.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// A name for this Voi's settings folder, so a test on a folder of its own
/// does not meet the Voi in use.
fn instance_name(what: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    voi_core::data_dir().ok().hash(&mut hash);
    format!("Local\\{what}-{:016x}", hash.finish())
}

/// Starting Voi again while it runs opens its Settings, instead of doing
/// nothing anyone can see: the second one rings this and leaves.
fn ring_the_running_one() {
    use windows_sys::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    let name: Vec<u16> = instance_name("VoiOpen").encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if !event.is_null() {
            SetEvent(event);
        }
    }
}

/// And the running one listens for it.
fn listen_for_another_start(ctx: egui::Context, open: Arc<Mutex<bool>>) {
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    let name: Vec<u16> = instance_name("VoiOpen").encode_utf16().chain(std::iter::once(0)).collect();
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) } as isize;
    if event == 0 {
        return;
    }
    std::thread::spawn(move || loop {
        unsafe { WaitForSingleObject(event as windows_sys::Win32::Foundation::HANDLE, INFINITE) };
        *open.lock().unwrap() = true;
        ctx.request_repaint();
    });
}

/// Voi in the Start menu, under its own name, pointing at this copy — so it
/// can be found and started like anything else. Kept pointing at wherever Voi
/// was last started from.
fn start_menu_entry() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(programs) = std::env::var_os("APPDATA").map(|d| {
        PathBuf::from(d).join("Microsoft").join("Windows").join("Start Menu").join("Programs")
    }) else {
        return;
    };
    let link = programs.join("Voi.lnk");
    let marker = programs.join("Voi.lnk.target");
    let target = exe.display().to_string();
    // What the entry was made for: this Voi.exe, with its icon. An entry
    // made before Voi.exe had one is made again.
    let made_for = format!("{target}\nicon 1");
    if link.exists() && std::fs::read_to_string(&marker).is_ok_and(|t| t == made_for) {
        return;
    }
    let made = mslnk::ShellLink::new(&exe).and_then(|mut shortcut| {
        shortcut.set_icon_location(Some(target.clone()));
        shortcut.create_lnk(&link)
    });
    match made {
        Ok(()) => {
            let _ = std::fs::write(&marker, &made_for);
            tell_explorer(&link);
            tracing::info!(link = %link.display(), "start menu entry made");
        }
        Err(e) => tracing::warn!(error = %e, "could not make the start menu entry"),
    }
}

/// Have Explorer and the Start menu draw a changed entry again, with its
/// new icon, rather than the one they remember.
fn tell_explorer(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_UPDATEITEM, SHCNF_PATHW};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe { SHChangeNotify(SHCNE_UPDATEITEM as i32, SHCNF_PATHW, wide.as_ptr().cast(), std::ptr::null()) };
}

/// A GUI binary has no console; borrow the one it was started from, if any,
/// so `--launch-at-login status` can answer.
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn open(path: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

/// Show the file in its folder, selected.
fn reveal(path: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", path.display())).spawn();
}

fn version_line() -> String {
    format!("Voi {} for Windows", env!("CARGO_PKG_VERSION"))
}

/// `%LOCALAPPDATA%\Voi\Voi.log`: a tray app has no terminal to print to.
fn init_logging() {
    let Some(dir) = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Voi")) else { return };
    std::fs::create_dir_all(&dir).ok();
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("Voi.log")) {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "warn,voi=info,voi_app=info,voi_core=info,voi_audio=info".into()),
            )
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .try_init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_the_macs_red_dot() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (32, 32));
        let at = |x: usize, y: usize| &icon.rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        let centre = at(16, 16);
        assert!(centre[0] > 200 && centre[1] < 90 && centre[3] == 255, "centre {centre:?}");
        assert_eq!(at(0, 0)[3], 0, "the corner is clear");
    }
}
