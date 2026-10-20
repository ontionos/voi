//! Session model, transcript assembly and on-disk layout.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use voi_audio::Origin;
use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod acl;
pub mod dictation;
pub mod echo;
pub mod hotkey;
pub mod mic_watch;
pub mod prefs;
mod recorder;
mod segment;
pub mod update;
pub use recorder::{
    api_key_path, find_api_key, load_overlay_position, save_api_key, save_overlay_position,
    transcribe_file, spawn as spawn_recorder, Recorder, RecorderConfig, RecorderEvent, Stopper,
};
pub use segment::{Attribution, SegmentBuilder};

/// A contiguous run of speech from one side of the call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// Who this is attributed to, ready to print.
    ///
    /// A live call knows by construction: the microphone is you, the system
    /// output is them. A file knows nothing of the sort — it is one track, and
    /// calling all of it "You" was simply wrong — so there it falls back to
    /// whatever Soniox's diarization separated out.
    pub speaker_label: String,
    pub origin: Origin,
    /// Soniox's speaker id. Advisory only — real-time diarization is unreliable,
    /// so `origin` is the authority on who is talking.
    pub speaker: Option<u32>,
    pub language: Option<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: u64,
    pub segment_count: usize,
    /// How many tokens each detected language contributed, most common first.
    pub languages: Vec<(String, usize)>,
}

/// Where a recording lives on disk.
///
/// One Markdown file per session, holding exactly what the overlay shows:
///
/// ```text
/// # 2026-08-03 21:40
///
/// You: привіт
/// Them: hello
/// ```
///
/// Not a folder of files. The transcript is the artefact; a session that also
/// produced a JSON sidecar and two WAVs made you open four things to read one
/// conversation. Audio is off unless asked for — 16 kHz mono still costs
/// 230 MB an hour for two streams, to keep a recording nobody plays back.
pub struct Session {
    pub id: String,
    pub dir: PathBuf,
    pub path: PathBuf,
    pub started_at: DateTime<Utc>,
    transcript: std::io::BufWriter<std::fs::File>,
    segments: usize,
    languages: std::collections::HashMap<String, usize>,
    last_end_ms: u64,
    /// Previous line's speaker, so a change can be spaced.
    last_speaker: Option<String>,
}

impl Session {
    /// `~/Library/Application Support/voi/sessions` on macOS,
    /// `%APPDATA%\voi\sessions` on Windows.
    pub fn default_root() -> Result<PathBuf> {
        Ok(data_dir()?.join("sessions"))
    }

