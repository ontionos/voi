//! The travelling edge highlight, drawn once and used by both windows.
//!
//! A soft radial blob, clipped to a soft-edged band along the border, carried
//! around that border by a keyframe animation.
//!
//! Both halves of "soft" matter, and each earlier attempt got only one:
//!
//! - A dashed stroke glows nicely (a layer shadow is a real blur) but has ends,
//!   and round caps are still caps.
//! - A blob clipped by a stroked mask has no ends at all, but a mask is
//!   all-or-nothing across its width, so the light stopped dead at the band's
//!   edge. Worse, Core Animation refuses to render a shadow for a masked layer,
//!   so no blur could be added back.
//! - Stacking a few masked bands to fake the falloff just contributed one hard
//!   edge each, and read as concentric lines.
//!
//! The fix is a mask that itself fades. It is painted once per layout as an
//! image: the outline stroked many times, from wide and faint to narrow and
//! solid, so its alpha ramps smoothly across the border. The blob supplies
//! softness along the border, the mask supplies it across, and neither needs a
//! shadow or a filter.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSBezierPath, NSColor, NSImage};
use objc2_foundation::{ns_string, NSArray, NSNumber, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{
    kCAAnimationPaced, kCAGradientLayerRadial, CAGradientLayer, CAKeyframeAnimation, CALayer,
    CAMediaTiming, CATransaction,
};

/// How far the glow reaches from the border, in points.
pub const REACH: f64 = 20.0;
/// Width of the fully-lit line at the centre of that glow.
const CORE: f64 = 1.4;
/// How quickly the glow falls off, in points. Most of the light lives within
/// this distance of the line.
const FALLOFF: f64 = 4.0;
/// Steps used to paint the glow's cross-section. Enough that consecutive
/// strokes overlap and the falloff has no visible banding.
const STEPS: usize = 56;
/// How strong the glow is relative to the line it surrounds.
const GLOW_ALPHA: f32 = 0.30;
/// Fraction of the border lit at any moment.
const SPAN: f64 = 0.60;

/// Radial falloff of the travelling blob: solid at the centre, gone at the rim.
pub const STOPS: [(f64, f64); 5] = [
    (0.00, 1.00),
    (0.25, 0.74),
    (0.50, 0.32),
    (0.75, 0.07),
    (1.00, 0.00),
];

/// Length of the rounded rectangle's outline.
pub fn perimeter(rect: NSRect, radius: f64) -> f64 {
    let r = radius.min(rect.size.width / 2.0).min(rect.size.height / 2.0);
    let straight_x = (rect.size.width - 2.0 * r).max(0.0);
    let straight_y = (rect.size.height - 2.0 * r).max(0.0);
    2.0 * straight_x + 2.0 * straight_y + std::f64::consts::TAU * r
}

/// How long one lap takes, given the outline length and the length one lap of
/// [`LAP`] seconds was chosen for.
///
/// Neither constant speed nor a constant lap works. Constant speed means a
/// panel with six times the perimeter takes six times as long — the highlight
/// visibly crawls. A constant lap makes the small widget frantic. Raising the
/// ratio to a fractional power splits the difference: six times the border
/// takes about twice as long, which reads as the same motion.
pub fn duration(perimeter: f64, reference: f64) -> f64 {
    const LAP: f64 = 3.4;
    const GROWTH: f64 = 0.35;

    LAP * (perimeter / reference).max(1.0).powf(GROWTH)
}

/// Which of the two things the sweep is made of.
///
/// They are separate on purpose. One crisp hairline reads as a line; one wide
/// faint wash reads as light. Trying to be both at once — a single band with a
/// gradient across it — gave a thick translucent smear that was neither.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Line,
    Glow,
}

/// One of the two, with the layers that draw it.
struct Pass {
    part: Part,
    host: Retained<CALayer>,
    mask: Retained<CALayer>,
    blob: Retained<CAGradientLayer>,
}

pub struct Sweep {
    passes: Vec<Pass>,
    /// Outline the animation was built for.
    built_for: f64,
    /// Outline the mask image was painted for. Tracked separately because
    /// repainting it is the one expensive thing here — forty strokes over the
    /// whole window — and it must not happen on every frame of a resize.
    painted_for: f64,
    /// Where the highlight is around the border, 0.0–1.0.
    ///
    /// Tracked here as well as in Core Animation. The path changes whenever the
    /// window resizes, so the animation has to be rebuilt — and a fresh
    /// animation starts at the beginning, which is why the highlight visibly
    /// jumped back and restarted every time the panel opened or closed.
    /// Carrying the phase across lets each rebuild resume where the last left
    /// off.
    phase: f64,
    /// Eased opacity, so recording starts and stops as a fade rather than a
    /// red ring snapping on.
    alpha: f64,
    /// How far the whole highlight is currently slid from where it was built.
    offset: NSPoint,
}

