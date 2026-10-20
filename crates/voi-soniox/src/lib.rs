//! Soniox real-time speech-to-text client.
//!
//! The protocol is a single WebSocket: a JSON config message, then raw PCM
//! frames, then an empty text frame to finish. The server streams back batches
//! of tokens, each flagged final or not.
//!
//! Two behaviours here are easy to get wrong and are the reason this logic is
//! isolated from the IO and unit-tested:
//!
//! 1. **Non-final tokens replace the pending tail wholesale**, they do not
//!    append. Appending them produces duplicated, stuttering text.
//! 2. **`<end>` and `<fin>` are control markers, not words.** They arrive with
//!    `speaker: null` when endpoint detection fires and must be dropped, or they
//!    end up concatenated into the transcript.

pub mod file;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use futures_util::stream::FuturesUnordered;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

pub const DEFAULT_URL: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
pub const DEFAULT_MODEL: &str = "stt-rt-v5";

/// A healthy connect takes about 0.6 s and the slowest seen took 2.5 s.
///
/// The outer limit on one whole connect — socket, TLS, upgrade, config. The
/// usual way a connect goes silent is a dropped socket, and [`ATTEMPT_DELAY`]
/// deals with that long before this runs out; this is for the rest.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one socket gets to answer before another is opened beside it.
///
/// Some networks drop a share of new connections outright — no answer, not
/// even to the kernel's own retries — while a fresh socket a moment later goes
/// straight through: on 2026-10-14 between a quarter and a half of the
/// connects to Soniox's edge did that, and each cost the press five seconds.
/// A socket to Soniox answers in a few milliseconds when it is going to
/// answer at all, and well inside this on a slow line, so the next one is
/// tried after this and the first to answer is used — RFC 8305's race, a
/// little quicker than its 250 ms, because a spare socket costs one packet.
pub const ATTEMPT_DELAY: Duration = Duration::from_millis(100);

/// Sockets one connect may open before it waits on the ones it has.
const MAX_ATTEMPTS: u32 = 16;

/// Control markers the server emits inline with real tokens.
const CONTROL_TOKENS: [&str; 2] = ["<end>", "<fin>"];

#[derive(Debug, Clone)]
pub struct SonioxConfig {
    pub api_key: String,
    pub url: String,
    pub model: String,
    pub sample_rate: u32,
    pub num_channels: u16,
    pub language_hints: Vec<String>,
    /// Names, jargon and product names to expect. Soniox calls this context,
    /// and it is a hint rather than a filter — nothing is forced to match, and
    /// a word not in the list is transcribed as well as it ever was.
    pub context_terms: Vec<String>,
    /// Leave off for a stream that is one known speaker by construction — it
    /// avoids the elevated real-time diarization error rate for no benefit.
    pub enable_speaker_diarization: bool,
    /// How long the socket and the handshake may take before the attempt is
    /// abandoned and retried. Without it a connect that neither succeeds nor
    /// fails waits on the operating system's TCP timeout — over a minute —
    /// and everything spoken in the meantime goes nowhere.
    pub connect_timeout: Duration,
    /// See [`ATTEMPT_DELAY`].
    pub attempt_delay: Duration,
}

impl SonioxConfig {
    pub fn new(api_key: impl Into<String>, sample_rate: u32, num_channels: u16) -> Self {
        Self {
            api_key: api_key.into(),
            url: DEFAULT_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            sample_rate,
            num_channels,
            language_hints: vec!["uk".into(), "ru".into(), "en".into()],
            context_terms: Vec::new(),
            enable_speaker_diarization: false,
            connect_timeout: CONNECT_TIMEOUT,
            attempt_delay: ATTEMPT_DELAY,
        }
    }

