//! The dictation window: hold the key, speak, let go, and it is typed.
//!
//! A second widget rather than a mode of the first one. It can be up while a
//! call is being recorded — two streams of the same microphone, each with its
//! own window — because dictating a sentence into a chat box in the middle of a
//! meeting is a thing people do, and one of the two would otherwise have to
//! stop.
//!
//! It looks like the overlay on purpose: the same glass, the same meter, the
//! same red. It is the same app doing the same listening. What it does *not*
//! have is a record button, a transcript to scroll or anything kept on disk —
//! this text exists for the few seconds before it lands in whatever you were
//! typing into.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameVibrantDark,
    NSAttributedStringNSExtendedStringDrawing, NSAttributedStringNSStringDrawing,
    NSBackingStoreType, NSBezierPath, NSColor, NSFont, NSLineBreakMode, NSMutableParagraphStyle,
    NSTextAlignment,
    NSGraphicsContext, NSPanel, NSScreen, NSStringDrawingOptions, NSView,
    NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowCollectionBehavior,
    NSWindowLevel, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSAttributedString, NSDictionary, NSMutableAttributedString, NSPoint, NSRect,
    NSSize, NSString,
};
use voi_core::dictation::{flush_deadline, Dictation, DictationEvent};
use voi_core::prefs::Prefs;

use crate::mac::meter::RED;
use crate::mac::paste::{deliver, Delivered};
use crate::state::Meter;

/// The glass, and the window that has to be bigger than it for the corners to
/// have somewhere to be soft.
const GLASS: NSSize = NSSize::new(368.0, 93.0);
/// Transparent room around the glass for the edge highlight to spill into.
/// Anything narrower slices the glow off square.
const PAD: f64 = crate::mac::sweep::REACH + 5.0;
const CORNER: f64 = 13.0;
const MARGIN: f64 = 18.0;
const TEXT_SIZE: f64 = 13.0;
/// The strip at the bottom that the meter lives in.
const METER_H: f64 = 22.0;

/// How tall one line of the dictation window's text is, asked of the font
/// rather than assumed — see the transcript, where guessing this put
/// everything vertical slightly out.
fn line_height() -> f64 {
    static CACHED: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        objc2_app_kit::NSLayoutManager::new()
            .defaultLineHeightForFont(&crate::mac::overlay::transcript_font_of(TEXT_SIZE))
    })
}

/// The gap between characters once the key has been let go: the rest of the
/// line arrives as fast as it can still be seen arriving.
const FLUSH: Duration = Duration::from_millis(5);

/// How far the glass travels towards its edge as it comes and goes.
///
/// The *glass* travels, inside a window that stays where it is. Moving a
/// window sixty times a second goes through the window server, which
/// coalesces the moves — the panel arrives in three or four jumps and looks
/// like it is running at fifteen frames a second. Moving what is drawn inside
/// one is an ordinary redraw, at the rate everything else here runs at.
const TRAVEL: f64 = 30.0;

/// How long the parting swell takes to die away — about as long as the window
/// has left once the key comes up: the flush, the paste, the confirmation.
const PARTING: Duration = Duration::from_millis(1_500);

/// How long the window stays once the text is in: not at all. It is already
/// white and already where you are looking; the way out is the last of it.
const DELIVERED: Duration = Duration::ZERO;
/// Unless there is something to read instead — "Copied — press ⌘V", or why
/// the microphone would not open — which needs the time to be read.
const TO_READ: Duration = Duration::from_millis(2_500);

#[derive(Default)]
struct VoiceState {
    meter: Meter,
    /// The level the meter is heading for, as the microphone reports it.
    meter_target: f32,
    /// What has been said and will not change: Soniox has settled it.
    settled: String,
    /// And the part that may still be revised — the last word or two, which
    /// the server rewrites as it hears more.
    tail: String,
    /// Shown instead of both once it has gone somewhere.
    note: Option<String>,
    /// 0 hidden, 1 fully arrived — and a little past 1 as it overshoots.
    open: f64,
    /// Where the glass is relative to home, in points.
    offset: NSPoint,
    /// When each character first existed, and when it stopped being a guess.
    ///
    /// Per character rather than one moving position along the line, which is
    /// what this was and why it blinked. A position has to be pulled back when
    /// the server rewrites the tail shorter, and pulling it back re-dims words
    /// that were already up. A birthday cannot be pulled back: a character
    /// that has been on screen for a tenth of a second has been on screen for
    /// a tenth of a second, whatever happens to the text around it.
    born: Vec<Instant>,
    settled_at: Vec<Instant>,
    listening: bool,
    /// How far the cancel veil has come up, 0 to 1.
    hover: f64,
}

type VoiceShared = Arc<Mutex<VoiceState>>;

