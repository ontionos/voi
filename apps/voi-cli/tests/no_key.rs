//! The command line that gets built carries no Soniox key. See `voi-scan`.

#[test]
fn the_built_cli_carries_no_key() {
    let keys = voi_scan::known_keys();
    let path = std::path::PathBuf::from(env!("CARGO_BIN_EXE_voi"));
    let found = voi_scan::scan_path(&path, &keys).expect("scannable");
    let report: Vec<String> = found.iter().map(ToString::to_string).collect();
    assert!(report.is_empty(), "{} must not ship:\n{}", path.display(), report.join("\n"));
}
