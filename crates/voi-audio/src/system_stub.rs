//! System audio capture — unimplemented outside macOS and Windows.

use anyhow::{bail, Result};

use crate::AudioStream;

pub struct SystemSource;

impl SystemSource {
    pub fn start() -> Result<AudioStream> {
        bail!("system audio capture is not yet implemented on this platform")
    }
}
