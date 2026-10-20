//! The recording hotkey: one key or a combination that starts and stops a
//! recording from anywhere, and that nothing else receives while it is set.
//!
//! Kept here, away from any platform, so that what a hotkey *is* — how it is
//! written down, which ones are refused, and what happens to each key press
//! while one is set — is the same everywhere and is tested without a keyboard.
//! The Windows front end only translates its keyboard hook's events into
//! [`Matcher::event`] calls and does what the answer says.
//!
//! Keys are named by their Windows virtual-key codes. Those are the only
//! keyboard codes with a published, stable meaning, and they read back as the
//! names printed on the keys, which is what the settings file stores:
//! `"Ctrl+Alt+Space"`, not a number.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The modifier bits of a combination, either side of the keyboard.
pub const CTRL: u8 = 1;
pub const ALT: u8 = 2;
pub const SHIFT: u8 = 4;
pub const WIN: u8 = 8;

/// The default: three keys nobody presses by accident, and that no part of
/// Windows or the usual apps already answers to.
pub const DEFAULT: &str = "Ctrl+Alt+Space";

/// Virtual-key codes the matcher has to know by name.
pub mod vk {
    pub const LSHIFT: u16 = 0xA0;
    pub const RSHIFT: u16 = 0xA1;
    pub const LCONTROL: u16 = 0xA2;
    pub const RCONTROL: u16 = 0xA3;
    pub const LMENU: u16 = 0xA4;
    pub const RMENU: u16 = 0xA5;
    pub const LWIN: u16 = 0x5B;
    pub const RWIN: u16 = 0x5C;
    /// The side-less versions some keyboards and injectors send.
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const MENU: u16 = 0x12;
    /// Unassigned by Windows. Pressed by the hook to "mask" a swallowed key —
    /// see [`super::Action::Swallow`].
    pub const MASK: u16 = 0xE8;
}

/// Every key the settings can name, as `(name, code)`.
///
/// The name is what is printed on the key, so the file reads as the key cap.
/// Anything not in here still works and is written as its code, `0xNN`.
const KEYS: &[(&str, u16)] = &[
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Esc", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("0", 0x30),
    ("1", 0x31),
    ("2", 0x32),
    ("3", 0x33),
    ("4", 0x34),
    ("5", 0x35),
    ("6", 0x36),
    ("7", 0x37),
    ("8", 0x38),
    ("9", 0x39),
    ("A", 0x41),
    ("B", 0x42),
    ("C", 0x43),
    ("D", 0x44),
    ("E", 0x45),
    ("F", 0x46),
    ("G", 0x47),
    ("H", 0x48),
    ("I", 0x49),
    ("J", 0x4A),
    ("K", 0x4B),
    ("L", 0x4C),
    ("M", 0x4D),
    ("N", 0x4E),
    ("O", 0x4F),
    ("P", 0x50),
    ("Q", 0x51),
    ("R", 0x52),
    ("S", 0x53),
    ("T", 0x54),
    ("U", 0x55),
    ("V", 0x56),
    ("W", 0x57),
    ("X", 0x58),
    ("Y", 0x59),
    ("Z", 0x5A),
    ("LeftWin", vk::LWIN),
    ("RightWin", vk::RWIN),
    ("Menu", 0x5D),
    ("Num0", 0x60),
    ("Num1", 0x61),
    ("Num2", 0x62),
    ("Num3", 0x63),
    ("Num4", 0x64),
    ("Num5", 0x65),
    ("Num6", 0x66),
    ("Num7", 0x67),
    ("Num8", 0x68),
    ("Num9", 0x69),
    ("NumMultiply", 0x6A),
    ("NumAdd", 0x6B),
    ("NumSubtract", 0x6D),
    ("NumDecimal", 0x6E),
    ("NumDivide", 0x6F),
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F8", 0x77),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7A),
    ("F12", 0x7B),
    ("F13", 0x7C),
    ("F14", 0x7D),
    ("F15", 0x7E),
    ("F16", 0x7F),
    ("F17", 0x80),
    ("F18", 0x81),
    ("F19", 0x82),
    ("F20", 0x83),
    ("F21", 0x84),
    ("F22", 0x85),
    ("F23", 0x86),
    ("F24", 0x87),
    ("NumLock", 0x90),
    ("ScrollLock", 0x91),
    ("LeftShift", vk::LSHIFT),
    ("RightShift", vk::RSHIFT),
    ("LeftCtrl", vk::LCONTROL),
    ("RightCtrl", vk::RCONTROL),
    ("LeftAlt", vk::LMENU),
    ("RightAlt", vk::RMENU),
    ("VolumeMute", 0xAD),
    ("VolumeDown", 0xAE),
    ("VolumeUp", 0xAF),
    ("MediaNext", 0xB0),
    ("MediaPrevious", 0xB1),
    ("MediaStop", 0xB2),
    ("MediaPlayPause", 0xB3),
    (";", 0xBA),
    ("=", 0xBB),
    (",", 0xBC),
    ("-", 0xBD),
    (".", 0xBE),
    ("/", 0xBF),
    ("`", 0xC0),
    ("[", 0xDB),
    ("\\", 0xDC),
    ("]", 0xDD),
    ("'", 0xDE),
];

