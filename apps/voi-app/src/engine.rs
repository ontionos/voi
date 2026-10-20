//! Starting and stopping a recording, and transcribing a file, for whichever
//! front end is asking.
//!
//! Nothing here draws or touches a window. Each front end calls these from
//! its own menu, button or key, and then paints whatever the worker has
//! written into the shared [`State`](crate::state::State) — so a recording
//! behaves the same whether it was started from the Mac's menu bar or from a
//! key pressed on Windows.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use voi_core::{fmt_hms, find_api_key, spawn_recorder, RecorderConfig, RecorderEvent, Session};

use crate::state::{Mode, Shared};

/// The tokio runtime outlives every recording; the UI owns the main thread.
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start tokio runtime")
    })
}

/// Start when idle, stop when recording, and nothing while it winds down —
/// a second press must not start a new recording on top of one still
/// flushing.
pub fn toggle(shared: &Shared) {
    let mode = shared.lock().unwrap().mode;
    match mode {
        Mode::Idle => start(shared),
        Mode::Recording => stop(shared),
        Mode::Stopping => {}
    }
}

pub fn stop(shared: &Shared) {
    let stopper = {
        let mut s = shared.lock().unwrap();
        s.mode = Mode::Stopping;
        s.stopper.clone()
    };
    match stopper {
        Some(s) => s.stop(),
        // Nothing to stop (start failed before the recorder came up).
        None => shared.lock().unwrap().mode = Mode::Idle,
    }
}

pub fn start(shared: &Shared) {
    let shared = shared.clone();

    // Claim the recording state *synchronously*, before spawning anything.
    // Doing it in the worker left the menu showing "Start Recording" for as
    // long as device setup took, so the next click started a second
    // recording on top of the first.
    {
        let mut s = shared.lock().unwrap();
        if s.is_active() {
            return;
        }
        s.mode = Mode::Recording;
        s.started_at = Some(Instant::now());
        s.segments = 0;
        s.session_dir = None;
        s.last_line.clear();
        s.lines.clear();
        s.clear_pending();
        s.level_mic = 0.0;
        s.level_system = 0.0;
        s.error = None;
        // A recording always shows; `overlay_visible` handles that.
    }

    let fail = |shared: &Shared, msg: String| {
        let mut s = shared.lock().unwrap();
        s.mode = Mode::Idle;
        s.started_at = None;
        s.error = Some(msg);
    };

    // Resolve config on this thread so failures surface immediately in the
    // menu rather than disappearing into the worker.
    let key = match find_api_key() {
        Ok(k) => k,
        Err(e) => return fail(&shared, e.to_string()),
    };
    let root = match Session::default_root() {
        Ok(r) => r,
        Err(e) => return fail(&shared, e.to_string()),
    };

    // Replaying a WAV as the microphone makes the live path testable
    // without speakers, room volume, or a permission prompt.
    let mut cfg = RecorderConfig::new(root, key);
    if let Some(path) = std::env::var_os("VOI_REPLAY") {
        cfg.from_file = Some((path.into(), voi_audio::Origin::Mic));
    }

    runtime().spawn(async move {
        let mut recorder = match spawn_recorder(cfg) {
            Ok(r) => r,
            Err(e) => {
                let mut s = shared.lock().unwrap();
                s.mode = Mode::Idle;
                s.started_at = None;
                s.error = Some(format!("{e:#}"));
                return;
            }
        };

        {
            let mut s = shared.lock().unwrap();
            // A stop may have arrived while devices were opening.
            if s.mode == Mode::Stopping {
                recorder.stop();
            }
            s.stopper = Some(recorder.stopper());
        }

        while let Some(event) = recorder.events.recv().await {
            let mut s = shared.lock().unwrap();
            match event {
                RecorderEvent::Started { dir, .. } => s.session_dir = Some(dir),
                RecorderEvent::Segment(seg) => {
                    s.segments += 1;
                    s.last_line = seg.text.clone();
                    s.push_line(seg.origin, seg.text);
                    s.set_pending(seg.origin, String::new(), String::new(), None);
                }
                RecorderEvent::Pending { origin, settled, tail, start_ms } => {
                    s.set_pending(origin, settled, tail, start_ms);
                }
                RecorderEvent::Level { origin, level } => match origin {
                    voi_audio::Origin::Mic => s.level_mic = level,
                    voi_audio::Origin::System => s.level_system = level,
                },
                RecorderEvent::Source { ok: false, origin, detail } => {
                    tracing::warn!(?origin, %detail, "capture source unavailable");
                    s.error = Some(format!("{}: {detail}", origin.label()));
                }
                RecorderEvent::Warning { origin, message } => {
                    tracing::warn!(?origin, %message, "recorder warning");
                    s.error = Some(format!("{}: {message}", origin.label()));
                }
                RecorderEvent::Finished(meta) => {
                    tracing::info!(id = %meta.id, segments = meta.segment_count, "session finished");
                }
                _ => {}
            }
        }

        let mut s = shared.lock().unwrap();
        s.mode = Mode::Idle;
        s.started_at = None;
        s.stopper = None;
        s.level_mic = 0.0;
        s.level_system = 0.0;
        s.clear_pending();
    });
}

