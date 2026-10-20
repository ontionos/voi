//! `Voi.exe --screenshots DIR`: the Windows half of the side-by-side with the
//! Mac (docs/windows-parity.md). The same states as the Mac's `--screenshots`
//! (`state::parity`), with the same words, each run for the same number of
//! frames, then drawn into the main window and read back from its own
//! framebuffer — transparency included, so both sides can be laid on the
//! same desktop grey. Settings' four tabs follow.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, ColorImage, ViewportCommand, ViewportId};

use crate::state::parity::{Shot, FRAMES, SHOTS};

/// What is being drawn now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Parity(Shot),
    Settings(usize),
}

const SETTINGS_TABS: [&str; 4] = ["general", "audio", "dictation", "dictionary"];

pub struct Plan {
    dir: PathBuf,
    index: usize,
    frames: u32,
    asked: bool,
    done: Arc<Mutex<bool>>,
}

impl Plan {
    pub fn new(dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).ok();
        Self { dir, index: 0, frames: 0, asked: false, done: Default::default() }
    }

    fn stages() -> Vec<(Stage, String)> {
        let mut all: Vec<(Stage, String)> = SHOTS.iter().map(|(s, n)| (Stage::Parity(*s), n.to_string())).collect();
        all.extend(SETTINGS_TABS.iter().enumerate().map(|(i, n)| (Stage::Settings(i), format!("settings-{n}"))));
        all
    }

    /// The stage to draw this frame, or `None` once every picture is taken.
    pub fn current(&self) -> Option<Stage> {
        Self::stages().get(self.index).map(|(s, _)| *s)
    }

    /// Whether this frame is the first of its stage, to stage it.
    pub fn entering(&self) -> bool {
        self.frames == 0
    }

    /// Advance a frame; once the stage has run its frames, read the window
    /// back and move on.
    pub fn step(&mut self, ctx: &egui::Context) {
        ctx.request_repaint();
        let Some((_, name)) = Self::stages().get(self.index).cloned() else { return };
        self.frames += 1;
        if self.frames < FRAMES {
            return;
        }
        if !self.asked {
            self.asked = true;
            *self.done.lock().unwrap() = false;
            let path = self.dir.join(format!("{name}.png"));
            let done = self.done.clone();
            let callback = egui::ScreenshotCallback::new(move |image: Arc<ColorImage>| {
                match write(&path, &image) {
                    Ok(()) => tracing::info!(path = %path.display(), "screenshot written"),
                    Err(e) => tracing::error!(error = %e, "screenshot failed"),
                }
                *done.lock().unwrap() = true;
            });
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Screenshot(callback));
            return;
        }
        if *self.done.lock().unwrap() || self.frames > FRAMES + 120 {
            self.index += 1;
            self.frames = 0;
            self.asked = false;
        }
    }
}

/// The picture as a PNG with straight alpha, as the Mac's are.
fn write(path: &std::path::Path, image: &ColorImage) -> anyhow::Result<()> {
    let [w, h] = image.size;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for c in &image.pixels {
        // The framebuffer holds premultiplied colour.
        let a = c.a();
        let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
        rgba.extend_from_slice(&[un(c.r()), un(c.g()), un(c.b()), a]);
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    Ok(())
}