struct VoiceIvars {
    shared: VoiceShared,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "VoiVoiceView"]
    #[ivars = VoiceIvars]
    struct VoiceView;

    impl VoiceView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.paint();
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }
    }
);

impl VoiceView {
    fn new(mtm: MainThreadMarker, frame: NSRect, shared: VoiceShared) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(VoiceIvars { shared });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn paint(&self) {
        let (wave, settled, tail, note, open, offset, listening, born, settled_at, hover) = {
            let s = self.ivars().shared.lock().unwrap();
            (
                s.meter.wave(),
                s.settled.clone(),
                s.tail.clone(),
                s.note.clone(),
                s.open,
                s.offset,
                s.listening,
                s.born.clone(),
                s.settled_at.clone(),
                s.hover,
            )
        };
        if open <= 0.01 {
            return;
        }
        let glass = glass_rect(self.bounds().size, offset);

        // A hairline, the same as the overlay's: the frosted backdrop is a real
        // view underneath, and this is what gives it an edge against a bright
        // window behind.
        let border = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            inset(glass, 0.5),
            CORNER,
            CORNER,
        );
        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.10 * open).setStroke();
        border.setLineWidth(1.0);
        border.stroke();

        // The words, hanging from the bottom of their box so the newest line is
        // always the one you can see.
        //
        // Three states, and each one looks like what it is: settled words in
        // white, the tail the server may still rewrite in grey, and once the
        // key is let go the whole thing in white, because by then none of it
        // can change.
        let shown = note.clone().unwrap_or_else(|| format!("{settled}{tail}"));
        if !shown.trim().is_empty() {
            let body = match &note {
                Some(note) => styled(note, open, 1.0),
                None => revealed(&settled, &tail, &born, &settled_at, open),
            };
            let width = glass.size.width - 2.0 * MARGIN;
            let bottom = glass.origin.y + METER_H + 6.0;
            // The window on the text is a whole number of lines tall.
            //
            // Text is drawn from the bottom up, so whatever does not fit is cut
            // off at the top — and cut at an arbitrary height that is half a
            // line of letters with their tops sliced away. Trimming the window
            // to whole lines puts the cut exactly between two of them, so the
            // top line is either there or it is not.
            let line = line_height();
            let room = glass.origin.y + glass.size.height - 12.0 - bottom;
            let top = bottom + (room / line).floor().max(1.0) * line;
            let needed = body
                .boundingRectWithSize_options_context(
                    NSSize::new(width, 1.0e5),
                    NSStringDrawingOptions::UsesLineFragmentOrigin,
                    None,
                )
                .size
                .height;

            // Drawn at its full height with its *bottom* on the bottom of the
            // box, and clipped to the box. Text is laid out downward from the
            // top of whatever rect it is given, so a rect cut to the box shows
            // the beginning and hides everything said after it — which is the
            // wrong half. Anchoring the bottom pushes the beginning up out of
            // sight instead, and the panel reads like a terminal.
            let window = NSRect::new(
                NSPoint::new(glass.origin.x + MARGIN, bottom),
                NSSize::new(width, top - bottom),
            );
            if let Some(ctx) = NSGraphicsContext::currentContext() {
                ctx.saveGraphicsState();
                NSBezierPath::bezierPathWithRect(window).addClip();
                body.drawInRect(NSRect::new(
                    NSPoint::new(window.origin.x, bottom),
                    NSSize::new(width, needed.max(window.size.height)),
                ));
                ctx.restoreGraphicsState();
            }
        }

        // And the meter along the bottom, red while it is listening.
        let tint = if listening { 1.0 } else { 0.0 };
        // Edge to edge, with no inset of its own. The row already fades out
        // at both ends by itself — see `meter::ends_taper` — so what reaches
        // the corner is nothing, and the inset was leaving a visible gap
        // before that fade even began.
        crate::mac::meter::paint(
            glass.origin.x,
            glass.origin.x + glass.size.width,
            glass.origin.y + METER_H / 2.0 + 2.0,
            18.0,
            &wave,
            tint,
            open,
        );

        // Last, so it is over everything: the words, the meter, the border.
        // Putting the pointer here means throwing this away, and the window
        // has to say so with its whole face rather than with a small button in
        // a corner — the gesture is "the pointer is on it", not "the pointer
        // is on a particular part of it".
        if hover > 0.001 {
            let shade = hover * open;
            let veil =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(glass, CORNER, CORNER);
            // Darkened first. The red alone over live text leaves both
            // competing; under it, the words recede to what they are at that
            // moment, which is something you have decided not to keep.
            NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.40 * shade).setFill();
            veil.fill();
            NSColor::colorWithSRGBRed_green_blue_alpha(RED.0, RED.1, RED.2, 0.20 * shade).setFill();
            veil.fill();

            let ring = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                inset(glass, 0.75),
                CORNER,
                CORNER,
            );
            NSColor::colorWithSRGBRed_green_blue_alpha(RED.0, RED.1, RED.2, 0.75 * shade)
                .setStroke();
            ring.setLineWidth(1.5);
            ring.stroke();

            // Lifted towards white rather than the flat red: the word sits on
            // a red wash, and a red on red is a word you have to look for.
            let style = NSMutableParagraphStyle::new();
            style.setAlignment(NSTextAlignment::Center);
            // The system font at the size the transcript is set in, and
            // nothing else: the word belongs to this window, so it is written
            // in the window's own voice rather than announced in a different
            // one.
            let attrs = NSDictionary::from_slices(
                &[
                    unsafe { objc2_app_kit::NSForegroundColorAttributeName },
                    unsafe { objc2_app_kit::NSFontAttributeName },
                    unsafe { objc2_app_kit::NSParagraphStyleAttributeName },
                ],
                &[
                    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.44, 0.40, shade).as_ref()
                        as &AnyObject,
                    NSFont::systemFontOfSize(TEXT_SIZE).as_ref() as &AnyObject,
                    style.as_ref() as &AnyObject,
                ],
            );
            // Unsafe only because the attribute dictionary is untyped; the
            // keys are the documented AppKit constants.
            let label = unsafe {
                NSAttributedString::new_with_attributes(ns_string!("Cancel"), &attrs)
            };
            let height = label.size().height;
            label.drawInRect(NSRect::new(
                NSPoint::new(
                    glass.origin.x,
                    glass.origin.y + (glass.size.height - height) / 2.0,
                ),
                NSSize::new(glass.size.width, height),
            ));
        }
    }
}

