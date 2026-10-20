//! The notification-area icon and its menu: the Windows counterpart of the
//! Mac's menu-bar item, with the same entries in the same order.
//!
//! Menu clicks arrive on the UI thread from inside Windows' message loop. They
//! are queued and the HUD is asked to repaint, so they are acted on in the
//! next frame along with everything else — and acted on even while the HUD is
//! off screen, because a repaint request runs the app's logic regardless.
//!
//! Windows 11 puts a new icon behind the ^ chevron, not on the taskbar, until
//! the user pins it; nothing a program may do changes that. So the icon is
//! Voi's own — the red dot on a white tile — to be told apart from the logos
//! around it there, where a bare white dot was not. Whether the shell really
//! holds it is checked on every refresh (`NIM_MODIFY` fails for an icon it
//! does not have), logged, and repaired by adding it again: Explorer
//! restarting is handled by tray-icon itself (`TaskbarCreated`), anything else
//! that drops it by this.

use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Toggle,
    TranscribeFile,
    OpenLast,
    OpenSessions,
    KeepVisible,
    Hide,
    Settings,
    AudioPermissions,
    Update,
    Quit,
}

const IDS: [(&str, Command); 12] = [
    ("permissions", Command::AudioPermissions),
    ("hud-current", Command::OpenLast),
    ("hud-sessions", Command::OpenSessions),
    ("toggle", Command::Toggle),
    ("file", Command::TranscribeFile),
    ("last", Command::OpenLast),
    ("sessions", Command::OpenSessions),
    ("pin", Command::KeepVisible),
    ("hide", Command::Hide),
    ("settings", Command::Settings),
    ("update", Command::Update),
    ("quit", Command::Quit),
];

pub struct Tray {
    icon: TrayIcon,
    /// Since when the shell has refused to update the icon, if it has.
    lost: Option<Instant>,
    /// When the icon was last added again, so a missing shell is not hammered.
    readded: Option<Instant>,
    pub menu: Menu,
    /// The HUD header's own menu, as on the Mac: the session and its folder.
    pub hud_menu: Menu,
    header: MenuItem,
    toggle: MenuItem,
    last: MenuItem,
    pin: MenuItem,
    hide: MenuItem,
    /// "Update Available", put just above Quit while there is one.
    update: MenuItem,
    update_shown: bool,
    rx: Receiver<Command>,
    /// What the icon was last drawn as, so it is only rebuilt on a change:
    /// recording or not, and the halo's step while recording.
    drawn: Option<(bool, u8)>,
}

