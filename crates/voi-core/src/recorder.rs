//! Recording orchestration shared by the CLI and the menu-bar app.
//!
//! Owns the whole live path: open the capture sources, fan each one into both a
//! WAV on disk and a Soniox stream, assemble segments, persist them, and report
//! progress as a stream of [`RecorderEvent`]s. Consumers decide how to present
//! it — the CLI paints a terminal, the app updates a menu.

use std::path::PathBuf;

use anyhow::{Context, Result};
use voi_audio::{
    f32_to_s16le, wav::WavSink, AudioStream, FileSource, MicSource, Origin, Resampler,
    SystemSource, TARGET_RATE,
};
use voi_soniox::{SonioxConfig, TranscriptEvent};
use tokio::sync::{mpsc, watch};

use crate::{SegmentBuilder, Session, SessionMeta, Segment};

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub root: PathBuf,
    pub api_key: String,
    /// Capture the microphone.
    pub mic: bool,
    /// Capture system output audio.
    pub system: bool,
    /// Replay a WAV instead of opening devices, tagged as this origin.
    pub from_file: Option<(PathBuf, Origin)>,
    /// Also keep the audio. Off by default: two 16 kHz streams still cost
    /// ~230 MB an hour, and the transcript is what anyone actually reads.
    pub keep_audio: bool,
    /// Languages that may appear, and the words worth expecting. Read from the
    /// saved settings when the config is built, so both streams and every
    /// caller get the same answer.
    pub languages: Vec<String>,
    pub terms: Vec<String>,
    /// Which microphone, by name; `None` is the system's default.
    pub microphone: Option<String>,
}

