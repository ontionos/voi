//! What the app remembers between runs, apart from the key.
//!
//! A JSON file beside the key rather than more lines in `config.env`: the
//! context dictionary is free text somebody pastes, and a `KEY=value` file has
//! no answer for a value with a newline in it. The key keeps its own file —
//! it is chmod 600, and the command line reads it too.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Soniox's ceiling for the whole context object: 8,000 tokens, which their
/// documentation puts at roughly this many characters.
pub const MAX_CONTEXT_CHARS: usize = 10_000;

/// What to expect in the audio when nobody has said otherwise.
pub fn default_languages() -> Vec<String> {
    vec!["uk".into(), "ru".into(), "en".into()]
}

/// How the dictation key behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DictationMode {
    /// Held down while you speak, like a walkie-talkie. Nothing to remember to
    /// switch off, so nothing is ever left listening.
    #[default]
    PushToTalk,
    /// Once to start, once to stop — for a long dictation, or a keyboard where
    /// holding a key down is awkward.
    Toggle,
}

/// Which key starts dictation.
///
/// All of these are modifier keys, and that is not a limitation of taste: the
/// state of the modifiers is published by the window server for anyone to
/// read, where knowing that a *letter* is down means watching the keyboard,
/// which macOS gates behind a permission. A key that needs permission before it
/// works at all is a key that does not work.
///
/// Left and right are separate entries because they are separate keys, and the
/// difference matters: the left-hand ones are held for ⌘C, ⌘V and ⌘Tab all day
/// long, and the right-hand ones are pressed by themselves or not at all.
///
/// Windows has its own set — right Alt, right Ctrl, the Windows key — and this
/// is where they will go, which is why the choice is a list rather than the one
/// key that happens to suit a Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DictationKey {
    #[default]
    RightCommand,
    LeftCommand,
    RightOption,
    LeftOption,
    RightControl,
    Fn,
}

impl DictationKey {
    pub const ALL: [DictationKey; 6] = [
        DictationKey::RightCommand,
        DictationKey::LeftCommand,
        DictationKey::RightOption,
        DictationKey::LeftOption,
        DictationKey::RightControl,
        DictationKey::Fn,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            DictationKey::RightCommand => "Right ⌘",
            DictationKey::LeftCommand => "Left ⌘",
            DictationKey::RightOption => "Right ⌥",
            DictationKey::LeftOption => "Left ⌥",
            DictationKey::RightControl => "Right ⌃",
            DictationKey::Fn => "Fn (Globe)",
        }
    }

    /// The bit in the modifier flags that means this key.
    ///
    /// The device-dependent bits, which have said which side a modifier was
    /// pressed on since NeXT, plus Apple's own flag for the Function key.
    pub fn mask(&self) -> usize {
        match self {
            DictationKey::LeftCommand => 0x0000_0008,
            DictationKey::RightCommand => 0x0000_0010,
            DictationKey::LeftOption => 0x0000_0020,
            DictationKey::RightOption => 0x0000_0040,
            DictationKey::RightControl => 0x0000_2000,
            DictationKey::Fn => 0x0080_0000,
        }
    }
}

/// Where on screen the dictation window appears: the nine places anything can
/// sensibly sit on a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Spot {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    Center,
    MiddleRight,
    BottomLeft,
    /// Under the eyes and out of the way of most windows' content.
    #[default]
    BottomCenter,
    BottomRight,
}

