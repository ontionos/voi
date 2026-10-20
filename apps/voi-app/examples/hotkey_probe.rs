//! Proof that the record hotkey is exclusive, run on a real Windows desktop.
//!
//! `hotkey_probe --voi PATH\Voi.exe --data DIR --out RESULT.txt`
//!
//! The probe plays "every other program" before Voi starts: it installs a
//! low-level keyboard hook of its own, which is how any program sees keys
//! without having focus, and registers the same combination with
//! `RegisterHotKey`, which is how most programs claim a shortcut. Windows
//! calls the newest hook first, so once Voi is up, a key Voi swallows never
//! reaches the probe by either route. Then:
//!
//! 1. Voi bound to Ctrl+Shift+F23: the combination is pressed (injected) along
//!    with an unrelated F22. Voi must report the press; the probe must see
//!    F22 and the modifiers but no F23 and no WM_HOTKEY.
//! 2. The same with Voi gone: now the probe must see F23 and get WM_HOTKEY —
//!    the control showing the checks in 1 can fail.
//! 3. The round trip: Voi started with Settings waiting for a key, Ctrl+Alt+F21
//!    pressed, Voi stopped, `settings.json` read, Voi started again — and
//!    Ctrl+Alt+F21 must now be the hotkey, swallowed, with the mask key tapped
//!    behind it because Alt was held.
//!
//! F21–F23 are on no keyboard, so nothing on the desktop answers to them if a
//! step goes wrong. Voi never takes focus during any of it, which is checked.

#[cfg(not(windows))]
fn main() {
    eprintln!("hotkey_probe runs on Windows");
}

#[cfg(windows)]
fn main() {
    if let Err(e) = probe::run() {
        eprintln!("probe failed to run: {e:#}");
        std::process::exit(2);
    }
}