/// What the key is called, or its code when it has no name here.
pub fn key_name(code: u16) -> String {
    KEYS.iter()
        .find(|(_, c)| *c == code)
        .map(|(n, _)| n.to_string())
        .unwrap_or_else(|| format!("0x{code:02X}"))
}

fn key_code(name: &str) -> Option<u16> {
    if let Some(hex) = name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
        return u16::from_str_radix(hex, 16).ok().filter(|c| *c > 0 && *c < 0xFF);
    }
    KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| *c)
}

/// Which modifier bit a key is, if it is a modifier at all.
pub fn modifier_of(code: u16) -> Option<u8> {
    match code {
        vk::LCONTROL | vk::RCONTROL | vk::CONTROL => Some(CTRL),
        vk::LMENU | vk::RMENU | vk::MENU => Some(ALT),
        vk::LSHIFT | vk::RSHIFT | vk::SHIFT => Some(SHIFT),
        vk::LWIN | vk::RWIN => Some(WIN),
        _ => None,
    }
}

/// A key, and the modifiers that have to be held with it.
///
/// A modifier on its own — right Ctrl, say — is a hotkey too, and then it is
/// the key rather than a modifier: `mods` is empty and `key` is that side's
/// code, so the other Ctrl still works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: u8,
    pub key: u16,
}

/// Whether a hotkey may be set, and what to say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// Allowed, but it takes something away worth saying out loud.
    Warn(String),
    /// Not set: Windows keeps it, or it would stop you typing.
    Refuse(String),
}

impl Hotkey {
    pub fn new(mods: u8, key: u16) -> Self {
        Self { mods, key }
    }

    pub fn default_key() -> Self {
        DEFAULT.parse().expect("the default hotkey parses")
    }

    /// One number, for a keyboard hook to compare against without a lock.
    /// Never zero, so zero can mean "none".
    pub fn packed(&self) -> u32 {
        (self.mods as u32) << 16 | self.key as u32
    }

    pub fn unpack(packed: u32) -> Option<Self> {
        let key = (packed & 0xFFFF) as u16;
        (key != 0).then(|| Self { mods: (packed >> 16) as u8, key })
    }

