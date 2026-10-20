//! Floating always-on-top HUD.
//!
//! At rest it is two things and nothing else: three bars that move with the
//! microphone, and a record dot. Hovering expands it into a scrolling live
//! transcript with a copy button.
//!
//! # Why the panel is bigger than it looks
//!
//! The visible glass is inset by [`PAD`] inside the window. A view clips its
//! drawing at its own bounds, and the frosted backdrop additionally masks to a
//! rounded rect — so anything drawn at the glass edge loses its outward half.
//! The padding is transparent room for the edge glow to spill into. Clicks
//! landing in it are passed through, so the invisible margin does not swallow
//! whatever is behind the widget.
//!
//! The backdrop is an `NSVisualEffectView`, so it frosts what is behind it the
//! way system HUDs do. Everything else is drawn over it in `drawRect`.
//!
//! The record dot is drawn rather than being an `NSButton` image: a button
//! image carries its own bezel and template padding, which read as a dark ring
//! around the dot. The button is still there, sized over the dot, but it is
//! invisible and only collects clicks.
//!
//! It is an `NSPanel` rather than a window so it never steals focus from the
//! call you are on, sits at status level so it stays above full-screen apps,
//! and joins every Space.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameVibrantDark, NSBackingStoreType,
    NSBezierPath, NSButton, NSColor, NSEvent, NSGraphicsContext, NSLineCapStyle,
    NSLineJoinStyle, NSMenu, NSMenuItem, NSFont, NSLayoutManager, NSPanel, NSScreen, NSTextView,
    NSView, NSWindingRule, NSCursor, NSMutableParagraphStyle, NSTextAlignment,
    NSAttributedStringNSStringDrawing,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowCollectionBehavior, NSWindowLevel, NSWindowStyleMask,
};
use objc2_quartz_core::{
    CAGradientLayer, CATransaction,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSAttributedString, NSDictionary,
    NSMutableAttributedString, NSNumber, NSPoint, NSRange, NSRect, NSSize, NSString,
};


use crate::mac::meter::RED;
use crate::state::Shared;

/// Transparent margin around the glass, for the edge sweep to spill into.
///
/// Derived, not chosen: the window clips at its own bounds, and the widest band
/// is centred on the border, so half of it hangs outside. Anything narrower
/// than that slices the glow off square — more obvious than no glow at all.
const PAD: f64 = crate::mac::sweep::REACH + 5.0;

/// Visible glass size at rest: just the meter and the dot.
/// At rest, with nothing to show: a rounded square with the dot in the middle
/// of it. The width is twice the dot's distance from the right edge, so the
/// centring is a consequence of where the dot already is rather than a second
/// number to keep in step with it.
const GLASS_IDLE: NSSize = NSSize::new(2.0 * (DOT_TO_EDGE + HIT_SIZE / 2.0), HEADER_H);
/// Recording, but not being looked at: room for the dot and a short meter.
const GLASS_TINY: NSSize = NSSize::new(78.0, 38.0);
const GLASS_EXPANDED: NSSize = NSSize::new(408.0, 174.0);
const MARGIN: f64 = 16.0;
/// Height of the strip holding the meter and dot, in both sizes.
const HEADER_H: f64 = 38.0;
const CORNER: f64 = 11.0;

/// Gap between the meter and the record dot, and between the dot and the edge.
const BARS_TO_DOT: f64 = 9.0;
const DOT_TO_EDGE: f64 = 12.0;
const HIT_SIZE: f64 = 20.0;
const DOT_RADIUS: f64 = 4.5;
/// How many characters wide the fade-in ramp is at the reveal head.
const FADE: f64 = 5.0;
const TRANSCRIPT_SIZE: f64 = 14.0;
/// Least amount of space to leave between the text and the panel around it.
/// The actual margin is this or a little more — see [`text_margin`].
const TEXT_MARGIN_MIN: f64 = 7.0;

/// Height of one line of transcript, asked of the font rather than assumed.
///
/// Everything vertical is counted in these: how far a wheel notch scrolls, how
/// much re-wrap to tolerate before it counts as new text, and how many lines
/// the panel is tall. A guessed line height puts all three slightly out, and
/// the error shows up as text that does not sit level in the panel.
fn line_height() -> f64 {
    static CACHED: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| NSLayoutManager::new().defaultLineHeightForFont(&transcript_font()))
}

/// How far to drop the text so its ink sits level in the panel.
///
/// Equal margins on the box the text is laid out in are not equal margins on
/// the text you can see. A line box carries white above its capitals and rather
/// more below its baseline, where the descenders go — so a first line measured
/// flush to the top looks closer to the top than the last line looks to the
/// bottom, by the difference between the two. This is half that difference:
/// added above and taken from below, it splits the white evenly.
fn text_nudge() -> f64 {
    static CACHED: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        let font = transcript_font();
        /// Lifted a little beyond what the metrics ask for.
        ///
        /// The metrics balance the space above a capital against the space
        /// below a baseline, but most lines start with neither — a lower-case
        /// word with no ascender leaves more air at the top than a line of
        /// descenders leaves at the bottom, so the block reads as sitting low.
        const OPTICAL_LIFT: f64 = 1.6;

        let above = font.ascender() - font.capHeight();
        let below = -font.descender();
        (below - above) / 2.0 - OPTICAL_LIFT
    })
}

/// The margin above and below the transcript — the same number for both.
///
/// The panel holds a whole number of lines and the remainder is split evenly
/// between the two edges, so the first line stands as far from the header as
/// the last one does from the bottom. Letting the text start hard against the
/// top and leaving whatever is left over at the bottom is what made the two
/// ends look nothing alike.
fn text_margin() -> f64 {
    let space = GLASS_EXPANDED.height - HEADER_H;
    let lines = ((space - 2.0 * TEXT_MARGIN_MIN) / line_height()).floor().max(1.0);
    (space - lines * line_height()) / 2.0
}

/// The copy control: a small glyph in the bottom-right corner, floating over
/// the transcript.
///
/// Over it rather than beside it, because reserving a strip for something this
/// small costs more reading area than the collisions are worth — and there are
/// hardly any: the glyph is a corner, and a line long enough to reach it wraps
/// its last word to the next line anyway.
///
/// It carries its own confirmation. A "Copied ✓" label used to sit beside it,
/// which is a second thing to place, align and hide for a message the icon can
/// give by turning green with a tick on it.
const COPY_SIZE: f64 = 24.0;
/// How far the *glyph* is inset from the corner. The click target around it is
/// [`COPY_SIZE`] and reaches further — a target the size of the glyph is one
/// you fall off by moving a couple of points, which is what made it flicker
/// between its hovered and resting size. It stays clear of the glass edge,
/// though: a target that reaches the corner invites the pointer to the one
/// place where a further step leaves the panel and closes the whole thing.
const COPY_INSET: f64 = 11.0;
/// The glyph inside that button. The clipboard and the tick it becomes are
/// both this size, so nothing shifts when one replaces the other.
const COPY_GLYPH: f64 = 13.0;

/// Above normal windows and full-screen apps. `NSStatusWindowLevel` is 25.
const STATUS_LEVEL: NSWindowLevel = 25;

fn inset(r: NSRect, d: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(r.origin.x + d, r.origin.y + d),
        NSSize::new(r.size.width - 2.0 * d, r.size.height - 2.0 * d),
    )
}

fn contains(r: NSRect, p: NSPoint) -> bool {
    p.x >= r.origin.x
        && p.y >= r.origin.y
        && p.x <= r.origin.x + r.size.width
        && p.y <= r.origin.y + r.size.height
}

fn window_size(glass: NSSize) -> NSSize {
    // Room under the glass for the prompt as well. The glass hangs from the
    // top-right corner, so the extra height all falls below it and nothing
    // else moves.
    NSSize::new(glass.width + 2.0 * PAD, glass.height + 2.0 * PAD + PROMPT_ROOM)
}

/// The prompt that appears under the widget when something else takes the
/// microphone: a bubble with a tail pointing up at the record dot.
const PROMPT_W: f64 = 86.0;
const PROMPT_H: f64 = 26.0;
/// Height of the tail, and the gap between it and the glass.
const PROMPT_TAIL: f64 = 7.0;
const PROMPT_GAP: f64 = 6.0;
/// How much taller the window is to hold all that.
const PROMPT_ROOM: f64 = PROMPT_H + PROMPT_TAIL + PROMPT_GAP;

/// Where the prompt's bubble sits: centred under the record dot, which is what
/// its tail points at — and, when the widget is at rest, the middle of it.
fn prompt_rect(glass: NSRect) -> NSRect {
    let dot = hit_rect(glass);
    let centre = dot.origin.x + dot.size.width / 2.0;
    NSRect::new(
        NSPoint::new(
            centre - PROMPT_W / 2.0,
            glass.origin.y - PROMPT_GAP - PROMPT_TAIL - PROMPT_H,
        ),
        NSSize::new(PROMPT_W, PROMPT_H),
    )
}

/// Visible glass size at an open fraction between 0 and 1.
///
/// Linear in `open`, deliberately. `open` itself is driven by exponential
/// approach, which is already quick to start and gentle to settle — and being
/// exponential, it behaves the same in both directions. Layering an ease-out
/// curve on top made closing crawl: that curve is flat near 1.0, which is
/// exactly where a close begins.
fn glass_size(open: f64, live: f64, appear: f64) -> NSSize {
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    // Three sizes, not two: the square it is at rest, the strip it grows into
    // when there is a meter to show, and the panel it opens to under the
    // pointer. Both steps are eased by values that are already eased, so the
    // widget only ever grows and shrinks — it never changes shape at a step.
    let width = lerp(GLASS_IDLE.width, GLASS_TINY.width, live);
    let height = lerp(GLASS_IDLE.height, GLASS_TINY.height, live);
    // And the arrival: it grows the last of the way into place rather than
    // being there suddenly, which is the difference between a thing that
    // appeared and a thing that was always going to.
    let arriving = lerp(0.86, 1.0, appear);
    NSSize::new(
        lerp(width, GLASS_EXPANDED.width, open) * arriving,
        lerp(height, GLASS_EXPANDED.height, open) * arriving,
    )
}

