//! The real system capture, opened and closed again and again, as the
//! supervisor does when a device goes away: every round must deliver audio in
//! real time, and nothing may stay open after it.
//!
//!   cargo run --release --example system_reopen_check -- [rounds] [seconds]
//!
//! Silent: it only captures. Prints one line per round and `RESULT: PASS`.

use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    // The supervisor's own word on what it did, if anything.
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    let mut args = std::env::args().skip(1);
    let rounds: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(20);
    let seconds: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(2.0);
    let mut failures = 0;
    for round in 1..=rounds {
        let started = Instant::now();
        let mut stream = match voi_audio::SystemSource::start() {
            Ok(s) => s,
            Err(e) => {
                println!("round {round}: FAILED to open: {e:#}");
                failures += 1;
                continue;
            }
        };
        let opened_ms = started.elapsed().as_millis();
        let rate = stream.sample_rate as f64;
        let channels = stream.channels.max(1) as f64;
        let (mut samples, mut longest_gap, mut last) = (0usize, Duration::ZERO, Instant::now());
        let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(seconds);
        while let Ok(Some(chunk)) = tokio::time::timeout_at(deadline, stream.recv()).await {
            longest_gap = longest_gap.max(last.elapsed());
            last = Instant::now();
            samples += chunk.samples.len();
        }
        drop(stream);
        let audio = samples as f64 / channels / rate;
        let open_after = voi_audio::open_streams();
        // Real time, give or take a buffer; no stall; nothing left open.
        let ok = audio >= seconds * 0.8 && longest_gap < Duration::from_millis(1500) && open_after == 0;
        if !ok {
            failures += 1;
        }
        println!(
            "round {round}: opened in {opened_ms} ms, {audio:.2} s of audio in {seconds:.1} s, longest gap {} ms, open after close: {open_after} {}",
            longest_gap.as_millis(),
            if ok { "ok" } else { "FAIL" }
        );
    }
    println!("RESULT: {}", if failures == 0 { "PASS".to_string() } else { format!("FAIL ({failures} of {rounds})") });
    std::process::exit(if failures == 0 { 0 } else { 1 });
}