impl RecorderConfig {
    pub fn new(root: PathBuf, api_key: String) -> Self {
        let prefs = crate::prefs::load();
        Self {
            root,
            api_key,
            mic: true,
            system: true,
            from_file: None,
            keep_audio: false,
            languages: prefs.languages,
            terms: prefs.terms,
            microphone: prefs.microphone,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RecorderEvent {
    Started { session_id: String, dir: PathBuf },
    /// A capture source came up, or failed to.
    Source { origin: Origin, ok: bool, detail: String },
    /// Everything spoken but not yet committed as a finished segment.
    ///
    /// Split in two because they mean different things on screen. `settled`
    /// holds tokens Soniox has finalized — they will not change, so they should
    /// look no different from a closed line, even though the segment is still
    /// open. `tail` is the unstable part that may still be rewritten.
    ///
    /// Sending only the unstable tail made text vanish the moment it was
    /// finalized; sending them merged left finished words looking provisional
    /// until a pause happened to close the segment.
    Pending {
        origin: Origin,
        settled: String,
        tail: String,
        /// When this line began, on the audio's own clock. Consumers showing
        /// both sides at once order the two live lines by it — arrival order
        /// says nothing about who spoke first.
        start_ms: Option<u64>,
    },
    /// Input loudness, 0.0–1.0, roughly once per 100ms per stream. Drives the
    /// level meter; consumers that do not draw one should ignore it.
    Level { origin: Origin, level: f32 },
    /// A finalized segment, already persisted.
    Segment(Segment),
    Warning { origin: Origin, message: String },
    Finished(SessionMeta),
}

pub struct Recorder {
    pub events: mpsc::Receiver<RecorderEvent>,
    stop: Stopper,
}

/// Stops a recording from anywhere, including a thread that does not own the
/// [`Recorder`] — the menu-bar app holds one on the UI thread while the event
/// loop runs on a worker.
#[derive(Clone)]
pub struct Stopper(std::sync::Arc<watch::Sender<bool>>);

impl Stopper {
    pub fn stop(&self) {
        let _ = self.0.send(true);
    }
}

impl Recorder {
    /// Ask the recording to wind down. Remaining final tokens are still flushed,
    /// so expect more events (ending in [`RecorderEvent::Finished`]) after this.
    pub fn stop(&self) {
        self.stop.stop();
    }

    pub fn stopper(&self) -> Stopper {
        self.stop.clone()
    }
}

/// One event, tagged with the stream it came from.
struct Tagged(Origin, TranscriptEvent);

/// Start recording. Fails before touching any device if the session directory
/// cannot be created, so a permission prompt is never raised for a doomed run.
pub fn spawn(cfg: RecorderConfig) -> Result<Recorder> {
    let mut session = Session::create(&cfg.root)?;
    let (event_tx, event_rx) = mpsc::channel::<RecorderEvent>(512);
    let (stop_tx, stop_rx) = watch::channel(false);

    let started = RecorderEvent::Started {
        session_id: session.id.clone(),
        dir: session.dir.clone(),
    };
    event_tx
        .try_send(started)
        .context("event channel closed before start")?;

    let (raw_tx, mut raw_rx) = mpsc::channel::<Tagged>(512);
    let mut streams = 0usize;

    // What the system stream is playing, for the microphone's gate to hear
    // it against — see `crate::echo`.
    let playing = crate::echo::Playing::new();
    let open = |stream: AudioStream, origin: Origin, wav: Option<PathBuf>| -> Result<()> {
        let _ = event_tx.try_send(RecorderEvent::Source {
            origin,
            ok: true,
            detail: format!("{} Hz, {} ch", stream.sample_rate, stream.channels),
        });
        spawn_pipeline(
            stream,
            origin,
            &cfg.api_key,
            cfg.languages.clone(),
            cfg.terms.clone(),
            wav,
            raw_tx.clone(),
            event_tx.clone(),
            stop_rx.clone(),
            playing.clone(),
        )
    };

    if let Some((path, origin)) = &cfg.from_file {
        let wav = cfg.keep_audio.then(|| match origin {
            Origin::Mic => session.mic_wav(),
            Origin::System => session.system_wav(),
        });
        let stream = FileSource::start(path, 100, true)
            .with_context(|| format!("failed to open {}", path.display()))?;
        open(stream, *origin, wav)?;
        streams += 1;
    } else {
        if cfg.mic {
            match MicSource::start_named(cfg.microphone.as_deref()) {
                Ok(s) => {
                    open(s, Origin::Mic, cfg.keep_audio.then(|| session.mic_wav()))?;
                    streams += 1;
                }
                Err(e) => {
                    let _ = event_tx.try_send(RecorderEvent::Source {
                        origin: Origin::Mic,
                        ok: false,
                        detail: format!("{e:#}"),
                    });
                }
            }
        }
        if cfg.system {
            match SystemSource::start() {
                Ok(s) => {
                    open(s, Origin::System, cfg.keep_audio.then(|| session.system_wav()))?;
                    streams += 1;
                }
                Err(e) => {
                    let _ = event_tx.try_send(RecorderEvent::Source {
                        origin: Origin::System,
                        ok: false,
                        detail: format!("{e:#}"),
                    });
                }
            }
        }
    }

    drop(raw_tx);
    anyhow::ensure!(streams > 0, "no audio sources available");

    tokio::spawn(async move {
        let mut builders = [
            (Origin::Mic, SegmentBuilder::new(Origin::Mic)),
            (Origin::System, SegmentBuilder::new(Origin::System)),
        ];
        // Soniox's unstable tail, per stream, kept so the live text can be
        // rebuilt whenever either half of it changes.
        let mut tails = [(Origin::Mic, String::new()), (Origin::System, String::new())];
        // When each side last had anything to say, by the clock rather than by
        // the timeline in the audio — see the silence sweep below.
        let mut heard = [
            (Origin::Mic, std::time::Instant::now()),
            (Origin::System, std::time::Instant::now()),
        ];
        let mut sweep = tokio::time::interval(std::time::Duration::from_millis(250));
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            let Tagged(origin, ev) = tokio::select! {
                event = raw_rx.recv() => match event {
                    Some(event) => event,
                    None => break,
                },
                _ = sweep.tick() => {
                    // Close a line that has simply stopped.
                    //
                    // A segment closes when the *next* token shows it began
                    // after a pause — so the last thing anybody says stays open
                    // until they say something else. Which is to say: you
                    // finish a sentence, stop talking, and it hangs there as
                    // live text at the bottom of the panel while the other
                    // side's line goes on being typed above it. It could hang
                    // for the rest of the call.
                    //
                    // Measured on the wall clock, not on the audio timeline:
                    // the question is how long it has been since anything
                    // arrived, and nothing arrives at all during a silence.
                    let quiet = builders
                        .iter()
                        .position(|(origin, builder)| {
                            builder.open_start_ms().is_some()
                                && heard
                                    .iter()
                                    .find(|(o, _)| o == origin)
                                    .is_some_and(|(_, at)| at.elapsed() >= SETTLE_AFTER)
                        });
                    let Some(quiet) = quiet else { continue };

                    let Some(seg) = builders[quiet].1.flush() else { continue };
                    let side = builders[quiet].0;
                    // And cut the other side at the moment this line began, so
                    // the two still read in the order they were said. Same rule
                    // as when words land — see below.
                    let interrupted = builders
                        .iter_mut()
                        .find(|(o, _)| *o != side)
                        .and_then(|(_, b)| b.close_before(seg.start_ms));

                    for seg in interleaved(vec![seg], interrupted.clone()) {
                        if let Err(e) = session.append(&seg) {
                            tracing::error!(error = %e, "failed to persist segment");
                        }
                        if event_tx.send(RecorderEvent::Segment(seg)).await.is_err() {
                            return;
                        }
                    }
                    let _ = event_tx
                        .send(RecorderEvent::Pending {
                            origin: side,
                            settled: String::new(),
                            tail: String::new(),
                            start_ms: None,
                        })
                        .await;
                    if let Some(seg) = &interrupted {
                        let (open, start_ms) = builders
                            .iter()
                            .find(|(o, _)| *o == seg.origin)
                            .map(|(_, b)| (b.open_text(), b.open_start_ms()))
                            .unwrap_or_default();
                        let tail = tails
                            .iter()
                            .find(|(o, _)| *o == seg.origin)
                            .map(|(_, t)| t.clone())
                            .unwrap_or_default();
                        let _ = event_tx
                            .send(RecorderEvent::Pending {
                                origin: seg.origin,
                                settled: open,
                                tail,
                                start_ms,
                            })
                            .await;
                    }
                    continue;
                }
            };
            // Only when something was actually said. Soniox sends a frame per
            // chunk of audio whether or not there are words in it, so counting
            // every message as "heard" means the sweep below never fires and a
            // finished phrase hangs live for the rest of the call.
            let spoke = match &ev {
                TranscriptEvent::Final(tokens) | TranscriptEvent::Pending(tokens) => {
                    tokens.iter().any(|t| !t.text.trim().is_empty())
                }
                _ => false,
            };
            if spoke {
                if let Some(entry) = heard.iter_mut().find(|(o, _)| *o == origin) {
                    entry.1 = std::time::Instant::now();
                }
            }
            let builder = &mut builders.iter_mut().find(|(o, _)| *o == origin).unwrap().1;
            match ev {
                TranscriptEvent::Connected => {}
                TranscriptEvent::Final(tokens) => {
                    // Cut the other side at the moment these words begin.
                    //
                    // Every batch, not only the ones that close a line. The two
                    // streams are transcribed independently and each holds its
                    // line open until its own speaker pauses, so a steady
                    // talker's line stays open across everything the other
                    // person says — and their later words keep being added to a
                    // line that sits above it. On screen that is text appearing
                    // in the middle of the page; on disk it is a transcript in
                    // the wrong order.
                    //
                    // The timestamps say where every word belongs, so that is
                    // what decides. A line breaks wherever the other side
                    // spoke, which is exactly where a reader would break it.
                    let interrupted = tokens
                        .first()
                        .map(|t| t.start_ms)
                        .and_then(|at| {
                            builders
                                .iter_mut()
                                .find(|(o, _)| *o != origin)
                                .and_then(|(_, b)| b.close_before(at))
                        });
                    let builder =
                        &mut builders.iter_mut().find(|(o, _)| *o == origin).unwrap().1;
                    let closed = builder.push(tokens);

                    // Written before either side's live text is updated.
                    //
                    // A line that closes moves from the live text to the record
                    // above it, and if the live text is cleared first there is
                    // a frame in which the line is in neither place. On screen
                    // that is a whole line blinking out and back, over and over,
                    // for as long as the other person keeps talking.
                    for seg in interleaved(closed, interrupted.clone()) {
                        if let Err(e) = session.append(&seg) {
                            tracing::error!(error = %e, "failed to persist segment");
                        }
                        if event_tx.send(RecorderEvent::Segment(seg)).await.is_err() {
                            return;
                        }
                    }
                    if let Some(seg) = &interrupted {
                        let (open, start_ms) = builders
                            .iter()
                            .find(|(o, _)| *o == seg.origin)
                            .map(|(_, b)| (b.open_text(), b.open_start_ms()))
                            .unwrap_or_default();
                        let tail = tails
                            .iter()
                            .find(|(o, _)| *o == seg.origin)
                            .map(|(_, t)| t.clone())
                            .unwrap_or_default();
                        let _ = event_tx
                            .send(RecorderEvent::Pending {
                                origin: seg.origin,
                                settled: open,
                                tail,
                                start_ms,
                            })
                            .await;
                    }
                    let builder =
                        &mut builders.iter_mut().find(|(o, _)| *o == origin).unwrap().1;
                    // Finalization empties Soniox's tail; without re-sending the
                    // open segment here the display would blank out.
                    let tail = tails.iter().find(|(o, _)| *o == origin).unwrap().1.clone();
                    let _ = event_tx
                        .send(RecorderEvent::Pending {
                            origin,
                            settled: builder.open_text(),
                            tail,
                            start_ms: builder.open_start_ms(),
                        })
                        .await;
                }
                TranscriptEvent::Pending(tokens) => {
                    // A new phrase is starting: close the previous one now, so
                    // the line it will occupy is already the line it occupies.
                    if let Some(first) = tokens.first() {
                        if let Some(seg) = builder.split_before(first.start_ms) {
                            if let Err(e) = session.append(&seg) {
                                tracing::error!(error = %e, "failed to persist segment");
                            }
                            if event_tx.send(RecorderEvent::Segment(seg)).await.is_err() {
                                return;
                            }
                        }
                    }
                    let tail: String = tokens.iter().map(|t| t.text.as_str()).collect();
                    tails.iter_mut().find(|(o, _)| *o == origin).unwrap().1 = tail.clone();
                    let _ = event_tx
                        .send(RecorderEvent::Pending {
                            origin,
                            settled: builder.open_text(),
                            tail,
                            start_ms: builder.open_start_ms(),
                        })
                        .await;
                }
                TranscriptEvent::Reconnecting { attempt } => {
                    let _ = event_tx
                        .send(RecorderEvent::Pending {
                            origin,
                            settled: String::new(),
                            tail: String::new(),
                            start_ms: None,
                        })
                        .await;
                    let _ = event_tx
                        .send(RecorderEvent::Warning {
                            origin,
                            message: format!("reconnecting (attempt {attempt})"),
                        })
                        .await;
                }
                TranscriptEvent::Finished => {
                    let _ = event_tx
                        .send(RecorderEvent::Pending {
                            origin,
                            settled: String::new(),
                            tail: String::new(),
                            start_ms: None,
                        })
                        .await;
                }
                TranscriptEvent::Error(e) => {
                    let _ = event_tx
                        .send(RecorderEvent::Warning { origin, message: e })
                        .await;
                }
            }
        }

        // Streams are done; flush whatever each builder still holds.
        for (_, builder) in builders.iter_mut() {
            if let Some(seg) = builder.flush() {
                if let Err(e) = session.append(&seg) {
                    tracing::error!(error = %e, "failed to persist trailing segment");
                }
                let _ = event_tx.send(RecorderEvent::Segment(seg)).await;
            }
        }

        match session.finish() {
            Ok(meta) => {
                let _ = event_tx.send(RecorderEvent::Finished(meta)).await;
            }
            Err(e) => tracing::error!(error = %e, "failed to finalize session"),
        }
    });

    Ok(Recorder { events: event_rx, stop: Stopper(std::sync::Arc::new(stop_tx)) })
}

/// Wire one audio stream to both disk and Soniox.
///
/// Disk is fed unconditionally; the network path uses a bounded channel and
/// drops under backpressure. A network problem must never cost the recording.
#[allow(clippy::too_many_arguments)]
fn spawn_pipeline(
    stream: AudioStream,
    origin: Origin,
    api_key: &str,
    languages: Vec<String>,
    terms: Vec<String>,
    wav_path: Option<PathBuf>,
    events: mpsc::Sender<Tagged>,
    notices: mpsc::Sender<RecorderEvent>,
    mut shutdown: watch::Receiver<bool>,
    playing: std::sync::Arc<crate::echo::Playing>,
) -> Result<()> {
    let capture_rate = stream.sample_rate;
    // Everything downstream works at 16 kHz: speech recognition gains nothing
    // above it, and it cuts disk and bandwidth to a third.
    let mut resampler = Resampler::new(capture_rate);

    let mut cfg = SonioxConfig::new(api_key, TARGET_RATE, 1);
    if !languages.is_empty() {
        cfg.language_hints = languages;
    }
    cfg.context_terms = terms;
    // Only the system stream carries multiple unknown speakers. The microphone
    // is one known person, so diarization there costs accuracy for nothing.
    cfg.enable_speaker_diarization = origin == Origin::System;
    let voi_soniox::SonioxSession { audio: audio_tx, events: mut soniox_events } =
        voi_soniox::spawn(cfg);

    // Forward transcript events on their own task.
    //
    // These used to be drained after the capture loop exited, which meant every
    // token sat in the channel until the recording stopped and the whole
    // transcript appeared at once. Capture runs until you press stop, so
    // "after the loop" is "never" for a live session.
    tokio::spawn(async move {
        while let Some(ev) = soniox_events.recv().await {
            if events.send(Tagged(origin, ev)).await.is_err() {
                break;
            }
        }
    });

    let mut wav = match &wav_path {
        Some(path) => Some(
            WavSink::create(path, TARGET_RATE, 1)
                .with_context(|| format!("failed to open {}", path.display()))?,
        ),
        None => None,
    };

    tokio::spawn(async move {
        // Bind the whole stream so its device guard is captured along with the
        // receiver — see the note on AudioStream.
        let mut stream = stream;
        let mut silent_samples: u64 = 0;
        let mut warned_silent = false;
        let mut warned_rate = false;
        let mut dropped: u64 = 0;
        // One side says what is playing, the other listens against it.
        let mut source = (origin == Origin::System).then(|| crate::echo::Source::new(playing.clone()));
        let mut gate = (origin == Origin::Mic).then(|| crate::echo::Gate::new(playing.clone()));
        let mut reported = std::time::Instant::now();

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { break; }
                }
                chunk = stream.recv() => {
                    let Some(chunk) = chunk else { break };

                    if chunk.sample_rate != resampler.from_rate() {
                        if !warned_rate {
                            warned_rate = true;
                            tracing::warn!(
                                was = resampler.from_rate(), now = chunk.sample_rate,
                                "capture sample rate changed mid-session; following it"
                            );
                        }
                        // Follow the device instead of silently pitch-shifting
                        // everything that comes after a device switch.
                        resampler = Resampler::new(chunk.sample_rate);
                    }

                    let mono = resampler.process(&chunk.to_mono());
                    if mono.is_empty() {
                        continue;
                    }
                    let _ = notices.try_send(RecorderEvent::Level {
                        origin,
                        level: loudness(&mono),
                    });
                    if let Some(wav) = wav.as_mut() {
                        if let Err(e) = wav.write(&mono) {
                            tracing::error!(error = %e, "failed writing audio to disk");
                        }
                    }

                    // A tap with no permission yields perfect digital silence.
                    // Say so, rather than producing a mysteriously empty transcript.
                    if origin == Origin::System && !warned_silent {
                        if mono.iter().all(|s| *s == 0.0) {
                            silent_samples += mono.len() as u64;
                            if silent_samples > TARGET_RATE as u64 * 5 {
                                warned_silent = true;
                                // Windows asks no permission for loopback, so
                                // there silence only ever means nothing plays.
                                let message = if cfg!(target_os = "macos") {
                                    "no system audio — grant System Audio \
                                    Recording in Privacy & Security, or check audio is playing"
                                } else {
                                    "no system audio yet — nothing is playing"
                                };
                                tracing::warn!(message);
                                // Surface it, not just log it: silence is
                                // indistinguishable from a working capture until
                                // the transcript comes back empty.
                                let _ = notices
                                    .try_send(RecorderEvent::Warning {
                                        origin,
                                        message: message.to_string(),
                                    });
                            }
                        } else {
                            warned_silent = true;
                        }
                    }

                    let now = std::time::Instant::now();
                    if let Some(source) = source.as_mut() {
                        source.heard(&mono, now);
                    }
                    let bytes = match gate.as_mut() {
                        Some(gate) => f32_to_s16le(&gate.filter(&mono, now)),
                        None => f32_to_s16le(&mono),
                    };
                    if let Some(gate) = gate.as_mut() {
                        if reported.elapsed() >= std::time::Duration::from_secs(10) {
                            reported = now;
                            if let Some(r) = gate.report() {
                                tracing::info!(
                                    silenced = %format!("{:.0}%", r.silenced * 100.0),
                                    delay_ms = r.delay_ms,
                                    loss_db = %format!("{:.0}", r.loss_db),
                                    echoing = r.echoing,
                                    matched = %format!("{:.2}", r.matched),
                                    "microphone held back while something played"
                                );
                            }
                        }
                    }
                    if bytes.is_empty() {
                        continue;
                    }

                    // Drop rather than block: the capture side must keep
                    // moving. Dropping is not free, though — those are words
                    // that will not be transcribed, and a stream that stays
                    // full is a stream sending nothing, which the server ends
                    // as a timeout. So say when it happens.
                    if audio_tx.try_send(bytes).is_err() {
                        dropped += 1;
                        if dropped % 50 == 1 {
                            tracing::warn!(
                                ?origin,
                                dropped,
                                "audio dropped on the way to Soniox; the network side is behind"
                            );
                        }
                    }
                }
            }
        }

        // The gate holds the last few milliseconds back to judge them; they
        // belong to the recording too.
        if let Some(rest) = gate.as_mut().map(|gate| gate.flush()).filter(|rest| !rest.is_empty()) {
            let _ = audio_tx.try_send(f32_to_s16le(&rest));
        }
        // Closing the audio side makes Soniox flush its remaining final tokens;
        // the forwarding task above delivers them and then ends.
        drop(audio_tx);
        if let Some(mut wav) = wav {
            if let Err(e) = wav.flush() {
                tracing::error!(error = %e, "failed flushing wav");
            }
            if let Err(e) = wav.finalize() {
                tracing::error!(error = %e, "failed finalizing wav");
            }
        }
    });

    Ok(())
}

