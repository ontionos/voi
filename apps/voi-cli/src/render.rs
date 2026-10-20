//! Live terminal rendering.
//!
//! Finalized text scrolls up as normal lines. The unstable tail from each stream
//! is drawn dim on a single line at the bottom and redrawn in place, so the
//! transcript reads as a stable document with a live edge rather than a wall of
//! repeated, mutating text.

use std::io::Write;

use crossterm::{
    cursor,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal, QueueableCommand,
};
use voi_audio::Origin;
use voi_core::{fmt_ts, Segment};

pub struct Renderer {
    pending_mic: String,
    pending_system: String,
    /// Number of pending lines currently drawn, so we know how much to erase.
    drawn: u16,
}

fn color(origin: Origin) -> Color {
    match origin {
        Origin::Mic => Color::Cyan,
        Origin::System => Color::Green,
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self { pending_mic: String::new(), pending_system: String::new(), drawn: 0 }
    }

    pub fn set_pending(&mut self, origin: Origin, text: String) {
        match origin {
            Origin::Mic => self.pending_mic = text,
            Origin::System => self.pending_system = text,
        }
        let _ = self.redraw();
    }

    /// Print a finalized segment above the live edge.
    pub fn commit(&mut self, seg: &Segment) {
        let mut out = std::io::stdout();
        let _ = self.erase(&mut out);

        let lang = seg.language.as_deref().unwrap_or("--");
        let speaker = match (seg.origin, seg.speaker) {
            // A speaker id on the system stream distinguishes remote participants.
            // On the mic stream it is noise: that speaker is known.
            (Origin::System, Some(n)) => format!(" #{n}"),
            _ => String::new(),
        };

        let _ = out
            .queue(SetForegroundColor(Color::DarkGrey))
            .and_then(|o| o.queue(Print(format!("{} ", fmt_ts(seg.start_ms)))))
            .and_then(|o| o.queue(SetForegroundColor(color(seg.origin))))
            .and_then(|o| o.queue(Print(format!("{}{speaker}", seg.origin.label()))))
            .and_then(|o| o.queue(SetForegroundColor(Color::DarkGrey)))
            .and_then(|o| o.queue(Print(format!(" [{lang}] "))))
            .and_then(|o| o.queue(ResetColor))
            .and_then(|o| o.queue(Print(&seg.text)))
            .and_then(|o| o.queue(Print("\n")));

        let _ = self.draw_pending(&mut out);
        let _ = out.flush();
    }

    pub fn note(&mut self, text: &str, c: Color) {
        let mut out = std::io::stdout();
        let _ = self.erase(&mut out);
        let _ = out
            .queue(SetForegroundColor(c))
            .and_then(|o| o.queue(Print(format!("{text}\n"))))
            .and_then(|o| o.queue(ResetColor));
        let _ = self.draw_pending(&mut out);
        let _ = out.flush();
    }

    fn redraw(&mut self) -> std::io::Result<()> {
        let mut out = std::io::stdout();
        self.erase(&mut out)?;
        self.draw_pending(&mut out)?;
        out.flush()
    }

    fn erase(&mut self, out: &mut std::io::Stdout) -> std::io::Result<()> {
        for _ in 0..self.drawn {
            out.queue(cursor::MoveToPreviousLine(1))?;
            out.queue(terminal::Clear(terminal::ClearType::CurrentLine))?;
        }
        self.drawn = 0;
        Ok(())
    }

    fn draw_pending(&mut self, out: &mut std::io::Stdout) -> std::io::Result<()> {
        let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(100);
        for (origin, text) in [
            (Origin::Mic, self.pending_mic.clone()),
            (Origin::System, self.pending_system.clone()),
        ] {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let prefix = format!("      {} ", origin.label());
            // Keep the live edge to one line so it can be erased deterministically.
            let budget = width.saturating_sub(prefix.len() + 1);
            let shown: String = if text.chars().count() > budget {
                text.chars().skip(text.chars().count() - budget).collect()
            } else {
                text.to_string()
            };
            out.queue(SetForegroundColor(color(origin)))?;
            out.queue(Print(prefix))?;
            out.queue(SetForegroundColor(Color::DarkGrey))?;
            out.queue(Print(shown))?;
            out.queue(ResetColor)?;
            out.queue(Print("\n"))?;
            self.drawn += 1;
        }
        Ok(())
    }

    /// Clear the live edge for good, before final output.
    pub fn finish(&mut self) {
        let mut out = std::io::stdout();
        let _ = self.erase(&mut out);
        let _ = out.flush();
    }
}
