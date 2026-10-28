# Voi — multilingual call transcription for macOS

**Date:** 2026-08-03
**Status:** Shipping. Originally built as "Meetly"; renamed to Voi once it
stopped resembling the project it started from.

## Problem

Record and transcribe calls in real time, in whatever languages they are in —
including code-switching mid-sentence, which is the normal case in multilingual
calls. Local Whisper handles this badly. Soniox handles mixed languages in a single
streaming call at ~$0.12/hour.

The app must eventually run on Windows, so platform-specific code stays behind a
narrow interface from day one.

## Validation performed before committing to this design

A 22.9s EN/UK/RU fixture generated with macOS TTS voices (Kathy, Lesya, Milena) was
streamed to the real Soniox API. Results:

- Transcription accurate across all three languages.
- Code-switching handled correctly: `я зроблю деплоймент на staging` — English
  technical term kept in Latin script, surrounding Ukrainian transliterated properly.
- Per-token language identification fired correctly (`en: 23, uk: 58, ru: 32`).
- First-token latency: 1.50s.
- **Speaker diarization proved unreliable** — it split a single continuous English
  utterance across two speaker IDs. This is the documented real-time limitation.
- **Undocumented `<end>` control token** is emitted with `speaker: null` when
  endpoint detection fires. It must be filtered, not concatenated into the transcript.

The two negative findings are load-bearing: they are why the design uses two
independent audio streams rather than one mixed stream with diarization.

## Architecture

Cargo workspace of plain library crates. A CLI drives them now; a Tauri UI drives
them later. Nothing platform-specific or UI-specific leaks into the core.

```
voi/
├── crates/
│   ├── voi-audio/    capture → PCM chunks (trait + per-platform impls)
│   ├── voi-soniox/   WebSocket client → transcript token events
│   └── voi-core/     session model, transcript assembly, storage
└── apps/
    └── voi-cli/      `voi listen`
```

### voi-audio

One trait, `AudioSource`, yielding `(Vec<f32>, sample_rate)` chunks:

| Impl | Purpose |
|---|---|
| `MicSource` | cpal default input. Cross-platform, works on Windows unchanged. |
| `SystemSource` | macOS: Core Audio process tap via `objc2-core-audio`. Windows: `Unsupported` stub until Phase 3. |
| `FileSource` | Replays a WAV as if live. Makes the whole stack testable without hardware. |

**No resampling and no mixing.** Each stream declares its own native sample rate to
Soniox and stays independent end to end. `f32 → s16le` is the only conversion, which
halves bandwidth versus sending `pcm_f32le`.

Keeping the streams separate is what produces reliable speaker attribution: the
microphone is the user by construction, and the system stream is everyone else.
No inference required.

### voi-soniox

`tokio-tungstenite` to `wss://stt-rt.soniox.com/transcribe-websocket`.

Config sent per stream:

```json
{ "model": "stt-rt-v5",
  "audio_format": "pcm_s16le",
  "sample_rate": "<native>",
  "num_channels": 1,
  "language_hints": ["uk", "ru", "en"],
  "enable_language_identification": true,
  "enable_endpoint_detection": true,
  "enable_speaker_diarization": "<true for system, false for mic>" }
```

Diarization is disabled on the microphone stream — that speaker is known by
construction, and disabling it avoids the documented real-time error rate. On the
system stream it only has to separate remote participants, and its labels are
treated as advisory hints rather than ground truth.

Token handling maintains `finalized` and `pending` buffers. Tokens with
`is_final: true` append permanently. Non-final tokens **replace the pending tail
wholesale** rather than appending. `<end>` control tokens are discarded.

### voi-core

Two token streams merge into a single timeline against a shared `t0` captured at
recording start, with each stream offset by its own connect delay. Sessions are
written to `~/Library/Application Support/voi/sessions/<uuid>/`:

```
audio-mic.wav   audio-system.wav   transcript.jsonl   session.json
```

WAV data streams to disk continuously, so a two-hour call never accumulates in RAM.

### Speakers in the microphone

The one place the two sides cross: whatever the speakers play leaks into the
microphone — faintly in a headset, loudly from laptop speakers — and a speech
model transcribes it a second time under "You". `voi-core::echo` holds it back
without a real echo canceller. The system stream is already known exactly, so
the gate learns how long after it the echo arrives (the lag at which the
microphone's loudness follows the system's most closely, from -480 to +400 ms —
the system tap can reach the app after its own echo) and how far below it the
echo sits (a low quartile while the system is loud). The microphone passes only
when it is well clear of that expected echo; otherwise it is replaced by silence
of the same length, so timestamps do not shift. A microphone that never follows
the system is left alone.

### Failure handling

The governing rule: **a network problem must never cost the recording.**

- Soniox disconnect → audio keeps writing to disk, PCM buffers in a bounded ring,
  the client reconnects with backoff and resumes. Only the pending tail is lost.
- The capture→WebSocket channel is bounded and drops oldest with a warning under
  backpressure. The capture→disk path never drops.
- Missing TCC permission → explicit message plus a link to the correct System
  Settings pane.
- Missing API key → fail at startup, before triggering any permission prompt.

### Testing

`FileSource` plus a mock WebSocket server keep almost everything testable offline:
token accumulation, final/non-final replacement, `<end>` filtering, reconnect and
resume, timeline merge, JSONL round-trip. One end-to-end test hits the real Soniox
API with the EN/UK/RU fixture. Live device capture is verified manually, since it
cannot be faked.