    /// Is this a combination the settings should refuse or warn about?
    ///
    /// Refused: what Windows keeps for itself (most of these never reach a
    /// hook at all, so taking them would be a promise the app cannot keep),
    /// and a bare key you type with, which would take that key away from
    /// every program.
    pub fn verdict(&self) -> Verdict {
        let m = self.mods;
        let k = self.key;
        let name = self.to_string();

        let reserved = |why: &str| Verdict::Refuse(format!("{name} belongs to Windows ({why})"));
        const TAB: u16 = 0x09;
        const ESC: u16 = 0x1B;
        const DELETE: u16 = 0x2E;
        const F4: u16 = 0x73;
        const L: u16 = 0x4C;
        match (m, k) {
            (x, DELETE) if x == CTRL | ALT => return reserved("the security screen"),
            (x, 0x6E) if x == CTRL | ALT => return reserved("the security screen"),
            (x, ESC) if x == CTRL | SHIFT => return reserved("Task Manager"),
            (CTRL, ESC) => return reserved("the Start menu"),
            (ALT, TAB) => return reserved("switching windows"),
            (x, TAB) if x == ALT | SHIFT => return reserved("switching windows"),
            (x, TAB) if x == CTRL | ALT => return reserved("switching windows"),
            (ALT, ESC) => return reserved("switching windows"),
            (ALT, F4) => return reserved("closing the window"),
            (WIN, L) => return reserved("locking the computer"),
            (x, 0x52) if x == WIN | ALT => return reserved("Game Bar recording"),
            (x, 0x47) if x == WIN | ALT => return reserved("Game Bar"),
            (x, 0x53) if x == WIN | SHIFT => return reserved("screen snipping"),
            (x, 0x44) if x == WIN | CTRL => return reserved("virtual desktops"),
            (x, F4) if x == WIN | CTRL => return reserved("virtual desktops"),
            (x, 0x25 | 0x27) if x == WIN | CTRL => return reserved("virtual desktops"),
            (x, 0x0D) if x == WIN | CTRL => return reserved("Narrator"),
            _ => {}
        }
        if m == WIN {
            return Verdict::Refuse(format!(
                "{name}: Windows keeps the Windows key with one other key for itself"
            ));
        }
        if m == 0 && (k == vk::LWIN || k == vk::RWIN) {
            return Verdict::Refuse(format!("{name} alone is the Start menu"));
        }
        if m & (CTRL | ALT | WIN) == 0 && is_typing_key(k) {
            return Verdict::Refuse(format!(
                "{name} on its own would stop you typing it — add Ctrl, Alt or Win, or pick a key such as F9 or Pause"
            ));
        }

        if m == 0 && modifier_of(k).is_some() {
            return Verdict::Warn(format!(
                "{} will no longer work as a modifier anywhere while this is set",
                key_name(k)
            ));
        }
        if m == 0 && k == 0x14 {
            return Verdict::Warn("Caps Lock will no longer switch capitals".into());
        }
        if m == CTRL && matches!(k, 0x41 | 0x43 | 0x56 | 0x58 | 0x59 | 0x5A | 0x53) {
            return Verdict::Warn(format!(
                "{name} is copy, paste, undo or save in nearly every program; it will stop working there"
            ));
        }
        Verdict::Ok
    }
}

/// Keys that put a character in a text field, or move about in one.
fn is_typing_key(k: u16) -> bool {
    matches!(k,
        0x08 | 0x09 | 0x0D | 0x1B | 0x20 // backspace, tab, enter, esc, space
        | 0x21..=0x28                    // page up/down, end, home, arrows
        | 0x2D | 0x2E                    // insert, delete
        | 0x30..=0x39 | 0x41..=0x5A      // digits and letters
        | 0x60..=0x6F                    // the number pad
        | 0xBA..=0xC0 | 0xDB..=0xDE)     // punctuation
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (bit, name) in [(WIN, "Win"), (CTRL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift")] {
            if self.mods & bit != 0 {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.key))
    }
}

impl std::str::FromStr for Hotkey {
    type Err = String;

