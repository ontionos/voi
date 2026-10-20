//! Where the dictation window goes, chosen by pointing at it.
//!
//! Nine dots in the shape of a screen. A list of names — "top left", "bottom
//! centre" — is a description of a picture, and you have to read it and then
//! picture it; the picture answers directly, and it is one click either way.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSBezierPath, NSColor, NSEvent, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use voi_core::prefs::Spot;

/// The frame the dots sit in, drawn to a screen's proportions.
pub const SIZE: NSSize = NSSize::new(112.0, 70.0);
const DOT: f64 = 5.0;
const CHOSEN: f64 = 9.0;
const INSET: f64 = 14.0;

pub struct GridIvars {
    /// Which of the nine is chosen, as an index into [`Spot::ALL`].
    chosen: Cell<usize>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "VoiSpotGrid"]
    #[ivars = GridIvars]
    pub struct SpotGrid;

    impl SpotGrid {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.paint();
        }

        /// Choose whichever dot was clicked nearest.
        ///
        /// Nearest rather than hit-tested: the dots are five points across and
        /// the gaps between them are forty, so a hit test would mean aiming.
        /// Every point in the frame belongs to some dot, and clicking near one
        /// means that one.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let at = self.convertPoint_fromView(event.locationInWindow(), None);
            let mut best = (f64::MAX, 0usize);
            for (i, _) in Spot::ALL.iter().enumerate() {
                let centre = self.centre(i);
                let d = (centre.x - at.x).powi(2) + (centre.y - at.y).powi(2);
                if d < best.0 {
                    best = (d, i);
                }
            }
            self.ivars().chosen.set(best.1);
            self.setNeedsDisplay(true);
        }
    }
);

impl SpotGrid {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(GridIvars { chosen: Cell::new(7) });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn chosen(&self) -> Spot {
        *Spot::ALL.get(self.ivars().chosen.get()).unwrap_or(&Spot::default())
    }

    pub fn choose(&self, spot: Spot) {
        let at = Spot::ALL.iter().position(|s| *s == spot).unwrap_or(7);
        self.ivars().chosen.set(at);
        self.setNeedsDisplay(true);
    }

    /// Where dot `i` sits. The nine are laid out as they are on a screen, so
    /// the order in [`Spot::ALL`] is read across the rows, top row first —
    /// and the top row is the *larger* y, because AppKit counts upward.
    fn centre(&self, i: usize) -> NSPoint {
        let bounds = self.bounds();
        let column = (i % 3) as f64;
        let row = (i / 3) as f64;
        let across = (bounds.size.width - 2.0 * INSET) / 2.0;
        let down = (bounds.size.height - 2.0 * INSET) / 2.0;
        NSPoint::new(
            bounds.origin.x + INSET + column * across,
            bounds.origin.y + bounds.size.height - INSET - row * down,
        )
    }

    fn paint(&self) {
        let bounds = self.bounds();

        // The screen the dots are arranged on, so the picture reads as one.
        let frame = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            NSRect::new(
                NSPoint::new(bounds.origin.x + 0.5, bounds.origin.y + 0.5),
                NSSize::new(bounds.size.width - 1.0, bounds.size.height - 1.0),
            ),
            6.0,
            6.0,
        );
        NSColor::colorWithSRGBRed_green_blue_alpha(0.5, 0.5, 0.55, 0.20).setFill();
        frame.fill();
        NSColor::colorWithSRGBRed_green_blue_alpha(0.5, 0.5, 0.55, 0.45).setStroke();
        frame.setLineWidth(1.0);
        frame.stroke();

        let chosen = self.ivars().chosen.get();
        for i in 0..Spot::ALL.len() {
            let centre = self.centre(i);
            let picked = i == chosen;
            let r = if picked { CHOSEN } else { DOT } / 2.0;
            if picked {
                NSColor::controlAccentColor().setFill();
            } else {
                NSColor::colorWithSRGBRed_green_blue_alpha(0.55, 0.57, 0.62, 0.85).setFill();
            }
            NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                NSPoint::new(centre.x - r, centre.y - r),
                NSSize::new(2.0 * r, 2.0 * r),
            ))
            .fill();
        }
    }
}
