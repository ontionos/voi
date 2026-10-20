//! The Windows answer to `chmod 600`: a file only its user (and the system)
//! can open.
//!
//! A file in `%APPDATA%` inherits the profile's permissions, which already
//! keep other users out — but they also let in anything granted to the
//! profile folder, and an inherited entry can be widened later by somebody
//! else's change to the folder. The key file gets its own, protected list:
//! the user who wrote it and SYSTEM, nothing inherited.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use anyhow::{bail, Result};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SetFileSecurityW, TokenUser, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// The current user's SID, as `S-1-5-21-…`.
fn user_sid() -> Result<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            bail!("could not open the process token");
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
        CloseHandle(token);
        if ok == 0 {
            bail!("could not read the token's user");
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            bail!("could not format the user's SID");
        }
        let n = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text.cast());
        Ok(sid)
    }
}

/// Replace `path`'s permissions with full control for this user and SYSTEM.
pub fn owner_only(path: &Path) -> Result<()> {
    let sddl = format!("D:P(A;;FA;;;{})(A;;FA;;;SY)", user_sid()?);
    unsafe {
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let sddl = wide(std::ffi::OsStr::new(&sddl));
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        ) == 0
        {
            bail!("could not build the key file's permissions");
        }
        let file = wide(path.as_os_str());
        let ok = SetFileSecurityW(
            file.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            sd,
        );
        LocalFree(sd.cast());
        if ok == 0 {
            bail!("could not restrict {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Asked of `icacls`, the tool anyone would use to check by hand: two
    /// entries, the user and SYSTEM, and nothing inherited.
    #[test]
    fn the_key_file_is_the_users_alone() {
        let path = std::env::temp_dir().join(format!("voi-acl-{}.txt", std::process::id()));
        std::fs::write(&path, "not a key").unwrap();
        super::owner_only(&path).unwrap();
        let out = std::process::Command::new("icacls").arg(&path).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        std::fs::remove_file(&path).ok();
        let entries: Vec<&str> = text.lines().filter(|l| l.contains(":(")).collect();
        assert_eq!(entries.len(), 2, "exactly the user and SYSTEM:\n{text}");
        assert!(text.contains("SYSTEM"), "{text}");
        assert!(!text.contains("(I)"), "nothing inherited:\n{text}");
        let user = std::env::var("USERNAME").unwrap();
        assert!(text.to_lowercase().contains(&user.to_lowercase()), "{text}");
    }
}