    /// `Ctrl+Alt+Space`, `F9`, `RightCtrl`. Case does not matter; a `+` key
    /// is not on the list (it is `NumAdd` or `=`), so `+` only ever separates.
    fn from_str(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let Some((key, mods)) = parts.split_last() else { return Err("empty hotkey".into()) };
        let mut bits = 0u8;
        for m in mods {
            bits |= match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => CTRL,
                "alt" => ALT,
                "shift" => SHIFT,
                "win" | "windows" | "super" => WIN,
                other => return Err(format!("unknown modifier {other:?} in {text:?}")),
            };
        }
        let key = key_code(key).ok_or_else(|| format!("unknown key {key:?} in {text:?}"))?;
        if modifier_of(key).is_some() && bits != 0 {
            return Err(format!("{text:?}: a modifier key can only be a hotkey on its own"));
        }
        Ok(Self { mods: bits, key })
    }
}

impl Serialize for Hotkey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hotkey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// What the keyboard hook should do with the key press it was just handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Let it through to whoever was going to get it.
    Pass,
    /// Swallow it: no other program sees it. `mask` says to press and release
    /// the unassigned [`vk::MASK`] key straight away, because Alt or Win is
    /// being held — a swallowed key leaves the application seeing a lone Alt
    /// tap, which opens its menu bar, or a lone Win tap, which opens Start.
    Swallow { mask: bool },
    /// Swallow it: hotkey number `slot` went down.
    Trigger { slot: usize, mask: bool },
    /// Swallow it: hotkey number `slot` has been let go — which is when a
    /// held-to-talk dictation ends.
    Released { slot: usize },
    /// Swallow it: Settings asked for the next key, and this is it.
    Captured(Hotkey),
}

/// A key press, as the hook saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: u16,
    pub down: bool,
}

/// What every key press means while a hotkey is set.
///
/// The hook keeps one of these and feeds it every event. It tracks the
/// modifiers itself rather than asking the system, because a swallowed key
/// is a key the system's own idea of the keyboard never hears about.
#[derive(Debug, Default)]
pub struct Matcher {
    /// What is held, by modifier bit.
    held: u8,
    /// Which modifier keys are down, by side, so releasing one Ctrl while the
    /// other is held does not clear Ctrl.
    down: Vec<u16>,
    /// The key currently being swallowed, so its repeats and its release are
    /// swallowed too, whatever happens to the modifiers in the meantime — and
    /// which hotkey it was, if it was one, so its release can be reported.
    swallowing: Option<(u16, Option<usize>)>,
    /// A modifier pressed while capturing, in case it is let go alone and is
    /// the key being chosen.
    capture_lone: Option<u16>,
}

impl Matcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one event. `bound` holds the hotkeys now in force, by slot;
    /// `capture` says Settings is waiting for a key to be pressed.
    pub fn event(&mut self, ev: KeyEvent, bound: &[Option<Hotkey>], capture: bool) -> Action {
        let modifier = modifier_of(ev.key);

        // The release of a key being swallowed is swallowed, always: the
        // program that never saw it go down must not see it come up.
        if let Some((_, slot)) = self.swallowing.filter(|(key, _)| *key == ev.key) {
            if ev.down {
                return Action::Swallow { mask: false }; // auto-repeat
            }
            self.swallowing = None;
            self.track(ev, modifier);
            return match slot {
                Some(slot) => Action::Released { slot },
                None => Action::Swallow { mask: false },
            };
        }

        if capture {
            return self.capture(ev, modifier);
        }
        self.capture_lone = None;

        let held_before = self.held;
        self.track(ev, modifier);

        if !ev.down {
            return Action::Pass;
        }
        for (slot, hotkey) in bound.iter().enumerate() {
            let Some(hotkey) = hotkey else { continue };
            if ev.key != hotkey.key && !same_key(ev.key, hotkey.key) {
                continue;
            }
            // The key itself is not one of its own modifiers.
            let held = match modifier {
                Some(bit) if hotkey.mods == 0 => held_before & !bit,
                _ => held_before,
            };
            if held != hotkey.mods {
                continue;
            }
            self.swallowing = Some((ev.key, Some(slot)));
            return Action::Trigger { slot, mask: held & (ALT | WIN) != 0 };
        }
        Action::Pass
    }

    fn capture(&mut self, ev: KeyEvent, modifier: Option<u8>) -> Action {
        let held_before = self.held;
        self.track(ev, modifier);
        match (modifier, ev.down) {
            // A modifier going down might be the start of a combination or a
            // key chosen on its own; which, is known when it comes back up.
            (Some(_), true) => {
                self.capture_lone = (held_before == 0).then_some(ev.key);
                Action::Pass
            }
            (Some(_), false) => match self.capture_lone.take() {
                Some(key) if key == ev.key => Action::Captured(Hotkey::new(0, sided(key))),
                _ => Action::Pass,
            },
            (None, true) => {
                self.capture_lone = None;
                self.swallowing = Some((ev.key, None));
                Action::Captured(Hotkey::new(held_before, ev.key))
            }
            (None, false) => Action::Pass,
        }
    }

    fn track(&mut self, ev: KeyEvent, modifier: Option<u8>) {
        if modifier.is_none() {
            return;
        }
        if ev.down {
            if !self.down.contains(&ev.key) {
                self.down.push(ev.key);
            }
        } else {
            self.down.retain(|k| *k != ev.key);
        }
        self.held = self.down.iter().filter_map(|k| modifier_of(*k)).fold(0, |a, b| a | b);
    }

    /// Forget what is held — after the hook was away long enough for the
    /// keyboard to have moved on without it.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// A side-less modifier code matches either side of it.