/// Perceptual loudness of a block, mapped to 0.0–1.0.
///
/// Linear RMS is useless for a meter — speech sits near 0.02 and the bar barely
/// moves. Mapping the last 60 dB onto the range gives the dynamics an ear
/// expects.
pub(crate) fn loudness(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_sq / samples.len() as f32).sqrt();
    let db = 20.0 * (rms + 1e-9).log10();
    // -55 dB to -5 dB spans the useful range of a laptop microphone; the
    // exponent lifts the quiet end further so ordinary speech uses most of the
    // meter rather than hovering near the floor.
    ((db + 55.0) / 50.0).clamp(0.0, 1.0).powf(0.75)
}

/// How long a side has to be quiet before its open line is closed and written.
///
/// Longer than the pause that separates segments within a stream: tokens arrive
/// a moment after the sound that made them, and cutting on the first quiet
/// quarter-second would break sentences in half while their tail is still on
/// its way from the server.
const SETTLE_AFTER: std::time::Duration = std::time::Duration::from_millis(1_800);

/// The segments one side just closed, together with the line the other side
/// was interrupted in the middle of, in the order they were begun.
fn interleaved(closed: Vec<Segment>, interrupted: Option<Segment>) -> Vec<Segment> {
    let mut all = closed;
    if let Some(seg) = interrupted {
        all.push(seg);
    }
    all.sort_by_key(|s| s.start_ms);
    all
}