impl Sweep {
    /// Build the layers and hang them off `root`.
    pub fn attach(root: &CALayer) -> Self {
        let mut passes = Vec::new();
        // Glow first so the line lands on top of it.
        for part in [Part::Glow, Part::Line] {
            let host = CALayer::new();
            let mask = CALayer::new();
            unsafe { host.setMask(Some(&mask)) };
            host.setHidden(true);

            let blob = CAGradientLayer::new();
            blob.setType(unsafe { kCAGradientLayerRadial });
            blob.setStartPoint(NSPoint::new(0.5, 0.5));
            blob.setEndPoint(NSPoint::new(1.0, 1.0));
            let colors: Vec<Retained<AnyObject>> = STOPS
                .iter()
                .map(|(_, a)| {
                    let c =
                        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.30, 0.24, *a).CGColor();
                    unsafe { Retained::cast_unchecked::<AnyObject>(c) }
                })
                .collect();
            unsafe { blob.setColors(Some(&NSArray::from_retained_slice(&colors))) };
            blob.setLocations(Some(&NSArray::from_retained_slice(
                &STOPS.iter().map(|(p, _)| NSNumber::new_f64(*p)).collect::<Vec<_>>(),
            )));
            host.addSublayer(&blob);
            // Above the frosted glass, so the glow lights the panel as well as
            // the wall behind it.
            root.addSublayer(&host);
            passes.push(Pass { part, host, mask, blob });
        }
        Self {
            passes,
            built_for: 0.0,
            painted_for: 0.0,
            phase: 0.0,
            alpha: 0.0,
            offset: NSPoint::new(0.0, 0.0),
        }
    }

    /// Point the highlight at an outline: `track` inside a window of `window`,
    /// with corners of `corner`, pacing itself against `reference`.
    ///
    /// Inside a transaction with implicit animation off: otherwise Core
    /// Animation tweens every path change and the outline lags behind the glass
    /// as it grows.
    pub fn follow(&mut self, window: NSSize, track: NSRect, corner: f64, reference: f64) {
        let full = NSRect::new(self.offset, window);
        let outline = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(track, corner, corner);
        let length = perimeter(track, corner);
        let rebuild = (length - self.built_for).abs() > 0.5;
        let repaint = (length - self.painted_for).abs() > 2.0;

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for Pass { part, host, mask, blob } in &self.passes {
            host.setFrame(full);
            mask.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), window));
            // A couple of points of lag during a resize is invisible;
            // repainting this sixty times a second is not.
            if repaint {
                unsafe {
                    mask.setContents(Some(&Retained::cast_unchecked::<AnyObject>(mask_image(
                        window, track, corner, *part,
                    ))));
                }
            }

            // Sized from the perimeter so the lit stretch stays the same share
            // of the border whatever size the window is.
            let d = length * SPAN;
            blob.setBounds(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(d, d)));

            if rebuild {
                let lap = duration(length, reference);
                let anim = CAKeyframeAnimation::animationWithKeyPath(Some(ns_string!("position")));
                anim.setPath(Some(&outline.CGPath()));
                // Paced: constant speed along the path. Without it the blob
                // crawls on the long edges and darts around the corners.
                anim.setCalculationMode(unsafe { kCAAnimationPaced });
                anim.setDuration(lap);
                anim.setRepeatCount(f32::INFINITY);
                // Resume where the old one was, rather than snapping to the
                // start of the path.
                anim.setTimeOffset(self.phase * lap);
                blob.addAnimation_forKey(&anim, Some(ns_string!("sweep")));
            }
        }
        if repaint {
            self.painted_for = length;
        }
        if rebuild {
            self.built_for = length;
        }
        CATransaction::commit();
    }

    /// Slide the whole highlight — band, blob and its path together — by
    /// `offset`.
    ///
    /// The mask is an image painted for one particular outline and the blob
    /// travels a path built for the same one, both in the layer's coordinates.
    /// So when the thing they surround moves, they have to move with it or
    /// they stay where they were: the ring goes on turning around a rectangle
    /// that is no longer there. Translating the layer moves all three at once
    /// and repaints nothing.
    pub fn shift(&mut self, window: NSSize, offset: NSPoint) {
        self.offset = offset;
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let full = NSRect::new(offset, window);
        for Pass { host, mask, .. } in &self.passes {
            host.setFrame(full);
            mask.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), window));
        }
        CATransaction::commit();
    }

    /// Advance our own copy of the phase by one frame, so a rebuild can pick up
    /// from here instead of restarting.
    pub fn advance(&mut self, track: NSRect, corner: f64, reference: f64, dt: f64) {
        let lap = duration(perimeter(track, corner), reference);
        if lap > 0.0 {
            self.phase = (self.phase + dt / lap).rem_euclid(1.0);
        }
    }

    /// Ease toward being lit or not. Returns whether anything changed.
    pub fn glow(&mut self, wanted: f64, dt: f64) -> bool {
        if (self.alpha - wanted).abs() <= 0.001 {
            return false;
        }
        // 0.35s: long enough to read as a fade, short enough that pressing
        // record still feels immediate.
        let rate = 1.0 - (-dt / 0.35f64).exp();
        self.alpha += (wanted - self.alpha) * rate;
        if (self.alpha - wanted).abs() < 0.004 {
            self.alpha = wanted;
        }

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for Pass { part, host, .. } in &self.passes {
            let strength = match part {
                Part::Line => 1.0,
                Part::Glow => GLOW_ALPHA,
            };
            host.setOpacity(self.alpha as f32 * strength);
            host.setHidden(self.alpha <= 0.0);
        }
        CATransaction::commit();
        true
    }
}

