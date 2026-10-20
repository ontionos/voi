//! Noticing when something else on the computer starts listening.
//!
//! Core Audio exposes one object per process that touches audio, and each of
//! those can say whether it is currently taking input. Polling that list is
//! enough to tell that a call is starting — Zoom, Meet in a browser, a Telegram
//! call, anything — without asking the user, watching windows, or knowing which
//! apps exist.
//!
//! Microphone-in-use is the right signal, not audio playing: music and video
//! produce output constantly and mean nothing.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;

    use block2::RcBlock;
    use objc2_core_audio::{
        kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDevices,
        AudioObjectAddPropertyListenerBlock,
        kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyElementMain,
        kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioProcessPropertyBundleID, kAudioProcessPropertyPID,
        kAudioProcessPropertyIsRunningInput, AudioObjectGetPropertyData,
        AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
        AudioObjectPropertySelector,
    };

    fn address(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        }
    }

    unsafe fn get<T: Copy>(id: AudioObjectID, selector: AudioObjectPropertySelector) -> Option<T> {
        let addr = address(selector);
        let mut size = std::mem::size_of::<T>() as u32;
        let mut out = std::mem::MaybeUninit::<T>::uninit();
        let status = AudioObjectGetPropertyData(
            id,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(out.as_mut_ptr() as *mut c_void)?,
        );
        (status == 0).then(|| out.assume_init())
    }

    /// Every process Core Audio knows about.
    unsafe fn process_objects() -> Vec<AudioObjectID> {
        let addr = address(kAudioHardwarePropertyProcessObjectList);
        let system = kAudioObjectSystemObject as AudioObjectID;

        let mut size = 0u32;
        if AudioObjectGetPropertyDataSize(
            system,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        ) != 0
        {
            return Vec::new();
        }

        let count = size as usize / std::mem::size_of::<AudioObjectID>();
        let mut ids = vec![0 as AudioObjectID; count];
        let Some(data) = NonNull::new(ids.as_mut_ptr() as *mut c_void) else {
            return Vec::new();
        };
        if AudioObjectGetPropertyData(
            system,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            data,
        ) != 0
        {
            return Vec::new();
        }
        ids
    }

    /// Every process taking microphone input, as `(pid, bundle id)`.
    ///
    /// Exposed for diagnosis: "something is using the microphone" is a claim
    /// worth being able to check rather than believe.
    pub fn listeners() -> Vec<(i32, String)> {
        unsafe {
            process_objects()
                .into_iter()
                .filter(|id| get::<u32>(*id, kAudioProcessPropertyIsRunningInput) == Some(1))
                .map(|id| {
                    let pid = get::<i32>(id, kAudioProcessPropertyPID).unwrap_or(-1);
                    let bundle = cf_string(id, kAudioProcessPropertyBundleID)
                        .unwrap_or_else(|| "-".to_string());
                    (pid, bundle)
                })
                .collect()
        }
    }

    unsafe fn cf_string(
        id: AudioObjectID,
        selector: AudioObjectPropertySelector,
    ) -> Option<String> {
        let raw: *const objc2_core_foundation::CFString = get(id, selector)?;
        NonNull::new(raw as *mut objc2_core_foundation::CFString)
            .map(|s| s.as_ref().to_string())
    }

    /// Set by Core Audio whenever a device comes or goes or the default
    /// input changes; taken by [`inputs_changed`].
    static INPUTS_CHANGED: AtomicBool = AtomicBool::new(false);
    static LISTENING: Once = Once::new();

    /// Whether the microphones, or which of them is the system's default,
    /// have changed since the last time this was asked.
    ///
    /// Told by Core Audio rather than polled: the first call registers for
    /// both notifications, for the life of the process. The block goes in
    /// without a dispatch queue, so Core Audio calls it straight from its own
    /// thread and nothing depends on whose run loop is turning.
    pub fn inputs_changed() -> bool {
        LISTENING.call_once(|| {
            let block = RcBlock::new(|_: u32, _: NonNull<AudioObjectPropertyAddress>| {
                INPUTS_CHANGED.store(true, Ordering::SeqCst);
            });
            for selector in [kAudioHardwarePropertyDevices, kAudioHardwarePropertyDefaultInputDevice] {
                let addr = address(selector);
                let status = unsafe {
                    AudioObjectAddPropertyListenerBlock(
                        kAudioObjectSystemObject as AudioObjectID,
                        NonNull::from(&addr),
                        None,
                        RcBlock::as_ptr(&block),
                    )
                };
                if status != 0 {
                    tracing::warn!(selector, status, "could not watch the microphones for changes");
                }
            }
            // Core Audio copied the block; ours can go.
        });
        INPUTS_CHANGED.swap(false, Ordering::SeqCst)
    }

    /// Whether this process is taking input, as Core Audio sees it — not as
    /// our own bookkeeping believes.
    pub fn we_are_listening() -> bool {
        let me = std::process::id() as i32;
        unsafe {
            process_objects().into_iter().any(|id| {
                get::<i32>(id, kAudioProcessPropertyPID) == Some(me)
                    && get::<u32>(id, kAudioProcessPropertyIsRunningInput) == Some(1)
            })
        }
    }

    /// Whether any process other than this one is taking microphone input.
    pub fn someone_else_is_listening() -> bool {
        let me = std::process::id() as i32;
        unsafe {
            process_objects().into_iter().any(|id| {
                let running: u32 = get(id, kAudioProcessPropertyIsRunningInput).unwrap_or(0);
                if running == 0 {
                    return false;
                }
                // Our own capture must not count, or the overlay would keep
                // itself on screen for as long as it was recording.
                get::<i32>(id, kAudioProcessPropertyPID).is_none_or(|pid| pid != me)
            })
        }
    }
}