/// Where the glass sits inside the window.
///
/// The window never changes size — resizing it sixty times a second is what
/// made opening and closing stutter. It is always big enough for the expanded
/// panel, and the glass grows down and to the left from a fixed top-right
/// corner, so the widget stays put in the corner of the screen and the panel
/// unfolds into free space rather than off the edge.
fn glass_rect_for(window: NSSize, glass: NSSize) -> NSRect {
    NSRect::new(
        NSPoint::new(
            window.width - PAD - glass.width,
            window.height - PAD - glass.height,
        ),
        glass,
    )
}

/// Click target for the record dot.
///
/// Anchored to the glass's **right** edge, along with the meter. The panel
/// grows leftward, so anything measured from the left edge slides out from
/// under the pointer the moment it expands — which made the record button run
/// away exactly when you reached for it.
fn hit_rect(glass: NSRect) -> NSRect {
    NSRect::new(
        NSPoint::new(
            glass.origin.x + glass.size.width - DOT_TO_EDGE - HIT_SIZE,
            glass.origin.y + glass.size.height - HEADER_H + (HEADER_H - HIT_SIZE) / 2.0,
        ),
        NSSize::new(HIT_SIZE, HIT_SIZE),
    )
}

/// How far the expanded meter starts from the left edge of the glass.
///
/// Measured from the record dot, not chosen: the dot's own side sits this far
/// from the right edge, and the row of bars looks wrong unless it begins the
/// same distance from the other one. The click target around the dot is larger
/// than the dot, which is why this is not simply the dot's inset.
fn wave_margin() -> f64 {
    DOT_TO_EDGE + HIT_SIZE / 2.0 - DOT_RADIUS
}

/// The record dot's colour: [`RED`], waiting or recording alike.
///
/// The same red in both states on the widget itself. What tells them apart is
/// everything around it — the meter arriving, the halo breathing, the panel
/// growing — so the dot does not have to change colour as well, and a record
/// button that is red only once it is too late to press reads oddly.
///
/// The menu-bar icon does the opposite, for the opposite reason: there the dot
/// is all there is, so its colour has to carry the state on its own.
fn dot_color(alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(RED.0, RED.1, RED.2, alpha)
}

/// Colour stops for the transcript's edge fades.
///
/// An edge dissolves only when there is text beyond it. Opaque otherwise, so
/// the first line is as crisp as any other — a permanent fade at the top is
/// what made it look half-erased before anything had scrolled at all.
///
/// The mask is never hidden to switch a fade off, only recoloured: hiding a
/// mask leaves an empty one, and the text under it disappears entirely.
fn mask_stops(top_fades: bool, bottom_fades: bool) -> Retained<NSArray> {
    let edge = |fades: bool| if fades { 0.0 } else { 1.0 };
    let stops: Vec<Retained<AnyObject>> = [edge(top_fades), 1.0, 1.0, edge(bottom_fades)]
        .iter()
        .map(|a| {
            let c = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, *a).CGColor();
            unsafe { Retained::cast_unchecked::<AnyObject>(c) }
        })
        .collect();
    NSArray::from_retained_slice(&stops)
}

/// Where the transcript should be heading, given where it was heading.
///
/// Returns the new target offset, whether the newest line is still being
/// followed, whether to go there at once rather than easing, and how long the
/// document has been shorter than the offset.
///
/// `wheel` is this frame's wheel movement in points, positive when the content
/// should move down — that is, toward earlier text. `bottom` is the largest
/// offset the document allows.
fn scroll_decision(
    target: f64,
    pinned: bool,
    wheel: f64,
    bottom: f64,
    short_for: u32,
) -> (f64, bool, bool, u32) {
    /// How close to the end still counts as being at the end.
    const AT_END: f64 = 1.0;
    /// Frames a shorter document has to hold before it is believed — a quarter
    /// of a second at sixty.
    const SETTLED: u32 = 15;

    if wheel != 0.0 {
        // A gesture is obeyed exactly and immediately: easing something held
        // under a finger reads as lag, not as smoothness. Following resumes
        // only on scrolling back to the newest line.
        let target = (target - wheel).clamp(0.0, bottom);
        return (target, target >= bottom - AT_END, true, 0);
    }
    if !pinned {
        // Someone is reading back. The only thing that may move the text is
        // the document being trimmed away underneath them.
        return (target.min(bottom), false, false, 0);
    }
    if bottom >= target - AT_END {
        // Growing, or already there. Following growth at once is what keeps
        // the newest line against the bottom of the panel.
        return (bottom, true, false, 0);
    }
    // Shrinking. Waited out rather than followed, then followed anyway.
    //
    // The live tail is rewritten every frame as Soniox revises it, and a word
    // crossing the wrap point takes the paragraph one line taller and then one
    // line shorter again; chasing that faithfully made the text rock down and
    // up. But refusing to follow at all left the last line hanging short of the
    // bottom for the rest of the call. A shrink that is still there a quarter
    // of a second later is a real one.
    let short_for = short_for + 1;
    if short_for >= SETTLED {
        (bottom, true, false, 0)
    } else {
        (target, true, false, short_for)
    }
}

/// Where the transcript sits inside the glass: everything below the header,
/// with the same margin above it, below it and to either side.
///
/// While the panel is still opening its top edge is where it will end up — the
/// glass grows downward from a fixed corner — and only its height is short. So
/// the text never moves during the animation; more of it is simply uncovered.
fn content_rect(glass: NSRect) -> NSRect {
    let margin = text_margin();
    let top = glass.origin.y + glass.size.height - HEADER_H - margin;
    let bottom = glass.origin.y + margin;
    NSRect::new(
        NSPoint::new(glass.origin.x + margin, bottom),
        NSSize::new(
            (glass.size.width - 2.0 * margin).max(1.0),
            (top - bottom).max(1.0),
        ),
    )
}

/// The copy control's click target, in the bottom-right corner of the glass.
///
/// Centred on where the glyph sits, and larger than it.
fn copy_rect(glass: NSRect) -> NSRect {
    let centre = NSPoint::new(
        glass.origin.x + glass.size.width - COPY_INSET - COPY_GLYPH / 2.0,
        glass.origin.y + COPY_INSET + COPY_GLYPH / 2.0,
    );
    NSRect::new(
        NSPoint::new(centre.x - COPY_SIZE / 2.0, centre.y - COPY_SIZE / 2.0),
        NSSize::new(COPY_SIZE, COPY_SIZE),
    )
}

/// The outline the sweep paces itself against: the collapsed widget's, so the
/// highlight moves at the speed it was tuned at when the panel opens.
fn tiny_perimeter() -> f64 {
    crate::mac::sweep::perimeter(NSRect::new(NSPoint::new(0.0, 0.0), GLASS_TINY), CORNER)
}

/// The glass rectangle for a view of `size`, at open fraction `open`.
fn glass_rect(size: NSSize, open: f64, live: f64, appear: f64) -> NSRect {
    glass_rect_for(size, glass_size(open, live, appear))
}

struct HudIvars {
    shared: Shared,
    /// The prompt's text, built for a given strength and kept until that
    /// strength changes. Composing an attributed string means a font lookup, a
    /// paragraph style and a dictionary — cheap once, but this is drawn on
    /// every frame the prompt is up, and it showed up in a profile as the most
    /// expensive thing the widget does.
    prompt_text: std::cell::RefCell<Option<(i32, Retained<NSAttributedString>)>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "VoiHudView"]
    #[ivars = HudIvars]
    struct HudView;

    impl HudView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.paint();
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }

        /// Right-click on the header opens a small menu.
        ///
        /// Restricted to the header because the rest of the panel is the
        /// transcript, where a right-click should stay with the text view.
        #[unsafe(method(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> *mut NSMenu {
            let (open, live, appear) = {
                let s = self.ivars().shared.lock().unwrap();
                (s.open, s.tint, s.appear)
            };
            let glass = glass_rect(self.bounds().size, open, live, appear);
            let header = NSRect::new(
                NSPoint::new(glass.origin.x, glass.origin.y + glass.size.height - HEADER_H),
                NSSize::new(glass.size.width, HEADER_H),
            );
            let local = self.convertPoint_fromView(
                event.locationInWindow(),
                unsafe { self.superview() }.as_deref(),
            );
            match contains(header, local).then(|| self.menu()).flatten() {
                // AppKit expects an autoreleased reference here, not an owned one.
                Some(menu) => Retained::autorelease_return(menu),
                None => std::ptr::null_mut(),
            }
        }

        /// Let clicks in the transparent margin fall through to whatever is
        /// behind the widget.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            let (open, live, appear, prompt) = {
                let s = self.ivars().shared.lock().unwrap();
                (s.open, s.tint, s.appear, s.prompt)
            };
            let local =
                self.convertPoint_fromView(point, unsafe { self.superview() }.as_deref());
            let glass = glass_rect(self.bounds().size, open, live, appear);
            let asking = prompt > 0.5 && contains(prompt_rect(glass), local);
            if !contains(glass, local) && !asking {
                return std::ptr::null_mut();
            }
            unsafe { msg_send![super(self), hitTest: point] }
        }

    }
);