    fn to_handshake(&self) -> serde_json::Value {
        let mut msg = serde_json::json!({
            "api_key": self.api_key,
            "model": self.model,
            "audio_format": "pcm_s16le",
            "sample_rate": self.sample_rate,
            "num_channels": self.num_channels,
            "language_hints": self.language_hints,
            "enable_language_identification": true,
            "enable_endpoint_detection": true,
            "enable_speaker_diarization": self.enable_speaker_diarization,
        });
        // Only when there is something to say. An empty context object is not
        // the same as no context object, and there is no reason to find out
        // which way the server reads it.
        if !self.context_terms.is_empty() {
            msg["context"] = serde_json::json!({ "terms": self.context_terms });
        }
        msg
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub start_ms: u64,
    #[serde(default)]
    pub end_ms: u64,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub is_final: bool,
    /// The server sends this as a *string* (`"speaker":"1"`), not a number.
    /// Accept either, so a change on their side cannot break parsing.
    #[serde(default, deserialize_with = "de_speaker")]
    pub speaker: Option<u32>,
    #[serde(default)]
    pub language: Option<String>,
}

fn de_speaker<'de, D>(d: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Num(u32),
        Str(String),
    }
    Ok(match Option::<Raw>::deserialize(d)? {
        Some(Raw::Num(n)) => Some(n),
        Some(Raw::Str(s)) => s.trim().parse().ok(),
        None => None,
    })
}

impl Token {
    fn is_control(&self) -> bool {
        CONTROL_TOKENS.contains(&self.text.trim())
    }
}

#[derive(Debug, Deserialize)]
struct ServerMessage {
    #[serde(default)]
    tokens: Vec<Token>,
    #[serde(default)]
    finished: bool,
    #[serde(default)]
    error_code: Option<serde_json::Value>,
    #[serde(default)]
    error_message: Option<String>,
}

/// What the caller sees.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEvent {
    /// The stream is up; audio queued while connecting is on its way.
    Connected,
    /// Tokens promoted to final. These will never change.
    Final(Vec<Token>),
    /// The current unstable tail, replacing any previous pending text.
    Pending(Vec<Token>),
    /// The connection dropped and is being retried; pending text is void.
    Reconnecting { attempt: u32 },
    Finished,
    Error(String),
}

/// Turns raw server messages into events, applying the two rules in the module docs.
#[derive(Debug, Default)]
pub struct TokenAccumulator {
    /// Added to every token timestamp so a reconnect keeps one continuous timeline.
    pub time_offset_ms: u64,
}

impl TokenAccumulator {
    pub fn ingest(&self, msg_json: &str) -> Result<(Vec<TranscriptEvent>, bool)> {
        let msg: ServerMessage =
            serde_json::from_str(msg_json).context("malformed server message")?;

        if let Some(code) = msg.error_code {
            let text = msg.error_message.unwrap_or_default();
            return Ok((vec![TranscriptEvent::Error(format!("{code}: {text}"))], true));
        }

        let mut finals = Vec::new();
        let mut pending = Vec::new();
        for mut t in msg.tokens {
            if t.is_control() || t.text.is_empty() {
                continue;
            }
            t.start_ms += self.time_offset_ms;
            t.end_ms += self.time_offset_ms;
            if t.is_final {
                finals.push(t);
            } else {
                pending.push(t);
            }
        }

        let mut events = Vec::new();
        if !finals.is_empty() {
            events.push(TranscriptEvent::Final(finals));
        }
        // Always emit pending, even when empty: an empty pending set is how the
        // server says "the unstable tail is now gone", usually because it was
        // just promoted to final.
        events.push(TranscriptEvent::Pending(pending));

        if msg.finished {
            events.push(TranscriptEvent::Finished);
        }
        Ok((events, msg.finished))
    }
}

/// A live transcription session.
pub struct SonioxSession {
    /// Send `pcm_s16le` bytes here. Closing it ends the stream.
    pub audio: mpsc::Sender<Vec<u8>>,
    pub events: mpsc::Receiver<TranscriptEvent>,
}

/// Connect and run until the audio channel closes.
///
/// On an unexpected disconnect the session reconnects with backoff. Audio
/// captured during the outage is not replayed — it stays on disk, but it will be
/// missing from the transcript, and the timeline is offset so later timestamps
/// remain correct rather than restarting at zero.
pub fn spawn(config: SonioxConfig) -> SonioxSession {
    // Deep enough to hold what is said while a slow connect is retried: a
    // chunk is about 10 ms of audio, so this is ten seconds or so.
    let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(1024);
    let (event_tx, event_rx) = mpsc::channel::<TranscriptEvent>(256);

    tokio::spawn(async move {
        if let Err(e) = run(config, audio_rx, event_tx.clone()).await {
            let _ = event_tx.send(TranscriptEvent::Error(e.to_string())).await;
        }
    });

    SonioxSession { audio: audio_tx, events: event_rx }
}