fn same_key(seen: u16, bound: u16) -> bool {
    matches!(
        (seen, bound),
        (vk::CONTROL, vk::LCONTROL | vk::RCONTROL)
            | (vk::MENU, vk::LMENU | vk::RMENU)
            | (vk::SHIFT, vk::LSHIFT | vk::RSHIFT)
    )
}

/// A captured modifier, by side. Injected input sometimes carries only the
/// side-less code; then it is taken as the left one.
fn sided(key: u16) -> u16 {
    match key {
        vk::CONTROL => vk::LCONTROL,
        vk::MENU => vk::LMENU,
        vk::SHIFT => vk::LSHIFT,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(text: &str) -> Hotkey {
        text.parse().unwrap()
    }
    fn down(key: u16) -> KeyEvent {
        KeyEvent { key, down: true }
    }
    fn up(key: u16) -> KeyEvent {
        KeyEvent { key, down: false }
    }
    const SPACE: u16 = 0x20;
    const F9: u16 = 0x78;
    const A: u16 = 0x41;

    #[test]
    fn hotkeys_read_and_write_as_the_keys_are_labelled() {
        for text in ["Ctrl+Alt+Space", "F9", "RightCtrl", "Ctrl+Shift+F23", "Win+Alt+K", "Pause", "Alt+`"] {
            let parsed = hk(text);
            assert_eq!(parsed.to_string(), text, "round trip of {text}");
            assert_eq!(Hotkey::unpack(parsed.packed()), Some(parsed));
        }
        // Order and case do not matter on the way in; the way out is canonical.
        assert_eq!(hk("alt+ctrl+space").to_string(), "Ctrl+Alt+Space");
        assert_eq!(hk("Shift+0xE9").to_string(), "Shift+0xE9", "an unnamed key keeps its code");
        assert!("Ctrl+".parse::<Hotkey>().is_err());
        assert!("Hyper+A".parse::<Hotkey>().is_err());
        assert!("Ctrl+RightAlt".parse::<Hotkey>().is_err(), "a modifier is a hotkey only alone");
        assert_eq!(Hotkey::unpack(0), None, "zero means none");
    }

    #[test]
    fn the_default_is_allowed() {
        assert_eq!(Hotkey::default_key().verdict(), Verdict::Ok);
        assert_eq!(Hotkey::default_key().to_string(), DEFAULT);
    }

    #[test]
    fn what_windows_keeps_is_refused() {
        for text in [
            "Ctrl+Alt+Delete", "Ctrl+Shift+Esc", "Alt+Tab", "Alt+Shift+Tab", "Alt+Esc", "Alt+F4",
            "Win+L", "Win+D", "Win+Tab", "Ctrl+Esc", "Win+Alt+R", "Win+Shift+S", "LeftWin",
        ] {
            assert!(matches!(hk(text).verdict(), Verdict::Refuse(_)), "{text} should be refused");
        }
    }

    #[test]
    fn a_bare_typing_key_is_refused_and_a_spare_one_is_not() {
        for text in ["A", "Shift+A", "Space", "Enter", "5", "Left", "Num3", "-"] {
            assert!(matches!(hk(text).verdict(), Verdict::Refuse(_)), "{text} should be refused");
        }
        for text in ["F9", "Pause", "ScrollLock", "Ctrl+Alt+A", "Win+Alt+K", "Ctrl+Shift+F23", "MediaPlayPause"] {
            assert_eq!(hk(text).verdict(), Verdict::Ok, "{text} should be allowed");
        }
        assert!(matches!(hk("RightCtrl").verdict(), Verdict::Warn(_)));
        assert!(matches!(hk("Ctrl+C").verdict(), Verdict::Warn(_)));
    }

    #[test]
    fn the_combination_is_swallowed_whole_and_nothing_else_is() {
        let bound = [Some(hk("Ctrl+Alt+Space"))];
        let mut m = Matcher::new();
        assert_eq!(m.event(down(vk::LCONTROL), &bound, false), Action::Pass, "modifiers pass");
        assert_eq!(m.event(down(vk::LMENU), &bound, false), Action::Pass);
        assert_eq!(m.event(down(SPACE), &bound, false), Action::Trigger { slot: 0, mask: true }, "Alt held: mask it");
        assert_eq!(m.event(down(SPACE), &bound, false), Action::Swallow { mask: false }, "repeat");
        // The modifiers come up first: the space's release is still swallowed.
        assert_eq!(m.event(up(vk::LMENU), &bound, false), Action::Pass);
        assert_eq!(m.event(up(SPACE), &bound, false), Action::Released { slot: 0 });
        assert_eq!(m.event(up(vk::LCONTROL), &bound, false), Action::Pass);

        // Space alone, or with only some of the modifiers, is just a space.
        assert_eq!(m.event(down(SPACE), &bound, false), Action::Pass);
        assert_eq!(m.event(up(SPACE), &bound, false), Action::Pass);
        m.event(down(vk::RCONTROL), &bound, false);
        assert_eq!(m.event(down(SPACE), &bound, false), Action::Pass, "Ctrl+Space is not it");
        m.event(up(SPACE), &bound, false);
        m.event(down(vk::LSHIFT), &bound, false);
        m.event(down(vk::RMENU), &bound, false);
        assert_eq!(m.event(down(SPACE), &bound, false), Action::Pass, "an extra Shift is not it either");
        assert_eq!(m.event(down(A), &bound, false), Action::Pass, "unrelated keys pass");
    }

    #[test]
    fn letting_go_of_one_ctrl_keeps_the_other() {
        let bound = [Some(hk("Ctrl+F9"))];
        let mut m = Matcher::new();
        m.event(down(vk::LCONTROL), &bound, false);
        m.event(down(vk::RCONTROL), &bound, false);
        m.event(up(vk::LCONTROL), &bound, false);
        assert_eq!(m.event(down(F9), &bound, false), Action::Trigger { slot: 0, mask: false });
    }

    #[test]
    fn a_single_key_is_taken_from_everyone() {
        let bound = [Some(hk("F9"))];
        let mut m = Matcher::new();
        assert_eq!(m.event(down(F9), &bound, false), Action::Trigger { slot: 0, mask: false });
        assert_eq!(m.event(up(F9), &bound, false), Action::Released { slot: 0 });
        // With a modifier held it is a different combination and passes.
        m.event(down(vk::LSHIFT), &bound, false);
        assert_eq!(m.event(down(F9), &bound, false), Action::Pass);
    }

    #[test]
    fn a_modifier_alone_is_taken_and_the_other_side_still_works() {
        let bound = [Some(hk("RightCtrl"))];
        let mut m = Matcher::new();
        assert_eq!(m.event(down(vk::RCONTROL), &bound, false), Action::Trigger { slot: 0, mask: false });
        assert_eq!(m.event(up(vk::RCONTROL), &bound, false), Action::Released { slot: 0 });
        assert_eq!(m.event(down(vk::LCONTROL), &bound, false), Action::Pass, "left Ctrl is still Ctrl");
        assert_eq!(m.event(down(0x43), &bound, false), Action::Pass);
    }

    /// Recording and dictation each have a key; each press is reported as
    /// its own, and so is letting go — which ends a held dictation.
    #[test]
    fn two_hotkeys_are_told_apart_and_their_release_is_reported() {
        let bound = [Some(hk("Ctrl+Alt+Space")), Some(hk("RightCtrl"))];
        let mut m = Matcher::new();
        assert_eq!(m.event(down(vk::RCONTROL), &bound, false), Action::Trigger { slot: 1, mask: false });
        assert_eq!(m.event(down(vk::RCONTROL), &bound, false), Action::Swallow { mask: false }, "repeat");
        assert_eq!(m.event(down(A), &bound, false), Action::Pass, "typing while held still types");
        assert_eq!(m.event(up(vk::RCONTROL), &bound, false), Action::Released { slot: 1 });
        m.event(down(vk::LCONTROL), &bound, false);
        m.event(down(vk::LMENU), &bound, false);
        assert_eq!(m.event(down(SPACE), &bound, false), Action::Trigger { slot: 0, mask: true });
        assert_eq!(m.event(up(SPACE), &bound, false), Action::Released { slot: 0 });
        // A slot that is off is skipped, not matched.
        m.event(up(vk::LMENU), &bound, false);
        m.event(up(vk::LCONTROL), &bound, false);
        let bound = [None, Some(hk("F9"))];
        assert_eq!(m.event(down(F9), &bound, false), Action::Trigger { slot: 1, mask: false });
    }

    #[test]
    fn nothing_is_swallowed_with_no_hotkey_set() {
        let mut m = Matcher::new();
        for key in [vk::LCONTROL, vk::LMENU, SPACE, F9] {
            assert_eq!(m.event(down(key), &[None], false), Action::Pass);
        }
    }

    #[test]
    fn settings_capture_the_next_key_and_the_hotkey_rests_meanwhile() {
        let bound = [Some(hk("F9"))];
        let mut m = Matcher::new();
        // While capturing, the bound key is just a candidate.
        assert_eq!(m.event(down(F9), &bound, true), Action::Captured(hk("F9")));
        assert_eq!(m.event(up(F9), &bound, true), Action::Swallow { mask: false });

        m.event(down(vk::LCONTROL), &bound, true);
        m.event(down(vk::LSHIFT), &bound, true);
        assert_eq!(m.event(down(0x52), &bound, true), Action::Captured(hk("Ctrl+Shift+R")));
        assert_eq!(m.event(up(0x52), &bound, true), Action::Swallow { mask: false });
        m.event(up(vk::LSHIFT), &bound, true);
        m.event(up(vk::LCONTROL), &bound, true);

        // A modifier pressed and let go alone is the modifier, by side.
        assert_eq!(m.event(down(vk::RMENU), &bound, true), Action::Pass);
        assert_eq!(m.event(up(vk::RMENU), &bound, true), Action::Captured(hk("RightAlt")));
        // But not when it was part of a combination that went nowhere.
        m.event(down(vk::LCONTROL), &bound, true);
        m.event(down(vk::LSHIFT), &bound, true);
        m.event(up(vk::LSHIFT), &bound, true);
        assert_eq!(m.event(up(vk::LCONTROL), &bound, true), Action::Pass);
    }
}
