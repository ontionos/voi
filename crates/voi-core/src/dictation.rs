//! Voice input: speak, watch it appear, let go, and it lands where the cursor is.
//!
//! The same microphone and the same Soniox account as a recording, and nothing
//! else in common. Nothing is written to disk, no session is opened, no
//! speakers are told apart — a dictated sentence is not a call, it is a thing
//! you are about to type. It also runs *alongside* a recording rather than
//! instead of one: two streams of the same microphone, which is allowed, and
//! each has its own widget.

use std::time::Instant;

use tokio::sync::{mpsc, watch};
use voi_audio::{f32_to_s16le, FileSource, MicSource, Resampler, TARGET_RATE};
use voi_soniox::{SonioxConfig, TranscriptEvent};

use crate::recorder::loudness;

/// How long to wait for the last words after the microphone closes.
///
/// Letting go of the key does not mean the server is finished: the tail of a
/// sentence is still in flight, and Soniox flushes it when the audio stream
/// ends. Pasting immediately would cut a word or two off every time.
pub const FLUSH_GRACE: std::time::Duration = std::time::Duration::from_millis(1_800);

/// The longest a press waits for its words after the key is let go, however
/// late Soniox answered.
pub const GIVE_UP: std::time::Duration = std::time::Duration::from_secs(10);

