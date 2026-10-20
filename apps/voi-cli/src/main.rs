//! `voi` — real-time multilingual call transcription.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::style::Color;
use voi_audio::Origin;
use voi_core::{
    find_api_key, fmt_ts, read_transcript, spawn_recorder, RecorderConfig, RecorderEvent, Session,
};

mod render;
use render::Renderer;

#[derive(Parser)]
#[command(name = "voi", about = "Real-time multilingual call transcription")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Record and transcribe until Ctrl-C.
    Listen {
        /// Replay a WAV instead of capturing live audio.
        #[arg(long, value_name = "PATH")]
        from_file: Option<PathBuf>,
        /// Which side a replayed file represents.
        #[arg(long, default_value = "system")]
        as_origin: OriginArg,
        /// Capture the microphone only, skipping system audio.
        #[arg(long)]
        mic_only: bool,
        /// Also keep the recorded audio next to the transcript.
        #[arg(long)]
        keep_audio: bool,
    },
    /// List recorded sessions.
    Sessions,
    /// Print a stored transcript.
    Show { id: String },
    /// Transcribe an existing recording — a dictaphone file, a voice memo.
    Transcribe { path: PathBuf },
    /// Store the Soniox key, read from standard input so it never sits in the
    /// shell's history or on screen: `voi key < key.txt`.
    Key,
    /// Repair WAV headers left unwritten by an interrupted recording.
    Repair {
        /// Session id; all sessions if omitted.
        id: Option<String>,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum OriginArg {
    Mic,
    System,
}

impl From<OriginArg> for Origin {
    fn from(a: OriginArg) -> Self {
        match a {
            OriginArg::Mic => Origin::Mic,
            OriginArg::System => Origin::System,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,voi=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Command::Listen { from_file, as_origin, mic_only, keep_audio } => {
            listen(from_file, as_origin.into(), mic_only, keep_audio).await
        }
        Command::Sessions => sessions(),
        Command::Show { id } => show(&id),
        Command::Transcribe { path } => transcribe(&path).await,
        Command::Repair { id } => repair(id.as_deref()),
        Command::Key => key(),
    }
}

fn key() -> Result<()> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
    let key = text.trim().strip_prefix("SONIOX_API_KEY=").unwrap_or(text.trim()).trim_matches('"').trim();
    anyhow::ensure!(!key.is_empty(), "no key on standard input");
    voi_core::save_api_key(key)?;
    // The path, never the key.
    println!("saved to {}", voi_core::api_key_path()?.display());
    Ok(())
}

async fn transcribe(path: &std::path::Path) -> Result<()> {
    use voi_soniox::file::Progress;

    let key = find_api_key()?;
    let root = Session::default_root()?;
    let out = voi_core::transcribe_file(path, &key, &root, |p| match p {
        Progress::Uploading => println!("uploading…"),
        Progress::Queued => println!("queued"),
        Progress::Processing { audio_duration_ms: Some(ms) } => {
            println!("transcribing {} of audio…", fmt_ts(ms))
        }
        Progress::Processing { .. } => println!("transcribing…"),
        Progress::Done => {}
    })
    .await?;
    println!("{}", out.display());
    Ok(())
}

/// Fix recordings whose header was never updated — the samples are on disk,
/// only the two length fields are wrong.
fn repair(id: Option<&str>) -> Result<()> {
    let root = Session::default_root()?;
    let wavs: Vec<PathBuf> = std::fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .filter(|p| {
            id.is_none_or(|id| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(id))
            })
        })
        .collect();

    let mut fixed = 0;
    for path in wavs {
        match voi_audio::wav::repair_header(&path) {
            Ok(Some(frames)) => {
                fixed += 1;
                println!(
                    "repaired {} — {} recovered",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    fmt_ts(frames * 1000 / voi_audio::TARGET_RATE as u64)
                );
            }
            Ok(None) => {}
            Err(e) => eprintln!("skipped {}: {e:#}", path.display()),
        }
    }
    if fixed == 0 {
        println!("nothing to repair");
    }
    Ok(())
}

async fn listen(
    from_file: Option<PathBuf>,
    file_origin: Origin,
    mic_only: bool,
    keep_audio: bool,
) -> Result<()> {
    // Fail before opening any device, so we never trigger a permission prompt
    // only to die on a missing key immediately after.
    let key = find_api_key()?;
    let root = Session::default_root()?;

    let mut cfg = RecorderConfig::new(root.clone(), key);
    cfg.system = !mic_only;
    cfg.keep_audio = keep_audio;
    if let Some(path) = from_file {
        cfg.from_file = Some((path, file_origin));
    }

    let mut ui = Renderer::new();
    let mut recorder = spawn_recorder(cfg)?;

    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    let mut stopping = false;
    let mut finished = None;

    loop {
        tokio::select! {
            _ = &mut ctrl_c, if !stopping => {
                stopping = true;
                ui.note("stopping — flushing final transcript…", Color::Magenta);
                recorder.stop();
            }
            event = recorder.events.recv() => {
                let Some(event) = event else { break };
                match event {
                    RecorderEvent::Started { session_id, dir } => {
                        ui.note(&format!("session {session_id}"), Color::Magenta);
                        ui.note(&format!("  {}", dir.display()), Color::DarkGrey);
                    }
                    RecorderEvent::Source { origin, ok: true, detail } => {
                        ui.note(&format!("{}: {detail}", origin.label()), Color::DarkGrey);
                    }
                    RecorderEvent::Source { origin, ok: false, detail } => {
                        ui.note(&format!("{} unavailable: {detail}", origin.label()), Color::Red);
                    }
                    RecorderEvent::Segment(seg) => ui.commit(&seg),
                    RecorderEvent::Pending { origin, settled, tail, .. } => {
                        ui.set_pending(origin, format!("{settled}{tail}"))
                    }
                    RecorderEvent::Warning { origin, message } => {
                        ui.note(&format!("{}: {message}", origin.label()), Color::Yellow);
                    }
                    // The terminal UI has no meter to drive.
                    RecorderEvent::Level { .. } => {}
                    RecorderEvent::Finished(meta) => finished = Some(meta),
                }
            }
        }
    }

    ui.finish();
    if let Some(meta) = finished {
        println!();
        println!(
            "saved {} — {} segments, {}",
            meta.id,
            meta.segment_count,
            fmt_ts(meta.duration_ms)
        );
        if !meta.languages.is_empty() {
            let langs: Vec<String> =
                meta.languages.iter().map(|(l, n)| format!("{l}:{n}")).collect();
            println!("languages: {}", langs.join(" "));
        }
        println!("{}", root.join(format!("{}.md", meta.id)).display());
    }
    Ok(())
}

fn sessions() -> Result<()> {
    let root = Session::default_root()?;
    if !root.exists() {
        println!("no sessions yet");
        return Ok(());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();

    for path in files {
        let lines = std::fs::read_to_string(&path)
            .map(|t| t.lines().filter(|l| l.contains(": ")).count())
            .unwrap_or(0);
        println!(
            "{}  {lines} lines",
            path.file_stem().unwrap_or_default().to_string_lossy()
        );
    }
    Ok(())
}

fn show(id: &str) -> Result<()> {
    let path = Session::default_root()?.join(format!("{id}.md"));
    anyhow::ensure!(path.exists(), "no such session: {id}");
    print!("{}", read_transcript(&path)?);
    Ok(())
}