/// The transcript's window onto itself: clips it, fades its edges, and takes
/// the scroll wheel.
///
/// There was an `NSScrollView` here. It brought its own scrolling — elastic
/// overscroll, momentum, a clip view that animates itself toward a new origin —
/// and the panel drives the offset every frame, so the two took turns moving
/// the same text. Touching the trackpad made it plain: the wheel handed the
/// offset to AppKit, the next frame handed it back, and the fade mask arrived a
/// beat behind whichever won. Owning the offset outright removes the argument.
#[derive(Default)]
struct ClipIvars {
    /// Wheel movement in points, waiting for the next frame to spend it.
    wheel: std::cell::Cell<f64>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "VoiClipView"]
    #[ivars = ClipIvars]
    struct ClipView;

    impl ClipView {
        /// Collect wheel movement; the frame loop applies it.
        ///
        /// Accumulated rather than acted on, because a trackpad delivers a
        /// stream of small deltas and the text should move once per frame, at
        /// the same moment everything else does.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let (dy, precise) = (event.scrollingDeltaY(), event.hasPreciseScrollingDeltas());
            // A trackpad reports points; a wheel notch reports lines.
            let points = if precise { dy } else { dy * line_height() };
            self.ivars().wheel.set(self.ivars().wheel.get() + points);
        }
    }
);

impl ClipView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ClipIvars::default());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Wheel movement since the last frame, in points.
    fn take_wheel(&self) -> f64 {
        self.ivars().wheel.replace(0.0)
    }
}

impl HudView {
    fn new(mtm: MainThreadMarker, frame: NSRect, shared: Shared) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(HudIvars {
            shared,
            prompt_text: std::cell::RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn paint(&self) {
        let (active, open, pulse, tint, dot_scale, copy, prompt, appear) = {
            let s = self.ivars().shared.lock().unwrap();
            (
                s.is_active(),
                s.open,
                s.meter.pulse,
                s.tint,
                s.dot_scale,
                (s.copy_flash, s.copy_grow),
                s.prompt,
                s.appear,
            )
        };
        let glass = glass_rect(self.bounds().size, open, tint, appear);
        let expanded = open > 0.55;

        // The frosted backdrop is a real view underneath; all we add is a
        // hairline so the panel has a defined edge against a bright window.
        let hairline = inset(glass, 0.5);
        let border =
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(hairline, CORNER, CORNER);
        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.10).setStroke();
        border.setLineWidth(1.0);
        border.stroke();

        if expanded {
            let y = glass.origin.y + glass.size.height - HEADER_H;
            let line = NSRect::new(
                NSPoint::new(glass.origin.x + 10.0, y),
                NSSize::new(glass.size.width - 20.0, 1.0),
            );
            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.10).setFill();
            NSBezierPath::bezierPathWithRect(line).fill();
        }

        // The meter is only there while recording. At rest there is nothing
        // for it to show — it stood as a flat line of dots, which is a widget
        // pretending to work — so the dot is left to say, on its own and
        // slowly, that there is something to press.
        if tint > 0.01 {
            self.paint_wave(glass, tint, tint);
        }
        self.paint_dot(glass, active, pulse, tint, dot_scale);
        self.paint_copy(glass, open, copy.0, copy.1);
        if prompt > 0.01 {
            let step = (prompt * 24.0).round() as i32;
            let mut cache = self.ivars().prompt_text.borrow_mut();
            if cache.as_ref().is_none_or(|(had, _)| *had != step) {
                *cache = Some((step, prompt_text(step as f64 / 24.0)));
            }
            let text = cache.as_ref().map(|(_, text)| text.clone());
            drop(cache);
            if let Some(text) = text {
                paint_prompt(glass, hit_rect(glass), prompt, &text);
            }
        }
    }

    /// The copy control: a clipboard that becomes a green tick for a moment
    /// after it is used, and returns.
    ///
    /// One replaces the other in the same place and at the same size — a tick
    /// laid *over* the clipboard is two things where there should be one, and
    /// reads as clutter. Neither ever appears or leaves abruptly: `flash` is
    /// eased, so the clipboard dims out as the tick comes up and back again.
    ///
    /// Drawn as paths, like the record dot, and for the same reason. As a pair
    /// of system symbols it was an *image*, and an image drawn at a size other
    /// than the one it was rendered at gets snapped to the screen's pixel grid
    /// — so growing it under the pointer arrived in visible steps, and at the
    /// far end of the growth it could round back down. A stroked path has no
    /// such grid: every size in between is exactly itself.
    fn paint_copy(&self, glass: NSRect, open: f64, flash: f64, grow: f64) {
        let fade = ((open - 0.55) / 0.45).clamp(0.0, 1.0);
        if fade <= 0.01 {
            return;
        }
        let hit = copy_rect(glass);
        // A square about the middle of the click target, sized by `scale`.
        let centred = |scale: f64| {
            let side = COPY_GLYPH * scale;
            NSRect::new(
                NSPoint::new(
                    hit.origin.x + (COPY_SIZE - side) / 2.0,
                    hit.origin.y + (COPY_SIZE - side) / 2.0,
                ),
                NSSize::new(side, side),
            )
        };
        // The clipboard grows a little under the pointer and dips when pressed,
        // about its own centre so it does not drift while it does. The tick
        // does not: it appears during the press, and a confirmation that jumps
        // in size on arrival reads as a glitch rather than as an answer.
        let clipboard = fade * (1.0 - flash);
        let tick = fade * flash;

        if clipboard > 0.004 {
            // Grey and half-lit. At full strength it is as bright as the
            // transcript and reads as another word in it.
            let color =
                NSColor::colorWithSRGBRed_green_blue_alpha(0.70, 0.73, 0.78, clipboard * 0.55);
            draw_copy_glyph(centred(1.0 + grow), &color);
        }
        if tick > 0.004 {
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(0.32, 0.80, 0.44, tick);
            draw_tick_glyph(centred(1.0), &color);
        }
    }

    /// The meter, at whatever width the panel currently has.
    ///
    /// One meter for both sizes. The collapsed widget used to be three fat bars
    /// and the open panel a row of thin ones, which meant two things to build,
    /// two to keep looking alike, and a crossfade between them at every hover.
    /// Now it is the same row throughout: bars of the same width on the same
    /// pitch, and simply fewer of them when there is less room.
    ///
    /// The row is fixed. Nothing slides along it — the level starts at the
    /// middle bar and spreads outward to both ends, so the motion is symmetric
    /// and every bar keeps its place. A scrolling history was drawn here
    /// before, and a row of hairlines crawling leftward reads as a recording
    /// waveform, not as a meter.
    fn paint_wave(&self, glass: NSRect, fade: f64, tint: f64) {
        let wave = self.ivars().shared.lock().unwrap().meter.wave();
        // The margin that matches the record dot's, unless the panel is too
        // narrow to spare it: on the collapsed widget that inset is a fifth of
        // the whole width, and it leaves room for a handful of bars.
        let left = glass.origin.x + wave_margin().min(glass.size.width * 0.10);
        let right = hit_rect(glass).origin.x - BARS_TO_DOT;
        let mid = glass.origin.y + glass.size.height - HEADER_H / 2.0;
        crate::mac::meter::paint(left, right, mid, 22.0, &wave, tint, fade);
    }

    /// The record dot, with a breathing halo while recording.
    ///
    /// Halo rings are drawn largest first so each smaller, brighter ring lands
    /// on top. Painting them the other way round leaves the dimmest ring
    /// covering everything and reads as a dark band around the dot.
    fn paint_dot(&self, glass: NSRect, active: bool, pulse: f32, tint: f64, scale: f64) {
        let hit = hit_rect(glass);
        let cx = hit.origin.x + hit.size.width / 2.0;
        let cy = hit.origin.y + hit.size.height / 2.0;

        // Smooth 0→1→0 breathing, with no hard turn at either end.
        let phase = 0.5 - 0.5 * (pulse as f64 * std::f64::consts::TAU).cos();
        if active {
            const RINGS: usize = 10;
            for i in (0..RINGS).rev() {
                let t = i as f64 / (RINGS - 1) as f64;
                let radius = DOT_RADIUS * scale + 1.0 + t * (4.0 + 7.0 * phase);
                let alpha = 0.26 * (1.0 - t).powf(1.6) * (0.32 + 0.68 * phase);
                dot_color(alpha).setFill();
                fill_circle(cx, cy, radius);
            }
        }

        // White at rest, red while recording, crossfading between the two. At
        // rest it also breathes, barely and slowly: the only moving thing on a
        // panel that is otherwise doing nothing, which is what makes it read as
        // something you can press rather than an indicator.
        //
        // Size only. Brightness held flat: a dot that fades in and out reads as
        // a signal going away, where one that swells and settles reads as
        // something waiting to be pressed. It spends most of the cycle at its
        // resting size and only briefly at the top of the breath.
        let idle = 1.0 - tint;
        // Both states breathe: wider at rest, where it is the only sign of
        // life, and enough while recording that the halo has something at its
        // centre moving with it.
        let breath = 1.0 + 0.40 * phase * idle + 0.13 * phase * tint;
        dot_color(1.0).setFill();
        fill_circle(cx, cy, DOT_RADIUS * scale * breath);
    }
}

/// The prompt under the widget: "Record?" in a bubble with a tail pointing up
/// at the record dot.
///
/// Drawn rather than made a window of its own. A second window would need its
/// own level, its own shadow and its own rules about which Spaces it joins, to
/// say one word — and it would arrive a frame late every time this one moves.
fn prompt_text(alpha: f64) -> Retained<NSAttributedString> {
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(NSTextAlignment::Center);
    let attrs = NSDictionary::from_slices(
        &[
            unsafe { objc2_app_kit::NSForegroundColorAttributeName },
            unsafe { objc2_app_kit::NSFontAttributeName },
            unsafe { objc2_app_kit::NSParagraphStyleAttributeName },
        ],
        &[
            NSColor::colorWithSRGBRed_green_blue_alpha(0.94, 0.95, 0.97, alpha).as_ref()
                as &AnyObject,
            NSFont::systemFontOfSize(11.5).as_ref() as &AnyObject,
            style.as_ref() as &AnyObject,
        ],
    );
    // Unsafe only because the attribute dictionary is untyped; the keys are the
    // documented AppKit constants.
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            ns_string!("Record?"),
            Some(&attrs),
        )
    }
}