## Phases

| Phase | Scope |
|---|---|
| **1 (current)** | CLI prototype, macOS, dual-stream, live terminal transcript, WAV + JSONL on disk |
| **2** | Tauri 2 + Next.js + shadcn/Tailwind (light/dark/system), menu-bar tray, mic-in-use auto-detect with confirmation, OpenRouter summary and action items, session library |
| **3** | Windows: `SystemSource` via WASAPI loopback |

Recording is triggered by auto-detection with explicit confirmation (Phase 2). The
detection signal is microphone-in-use via Core Audio, not merely audio playback —
that alone excludes music and video playback.

## Findings from implementation

Three things were only discovered by building it, and each is pinned by a test or
a comment so it cannot be rediscovered the hard way:

**Soniox sends `speaker` as a string** (`"speaker":"1"`), not a number. A `u32`
field fails to deserialize and every message is rejected as malformed. The
parser now accepts either form; `real_server_payload_parses` pins a verbatim
captured payload.

**Rust 2021 disjoint capture silently killed audio capture.** `AudioStream`
exposed `rx` publicly, so `async move { stream.rx.recv() }` captured only that
one field and dropped the struct's device guard at the end of the enclosing
function. The device shut down within milliseconds and the channel closed —
presenting as capture backends that produced nothing at all, on both platforms
at once. `rx` is now private behind `recv(&mut self)`, which forces whole-struct
capture, and `moving_a_stream_into_a_task_keeps_the_guard_alive` is the
regression test.

**`cidre` requires a full Xcode install**; its build script shells out to
`xcodebuild`. Replaced with `objc2-core-audio`, which builds against Command
Line Tools alone and drops a heavy git dependency. The tap is written directly
against `CATapDescription` and `AudioHardwareCreateProcessTap`.

## Known risks

**Core Audio taps require `NSAudioCaptureUsageDescription` in an `Info.plist`**,
which a bare CLI binary does not have. Confirmed and handled: `scripts/bundle.sh`
wraps the binary in a minimal ad-hoc-signed `.app`. Re-signing changes the TCC
identity, so macOS re-prompts after a rebuild.

**The system capture is tied to the output device it opened on.** When that
device goes away mid-call (AirPods dropping out, headphones unplugged), the
capture dies with it, silently: on 2026-10-28 a call lost the other side for
its last fourteen minutes. `voi-audio/src/supervise.rs` now watches the stream
and opens it again on the current output device when it stops delivering for
2 s, when the default output changes, when the device reports an error, or (on
the Mac) when the device it is clocked by is no longer alive. Silence fills
the outage so the transcript's clock does not slip. A changed sample rate is
followed by the recorder.

**meetily's Windows system audio path is not usable.** Its WASAPI references are
device-name string matching for Bluetooth detection only; actual capture calls
`cpal::output_devices()`, and cpal 0.15 has no loopback support. Phase 3 is original
work, not a port.

**Legal:** call recording is two-party-consent in many jurisdictions. Out of scope
for the software, noted for the operator.

## Relationship to meetily

[Zackriya-Solutions/meetily](https://github.com/Zackriya-Solutions/meetily) is used
as a **reference, not a base**. Its `src-tauri/src/audio/capture/` module is 848 LOC
and Tauri-free, and `core_audio.rs` (445 LOC) is a working Core Audio
process-tap implementation worth adapting — though its `cidre` dependency was
dropped in favour of `objc2-core-audio` to avoid requiring Xcode. The one
genuinely hard-won detail taken from it is that the aggregate device must wrap
the tap *alone*; including the output device as well captures every sample twice.

The remaining ~18.7k LOC of its audio layer is Tauri-coupled orchestration plus
Whisper-era chunking, VAD and buffer pooling that a continuous Soniox stream makes
unnecessary. Its Whisper, Parakeet, llama-helper and Python backend components are
not used at all.


## What changed after the original design

The plan above held. What it did not anticipate:

**Audio is not kept.** The design assumed WAV alongside every transcript. In use
that was 690 MB an hour for recordings nobody plays back, so audio moved behind
`--keep-audio` and everything downstream dropped to 16 kHz mono — speech
recognition gains nothing above it.

**A session is one file, not a folder.** `transcript.jsonl` plus `session.json`
plus two WAVs meant opening four things to read one conversation. It is now a
single Markdown file with timestamps, which is also what the overlay shows.

**The overlay became the product.** The design treated the UI as a thin shell
over the recorder. In practice `voi-app` is the largest crate, and most of the
work went into making one travelling highlight look like light rather than a
line — four failed approaches, each documented in `overlay.rs` with why it
failed.

**File transcription was added.** A different model (`stt-async-v4`), a
different protocol, and up to five hours per request. It uses diarization
because a file has no sides.

**It notices calls by itself.** Core Audio exposes per-process input state, so
the overlay appears when anything else starts listening — the auto-detection the
design deferred to Phase 2, without needing to know which apps exist.

## Still open

- Windows: `SystemSource` via WASAPI loopback, and a second front end.
- LLM summaries and action items.
- Distribution: the bundle is ad-hoc signed, so Gatekeeper rejects it elsewhere.
- Real-time diarization among remote participants is available and measured as
  unreliable; deliberately left off.
