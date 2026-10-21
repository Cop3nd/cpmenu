//! `cpmenu.exe install [exe]`: puts an exe (default: the one running) at
//! `%LOCALAPPDATA%\cpmenu\cpmenu.exe`, makes that copy the autostart entry and starts it.
//! `cpmenu-setup.exe` calls it with the exe it carries.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::autostart;

/// How often `replace` retries while the quitting copy still holds its file.
const ATTEMPTS: usize = 100;

/// Installs `source` over `%LOCALAPPDATA%\cpmenu\cpmenu.exe` and starts that copy.
pub fn run(source: &Path) -> io::Result<PathBuf> {
    let dir = install_dir()?;
    let dest = exe_in(&dir);
    std::fs::create_dir_all(&dir)?;
    if !same_exe(source, &dest) {
        quit_running(&dest);
        replace(source, &dest)?;
    }
    autostart::set_exe(&dest, true)?;
    Command::new(&dest).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    Ok(dest)
}

fn install_dir() -> io::Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("cpmenu"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))
}

fn exe_in(dir: &Path) -> PathBuf {
    dir.join("cpmenu.exe")
}

/// Whether `source` is already the installed copy, so there is nothing to quit or replace.
fn same_exe(source: &Path, dest: &Path) -> bool {
    match (std::fs::canonicalize(source), std::fs::canonicalize(dest)) {
        (Ok(a), Ok(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
        _ => false,
    }
}

/// Asks the copy at `dest` to quit: it may be running and holding its own file.
fn quit_running(dest: &Path) {
    if !dest.exists() {
        return;
    }
    if let Ok(mut child) = Command::new(dest).arg("quit").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        let _ = child.wait();
    }
}

/// Copies over the installed exe, retrying while the quitting copy releases the file.
fn replace(source: &Path, dest: &Path) -> io::Result<()> {
    let mut error = None;
    for _ in 0..ATTEMPTS {
        match std::fs::copy(source, dest) {
            Ok(_) => return Ok(()),
            Err(err) => error = Some(err),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(error.unwrap_or_else(|| io::Error::other("the installed exe could not be replaced")))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{exe_in, same_exe};

    #[test]
    fn the_installed_exe_lives_under_locallappdata() {
        assert_eq!(exe_in(Path::new(r"C:\Users\me\AppData\Local\cpmenu")), Path::new(r"C:\Users\me\AppData\Local\cpmenu\cpmenu.exe"));
    }

    #[test]
    fn the_installed_copy_is_recognised_through_a_dot_segment_or_another_case() {
        let dir = std::env::temp_dir().join(format!("cpmenu-install-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = exe_in(&dir);
        std::fs::write(&exe, b"MZ").unwrap();
        assert!(same_exe(&exe, &dir.join(".").join("cpmenu.exe")));
        assert!(same_exe(&exe, &dir.join("CPMENU.EXE")));
        assert!(!same_exe(&exe, &dir.join("other.exe")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_destination_is_not_the_source() {
        assert!(!same_exe(Path::new(r"C:\nope\cpmenu.exe"), Path::new(r"C:\nope\cpmenu.exe")));
    }
}