async fn run(
    config: SonioxConfig,
    mut audio_rx: mpsc::Receiver<Vec<u8>>,
    event_tx: mpsc::Sender<TranscriptEvent>,
) -> Result<()> {
    let bytes_per_ms = (config.sample_rate as u64 * config.num_channels as u64 * 2) / 1000;
    let mut sent_bytes: u64 = 0;
    let mut attempt: u32 = 0;

    loop {
        let offset_ms = sent_bytes.checked_div(bytes_per_ms).unwrap_or(0);
        let acc = TokenAccumulator { time_offset_ms: offset_ms };
        let bytes_before = sent_bytes;

        let outcome =
            session_once(&config, &acc, &mut audio_rx, &event_tx, &mut sent_bytes).await;

        if session_was_healthy(sent_bytes.saturating_sub(bytes_before), bytes_per_ms) {
            attempt = 0;
        }

        // Audio the caller has finished with but nobody has sent. A key let go
        // while the connect was still pending leaves the whole utterance here,
        // and giving up on it is how a press loses everything that was said.
        let unsent = !audio_rx.is_empty();
        if event_tx.is_closed() {
            if unsent {
                let lost_ms = drain_bytes(&mut audio_rx) / bytes_per_ms.max(1);
                error!(lost_ms, "soniox never connected; speech lost (nobody waiting for it)");
            }
            return Ok(());
        }

        match outcome {
            Ok(Outcome::Complete) => return Ok(()),
            Ok(Outcome::Disconnected) if audio_rx.is_closed() && !unsent => return Ok(()),
            Err(e) if audio_rx.is_closed() && !unsent => {
                warn!(error = %format!("{e:#}"), "soniox did not connect before the audio ended");
                return Ok(());
            }
            Ok(Outcome::Disconnected) => {
                attempt += 1;
                warn!(attempt, "soniox disconnected, retrying");
            }
            Err(e) => {
                attempt += 1;
                warn!(attempt, error = %format!("{e:#}"), "soniox session failed, retrying");
            }
        }

        if attempt > 8 {
            let lost_ms = drain_bytes(&mut audio_rx) / bytes_per_ms.max(1);
            if lost_ms > 0 {
                error!(lost_ms, "soniox never connected; speech lost");
            }
            anyhow::bail!("soniox unreachable after {attempt} attempts");
        }
        let _ = event_tx.send(TranscriptEvent::Reconnecting { attempt }).await;
        let backoff = Duration::from_millis(250u64.saturating_mul(1 << attempt.min(6)));
        tokio::time::sleep(backoff.min(Duration::from_secs(15))).await;
    }
}

/// Whether a finished connection carried enough audio to count as healthy.
///
/// A session that streamed real audio proves the connection works, so it must
/// not count toward the retry budget. Without this the counter only ever
/// climbs, and a recording long enough to see a handful of blips — any
/// multi-hour call — would give up permanently.
fn session_was_healthy(bytes_this_session: u64, bytes_per_ms: u64) -> bool {
    const HEALTHY_MS: u64 = 30_000;
    bytes_per_ms > 0 && bytes_this_session > bytes_per_ms * HEALTHY_MS
}

enum Outcome {
    /// The caller closed the audio channel and the server acknowledged the end.
    Complete,
    /// The socket dropped unexpectedly.
    Disconnected,
}

/// rustls refuses to pick a crypto provider on its own when the choice is
/// ambiguous, and panics deep inside the TLS handshake if you skip this.
fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Everything still queued in `rx`, counted in bytes and thrown away.
fn drain_bytes(rx: &mut mpsc::Receiver<Vec<u8>>) -> u64 {
    let mut bytes = 0;
    while let Ok(chunk) = rx.try_recv() {
        bytes += chunk.len() as u64;
    }
    bytes
}

/// How a socket was won.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sockets {
    /// How many were opened in all.
    opened: u32,
    /// How many were still waiting for an answer when one came — each a
    /// connection the network dropped, or was about to.
    unanswered: u32,
}