/// Paint the band used to clip one part of the sweep.
fn mask_image(size: NSSize, track: NSRect, corner: f64, part: Part) -> Retained<NSImage> {
    let handler = RcBlock::new(move |_: NSRect| {
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(track, corner, corner);
        match part {
            Part::Line => {
                NSColor::blackColor().setStroke();
                path.setLineWidth(CORE);
                path.stroke();
            }
            Part::Glow => {
                for i in 0..STEPS {
                    // Distance from the line, walked inward from the outer edge.
                    let r = REACH * (1.0 - i as f64 / (STEPS - 1) as f64);
                    // Gaussian, not linear. Spreading the opacity evenly across
                    // the width gave a band as strong ten points out as at one.
                    let alpha = 0.17 * (-(r / FALLOFF).powi(2)).exp();
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, alpha).setStroke();
                    path.setLineWidth(2.0 * r + CORE);
                    path.stroke();
                }
            }
        }
        objc2::runtime::Bool::YES
    });
    // Block-based rather than lockFocus: the latter is deprecated and renders
    // at a fixed scale, which would leave the mask wrong on a Retina display.
    NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lap has to look like the same motion at either size: not the same
    /// speed, not the same time.
    #[test]
    fn the_lap_grows_far_slower_than_the_border() {
        let small = perimeter(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(78.0, 38.0)), 11.0);
        let big = perimeter(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(408.0, 174.0)), 11.0);
        assert!(big / small > 4.0, "the borders differ by a factor of {}", big / small);

        let (short, long) = (duration(small, small), duration(big, small));
        assert!(long > short, "a longer border should take longer");
        assert!(
            long < short * 2.5,
            "but nothing like proportionally: {short:.1}s against {long:.1}s"
        );
    }

    /// The blob has to fade to nothing at its rim, or its edge is a visible
    /// disc travelling around the border.
    #[test]
    fn the_sweep_blob_fades_to_nothing() {
        assert_eq!(STOPS[0].1, 1.0, "solid at the centre");
        assert_eq!(STOPS[STOPS.len() - 1].1, 0.0, "gone at the rim");
        for pair in STOPS.windows(2) {
            assert!(pair[1].0 > pair[0].0, "stops must be ordered");
            assert!(pair[1].1 < pair[0].1, "and fall off monotonically");
        }
    }

    /// The glow is painted as many strokes from wide and faint to narrow and
    /// solid; the falloff across it has to be steep, or the band reads as a
    /// stripe rather than as light.
    #[test]
    fn the_glow_falls_off_steeply_across_the_border() {
        let radius = |i: usize| REACH * (1.0 - i as f64 / (STEPS - 1) as f64);
        let alpha = |r: f64| 0.17 * (-(r / FALLOFF).powi(2)).exp();

        assert_eq!(radius(STEPS - 1), 0.0, "the innermost pass is the line itself");
        assert_eq!(radius(0), REACH, "the outermost pass is the edge of the glow");
        assert!(
            alpha(REACH / 2.0) < alpha(0.0) / 20.0,
            "half way out it is still {:.1}% as strong",
            100.0 * alpha(REACH / 2.0) / alpha(0.0)
        );
    }
}