impl Tray {
    pub fn new(ctx: &eframe::egui::Context) -> anyhow::Result<Self> {
        // The Mac's menu, item for item (mac/mod.rs build_menu). Its ⌘ keys are
        // Ctrl here, and work, as there, only while the menu is open.
        let key = |c: Code| Some(Accelerator::new(Modifiers::CONTROL, c));
        let header = MenuItem::with_id("header", "Idle", false, None);
        let toggle = MenuItem::with_id("toggle", "Start Recording", true, key(Code::KeyR));
        let file = MenuItem::with_id("file", "Transcribe File…", true, key(Code::KeyT));
        let last = MenuItem::with_id("last", "Open Last Transcript", false, None);
        let sessions = MenuItem::with_id("sessions", "Open Sessions Folder", true, None);
        let pin = MenuItem::with_id("pin", "Show Overlay", true, key(Code::KeyO));
        let hide = MenuItem::with_id("hide", "Hide Voi", true, key(Code::KeyH));
        let settings = MenuItem::with_id("settings", "Settings…", true, key(Code::Comma));
        let permissions = MenuItem::with_id("permissions", "Audio Permissions…", true, None);
        let quit = MenuItem::with_id("quit", "Quit Voi", true, key(Code::KeyQ));
        let update = MenuItem::with_id("update", "Update Available", true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &header,
            &PredefinedMenuItem::separator(),
            &toggle,
            &file,
            &last,
            &sessions,
            &pin,
            &hide,
            &PredefinedMenuItem::separator(),
            &settings,
            &permissions,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;

        let (tx, rx) = std::sync::mpsc::channel::<Command>();
        let menu_ctx = ctx.clone();
        let menu_tx: Sender<Command> = tx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, command)) = IDS.iter().find(|(id, _)| event.id == *id) {
                let _ = menu_tx.send(*command);
                menu_ctx.request_repaint();
            }
        }));
        drop(tx);

        let icon = add(&menu, false)?;
        // tray-icon does not say whether the shell took the icon; changing it
        // does: Windows refuses to modify an icon it does not hold.
        match icon.set_tooltip(Some("Voi")) {
            Ok(()) => tracing::info!("tray icon added to the notification area"),
            Err(e) => tracing::warn!(error = %e, "the shell has not taken the tray icon yet; it is added again when the taskbar appears"),
        }

        let hud_menu = Menu::new();
        hud_menu.append_items(&[
            &MenuItem::with_id("hud-current", "Open Current Session", true, None),
            &MenuItem::with_id("hud-sessions", "Open Sessions Folder", true, None),
        ])?;

        Ok(Self {
            hud_menu,
            icon,
            lost: None,
            readded: None,
            menu,
            header,
            toggle,
            last,
            pin,
            hide,
            update,
            update_shown: false,
            rx,
            drawn: Some((false, 0)),
        })
    }

    pub fn commands(&self) -> Vec<Command> {
        self.rx.try_iter().collect()
    }

    /// Bring the menu and icon up to date with what is happening.
    pub fn refresh(&mut self, view: &View) {
        self.header.set_text(decorate(&view.summary, view.spin));
        self.toggle.set_text(view.toggle_title);
        self.last.set_enabled(view.has_last);
        self.pin.set_text(if view.pinned { "Unpin Overlay" } else { "Keep Overlay Visible" });
        // Pinning something that cannot appear says nothing useful.
        self.pin.set_enabled(!view.hidden);
        self.hide.set_text(if view.hidden { "Show Voi" } else { "Hide Voi" });
        // Above Quit, after the separator before it, as on the Mac.
        match (&view.update, self.update_shown) {
            (Some(title), false) => {
                self.update.set_text(title);
                let quit = self.menu.items().len().saturating_sub(1);
                if self.menu.insert(&self.update, quit).is_ok() {
                    self.update_shown = true;
                }
            }
            (Some(title), true) => self.update.set_text(title),
            (None, true) => {
                if self.menu.remove(&self.update).is_ok() {
                    self.update_shown = false;
                }
            }
            (None, false) => {}
        }
        // The halo breathes in sixteen steps; the icon is redrawn when the
        // step changes.
        let look = (view.recording, if view.recording { (view.pulse * 16.0) as u8 % 16 } else { 0 });
        if self.drawn != Some(look) && self.icon.set_icon(Some(dot(view.recording, view.stopping, view.pulse, view.light_taskbar))).is_ok() {
            self.drawn = Some(look);
        }
        let tip = match &view.error {
            Some(e) => format!("Voi — {}\n{e}", view.summary),
            None => format!("Voi — {}", view.summary),
        };
        // Windows cuts a tooltip off at 127 characters, mid-word.
        let tip: String = tip.chars().take(120).collect();
        let held = self.icon.set_tooltip(Some(tip));
        self.keep(held, view.recording);
    }

    /// Act on whether the shell still holds the icon: log the change, and add
    /// it again once it has been gone for a few seconds — long enough for
    /// Explorer restarting to bring it back through `TaskbarCreated` first.
    fn keep(&mut self, held: tray_icon::Result<()>, recording: bool) {
        const GRACE: Duration = Duration::from_secs(5);
        const RETRY: Duration = Duration::from_secs(10);
        match held {
            Ok(()) => {
                if let Some(since) = self.lost.take() {
                    tracing::info!(after_ms = since.elapsed().as_millis() as u64, "tray icon back in the notification area");
                }
            }
            Err(e) => {
                let since = *self.lost.get_or_insert_with(|| {
                    tracing::warn!(error = %e, "tray icon gone from the notification area");
                    Instant::now()
                });
                let due = self.readded.is_none_or(|at| at.elapsed() >= RETRY);
                if since.elapsed() >= GRACE && due {
                    self.readded = Some(Instant::now());
                    match add(&self.menu, recording) {
                        Ok(icon) => {
                            tracing::info!("tray icon added again");
                            // The old one goes after the new one is in, so
                            // the menu never has no window to belong to.
                            self.icon = icon;
                            self.drawn = Some((recording, 0));
                        }
                        Err(e) => tracing::warn!(error = %e, "could not add the tray icon again"),
                    }
                }
            }
        }
    }
}