#[cfg(windows)]
mod probe {
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use anyhow::{bail, Context, Result};
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, PostThreadMessageW,
        SetWindowsHookExW, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_QUIT,
        WM_SYSKEYDOWN,
    };

    const CTRL: u16 = 0xA2;
    const SHIFT: u16 = 0xA0;
    const ALT: u16 = 0xA4;
    const F21: u16 = 0x84;
    const F22: u16 = 0x85;
    const F23: u16 = 0x86;
    const MASK: u16 = 0xE8;

    /// Everything the probe's hook saw: (key, down).
    static SEEN: Mutex<Vec<(u16, bool)>> = Mutex::new(Vec::new());
    static HOTKEYS: Mutex<u32> = Mutex::new(0);

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let info = &*(lparam as *const KBDLLHOOKSTRUCT);
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            SEEN.lock().unwrap().push((info.vkCode as u16, down));
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    /// The probe's own hook and hotkey, on a thread with a message loop.
    fn listen() -> u32 {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), GetModuleHandleW(std::ptr::null()), 0);
            // Both of the combinations Voi will be holding, claimed first.
            RegisterHotKey(std::ptr::null_mut(), 1, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, F23 as u32);
            if std::env::var_os("PROBE_NO_F21").is_none() {
                RegisterHotKey(std::ptr::null_mut(), 2, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, F21 as u32);
            }
            tx.send(GetCurrentThreadId()).unwrap();
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                if msg.message == WM_HOTKEY {
                    *HOTKEYS.lock().unwrap() += 1;
                }
            }
        });
        rx.recv().unwrap()
    }

    fn press(keys: &[(u16, bool)]) {
        let inputs: Vec<INPUT> = keys
            .iter()
            .map(|&(vk, down)| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: if down { 0 } else { KEYEVENTF_KEYUP },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
            .collect();
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        std::thread::sleep(Duration::from_millis(400));
    }

    fn combo(mods: &[u16], key: u16) -> Vec<(u16, bool)> {
        let mut v: Vec<(u16, bool)> = mods.iter().map(|m| (*m, true)).collect();
        v.push((key, true));
        v.push((key, false));
        v.extend(mods.iter().rev().map(|m| (*m, false)));
        v
    }

    fn take() -> (Vec<(u16, bool)>, u32) {
        let seen = std::mem::take(&mut *SEEN.lock().unwrap());
        let hk = std::mem::take(&mut *HOTKEYS.lock().unwrap());
        (seen, hk)
    }

    fn log_path() -> PathBuf {
        PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Voi").join("Voi.log")
    }

    fn log_len() -> u64 {
        std::fs::metadata(log_path()).map(|m| m.len()).unwrap_or(0)
    }

    fn log_since(from: u64) -> String {
        let bytes = std::fs::read(log_path()).unwrap_or_default();
        String::from_utf8_lossy(&bytes[(from as usize).min(bytes.len())..]).to_string()
    }

    fn wait_log(from: u64, needle: &str, secs: u64) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            if log_since(from).contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    struct Voi(std::process::Child);
    /// Closed the way Windows closes a program — its windows asked to close —
    /// so the tray icon goes with it instead of lingering as a ghost.
    impl Drop for Voi {
        fn drop(&mut self) {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &self.0.id().to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(4) {
                if let Ok(Some(_)) = self.0.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn start(voi: &Path, data: &Path, args: &[&str]) -> Result<(Voi, u64)> {
        let from = log_len();
        let child = std::process::Command::new(voi)
            .args(args)
            .env("VOI_DATA_DIR", data)
            .spawn()
            .context("start Voi")?;
        let voi = Voi(child);
        if !wait_log(from, "keyboard hook installed", 20) {
            bail!("Voi did not report its keyboard hook within 20 s");
        }
        std::thread::sleep(Duration::from_millis(300));
        Ok((voi, from))
    }

    /// Hand the foreground to a window, which Windows allows the process that
    /// sent the last input — so the probe sends some first. A mouse move of
    /// nothing, not a key: Settings is waiting to take the next key pressed.
    fn bring_to_front(title: &str) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow};
        let name: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), name.as_ptr());
            if hwnd.is_null() {
                return false;
            }
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOUSEINPUT, INPUT_MOUSE, MOUSEEVENTF_MOVE};
            let nudge = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 },
                },
            };
            SendInput(1, &nudge, std::mem::size_of::<INPUT>() as i32);
            std::thread::sleep(Duration::from_millis(100));
            SetForegroundWindow(hwnd);
            std::thread::sleep(Duration::from_millis(300));
            GetForegroundWindow() == hwnd
        }
    }

    fn foreground_is(pid: u32) -> bool {
        unsafe {
            let mut owner = 0u32;
            GetWindowThreadProcessId(GetForegroundWindow(), &mut owner);
            owner == pid
        }
    }

    pub fn run() -> Result<()> {
        let mut args = std::env::args().skip(1);
        let (mut voi, mut data, mut out) = (None, None, None);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--voi" => voi = args.next().map(PathBuf::from),
                "--data" => data = args.next().map(PathBuf::from),
                "--out" => out = args.next().map(PathBuf::from),
                _ => {}
            }
        }
        let (voi, data, out) = (voi.context("--voi")?, data.context("--data")?, out.context("--out")?);
        std::fs::create_dir_all(&data)?;
        let settings = data.join("settings.json");
        std::fs::write(&settings, r#"{ "record_hotkey": "Ctrl+Shift+F23" }"#)?;

        let thread = listen();
        std::thread::sleep(Duration::from_millis(300));
        let mut report = String::new();
        let mut ok = true;
        let mut check = |report: &mut String, pass: bool, what: &str| {
            ok &= pass;
            let _ = writeln!(report, "{} {what}", if pass { "PASS" } else { "FAIL" });
        };

        // 1. Swallowed while Voi holds it.
        {
            let (child, from) = start(&voi, &data, &[])?;
            let pid = child.0.id();
            take();
            press(&combo(&[CTRL, SHIFT], F23));
            press(&[(F22, true), (F22, false)]);
            let (seen, hotkeys) = take();
            let log = log_since(from);
            let _ = writeln!(report, "step 1 probe saw {seen:x?}, WM_HOTKEY {hotkeys}");
            check(&mut report, log.matches("record hotkey pressed").count() == 1, "Voi got Ctrl+Shift+F23 exactly once");
            check(&mut report, !seen.iter().any(|(k, _)| *k == F23), "no other hook saw F23 go down or up");
            check(&mut report, hotkeys == 0, "the program that registered Ctrl+Shift+F23 first got no WM_HOTKEY");
            check(&mut report, seen.contains(&(F22, true)) && seen.contains(&(F22, false)), "an unrelated key (F22) went through untouched");
            check(&mut report, seen.contains(&(CTRL, true)) && seen.contains(&(SHIFT, false)), "the modifiers went through");
            check(&mut report, !foreground_is(pid), "Voi never took the foreground");
        }

        // 2. The control: with Voi gone the same press reaches everyone.
        {
            std::thread::sleep(Duration::from_millis(500));
            take();
            press(&combo(&[CTRL, SHIFT], F23));
            let (seen, hotkeys) = take();
            let _ = writeln!(report, "step 2 probe saw {seen:x?}, WM_HOTKEY {hotkeys}");
            check(&mut report, seen.contains(&(F23, true)), "control: without Voi, the probe's hook sees F23");
            check(&mut report, hotkeys == 1, "control: without Voi, the registered hotkey fires");
        }

        // 3. Chosen in Settings, saved, and still in force after a restart.
        {
            let (child, from) = start(&voi, &data, &["--capture-hotkey"])?;
            // Keys pressed while the window is still being made are not seen
            // by anyone's hook; a person cannot press that fast anyway.
            if !wait_log(from, "settings opened", 20) {
                bail!("Settings did not open within 20 s");
            }
            // In front, the way it is when somebody clicks the box: a Voi
            // window in front is exactly when Windows used to stop calling
            // Voi's hook (see `drop_raw_keyboard` in the app).
            let settings_window = bring_to_front("Voi Settings");
            check(&mut report, settings_window, "Settings is the window in front");
            std::thread::sleep(Duration::from_millis(700));
            press(&combo(&[CTRL, ALT], F21));
            let t = Instant::now();
            let mut saved = String::new();
            while t.elapsed() < Duration::from_secs(5) {
                saved = std::fs::read_to_string(&settings).unwrap_or_default();
                if saved.contains("Ctrl+Alt+F21") {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            check(&mut report, saved.contains(r#""record_hotkey": "Ctrl+Alt+F21""#), "capturing Ctrl+Alt+F21 in Settings wrote it to settings.json");
            check(&mut report, log_since(from).contains("hotkey captured in settings"), "Settings' capture box took the key from the hook");
            // And with Settings still in front, the new hotkey works at once.
            take();
            press(&combo(&[CTRL, ALT], F21));
            let (seen, _) = take();
            check(&mut report, log_since(from).contains("record hotkey pressed"), "with Voi's own window in front, the new hotkey still works");
            check(&mut report, !seen.iter().any(|(k, _)| *k == F21), "and is still swallowed");
            drop(child);
            std::thread::sleep(Duration::from_millis(500));
            take();

            let (child, from) = start(&voi, &data, &[])?;
            let pid = child.0.id();
            let log = log_since(from);
            check(&mut report, log.contains("hotkey=Ctrl+Alt+F21"), "after a restart, the settings loaded say Ctrl+Alt+F21");
            take();
            press(&combo(&[CTRL, ALT], F21));
            let (seen, hotkeys) = take();
            let log = log_since(from);
            let _ = writeln!(report, "step 3 probe saw {seen:x?}, WM_HOTKEY {hotkeys}");
            check(&mut report, log.matches("record hotkey pressed").count() == 1, "and Ctrl+Alt+F21 is now the hotkey");
            check(&mut report, !seen.iter().any(|(k, _)| *k == F21) && hotkeys == 0, "swallowed: no hook and no registration saw F21");
            check(&mut report, seen.contains(&(MASK, true)) && seen.contains(&(MASK, false)), "the mask key was tapped, so the held Alt is not a lone Alt");
            check(&mut report, !foreground_is(pid), "Voi never took the foreground");
            // The old one is free again.
            take();
            press(&combo(&[CTRL, SHIFT], F23));
            let (seen, _) = take();
            check(&mut report, seen.contains(&(F23, true)), "the previous hotkey is released once replaced");
            drop(child);
        }

        unsafe { PostThreadMessageW(thread, WM_QUIT, 0, 0) };
        let _ = writeln!(report, "{}", if ok { "ALL PASS" } else { "SOME FAILED" });
        std::fs::write(&out, &report)?;
        print!("{report}");
        std::process::exit(if ok { 0 } else { 1 });
    }
}