fn paint_prompt(glass: NSRect, dot: NSRect, alpha: f64, text: &NSAttributedString) {
    let bubble = prompt_rect(glass);
    // The tail leaves the bubble under the dot and points at it, wherever the
    // dot happens to be — which is not the middle of the bubble.
    let tip_x = dot.origin.x + dot.size.width / 2.0;
    let top = bubble.origin.y + bubble.size.height;

    // One outline for both, walked by hand: the top edge simply steps up into
    // the tail and back down again. Drawing a rounded rect and a triangle as
    // two shapes fills correctly but strokes each of them, and the tail's base
    // is then a line across the bubble — the seam that gives away that these
    // are two things stuck together rather than one shape.
    let (left, right) = (bubble.origin.x, bubble.origin.x + PROMPT_W);
    let bottom = bubble.origin.y;
    let r = 7.0;
    let path = NSBezierPath::bezierPath();
    path.moveToPoint(NSPoint::new(left + r, bottom));
    path.lineToPoint(NSPoint::new(right - r, bottom));
    {
        path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
            NSPoint::new(right - r, bottom + r),
            r,
            -90.0,
            0.0,
        );
        path.lineToPoint(NSPoint::new(right, top - r));
        path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
            NSPoint::new(right - r, top - r),
            r,
            0.0,
            90.0,
        );
        path.lineToPoint(NSPoint::new(tip_x + PROMPT_TAIL, top));
        path.lineToPoint(NSPoint::new(tip_x, top + PROMPT_TAIL));
        path.lineToPoint(NSPoint::new(tip_x - PROMPT_TAIL, top));
        path.lineToPoint(NSPoint::new(left + r, top));
        path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
            NSPoint::new(left + r, top - r),
            r,
            90.0,
            180.0,
        );
        path.lineToPoint(NSPoint::new(left, bottom + r));
        path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
            NSPoint::new(left + r, bottom + r),
            r,
            180.0,
            270.0,
        );
    }
    path.closePath();

    NSColor::colorWithSRGBRed_green_blue_alpha(0.13, 0.13, 0.15, 0.94 * alpha).setFill();
    path.fill();
    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.12 * alpha).setStroke();
    path.setLineWidth(1.0);
    path.stroke();

    // Centred by hand vertically: the drawing rect is the top-left of the line,
    // and a line box is taller than the letters in it.
    let line = text.size().height;
    text.drawInRect(NSRect::new(
        NSPoint::new(bubble.origin.x, bubble.origin.y + (PROMPT_H - line) / 2.0),
        NSSize::new(PROMPT_W, line),
    ));
}

/// Two pages, one behind the other: the copy control at rest.
///
/// Everything is a fraction of `rect`, so the glyph is the same drawing at any
/// size. The back page is stroked through a clip that excludes the front one,
/// which is what leaves the gap between them — drawn plainly, its lines run
/// straight across the front page and the two read as a grid rather than as
/// one sheet in front of another.
fn draw_copy_glyph(rect: NSRect, color: &NSColor) {
    let side = rect.size.width;
    let at = |x: f64, y: f64, w: f64, h: f64| {
        NSRect::new(
            NSPoint::new(rect.origin.x + x * side, rect.origin.y + y * side),
            NSSize::new(w * side, h * side),
        )
    };
    let line = side * 0.10;
    let corner = side * 0.16;

    let front = at(0.0, 0.0, 0.72, 0.72);
    let back = at(0.28, 0.28, 0.72, 0.72);

    color.setStroke();
    if let Some(ctx) = NSGraphicsContext::currentContext() {
        ctx.saveGraphicsState();
        // Everything except the front page and the gap around it.
        let mask = NSBezierPath::bezierPathWithRect(inset(rect, -side));
        mask.appendBezierPathWithRoundedRect_xRadius_yRadius(
            inset(front, -line),
            corner + line,
            corner + line,
        );
        mask.setWindingRule(NSWindingRule::EvenOdd);
        mask.addClip();
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(back, corner, corner);
        path.setLineWidth(line);
        path.stroke();
        ctx.restoreGraphicsState();
    }
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(front, corner, corner);
    path.setLineWidth(line);
    path.stroke();
}

/// The tick that replaces it once something has been copied.
fn draw_tick_glyph(rect: NSRect, color: &NSColor) {
    let side = rect.size.width;
    let at = |x: f64, y: f64| {
        NSPoint::new(rect.origin.x + x * side, rect.origin.y + y * side)
    };

    let path = NSBezierPath::bezierPath();
    path.moveToPoint(at(0.12, 0.54));
    path.lineToPoint(at(0.38, 0.24));
    path.lineToPoint(at(0.88, 0.78));
    path.setLineWidth(side * 0.14);
    path.setLineCapStyle(NSLineCapStyle::Round);
    path.setLineJoinStyle(NSLineJoinStyle::Round);
    color.setStroke();
    path.stroke();
}

fn fill_circle(cx: f64, cy: f64, r: f64) {
    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(cx - r, cy - r),
        NSSize::new(r * 2.0, r * 2.0),
    ))
    .fill();
}

pub struct Overlay {
    /// For screenshots: never on screen, and open or shut as told rather
    /// than by where the pointer is.
    pub offscreen: Option<bool>,
    panel: Retained<NSPanel>,
    sweep: crate::mac::sweep::Sweep,
    blur: Retained<NSVisualEffectView>,
    button: Retained<NSButton>,
    copy_button: Retained<NSButton>,
    /// Click target over the "Record?" prompt. Invisible, like the others: the
    /// bubble is painted by the view.
    prompt_button: Retained<NSButton>,
    /// Clips the transcript while the panel is growing. See [`Overlay::layout`].
    clip: Retained<ClipView>,
    transcript: Retained<NSTextView>,
    view: Retained<HudView>,
    /// Where the panel is between collapsed (0.0) and expanded (1.0). Eased
    /// every frame rather than switched, so opening and closing glide.
    open: f64,
    placed: bool,
    /// Last origin written to disk.
    saved_origin: NSPoint,
    /// Settled text currently in the storage, and its length in UTF-16 units.
    /// Only the part after it is rewritten each frame — rebuilding the whole
    /// document at 60 fps would grow linearly with the length of the call.
    committed_shown: String,
    head_len: usize,
    /// Cache key for the rendered tail, so an unchanged frame does no work.
    tail_shown: String,
    /// Fractional reveal position, in characters. Fractional so the character
    /// at the head can fade in rather than blink on.
    reveal: f64,
    /// How far the transcript is scrolled down from its first line, in points.
    /// Eased, so new lines slide the text rather than jumping it.
    scroll_y: f64,
    /// Where that offset is heading. Held separately so a paragraph that
    /// briefly re-wraps cannot drag the text back up.
    scroll_target: f64,
    /// Whether the newest line is still being followed. Cleared as soon as the
    /// wheel is used to read back, so arriving text cannot yank the panel away
    /// from what is being read, and set again on returning to the bottom.
    pinned: bool,
    /// Whether the scroll offset was left behind while the panel was shut, and
    /// so has to be put at the end before anything is drawn.
    scroll_stale: bool,
    /// Frames the document has been shorter than where the text is scrolled
    /// to, so a re-wrap that lasts a frame can be ignored and a real one cannot.
    short_for: u32,
    /// Which edges of the transcript are currently dissolving, top and bottom.
    faded: (bool, bool),
    /// The colour the glass was last laid out for. Its size depends on this as
    /// well as on `open` — see `glass_size` — so a change here has to reach the
    /// layout too, or the frosted backdrop keeps the width it had when
    /// recording stopped and a strip of it is left showing beside the widget.
    laid_out_tint: f64,
    /// And the arrival it was last laid out for, for the same reason.
    laid_out_appear: f64,
    /// The transcript as the panel last built it, and the revision it was
    /// built from. Rebuilt when the words change, not when a frame elapses.
    runs: Vec<(String, bool)>,
    revision: u64,
    /// Eased presence of the "Record?" prompt.
    prompt: f64,
    /// How far the widget has arrived on screen, 0.0–1.0.
    appear: f64,
    /// Eased hover and press states of the record dot.
    dot_hover: f64,
    dot_press: f64,
    /// The same for the copy control, and how far it has turned into its
    /// confirmation.
    copy_hover: f64,
    copy_press: f64,
    copy_flash: f64,
    tint: f64,
    /// Cheap signature of everything `drawRect` looks at. Redrawing sixty times
    /// a second when nothing has moved costs about ten per cent of a core for
    /// an identical picture.
    painted: (i32, i32, i32, i32, i32, bool, bool),
}

impl Overlay {
    pub fn new(mtm: MainThreadMarker, shared: Shared) -> Self {
        let shared_for_layout = shared.clone();
        // The window is always big enough for the expanded panel; only the
        // glass inside it changes size.
        let size = window_size(GLASS_EXPANDED);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        );

        panel.setLevel(STATUS_LEVEL);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        // The glow is the shadow; a system one would outline the padding.
        panel.setHasShadow(false);
        panel.setMovableByWindowBackground(true);
        // Stay put when you switch Spaces or enter a full-screen call.
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setHidesOnDeactivate(false);
        // A HUD must never take focus from the call you are on.
        panel.setBecomesKeyOnlyIfNeeded(true);

        // Plain container so nothing clips the glow; the glass is inset in it.
        let container: Retained<NSView> =
            unsafe { msg_send![NSView::alloc(mtm), initWithFrame: frame] };
        panel.setContentView(Some(&container));