/// A TCP connection to the host in `url`, raced across sockets.
///
/// Also how long the address lookup took, so the log can say where a slow
/// connect spent its time.
async fn open_socket(url: &str, delay: Duration) -> Result<(TcpStream, Sockets, Duration)> {
    let uri: Uri = url.parse().with_context(|| format!("not a url: {url}"))?;
    let host = uri.host().context("the soniox url has no host")?;
    let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("ws") { 80 } else { 443 });
    let looking = Instant::now();
    let found: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("could not look up {host}"))?
        .collect();
    let lookup = looking.elapsed();
    let (tcp, sockets) = race(&interleaved(found), delay, TcpStream::connect)
        .await
        .context("websocket connect failed")?;
    Ok((tcp, sockets, lookup))
}

/// Alternate the address families, so a network whose IPv6 goes nowhere
/// costs one [`ATTEMPT_DELAY`] rather than every IPv6 address in turn.
fn interleaved(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let first_v6 = addrs.first().is_some_and(SocketAddr::is_ipv6);
    let (mut lead, mut other): (Vec<_>, Vec<_>) =
        addrs.into_iter().partition(|a| a.is_ipv6() == first_v6);
    lead.reverse();
    other.reverse();
    let mut out = Vec::with_capacity(lead.len() + other.len());
    while !lead.is_empty() || !other.is_empty() {
        out.extend(lead.pop());
        out.extend(other.pop());
    }
    out
}

/// Connect to `addrs` the way RFC 8305 says to: one socket at a time, the
/// next after `delay` if none has answered, at once if one has failed; the
/// first to answer wins and the rest are dropped unanswered. Round the list
/// again if it runs out, because a dropped connection is about the socket,
/// not the address.
async fn race<S, F, Fut>(addrs: &[SocketAddr], delay: Duration, connect: F) -> Result<(S, Sockets)>
where
    F: Fn(SocketAddr) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<S>>,
{
    if addrs.is_empty() {
        anyhow::bail!("no addresses to connect to");
    }
    let mut racing = FuturesUnordered::new();
    let mut opened = 0u32;
    let mut last_error = None;
    let mut next = tokio::time::Instant::now();
    loop {
        let more = opened < MAX_ATTEMPTS;
        if !more && racing.is_empty() {
            return Err(last_error.map_or_else(|| anyhow!("nothing answered"), anyhow::Error::from));
        }
        tokio::select! {
            biased;

            Some(answer) = racing.next(), if !racing.is_empty() => match answer {
                Ok(socket) => {
                    let unanswered = racing.len() as u32;
                    return Ok((socket, Sockets { opened, unanswered }));
                }
                Err(e) => {
                    last_error = Some(e);
                    next = tokio::time::Instant::now();
                }
            },

            _ = tokio::time::sleep_until(next), if more => {
                let addr = addrs[opened as usize % addrs.len()];
                opened += 1;
                racing.push(connect(addr));
                next = tokio::time::Instant::now() + delay;
            }
        }
    }
}

