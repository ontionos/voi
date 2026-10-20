//! The row of bars, drawn once and used by both widgets.
//!
//! The overlay puts it in its header; the dictation window puts it under the
//! text. Same drawing, same rules about density and fading ends — two copies of
//! this would drift apart within a week, and the difference would show, because
//! it is the same object appearing twice on the same screen.

use objc2::rc::Retained;
use objc2_app_kit::{NSBezierPath, NSColor};
use objc2_foundation::{NSPoint, NSRect, NSSize};

/// The recording red, one definition for everything that turns red: the record
/// dot and both meters. Anything mixed by hand next to it comes out a shade
/// off, and a red that has drifted towards orange is the thing you notice.
pub const RED: (f64, f64, f64) = (1.0, 0.25, 0.22);

/// The meter's colour: white at rest, exactly [`RED`] while recording.
///
/// One colour for every bar, at full strength. Fading a bar by how loud it is
/// makes a quiet passage look like a half-broken widget — the height already
/// says how loud it is, so the colour does not need to say it again.
pub fn color(tint: f64, alpha: f64) -> Retained<NSColor> {
    let mix = |white: f64, red: f64| white + (red - white) * tint;
    NSColor::colorWithSRGBRed_green_blue_alpha(
        mix(1.0, RED.0),
        mix(1.0, RED.1),
        mix(1.0, RED.2),
        alpha,
    )
}

/// How much of a bar's height survives at `r`, its distance from the centre of
/// the row as a fraction of the way to the end.
///
/// Full height across the middle, then a smooth fall to [`ENDS`] at the last
/// bar. The outermost bars must never reach the heights the middle does — a row
/// that stops at full height has a cut edge, and the eye reads the cut before
/// it reads the wave. They still move, just barely, and the row fades out into
/// the panel instead of ending.
///
/// Measured across the bars actually on screen, not across the meter's own
/// resolution: the collapsed widget shows a dozen of them, and a fade defined
/// over sixty leaves it with one tiny bar on the end of a row of full-height
/// ones.
pub fn ends_taper(r: f64) -> f64 {
    /// Where the fall begins, as a fraction of the half-row.
    const SHOULDER: f64 = 0.55;
    /// What is left of the height at the very last bar.
    const ENDS: f64 = 0.12;

    if r <= SHOULDER {
        return 1.0;
    }
    let t = ((r - SHOULDER) / (1.0 - SHOULDER)).clamp(0.0, 1.0);
    // Smoothstep, so the falloff starts and finishes without a corner.
    let s = t * t * (3.0 - 2.0 * t);
    1.0 - (1.0 - ENDS) * s
}