        let blur: Retained<NSVisualEffectView> =
            unsafe { msg_send![NSVisualEffectView::alloc(mtm), initWithFrame: glass_rect(size, 0.0, 0.0, 1.0)] };
        // Popover is the material system floating chrome uses; HUDWindow is
        // flatter and darker, which reads as a painted panel rather than glass.
        blur.setMaterial(NSVisualEffectMaterial::Popover);
        blur.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        blur.setState(NSVisualEffectState::Active);
        // Force the dark variant so the material stays consistent over light
        // and dark windows alike, and light text keeps its contrast.
        blur.setAppearance(
            NSAppearance::appearanceNamed(unsafe { NSAppearanceNameVibrantDark }).as_deref(),
        );
        blur.setWantsLayer(true);
        if let Some(layer) = blur.layer() {
            layer.setCornerRadius(CORNER);
            layer.setMasksToBounds(true);
        }
        container.addSubview(&blur);

        // Above the glass but below the controls, and outside the blur so its
        // glow is not clipped by the rounded mask.
        container.setWantsLayer(true);


        let view = HudView::new(mtm, frame, shared.clone());
        container.addSubview(&view);

        // Invisible: the dot is drawn by the view, this only takes the click.
        let button: Retained<NSButton> = unsafe {
            msg_send![
                NSButton::alloc(mtm),
                initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(HIT_SIZE, HIT_SIZE))
            ]
        };
        button.setBordered(false);
        button.setTransparent(true);
        button.setTitle(ns_string!(""));

        // An icon, not a labelled button — the panel is a HUD, not a form.
        let copy_button: Retained<NSButton> = unsafe {
            msg_send![
                NSButton::alloc(mtm),
                initWithFrame: NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(COPY_SIZE, COPY_SIZE)
                )
            ]
        };
        let prompt_button: Retained<NSButton> = unsafe {
            msg_send![
                NSButton::alloc(mtm),
                initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PROMPT_W, PROMPT_H))
            ]
        };
        prompt_button.setBordered(false);
        prompt_button.setTransparent(true);
        prompt_button.setTitle(ns_string!(""));
        prompt_button.setHidden(true);

        // Invisible, like the record dot's: the icon is painted by the view, so
        // this only has to collect the click.
        copy_button.setBordered(false);
        copy_button.setTransparent(true);
        copy_button.setTitle(ns_string!(""));
        copy_button.setHidden(true);

        // A scrolling text view, not a label: a long call must stay readable
        // all the way back to its beginning. Scrolling is done by moving this
        // view inside the clip below — see `Overlay::follow_text`.
        let full = content_rect(glass_rect_for(size, GLASS_EXPANDED)).size;
        let doc = NSRect::new(NSPoint::new(0.0, 0.0), full);
        let transcript: Retained<NSTextView> =
            unsafe { msg_send![NSTextView::alloc(mtm), initWithFrame: doc] };
        transcript.setEditable(false);
        // Not selectable: an editable-looking text view claims the I-beam
        // cursor for itself, and copying is a button away.
        transcript.setSelectable(false);
        transcript.setDrawsBackground(false);
        // The frame is ours: set from the laid-out height every frame, so
        // AppKit must not also be resizing it. Its width is fixed at the
        // expanded panel's and never changes, even while the panel is still
        // growing — a width that moves re-wraps every line on every frame.
        transcript.setVerticallyResizable(false);
        transcript.setHorizontallyResizable(false);
        transcript.setMinSize(NSSize::new(0.0, 0.0));
        transcript.setMaxSize(NSSize::new(1.0e6, 1.0e6));
        if let Some(container) = unsafe { transcript.textContainer() } {
            container.setWidthTracksTextView(true);
            // Height explicitly unbounded, or the container stops laying out
            // text at the bottom of the view and the transcript ends there.
            container.setHeightTracksTextView(false);
            container.setContainerSize(NSSize::new(full.width, 1.0e7));
        }
        // Nothing between the text and the edge but the padding we choose.
        transcript.setTextContainerInset(NSSize::new(0.0, 0.0));

        let clip = ClipView::new(mtm, doc);
        clip.setWantsLayer(true);
        if let Some(layer) = clip.layer() {
            layer.setMasksToBounds(true);
            // Lines dissolve as they leave rather than being cut off at the
            // edge, which is what makes the movement read as continuous.
            let fade = CAGradientLayer::new();
            fade.setStartPoint(NSPoint::new(0.5, 1.0));
            fade.setEndPoint(NSPoint::new(0.5, 0.0));
            unsafe { fade.setColors(Some(&mask_stops(false, false))) };
            // The top is cut short: a few points, so a line leaving is gone
            // quickly and the eye stays on what it is reading. The bottom
            // dissolves over a whole line instead — text arrives from under
            // the edge rather than appearing at it.
            let arriving = (line_height() / full.height).clamp(0.06, 0.35);
            fade.setLocations(Some(&NSArray::from_retained_slice(&[
                NSNumber::new_f64(0.0),
                NSNumber::new_f64(0.045),
                NSNumber::new_f64(1.0 - arriving),
                NSNumber::new_f64(1.0),
            ])));
            unsafe { layer.setMask(Some(&fade)) };
        }
        clip.addSubview(&transcript);

        view.addSubview(&button);
        view.addSubview(&clip);
        // After the transcript, since it now floats over the text.
        view.addSubview(&copy_button);
        view.addSubview(&prompt_button);

        view.setWantsLayer(true);
        let sweep = match view.layer() {
            Some(root) => crate::mac::sweep::Sweep::attach(&root),
            None => unreachable!("a layer was just asked for"),
        };

        let mut overlay = Self {
            offscreen: None,
            panel,
            sweep,
            blur,
            button,
            copy_button,
            prompt_button,
            clip,
            transcript,
            view,
            open: 0.0,
            placed: false,
            saved_origin: NSPoint::new(f64::MIN, f64::MIN),
            committed_shown: String::new(),
            head_len: 0,
            tail_shown: String::new(),
            reveal: 0.0,
            scroll_y: 0.0,
            scroll_target: 0.0,
            pinned: true,
            scroll_stale: true,
            short_for: 0,
            faded: (false, false),
            laid_out_tint: -1.0,
            laid_out_appear: -1.0,
            runs: Vec::new(),
            revision: u64::MAX,
            prompt: 0.0,
            appear: 0.0,
            dot_hover: 0.0,
            dot_press: 0.0,
            copy_hover: 0.0,
            copy_press: 0.0,
            copy_flash: 0.0,
            tint: 0.0,
            painted: (-1, -1, -1, -1, -1, false, false),
        };
        overlay.layout(0.0, &shared_for_layout);
        overlay
    }

    /// Wire the buttons and the context menu to the controller. Done after
    /// construction because the controller does not exist yet when this is
    /// built.
    pub fn set_target(&self, target: &AnyObject, mtm: MainThreadMarker) {
        unsafe {
            self.button.setTarget(Some(target));
            self.button.setAction(Some(sel!(toggle:)));
            self.copy_button.setTarget(Some(target));
            self.copy_button.setAction(Some(sel!(copyTranscript:)));
            // Asking "Record?" and answering it are the same action as the dot.
            self.prompt_button.setTarget(Some(target));
            self.prompt_button.setAction(Some(sel!(toggle:)));
        }

        // The menu bar item is easy to lose among a row of others, so the
        // overlay carries the same escape hatch.
        let menu = NSMenu::init(NSMenu::alloc(mtm));
        // Unsafe only because the initialiser takes an arbitrary selector and
        // the menu holds an unretained target; both are ours and outlive it.
        unsafe {
            for (title, action) in [
                (ns_string!("Open Current Session"), sel!(openLastTranscript:)),
                (ns_string!("Open Sessions Folder"), sel!(openSessions:)),
            ] {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    title,
                    Some(action),
                    ns_string!(""),
                );
                item.setTarget(Some(target));
                menu.addItem(&item);
            }
            self.view.setMenu(Some(&menu));
        }
    }

    /// Lay out for an open fraction between 0 and 1.
    ///
    /// Interpolating the geometry, rather than swapping between two states, is
    /// what makes the panel grow instead of appearing already open. The window
    /// itself is left alone — only the glass and the controls move.
    fn layout(&mut self, open: f64, shared: &Shared) {
        let open = open.clamp(0.0, 1.0);
        // Every layer touched below gets its new geometry immediately.
        //
        // Core Animation animates layer properties by default, so a frame set
        // during our own animation is tweened *again* over its own quarter
        // second — the layer arrives late, trailing the view it belongs to.
        // That lag is what showed as a mask sliding behind the panel.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let size = self.panel.frame().size;
        let glass = glass_rect(size, open, self.tint, self.appear);
        self.blur.setFrame(glass);
        // Drawing reads this to paint the in-between sizes.
        shared.lock().unwrap().open = open;

        self.button.setFrame(hit_rect(glass));
        // Placed whether or not the panel is open: the prompt belongs to the
        // collapsed widget, which is when a call has just started.
        self.prompt_button.setFrame(prompt_rect(glass));
        self.prompt_button.setHidden(self.prompt <= 0.5);

        // Contents appear only once there is room for them; fading them in
        // from the first pixel looks like clutter sliding around.
        let contents_visible = open > 0.55;
        if contents_visible {
            self.clip.setHidden(false);
            self.copy_button.setHidden(false);
            self.copy_button.setFrame(copy_rect(glass));
            // The clip shrinks with the panel; the text inside it keeps its
            // final width, so lines never re-wrap while the panel is growing.
            // Its top edge does not move — the glass grows downward from a
            // fixed corner — so the text stays put and only more of it shows.
            let visible = content_rect(glass);
            self.clip.setFrame(visible);
            if let Some(mask) = self.clip.layer().and_then(|l| l.mask()) {
                mask.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), visible.size));
            }
            // Fade in over the second half of the opening, so the text arrives
            // once there is room for it rather than sliding into view.
            let fade = ((open - 0.55) / 0.45).clamp(0.0, 1.0);
            self.clip.setAlphaValue(fade);
        } else {
            self.clip.setHidden(true);
            self.copy_button.setHidden(true);
        }

        self.open = open;
        self.laid_out_tint = self.tint;
        self.laid_out_appear = self.appear;
        self.update_sweep(glass);
        self.view.setNeedsDisplay(true);
        CATransaction::commit();
    }

    /// Ease the two controls' hover, press and colour toward where they belong.
    ///
    /// Hover is sampled from the pointer rather than a tracking area: the view
    /// already owns one for the panel, and a second overlapping area would make
    /// the enter and exit callbacks ambiguous.
    fn animate_dot(&mut self, shared: &Shared) {
        let ease = |secs: f64| 1.0 - (-(1.0 / 60.0) / secs).exp();

        let (open, active, copied, asking) = {
            let s = shared.lock().unwrap();
            (s.open, s.is_active(), s.copied_strength(), s.wants_prompt())
        };
        let glass = glass_rect(self.panel.frame().size, open, self.tint, self.appear);
        let local = {
            let mouse = NSEvent::mouseLocation();
            self.panel.convertPointFromScreen(mouse)
        };
        // Not `isHighlighted`: the buttons are transparent so they never draw a
        // highlight, and that flag stayed false — which is why pressing one
        // animated nothing at all.
        let down = NSEvent::pressedMouseButtons() & 1 != 0;
        // Expansion follows the pointer, sampled here rather than driven by
        // `mouseEntered:` and `mouseExited:`. A tracking area rebuilt on every
        // resize — and this one resizes sixty times a second while the panel
        // opens — can miss the exit entirely, and the panel then stays open
        // with the pointer nowhere near it. A test each frame cannot: it says
        // where the pointer is now, not where it was last seen crossing a line.
        let over_panel = self.offscreen.unwrap_or_else(|| contains(glass, local));
        // Whatever is behind the widget does not get to set the cursor over it.
        // A text document underneath turns the pointer into an I-beam, and the
        // panel is not a place to select text — the copy control is right
        // there. Forced every frame rather than left to cursor rects, which a
        // non-activating panel does not reliably own.
        if over_panel {
            NSCursor::arrowCursor().set();
        }
        let over_dot = contains(hit_rect(glass), local);
        let over_copy = self.is_open() && contains(copy_rect(glass), local);

        // Fast down, slower back up: that asymmetry is what a press feels like.
        let press_speed = |pressed: bool| if pressed { 0.025 } else { 0.09 };
        self.dot_hover += ((over_dot as u8 as f64) - self.dot_hover) * ease(0.09);
        let pressed = over_dot && down;
        self.dot_press += ((pressed as u8 as f64) - self.dot_press) * ease(press_speed(pressed));
        self.tint += ((active as u8 as f64) - self.tint) * ease(0.45);

        // Slower to let go than to take hold, so a hand that wanders off the
        // control by a point does not snap it back to size.
        let hover_speed = if over_copy { 0.09 } else { 0.16 };
        self.copy_hover += ((over_copy as u8 as f64) - self.copy_hover) * ease(hover_speed);
        let pressed = over_copy && down;
        self.copy_press += ((pressed as u8 as f64) - self.copy_press) * ease(press_speed(pressed));
        // Quick in, gentler out: the confirmation should register at once and
        // then let go of your attention.
        let flash = if copied > self.copy_flash { 0.07 } else { 0.16 };
        self.copy_flash += (copied - self.copy_flash) * ease(flash);
        // Slower than anything else here. It is a question, not a reaction, and
        // it should arrive as though it had been considered.
        self.prompt += ((asking as u8 as f64) - self.prompt) * ease(0.30);

        let mut s = shared.lock().unwrap();
        // Grows a little under the pointer, dips when pressed. Press wins, so a
        // click always reads as a click.
        s.dot_scale = 1.0 + 0.20 * self.dot_hover - 0.40 * self.dot_press;
        s.tint = self.tint;
        s.copy_grow = 0.16 * self.copy_hover - 0.30 * self.copy_press;
        s.copy_flash = self.copy_flash;
        s.prompt = self.prompt;
        // Not while a button is held. Dragging the widget moves the window
        // under the pointer, and if it collapses mid-drag the glass shrinks
        // away from underneath — which reads as the window flinching.
        if !down {
            s.expanded = over_panel;
        }
    }

    /// Point the halo at the current glass outline.
    ///
    /// Inside a transaction with implicit animation off: otherwise Core
    /// Animation tweens every path change and the outline lags behind the glass
    /// as it grows.
    fn update_sweep(&mut self, glass: NSRect) {
        self.sweep.follow(
            self.panel.frame().size,
            inset(glass, 0.5),
            CORNER,
            tiny_perimeter(),
        );
    }

    fn is_open(&self) -> bool {
        self.open > 0.55
    }

    /// Called every animation frame. Cheap when nothing changed.
    /// The panel's contents drawn into a PNG, on screen or not.
    pub fn render_png(&self, path: &std::path::Path) {
        if let Some(view) = self.panel.contentView() {
            crate::mac::shot::png(&view, path);
            if crate::mac::shot::showcase() {
                // And the contents alone, for laying over a blurred backdrop:
                // a bitmap cannot hold the window server's blur.
                self.blur.setHidden(true);
                crate::mac::shot::png(&view, &crate::mac::shot::content_path(path));
                self.blur.setHidden(false);
            }
        }
    }

    pub fn refresh(&mut self, shared: &Shared) {
        let (expanded, visible, revision) = {
            let s = shared.lock().unwrap();
            (s.expanded, s.overlay_visible(), s.revision)
        };
        if revision != self.revision {
            self.revision = revision;
            self.runs = shared.lock().unwrap().display_runs();
        }

        // Arriving and leaving are the same animation run in either direction:
        // fading and growing into place. It used to be ordered in at full size
        // and full strength the instant a poll noticed the microphone, which is
        // the one moment the widget is asking to be looked at.
        let rate = 1.0 - (-(1.0 / 60.0) / 0.14f64).exp();
        self.appear += ((visible as u8 as f64) - self.appear) * rate;
        if !visible {
            // Everything the pointer drives goes back to rest while the widget
            // is off screen. These are eased by `animate_dot`, which does not
            // run while hidden — so without this they keep whatever they held
            // when the widget last went away, and the next appearance paints
            // that for a frame or two before the easing catches up. What that
            // looked like was the "Record?" prompt flashing up and vanishing
            // on a widget shown by hand from the menu.
            self.prompt = 0.0;
            self.dot_hover = 0.0;
            self.dot_press = 0.0;
            self.copy_hover = 0.0;
            self.copy_press = 0.0;
            self.copy_flash = 0.0;
            let mut s = shared.lock().unwrap();
            s.prompt = 0.0;
            s.expanded = false;
            s.dot_scale = 1.0;
            s.copy_grow = 0.0;
            s.copy_flash = 0.0;
        }
        if !visible && self.appear < 0.02 {
            self.appear = 0.0;
            if self.panel.isVisible() {
                self.panel.orderOut(None);
            }
            return;
        }
        if !visible && !self.panel.isVisible() {
            return;
        }

        if !self.panel.isVisible() && self.offscreen.is_none() {
            if !self.placed {
                self.restore_position();
                self.placed = true;
            }
            self.panel.orderFrontRegardless();
        }
        self.panel.setAlphaValue(self.appear);
        {
            let mut s = shared.lock().unwrap();
            s.appear = self.appear;
        }
        self.remember_position();

        let runs = std::mem::take(&mut self.runs);
        // Ease toward the hover state. 0.12s is quick enough to feel immediate
        // and slow enough to read as motion.
        let target = if expanded { 1.0 } else { 0.0 };
        if (self.open - target).abs() > 0.0005 {
            let rate = 1.0 - (-(1.0 / 60.0) / 0.12f64).exp();
            let next = self.open + (target - self.open) * rate;
            let next = if (next - target).abs() < 0.005 { target } else { next };
            self.layout(next, shared);
        } else if (self.tint - self.laid_out_tint).abs() > 0.002
            || (self.appear - self.laid_out_appear).abs() > 0.002
        {
            // Same state, different size: both the colour and the arrival
            // change the size of the glass without touching `open`.
            self.layout(self.open, shared);
        }

        let track = inset(
            glass_rect(self.panel.frame().size, self.open, self.tint, self.appear),
            0.5,
        );
        self.sweep.advance(track, CORNER, tiny_perimeter(), 1.0 / 60.0);

        self.animate_dot(shared);

        // The prompt comes and goes without the panel changing size, so its
        // click target is placed here rather than in the layout.
        let asking = self.prompt > 0.5;
        if asking == self.prompt_button.isHidden() {
            let glass =
                glass_rect(self.panel.frame().size, self.open, self.tint, self.appear);
            self.prompt_button.setFrame(prompt_rect(glass));
            self.prompt_button.setHidden(!asking);
        }

        // The sweep only runs while recording, and fades rather than snapping.
        let active = shared.lock().unwrap().is_active();
        self.sweep.glow(if active { 1.0 } else { 0.0 }, 1.0 / 60.0);

        // Ease the reveal toward the end of the live text. Easing rather than a
        // fixed step means it always catches up with speech but never races
        // ahead, and it settles instead of stopping dead. The target overshoots
        // by the ramp width so the last characters finish fading in instead of
        // staying half-transparent once speech pauses.
        let live_chars: usize = runs
            .iter()
            .filter(|(_, live)| *live)
            .map(|(text, _)| text.chars().count())
            .sum();
        let target = live_chars as f64 + FADE;
        if self.reveal > target {
            self.reveal = target;
        }
        self.reveal += (target - self.reveal) * 0.08;
        if target - self.reveal < 0.05 {
            self.reveal = target;
        }


        if self.is_open() {
            self.render(&runs);
            self.follow_text();
        } else {
            // Closed, the transcript is not laid out at all — measuring text
            // nobody can see is work for nothing, and while the panel is shut
            // that is most of the time. The offset is simply marked stale, and
            // the first open frame goes straight to the end.
            //
            // Without that, opening the panel replays every line that arrived
            // while it was shut: the offset is still where the panel was left,
            // and it eases from there to the bottom, scrolling the whole
            // transcript past in a rush.
            self.scroll_stale = true;
        }
        self.runs = runs;

        // Repaint only when the picture actually differs. Quantised, because
        // sub-pixel changes in the meter are not worth a redraw.
        let (meter, active, pulse) = {
            let s = shared.lock().unwrap();
            (s.meter.probe(), s.is_active(), s.meter.pulse)
        };
        let q = |v: f32| (v * 200.0) as i32;
        let signature = (
            // Both ends of the row: what is arriving at the centre and what is
            // leaving at the tips move at different times, and everything
            // between them moves with one or the other.
            q(meter.0) * 256 + q(meter.1),
            q(self.dot_hover as f32 + self.dot_press as f32),
            q(self.tint as f32),
            // The dot breathes whether or not it is recording: a halo while
            // live, and the dot itself, barely, while idle. Coarsely counted:
            // a breath takes three and a half seconds, so twenty steps a
            // second are more than the eye can follow, and the rest would be
            // sixty repaints a second of a translucent panel to move a dot by
            // a fraction of a point.
            (pulse * 72.0) as i32,
            // The prompt fading in, the copy control's confirmation, and its
            // hover — all painted here, all eased.
            q(self.prompt as f32) * 65536
                + q(self.copy_flash as f32) * 256
                + q(self.copy_hover as f32 + self.copy_press as f32),
            active,
            self.open > 0.0,
        );
        if signature != self.painted {
            // Only the strip the meter and the dot live in, when that is all
            // that moved. A repaint of the whole view makes the frosted
            // backdrop under it recomposite, and at sixty frames a second over
            // a panel this size that is most of what the widget costs — for a
            // row of bars occupying its top thirty-eight points.
            let header_only = signature.1 == self.painted.1
                && signature.2 == self.painted.2
                && signature.4 == self.painted.4
                && signature.5 == self.painted.5
                && signature.6 == self.painted.6;
            self.painted = signature;
            if header_only {
                let glass =
                    glass_rect(self.panel.frame().size, self.open, self.tint, self.appear);
                let strip = NSRect::new(
                    NSPoint::new(
                        glass.origin.x,
                        glass.origin.y + glass.size.height - HEADER_H,
                    ),
                    NSSize::new(glass.size.width, HEADER_H),
                );
                self.view.setNeedsDisplayInRect(strip);
            } else {
                self.view.setNeedsDisplay(true);
            }
        }
    }

    /// Settled text in white; live text dimmer, with its newest characters
    /// fading in one by one.
    ///
    /// Only the region from the first live run onward is rewritten each frame.
    /// Rebuilding the whole document at 60 fps would cost more with every
    /// minute of the call.
    fn render(&mut self, runs: &[(String, bool)]) {
        let split = runs.iter().position(|(_, live)| *live).unwrap_or(runs.len());
        let head: String = runs[..split].iter().map(|(t, _)| t.as_str()).collect();
        let live_runs = faded(&runs[split..], self.reveal);

        let key = format!("{:.2}|{}", self.reveal, runs.len());
        let head_changed = head != self.committed_shown;
        if !head_changed && key == self.tail_shown {
            return;
        }

        let Some(storage) = (unsafe { self.transcript.textStorage() }) else { return };

        let live_doc = NSMutableAttributedString::new();
        for (text, alpha, live) in &live_runs {
            live_doc.appendAttributedString(&styled(text, *alpha, *live));
        }

        if head_changed && head.starts_with(&self.committed_shown) {
            // The usual case: a phrase settled and was appended. Replacing the
            // whole document instead re-lays out every line said so far, which
            // is what made the text visibly jump when a sentence finished.
            let added = &head[self.committed_shown.len()..];
            let doc = NSMutableAttributedString::new();
            doc.appendAttributedString(&styled(added, 1.0, false));
            doc.appendAttributedString(&live_doc);

            let end = storage.length();
            storage.replaceCharactersInRange_withAttributedString(
                NSRange::new(self.head_len, end.saturating_sub(self.head_len)),
                &doc,
            );
            self.head_len += NSString::from_str(added).length();
            self.committed_shown = head;
        } else if head_changed {
            // Only when the settled text is not simply longer than before —
            // a new session, or history trimmed from the front.
            self.head_len = NSString::from_str(&head).length();
            let doc = NSMutableAttributedString::new();
            if !head.is_empty() {
                doc.appendAttributedString(&styled(&head, 1.0, false));
            }
            doc.appendAttributedString(&live_doc);
            storage.setAttributedString(&doc);
            self.committed_shown = head;
        } else {
            let end = storage.length();
            storage.replaceCharactersInRange_withAttributedString(
                NSRange::new(self.head_len, end.saturating_sub(self.head_len)),
                &live_doc,
            );
        }
        self.tail_shown = key;
    }

    /// Height of the transcript as laid out, in points.
    ///
    /// Asked of the layout manager rather than read off the view's frame: the
    /// frame is what this function is about to set, so taking the height from
    /// it would be one frame stale — and a height that lags is a text view that
    /// slides into place a moment after the text it holds.
    fn text_height(&self) -> f64 {
        unsafe {
            let (Some(manager), Some(container)) =
                (self.transcript.layoutManager(), self.transcript.textContainer())
            else {
                return 0.0;
            };
            manager.ensureLayoutForTextContainer(&container);
            manager.usedRectForTextContainer(&container).size.height
        }
    }

    /// Glide the transcript toward the newest text, and put it where the wheel
    /// asks when the wheel is used.
    ///
    /// Scrolling is one number, `scroll_y`, and this is the only thing that
    /// moves the text: the view is placed at that offset inside the clip and
    /// nothing else has an opinion. Jumping to the bottom whenever a line
    /// arrived made the whole paragraph hop by a line height, so the offset is
    /// eased — but a wheel gesture is applied whole, since easing direct
    /// manipulation reads as lag rather than smoothness.
    fn follow_text(&mut self) {
        let clip = self.clip.frame().size;
        let doc = self.text_height();
        // Measured against the fully-open panel, not against the clip: the clip
        // is still growing while the panel opens, and scrolling to keep up with
        // a height that changes every frame would slide the text under it.
        let full = content_rect(glass_rect_for(self.panel.frame().size, GLASS_EXPANDED))
            .size
            .height;
        let bottom = (doc - full).max(0.0);

        if self.scroll_stale {
            // Straight to the newest line, and no easing on the way.
            self.scroll_stale = false;
            self.pinned = true;
            self.short_for = 0;
            self.scroll_target = bottom;
            self.scroll_y = bottom;
        }
        let wheel = self.clip.take_wheel();
        let (target, pinned, snap, short_for) =
            scroll_decision(self.scroll_target, self.pinned, wheel, bottom, self.short_for);
        self.scroll_target = target;
        self.pinned = pinned;
        self.short_for = short_for;
        if snap {
            self.scroll_y = target;
        }

        if (self.scroll_y - self.scroll_target).abs() < 0.2 {
            self.scroll_y = self.scroll_target;
        } else {
            // 0.18s: fast enough to keep up with speech, slow enough to read
            // as movement rather than a jump.
            let rate = 1.0 - (-(1.0 / 60.0) / 0.18f64).exp();
            self.scroll_y += (self.scroll_target - self.scroll_y) * rate;
        }

        // Written top-down: the first line sits against the top of the panel
        // and the text grows downward from there, the offset walking it past
        // the window once there is more of it than fits. The panel is a whole
        // number of lines tall with the same margin at both ends, so a full one
        // stands level — see `text_margin`.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.transcript.setFrame(NSRect::new(
            NSPoint::new(0.0, clip.height - doc + self.scroll_y - text_nudge()),
            NSSize::new(self.transcript.frame().size.width, doc),
        ));

        // An edge dissolves only while there is text past it, so the first line
        // is crisp until something is actually above it.
        let fades = (self.scroll_y > 1.0, self.scroll_y < bottom - 1.0);
        if fades != self.faded {
            self.faded = fades;
            if let Some(mask) = self.clip.layer().and_then(|l| l.mask()) {
                if let Some(gradient) = mask.downcast_ref::<CAGradientLayer>() {
                    // Recoloured rather than hidden: hiding a mask does not
                    // switch masking off — it leaves an empty mask, and the
                    // text under it disappears entirely. That is why nothing
                    // showed until enough lines had built up to scroll.
                    unsafe { gradient.setColors(Some(&mask_stops(fades.0, fades.1))) };
                }
            }
        }
        CATransaction::commit();
    }

    /// Put the panel back where it was left, or in the corner on first run.
    fn restore_position(&self) {
        if let Some((x, y)) = voi_core::load_overlay_position() {
            self.panel.setFrameOrigin(NSPoint::new(x, y));
            return;
        }
        self.place_top_right();
    }

    /// Note where it has been dragged to, so it comes back there.
    ///
    /// Written only when it actually moves — the panel is draggable, and a file
    /// write per frame would be absurd.
    fn remember_position(&mut self) {
        let origin = self.panel.frame().origin;
        if (origin.x - self.saved_origin.x).abs() < 1.0
            && (origin.y - self.saved_origin.y).abs() < 1.0
        {
            return;
        }
        self.saved_origin = origin;
        let _ = voi_core::save_overlay_position(origin.x, origin.y);
    }

    fn place_top_right(&self) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(screen) = NSScreen::mainScreen(mtm) else { return };
        let vis = screen.visibleFrame();
        // Place by the glass, not the window: the window is mostly transparent
        // padding, so aligning its edge would leave a visible gap.
        let size = self.panel.frame().size;
        self.panel.setFrameOrigin(NSPoint::new(
            vis.origin.x + vis.size.width - size.width - MARGIN + PAD,
            vis.origin.y + vis.size.height - size.height - MARGIN + PAD,
        ));
    }
}

