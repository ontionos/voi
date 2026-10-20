//! Transcribing a recorded file, rather than a live stream.
//!
//! A different model and a different protocol from the live path: uploading a
//! file, asking for a transcription, waiting, and collecting the result. There
//! is no reason to pay real-time latency for audio that already exists, and the
//! async model reads the whole recording at once, so it is more accurate on
//! long material.
//!
//! Handles up to five hours in a single request, which is the point — a
//! dictaphone recording is not a phone call.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::Token;

const API: &str = "https://api.soniox.com/v1";
/// The async model. Unrelated to the real-time one, and better on long audio.
pub const DEFAULT_MODEL: &str = "stt-async-v4";

/// Progress, for something to show while a long file is processed.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Uploading,
    Queued,
    Processing { audio_duration_ms: Option<u64> },
    Done,
}

#[derive(Deserialize)]
struct Uploaded {
    id: String,
}

#[derive(Deserialize)]
struct Job {
    id: String,
    status: String,
    #[serde(default)]
    audio_duration_ms: Option<u64>,
    #[serde(default)]
    error_message: Option<String>,
}

#[derive(Deserialize)]
struct Transcript {
    #[serde(default)]
    tokens: Vec<Token>,
    #[serde(default)]
    text: Option<String>,
}

/// Upload, transcribe, and wait. `on_progress` is called as the state changes.
pub async fn transcribe(
    path: &Path,
    api_key: &str,
    language_hints: &[String],
    context_terms: &[String],
    mut on_progress: impl FnMut(Progress),
) -> Result<Vec<Token>> {
    let client = reqwest::Client::builder()
        // A five-hour upload is not quick, and the default timeout is not
        // written with that in mind.
        .timeout(Duration::from_secs(60 * 60))
        .build()?;

    on_progress(Progress::Uploading);
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio".to_string());

    let form = reqwest::multipart::Form::new()
        .part("file", reqwest::multipart::Part::bytes(bytes).file_name(name));
    let uploaded: Uploaded = send(
        client
            .post(format!("{API}/files"))
            .bearer_auth(api_key)
            .multipart(form),
    )
    .await
    .context("upload failed")?;

    let mut request = serde_json::json!({
        "file_id": uploaded.id,
        "model": DEFAULT_MODEL,
        "language_hints": language_hints,
        "enable_language_identification": true,
        "enable_speaker_diarization": true,
    });
    // The same dictionary the live path uses — a name worth getting right in a
    // call is worth getting right in a recording.
    if !context_terms.is_empty() {
        request["context"] = serde_json::json!({ "terms": context_terms });
    }
    let job: Job = send(
        client
            .post(format!("{API}/transcriptions"))
            .bearer_auth(api_key)
            .json(&request),
    )
    .await
    .context("could not start transcription")?;

    on_progress(Progress::Queued);
    let id = job.id;
    loop {
        let job: Job = send(
            client
                .get(format!("{API}/transcriptions/{id}"))
                .bearer_auth(api_key),
        )
        .await
        .context("could not read transcription status")?;

        match job.status.as_str() {
            "completed" => break,
            "error" => bail!(
                "transcription failed: {}",
                job.error_message.unwrap_or_else(|| "unknown".into())
            ),
            _ => on_progress(Progress::Processing {
                audio_duration_ms: job.audio_duration_ms,
            }),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let transcript: Transcript = send(
        client
            .get(format!("{API}/transcriptions/{id}/transcript"))
            .bearer_auth(api_key),
    )
    .await
    .context("could not fetch the transcript")?;

    on_progress(Progress::Done);

    if !transcript.tokens.is_empty() {
        return Ok(transcript.tokens);
    }
    // Older shapes return a flat string; keep it usable rather than empty.
    Ok(transcript
        .text
        .filter(|t| !t.is_empty())
        .map(|text| vec![Token { text, is_final: true, ..Default::default() }])
        .unwrap_or_default())
}

/// Send a request and decode it, surfacing the server's message on failure.
async fn send<T: serde::de::DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T> {
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("{status}: {body}");
    }
    serde_json::from_str(&body).with_context(|| format!("unexpected response: {body}"))
}
