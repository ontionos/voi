//! Counts chunks from each capture source. Isolates capture from everything downstream.
//!
//! Run from inside the signed bundle so TCC has an identity to grant against:
//!   cargo build --release --example probe && ./target/release/examples/probe

use std::time::Duration;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();

    for name in ["mic", "system"] {
        let stream = match name {
            "mic" => voi_audio::MicSource::start(),
            _ => voi_audio::SystemSource::start(),
        };
        let mut stream = match stream {
            Ok(s) => {
                println!("{name}: opened {} Hz, {} ch", s.sample_rate, s.channels);
                s
            }
            Err(e) => {
                println!("{name}: FAILED {e:#}");
                continue;
            }
        };

        let mut chunks = 0usize;
        let mut samples = 0usize;
        let mut peak = 0.0f32;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);

        loop {
            match tokio::time::timeout_at(deadline, stream.recv()).await {
                Ok(Some(c)) => {
                    chunks += 1;
                    samples += c.samples.len();
                    for s in &c.samples {
                        peak = peak.max(s.abs());
                    }
                }
                Ok(None) => {
                    println!("{name}: stream closed early");
                    break;
                }
                Err(_) => break,
            }
        }
        println!("{name}: {chunks} chunks, {samples} samples, peak {peak:.4}\n");
    }
}