/// Apply the reveal ramp across a slice of runs.
///
/// Only characters in live runs fade; settled runs mixed in among them stay
/// opaque. Alpha is quantised so a long tail becomes a handful of runs rather
/// than one per character.
fn faded(runs: &[(String, bool)], reveal: f64) -> Vec<(String, f64, bool)> {
    const STEPS: f64 = 10.0;

    let mut out: Vec<(String, f64, bool)> = Vec::new();
    let mut live_index = 0usize;

    for (text, live) in runs {
        if !*live {
            // Speaker labels and newlines are structure, not speech.
            out.push((text.clone(), 1.0, false));
            continue;
        }
        for ch in text.chars() {
            let alpha = ((reveal - live_index as f64) / FADE).clamp(0.0, 1.0);
            live_index += 1;
            if alpha <= 0.0 {
                return out;
            }
            let quantised = (alpha * STEPS).round() / STEPS;
            match out.last_mut() {
                Some((t, a, l)) if *l && (*a - quantised).abs() < f64::EPSILON => t.push(ch),
                _ => out.push((ch.to_string(), quantised, true)),
            }
        }
    }
    out
}

/// The face the transcript is set in: SF Mono, the one macOS ships for code.
///
/// Monospaced on purpose. A fixed advance holds the speaker labels and the
/// wrapped lines under each other in a column, so a live transcript reads as a
/// log rather than as prose being retyped in front of you — and the live tail
/// stops shuffling sideways as Soniox revises a word.
fn transcript_font() -> Retained<NSFont> {
    transcript_font_of(TRANSCRIPT_SIZE)
}

