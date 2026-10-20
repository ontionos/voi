//! Voi menu-bar app.
//!
//! A status-bar item you click to start and stop recording. The recording
//! engine is the same `voi-core` recorder the CLI drives, running in-process
//! on a background tokio runtime — no subprocess, so macOS attributes the
//! microphone and system-audio permissions directly to this bundle.
//!
//! AppKit is main-thread-only. Everything here that touches UI runs on the main
//! thread; the worker only ever mutates the shared [`State`] behind its mutex,
//! and a one-second timer repaints from it.

use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadOnly};
use block2::RcBlock;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBezierPath, NSColor, NSImage, NSMenu,
    NSModalResponseOK, NSOpenPanel,
    NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength, NSWorkspace,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSObject, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize,
    NSString, NSTimer, NSURL,
};

/// Animation step. 60 fps, matching a standard display.
const FRAME_DT: f32 = 1.0 / 60.0;

use voi_core::{find_api_key, Session};

mod hotkey;
mod login;
mod meter;
mod overlay;
mod paste;
mod settings;
mod shot;
mod sound;
mod spotgrid;
mod sweep;
mod voice;
use overlay::Overlay;
use settings::Settings;
use crate::state::Shared;

use crate::engine::runtime;

struct Ivars {
    shared: Shared,
    overlay: std::cell::RefCell<Overlay>,
    /// The dictation window, and the key that opens it. Kept beside the
    /// overlay rather than inside it: two widgets, either of which can be up
    /// while the other works.
    voice: std::cell::RefCell<crate::mac::voice::Voice>,
    hotkey: std::cell::RefCell<hotkey::Hotkey>,
    /// Whether the dictation key has ever been seen down. Logged once, so
    /// "I press it and nothing happens" has an answer that does not need a
    /// debugger: either the key was seen or it was not.
    key_seen: std::cell::Cell<bool>,
    /// Whether a dictation was running last frame, so the mic watcher hears
    /// about each start and end once.
    dictating: std::cell::Cell<bool>,
    /// The saved settings, re-read whenever they are saved rather than on
    /// every frame — the dictation key is checked sixty times a second and
    /// must not read a file to do it.
    prefs: std::cell::RefCell<voi_core::prefs::Prefs>,
    settings: std::cell::RefCell<Option<Settings>>,
    overlay_item: std::cell::RefCell<Option<Retained<NSMenuItem>>>,
    hide_item: std::cell::RefCell<Option<Retained<NSMenuItem>>>,
    last_item: std::cell::RefCell<Option<Retained<NSMenuItem>>>,
    /// "Update Available", above Quit; hidden while there is none.
    update_item: std::cell::RefCell<Option<Retained<NSMenuItem>>>,
    /// Set when Quit was chosen while a recording was still flushing.
    quitting: std::cell::Cell<bool>,
    frames: std::cell::Cell<u32>,
    spin: std::cell::Cell<usize>,
    /// Shade the status dot was last drawn at.
    dot_alpha: std::cell::Cell<f64>,
    status_item: Retained<NSStatusItem>,
    header: Retained<NSMenuItem>,
    toggle: Retained<NSMenuItem>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "VoiController"]
    #[ivars = Ivars]
    struct Controller;

    impl Controller {
        #[unsafe(method(toggle:))]
        fn toggle_action(&self, _sender: Option<&AnyObject>) {
            crate::engine::toggle(&self.ivars().shared);
            self.refresh();
        }

        /// Open the transcript this run produced, not the folder around it.
        #[unsafe(method(openLastTranscript:))]
        fn open_last_transcript_action(&self, _sender: Option<&AnyObject>) {
            let path = self.ivars().shared.lock().unwrap().last_transcript.clone();
            if let Some(path) = path {
                open_path(&path.to_string_lossy());
            }
        }

        #[unsafe(method(openSessions:))]
        fn open_sessions_action(&self, _sender: Option<&AnyObject>) {
            if let Ok(root) = Session::default_root() {
                std::fs::create_dir_all(&root).ok();
                open_path(&root.to_string_lossy());
            }
        }

        /// Transcribe a recording that already exists — a dictaphone file, a
        /// voice memo — rather than listening live.
        #[unsafe(method(transcribeFile:))]
        fn transcribe_file_action(&self, _sender: Option<&AnyObject>) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let shared = self.ivars().shared.clone();

            let key = match find_api_key() {
                Ok(k) => k,
                Err(e) => {
                    shared.lock().unwrap().error = Some(e.to_string());
                    return;
                }
            };
            let Ok(root) = Session::default_root() else { return };

            let panel = NSOpenPanel::openPanel(mtm);
            panel.setAllowsMultipleSelection(false);
            panel.setCanChooseDirectories(false);
            panel.setMessage(Some(ns_string!("Choose a recording to transcribe")));
            NSApplication::sharedApplication(mtm).activate();
            if panel.runModal() != NSModalResponseOK {
                return;
            }
            let Some(url) = panel.URL() else { return };
            let Some(path) = url.path() else { return };
            let path = std::path::PathBuf::from(path.to_string());

            // Reveal rather than open: it shows the folder *and* the file, so
            // either is one click away.
            crate::engine::transcribe(&shared, key, root, path, |out| reveal(&out));
        }

        #[unsafe(method(openSettings:))]
        fn open_settings_action(&self, _sender: Option<&AnyObject>) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let mut slot = self.ivars().settings.borrow_mut();
            let settings = slot.get_or_insert_with(|| Settings::new(mtm));
            settings.show();
        }

        /// Download the newer build's DMG, in the browser.
        #[unsafe(method(openUpdate:))]
        fn open_update_action(&self, _sender: Option<&AnyObject>) {
            let update = self.ivars().shared.lock().unwrap().update.clone();
            if let Some(update) = update {
                tracing::info!(build = update.build, "downloading the update");
                open_url(&update.download);
            }
        }

        #[unsafe(method(openPrivacy:))]
        fn open_privacy_action(&self, _sender: Option<&AnyObject>) {
            open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture");
        }

        #[unsafe(method(copyTranscript:))]
        fn copy_transcript_action(&self, _sender: Option<&AnyObject>) {
            let text = self.ivars().shared.lock().unwrap().transcript_text();
            if text.is_empty() {
                return;
            }
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            unsafe { pb.setString_forType(&NSString::from_str(&text), NSPasteboardTypeString) };
            // Drives the "Copied ✓" flash in the overlay.
            self.ivars().shared.lock().unwrap().copied_at = Some(Instant::now());
        }

        /// Take the widget off screen entirely, and remember that.
        ///
        /// Not the same as unpinning it: unpinned, it still appears when
        /// something takes the microphone, which is the whole point of it.
        /// This is for when you want the machine to record and say nothing.
        #[unsafe(method(toggleHidden:))]
        fn toggle_hidden_action(&self, _sender: Option<&AnyObject>) {
            let hidden = {
                let mut s = self.ivars().shared.lock().unwrap();
                s.hidden = !s.hidden;
                s.hidden
            };
            let mut prefs = voi_core::prefs::load();
            prefs.hidden = hidden;
            if let Err(e) = voi_core::prefs::save(&prefs) {
                tracing::warn!(error = %e, "could not save the hidden setting");
            }
            self.refresh();
        }

        #[unsafe(method(toggleOverlay:))]
        fn toggle_overlay_action(&self, _sender: Option<&AnyObject>) {
            {
                let mut s = self.ivars().shared.lock().unwrap();
                s.overlay_pinned = !s.overlay_pinned;
            }
            self.refresh();
        }

        /// One second: clock and menu text.
        #[unsafe(method(tick:))]
        fn tick_action(&self, _sender: Option<&AnyObject>) {
            self.refresh();
        }

        /// 60 fps: meter animation and overlay contents. Kept separate from the
        /// one-second tick so the bars move smoothly without repainting the menu
        /// sixty times a second.
        #[unsafe(method(frame:))]
        fn frame_action(&self, _sender: Option<&AnyObject>) {
            let shared = &self.ivars().shared;
            {
                let mut s = shared.lock().unwrap();
                let target = if s.is_active() {
                    s.level_mic.max(s.level_system)
                } else {
                    0.0
                };
                s.meter.tick(target, FRAME_DT);
                s.meter.tick_phases(FRAME_DT);
            }
            self.ivars().overlay.borrow_mut().refresh(shared);
            self.dictate();

            // The menu bar only needs a fraction of the overlay's frame rate;
            // a breath is slow, and this redraw goes through AppKit.
            let n = self.ivars().frames.get().wrapping_add(1);
            self.ivars().frames.set(n);

            // Twice a second, and only while the panel is open: whatever it
            // says gets written, so there is no Save button to press and no
            // control that can be added without being saved.
            if n.is_multiple_of(30) {
                self.autosave_settings();
            }
            if n.is_multiple_of(4) {
                self.pulse_status_dot();
            }
            // A spinner at roughly eight frames a second, and the menu redrawn
            // with it — the point of a long job is seeing that it has not hung.
            if n.is_multiple_of(8) && self.ivars().shared.lock().unwrap().job.is_some() {
                self.ivars().spin.set(self.ivars().spin.get().wrapping_add(1));
                self.refresh();
            }
        }

        #[unsafe(method(quit:))]
        fn quit_action(&self, _sender: Option<&AnyObject>) {
            // Stop cleanly so the transcript and WAV headers are finalized, then
            // let the run loop keep turning — the timer terminates us once the
            // worker reports Idle. Blocking the main thread here would freeze
            // the UI and starve the very task we are waiting on.
            let active = self.ivars().shared.lock().unwrap().is_active();
            if active {
                self.stop_recording();
                self.ivars().quitting.set(true);
            } else {
                terminate();
            }
        }
    }
);

