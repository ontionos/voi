//! Groups a stream of final tokens into readable segments.

use voi_audio::Origin;
use voi_soniox::Token;

use crate::Segment;

/// A pause longer than this starts a new segment. Long enough to survive the
/// natural gaps inside a sentence, short enough that turns stay separate.
const GAP_MS: u64 = 900;

/// Force a segment to close after this much continuous speech, even without a
/// pause. Two reasons: an unbroken monologue is unreadable as one blob, and
/// nothing reaches disk until a segment closes — so an uninterrupted talker
/// could lose minutes of transcript if the process dies.
const MAX_SEGMENT_MS: u64 = 20_000;

/// True for tokens made entirely of punctuation and space.
fn is_punctuation(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && t.chars().all(|c| c.is_ascii_punctuation() || "…—–«»„“”‘’".contains(c))
}

/// How finished segments should be attributed.
#[derive(Clone, Copy, PartialEq)]
pub enum Attribution {
    /// A live call: the microphone is you, system output is everyone else.
    BySide,
    /// A recording with one track: lean on Soniox's diarization instead.
    ByDiarization,
}

pub struct SegmentBuilder {
    origin: Origin,
    attribution: Attribution,
    buf: Vec<Token>,
}

impl SegmentBuilder {
    pub fn new(origin: Origin) -> Self {
        Self { origin, attribution: Attribution::BySide, buf: Vec::new() }
    }

    /// For a file, where there are no sides — only voices.
    pub fn diarized() -> Self {
        Self {
            origin: Origin::Mic,
            attribution: Attribution::ByDiarization,
            buf: Vec::new(),
        }
    }

    fn label(&self, speaker: Option<u32>) -> String {
        match self.attribution {
            Attribution::BySide => self.origin.label().to_string(),
            // Numbered, not named: diarization separates voices, it does not
            // know whose they are.
            Attribution::ByDiarization => match speaker {
                Some(n) => format!("Speaker {n}"),
                None => "Speaker".to_string(),
            },
        }
    }

    /// Feed final tokens. Returns any segments completed by this batch.
    pub fn push(&mut self, tokens: Vec<Token>) -> Vec<Segment> {
        let mut done = Vec::new();
        for t in tokens {
            if let Some(last) = self.buf.last() {
                let gap = t.start_ms.saturating_sub(last.end_ms);
                let span = t.end_ms.saturating_sub(self.buf[0].start_ms);
                // Punctuation belongs to the sentence it follows. Splitting
                // before it strands a segment containing just "." — Soniox
                // often emits the full stop after the pause it describes.
                let splittable = !is_punctuation(&t.text);
                if splittable && (t.speaker != last.speaker || gap > GAP_MS || span > MAX_SEGMENT_MS)
                {
                    if let Some(s) = self.take() {
                        done.push(s);
                    }
                }
            }
            self.buf.push(t);
        }
        done
    }

    /// Text of the segment currently being built, before it closes.
    ///
    /// Consumers need this for a live display: once tokens go final Soniox
    /// stops sending them as pending, but they sit here until a pause closes
    /// the segment. Without showing it, the on-screen text vanishes and then
    /// reappears as a block.
    pub fn open_text(&self) -> String {
        self.buf.iter().map(|t| t.text.as_str()).collect()
    }

    /// Close the open segment if `next_start_ms` begins after a pause.
    ///
    /// Call this with the first *unstable* token of a new phrase. Without it the
    /// split only happens once that phrase is finalized, so a new sentence
    /// spends its whole life appended to the previous one and then jumps onto
    /// its own line the moment it settles — the paragraph visibly re-wraps
    /// under the reader.
    pub fn split_before(&mut self, next_start_ms: u64) -> Option<Segment> {
        let last_end = self.buf.last()?.end_ms;
        if next_start_ms.saturating_sub(last_end) > GAP_MS {
            self.take()
        } else {
            None
        }
    }

    /// When the open segment began, if there is one.
    pub fn open_start_ms(&self) -> Option<u64> {
        self.buf.first().map(|t| t.start_ms)
    }

    /// Close off everything said before `ms`, keeping the rest open.
    ///
    /// For when the other side of the conversation has just had a line land at
    /// `ms`: the words spoken before it belong above that line and the words
    /// after it belong below, and the timestamps say which are which. Without
    /// this the whole open line keeps its original position and goes on
    /// growing there — new words appearing above text that was said later.
    pub fn close_before(&mut self, ms: u64) -> Option<Segment> {
        // Everything, if none of it is after that moment.
        let split = self
            .buf
            .iter()
            .position(|t| t.start_ms >= ms)
            .unwrap_or(self.buf.len());
        if split == 0 {
            return None;
        }
        let rest = self.buf.split_off(split);
        let closed = self.take();
        self.buf = rest;
        closed
    }