async fn session_once(
    config: &SonioxConfig,
    acc: &TokenAccumulator,
    audio_rx: &mut mpsc::Receiver<Vec<u8>>,
    event_tx: &mpsc::Sender<TranscriptEvent>,
    sent_bytes: &mut u64,
) -> Result<Outcome> {
    ensure_crypto_provider();
    let started = Instant::now();
    let connecting = async {
        let (tcp, sockets, lookup) = open_socket(&config.url, config.attempt_delay).await?;
        let socket = started.elapsed();
        let (mut ws, _) = tokio_tungstenite::client_async_tls(config.url.as_str(), tcp)
            .await
            .context("websocket handshake failed")?;
        let upgraded = started.elapsed();
        ws.send(Message::Text(config.to_handshake().to_string()))
            .await
            .context("failed to send config")?;
        // Where the time went: looking the host up, the TCP connection, and
        // TLS with the WebSocket upgrade on top.
        let phases = (
            lookup.as_millis() as u64,
            (socket - lookup).as_millis() as u64,
            (upgraded - socket).as_millis() as u64,
        );
        anyhow::Ok((ws, sockets, phases))
    };
    let (ws, sockets, (lookup_ms, tcp_ms, tls_ws_ms)) = tokio::time::timeout(config.connect_timeout, connecting)
        .await
        .map_err(|_| anyhow!("no answer from soniox within {:?}", config.connect_timeout))??;
    let (mut sink, mut stream) = ws.split();
    let connect_ms = started.elapsed().as_millis() as u64;
    if sockets.unanswered > 0 {
        // The network dropped a connection; say so, or a slow press looks
        // like a slow server.
        warn!(
            unanswered = sockets.unanswered,
            opened = sockets.opened,
            connect_ms,
            "a socket to soniox went unanswered; another one got through"
        );
    }
    info!(
        sample_rate = config.sample_rate,
        diarization = config.enable_speaker_diarization,
        connect_ms,
        lookup_ms,
        tcp_ms,
        tls_ws_ms,
        sockets = sockets.opened,
        "soniox connected"
    );
    if event_tx.send(TranscriptEvent::Connected).await.is_err() {
        return Ok(Outcome::Complete); // consumer gone
    }

    // Soniox closes a stream that goes quiet — error 408, "request timeout" —
    // and it gives you a control message to say "still here". Audio normally
    // arrives every hundred milliseconds so this never fires; it matters when
    // the capture side stalls for a moment, or when the channel between us and
    // it fills and chunks are dropped, and it is the difference between a
    // pause and a dropped connection.
    const KEEPALIVE_AFTER: Duration = Duration::from_secs(3);
    let opened = Instant::now();
    let mut last_sent = Instant::now();
    let mut nudge = tokio::time::interval(Duration::from_secs(1));
    nudge.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut audio_done = false;
    loop {
        tokio::select! {
            biased;

            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(txt))) => {
                    let (events, finished) = acc.ingest(&txt)?;
                    let fatal = events.iter().any(|e| matches!(e, TranscriptEvent::Error(_)));
                    for e in events {
                        if event_tx.send(e).await.is_err() {
                            return Ok(Outcome::Complete); // consumer gone
                        }
                    }
                    if finished || fatal {
                        return Ok(Outcome::Complete);
                    }
                }
                closed @ (Some(Ok(Message::Close(_))) | None) => {
                    let frame = match closed {
                        Some(Ok(Message::Close(frame))) => frame,
                        _ => None,
                    };
                    // Say why, and how long it lasted. Without this a stream
                    // that drops every few minutes is indistinguishable from
                    // one that drops for a good reason.
                    let (code, reason) = match &frame {
                        Some(f) => (f.code.to_string(), f.reason.to_string()),
                        None => ("none".into(), "connection ended".into()),
                    };
                    if !audio_done {
                        warn!(
                            %code,
                            %reason,
                            lasted_s = opened.elapsed().as_secs(),
                            sent_kb = *sent_bytes / 1024,
                            "soniox closed the stream"
                        );
                    }
                    return Ok(if audio_done { Outcome::Complete } else { Outcome::Disconnected });
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    warn!(
                        error = %e,
                        lasted_s = opened.elapsed().as_secs(),
                        sent_kb = *sent_bytes / 1024,
                        "websocket read error"
                    );
                    return Ok(Outcome::Disconnected);
                }
            },

            chunk = audio_rx.recv(), if !audio_done => match chunk {
                Some(bytes) => {
                    *sent_bytes += bytes.len() as u64;
                    last_sent = Instant::now();
                    if sink.send(Message::Binary(bytes)).await.is_err() {
                        return Ok(Outcome::Disconnected);
                    }
                }
                None => {
                    // Empty text frame is Soniox's end-of-stream signal; the
                    // server then flushes remaining finals and sets `finished`.
                    audio_done = true;
                    let _ = sink.send(Message::Text(String::new())).await;
                }
            },

            _ = nudge.tick(), if !audio_done => {
                if last_sent.elapsed() >= KEEPALIVE_AFTER {
                    last_sent = Instant::now();
                    let msg = serde_json::json!({ "type": "keepalive" }).to_string();
                    if sink.send(Message::Text(msg)).await.is_err() {
                        return Ok(Outcome::Disconnected);
                    }
                }
            }
        }
    }
}

