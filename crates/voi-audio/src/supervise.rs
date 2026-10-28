//! Keeping the system stream ("Them") alive for a whole call.
//!
//! A capture of what the computer plays is tied to the output device that was
//! the default when it opened. When that device goes away mid-call — AirPods
//! dropping out for a second, headphones unplugged, a monitor's speakers
//! switched off — the capture dies with it, and nothing says so: on the Mac
//! the tap simply stops delivering blocks, on Windows the loopback stops while
//! the filler keeps topping it up with silence. On 2026-10-28 a call lost the
//! other side for its last fourteen minutes that way.
//!
//! So the platform stream is watched, and opened again on the device that is
//! the default *now*, when:
//! - it has delivered nothing for [`Timing::stall`] — a running capture always
//!   delivers, zeros included, so silence in the room never looks like this;
//! - the default output device is no longer the one it opened on;
//! - the platform reports the stream failed;
//! - its channel closed.
//!
//! While it is being reopened, silence keeps flowing at the last rate, so the
//! transcription service neither gives up for lack of audio nor puts what is
//! said afterwards early against the microphone's clock. Reopening is retried
//! for as long as the stream is wanted, backing off to [`Timing::retry_max`].
//! The first open is not retried: if there is no system audio at all when a
//! session starts, the caller hears about it, as before.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{AudioChunk, AudioStream};

/// A platform capture, opened once.
pub(crate) struct Opened {
    pub stream: AudioStream,
    /// Which device it captures, in the terms [`Platform::current_device`]
    /// answers in.
    pub device: String,
    /// Set by the platform when the stream reports an error it cannot recover
    /// from on its own.
    pub failed: Arc<AtomicBool>,
    /// Asked every [`Timing::poll`]: why the capture can no longer work, if
    /// the platform can tell — the device it is clocked by gone, say.
    pub check: Option<Box<dyn FnMut() -> Option<String> + Send>>,
}

/// How to open the system capture, and what the default device is now.
pub(crate) trait Platform: Send + 'static {
    fn open(&mut self) -> Result<Opened>;
    /// The device a fresh open would capture right now; `None` when it cannot
    /// tell, which never triggers a reopen by itself.
    fn current_device(&mut self) -> Option<String>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// Nothing delivered for this long means the capture is dead.
    pub stall: Duration,
    /// How often the default device is compared with the one in use.
    pub poll: Duration,
    /// The first wait before reopening after a failed attempt, doubling...
    pub retry_min: Duration,
    /// ...up to this.
    pub retry_max: Duration,
    /// The size of each block of silence sent while reopening.
    pub filler: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            stall: Duration::from_secs(2),
            poll: Duration::from_secs(1),
            retry_min: Duration::from_millis(250),
            retry_max: Duration::from_secs(5),
            filler: Duration::from_millis(100),
        }
    }
}