/// How long one character takes to come up from nothing.
const ARRIVING: Duration = Duration::from_millis(50);
/// And how long grey takes to become white once the server has settled it.
const WHITENING_TIME: Duration = Duration::from_millis(50);
/// The gap between one character starting and the next.
///
/// A phrase arrives all at once, and writing it out at this pace is what makes
/// it read as writing. Three hundred a second: still seen as writing, and never
/// far enough behind that the paste lands before the window has shown it.
const LETTER: Duration = Duration::from_millis(3);
/// The same, for the sweep of grey turning white — quicker, because it is a
/// second pass over words that are already there.
const WHITENING: Duration = Duration::from_millis(2);
/// The longest the writing may take to catch up with what has been said.
///
/// Speech pauses, Soniox hands over a paragraph, and without this the line
/// would still be writing it out a sentence later. Fifty milliseconds: still
/// long enough for each character to be seen arriving, short enough that the
/// words on screen are the words just spoken — a third of the 150 it was,
/// which left the window a word behind the paste.
const TRAIL: Duration = Duration::from_millis(50);

/// The same text, with its newest characters still arriving.
///
/// Words appearing all at once read as a paste; appearing one at a time they
/// read as speech being heard. The transcript does this, and this is the same
/// app listening to the same voice — it should look the same doing it.
fn revealed(
    settled: &str,
    tail: &str,
    born: &[Instant],
    settled_at: &[Instant],
    alpha: f64,
) -> Retained<NSAttributedString> {
    /// Alpha is rounded to this many steps, so a run of characters at the same
    /// strength is one piece of attributed text rather than one per letter.
    /// Fine enough that the ramp has no visible bands in it.
    const STEPS: f64 = 40.0;

    let out = NSMutableAttributedString::new();
    let now = Instant::now();
    let age = |at: Option<&Instant>, over: Duration| match at {
        Some(at) if *at <= now => {
            (now.duration_since(*at).as_secs_f64() / over.as_secs_f64()).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };

    let mut runs: Vec<(String, f64, f64)> = Vec::new();
    for (i, ch) in settled.chars().chain(tail.chars()).enumerate() {
        // Two ramps, each measured from when this character earned it: one for
        // arriving at all, one for being certain.
        let showing = age(born.get(i), ARRIVING);
        if showing <= 0.0 {
            break;
        }
        let certain = age(settled_at.get(i), WHITENING_TIME);
        let (showing, certain) = (
            (showing * STEPS).round() / STEPS,
            (certain * STEPS).round() / STEPS,
        );
        match runs.last_mut() {
            Some((run, had, was))
                if (*had - showing).abs() < f64::EPSILON
                    && (*was - certain).abs() < f64::EPSILON =>
            {
                run.push(ch)
            }
            _ => runs.push((ch.to_string(), showing, certain)),
        }
    }
    for (run, showing, certain) in runs {
        out.appendAttributedString(&styled(&run, alpha * showing, certain));
    }
    Retained::into_super(out)
}

/// One run of text. `certain` is how far it has turned from the grey of
/// something the server may still rewrite to the white of something settled.
fn styled(text: &str, alpha: f64, certain: f64) -> Retained<NSAttributedString> {
    let mix = |unsure: f64, sure: f64| unsure + (sure - unsure) * certain;
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(
        mix(0.62, 0.96),
        mix(0.66, 0.97),
        mix(0.72, 0.99),
        alpha,
    );
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    let attrs = NSDictionary::from_slices(
        &[
            unsafe { objc2_app_kit::NSForegroundColorAttributeName },
            unsafe { objc2_app_kit::NSFontAttributeName },
            unsafe { objc2_app_kit::NSParagraphStyleAttributeName },
        ],
        &[
            color.as_ref() as &AnyObject,
            crate::mac::overlay::transcript_font_of(TEXT_SIZE).as_ref() as &AnyObject,
            style.as_ref() as &AnyObject,
        ],
    );
    // Unsafe only because the attribute dictionary is untyped; the keys are the
    // documented AppKit constants.
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attrs),
        )
    }
}