impl Default for Token {
    fn default() -> Self {
        Self {
            text: String::new(),
            start_ms: 0,
            end_ms: 0,
            confidence: 0.0,
            is_final: false,
            speaker: None,
            language: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc() -> TokenAccumulator {
        TokenAccumulator::default()
    }

    /// The dictionary goes out in the shape Soniox documents, and only when
    /// there is one. An empty context object is not the same as none.
    #[test]
    fn the_handshake_carries_the_context_only_when_it_has_terms() {
        let mut cfg = SonioxConfig::new("k", 16_000, 1);
        assert!(cfg.to_handshake().get("context").is_none(), "nothing to say, nothing sent");

        cfg.context_terms = vec!["Acme".into(), "playable ad".into()];
        let msg = cfg.to_handshake();
        assert_eq!(msg["context"]["terms"][0], "Acme");
        assert_eq!(msg["context"]["terms"][1], "playable ad");
        assert_eq!(msg["language_hints"][0], "uk", "hints are still their own field");
    }

    #[test]
    fn control_tokens_are_dropped() {
        let json = r#"{"tokens":[
            {"text":"hello","is_final":true,"start_ms":0,"end_ms":100},
            {"text":"<end>","is_final":true,"speaker":null},
            {"text":"<fin>","is_final":true}
        ]}"#;
        let (events, _) = acc().ingest(json).unwrap();
        match &events[0] {
            TranscriptEvent::Final(t) => {
                assert_eq!(t.len(), 1, "control markers must not reach the transcript");
                assert_eq!(t[0].text, "hello");
            }
            other => panic!("expected Final, got {other:?}"),
        }
    }