/// When to stop waiting for the last words and type what has arrived.
///
/// [`FLUSH_GRACE`] after letting go, when Soniox was listening by then — which
/// it is within a second of the key going down. When it was not, everything
/// said so far was held for it, and the clock cannot start before it has even
/// heard that: a press whose connect came late gets the grace from the
/// connect, plus the time its held audio takes to catch up. Waiting is
/// better than typing half a sentence, up to [`GIVE_UP`].
pub fn flush_deadline(began: Instant, released: Instant, connected: Option<Instant>) -> Instant {
    let cap = released + GIVE_UP;
    match connected {
        None => cap,
        Some(at) => {
            let caught_up = at + at.saturating_duration_since(began);
            (released.max(caught_up) + FLUSH_GRACE).min(cap)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DictationEvent {
    /// Input loudness, 0.0–1.0, for the meter.
    Level(f32),
    /// Soniox is listening. Everything said since the key went down was held
    /// for it and is on its way now, so the wait for the last words counts
    /// from here, not from letting go.
    Connected,
    /// What has been said so far: `settled` will not change, `tail` may.
    Text { settled: String, tail: String },
    /// The stream has flushed: this is the whole utterance, ready to paste.
    Done(String),
    Warning(String),
    /// The microphone would not open; nothing is coming.
    Failed(String),
}

pub struct Dictation {
    pub events: mpsc::Receiver<DictationEvent>,
    stop: watch::Sender<bool>,
}

impl Dictation {
    /// Start transcribing. Nothing is recorded.
    ///
    /// Returns at once; the microphone opens behind it. Opening AirPods' or
    /// any Bluetooth headset's microphone first switches the headset out of its
    /// music mode, which takes a second or two, and the window and the note
    /// waiting on that made the key feel dead. Soniox connects in the meantime,
    /// and a microphone that will not open says so with
    /// [`DictationEvent::Failed`].
    pub fn start(
        api_key: &str,
        languages: Vec<String>,
        terms: Vec<String>,
        microphone: Option<String>,
    ) -> Self {
        let (events_tx, events) = mpsc::channel::<DictationEvent>(256);
        let (stop, mut stop_rx) = watch::channel(false);

        let mut cfg = SonioxConfig::new(api_key, TARGET_RATE, 1);
        if !languages.is_empty() {
            cfg.language_hints = languages;
        }
        cfg.context_terms = terms;
        // One person, one microphone, one turn. Telling speakers apart costs
        // accuracy and there is nobody here to tell apart.
        cfg.enable_speaker_diarization = false;
        let voi_soniox::SonioxSession { audio, events: mut from_soniox } = voi_soniox::spawn(cfg);

        // Microphone in, Soniox out.
        let levels = events_tx.clone();
        tokio::spawn(async move {
            // `VOI_DICTATE_FROM_FILE` speaks a WAV into it in real time instead
            // of the microphone: the whole path, testable without a room.
            let replay = std::env::var_os("VOI_DICTATE_FROM_FILE");
            let opened = tokio::task::spawn_blocking(move || match replay {
                Some(path) => FileSource::start(std::path::PathBuf::from(path), 100, true),
                None => MicSource::start_named(microphone.as_deref()),
            })
            .await;
            let mut stream = match opened.map_err(anyhow::Error::from).and_then(|r| r) {
                Ok(stream) => stream,
                Err(e) => {
                    let e = e.context("could not open the microphone");
                    tracing::warn!(error = %format!("{e:#}"), "dictation could not start");
                    let _ = levels.send(DictationEvent::Failed(format!("{e:#}"))).await;
                    return;
                }
            };
            let mut resampler = Resampler::new(stream.sample_rate);
            loop {
                tokio::select! {
                    changed = stop_rx.changed() => {
                        if changed.is_err() || *stop_rx.borrow() {
                            break;
                        }
                    }
                    chunk = stream.recv() => {
                        let Some(chunk) = chunk else { break };
                        if chunk.sample_rate != resampler.from_rate() {
                            resampler = Resampler::new(chunk.sample_rate);
                        }
                        let mono = resampler.process(&chunk.to_mono());
                        if mono.is_empty() {
                            continue;
                        }
                        let _ = levels.try_send(DictationEvent::Level(loudness(&mono)));
                        // Still listening for the key while waiting to hand
                        // the audio over. Soniox reads nothing until it has
                        // connected, and a send that waits on a full channel
                        // without it holds the microphone open long after the
                        // key is let go.
                        tokio::select! {
                            sent = audio.send(f32_to_s16le(&mono)) => {
                                if sent.is_err() {
                                    break;
                                }
                            }
                            _ = stop_rx.changed() => break,
                        }
                    }
                }
            }
            // Closing the audio side is what makes Soniox flush the last words.
            drop(audio);
        });

        // Soniox out, text in.
        tokio::spawn(async move {
            let mut settled = String::new();
            let mut tail = String::new();
            while let Some(event) = from_soniox.recv().await {
                // Nobody is waiting for the words any more. Letting go of
                // Soniox's side tells it to stop trying, rather than retrying
                // for a dictation that has already been typed or thrown away.
                if events_tx.is_closed() {
                    break;
                }
                match event {
                    TranscriptEvent::Connected => {
                        let _ = events_tx.send(DictationEvent::Connected).await;
                        continue;
                    }
                    TranscriptEvent::Final(tokens) => {
                        for token in tokens {
                            settled.push_str(&token.text);
                        }
                        tail.clear();
                    }
                    // Unstable text replaces the tail wholesale; appending it
                    // is what produces stuttering, duplicated words.
                    TranscriptEvent::Pending(tokens) => {
                        tail = tokens.iter().map(|t| t.text.as_str()).collect();
                    }
                    TranscriptEvent::Reconnecting { attempt } => {
                        tail.clear();
                        let _ = events_tx
                            .send(DictationEvent::Warning(format!(
                                "reconnecting (attempt {attempt})"
                            )))
                            .await;
                    }
                    TranscriptEvent::Error(message) => {
                        let _ = events_tx.send(DictationEvent::Warning(message)).await;
                    }
                    TranscriptEvent::Finished => break,
                }
                let _ = events_tx
                    .send(DictationEvent::Text { settled: settled.clone(), tail: tail.clone() })
                    .await;
            }
            let _ = events_tx.send(DictationEvent::Done(assembled(&settled, &tail))).await;
        });

        Self { events, stop }
    }

    /// Stop listening. The last words are still on their way — wait for
    /// [`DictationEvent::Done`], or for [`FLUSH_GRACE`] to pass.
    pub fn finish(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        self.finish();
    }
}

/// What actually gets typed.
///
/// Soniox's tokens carry their own leading spaces, so the pieces already join
/// correctly; what is left is the edges, and the double space left behind where
/// a settled phrase meets an unstable one.
pub fn assembled(settled: &str, tail: &str) -> String {
    let joined = format!("{settled}{tail}");
    let mut out = String::with_capacity(joined.len());
    let mut last_was_space = false;
    for ch in joined.chars() {
        let is_space = ch == ' ' || ch == '\n' || ch == '\t';
        if is_space && last_was_space {
            continue;
        }
        out.push(if is_space { ' ' } else { ch });
        last_was_space = is_space;
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_gets_typed_is_tidy() {
        // Tokens arrive with their spaces attached, settled and unstable both.
        assert_eq!(assembled(" Привіт", " світ"), "Привіт світ");
        // The seam between the two is where a double space shows up.
        assert_eq!(assembled("one ", " two"), "one two");
        assert_eq!(assembled("", ""), "");
        assert_eq!(assembled("  spaced  out  ", ""), "spaced out");
        // A newline mid-utterance is still one line of typing.
        assert_eq!(assembled("first\n\nsecond", ""), "first second");
    }

    /// 16:10:05 on 2026-10-14: held 2.8 s, Soniox not there yet, and the
    /// paste went out 1.8 s after letting go with nothing in it.
    #[test]
    fn the_wait_for_the_last_words_starts_when_soniox_is_there() {
        use std::time::Duration;
        let ms = Duration::from_millis;
        let began = Instant::now();

        // The usual press: connected in 0.6 s, held 5 s. Nothing changes.
        let released = began + ms(5_000);
        assert_eq!(flush_deadline(began, released, Some(began + ms(600))), released + FLUSH_GRACE);

        // Let go at 2.8 s, connected at 3.4 s with 3.4 s of speech held for
        // it: the grace starts once that speech has had time to go through.
        let released = began + ms(2_800);
        let late = began + ms(3_400);
        assert_eq!(flush_deadline(began, released, Some(late)), late + ms(3_400) + FLUSH_GRACE);
        assert!(flush_deadline(began, released, Some(late)) > released + FLUSH_GRACE);

        // Not connected yet: keep waiting, but not for ever.
        assert_eq!(flush_deadline(began, released, None), released + GIVE_UP);
        let very_late = began + ms(9_000);
        assert_eq!(flush_deadline(began, released, Some(very_late)), released + GIVE_UP);
    }
}