fn inset(r: NSRect, d: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(r.origin.x + d, r.origin.y + d),
        NSSize::new(r.size.width - 2.0 * d, r.size.height - 2.0 * d),
    )
}

/// The screen a point is on.
fn screen_at(point: NSPoint, mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    NSScreen::screens(mtm).iter().find(|screen| {
        let f = screen.frame();
        point.x >= f.origin.x
            && point.x <= f.origin.x + f.size.width
            && point.y >= f.origin.y
            && point.y <= f.origin.y + f.size.height
    })
}

/// The glass inside the window. One size, always.
///
/// It used to grow the last of the way in, which meant the text box grew with
/// it — and text in a box that changes width re-wraps. On the way out that is
/// a line of words visibly rearranging themselves as the panel goes, in the
/// half-second when nobody wants to be shown anything new. The arrival is the
/// window travelling and fading instead, which moves the words without
/// touching how they are set.
fn glass_rect(window: NSSize, offset: NSPoint) -> NSRect {
    NSRect::new(
        NSPoint::new(
            (window.width - GLASS.width) / 2.0 + offset.x,
            (window.height - GLASS.height) / 2.0 + offset.y,
        ),
        GLASS,
    )
}

pub struct Voice {
    panel: Retained<NSPanel>,
    blur: Retained<NSVisualEffectView>,
    view: Retained<VoiceView>,
    /// The red highlight travelling round the border while it listens — the
    /// same one the overlay wears while it records, because it means the same
    /// thing: this is live.
    sweep: crate::mac::sweep::Sweep,
    shared: VoiceShared,
    /// The running stream, while there is one.
    session: Option<Dictation>,
    /// When the microphone was closed, so the last words have a moment to
    /// arrive before whatever we have is used.
    finishing: Option<Instant>,
    /// When the key went down, and when Soniox first answered: what was said
    /// in between was held for it, and has to go through before the end.
    began: Instant,
    connected: Option<Instant>,
    /// Whether the first words have come yet, so how long they took is said
    /// once: that is the wait that is Soniox's rather than the window's.
    heard: bool,
    /// And the moment of letting go, which the meter marks with a pulse.
    released: Option<Instant>,
    /// Where the window sits when it has fully arrived, and which way it
    /// travels to get there.
    home: NSPoint,
    from: (f64, f64),
    /// Whether it is beside the pointer, shooting out of its tip, rather than
    /// on the grid, sliding in from an edge.
    beside_pointer: bool,
    /// When the confirmation went up, and how long it stays.
    confirmed: Option<(Instant, Duration)>,
    /// Whether this dictation is allowed to make a noise, as the settings said
    /// when it started. Kept here so cancelling can say so out loud without
    /// reaching back for the preferences.
    sounds: bool,
    /// Whether the pointer is on the glass right now — the whole of the cancel
    /// gesture, and the thing the window reads back to you in red.
    over: bool,
    /// The same, eased, so the veil comes up rather than appearing.
    hover: f64,
    /// Last frame's mouse button, to catch the press rather than the holding.
    button: bool,
    open: f64,
    /// How fast that is changing. A spring rather than an eased approach: an
    /// exponential arrives and stops dead, and the difference between that and
    /// a thing that settles is the whole feel of it.
    speed: f64,
    wanted: f64,
}

impl Voice {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let size = NSSize::new(
            GLASS.width + 2.0 * (PAD + TRAVEL),
            GLASS.height + 2.0 * (PAD + TRAVEL),
        );
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        // Above everything, and it must never take focus: the whole point is
        // that the app you were typing into stays the app you are typing into.
        panel.setLevel(25 as NSWindowLevel);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        // No system shadow: the glow around the border is the shadow, and a
        // real one would outline the transparent padding it needs.
        panel.setHasShadow(false);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setIgnoresMouseEvents(true);