impl Controller {
    fn new(mtm: MainThreadMarker, shared: Shared) -> Retained<Self> {
        let status_item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);

        let header = menu_item(mtm, "Idle", None, "");
        header.setEnabled(false);

        let toggle = menu_item(mtm, "Start Recording", Some(sel!(toggle:)), "r");

        let this = Self::alloc().set_ivars(Ivars {
            overlay: std::cell::RefCell::new(Overlay::new(mtm, shared.clone())),
            voice: std::cell::RefCell::new(voice::Voice::new(mtm)),
            hotkey: Default::default(),
            key_seen: std::cell::Cell::new(false),
            dictating: std::cell::Cell::new(false),
            prefs: std::cell::RefCell::new(voi_core::prefs::load()),
            settings: std::cell::RefCell::new(None),
            overlay_item: std::cell::RefCell::new(None),
            hide_item: std::cell::RefCell::new(None),
            update_item: std::cell::RefCell::new(None),
            last_item: std::cell::RefCell::new(None),
            shared,
            quitting: std::cell::Cell::new(false),
            frames: std::cell::Cell::new(0),
            spin: std::cell::Cell::new(0),
            dot_alpha: std::cell::Cell::new(-1.0),
            status_item,
            header,
            toggle,
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // The overlay's play/stop button drives the same action as the menu.
        this.ivars().overlay.borrow().set_target(&this, mtm);
        this.build_menu(mtm);
        this.refresh();
        this
    }

    /// The controller is the action target for every item; it lives as long as
    /// the app, which is what `setTarget` requires (it holds an unretained ref).
    fn build_menu(&self, mtm: MainThreadMarker) {
        let ivars = self.ivars();
        let menu = NSMenu::init(NSMenu::alloc(mtm));

        menu.addItem(&ivars.header);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        unsafe { ivars.toggle.setTarget(Some(self)) };
        menu.addItem(&ivars.toggle);

        let transcribe = menu_item(mtm, "Transcribe File…", Some(sel!(transcribeFile:)), "t");
        unsafe { transcribe.setTarget(Some(self)) };
        menu.addItem(&transcribe);

        let last = menu_item(mtm, "Open Last Transcript", Some(sel!(openLastTranscript:)), "");
        unsafe { last.setTarget(Some(self)) };
        menu.addItem(&last);
        ivars.last_item.replace(Some(last));

        let sessions = menu_item(mtm, "Open Sessions Folder", Some(sel!(openSessions:)), "");
        unsafe { sessions.setTarget(Some(self)) };
        menu.addItem(&sessions);

        let overlay_item = menu_item(
            mtm,
            "Show Overlay",
            Some(crate::mac::overlay::toggle_selector()),
            "o",
        );
        unsafe { overlay_item.setTarget(Some(self)) };
        menu.addItem(&overlay_item);
        ivars.overlay_item.replace(Some(overlay_item));

        let hide_item = menu_item(mtm, "Hide Voi", Some(sel!(toggleHidden:)), "h");
        unsafe { hide_item.setTarget(Some(self)) };
        menu.addItem(&hide_item);
        ivars.hide_item.replace(Some(hide_item));

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let settings = menu_item(mtm, "Settings…", Some(sel!(openSettings:)), ",");
        unsafe { settings.setTarget(Some(self)) };
        menu.addItem(&settings);

        let privacy = menu_item(mtm, "Audio Permissions…", Some(sel!(openPrivacy:)), "");
        unsafe { privacy.setTarget(Some(self)) };
        menu.addItem(&privacy);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let update = menu_item(mtm, "Update Available", Some(sel!(openUpdate:)), "");
        unsafe { update.setTarget(Some(self)) };
        update.setHidden(true);
        menu.addItem(&update);
        ivars.update_item.replace(Some(update));

        let quit = menu_item(mtm, "Quit Voi", Some(sel!(quit:)), "q");
        unsafe { quit.setTarget(Some(self)) };
        menu.addItem(&quit);

        // Off, or AppKit decides enablement itself and ignores setEnabled.
        menu.setAutoenablesItems(false);
        ivars.status_item.setMenu(Some(&menu));
    }

    /// Repaint the status title and menu from shared state.
    fn refresh(&self) {
        let ivars = self.ivars();
        let (title, summary, toggle_title, dot_alpha, active) = {
            let s = ivars.shared.lock().unwrap();
            (
                s.clock_title(),
                s.summary(),
                s.toggle_title(),
                s.dot_alpha(),
                s.is_active(),
            )
        };

        if ivars.quitting.get() && !active {
            terminate();
            return;
        }

        let Some(mtm) = MainThreadMarker::new() else { return };
        if let Some(button) = ivars.status_item.button(mtm) {
            button.setTitle(&NSString::from_str(&title));
            // Only rebuilt when the shade changes; while recording the frame
            // tick redraws it instead, so the halo can breathe.
            if !active && (dot_alpha - ivars.dot_alpha.get()).abs() > 0.001 {
                ivars.dot_alpha.set(dot_alpha);
                button.setImage(Some(&status_dot(dot_alpha, None)));
            }
        }
        ivars.header.setTitle(&NSString::from_str(&decorate(&summary, ivars.spin.get())));
        if let Some(item) = ivars.last_item.borrow().as_ref() {
            // Asked of the folder, not of memory: the answer changes when you
            // delete a transcript, and it was previously only ever set by a
            // file transcription in this same run — so it sat grey with a
            // folder full of transcripts beside it.
            let latest = voi_core::latest_transcript();
            item.setEnabled(latest.is_some());
            ivars.shared.lock().unwrap().last_transcript = latest;
        }
        ivars.toggle.setTitle(&NSString::from_str(toggle_title));
        let (pinned, hidden) = {
            let s = ivars.shared.lock().unwrap();
            (s.overlay_pinned, s.hidden)
        };
        if let Some(item) = ivars.overlay_item.borrow().as_ref() {
            item.setTitle(&NSString::from_str(if pinned {
                "Unpin Overlay"
            } else {
                "Keep Overlay Visible"
            }));
            // Pinning something that cannot appear says nothing useful.
            item.setEnabled(!hidden);
        }
        if let Some(item) = ivars.hide_item.borrow().as_ref() {
            item.setTitle(&NSString::from_str(if hidden { "Show Voi" } else { "Hide Voi" }));
        }
        let update = ivars.shared.lock().unwrap().update.clone();
        if let Some(item) = ivars.update_item.borrow().as_ref() {
            item.setHidden(update.is_none());
            if let Some(update) = &update {
                item.setTitle(&NSString::from_str(&crate::updates::title(update)));
            }
        }
    }

    /// Watch the dictation key and drive the window it opens.
    ///
    /// Runs in the same frame loop as everything else: the key is read rather
    /// than intercepted, so there is nothing to install and nothing that can
    /// swallow a keystroke — see [`crate::mac::hotkey`].
    fn dictate(&self) {
        let ivars = self.ivars();
        let mut voice = ivars.voice.borrow_mut();

        let down = hotkey::Hotkey::pressed(ivars.prefs.borrow().dictation_key);
        if down && !ivars.key_seen.replace(true) {
            tracing::info!("dictation key seen");
        }
        let trigger = {
            let mut key = ivars.hotkey.borrow_mut();
            key.poll(
                down,
                Instant::now(),
                ivars.prefs.borrow().dictation_mode,
                voice.listening(),
            )
        };
        match trigger {
            Some(hotkey::Trigger::Start) => {
                let Some(mtm) = MainThreadMarker::new() else { return };
                match find_api_key() {
                    Ok(key) => {
                        // Inside the runtime, because starting a stream spawns
                        // tasks — and `tokio::spawn` outside a runtime panics.
                        // This runs from a timer callback on the main thread,
                        // where there is no runtime unless one is entered, and
                        // a panic through an Objective-C frame takes the whole
                        // app with it. Which is what "I press the key and
                        // nothing happens" was: it had died on the spot.
                        tracing::info!("dictation starting");
                        let _enter = runtime().enter();
                        voice.start(&key, &ivars.prefs.borrow(), mtm);
                        // At once, with the window: the microphone is still
                        // opening, and a Bluetooth one can take a second or
                        // two. If it fails, the window says so.
                        crate::mac::sound::play(sound::Note::Start, ivars.prefs.borrow().sounds);
                    }
                    Err(e) => {
                        ivars.shared.lock().unwrap().error = Some(e.to_string());
                        tracing::warn!(error = %e, "dictation needs a key");
                    }
                }
            }
            Some(hotkey::Trigger::Stop) => {
                // Letting go with the pointer on the window means the opposite
                // of letting go anywhere else: throw it away rather than type
                // it. The window has been saying so in red the whole time the
                // pointer has been there.
                if voice.hovered() {
                    voice.cancel();
                } else {
                    tracing::info!("dictation stopping");
                    voice.finish();
                    // On release, not when the words finally arrive a second
                    // and a half later: the note answers "did it hear me let
                    // go", and an answer that late is not an answer.
                    crate::mac::sound::play(sound::Note::End, ivars.prefs.borrow().sounds);
                }
            }
            None => {}
        }
        voice.refresh();
        // Told when it changes, not every frame: the watcher has to know a
        // dictation of ours is under way, or the system taking the microphone
        // back after it reads as a call starting.
        let on = voice.listening();
        if ivars.dictating.replace(on) != on {
            ivars.shared.lock().unwrap().mic_watch.set_dictating(on, Instant::now());
        }
    }

    /// Write anything the settings panel has changed, and pick it up here.
    ///
    /// The prefs the frame loop reads are a copy, refreshed when the file is
    /// written rather than re-read sixty times a second — the dictation key is
    /// checked on every frame and must not touch the disk to do it.
    fn autosave_settings(&self) {
        let ivars = self.ivars();
        let saved = {
            let slot = ivars.settings.borrow();
            let Some(settings) = slot.as_ref() else { return };
            match settings.autosave() {
                Some(message) => {
                    settings.report(&message);
                    true
                }
                None => false,
            }
        };
        if saved {
            *ivars.prefs.borrow_mut() = voi_core::prefs::load();
            ivars.shared.lock().unwrap().hidden = ivars.prefs.borrow().hidden;
        }
    }

    /// Redraw the menu-bar dot so its halo breathes while recording.
    fn pulse_status_dot(&self) {
        let ivars = self.ivars();
        let (active, pulse, alpha) = {
            let s = ivars.shared.lock().unwrap();
            (s.is_active(), s.meter.pulse, s.dot_alpha())
        };
        if !active {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else { return };
        if let Some(button) = ivars.status_item.button(mtm) {
            button.setImage(Some(&status_dot(alpha, Some(pulse))));
        }
        // Force the idle path to redraw once recording stops.
        ivars.dot_alpha.set(-1.0);
    }

    fn stop_recording(&self) {
        crate::engine::stop(&self.ivars().shared);
    }
}

/// Build a menu item. The AppKit initialiser is unsafe only because it can take
/// an arbitrary selector; every call site here passes one we define.
fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<objc2::runtime::Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

/// The menu-bar dot: a red circle, drawn rather than borrowed from SF Symbols.
///
/// Not a template image. A template is recoloured to match the menu bar, which
/// would turn it black or white — and the whole point is that it is red.
///
/// A status item's image is just an image, so it can be replaced as often as
/// you like; that is all an animation is here. `pulse` breathes a halo around
/// the dot while recording.
fn status_dot(alpha: f64, pulse: Option<f32>) -> Retained<NSImage> {
    const SIZE: f64 = 18.0;
    const RADIUS: f64 = 4.4;
    // Red while recording, white otherwise — the colour is what says it is
    // live, so it cannot also be there when it is not. Idle, the dot is drawn
    // as a template image below, which macOS renders in the menu bar's own
    // foreground colour: white on a dark bar, black on a light one, so "white"
    // does not mean invisible for anyone running the light theme.
    let live = pulse.is_some();
    let (cr, cg, cb) = if live { (0.93, 0.24, 0.20) } else { (1.0, 1.0, 1.0) };

    let handler = RcBlock::new(move |_: NSRect| {
        let c = SIZE / 2.0;

        if let Some(pulse) = pulse {
            // Smooth 0→1→0 breathing, with no hard turn at either end.
            let phase = 0.5 - 0.5 * (pulse as f64 * std::f64::consts::TAU).cos();
            const RINGS: usize = 8;
            for i in (0..RINGS).rev() {
                let t = i as f64 / (RINGS - 1) as f64;
                let r = RADIUS + 0.8 + t * (2.2 + 2.6 * phase);
                let a = 0.20 * (1.0 - t).powf(1.5) * (0.35 + 0.65 * phase);
                NSColor::colorWithSRGBRed_green_blue_alpha(cr, cg, cb, a).setFill();
                NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                    NSPoint::new(c - r, c - r),
                    NSSize::new(r * 2.0, r * 2.0),
                ))
                .fill();
            }
        }

        NSColor::colorWithSRGBRed_green_blue_alpha(cr, cg, cb, alpha).setFill();
        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(c - RADIUS, c - RADIUS),
            NSSize::new(RADIUS * 2.0, RADIUS * 2.0),
        ))
        .fill();
        objc2::runtime::Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(SIZE, SIZE), false, &handler);
    // Idle, let macOS colour it: a template image is drawn in the menu bar's
    // foreground colour, which is white on a dark bar and black on a light one.
    // Recording, the red is the point, so it is left alone.
    image.setTemplate(!live);
    image
}