    pub fn create(root: impl AsRef<Path>) -> Result<Self> {
        use std::io::Write;

        let started_at = Utc::now();
        let id = format!(
            "{}-{}",
            started_at.format("%Y%m%d-%H%M%S"),
            &uuid::Uuid::new_v4().to_string()[..8]
        );
        let dir = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;

        let path = dir.join(format!("{id}.md"));
        let mut file = std::fs::File::create(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        writeln!(file, "# {}\n", Local::now().format("%Y-%m-%d %H:%M"))?;

        Ok(Self {
            id,
            dir,
            path,
            started_at,
            transcript: std::io::BufWriter::new(file),
            segments: 0,
            languages: Default::default(),
            last_end_ms: 0,
            last_speaker: None,
        })
    }

    /// Audio path for a session, used only when audio capture is asked for.
    pub fn mic_wav(&self) -> PathBuf {
        self.dir.join(format!("{}-mic.wav", self.id))
    }

    pub fn system_wav(&self) -> PathBuf {
        self.dir.join(format!("{}-system.wav", self.id))
    }

    /// Append a finished segment. Flushed immediately so a crash cannot lose it.
    pub fn append(&mut self, seg: &Segment) -> Result<()> {
        use std::io::Write;
        // A blank line when the speaker changes, so a conversation reads as a
        // conversation rather than a wall.
        if self.last_speaker.as_deref().is_some_and(|s| s != seg.speaker_label) {
            writeln!(self.transcript)?;
        }
        // Timestamps go in the file but never on screen. Reading back through
        // a long call needs them; watching one being written does not.
        writeln!(
            self.transcript,
            "[{}] {}: {}",
            fmt_hms(seg.start_ms),
            seg.speaker_label,
            seg.text
        )?;
        self.transcript.flush()?;
        self.last_speaker = Some(seg.speaker_label.clone());

        self.segments += 1;
        self.last_end_ms = self.last_end_ms.max(seg.end_ms);
        if let Some(lang) = &seg.language {
            *self.languages.entry(lang.clone()).or_default() += 1;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<SessionMeta> {
        use std::io::Write;
        self.transcript.flush()?;

        let mut languages: Vec<(String, usize)> = self.languages.into_iter().collect();
        languages.sort_by_key(|(_, n)| std::cmp::Reverse(*n));

        Ok(SessionMeta {
            id: self.id,
            started_at: self.started_at,
            ended_at: Some(Utc::now()),
            duration_ms: self.last_end_ms,
            segment_count: self.segments,
            languages,
        })
    }
}

/// Where everything Voi keeps lives: the key, the settings, the sessions.
///
/// `~/Library/Application Support/voi` on macOS, `%APPDATA%\voi` on Windows —
/// per user, outside wherever the app itself is installed, which is what lets
/// the app be handed to someone else without anything of yours inside it.
/// `VOI_DATA_DIR` moves it, so a test can run the real app against a folder
/// of its own instead of the settings of whoever is using the machine.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("VOI_DATA_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::data_dir().context("could not resolve a data directory")?;
    Ok(base.join("voi"))
}

/// The most recent transcript on disk, if there is one.
///
/// Read from the folder rather than remembered in memory: transcripts are
/// files, and files come and go without asking the app. A menu item that
/// decides once at launch whether any exist is wrong the moment you delete or
/// add one.
pub fn latest_transcript() -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(Session::default_root().ok()?)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files.pop()
}

/// Read a stored transcript back as plain text.
pub fn read_transcript(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

/// Format a millisecond offset as `h:mm:ss`.
///
/// Always with hours, because a two-hour call is an ordinary case here and
/// `137:04` is harder to read than `2:17:04`.
pub fn fmt_hms(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// Format a millisecond offset as `mm:ss`.
pub fn fmt_ts(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60_000, (ms / 1000) % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "voi-core-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn seg(origin: Origin, start: u64, text: &str) -> Segment {
        Segment {
            speaker_label: origin.label().to_string(),
            origin,
            speaker: None,
            language: Some("uk".into()),
            start_ms: start,
            end_ms: start + 500,
            text: text.into(),
        }
    }

    #[test]
    fn a_session_is_one_readable_file() {
        let root = tmp();
        let mut s = Session::create(&root).unwrap();
        s.append(&seg(Origin::Mic, 0, "привіт")).unwrap();
        s.append(&seg(Origin::System, 600, "hello")).unwrap();
        let path = s.path.clone();
        let meta = s.finish().unwrap();

        assert_eq!(meta.segment_count, 2);
        assert_eq!(meta.languages, vec![("uk".to_string(), 2)]);
        assert_eq!(path.extension().unwrap(), "md");

        let text = read_transcript(&path).unwrap();
        assert!(text.contains("You: привіт"), "got:
{text}");
        assert!(text.contains("Them: hello"), "got:
{text}");
        // Nothing else beside it — the transcript is the whole artefact, not a
        // folder holding it plus a JSON sidecar plus two audio files.
        let files = std::fs::read_dir(&root).unwrap().count();
        assert_eq!(files, 1, "a session must leave exactly one file");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Session ids are timestamps, so the newest sorts last. If that ever
    /// stopped holding, "open the last transcript" would open the wrong one.
    #[test]
    fn session_ids_sort_oldest_to_newest() {
        let mut ids = [
            "20260803-235959-aaaaaaaa.md",
            "20260101-000000-zzzzzzzz.md",
            "20260803-090000-bbbbbbbb.md",
        ];
        ids.sort();
        assert_eq!(ids.last().unwrap(), &"20260803-235959-aaaaaaaa.md");
        assert_eq!(ids.first().unwrap(), &"20260101-000000-zzzzzzzz.md");
    }

    /// A change of speaker gets a blank line, so a conversation reads as one.
    #[test]
    fn speakers_are_separated_by_a_blank_line() {
        let root = tmp();
        let mut s = Session::create(&root).unwrap();
        s.append(&seg(Origin::Mic, 0, "перше")).unwrap();
        s.append(&seg(Origin::Mic, 600, "друге")).unwrap();
        s.append(&seg(Origin::System, 1200, "theirs")).unwrap();
        let path = s.path.clone();
        s.finish().unwrap();

        let text = read_transcript(&path).unwrap();
        assert!(
            text.contains("[0:00:00] You: перше\n[0:00:00] You: друге\n\n[0:00:01] Them: theirs"),
            "got:\n{text}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A two-hour call is an ordinary case, so the file's timestamps have to
    /// stay readable well past the hour.
    #[test]
    fn file_timestamps_carry_hours() {
        assert_eq!(fmt_hms(0), "0:00:00");
        assert_eq!(fmt_hms(65_000), "0:01:05");
        assert_eq!(fmt_hms(3_600_000), "1:00:00");
        assert_eq!(fmt_hms(8_224_000), "2:17:04");
    }

    #[test]
    fn timestamps_format_past_a_minute() {
        assert_eq!(fmt_ts(0), "00:00");
        assert_eq!(fmt_ts(65_000), "01:05");
        assert_eq!(fmt_ts(3_601_000), "60:01");
    }
}