/// Windows has no per-process audio object to ask, but it keeps the answer
/// for its own privacy indicator — the microphone icon in the taskbar — in the
/// registry: one key per app that has used the microphone, under
/// `CapabilityAccessManager\ConsentStore\microphone`, with the time it last
/// started and stopped. An app whose stop time is zero is listening now.
/// Desktop programs sit under `NonPackaged`, named by their path with `#` for
/// `\`; Store apps sit directly under the store, named by package.
#[cfg(windows)]
mod imp {
    use std::collections::HashSet;

    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const STORE: &str =
        r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

    fn listening(key: &RegKey) -> bool {
        let start: u64 = key.get_value("LastUsedTimeStart").unwrap_or(0);
        let stop: u64 = key.get_value("LastUsedTimeStop").unwrap_or(1);
        start != 0 && stop == 0
    }

    /// This executable, the way the store names it.
    fn me() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        Some(exe.to_string_lossy().replace('\\', "#").to_lowercase())
    }

    /// Every running program's executable, lower-cased.
    fn running_programs() -> HashSet<String> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::ProcessStatus::EnumProcesses;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let mut pids = vec![0u32; 4096];
        let mut bytes = 0u32;
        let ok = unsafe { EnumProcesses(pids.as_mut_ptr(), (pids.len() * 4) as u32, &mut bytes) };
        if ok == 0 {
            return HashSet::new();
        }
        pids.truncate(bytes as usize / 4);
        let mut out = HashSet::new();
        for pid in pids.into_iter().filter(|p| *p != 0) {
            unsafe {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if process.is_null() {
                    continue;
                }
                let mut buf = [0u16; 1024];
                let mut len = buf.len() as u32;
                if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0 {
                    out.insert(String::from_utf16_lossy(&buf[..len as usize]).to_lowercase());
                }
                CloseHandle(process);
            }
        }
        out
    }

    /// Every app listening now, as `(0, name)`: the registry knows paths and
    /// packages, not process ids. Ourselves included.
    ///
    /// A desktop program counts only while it runs. Windows marks a program
    /// as using the microphone when it starts and clears that when it stops;
    /// one that ends without stopping — killed, crashed, an old version an
    /// update replaced — stays marked for good, and the "Record?" prompt
    /// hung on screen for programs that were not there. The Mac asks the
    /// audio system what is capturing now; this is the nearest Windows has.
    fn all() -> Vec<String> {
        let mut out = Vec::new();
        let Ok(store) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(STORE) else { return out };
        let mut running: Option<HashSet<String>> = None;
        for name in store.enum_keys().flatten() {
            let Ok(key) = store.open_subkey(&name) else { continue };
            if name == "NonPackaged" {
                for app in key.enum_keys().flatten() {
                    if key.open_subkey(&app).is_ok_and(|k| listening(&k)) {
                        let running = running.get_or_insert_with(running_programs);
                        if running.contains(&exe_of(&app)) {
                            out.push(app);
                        }
                    }
                }
            } else if listening(&key) {
                out.push(name);
            }
        }
        out
    }

    /// The executable a NonPackaged entry names: its path with `#` for `\`.
    fn exe_of(entry: &str) -> String {
        entry.replace('#', "\\").to_lowercase()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn an_entry_names_its_program_as_running_programs_are_named() {
            assert_eq!(super::exe_of(r"C:#Users#Me#Downloads#Tool.EXE"), r"c:\users\me\downloads\tool.exe");
        }

        #[test]
        fn this_test_is_a_running_program() {
            let me = std::env::current_exe().unwrap().to_string_lossy().to_lowercase();
            assert!(super::running_programs().contains(&me));
        }
    }

    pub fn listeners() -> Vec<(i32, String)> {
        let me = me();
        all()
            .into_iter()
            .filter(|app| Some(app.to_lowercase()) != me)
            .map(|app| (0, app.replace('#', "\\")))
            .collect()
    }

    pub fn someone_else_is_listening() -> bool {
        !listeners().is_empty()
    }

    pub fn we_are_listening() -> bool {
        let me = me();
        all().into_iter().any(|app| Some(app.to_lowercase()) == me)
    }

    /// Not watched: Settings lists the microphones afresh whenever it opens.
    pub fn inputs_changed() -> bool {
        false
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    pub fn someone_else_is_listening() -> bool {
        false
    }

    pub fn listeners() -> Vec<(i32, String)> {
        Vec::new()
    }

    pub fn we_are_listening() -> bool {
        false
    }

    pub fn inputs_changed() -> bool {
        false
    }
}

pub use imp::{inputs_changed, listeners, someone_else_is_listening, we_are_listening};
