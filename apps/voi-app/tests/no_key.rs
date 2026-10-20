//! The app that gets built carries no Soniox key — checked on every
//! `cargo test`, against the key this machine actually uses.
//!
//! Packaging runs it again with `VOI_ARTIFACT` pointing at the finished zip or
//! image, so what is checked is what is handed over. See `voi-scan`.

#[test]
fn the_built_app_carries_no_key() {
    let keys = voi_scan::known_keys();
    let mut targets = vec![std::path::PathBuf::from(env!("CARGO_BIN_EXE_voi-app"))];
    if let Some(artifact) = std::env::var_os("VOI_ARTIFACT") {
        targets.push(artifact.into());
    }
    for target in targets {
        let found = voi_scan::scan_path(&target, &keys).expect("scannable");
        let report: Vec<String> = found.iter().map(ToString::to_string).collect();
        assert!(report.is_empty(), "{} must not ship:\n{}", target.display(), report.join("\n"));
    }
}