        let container: Retained<NSView> =
            unsafe { msg_send![NSView::alloc(mtm), initWithFrame: frame] };
        panel.setContentView(Some(&container));

        let blur: Retained<NSVisualEffectView> = unsafe {
            msg_send![NSVisualEffectView::alloc(mtm), initWithFrame: glass_rect(size, NSPoint::new(0.0, 0.0))]
        };
        blur.setMaterial(NSVisualEffectMaterial::Popover);
        blur.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        blur.setState(NSVisualEffectState::Active);
        blur.setAppearance(
            NSAppearance::appearanceNamed(unsafe { NSAppearanceNameVibrantDark }).as_deref(),
        );
        blur.setWantsLayer(true);
        if let Some(layer) = blur.layer() {
            layer.setCornerRadius(CORNER);
            layer.setMasksToBounds(true);
        }
        container.addSubview(&blur);

        let shared: VoiceShared = Default::default();
        let view = VoiceView::new(mtm, frame, shared.clone());
        container.addSubview(&view);
        view.setWantsLayer(true);
        let sweep = match view.layer() {
            Some(root) => crate::mac::sweep::Sweep::attach(&root),
            None => unreachable!("a layer was just asked for"),
        };

        Self {
            panel,
            blur,
            view,
            sweep,
            shared,
            session: None,
            finishing: None,
            began: Instant::now(),
            connected: None,
            heard: false,
            released: None,
            home: NSPoint::new(0.0, 0.0),
            from: (0.0, -1.0),
            beside_pointer: false,
            confirmed: None,
            sounds: false,
            over: false,
            hover: 0.0,
            button: false,
            open: 0.0,
            speed: 0.0,
            wanted: 0.0,
        }
    }

    /// For screenshots: words as though they had been heard, fully arrived
    /// and settled, the window as though it had just come up, and whether
    /// the pointer is on it.
    /// For pictures: the level the meter follows now.
    pub fn set_level(&self, level: f32) {
        self.shared.lock().unwrap().meter_target = level;
    }

    pub fn stage(&mut self, settled: &str, tail: &str, listening: bool, hover: bool, prefs: &Prefs, mtm: MainThreadMarker) {
        let long_ago = Instant::now() - Duration::from_secs(5);
        {
            let mut s = self.shared.lock().unwrap();
            s.settled = settled.to_string();
            s.tail = tail.to_string();
            s.note = None;
            let n = settled.chars().count() + tail.chars().count();
            s.born = vec![long_ago; n];
            s.settled_at = vec![long_ago; settled.chars().count()];
            s.listening = listening;
            s.meter_target = if listening { 0.6 } else { 0.0 };
            s.hover = if hover { 1.0 } else { 0.0 };
        }
        self.place(prefs, mtm);
        self.wanted = 1.0;
        self.open = 1.0;
        self.speed = 0.0;
        self.hover = if hover { 1.0 } else { 0.0 };
    }

    /// For screenshots: the cancel veil up, as though the pointer were on it.
    pub fn set_hover(&mut self, on: bool) {
        self.hover = if on { 1.0 } else { 0.0 };
        self.shared.lock().unwrap().hover = self.hover;
    }

    /// The window's contents drawn into a PNG, on screen or not.
    pub fn render(&self, path: &std::path::Path) {
        {
            let mut s = self.shared.lock().unwrap();
            s.open = self.open;
            s.offset = NSPoint::new(0.0, 0.0);
        }
        if let Some(view) = self.panel.contentView() {
            crate::mac::shot::png(&view, path);
            if crate::mac::shot::showcase() {
                self.blur.setHidden(true);
                crate::mac::shot::png(&view, &crate::mac::shot::content_path(path));
                self.blur.setHidden(false);
            }
        }
    }

    pub fn listening(&self) -> bool {
        self.session.is_some()
    }

    /// Open the microphone and put the window up.
    pub fn start(&mut self, api_key: &str, prefs: &Prefs, mtm: MainThreadMarker) {
        if self.session.is_some() {
            return;
        }
        {
            let mut s = self.shared.lock().unwrap();
            s.settled.clear();
            s.tail.clear();
            s.note = None;
            s.born.clear();
            s.settled_at.clear();
            s.listening = true;
        }
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
        self.place(prefs, mtm);
        self.wanted = 1.0;
        self.panel.orderFrontRegardless();
    }

    /// Close the microphone. The last words are still coming.
    pub fn finish(&mut self) {
        if let Some(session) = &self.session {
            session.finish();
            // Whatever is still queued to be written gets written now.
            //
            // Letting go means you have finished saying it and are watching to
            // see that it was heard. Anything still being spelled out at
            // reading pace at that moment is the app telling you it is behind,
            // which it is not — the words have been right for a while.
            let now = Instant::now();
            let mut s = self.shared.lock().unwrap();
            let mut next = now;
            for born in s.born.iter_mut().filter(|at| **at > now) {
                *born = next;
                next += FLUSH;
            }
            let mut next = now;
            for settled in s.settled_at.iter_mut().filter(|at| **at > now) {
                *settled = next;
                next += FLUSH;
            }
            drop(s);
            self.finishing = Some(Instant::now());
            self.released = Some(Instant::now());
            self.shared.lock().unwrap().listening = false;
        }
    }

    /// Is the pointer on the glass, with something there to cancel?
    ///
    /// The whole of the cancel gesture. Put the pointer on the window and it
    /// turns red and says so; let go of the key there, or click it, and the
    /// words are thrown away instead of typed. Nothing to remember, nothing to
    /// press, and — unlike a key — nothing that has to be asked of macOS
    /// first: where the pointer is has never been a secret.
    pub fn hovered(&self) -> bool {
        self.session.is_some() && self.over
    }

    /// Test the pointer against the glass, in screen coordinates.
    ///
    /// Against the glass rather than the window: the window is larger by the
    /// margin the sweep needs to glow into, and that margin is invisible. A
    /// hit area you cannot see is a hit area that goes off in your hand.
    fn pointer_is_on_the_glass(&self) -> bool {
        if !self.panel.isVisible() {
            return false;
        }
        let frame = self.panel.frame();
        // The live offset, because the glass slides inside its window as the
        // spring settles, and the red has to mean the thing under the pointer.
        // Except beside the pointer: shooting out of its tip, the glass passes
        // under it on the way, and a dictation started and let go that fast
        // would be thrown away. There it is where it is going to be, which is
        // never under the pointer.
        let offset = if self.beside_pointer {
            NSPoint::new(0.0, 0.0)
        } else {
            self.shared.lock().unwrap().offset
        };
        let glass = glass_rect(frame.size, offset);
        let at = objc2_app_kit::NSEvent::mouseLocation();
        let x = frame.origin.x + glass.origin.x;
        let y = frame.origin.y + glass.origin.y;
        at.x >= x && at.x <= x + glass.size.width && at.y >= y && at.y <= y + glass.size.height
    }

    /// Throw it away: close the microphone, keep nothing, type nothing.
    ///
    /// The counterweight to a feature that types into whatever happens to be
    /// in front of you. Letting go commits; this is the way out, and it stays
    /// open through the moment after letting go, while the last words are
    /// still arriving — which is exactly when you notice you did not mean it.
    ///
    /// Same sound as finishing, because the same thing happened to the
    /// microphone. What is different is what is *not* done: `complete` is
    /// never reached, so `deliver` is never called, so nothing is put on the
    /// clipboard and nothing is typed.
    pub fn cancel(&mut self) {
        // Taking it drops it, and dropping a `Dictation` closes the
        // microphone. Whatever the server sends after that lands nowhere: the
        // frame loop only drains a session it still holds.
        if self.session.take().is_none() {
            return;
        }
        self.finishing = None;
        self.confirmed = None;
        self.shared.lock().unwrap().listening = false;
        // Straight out, without the pause that follows a delivery. There is no
        // result to look at, and holding an empty window up would only make
        // you wait to find out that nothing happened.
        self.wanted = 0.0;
        crate::mac::sound::play(crate::mac::sound::Note::End, self.sounds);
        tracing::info!("dictation cancelled");
    }

    /// Put the window where the settings say: beside the pointer when it is
    /// to follow it, and otherwise on the grid's spot, on the screen the
    /// pointer is on.
    fn place(&mut self, prefs: &Prefs, mtm: MainThreadMarker) {
        let surround = PAD + TRAVEL;
        let mouse = objc2_app_kit::NSEvent::mouseLocation();
        let screen = screen_at(mouse, mtm).or_else(|| NSScreen::mainScreen(mtm));
        let Some(screen) = screen else { return };
        // The usable area, so a window at the top is under the menu bar rather
        // than behind it, and one at the bottom clears the Dock.
        let area = screen.visibleFrame();

        if prefs.dictation_follow_pointer {
            let (x, y) = voi_core::prefs::beside_pointer(
                (mouse.x, mouse.y),
                (area.origin.x, area.origin.y),
                (area.size.width, area.size.height),
                (GLASS.width, GLASS.height),
            );
            self.home = NSPoint::new(x - surround, y - surround);
            // Out of the pointer: the corner nearest it starts at its tip and
            // the glass shoots to where it belongs — whichever side of the
            // pointer that turned out to be. Moved, never scaled: scaling the
            // blur under it makes the window server redo the blur every frame,
            // and that is what made it stutter.
            let (_, (dx, dy)) = voi_core::prefs::facing_corner(
                (mouse.x, mouse.y),
                (x, y),
                (GLASS.width, GLASS.height),
            );
            self.beside_pointer = true;
            self.from = ((dx / TRAVEL).clamp(-1.0, 1.0), (dy / TRAVEL).clamp(-1.0, 1.0));
            self.panel.setFrameOrigin(self.home);
            return;
        }

        // Placed by where the *glass* should sit; the window is bigger than it
        // by the padding the glow needs and the room the glass travels in, and
        // that surround is invisible.
        let (x, y) = prefs.dictation_spot.place(
            (area.origin.x, area.origin.y),
            (area.size.width, area.size.height),
            (GLASS.width, GLASS.height),
            prefs.dictation_margin,
        );
        self.home = NSPoint::new(x - surround, y - surround);
        self.from = prefs.dictation_spot.slide();
        self.beside_pointer = false;
        self.panel.setFrameOrigin(self.home);
    }

    /// Called every frame: drain what has been said, animate, and deliver the
    /// text once the stream has finished flushing.
    pub fn refresh(&mut self) {
        self.over = self.pointer_is_on_the_glass();

        // A click on the red is a cancel in its own right, which is what the
        // toggle mode needs: there is no key to let go of there. The press,
        // not the holding — otherwise every frame of a held button is another
        // cancel.
        let down = objc2_app_kit::NSEvent::pressedMouseButtons() & 1 != 0;
        let pressed = down && !self.button;
        self.button = down;
        if pressed && self.hovered() {
            self.cancel();
        }

        self.drain();

        // Nothing more is coming, or nothing more is worth waiting for.
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
                let text = {
                    let s = self.shared.lock().unwrap();
                    voi_core::dictation::assembled(&s.settled, &s.tail)
                };
                self.complete(text);
            }
        }

        if let Some((since, stay)) = self.confirmed {
            if since.elapsed() >= stay {
                self.confirmed = None;
                self.wanted = 0.0;
            }
        }

        // A spring, pulled hard and damped just short of critically, so it
        // arrives quickly and settles with one small overshoot rather than
        // creeping up to its mark.
        //
        // Beside the pointer it is pulled harder still: it comes out of the
        // pointer you are looking at, and anything slower than a flick reads
        // as the window being late.
        let (stiffness, damping) = if self.beside_pointer { (1100.0, 52.0) } else { (420.0, 26.0) };
        let dt = 1.0 / 60.0;
        let before = self.open;
        self.speed += ((self.wanted - self.open) * stiffness - self.speed * damping) * dt;
        self.open += self.speed * dt;
        if (self.open - self.wanted).abs() < 0.002 && self.speed.abs() < 0.05 {
            self.open = self.wanted;
            self.speed = 0.0;
        }
        if self.open <= 0.001 && self.wanted == 0.0 {
            if self.panel.isVisible() {
                self.panel.orderOut(None);
            }
            return;
        }

        let dt = 1.0 / 60.0;
        {
            let mut s = self.shared.lock().unwrap();
            // Letting go sends one pulse down the meter.
            //
            // The row is a delay line — the middle bar is the signal and every
            // other bar is what the middle one held a moment ago — so a single
            // loud frame fed into it travels outward to both ends by itself.
            // Nothing to animate: the shape the meter already has is exactly
            // "the last of it is on its way out", which is what letting go
            // means while the tail of the sentence is still in flight.
            // One long breath out rather than a handful of pulses: the level
            // is held and let down over the time the window has left, so what
            // travels outward is a single fading swell. Repeating bursts each
            // read as another event, and only one thing happened.
            let parting = self.released.map_or(0.0, |at| {
                let done = at.elapsed().as_secs_f64() / PARTING.as_secs_f64();
                (1.0 - done).clamp(0.0, 1.0).powf(1.6) as f32
            });
            // A tenth of a second up and the same down. Instant would be the
            // one thing in this window that snaps.
            let wanted = if self.over && self.session.is_some() { 1.0 } else { 0.0 };
            // Its own step rather than the meter's `dt`, which is f32.
            let step = (1.0 / 60.0) / 0.10_f64;
            self.hover += (wanted - self.hover).clamp(-step, step);
            s.hover = self.hover;

            let target = if s.listening { s.meter_target } else { parting };
            s.meter.tick(target, dt);
            s.meter.tick_phases(dt);
            s.open = self.open.clamp(0.0, 1.0);
            // Everything left to travel, which goes slightly negative as the
            // spring overshoots — the glass sails a little past home and comes
            // back, which is the bounce.
            let away = (1.0 - self.open) * TRAVEL;
            s.offset = NSPoint::new(self.from.0 * away, self.from.1 * away);

            // Give a birthday to every character that has appeared since the
            // last frame, and a settling time to every one the server has just
            // stopped calling a guess.
            //
            // Staggered rather than stamped with the same instant: Soniox
            // hands over a whole phrase at once, and a phrase that all shares
            // one birthday fades in as one block. Each character starts a
            // little after the one before it, so the line writes itself — and
            // never more than [`TRAIL`] behind, which is about half a word.
            let now = Instant::now();
            let settled_len = s.settled.chars().count();
            let length = settled_len + s.tail.chars().count();

            // Never in the past, however far behind the writing has fallen.
            //
            // Catching up used to mean stamping the backlog with a time
            // already gone, and a character born four tenths of a second ago
            // has already finished fading in — so a whole phrase would land at
            // full strength, which is the snap. It catches up by writing
            // *faster* instead: the gap closes so the backlog fits inside
            // [`TRAIL`], and every character still gets its own arrival.
            let stagger = |backlog: usize, ideal: Duration| {
                let backlog = backlog.max(1) as u32;
                ideal.min(TRAIL / backlog)
            };

            let gap = stagger(length.saturating_sub(s.born.len()), LETTER);
            let mut next = s.born.last().map_or(now, |last| (*last + gap).max(now));
            while s.born.len() < length {
                s.born.push(next);
                next += gap;
            }
            let gap = stagger(settled_len.saturating_sub(s.settled_at.len()), WHITENING);
            let mut next = s.settled_at.last().map_or(now, |last| (*last + gap).max(now));
            while s.settled_at.len() < settled_len {
                s.settled_at.push(next);
                next += gap;
            }
        }

        // The highlight follows the glass, keeps its own place around the
        // border between rebuilds, and is only lit while the microphone is.
        let window = self.panel.frame().size;
        let offset = self.shared.lock().unwrap().offset;
        // Built for home and slid to wherever the glass is at the moment: the
        // outline never changes shape, only place.
        let track = inset(glass_rect(window, NSPoint::new(0.0, 0.0)), 0.5);
        let reference = crate::mac::sweep::perimeter(
            NSRect::new(NSPoint::new(0.0, 0.0), GLASS),
            CORNER,
        );
        let dt = 1.0 / 60.0;
        self.sweep.advance(track, CORNER, reference, dt);
        self.sweep.glow(if self.listening() { 1.0 } else { 0.0 }, dt);

        self.sweep.follow(window, track, CORNER, reference);
        if (self.open - before).abs() > 0.0005 {
            // Opaque a third of the way in: seen the moment it starts moving,
            // still arriving after. Fading over the whole travel made it look
            // later than it was.
            self.panel.setAlphaValue((self.open / 0.35).clamp(0.0, 1.0));
            self.blur.setFrame(glass_rect(window, offset));
            self.sweep.shift(window, offset);
        }
        self.view.setNeedsDisplay(true);
    }

    /// Take everything the stream has said since the last frame.
    fn drain(&mut self) {
        let Some(session) = &mut self.session else { return };
        let mut done = None;
        let mut failed = None;
        while let Ok(event) = session.events.try_recv() {
            match event {
                DictationEvent::Level(level) => {
                    self.shared.lock().unwrap().meter_target = level;
                }
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
                    let mut s = self.shared.lock().unwrap();
                    s.settled = settled;
                    s.tail = tail;
                }
                DictationEvent::Done(text) => done = Some(text),
                DictationEvent::Warning(message) => {
                    tracing::warn!(%message, "dictation");
                }
                DictationEvent::Failed(message) => failed = Some(message),
            }
        }
        if let Some(message) = failed {
            self.session = None;
            self.finishing = None;
            let mut s = self.shared.lock().unwrap();
            s.listening = false;
            s.note = Some(message);
            drop(s);
            self.confirmed = Some((Instant::now(), TO_READ));
            return;
        }
        if let Some(text) = done {
            self.complete(text);
        }
    }

    /// The text is final: put it where it was going.
    fn complete(&mut self, text: String) {
        self.session = None;
        self.finishing = None;
        let mut s = self.shared.lock().unwrap();
        s.listening = false;
        // Nothing said, nothing shown. An empty window says it heard nothing
        // perfectly well, and a line of text about it is one more thing to
        // read at the moment you have stopped looking.
        let (note, stay) = if text.trim().is_empty() {
            (String::new(), DELIVERED)
        } else {
            match deliver(&text) {
                Delivered::Pasted => (text, DELIVERED),
                // Say what happened, because nothing appeared where they were
                // looking: the words are a ⌘V away instead.
                Delivered::Copied => (format!("Copied — press ⌘V to paste.\n\n{text}"), TO_READ),
            }
        };
        s.note = Some(note);
        drop(s);
        self.confirmed = Some((Instant::now(), stay));
    }
}