    #[test]
    fn final_and_pending_are_separated() {
        let json = r#"{"tokens":[
            {"text":"Привіт","is_final":true},
            {"text":" світ","is_final":false}
        ]}"#;
        let (events, _) = acc().ingest(json).unwrap();
        assert_eq!(events[0], TranscriptEvent::Final(vec![Token {
            text: "Привіт".into(), is_final: true, ..Default::default()
        }]));
        match &events[1] {
            TranscriptEvent::Pending(t) => assert_eq!(t[0].text, " світ"),
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn empty_pending_is_still_emitted() {
        // This is how the server signals the unstable tail is gone. Without it,
        // a UI would leave stale grey text on screen forever.
        let json = r#"{"tokens":[{"text":"done","is_final":true}]}"#;
        let (events, _) = acc().ingest(json).unwrap();
        assert_eq!(events[1], TranscriptEvent::Pending(vec![]));
    }

    #[test]
    fn timestamps_are_offset_after_reconnect() {
        let a = TokenAccumulator { time_offset_ms: 5_000 };
        let json = r#"{"tokens":[{"text":"x","is_final":true,"start_ms":100,"end_ms":200}]}"#;
        let (events, _) = a.ingest(json).unwrap();
        match &events[0] {
            TranscriptEvent::Final(t) => {
                assert_eq!(t[0].start_ms, 5_100);
                assert_eq!(t[0].end_ms, 5_200);
            }
            other => panic!("expected Final, got {other:?}"),
        }
    }

    #[test]
    fn errors_are_surfaced_and_terminal() {
        let json = r#"{"error_code":401,"error_message":"invalid api key"}"#;
        let (events, finished) = acc().ingest(json).unwrap();
        assert!(finished, "an error must terminate the session");
        assert_eq!(
            events[0],
            TranscriptEvent::Error("401: invalid api key".into())
        );
    }

    /// Captured verbatim from the live API. Speaker arrives as a string and the
    /// `<end>` marker rides along with real tokens — both cost a debugging round
    /// trip the first time, so they are pinned here.
    #[test]
    fn real_server_payload_parses() {
        let json = r#"{"tokens":[
            {"text":"Hi","start_ms":420,"end_ms":480,"confidence":0.891,"is_final":true,"speaker":"1","language":"en"},
            {"text":" ever","start_ms":660,"end_ms":720,"confidence":0.998,"is_final":true,"speaker":"1","language":"en"},
            {"text":"<end>","start_ms":0,"end_ms":0,"confidence":1,"is_final":true}
        ],"final_audio_proc_ms":1440,"total_audio_proc_ms":1440}"#;
        let (events, _) = acc().ingest(json).unwrap();
        match &events[0] {
            TranscriptEvent::Final(t) => {
                assert_eq!(t.len(), 2);
                assert_eq!(t[0].speaker, Some(1), "string speaker ids must parse");
                assert_eq!(t[0].language.as_deref(), Some("en"));
            }
            other => panic!("expected Final, got {other:?}"),
        }
    }

    #[test]
    fn empty_first_message_is_harmless() {
        // The server opens with this before any audio has been processed.
        let (events, finished) = acc()
            .ingest(r#"{"tokens":[],"final_audio_proc_ms":0,"total_audio_proc_ms":0}"#)
            .unwrap();
        assert!(!finished);
        assert_eq!(events, vec![TranscriptEvent::Pending(vec![])]);
    }

    #[test]
    fn finished_flag_produces_finished_event() {
        let (events, finished) = acc().ingest(r#"{"tokens":[],"finished":true}"#).unwrap();
        assert!(finished);
        assert!(events.contains(&TranscriptEvent::Finished));
    }

    #[test]
    fn retry_budget_resets_after_a_healthy_session() {
        // 16kHz mono s16le.
        let per_ms = 32;
        assert!(
            session_was_healthy(per_ms * 60_000, per_ms),
            "a minute of streamed audio means the connection works"
        );
        assert!(
            !session_was_healthy(per_ms * 1_000, per_ms),
            "a session that died after a second is a genuine failure"
        );
        // Guards against a divide-by-zero path when the rate is unknown.
        assert!(!session_was_healthy(1_000_000, 0));
    }

    /// A server that takes the connection and never answers it — what a
    /// stalled route or a half-open socket looks like from here. The session
    /// has to give up on it and try again, not wait on the operating system.
    #[tokio::test]
    async fn a_connect_that_never_answers_is_abandoned_and_retried() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });

        let mut cfg = SonioxConfig::new("k", 16_000, 1);
        cfg.url = format!("ws://127.0.0.1:{port}");
        cfg.connect_timeout = Duration::from_millis(200);
        let mut session = spawn(cfg);

        let first = tokio::time::timeout(Duration::from_secs(3), session.events.recv())
            .await
            .expect("a silent server must not stall the session");
        assert_eq!(first, Some(TranscriptEvent::Reconnecting { attempt: 1 }));

        // Letting go while it is still trying ends it, rather than leaving it
        // to retry for a microphone that has already closed.
        drop(session.audio);
        let end = tokio::time::timeout(Duration::from_secs(3), async {
            while session.events.recv().await.is_some() {}
        })
        .await;
        assert!(end.is_ok(), "the session must end once the audio has");
    }

    /// What the network did on 2026-10-14: a socket that is never answered,
    /// not even refused, beside one that answers at once. The connect must not
    /// sit on the first one.
    #[tokio::test]
    async fn a_socket_that_is_never_answered_loses_to_the_next_one() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = listener.local_addr().unwrap();
        let opened = std::sync::atomic::AtomicU32::new(0);

        let started = Instant::now();
        let (_, sockets) = race(&[live], Duration::from_millis(100), |addr| {
            // The first socket goes into the void; every later one is real.
            let first = opened.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            async move {
                if first {
                    std::future::pending::<()>().await;
                    unreachable!()
                } else {
                    TcpStream::connect(addr).await
                }
            }
        })
        .await
        .expect("the second socket answers");
        let took = started.elapsed();

        assert_eq!(sockets, Sockets { opened: 2, unanswered: 1 });
        assert!(took >= Duration::from_millis(100), "the first socket gets its moment: {took:?}");
        assert!(took < Duration::from_millis(600), "and not much more: {took:?}");
    }

    /// A network that refuses everything is an error, quickly — not a loop
    /// that opens sockets for ever.
    #[tokio::test]
    async fn a_connect_refused_everywhere_gives_up() {
        let refused: SocketAddr = "127.0.0.1:9".parse().unwrap();
        // How long one refusal takes here. Immediate on macOS; Windows tries a
        // refused port again twice, half a second apart, before it believes
        // it, so the race can be no quicker than that.
        let once = Instant::now();
        assert!(TcpStream::connect(refused).await.is_err());
        let refusal = once.elapsed();

        let started = Instant::now();
        let result = race(&[refused], Duration::from_millis(100), TcpStream::connect).await;
        assert!(result.is_err());
        let took = started.elapsed();
        assert!(took < refusal + Duration::from_secs(2), "{took:?}, one refusal {refusal:?}");
    }

    #[test]
    fn address_families_take_turns() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        let mixed = vec![a("[::1]:1"), a("[::2]:1"), a("1.0.0.1:1"), a("1.0.0.2:1")];
        assert_eq!(
            interleaved(mixed),
            vec![a("[::1]:1"), a("1.0.0.1:1"), a("[::2]:1"), a("1.0.0.2:1")]
        );
        assert_eq!(interleaved(vec![a("1.0.0.1:1")]), vec![a("1.0.0.1:1")]);
    }

    /// A Soniox stand-in. Connections listed in `silent` are taken and never
    /// answered, the way a stalled connect looks from here; the rest read the
    /// config, count the audio, and on the end-of-audio frame answer with one
    /// final token saying how many bytes arrived, and `finished`.
    async fn fake_soniox(silent: Vec<usize>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            let mut n = 0;
            while let Ok((socket, _)) = listener.accept().await {
                n += 1;
                if silent.contains(&n) {
                    held.push(socket);
                    continue;
                }
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
                    let mut heard = 0usize;
                    let mut config = false;
                    while let Some(Ok(msg)) = ws.next().await {
                        match msg {
                            Message::Text(t) if !config => config = !t.is_empty(),
                            Message::Binary(b) => heard += b.len(),
                            Message::Text(t) if t.is_empty() => {
                                let answer = serde_json::json!({
                                    "tokens": [{"text": heard.to_string(), "is_final": true}],
                                    "finished": true,
                                });
                                let _ = ws.send(Message::Text(answer.to_string())).await;
                                break;
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        format!("ws://127.0.0.1:{port}")
    }

    /// One press: speak `chunks`, let go, and read what the server heard.
    /// Returns the bytes it reports, and how long until Soniox was there.
    async fn press(url: &str, chunks: usize, let_go_at_once: bool) -> (usize, Duration) {
        let mut cfg = SonioxConfig::new("k", 16_000, 1);
        cfg.url = url.to_string();
        cfg.connect_timeout = Duration::from_millis(300);
        let started = Instant::now();
        let mut session = spawn(cfg);
        for _ in 0..chunks {
            session.audio.send(vec![0u8; 320]).await.unwrap();
        }
        if !let_go_at_once {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        drop(session.audio);

        let mut connected = None;
        let mut heard = None;
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = session.events.recv().await {
                match event {
                    TranscriptEvent::Connected => connected = Some(started.elapsed()),
                    TranscriptEvent::Final(t) => heard = Some(t[0].text.parse().unwrap()),
                    _ => {}
                }
            }
        })
        .await
        .expect("the press must end");
        (heard.expect("the server must hear the press"), connected.expect("it must connect"))
    }

    /// 16:10:05 on 2026-10-14: the key was let go before Soniox answered, and
    /// the 2.8 s said into it were thrown away with "soniox did not connect
    /// before the audio ended". What was said while connecting is kept and
    /// sent, however the connect goes.
    #[tokio::test]
    async fn speech_held_for_a_late_connect_is_sent_not_dropped() {
        let url = fake_soniox(vec![1]).await;
        let (heard, connected) = press(&url, 25, true).await;
        assert_eq!(heard, 25 * 320, "every byte said before the connect arrives");
        assert!(connected >= Duration::from_millis(300), "the first connect was the silent one");
    }

    /// Presses one after another, every other connect silent: each one still
    /// connects, and each one's words all arrive — the previous session
    /// neither blocks the next nor takes its audio.
    #[tokio::test]
    async fn consecutive_presses_each_deliver_everything() {
        let url = fake_soniox(vec![2, 4, 6, 8]).await;
        for (i, chunks) in [10, 40, 3, 25, 60].into_iter().enumerate() {
            let let_go_at_once = i % 2 == 0;
            let (heard, connected) = press(&url, chunks, let_go_at_once).await;
            assert_eq!(heard, chunks * 320, "press {i}");
            assert!(connected < Duration::from_secs(2), "press {i} took {connected:?}");
        }
    }

    #[test]
    fn handshake_disables_diarization_when_asked() {
        let mut c = SonioxConfig::new("k", 16000, 1);
        c.enable_speaker_diarization = false;
        let h = c.to_handshake();
        assert_eq!(h["enable_speaker_diarization"], false);
        assert_eq!(h["audio_format"], "pcm_s16le");
        assert_eq!(h["sample_rate"], 16000);
        assert_eq!(h["language_hints"], serde_json::json!(["uk", "ru", "en"]));
    }
}