fn add(menu: &Menu, recording: bool) -> tray_icon::Result<TrayIcon> {
    TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(true)
        .with_tooltip("Voi")
        .with_icon(dot(recording, false, 0.0, light_taskbar()))
        .build()
}

/// What the menu shows, read from the shared state in one go.
pub struct View {
    pub summary: String,
    /// The spinner's frame, while a job runs.
    pub spin: usize,
    pub toggle_title: &'static str,
    pub has_last: bool,
    pub pinned: bool,
    pub hidden: bool,
    /// Recording or stopping: the dot is red and breathes.
    pub recording: bool,
    pub stopping: bool,
    /// The breathing's phase, 0 to 1.
    pub pulse: f32,
    pub light_taskbar: bool,
    pub error: Option<String>,
    /// The menu's "Update Available" title, while a newer build is out.
    pub update: Option<String>,
}

/// The menu's first line: the state, with a spinner while something takes
/// time (mac/mod.rs `decorate`).
pub fn decorate(summary: &str, spin: usize) -> String {
    const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
    match summary {
        "Idle" | "Recording" => summary.to_string(),
        _ => format!("{}  {summary}…", FRAMES[spin % FRAMES.len()]),
    }
}

/// Whether the taskbar is light. The Mac's idle dot is a template image that
/// macOS draws in the menu bar's own colour; Windows does nothing of the kind,
/// so the colour is chosen here the same way: white on dark, black on light.
pub fn light_taskbar() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("SystemUsesLightTheme"))
        .is_ok_and(|v| v == 1)
}

/// The Mac's menu-bar dot (mac/mod.rs `status_dot`), drawn 32 pixels square
/// for an 18-point box: radius 4.4, white (or black on a light taskbar) at
/// rest; red, with eight rings breathing around it, while recording, a
/// little dimmer while stopping.
fn dot(recording: bool, stopping: bool, pulse: f32, light: bool) -> Icon {
    const N: u32 = 32;
    let scale = N as f32 / 18.0;
    let radius = 4.4 * scale;
    let (r, g, b) = if recording {
        (0.93, 0.24, 0.20)
    } else if light {
        (0.0, 0.0, 0.0)
    } else {
        (1.0, 1.0, 1.0)
    };
    let alpha = if stopping { 0.85 } else { 1.0 };
    let c = N as f32 / 2.0;
    let phase = 0.5 - 0.5 * (pulse * std::f32::consts::TAU).cos();
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let d = (x as f32 + 0.5 - c).hypot(y as f32 + 0.5 - c);
            // Rings first, outermost in, then the dot over them.
            let mut a = 0.0f32;
            if recording {
                for i in (0..8).rev() {
                    let t = i as f32 / 7.0;
                    let ring = (4.4 + 0.8 + t * (2.2 + 2.6 * phase)) * scale;
                    let ring_alpha = 0.20 * (1.0 - t).powf(1.5) * (0.35 + 0.65 * phase);
                    let cover = (ring + 0.5 - d).clamp(0.0, 1.0) * ring_alpha;
                    a = cover + a * (1.0 - cover);
                }
            }
            let cover = (radius + 0.5 - d).clamp(0.0, 1.0) * alpha;
            a = cover + a * (1.0 - cover);
            rgba.extend_from_slice(&[(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, (a * 255.0) as u8]);
        }
    }
    Icon::from_rgba(rgba, N, N).expect("a 32×32 icon")
}