/// Which build this is, as one line.
///
/// Read from the bundle rather than compiled in, because the parts that
/// distinguish two builds — the commit, the build number — are only known when
/// the bundle is assembled, and a build script that shells out to git on every
/// compile is a worse trade than reading a plist once.
fn version_line() -> String {
    let version =
        info("CFBundleShortVersionString").unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
    match (info("CFBundleVersion"), info("VoiCommit")) {
        (Some(build), Some(commit)) => format!("Voi {version} · build {build} · {commit}"),
        // Run straight from cargo rather than the bundle: the crate version is
        // all there is, and saying so is better than inventing the rest.
        _ => format!("Voi {version} (unpackaged)"),
    }
}

fn info(key: &str) -> Option<String> {
    let value = objc2_foundation::NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str(key))?;
    value.downcast::<NSString>().ok().map(|s| s.to_string())
}

fn terminate() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).terminate(None);
    }
}

fn open_url(url: &str) {
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// Put a turning spinner in front of a running job's description.
///
/// Only for text that describes work in progress; a resting state gets none.
fn decorate(summary: &str, spin: usize) -> String {
    const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
    match summary {
        "Idle" | "Recording" => summary.to_string(),
        _ => format!("{}  {summary}…", FRAMES[spin % FRAMES.len()]),
    }
}

/// Reveal a file in Finder, on the main thread.
fn reveal(path: &std::path::Path) {
    let path = path.to_string_lossy().to_string();
    dispatch2::DispatchQueue::main().exec_async(move || {
        NSWorkspace::sharedWorkspace()
            .selectFile_inFileViewerRootedAtPath(
                Some(&NSString::from_str(&path)),
                ns_string!(""),
            );
    });
}

fn open_path(path: &str) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    NSWorkspace::sharedWorkspace().openURL(&url);
}