/// Where the key is kept for an installed app.
pub fn api_key_path() -> Result<PathBuf> {
    Ok(crate::data_dir()?.join("config.env"))
}

/// Store the Soniox key, readable only by this user.
///
/// Kept outside the bundle so it survives a reinstall, and so the app can be
/// handed to someone else without carrying your key inside it.
pub fn save_api_key(key: &str) -> Result<()> {
    let path = api_key_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("SONIOX_API_KEY={}\n", key.trim()))
        .with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    crate::acl::owner_only(&path)?;
    Ok(())
}

/// Where the overlay was last left, so it comes back there.
fn overlay_position_path() -> Result<PathBuf> {
    Ok(crate::data_dir()?.join("overlay-position"))
}

pub fn save_overlay_position(x: f64, y: f64) -> Result<()> {
    let path = overlay_position_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{x} {y}"))?;
    Ok(())
}

pub fn load_overlay_position() -> Option<(f64, f64)> {
    let text = std::fs::read_to_string(overlay_position_path().ok()?).ok()?;
    let mut parts = text.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Locate the Soniox key: environment first, then the installed app's config,
/// then a `.env` in the working directory for development.
pub fn find_api_key() -> Result<String> {
    if let Ok(k) = std::env::var("SONIOX_API_KEY") {
        if !k.trim().is_empty() {
            return Ok(k.trim().to_string());
        }
    }

    let mut candidates = Vec::new();
    if let Ok(path) = api_key_path() {
        candidates.push(path);
    }
    candidates.push(PathBuf::from(".env"));

    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            let line = line.trim_start_matches('\u{feff}');
            if let Some(v) = line.trim().strip_prefix("SONIOX_API_KEY=") {
                let v = v.trim().trim_matches('"');
                if !v.is_empty() {
                    return Ok(v.to_string());
                }
            }
        }
    }

    anyhow::bail!(
        "no Soniox API key — paste it into Settings, set SONIOX_API_KEY, or write it to {}",
        api_key_path().map(|p| p.display().to_string()).unwrap_or_else(|_| "config.env".into())
    )
}