    /// Emit whatever is buffered. Call at end of stream, and whenever the other
    /// side of the conversation has just closed a segment — see
    /// [`crate::recorder`].
    pub fn flush(&mut self) -> Option<Segment> {
        self.take()
    }

    fn take(&mut self) -> Option<Segment> {
        if self.buf.is_empty() {
            return None;
        }
        let tokens = std::mem::take(&mut self.buf);

        let text = tokens.iter().map(|t| t.text.as_str()).collect::<String>();
        let text = text.trim().to_string();
        if text.is_empty() {
            return None;
        }

        // Most frequent language wins — a code-switched line is labelled by its
        // dominant language rather than by whichever word happened to land last.
        let mut counts: std::collections::HashMap<&str, usize> = Default::default();
        for t in &tokens {
            if let Some(l) = &t.language {
                *counts.entry(l.as_str()).or_default() += 1;
            }
        }
        let language = counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(l, _)| l.to_string());

        Some(Segment {
            speaker_label: self.label(tokens[0].speaker),
            origin: self.origin,
            speaker: tokens[0].speaker,
            language,
            start_ms: tokens.first().map(|t| t.start_ms).unwrap_or(0),
            end_ms: tokens.last().map(|t| t.end_ms).unwrap_or(0),
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(text: &str, start: u64, end: u64, speaker: Option<u32>, lang: &str) -> Token {
        Token {
            text: text.into(),
            start_ms: start,
            end_ms: end,
            is_final: true,
            speaker,
            language: Some(lang.into()),
            ..Default::default()
        }
    }

    /// Words said before the other side's line belong above it; words said
    /// after belong below. The timestamps decide, not the order things
    /// happened to arrive in.
    #[test]
    fn an_open_line_splits_at_the_moment_the_other_side_spoke() {
        let mut b = SegmentBuilder::new(Origin::System);
        // One continuous line — no pause long enough to split it on its own.
        b.push(vec![
            tok("before", 1_000, 1_400, None, "en"),
            tok(" and", 1_400, 1_800, None, "en"),
            tok(" after", 2_000, 2_400, None, "en"),
        ]);

        let closed = b.close_before(1_900).expect("the first half should close");
        assert_eq!(closed.text, "before and");
        assert_eq!(closed.start_ms, 1_000);
        assert_eq!(b.open_text().trim(), "after", "the rest stays open");
        assert_eq!(b.open_start_ms(), Some(2_000));

        // Nothing said before that moment: nothing to close.
        assert!(b.close_before(1_900).is_none());
        assert!(b.close_before(9_000).is_some(), "everything before it closes");
        assert!(SegmentBuilder::new(Origin::Mic).close_before(1).is_none());
    }

    #[test]
    fn contiguous_tokens_join_into_one_segment() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        let done = b.push(vec![
            tok("При", 0, 100, Some(1), "uk"),
            tok("віт", 100, 200, Some(1), "uk"),
            tok(" світ", 200, 400, Some(1), "uk"),
        ]);
        assert!(done.is_empty(), "nothing should close while speech continues");
        let s = b.flush().unwrap();
        assert_eq!(s.text, "Привіт світ");
        assert_eq!(s.start_ms, 0);
        assert_eq!(s.end_ms, 400);
        assert_eq!(s.origin, Origin::Mic);
    }

    #[test]
    fn a_long_pause_splits_segments() {
        let mut b = SegmentBuilder::new(Origin::System);
        let done = b.push(vec![
            tok("hello", 0, 200, Some(1), "en"),
            tok("world", 200 + GAP_MS + 1, 1500, Some(1), "en"),
        ]);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].text, "hello");
        assert_eq!(b.flush().unwrap().text, "world");
    }

    #[test]
    fn a_speaker_change_splits_segments() {
        let mut b = SegmentBuilder::new(Origin::System);
        let done = b.push(vec![
            tok("hi", 0, 100, Some(1), "en"),
            tok("there", 110, 200, Some(2), "en"),
        ]);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].speaker, Some(1));
        assert_eq!(b.flush().unwrap().speaker, Some(2));
    }

    #[test]
    fn dominant_language_labels_a_code_switched_segment() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        b.push(vec![
            tok("я", 0, 50, Some(1), "uk"),
            tok(" зроблю", 50, 150, Some(1), "uk"),
            tok(" deployment", 150, 300, Some(1), "en"),
        ]);
        assert_eq!(b.flush().unwrap().language.as_deref(), Some("uk"));
    }

    /// Without this cap a continuous talker produces one endless segment, which
    /// is both unreadable and never written to disk until they stop.
    #[test]
    fn an_unbroken_monologue_is_split_by_length() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        // 30s of speech with no gap large enough to split on.
        let tokens: Vec<Token> = (0..60)
            .map(|i| tok("word ", i * 500, i * 500 + 500, Some(1), "uk"))
            .collect();
        let done = b.push(tokens);
        assert!(!done.is_empty(), "a long monologue must still produce segments");
        assert!(
            done.iter().all(|s| s.end_ms - s.start_ms <= MAX_SEGMENT_MS + 500),
            "no segment may exceed the cap"
        );
    }

    /// The observed symptom: a transcript line containing nothing but ".".
    #[test]
    fn trailing_punctuation_joins_the_previous_sentence() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        let done = b.push(vec![
            tok("Вау, працює", 0, 800, Some(1), "uk"),
            // Full stop lands well after the pause that ended the sentence.
            tok(".", 800 + GAP_MS + 400, 2200, Some(1), "uk"),
        ]);
        assert!(done.is_empty(), "punctuation must not start a new segment");
        assert_eq!(b.flush().unwrap().text, "Вау, працює.");
    }

    #[test]
    fn open_text_exposes_the_unclosed_segment() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        assert_eq!(b.open_text(), "");
        b.push(vec![
            tok("Привіт", 0, 200, Some(1), "uk"),
            tok(" світ", 200, 400, Some(1), "uk"),
        ]);
        // Live display needs this; otherwise the text disappears between
        // finalization and the segment closing.
        assert_eq!(b.open_text(), "Привіт світ");
        b.flush();
        assert_eq!(b.open_text(), "");
    }

    /// The split has to happen when the pause becomes visible, not when the
    /// next phrase happens to finalize.
    #[test]
    fn a_pause_closes_the_segment_before_the_next_phrase_is_final() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        b.push(vec![tok("перша фраза", 0, 1000, Some(1), "uk")]);

        // Same gap, but seen through an unstable token starting the next phrase.
        let closed = b
            .split_before(1000 + GAP_MS + 200)
            .expect("a pause must close the open segment right away");
        assert_eq!(closed.text, "перша фраза");
        assert_eq!(b.open_text(), "", "and leave nothing behind");
    }

    #[test]
    fn a_short_pause_does_not_split() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        b.push(vec![tok("слово", 0, 500, Some(1), "uk")]);
        assert!(b.split_before(500 + GAP_MS - 100).is_none());
        assert_eq!(b.open_text(), "слово");
    }

    #[test]
    fn splitting_an_empty_builder_is_harmless() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        assert!(b.split_before(10_000).is_none());
    }

    /// A file has one track, so "You" is a claim the audio cannot support.
    /// Diarization separates voices; numbering them is all it can honestly say.
    #[test]
    fn a_file_is_attributed_by_voice_not_by_side() {
        let mut b = SegmentBuilder::diarized();
        let done = b.push(vec![
            tok("привіт", 0, 500, Some(1), "uk"),
            tok("hello", 2000, 2500, Some(2), "en"),
        ]);
        assert_eq!(done[0].speaker_label, "Speaker 1");
        assert_eq!(b.flush().unwrap().speaker_label, "Speaker 2");
    }

    /// A live call knows who is who without guessing.
    #[test]
    fn a_live_call_is_attributed_by_side() {
        let mut mic = SegmentBuilder::new(Origin::Mic);
        mic.push(vec![tok("я", 0, 100, Some(1), "uk")]);
        assert_eq!(mic.flush().unwrap().speaker_label, "You");

        let mut system = SegmentBuilder::new(Origin::System);
        system.push(vec![tok("them", 0, 100, Some(7), "en")]);
        assert_eq!(
            system.flush().unwrap().speaker_label,
            "Them",
            "a diarization id must not leak into a call where the side is known"
        );
    }

    #[test]
    fn whitespace_only_segments_are_dropped() {
        let mut b = SegmentBuilder::new(Origin::Mic);
        b.push(vec![tok("   ", 0, 10, Some(1), "uk")]);
        assert!(b.flush().is_none());
    }
}
