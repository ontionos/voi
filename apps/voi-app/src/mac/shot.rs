//! `Voi --screenshots DIR`: the overlay, the dictation glass and every
//! Settings tab, drawn into PNGs without any window going on screen — the Mac
//! half of the side-by-side with the Windows build (docs/windows-parity.md).
//!
//! Each state is staged in the shared model, run through the same refresh
//! code for [`FRAMES`] frames, then the window's content view is drawn into a
//! bitmap. What a bitmap of a view cannot hold is the window server's: the
//! blur behind the glass, and the layers the edge light is made of; those are
//! compared from their numbers instead.

use std::path::Path;

use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSBitmapImageFileType, NSView};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSString};

use crate::state::parity::{speech, stage, Shot, DICTATED, FRAMES, SHOTS};
use crate::state::Shared;

/// `VOI_SHOT_SHOWCASE=1`: pictures for people rather than for the
/// comparison with Windows — Retina whatever screen the window is (not) on,
/// and the meter moving as with someone speaking instead of at one level.
pub fn showcase() -> bool {
    std::env::var_os("VOI_SHOT_SHOWCASE").is_some()
}

/// `hud-open.png` → `hud-open-content.png`.
pub fn content_path(path: &Path) -> std::path::PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{stem}-content.png"))
}

/// A Retina bitmap of `size` points.
fn retina(size: objc2_foundation::NSSize) -> Option<objc2::rc::Retained<objc2_app_kit::NSBitmapImageRep>> {
    use objc2::AnyThread;
    let rep = unsafe {
        objc2_app_kit::NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            objc2_app_kit::NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            (size.width * 2.0).round() as isize,
            (size.height * 2.0).round() as isize,
            8,
            4,
            true,
            false,
            objc2_app_kit::NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    rep.setSize(size);
    Some(rep)
}

/// Draw `view` into `path`.
pub fn png(view: &NSView, path: &Path) -> bool {
    let bounds = view.bounds();
    let rep = if showcase() { retina(bounds.size) } else { view.bitmapImageRepForCachingDisplayInRect(bounds) };
    let Some(rep) = rep else { return false };
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let Some(data) = (unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }) else {
        return false;
    };
    data.writeToFile_atomically(&NSString::from_str(&path.to_string_lossy()), true)
}

pub fn run(dir: &Path) -> i32 {
    std::fs::create_dir_all(dir).ok();
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let prefs = voi_core::prefs::Prefs::default();

    let shared: Shared = Default::default();
    let mut overlay = crate::mac::overlay::Overlay::new(mtm, shared.clone());
    let mut voice = crate::mac::voice::Voice::new(mtm);
    for (shot, name) in SHOTS {
        stage(&shared, shot);
        let path = dir.join(format!("{name}.png"));
        match shot {
            Shot::DictationListening | Shot::DictationCancel => {
                voice.stage(DICTATED.0, DICTATED.1, true, shot == Shot::DictationCancel, &prefs, mtm);
                for frame in 0..FRAMES {
                    if showcase() {
                        voice.set_level(speech(frame));
                    }
                    voice.refresh();
                }
                voice.set_hover(shot == Shot::DictationCancel);
                voice.render(&path);
            }
            _ => {
                overlay.offscreen = Some(shot == Shot::HudOpen);
                for frame in 0..FRAMES {
                    {
                        let mut s = shared.lock().unwrap();
                        let level = if showcase() { speech(frame) } else { s.level_mic.max(s.level_system) };
                        let target = if s.is_active() { level } else { 0.0 };
                        s.meter.tick(target, 1.0 / 60.0);
                        s.meter.tick_phases(1.0 / 60.0);
                    }
                    overlay.refresh(&shared);
                }
                overlay.render_png(&path);
            }
        }
        println!("{}", path.display());
    }
    let settings = crate::mac::settings::Settings::new(mtm);
    settings.render_tabs(dir);
    0
}