/// Log to a file — a menu-bar app has no terminal to print to.
fn init_logging() {
    let Some(home) = dirs_home() else { return };
    let dir = home.join("Library").join("Logs");
    std::fs::create_dir_all(&dir).ok();
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("Voi.log"))
    {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "warn,voi=info,Voi=info".into()),
            )
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .try_init();
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

pub fn main() {
    // `Voi --launch-at-login on|off|status`: the Settings switch from a shell,
    // for installing on a Mac without clicking, and for checking it took.
    // Answers and exits before anything else starts — no menu bar item, no
    // hotkey, no second Voi beside the running one.
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `Voi --check-settings`: build the Settings window without showing it and
    // say how much room each tab has left above its footer. A layout that
    // overflows is caught here, not by somebody opening the window.
    if args.first().map(String::as_str) == Some("--check-settings") {
        let mtm = MainThreadMarker::new().expect("main thread");
        let settings = Settings::new(mtm);
        let mut fits = true;
        for (tab, room) in settings.spare() {
            println!("{tab}: {room:.0} pt to spare");
            fits &= *room >= 0.0;
        }
        std::process::exit(if fits { 0 } else { 1 });
    }
    // `Voi --screenshots DIR`: every window and state drawn into PNGs, none
    // of them shown — the Mac half of the comparison with Windows.
    if args.first().map(String::as_str) == Some("--screenshots") {
        let dir = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("screenshots"));
        std::process::exit(shot::run(&dir));
    }
    if args.first().map(String::as_str) == Some("--launch-at-login") {
        let result = match args.get(1).map(String::as_str) {
            Some("on") => login::set(true),
            Some("off") => login::set(false),
            Some("status") | None => Ok(login::status()),
            Some(other) => Err(format!("expected on, off or status, not {other:?}")),
        };
        match result {
            Ok(status) => println!("{status:?}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }

    init_logging();
    tracing::info!(build = %version_line(), "voi app starting");

    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    // Accessory: menu-bar presence, no Dock icon, no menu bar of its own.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let shared: Shared = Default::default();
    // What was chosen last time. Only the widget's own visibility lives here;
    // the languages and the dictionary are read by the recorder when a
    // recording starts, so changing them does not need a restart.
    let saved = voi_core::prefs::load();
    // Said out loud at startup: the only way to answer "which settings is it
    // actually running with" without guessing, for me or for anyone I hand
    // this to.
    tracing::info!(
        languages = %voi_core::prefs::join_list(&saved.languages),
        terms = saved.terms.len(),
        hidden = saved.hidden,
        "settings loaded"
    );
    shared.lock().unwrap().hidden = saved.hidden;
    // The menu shows what it finds on its next refresh, once a second.
    crate::updates::watch(shared.clone(), || {});
    // Visible from launch: the HUD is meant to sit on screen permanently, going
    // from a dim idle meter to a live one when recording starts.
    // Hidden until something starts listening. Poll rather than subscribe:
    // Core Audio's property listeners fire on its own threads, and a check this
    // cheap once a second is simpler to reason about than that.
    {
        let watch = shared.clone();
        // Polls in a row in which Core Audio had us taking input while every
        // capture of ours was already closed. One is a device still winding
        // down; three, over three seconds, is a microphone left on.
        let mut orphaned = 0u32;
        std::thread::spawn(move || loop {
            let listeners = voi_audio::watch::listeners();
            if let Ok(mut s) = watch.lock() {
                let me = std::process::id() as i32;
                s.mic_in_use = s.mic_watch.someone_else(&listeners, me, Instant::now());
            }
            if voi_audio::open_streams() == 0 && voi_audio::watch::we_are_listening() {
                orphaned += 1;
                if orphaned == 3 {
                    tracing::error!(
                        "microphone still open with no capture running — a stream was dropped without closing its device"
                    );
                }
            } else {
                orphaned = 0;
            }
            std::thread::sleep(std::time::Duration::from_millis(1200));
        });
    }
    let controller = Controller::new(mtm, shared.clone());

    // Both audio permissions now, not on the first click. macOS only
    // raises a prompt when a device is actually touched, so this opens each
    // source briefly and releases it. Off the main thread: it blocks, and the
    // run loop has to keep turning for the dialogs to appear.
    std::thread::spawn(move || {
        let result = voi_audio::permissions::request();
        if let Some(e) = &result.mic {
            tracing::warn!(error = %e, "microphone unavailable");
        }
        if let Some(e) = &result.system {
            tracing::warn!(error = %e, "system audio unavailable");
        }
        let mut s = shared.lock().unwrap();
        s.error = result
            .mic
            .map(|e| format!("Microphone: {e}"))
            .or_else(|| result.system.map(|e| format!("System audio: {e}")));
    });

    // Repaint once a second so the elapsed timer advances even while the menu
    // is closed and the status title stays current.
    unsafe {
        // Common modes, like the frame timer: a timer in the default mode
        // stops firing while a menu is tracking, so the menu froze at whatever
        // it said when it opened — you had to close and reopen it to see any
        // change.
        let slow_timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            1.0,
            &controller,
            sel!(tick:),
            None,
            true,
        );
        NSRunLoop::currentRunLoop().addTimer_forMode(&slow_timer, NSRunLoopCommonModes);
        // Not `scheduled…`: a timer added only to the default mode stops firing
        // while a menu is open or the panel is being dragged, which shows up as
        // the animation freezing mid-gesture. Common modes keeps it running.
        let frame_timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            FRAME_DT as f64,
            &controller,
            sel!(frame:),
            None,
            true,
        );
        NSRunLoop::currentRunLoop().addTimer_forMode(&frame_timer, NSRunLoopCommonModes);
    }

    app.run();
}

#[cfg(test)]
mod tests {
    use super::decorate;

    /// A resting state is not work in progress, so it gets no spinner.
    #[test]
    fn only_work_in_progress_spins() {
        assert_eq!(decorate("Idle", 0), "Idle");
        assert_eq!(decorate("Recording", 3), "Recording");
    }

    /// The whole point is that it visibly turns: the same text at successive
    /// ticks has to differ, or a stalled job looks identical to a busy one.
    #[test]
    fn the_spinner_advances() {
        let a = decorate("Transcribing", 0);
        let b = decorate("Transcribing", 1);
        assert_ne!(a, b);
        assert!(a.contains("Transcribing…"));
        // And it wraps rather than running off the end of the frame list.
        assert_eq!(decorate("Transcribing", 0), decorate("Transcribing", 8));
    }
}