impl Spot {
    pub const ALL: [Spot; 9] = [
        Spot::TopLeft,
        Spot::TopCenter,
        Spot::TopRight,
        Spot::MiddleLeft,
        Spot::Center,
        Spot::MiddleRight,
        Spot::BottomLeft,
        Spot::BottomCenter,
        Spot::BottomRight,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Spot::TopLeft => "Top left",
            Spot::TopCenter => "Top centre",
            Spot::TopRight => "Top right",
            Spot::MiddleLeft => "Middle left",
            Spot::Center => "Centre",
            Spot::MiddleRight => "Middle right",
            Spot::BottomLeft => "Bottom left",
            Spot::BottomCenter => "Bottom centre",
            Spot::BottomRight => "Bottom right",
        }
    }

    /// Which way is "off screen" from here: the edge this spot sits against,
    /// as a direction to travel in.
    ///
    /// A window that fades in on the spot has arrived from nowhere. One that
    /// slides the last of the way in from the edge it lives on has come from
    /// somewhere, and goes back there — which is the difference between a
    /// thing appearing and a thing arriving. The centre has no edge to come
    /// from, so it only fades.
    pub fn slide(&self) -> (f64, f64) {
        match self {
            Spot::TopLeft | Spot::TopCenter | Spot::TopRight => (0.0, 1.0),
            Spot::BottomLeft | Spot::BottomCenter | Spot::BottomRight => (0.0, -1.0),
            Spot::MiddleLeft => (-1.0, 0.0),
            Spot::MiddleRight => (1.0, 0.0),
            Spot::Center => (0.0, 0.0),
        }
    }

    /// The bottom-left corner for a window of `size` on a screen whose usable
    /// area starts at `origin` and is `screen` big, keeping `margin` clear of
    /// the edges it touches.
    ///
    /// Everything is in AppKit's coordinates, which count upward from the
    /// bottom of the screen. Centred positions ignore the margin — there is
    /// nothing to be clear of in the middle.
    pub fn place(
        &self,
        origin: (f64, f64),
        screen: (f64, f64),
        size: (f64, f64),
        margin: f64,
    ) -> (f64, f64) {
        let (left, bottom) = origin;
        let (width, height) = screen;
        let (w, h) = size;

        let x = match self {
            Spot::TopLeft | Spot::MiddleLeft | Spot::BottomLeft => left + margin,
            Spot::TopCenter | Spot::Center | Spot::BottomCenter => left + (width - w) / 2.0,
            Spot::TopRight | Spot::MiddleRight | Spot::BottomRight => left + width - w - margin,
        };
        let y = match self {
            Spot::TopLeft | Spot::TopCenter | Spot::TopRight => bottom + height - h - margin,
            Spot::MiddleLeft | Spot::Center | Spot::MiddleRight => bottom + (height - h) / 2.0,
            Spot::BottomLeft | Spot::BottomCenter | Spot::BottomRight => bottom + margin,
        };
        (x, y)
    }
}

/// How far the glass keeps from the pointer's tip, sideways and above it.
const BESIDE_POINTER: f64 = 10.0;
/// And below it, which is where the arrow itself is drawn.
const UNDER_POINTER: f64 = 28.0;

/// Where the glass goes when it follows the pointer: the bottom-left corner
/// for a glass `size` big, given the pointer's tip.
///
/// Above the pointer and to its right; to its left when the right has no
/// room, under it when the top has none — always in one corner beside it and
/// never over it. Over it would not just hide the arrow: the pointer on the
/// glass is how a dictation is thrown away, so letting go would cancel. Kept
/// on the usable area, which starts at `origin` and is `screen` big. AppKit's
/// coordinates, y upward.
pub fn beside_pointer(
    pointer: (f64, f64),
    origin: (f64, f64),
    screen: (f64, f64),
    size: (f64, f64),
) -> (f64, f64) {
    let (px, py) = pointer;
    let (left, bottom) = origin;
    let (right, top) = (left + screen.0, bottom + screen.1);
    let (w, h) = size;

    let mut x = px + BESIDE_POINTER;
    if x + w > right {
        x = px - BESIDE_POINTER - w;
    }
    let mut y = py + BESIDE_POINTER;
    if y + h > top {
        y = py - UNDER_POINTER - h;
    }
    (x.clamp(left, (right - w).max(left)), y.clamp(bottom, (top - h).max(bottom)))
}

