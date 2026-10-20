//! `voi-scan PATH…` — exit 1 if any artifact carries a Soniox key.
//!
//! What packaging runs on the finished zip or image before it is handed to
//! anybody. See the library for what counts.
//!
//! `voi-scan --git` scans the repository instead: every commit on every
//! branch, as patches, plus the files not yet committed.

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let git = args.first().is_some_and(|a| a == "--git");
    let paths: Vec<std::path::PathBuf> = args.into_iter().filter(|a| a != "--git").map(Into::into).collect();
    if paths.is_empty() && !git {
        eprintln!("usage: voi-scan <file, folder or zip>…  |  voi-scan --git");
        std::process::exit(2);
    }
    let keys = voi_scan::known_keys();
    if git {
        std::process::exit(scan_git(&keys));
    }
    if keys.is_empty() {
        println!("no Soniox key on this machine to compare against; checking for key-shaped strings only");
    } else {
        println!("comparing against {} key(s) found in the environment and the user config", keys.len());
    }
    let mut failed = false;
    for path in &paths {
        match voi_scan::scan_path(path, &keys) {
            Ok(found) if found.is_empty() => println!("clean: {}", path.display()),
            Ok(found) => {
                failed = true;
                for f in found {
                    eprintln!("FOUND {f}");
                }
            }
            Err(e) => {
                failed = true;
                eprintln!("could not scan {}: {e:#}", path.display());
            }
        }
    }
    std::process::exit(i32::from(failed));
}

/// History as patches, and the working tree's uncommitted files, in memory.
fn scan_git(keys: &[String]) -> i32 {
    let run = |args: &[&str]| -> Vec<u8> {
        match std::process::Command::new("git").args(args).output() {
            Ok(out) if out.status.success() => out.stdout,
            _ => {
                eprintln!("git {} failed — run this inside the repository", args.join(" "));
                std::process::exit(2);
            }
        }
    };
    let commits = String::from_utf8_lossy(&run(&["rev-list", "--all", "--count"])).trim().to_string();
    let history = run(&["log", "--all", "-p", "--binary", "--full-history", "--no-color"]);
    let mut found = voi_scan::scan_bytes("git history", &history, keys);
    let listing = run(&["ls-files", "-co", "--exclude-standard", "-z"]);
    for name in listing.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let name = String::from_utf8_lossy(name).to_string();
        if let Ok(bytes) = std::fs::read(&name) {
            found.extend(voi_scan::scan_named(&name, &bytes, keys).unwrap_or_default());
        }
    }
    if found.is_empty() {
        println!("clean: {commits} commits and the working tree");
        0
    } else {
        for f in found {
            eprintln!("FOUND {f}");
        }
        1
    }
}