/// Draw the row between `left` and `right`, centred on `mid`, at most `box_h`
/// tall. `wave` is the meter's own row of levels, at whatever resolution it
/// keeps; how many bars actually appear depends on the room there is.
pub fn paint(
    left: f64,
    right: f64,
    mid: f64,
    box_h: f64,
    wave: &[f32],
    tint: f64,
    fade: f64,
) {
    let width = (right - left).max(1.0);

    // Bars stand close together and take up most of the pitch, so the row is a
    // surface with a shape moving through it rather than a handful of dashes
    // with air between them.
    //
    // Finer on a short row, which is read at a glance; a long one is looked at
    // properly and holds up better a little coarser. Not finer than this,
    // though: a bar thinner than a screen pixel does not come out thinner, only
    // paler and softer, and the row loses its edge. The gap between bars
    // survives at both — a row with no gaps is a bar chart of one bar.
    let room = ((width - 45.0) / 200.0).clamp(0.0, 1.0);
    let pitch = 1.8 + (2.1 - 1.8) * room;
    let fill = 0.58 + (0.62 - 0.58) * room;

    // As many as fit at that pitch, kept odd so one of them is the centre.
    let slots = ((width / pitch) as usize).max(3) | 1;
    let last = (wave.len() - 1) as f64;
    let span = last / (slots - 1) as f64;
    let wave: Vec<f32> = (0..slots)
        .map(|i| {
            // The stretch of the row this bar stands for, averaged rather than
            // sampled. Taking the nearest moment instead means a narrow panel
            // shows every fifth one and skips the rest, and a wave travelling
            // past turns into a twitch — neighbouring bars showing moments a
            // tenth of a second apart with nothing in between. Averaging shows
            // all of it, quieter.
            //
            // The windows overlap: neighbouring bars share part of what they
            // average, so the row bends from one to the next instead of each
            // standing on its own patch of the last half-second.
            let window = span * 1.6;
            let centre = i as f64 * span;
            let from = (centre - window / 2.0).max(0.0).round() as usize;
            let to = ((centre + window / 2.0).min(last).round() as usize).max(from);
            let taken = &wave[from..=to];
            taken.iter().sum::<f32>() / taken.len() as f32
        })
        .collect();

    // Bars sit on a fixed pitch and are centred in the space, so the middle bar
    // lands on the middle of the row whatever the count.
    let step = width / wave.len() as f64;
    let bar_w = (step * fill).clamp(0.9, 2.4);
    let x0 = left + (step - bar_w) / 2.0;

    // Every bar into one path, filled once. As a hundred and forty separate
    // fills — each with its own colour object — the row cost several times what
    // the rest of the widget does, and it is the one thing on screen that
    // genuinely changes every frame.
    let row = NSBezierPath::bezierPath();
    let middle = (wave.len() - 1) as f64 / 2.0;
    for (i, level) in wave.iter().enumerate() {
        // Silence is a line. The floor is a single point tall — enough for the
        // row to still be there, not enough to read as a level. Half-lit bars
        // in a silent room say the meter is broken.
        let taper = ends_taper((i as f64 - middle).abs() / middle);
        let h = (box_h * *level as f64 * taper * fade).max(1.0);
        let rect = NSRect::new(
            NSPoint::new(x0 + i as f64 * step, mid - h / 2.0),
            NSSize::new(bar_w, h),
        );
        // Round by whichever of the two is smaller, or a bar shorter than it is
        // wide comes out as a lens rather than a dash.
        let radius = bar_w.min(h) / 2.0;
        row.appendBezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
    }
    color(tint, fade).setFill();
    row.fill();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row fades out towards its ends instead of stopping at full height,
    /// and it does so over the bars there actually are — which on a short row
    /// is about a dozen, not sixty.
    #[test]
    fn the_row_fades_out_at_both_ends_whatever_its_width() {
        assert_eq!(ends_taper(0.0), 1.0, "the centre is never damped");
        assert_eq!(ends_taper(0.4), 1.0, "nor is the middle of the row");
        assert!(ends_taper(1.0) < 0.2, "the last bar must be a fraction of the rest");

        for bars in [11usize, 25, 121] {
            let middle = (bars - 1) as f64 / 2.0;
            let heights: Vec<f64> = (0..bars)
                .map(|i| ends_taper((i as f64 - middle).abs() / middle))
                .collect();

            // Symmetric, and falling away from the middle outward.
            for i in 0..bars / 2 {
                assert_eq!(heights[i], heights[bars - 1 - i], "{bars} bars: not mirrored");
                assert!(heights[i] <= heights[i + 1] + 1e-9, "{bars} bars: not monotonic");
            }
            // The fade takes more than one bar to happen, at every width.
            // Nothing else would be a fade: a single short bar on the end of a
            // row of full-height ones is a stub, which is what this looked
            // like when the falloff was measured across the meter's own
            // resolution rather than across the bars on screen.
            let end = heights[0];
            let sloping = heights[..bars / 2]
                .iter()
                .filter(|h| **h > end + 0.01 && **h < 0.999)
                .count();
            assert!(sloping >= 2, "{bars} bars: the fade happens in one step");

            // And on the long row — the one that is actually looked at — it is
            // gradual enough that no two neighbours differ by much at all.
            if bars > 100 {
                let step = heights
                    .windows(2)
                    .map(|w| (w[1] - w[0]).abs())
                    .fold(0.0, f64::max);
                assert!(step < 0.08, "{bars} bars: a step of {step} is visible as a step");
            }
        }
    }
}