/// The same face at whatever size. The dictation window sets its own words a
/// little larger — they are read at a glance, once, and then they are gone.
pub fn transcript_font_of(size: f64) -> Retained<NSFont> {
    // Weight zero is the regular face. Asked for by role rather than by name:
    // the name of the system's monospaced font is not API and has changed.
    NSFont::monospacedSystemFontOfSize_weight(size, 0.0)
}

/// One styled run. `live` text sits a shade dimmer than settled text, so you
/// can see at a glance which words may still change.
fn styled(text: &str, alpha: f64, live: bool) -> Retained<NSAttributedString> {
    let color = if live {
        NSColor::colorWithSRGBRed_green_blue_alpha(0.74, 0.77, 0.81, alpha)
    } else {
        NSColor::colorWithSRGBRed_green_blue_alpha(0.95, 0.96, 0.98, alpha)
    };
    let font = transcript_font();
    let attrs = NSDictionary::from_slices(
        &[unsafe { objc2_app_kit::NSForegroundColorAttributeName }, unsafe {
            objc2_app_kit::NSFontAttributeName
        }],
        &[color.as_ref() as &AnyObject, font.as_ref() as &AnyObject],
    );
    // Unsafe only because the attribute dictionary is untyped; the keys here
    // are the documented AppKit constants.
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attrs),
        )
    }
}