/// The corner of a glass at `origin`, `size` big, that faces the pointer, and
/// the step from that corner to the pointer's tip: where the glass grows out
/// of when it appears beside the pointer, and shrinks back into as it goes.
pub fn facing_corner(
    pointer: (f64, f64),
    origin: (f64, f64),
    size: (f64, f64),
) -> ((f64, f64), (f64, f64)) {
    let (px, py) = pointer;
    let (x, y) = origin;
    let (w, h) = size;
    let cx = if px <= x + w / 2.0 { x } else { x + w };
    let cy = if py <= y + h / 2.0 { y } else { y + h };
    ((cx, cy), (px - cx, py - cy))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Languages that may turn up in the audio. Soniox detects the language on
    /// its own; these weight the guess, and a call that switches language
    /// mid-sentence is exactly where that weighting earns its keep.
    pub languages: Vec<String>,
    /// Names, jargon and product names to expect — Soniox's context
    /// dictionary. It is a hint, not a filter: nothing is forced to match.
    pub terms: Vec<String>,
    /// Never put the widget on screen, whatever is happening.
    pub hidden: bool,
    /// Whether the dictation key is held or toggled.
    pub dictation_mode: DictationMode,
    /// Where the dictation window shows up.
    pub dictation_spot: Spot,
    /// Open it beside the pointer instead, wherever that is.
    pub dictation_follow_pointer: bool,
    /// Which key summons it.
    pub dictation_key: DictationKey,
    /// And how far from the edge of the screen it sits, in points.
    pub dictation_margin: f64,
    /// Whether dictation says out loud that it started and stopped.
    ///
    /// Worth having on: while dictating you are looking at what you are
    /// writing into, not at the window, and a sound is the only feedback that
    /// reaches you there.
    pub sounds: bool,
    /// Which microphone to listen with, by name; `None` follows the system's
    /// default. Worth choosing with AirPods: opening their microphone switches
    /// them from music to headset mode, which interrupts whatever is playing
    /// and takes a second or two — the MacBook's own microphone does neither.
    pub microphone: Option<String>,
    /// The key or combination that starts and stops a recording from
    /// anywhere; `None` when switched off. While set, no other program gets
    /// it — see [`crate::hotkey`]. Windows only for now: the Mac app records
    /// from its menu and its overlay.
    #[serde(deserialize_with = "lenient_hotkey")]
    pub record_hotkey: Option<crate::hotkey::Hotkey>,
}

/// The default distance from the edge: clear of it, without being adrift in the
/// middle of the screen.
pub const DEFAULT_MARGIN: f64 = 34.0;

impl Default for Prefs {
    fn default() -> Self {
        Self {
            languages: default_languages(),
            terms: Vec::new(),
            hidden: false,
            dictation_mode: DictationMode::default(),
            dictation_spot: Spot::default(),
            dictation_follow_pointer: false,
            dictation_key: DictationKey::default(),
            dictation_margin: DEFAULT_MARGIN,
            sounds: true,
            microphone: None,
            // The Mac has no key for recording, so none until one is chosen.
            record_hotkey: None,
        }
    }
}

/// A hotkey this build cannot read is left off instead of failing the whole
/// file, which would lose every other setting with it.
fn lenient_hotkey<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<crate::hotkey::Hotkey>, D::Error> {
    let text: Option<String> = Option::deserialize(d)?;
    Ok(text.and_then(|text| match text.parse() {
        Ok(hotkey) => Some(hotkey),
        Err(e) => {
            tracing::warn!(error = %e, "saved hotkey unreadable; leaving it off");
            None
        }
    }))
}

pub fn path() -> Result<PathBuf> {
    Ok(crate::data_dir()?.join("settings.json"))
}

/// Read them, or hand back the defaults.
///
/// Never fails: a missing file is a first run, and a damaged one is not worth
/// refusing to start over. Every field carries a default of its own, so a file
/// written by an older build still loads.
pub fn load() -> Prefs {
    let Ok(path) = path() else { return Prefs::default() };
    let Ok(text) = std::fs::read_to_string(&path) else { return Prefs::default() };
    match parse(&text) {
        Ok(prefs) => prefs,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "settings unreadable; using defaults");
            Prefs::default()
        }
    }
}