struct Guard {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Open the capture and keep it open; see the module notes.
pub(crate) fn supervise(mut platform: impl Platform, timing: Timing) -> Result<AudioStream> {
    let first = platform.open()?;
    let (sample_rate, channels) = (first.stream.sample_rate, first.stream.channels);
    let (tx, rx) = mpsc::channel::<AudioChunk>(64);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_watch = stop.clone();
    let thread = std::thread::Builder::new()
        .name("voi-system-watch".into())
        .spawn(move || watch(platform, first, tx, stop_watch, timing))?;
    let guard = Guard { stop, thread: Some(thread) };
    Ok(AudioStream::new(rx, sample_rate, channels, Box::new(guard)))
}

fn watch(mut platform: impl Platform, first: Opened, tx: mpsc::Sender<AudioChunk>, stop: Arc<AtomicBool>, timing: Timing) {
    let mut current = Some(first);
    let mut format = current.as_ref().map(|o| (o.stream.sample_rate, o.stream.channels)).unwrap_or((48_000, 1));
    let mut alive_at = Instant::now();
    let mut polled_at = Instant::now();
    // While reopening: since when, when to try next, how long to wait after.
    let mut lost_at = Instant::now();
    let mut next_try = Instant::now();
    let mut backoff = timing.retry_min;
    let mut filled = Duration::ZERO;
    let mut attempts = 0u32;

    while !stop.load(Ordering::Acquire) {
        if let Some(opened) = current.as_mut() {
            let mut why = None;
            loop {
                match opened.stream.rx.try_recv() {
                    Ok(chunk) => {
                        format = (chunk.sample_rate, chunk.channels);
                        if tx.blocking_send(chunk).is_err() {
                            return; // nobody listens any more
                        }
                        alive_at = Instant::now();
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        why = Some("its stream closed".to_string());
                        break;
                    }
                }
            }
            if why.is_none() && opened.failed.load(Ordering::Acquire) {
                why = Some("the device reported an error".into());
            }
            if why.is_none() && alive_at.elapsed() >= timing.stall {
                why = Some(format!("nothing arrived for {} ms", alive_at.elapsed().as_millis()));
            }
            if why.is_none() && polled_at.elapsed() >= timing.poll {
                polled_at = Instant::now();
                if let Some(check) = opened.check.as_mut() {
                    why = check();
                }
                if why.is_none() {
                    if let Some(now) = platform.current_device() {
                        if now != opened.device {
                            why = Some(format!("the output device changed to {now}"));
                        }
                    }
                }
            }
            if let Some(why) = why {
                warn!(device = %opened.device, reason = %why, "system audio lost; reopening it");
                current = None; // stops the old capture
                // From the last block that arrived, so the stall itself is
                // filled with silence too and the clock does not slip.
                lost_at = alive_at;
                next_try = Instant::now();
                backoff = timing.retry_min;
                filled = Duration::ZERO;
                attempts = 0;
                continue;
            }
        } else {
            // Silence for as long as there is no capture, in real time.
            let (rate, ch) = format;
            while filled + timing.filler <= lost_at.elapsed() {
                filled += timing.filler;
                let frames = (rate as f64 * timing.filler.as_secs_f64()).round() as usize;
                let silence = AudioChunk { samples: vec![0.0; frames * ch.max(1) as usize], sample_rate: rate, channels: ch };
                if tx.blocking_send(silence).is_err() {
                    return;
                }
            }
            if Instant::now() >= next_try {
                attempts += 1;
                match platform.open() {
                    Ok(opened) => {
                        info!(
                            device = %opened.device,
                            after_ms = lost_at.elapsed().as_millis() as u64,
                            attempts,
                            "system audio back"
                        );
                        alive_at = Instant::now();
                        polled_at = Instant::now();
                        current = Some(opened);
                        continue;
                    }
                    Err(e) => {
                        // Every attempt for a while, then now and then.
                        if attempts <= 3 || attempts % 20 == 0 {
                            warn!(error = %format!("{e:#}"), attempts, "system audio did not reopen yet");
                        }
                        next_try = Instant::now() + backoff;
                        backoff = (backoff * 2).min(timing.retry_max);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// What a fake device does once opened.
    #[derive(Clone, Copy)]
    enum Behaviour {
        /// Delivers 10 ms blocks of 0.5 for as long as it is open.
        Healthy,
        /// Delivers a few blocks, then nothing — the tap whose device left.
        DiesAfter(usize),
        /// Delivers, but reports a failure after a few blocks.
        FailsAfter(usize),
        /// Delivers zeros, as a tap does — but its device is gone after a few
        /// health checks: the case nothing else would catch.
        GoneAfter(usize),
    }

    /// Records every open and close, and plays each open's behaviour.
    #[derive(Default)]
    struct Log {
        opens: Vec<String>,
        live: usize,
    }

    struct Fake {
        script: Vec<Result<Behaviour, ()>>,
        device: Arc<Mutex<String>>,
        log: Arc<Mutex<Log>>,
    }

    struct FakeGuard {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        log: Arc<Mutex<Log>>,
    }

    impl Drop for FakeGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            self.log.lock().unwrap().live -= 1;
        }
    }

    impl Platform for Fake {
        fn open(&mut self) -> Result<Opened> {
            let next = if self.script.is_empty() { Ok(Behaviour::Healthy) } else { self.script.remove(0) };
            let behaviour = next.map_err(|_| anyhow::anyhow!("no device"))?;
            let device = self.device.lock().unwrap().clone();
            {
                let mut log = self.log.lock().unwrap();
                log.opens.push(device.clone());
                log.live += 1;
            }
            let (tx, rx) = mpsc::channel(64);
            let stop = Arc::new(AtomicBool::new(false));
            let failed = Arc::new(AtomicBool::new(false));
            let (stop_t, failed_t) = (stop.clone(), failed.clone());
            let thread = std::thread::spawn(move || {
                let mut sent = 0usize;
                while !stop_t.load(Ordering::Acquire) {
                    let deliver = match behaviour {
                        Behaviour::Healthy | Behaviour::GoneAfter(_) => true,
                        Behaviour::DiesAfter(n) => sent < n,
                        Behaviour::FailsAfter(n) => {
                            if sent >= n {
                                failed_t.store(true, Ordering::Release);
                            }
                            true
                        }
                    };
                    if deliver {
                        let chunk = AudioChunk { samples: vec![0.5; 160], sample_rate: 16_000, channels: 1 };
                        if tx.blocking_send(chunk).is_err() {
                            return;
                        }
                        sent += 1;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            let guard = FakeGuard { stop, thread: Some(thread), log: self.log.clone() };
            let check: Option<Box<dyn FnMut() -> Option<String> + Send>> = match behaviour {
                Behaviour::GoneAfter(n) => {
                    let mut asked = 0;
                    Some(Box::new(move || {
                        asked += 1;
                        (asked > n).then(|| "the output device is gone".to_string())
                    }))
                }
                _ => None,
            };
            Ok(Opened { stream: AudioStream::new(rx, 16_000, 1, Box::new(guard)), device, failed, check })
        }

        fn current_device(&mut self) -> Option<String> {
            Some(self.device.lock().unwrap().clone())
        }
    }

    fn fast() -> Timing {
        Timing {
            stall: Duration::from_millis(150),
            poll: Duration::from_millis(30),
            retry_min: Duration::from_millis(20),
            retry_max: Duration::from_millis(80),
            filler: Duration::from_millis(20),
        }
    }

    fn fake(script: Vec<Result<Behaviour, ()>>) -> (Fake, Arc<Mutex<String>>, Arc<Mutex<Log>>) {
        let device = Arc::new(Mutex::new("AirPods#41".to_string()));
        let log = Arc::new(Mutex::new(Log::default()));
        (Fake { script, device: device.clone(), log: log.clone() }, device, log)
    }

    /// What arrives within `for_ms`: (sound samples, silent samples), and the
    /// longest gap between two blocks.
    async fn collect(stream: &mut AudioStream, for_ms: u64) -> (usize, usize, Duration) {
        let (mut sound, mut silent) = (0, 0);
        let mut last = Instant::now();
        let mut gap = Duration::ZERO;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(for_ms);
        while let Ok(Some(chunk)) = tokio::time::timeout_at(deadline, stream.recv()).await {
            gap = gap.max(last.elapsed());
            last = Instant::now();
            if chunk.samples.iter().all(|s| *s == 0.0) {
                silent += chunk.samples.len();
            } else {
                sound += chunk.samples.len();
            }
        }
        (sound, silent, gap)
    }

    #[tokio::test]
    async fn a_capture_that_stops_delivering_is_opened_again_and_the_call_goes_on() {
        let (platform, _, log) = fake(vec![Ok(Behaviour::DiesAfter(5)), Ok(Behaviour::Healthy)]);
        let mut stream = supervise(platform, fast()).unwrap();
        let (sound, silent, gap) = collect(&mut stream, 800).await;
        assert_eq!(log.lock().unwrap().opens.len(), 2, "opened again after the stall");
        assert!(sound > 160 * 30, "sound after the reopen: {sound}");
        // Whole blocks of silence: all of the stall, less at most one block.
        let stall = (fast().stall - fast().filler).as_secs_f64();
        assert!(silent as f64 >= 16_000.0 * stall, "the stall was filled with silence: {silent}");
        // Nothing arrives during the stall window itself; never longer.
        assert!(gap < fast().stall + Duration::from_millis(100), "{gap:?}");
    }

    #[tokio::test]
    async fn a_new_default_output_device_is_followed() {
        let (platform, device, log) = fake(vec![]);
        let mut stream = supervise(platform, fast()).unwrap();
        let _ = collect(&mut stream, 150).await;
        *device.lock().unwrap() = "MacBook Pro Speakers#73".into();
        let (sound, _, _) = collect(&mut stream, 300).await;
        let opens = log.lock().unwrap().opens.clone();
        assert_eq!(opens, vec!["AirPods#41".to_string(), "MacBook Pro Speakers#73".to_string()]);
        assert!(sound > 0);
        assert_eq!(log.lock().unwrap().live, 1, "the old capture was closed");
    }

    #[tokio::test]
    async fn a_device_the_platform_says_is_gone_is_left_even_while_it_still_delivers() {
        let (platform, _, log) = fake(vec![Ok(Behaviour::GoneAfter(2)), Ok(Behaviour::Healthy)]);
        let mut stream = supervise(platform, fast()).unwrap();
        let _ = collect(&mut stream, 400).await;
        assert_eq!(log.lock().unwrap().opens.len(), 2);
        assert_eq!(log.lock().unwrap().live, 1);
    }

    #[tokio::test]
    async fn a_reported_failure_reopens() {
        let (platform, _, log) = fake(vec![Ok(Behaviour::FailsAfter(3)), Ok(Behaviour::Healthy)]);
        let mut stream = supervise(platform, fast()).unwrap();
        let _ = collect(&mut stream, 400).await;
        assert_eq!(log.lock().unwrap().opens.len(), 2);
    }

    #[tokio::test]
    async fn reopening_is_retried_until_a_device_comes_back_and_silence_flows_meanwhile() {
        let (platform, _, log) = fake(vec![Ok(Behaviour::DiesAfter(2)), Err(()), Err(()), Err(()), Ok(Behaviour::Healthy)]);
        let mut stream = supervise(platform, fast()).unwrap();
        let (sound, silent, gap) = collect(&mut stream, 1200).await;
        assert_eq!(log.lock().unwrap().opens.len(), 2, "one failed open after the first, three refused, then back");
        assert!(silent >= 16_000 * 2 / 10, "silence kept the clock going: {silent}");
        assert!(sound > 0, "and sound came back");
        assert!(gap < fast().stall + Duration::from_millis(100), "{gap:?}");
    }

    #[tokio::test]
    async fn a_first_open_that_fails_is_reported_not_retried() {
        let (platform, _, _) = fake(vec![Err(())]);
        assert!(supervise(platform, fast()).is_err());
    }

    #[tokio::test]
    async fn dropping_the_stream_closes_every_capture() {
        let (platform, _, log) = fake(vec![Ok(Behaviour::DiesAfter(2)), Ok(Behaviour::Healthy)]);
        let mut stream = supervise(platform, fast()).unwrap();
        let _ = collect(&mut stream, 500).await;
        drop(stream);
        assert_eq!(log.lock().unwrap().live, 0, "no device left open");
    }

    #[tokio::test]
    async fn a_healthy_capture_is_left_alone() {
        let (platform, _, log) = fake(vec![]);
        let mut stream = supervise(platform, fast()).unwrap();
        let (sound, silent, _) = collect(&mut stream, 600).await;
        assert_eq!(log.lock().unwrap().opens.len(), 1);
        assert_eq!(silent, 0);
        assert!(sound > 0);
    }
}