/// Selector used by the menu item that toggles the overlay.
pub fn toggle_selector() -> objc2::runtime::Sel {
    sel!(toggleOverlay:)
}

#[cfg(test)]
mod tests {
    use super::*;










    /// The controls must not slide out from under the pointer when the panel
    /// grows, or reaching for the record dot moves it away.
    #[test]
    fn the_controls_stay_put_while_the_panel_grows() {
        let window = window_size(GLASS_EXPANDED);
        let tiny = hit_rect(glass_rect_for(window, GLASS_TINY));
        let big = hit_rect(glass_rect_for(window, GLASS_EXPANDED));
        assert_eq!(tiny.origin.x, big.origin.x, "the record dot moved");
        assert_eq!(tiny.origin.y, big.origin.y, "the record dot moved");

        // The meter's right-hand end is anchored to the dot as well; only its
        // left end travels, uncovering more of the same row.
        let tiny = glass_rect_for(window, GLASS_TINY);
        let big = glass_rect_for(window, GLASS_EXPANDED);
        assert_eq!(
            hit_rect(tiny).origin.x - BARS_TO_DOT,
            hit_rect(big).origin.x - BARS_TO_DOT,
            "the meter moved"
        );
    }

    /// The panel grows from a fixed top-right corner, so the widget stays in
    /// the screen corner and unfolds into free space.
    #[test]
    fn the_glass_keeps_its_top_right_corner_while_growing() {
        let window = window_size(GLASS_EXPANDED);
        let tiny = glass_rect_for(window, GLASS_TINY);
        let big = glass_rect_for(window, GLASS_EXPANDED);
        assert_eq!(tiny.origin.x + tiny.size.width, big.origin.x + big.size.width);
        assert_eq!(tiny.origin.y + tiny.size.height, big.origin.y + big.size.height);
    }

    /// The glow needs transparent room outside the glass, or it is clipped and
    /// only the inward half survives.
    #[test]
    fn the_window_leaves_room_outside_the_glass() {
        let window = window_size(GLASS_EXPANDED);
        let glass = glass_rect_for(window, GLASS_EXPANDED);
        for room in [
            glass.origin.x,
            glass.origin.y,
            window.width - (glass.origin.x + glass.size.width),
            window.height - (glass.origin.y + glass.size.height),
        ] {
            assert!(
                room >= crate::mac::sweep::REACH,
                "{room}pt of margin cannot hold a glow reaching {}pt",
                crate::mac::sweep::REACH
            );
        }
    }

    /// The transcript stands level: as far from the header as from the bottom
    /// of the panel, and a whole number of lines tall so a full one has no
    /// part-line left over at either end.
    #[test]
    fn the_transcript_is_centred_between_the_header_and_the_bottom() {
        let glass = glass_rect_for(window_size(GLASS_EXPANDED), GLASS_EXPANDED);
        let text = content_rect(glass);

        let below = text.origin.y - glass.origin.y;
        let above =
            glass.origin.y + glass.size.height - HEADER_H - (text.origin.y + text.size.height);
        assert!(
            (above - below).abs() < 0.001,
            "the text sits {above} from the header and {below} from the bottom"
        );
        assert!(below >= TEXT_MARGIN_MIN, "{below} is too tight to the edge");

        let lines = text.size.height / line_height();
        assert!(
            (lines - lines.round()).abs() < 0.001,
            "the panel is {lines} lines tall, which leaves part of one showing"
        );
        // The copy control floats over the corner of the transcript rather than
        // having a strip of its own — but it must stay a corner.
        let copy = copy_rect(glass);
        assert!(
            copy.size.width * copy.size.height < text.size.width * text.size.height / 20.0,
            "the copy control covers too much of the transcript"
        );
    }

    /// The meter fills the header between its two edges, evenly: its left end
    /// as far from the glass as the record dot's right side is.
    #[test]
    fn the_meter_is_inset_like_the_record_dot() {
        let glass = glass_rect_for(window_size(GLASS_EXPANDED), GLASS_EXPANDED);
        let hit = hit_rect(glass);
        let dot_edge = hit.origin.x + hit.size.width / 2.0 + DOT_RADIUS;
        let right = glass.origin.x + glass.size.width - dot_edge;
        assert_eq!(
            wave_margin(),
            right,
            "the meter's margin should match the record dot's"
        );
    }

    /// The transcript is written top-down: the first line against the top of
    /// the panel, and it grows downward from there.
    #[test]
    fn the_transcript_starts_at_the_top() {
        let full = 134.0;
        // `follow_text` places the text view at `clip - doc + offset`; its top
        // edge is that plus its own height.
        let top_of_text = |doc: f64, offset: f64| full - doc + offset + doc;

        assert_eq!(top_of_text(40.0, 0.0), full, "two lines start at the top");
        assert_eq!(top_of_text(400.0, 0.0), full, "so does a long one unscrolled");
        assert_eq!(
            full - doc_bottom(400.0, 400.0 - full),
            full,
            "scrolled to its end, the last line lands on the bottom"
        );
    }

    /// Where the bottom edge of the text sits, in the clip's coordinates.
    fn doc_bottom(doc: f64, offset: f64) -> f64 {
        134.0 - doc + offset
    }

    /// Characters at the head must ramp in, not appear at full strength.
    #[test]
    fn the_newest_characters_fade_in() {
        let runs = faded(&[("abcdefghij".to_string(), true)], 4.0);
        let joined: String = runs.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(joined, "abcd", "nothing past the reveal head may show");

        let first = runs.first().unwrap().1;
        let last = runs.last().unwrap().1;
        assert!(first > last, "older characters must be more opaque than newer");
        assert!(last > 0.0 && last < 1.0, "the head character must be mid-fade");
    }

    /// Once the reveal has run past the end, nothing may stay half-faded.
    #[test]
    fn a_settled_tail_is_fully_opaque() {
        let runs = faded(&[("hello".to_string(), true)], 5.0 + FADE);
        assert!(runs.iter().all(|(_, a, _)| *a >= 0.99), "settled text must not stay dim");
        assert_eq!(runs.len(), 1);
    }

    /// Speaker labels and newlines are structure, not speech, so they must stay
    /// opaque even while the words beside them are still fading in.
    #[test]
    fn settled_runs_among_live_ones_stay_opaque() {
        let runs = faded(&[("You: ".to_string(), false), ("hi".to_string(), true)], 1.0);
        assert_eq!(runs[0].0, "You: ");
        assert_eq!(runs[0].1, 1.0);
        assert!(runs[1].1 < 1.0, "the spoken word should still be fading");
    }

    #[test]
    fn nothing_is_revealed_at_the_start() {
        assert!(faded(&[("hello".to_string(), true)], 0.0).is_empty());
    }
}

