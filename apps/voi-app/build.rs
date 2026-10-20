//! On Windows, build Voi's icon (`assets/voi.ico`, made from the Mac's by
//! `scripts/make_ico.py`) into Voi.exe, so Explorer, the Start menu and the
//! taskbar show it. It needs the Windows SDK's resource compiler, which the
//! Visual Studio Build Tools bring; a check from another system skips it.
//!
//! Everywhere: the build number — commits plus `config/build-base`, as
//! `CFBundleVersion` —
//! from `VOI_BUILD`, which `scripts/bundle.sh` and
//! `scripts/package-windows.ps1` set; 0 for a plain `cargo build`. It is what
//! asking for updates compares with.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/voi.ico");
    println!("cargo:rerun-if-env-changed=VOI_BUILD");
    let build = std::env::var("VOI_BUILD").ok().and_then(|b| b.trim().parse::<u64>().ok()).unwrap_or(0);
    println!("cargo:rustc-env=VOI_BUILD={build}");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if windows && cfg!(windows) {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/voi.ico");
        res.set("FileDescription", "Voi");
        res.set("ProductName", "Voi");
        res.compile().expect("Voi.exe's icon could not be built in");
    }
}
