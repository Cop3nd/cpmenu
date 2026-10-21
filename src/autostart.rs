//! The AUTOSTART setting: the `cpmenu` value under HKCU Run, the same one the installer writes. It is
//! kept in the registry rather than config.toml, so the settings panel reads it on open and writes
//! it on each toggle.

use std::io;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};
use windows::core::w;

/// Whether the Run value exists, whatever exe it names: a copy elsewhere still counts as on.
pub fn is_enabled() -> bool {
    Key::open(KEY_READ.0).ok().and_then(|key| key.read()).is_some()
}

/// Writes the Run value for the running exe (`on`), or deletes it. Writing is skipped when the
/// value already names this exe.
pub fn set(on: bool) -> io::Result<()> {
    set_exe(&std::env::current_exe()?, on)
}

/// Writes the Run value for `exe` (`on`), or deletes it. Writing is skipped when the value already
/// names `exe`. The installer sets it for the copy it puts under `%LOCALAPPDATA%`.
pub fn set_exe(exe: &Path, on: bool) -> io::Result<()> {
    let key = Key::open(KEY_READ.0 | KEY_SET_VALUE.0)?;
    if !on {
        let code = unsafe { RegDeleteValueW(key.0, w!("cpmenu")) };
        // Already off is what was asked for.
        return if code == ERROR_FILE_NOT_FOUND { Ok(()) } else { check(code) };
    }
    if key.read().is_some_and(|stored| names(&stored, exe)) {
        return Ok(());
    }
    let data: Vec<u8> = run_value(&exe).encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
    check(unsafe { RegSetValueExW(key.0, w!("cpmenu"), None, REG_SZ, Some(data.as_slice())) })
}

/// The Run value for `exe`: the path in quotes, as the installer writes it.
fn run_value(exe: &Path) -> String {
    format!("\"{}\"", exe.to_string_lossy())
}

/// Whether a stored Run value names `exe`, ignoring the quotes and letter case.
fn names(stored: &str, exe: &Path) -> bool {
    stored.trim().trim_matches('"').eq_ignore_ascii_case(&exe.to_string_lossy())
}

fn check(code: WIN32_ERROR) -> io::Result<()> {
    if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code.0 as i32)) }
}

/// An open HKCU Run key, closed on drop.
struct Key(HKEY);

impl Key {
    fn open(access: u32) -> io::Result<Key> {
        let mut key = HKEY(std::ptr::null_mut());
        check(unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Run"), None, REG_SAM_FLAGS(access), &mut key) })?;
        Ok(Key(key))
    }

    /// The `cpmenu` value's text, or `None` when it is missing or unreadable.
    fn read(&self) -> Option<String> {
        let mut size = 0u32;
        if unsafe { RegQueryValueExW(self.0, w!("cpmenu"), None, None, None, Some(&mut size as *mut u32)) } != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let mut len = size;
        if unsafe { RegQueryValueExW(self.0, w!("cpmenu"), None, None, Some(buf.as_mut_ptr()), Some(&mut len as *mut u32)) } != ERROR_SUCCESS {
            return None;
        }
        let wide: Vec<u16> = buf[..(len as usize).min(buf.len())].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        Some(String::from_utf16_lossy(&wide).trim_end_matches('\0').to_owned())
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{names, run_value};

    #[test]
    fn the_run_value_is_the_exe_path_in_quotes() {
        assert_eq!(run_value(Path::new(r"C:\Users\me\AppData\Local\cpmenu\cpmenu.exe")), r#""C:\Users\me\AppData\Local\cpmenu\cpmenu.exe""#);
    }

    #[test]
    fn a_stored_value_names_the_exe_whatever_its_quotes_or_case() {
        let exe = Path::new(r"C:\Apps\cpmenu.exe");
        assert!(names(r#""C:\Apps\cpmenu.exe""#, exe));
        assert!(names(r#""c:\apps\CPMENU.EXE""#, exe));
        assert!(names(r"C:\Apps\cpmenu.exe", exe), "unquoted, as typed by hand");
        assert!(!names(r#""D:\Other\cpmenu.exe""#, exe));
        assert!(!names(r#""C:\Apps\cpmenu.exe.bak""#, exe));
    }
}