/// Transcribe a recording that already exists, reporting progress as the
/// shared state's `job`, and hand the finished transcript to `done`.
///
/// `key` and `root` are resolved by the caller before it asks which file:
/// there is no point choosing a recording that cannot be sent anywhere.
pub fn transcribe(
    shared: &Shared,
    key: String,
    root: PathBuf,
    path: PathBuf,
    done: impl FnOnce(PathBuf) + Send + 'static,
) {
    let shared = shared.clone();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    shared.lock().unwrap().job = Some(format!("Uploading {name}…"));

    runtime().spawn(async move {
        let reporter = shared.clone();
        let progress = move |p: voi_soniox::file::Progress| {
            if let Ok(mut s) = reporter.lock() {
                s.job = Some(progress_text(&p));
            }
        };

        match voi_core::transcribe_file(&path, &key, &root, progress).await {
            Ok(out) => {
                tracing::info!(path = %out.display(), "file transcribed");
                {
                    let mut s = shared.lock().unwrap();
                    s.job = None;
                    s.last_transcript = Some(out.clone());
                }
                done(out);
            }
            Err(e) => {
                tracing::error!(error = %e, "file transcription failed");
                let mut s = shared.lock().unwrap();
                s.job = None;
                s.error = Some(format!("{e:#}"));
            }
        }
    });
}

/// Soniox reports no percentage, and a running clock only answers "how long
/// has this taken", which is not the question. The question is whether it is
/// still alive — the spinner beside this text answers that.
fn progress_text(p: &voi_soniox::file::Progress) -> String {
    use voi_soniox::file::Progress as P;
    let text = match p {
        P::Uploading => "Uploading".to_string(),
        P::Queued => "Queued".to_string(),
        P::Processing { audio_duration_ms: Some(ms) } => {
            format!("Transcribing {} of audio", fmt_hms(*ms))
        }
        P::Processing { .. } => "Transcribing".to_string(),
        P::Done => "Saving".to_string(),
    };
    tracing::info!(%text, "file transcription");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pressing the key while a recording winds down does nothing; it must
    /// not start a second recording on top of the one still flushing.
    #[test]
    fn a_press_while_stopping_is_ignored() {
        let shared: Shared = Default::default();
        shared.lock().unwrap().mode = Mode::Stopping;
        toggle(&shared);
        assert_eq!(shared.lock().unwrap().mode, Mode::Stopping);
    }

    /// Stopping with no recorder behind it (its start failed) goes straight
    /// back to idle instead of waiting for a finish that never comes.
    #[test]
    fn stopping_nothing_returns_to_idle() {
        let shared: Shared = Default::default();
        shared.lock().unwrap().mode = Mode::Recording;
        toggle(&shared);
        assert_eq!(shared.lock().unwrap().mode, Mode::Idle);
    }

    #[test]
    fn progress_reads_as_a_sentence() {
        use voi_soniox::file::Progress as P;
        assert_eq!(progress_text(&P::Processing { audio_duration_ms: Some(65_000) }), "Transcribing 0:01:05 of audio");
        assert_eq!(progress_text(&P::Queued), "Queued");
    }
}
