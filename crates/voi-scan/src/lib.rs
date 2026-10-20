//! Making sure nobody is handed your Soniox key along with the app.
//!
//! The key lives in the per-user data folder, never in the app — that is the
//! design, and it is what lets a build be given away. This is what checks the
//! design held: it reads every byte of an artifact (an executable, a folder,
//! a zip and every file inside it, zips inside zips) and fails on
//!
//! - the key itself — the one this machine actually uses, taken from
//!   `SONIOX_API_KEY` or the user's own config at the moment of the scan, so
//!   it is never written into the repository to compare against;
//! - anything shaped like a Soniox key: 64 hex digits standing on their own;
//! - a `SONIOX_API_KEY=` line with a value after it;
//! - a file that has no business in a build: `config.env`, `.env`,
//!   `settings.json`, a session transcript.
//!
//! Both the exact key and the shape are looked for as ASCII and as UTF-16,
//! the way Windows stores wide strings. Findings say what and where, never
//! the text they found.

use std::fmt;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

/// The length of a Soniox key, in hex digits.
pub const KEY_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// The key this machine uses.
    ExactKey,
    /// 64 hex digits on their own: a key, or something that cannot be told
    /// apart from one.
    KeyShaped,
    /// `SONIOX_API_KEY=` followed by a value.
    KeyAssignment,
    /// A file that only ever holds user data.
    UserFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Which file, with `!` between a zip and a member of it.
    pub file: String,
    pub offset: usize,
    pub kind: Kind,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            Kind::ExactKey => "the Soniox key in use on this machine",
            Kind::KeyShaped => "a 64-hex-digit string shaped like a Soniox key",
            Kind::KeyAssignment => "a SONIOX_API_KEY= line with a value",
            Kind::UserFile => "a user-data file (key, settings or transcript)",
        };
        write!(f, "{}: {what} at byte {}", self.file, self.offset)
    }
}

/// The keys to look for: `SONIOX_API_KEY`, and whatever this user's Voi would
/// use. Read now, from where they live, and kept in memory only.
pub fn known_keys() -> Vec<String> {
    let mut keys = Vec::new();
    if let Ok(k) = std::env::var("SONIOX_API_KEY") {
        keys.push(k.trim().to_string());
    }
    if let Ok(k) = voi_core::find_api_key() {
        keys.push(k);
    }
    keys.retain(|k| k.len() >= 16);
    keys.sort();
    keys.dedup();
    keys
}

/// Scan a file, a folder, or a zip.
pub fn scan_path(path: &Path, keys: &[String]) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .with_context(|| format!("cannot read {}", path.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for entry in entries {
            out.extend(scan_path(&entry, keys)?);
        }
        return Ok(out);
    }
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    out.extend(scan_named(&path.display().to_string(), &bytes, keys)?);
    Ok(out)
}

/// Scan one file's bytes, opening it up if it is a zip.
pub fn scan_named(name: &str, bytes: &[u8], keys: &[String]) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    if is_user_file(name) {
        out.push(Finding { file: name.to_string(), offset: 0, kind: Kind::UserFile });
    }
    out.extend(scan_bytes(name, bytes, keys));
    if bytes.starts_with(b"PK\x03\x04") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .with_context(|| format!("{name} looks like a zip but does not open"))?;
        for i in 0..archive.len() {
            let mut member = archive.by_index(i)?;
            let inner = format!("{name}!{}", member.name().map(|n| n.to_string()).unwrap_or_else(|_| format!("#{i}")));
            let mut data = Vec::new();
            member.read_to_end(&mut data)?;
            out.extend(scan_named(&inner, &data, keys)?);
        }
    }
    Ok(out)
}

fn is_user_file(name: &str) -> bool {
    let base = name.rsplit(['/', '\\', '!']).next().unwrap_or(name).to_ascii_lowercase();
    base == "config.env"
        || base == ".env"
        || base.starts_with(".env.") && base != ".env.example"
        || base == "settings.json"
        || base == "overlay-position"
        || (base.ends_with(".md") && base.len() > 15 && base.as_bytes()[..8].iter().all(u8::is_ascii_digit))
}

/// Scan raw bytes, as ASCII and as UTF-16LE.
pub fn scan_bytes(name: &str, bytes: &[u8], keys: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut push = |offset, kind| out.push(Finding { file: name.to_string(), offset, kind });

    for key in keys {
        for needle in [key.as_bytes().to_vec(), key.to_ascii_uppercase().into_bytes(), utf16(key)] {
            for offset in find_all(bytes, &needle) {
                push(offset, Kind::ExactKey);
            }
        }
    }
    for offset in hex_runs(bytes, 1) {
        // A crate's checksum in Cargo.lock is a SHA-256: the same shape, and
        // public by definition.
        if !bytes[..offset].ends_with(b"checksum = \"") {
            push(offset, Kind::KeyShaped);
        }
    }
    for offset in hex_runs(bytes, 2) {
        push(offset, Kind::KeyShaped);
    }
    for needle in [b"SONIOX_API_KEY=".to_vec(), utf16("SONIOX_API_KEY=")] {
        let step = if needle.len() > 15 { 2 } else { 1 };
        for offset in find_all(bytes, &needle) {
            let value = &bytes[offset + needle.len()..];
            let chars = value.iter().step_by(step).take_while(|b| b.is_ascii_alphanumeric()).count();
            if chars >= 16 {
                push(offset, Kind::KeyAssignment);
            }
        }
    }
    // The same place found as the key and as its shape is one finding.
    out.sort_by_key(|f| (f.offset, f.kind != Kind::ExactKey));
    out.dedup_by(|b, a| a.offset == b.offset);
    out
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return Vec::new();
    }
    hay.windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle)
        .map(|(i, _)| i)
        .collect()
}