/// The settings in `text`. Notepad and PowerShell on Windows start a UTF-8
/// file with a byte-order mark, which JSON does not allow; without this, one
/// hand-edit there would lose every setting.
fn parse(text: &str) -> serde_json::Result<Prefs> {
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(text))
}

pub fn save(prefs: &Prefs) -> Result<()> {
    let path = path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(prefs)?;
    std::fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Split what somebody typed into entries.
///
/// Commas or newlines, either way, because both are what people actually do —
/// and a term with a space in it ("playable ad") has to survive, so spaces are
/// not separators.
pub fn parse_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in text.split([',', '\n', ';']) {
        let item = item.trim();
        if item.is_empty() || out.iter().any(|seen| seen == item) {
            continue;
        }
        out.push(item.to_string());
    }
    out
}

/// And back, for putting in a field.
pub fn join_list(items: &[String]) -> String {
    items.join(", ")
}

/// Turn what somebody typed into the two-letter code Soniox expects.
///
/// People write "English", the API wants "en", and both should work — a
/// settings box that silently ignores what you typed is worse than one that
/// asks for codes. Anything already two letters is left alone, and anything
/// unrecognised is passed through untouched rather than dropped, so a language
/// this table has never heard of is still usable.
pub fn language_code(input: &str) -> String {
    const NAMES: [(&str, &str); 24] = [
        ("english", "en"),
        ("ukrainian", "uk"),
        ("українська", "uk"),
        ("russian", "ru"),
        ("polish", "pl"),
        ("german", "de"),
        ("french", "fr"),
        ("spanish", "es"),
        ("italian", "it"),
        ("portuguese", "pt"),
        ("dutch", "nl"),
        ("czech", "cs"),
        ("slovak", "sk"),
        ("turkish", "tr"),
        ("arabic", "ar"),
        ("hebrew", "he"),
        ("chinese", "zh"),
        ("japanese", "ja"),
        ("korean", "ko"),
        ("hindi", "hi"),
        ("romanian", "ro"),
        ("bulgarian", "bg"),
        ("swedish", "sv"),
        ("norwegian", "no"),
    ];
    let lower = input.trim().to_lowercase();
    if let Some((_, code)) = NAMES.iter().find(|(name, _)| *name == lower) {
        return (*code).to_string();
    }
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_survives_the_round_trip() {
        let typed = "three.js, grok,\nplayable ad ,, grok";
        let parsed = parse_list(typed);
        assert_eq!(parsed, ["three.js", "grok", "playable ad"], "trimmed, split and deduped");
        assert_eq!(parse_list(&join_list(&parsed)), parsed, "and stable through a re-read");
        assert!(parse_list("  ,, \n ").is_empty(), "nothing typed is no terms");
    }

    #[test]
    fn languages_are_stored_as_codes_however_they_are_typed() {
        assert_eq!(language_code("English"), "en");
        assert_eq!(language_code(" ukrainian "), "uk");
        assert_eq!(language_code("UK"), "uk", "already a code");
        // Not in the table: kept, not dropped. Soniox knows more languages
        // than this list does.
        assert_eq!(language_code("Sanskrit"), "sanskrit");
    }

    /// Nine places, and each one stays inside the screen it was given.
    #[test]
    fn every_spot_lands_inside_the_screen() {
        // A screen that does not start at zero: a second display, or the menu
        // bar taking a bite out of the top of the first.
        let origin = (100.0, 50.0);
        let screen = (1000.0, 600.0);
        let size = (300.0, 80.0);
        let margin = 24.0;

        for spot in Spot::ALL {
            let (x, y) = spot.place(origin, screen, size, margin);
            assert!(
                x >= origin.0 && x + size.0 <= origin.0 + screen.0,
                "{} runs off the side: {x}",
                spot.label()
            );
            assert!(
                y >= origin.1 && y + size.1 <= origin.1 + screen.1,
                "{} runs off the top or bottom: {y}",
                spot.label()
            );
        }

        // And they are where their names say. AppKit counts upward from the
        // bottom, so "top" is the larger y.
        let top = Spot::TopCenter.place(origin, screen, size, margin);
        let bottom = Spot::BottomCenter.place(origin, screen, size, margin);
        assert!(top.1 > bottom.1, "top should be above bottom");
        assert_eq!(top.0, bottom.0, "both centred means the same x");
        assert_eq!(bottom.1, origin.1 + margin, "the margin is kept clear");

        let left = Spot::MiddleLeft.place(origin, screen, size, margin);
        let right = Spot::MiddleRight.place(origin, screen, size, margin);
        assert_eq!(left.0, origin.0 + margin);
        assert_eq!(right.0, origin.0 + screen.0 - size.0 - margin);
        assert_eq!(left.1, right.1, "same row, same height");
    }

    /// Above and to the right of the pointer, over to the left or underneath
    /// only where the screen ends — and never over the pointer, where letting
    /// go would cancel.
    #[test]
    fn the_glass_keeps_beside_the_pointer() {
        let origin = (100.0, 50.0);
        let screen = (1000.0, 600.0);
        let size = (368.0, 93.0);
        let (right, top) = (1100.0, 650.0);

        // Room everywhere: above and to the right.
        let (x, y) = beside_pointer((400.0, 300.0), origin, screen, size);
        assert!(x > 400.0 && y > 300.0, "above-right: {x},{y}");

        // Too near the right: above and to the left.
        let (x, y) = beside_pointer((1000.0, 300.0), origin, screen, size);
        assert!(x + size.0 < 1000.0 && y > 300.0, "above-left: {x},{y}");

        // Too near the top: under, clear of the arrow.
        let (x, y) = beside_pointer((400.0, 600.0), origin, screen, size);
        assert!(x > 400.0 && y + size.1 <= 600.0 - 20.0, "under-right, below the arrow: {x},{y}");

        // Wherever the pointer is on the screen, the glass is on the screen
        // and the pointer is not on the glass.
        let mut px = origin.0;
        while px <= right {
            let mut py = origin.1;
            while py <= top {
                let (x, y) = beside_pointer((px, py), origin, screen, size);
                assert!(x >= origin.0 && x + size.0 <= right, "off the side at {px},{py}");
                assert!(y >= origin.1 && y + size.1 <= top, "off the top or bottom at {px},{py}");
                let under = px >= x && px <= x + size.0 && py >= y && py <= y + size.1;
                assert!(!under, "the pointer at {px},{py} is on the glass at {x},{y}");
                py += 7.0;
            }
            px += 7.0;
        }
    }

    /// It grows out of the corner nearest the pointer, whichever side of the
    /// pointer it ended up on, and that corner is a short step from the tip.
    #[test]
    fn it_grows_out_of_the_corner_by_the_pointer() {
        let origin = (100.0, 50.0);
        let screen = (1000.0, 600.0);
        let size = (368.0, 93.0);
        // Above-right, above-left, under-right, under-left.
        for (pointer, corner_is) in [
            ((400.0, 300.0), (false, false)),
            ((1000.0, 300.0), (true, false)),
            ((400.0, 620.0), (false, true)),
            ((1050.0, 620.0), (true, true)),
        ] {
            let at = beside_pointer(pointer, origin, screen, size);
            let ((cx, cy), (dx, dy)) = facing_corner(pointer, at, size);
            let (right, top) = corner_is;
            assert_eq!(cx, if right { at.0 + size.0 } else { at.0 }, "wrong side for {pointer:?}");
            assert_eq!(cy, if top { at.1 + size.1 } else { at.1 }, "wrong edge for {pointer:?}");
            assert_eq!((cx + dx, cy + dy), pointer, "the step ends at the tip");
            assert!(dx.abs() <= 30.0 && dy.abs() <= 30.0, "within the room it travels in: {dx},{dy}");
        }
    }

    /// Each spot leaves towards the edge it sits on, and the centre stays put.
    #[test]
    fn every_spot_knows_which_way_is_out() {
        assert_eq!(Spot::BottomCenter.slide(), (0.0, -1.0), "downward, off the bottom");
        assert_eq!(Spot::TopRight.slide(), (0.0, 1.0), "upward, off the top");
        assert_eq!(Spot::MiddleLeft.slide(), (-1.0, 0.0));
        assert_eq!(Spot::Center.slide(), (0.0, 0.0), "nowhere to go from the middle");

        // Whichever way it goes, it goes one way: a diagonal would leave a
        // corner window travelling further than an edge one for the same time.
        for spot in Spot::ALL {
            let (x, y) = spot.slide();
            assert!(x == 0.0 || y == 0.0, "{} travels diagonally", spot.label());
            assert!(x.abs() <= 1.0 && y.abs() <= 1.0);
        }
    }

    /// Every key is one bit, and no two keys are the same bit — otherwise
    /// choosing one would fire on another.
    #[test]
    fn each_key_is_its_own_bit() {
        let mut seen = 0usize;
        for key in DictationKey::ALL {
            let mask = key.mask();
            assert!(mask.count_ones() == 1, "{} is not a single bit", key.label());
            assert!(seen & mask == 0, "{} shares a bit with another key", key.label());
            seen |= mask;
        }
        // And the two Command keys really are told apart, which is the whole
        // reason for using these bits rather than the ordinary ones.
        assert_ne!(
            DictationKey::LeftCommand.mask(),
            DictationKey::RightCommand.mask()
        );
    }

    /// A file saved by a Windows editor, which starts with a byte-order
    /// mark, still loads.
    #[test]
    fn a_byte_order_mark_is_not_a_reason_to_forget_everything() {
        let prefs = parse("\u{feff}{\"terms\": [\"PlayCanvas\"]}").unwrap();
        assert_eq!(prefs.terms, ["PlayCanvas"]);
    }

    /// A settings file from a build that did not have every field yet still
    /// loads, and what it does not mention keeps its default.
    #[test]
    fn older_settings_files_still_load() {
        let prefs: Prefs = serde_json::from_str(r#"{"hidden": true}"#).unwrap();
        assert!(prefs.hidden);
        assert_eq!(prefs.languages, default_languages(), "unmentioned means default");
        assert!(prefs.terms.is_empty());
    }

    /// Everything a panel can change has to come back unchanged.
    #[test]
    fn prefs_survive_being_written_and_read() {
        let prefs = Prefs {
            languages: vec!["en".into(), "uk".into()],
            terms: vec!["Acme".into(), "playable ad".into()],
            hidden: true,
            dictation_mode: DictationMode::Toggle,
            dictation_spot: Spot::TopRight,
            dictation_follow_pointer: true,
            dictation_key: DictationKey::Fn,
            dictation_margin: 60.0,
            sounds: false,
            microphone: Some("MacBook Pro Microphone".into()),
            record_hotkey: Some("Ctrl+Shift+F23".parse().unwrap()),
        };
        let text = serde_json::to_string_pretty(&prefs).unwrap();
        assert_eq!(serde_json::from_str::<Prefs>(&text).unwrap(), prefs);
    }

    /// The hotkey reads as its key caps in the file, switching it off is
    /// remembered as off rather than falling back to the default, and a file
    /// that never mentioned it gets the default.
    #[test]
    fn the_hotkey_is_stored_as_written_and_off_means_off() {
        let on = Prefs { record_hotkey: Some("Ctrl+Alt+Space".parse().unwrap()), ..Prefs::default() };
        assert!(serde_json::to_string(&on).unwrap().contains(r#""record_hotkey":"Ctrl+Alt+Space""#));

        let off = Prefs { record_hotkey: None, ..Prefs::default() };
        let text = serde_json::to_string(&off).unwrap();
        assert_eq!(serde_json::from_str::<Prefs>(&text).unwrap().record_hotkey, None);

        let old: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(old.record_hotkey, None, "no record key unless one is chosen, as on the Mac");

        // A key this build cannot read is not a reason to lose every other
        // setting: the field falls back, the rest loads.
        let odd: Prefs =
            serde_json::from_str(r#"{"hidden": true, "record_hotkey": "Hyper+Q"}"#).unwrap();
        assert!(odd.hidden);
        assert_eq!(odd.record_hotkey, None);
    }
}