/// Transcribe a recorded file into a session, the same shape as a live one.
///
/// Uses the async model rather than the real-time one: the audio already
/// exists, so there is nothing to gain from streaming it, and reading the whole
/// recording at once is more accurate on long material.
///
/// Everything lands under [`Origin::Mic`]. A file has one track and no notion
/// of who is on which end, so pretending otherwise would be a guess.
pub async fn transcribe_file(
    path: &std::path::Path,
    api_key: &str,
    root: &std::path::Path,
    on_progress: impl FnMut(voi_soniox::file::Progress),
) -> Result<PathBuf> {
    let prefs = crate::prefs::load();
    let languages = if prefs.languages.is_empty() {
        crate::prefs::default_languages()
    } else {
        prefs.languages
    };
    let tokens =
        voi_soniox::file::transcribe(path, api_key, &languages, &prefs.terms, on_progress).await?;

    let mut session = Session::create(root)?;
    // A file is one track: there is no "you" and "them" to be had.
    let mut builder = SegmentBuilder::diarized();
    for seg in builder.push(tokens) {
        session.append(&seg)?;
    }
    if let Some(seg) = builder.flush() {
        session.append(&seg)?;
    }
    let path = session.path.clone();
    session.finish()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{interleaved, loudness};
    use crate::{Segment, SegmentBuilder};
    use voi_audio::Origin;
    use voi_soniox::Token;

    fn token(text: &str, start: u64, end: u64) -> Token {
        Token {
            text: text.into(),
            start_ms: start,
            end_ms: end,
            is_final: true,
            ..Default::default()
        }
    }

    fn line(text: &str, start: u64) -> Segment {
        Segment {
            speaker_label: "You".into(),
            origin: Origin::Mic,
            speaker: None,
            language: None,
            start_ms: start,
            end_ms: start + 500,
            text: text.into(),
        }
    }

    /// A transcript has to read in the order things were said. One side
    /// closing a line closes the other side's too, and the two are written
    /// oldest first — otherwise a steady talker's line stays open while whole
    /// phrases from the other side land underneath it, and it goes on being
    /// typed above them.
    #[test]
    fn an_interrupted_line_is_written_in_the_order_it_was_begun() {
        // Them, still talking, started first.
        let mut them = SegmentBuilder::new(Origin::System);
        them.push(vec![token("so as I was saying", 1_000, 3_000)]);
        assert_eq!(them.open_start_ms(), Some(1_000));

        // You, interrupting, finish a phrase.
        let mine = line("wait, hold on", 2_400);
        let order = interleaved(vec![mine], them.flush());
        let texts: Vec<&str> = order.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["so as I was saying", "wait, hold on"]);
        assert_eq!(them.open_start_ms(), None, "their line must have been closed");

        // And with nothing open on the other side, nothing is invented.
        assert_eq!(interleaved(vec![line("alone", 0)], None).len(), 1);
        assert!(interleaved(Vec::new(), None).is_empty());
    }

    #[test]
    fn silence_reads_zero_and_full_scale_reads_one() {
        assert_eq!(loudness(&[0.0; 128]), 0.0);
        assert_eq!(loudness(&[]), 0.0);
        assert!(loudness(&[1.0; 128]) >= 0.99);
    }

    #[test]
    fn quiet_speech_still_moves_the_meter() {
        // Linear RMS would put this at 0.02 and the bar would look dead.
        let quiet: Vec<f32> = (0..1000)
            .map(|i| 0.02 * (i as f32 * 0.3).sin())
            .collect();
        let level = loudness(&quiet);
        assert!(level > 0.35, "quiet speech mapped to {level}, too low to see");
        assert!(level < 0.9, "quiet speech mapped to {level}, too high");
    }

    #[test]
    fn louder_input_reads_higher() {
        let soft: Vec<f32> = (0..500).map(|i| 0.05 * (i as f32).sin()).collect();
        let loud: Vec<f32> = (0..500).map(|i| 0.5 * (i as f32).sin()).collect();
        assert!(loudness(&loud) > loudness(&soft));
    }
}