/// Where runs of exactly [`KEY_LEN`] hex digits start, with nothing
/// alphanumeric either side. `stride` 2 reads UTF-16LE: each digit followed
/// by a zero byte.
fn hex_runs(bytes: &[u8], stride: usize) -> Vec<usize> {
    let at = |i: usize| -> Option<u8> {
        let b = *bytes.get(i)?;
        if stride == 2 && bytes.get(i + 1).copied() != Some(0) {
            return None;
        }
        Some(b)
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !at(i).is_some_and(|b| b.is_ascii_hexdigit()) {
            i += 1;
            continue;
        }
        let start = i;
        let mut n = 0;
        while at(i).is_some_and(|b| b.is_ascii_hexdigit()) {
            n += 1;
            i += stride;
        }
        let before = start.checked_sub(stride).and_then(at);
        let after = at(i);
        let alone = !before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !after.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_');
        if n == KEY_LEN && alone && looks_random(bytes, start, stride) {
            out.push(start);
        }
        if n == 0 {
            i += 1;
        }
    }
    out
}

/// A key is 64 random hex digits, which use nearly all sixteen of them; the
/// chance of one using fewer than ten is about one in 10^19. A run of one
/// digit — Rust's own formatting code carries 64 zeros for padding, in every
/// binary — or of a few is a table, not a key.
fn looks_random(bytes: &[u8], start: usize, stride: usize) -> bool {
    let mut seen = [false; 16];
    for i in 0..KEY_LEN {
        if let Some(d) = (bytes[start + i * stride] as char).to_digit(16) {
            seen[d as usize] = true;
        }
    }
    seen.iter().filter(|s| **s).count() >= 10
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key-shaped string made at run time, so the repository never holds
    /// one — not even a fake one the scan of the repository would trip on.
    fn fake_key() -> String {
        (0..KEY_LEN).map(|i| char::from_digit((i * 7 % 16) as u32, 16).unwrap()).collect()
    }

    #[test]
    fn the_key_is_found_however_it_is_stored() {
        let key = fake_key();
        let ascii = format!("junk\0{key}\0more").into_bytes();
        let wide: Vec<u8> = utf16(&format!("x={key};"));
        let keys = [key.clone()];
        assert_eq!(scan_bytes("a", &ascii, &keys)[0].kind, Kind::ExactKey);
        assert_eq!(scan_bytes("w", &wide, &keys)[0].kind, Kind::ExactKey);
        // Without knowing it, its shape still gives it away.
        assert_eq!(scan_bytes("a", &ascii, &[])[0].kind, Kind::KeyShaped);
        assert_eq!(scan_bytes("w", &wide, &[])[0].kind, Kind::KeyShaped);
    }

    #[test]
    fn hex_that_is_not_a_key_is_left_alone() {
        let key = fake_key();
        let longer = format!("{key}ab").into_bytes();
        let shorter = key[..40].as_bytes().to_vec();
        let glued = format!("id{key}").into_bytes();
        assert!(scan_bytes("x", &longer, &[]).is_empty(), "66 digits is not a key");
        assert!(scan_bytes("x", &shorter, &[]).is_empty());
        assert!(scan_bytes("x", &glued, &[]).is_empty(), "part of a longer word");
    }

    #[test]
    fn an_assignment_with_a_value_is_found_and_the_bare_name_is_not() {
        let line = format!("SONIOX_API_KEY={}", &fake_key()[..30]).into_bytes();
        assert!(scan_bytes("c", &line, &[]).iter().any(|f| f.kind == Kind::KeyAssignment));
        // The name on its own is in every build: it is how the key is read.
        assert!(scan_bytes("c", b"strip_prefix(\"SONIOX_API_KEY=\")", &[]).is_empty());
    }

    #[test]
    fn a_zip_is_opened_and_user_files_in_it_are_named() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("Voi/Voi.exe", opts).unwrap();
            zip.write_all(format!("MZ...{}...", fake_key()).as_bytes()).unwrap();
            zip.start_file("Voi/config.env", opts).unwrap();
            zip.write_all(b"nothing").unwrap();
            zip.finish().unwrap();
        }
        let found = scan_named("Voi.zip", &buf, &[fake_key()]).unwrap();
        assert!(found.iter().any(|f| f.file == "Voi.zip!Voi/Voi.exe" && f.kind == Kind::ExactKey), "{found:?}");
        assert!(found.iter().any(|f| f.file == "Voi.zip!Voi/config.env" && f.kind == Kind::UserFile), "{found:?}");
    }

    #[test]
    fn rusts_own_padding_is_not_a_key() {
        let zeroes = format!("fmt\\mod.rs\0{}\0", "0".repeat(KEY_LEN));
        assert!(scan_bytes("voi.exe", zeroes.as_bytes(), &[]).is_empty());
        let table = format!("\0{}\0", "0123".repeat(KEY_LEN / 4));
        assert!(scan_bytes("voi.exe", table.as_bytes(), &[]).is_empty(), "four digits over and over");
    }

    #[test]
    fn a_lockfile_checksum_is_not_a_key() {
        let lock = format!("name = \"zip\"\nchecksum = \"{}\"\n", fake_key());
        assert!(scan_bytes("Cargo.lock", lock.as_bytes(), &[]).is_empty());
        // Unless it is the key.
        assert!(!scan_bytes("Cargo.lock", lock.as_bytes(), &[fake_key()]).is_empty());
    }

    #[test]
    fn findings_never_repeat_what_they_found() {
        let key = fake_key();
        let found = scan_bytes("Voi.exe", key.as_bytes(), &[key.clone()]);
        let said = found[0].to_string();
        assert!(!said.contains(&key[..8]), "{said}");
    }
}
