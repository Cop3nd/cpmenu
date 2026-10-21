//! cpmenu-setup.exe: the download that installs cpmenu. The release build appends cpmenu.exe after
//! this stub; running it writes that payload to a temp file and lets it run `install <temp>`, the
//! installer's steps, without PowerShell.

#![windows_subsystem = "windows"]

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};

use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::{HSTRING, w};

/// The last 16 bytes are this magic, then the payload's length as a little-endian u64.
const MAGIC: &[u8; 8] = b"CPMSETUP";
const TRAILER: u64 = 16;

fn main() {
    if let Err(err) = install() {
        message(&format!("cpmenu could not be installed.\n\n{err}"));
        std::process::exit(1);
    }
}

/// Writes the carried cpmenu.exe to a temp file and installs it.
fn install() -> io::Result<()> {
    let mut exe = File::open(std::env::current_exe()?)?;
    let payload = payload(&mut exe)?;
    let temp = std::env::temp_dir().join(format!("cpmenu-setup-{}.exe", std::process::id()));
    std::fs::write(&temp, payload)?;
    let result = run(&temp);
    let _ = std::fs::remove_file(&temp);
    result
}

/// Lets the carried copy install itself, as `cpmenu.exe install <itself>`. Its stderr goes to a file,
/// not a pipe: the menu it starts inherits the installer's handles, and a pipe would keep this setup
/// waiting until that menu quits.
fn run(temp: &Path) -> io::Result<()> {
    let log = std::env::temp_dir().join(format!("cpmenu-setup-{}.log", std::process::id()));
    let status = Command::new(temp)
        .arg("install")
        .arg(temp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&log)?)
        .status()?;
    let detail = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    if status.success() {
        return Ok(());
    }
    let detail = detail.trim();
    Err(io::Error::other(if detail.is_empty() { format!("the installer exited with {status}") } else { detail.to_owned() }))
}

/// The exe appended after the stub, read back from the trailer.
fn payload(file: &mut (impl Read + Seek)) -> io::Result<Vec<u8>> {
    let end = file.seek(SeekFrom::End(0))?;
    if end < TRAILER {
        return Err(no_payload());
    }
    file.seek(SeekFrom::End(-(TRAILER as i64)))?;
    let mut trailer = [0u8; 16];
    file.read_exact(&mut trailer)?;
    if trailer[..8] != *MAGIC {
        return Err(no_payload());
    }
    let len = u64::from_le_bytes(trailer[8..].try_into().unwrap());
    if len > end - TRAILER {
        return Err(no_payload());
    }
    file.seek(SeekFrom::Start(end - TRAILER - len))?;
    let mut payload = vec![0u8; len as usize];
    file.read_exact(&mut payload)?;
    if !payload.starts_with(b"MZ") {
        return Err(no_payload());
    }
    Ok(payload)
}

fn no_payload() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "this cpmenu-setup.exe does not carry cpmenu: it is a plain cargo build, not the one the release zip carries",
    )
}

fn message(text: &str) {
    unsafe { MessageBoxW(None, &HSTRING::from(text), w!("cpmenu setup"), MB_OK | MB_ICONERROR) };
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{MAGIC, payload};

    /// A stub with `exe` appended, the way the release zip builds the real one.
    fn packed(exe: &[u8]) -> Vec<u8> {
        let mut bytes = b"stub".to_vec();
        bytes.extend_from_slice(exe);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(exe.len() as u64).to_le_bytes());
        bytes
    }

    #[test]
    fn reads_the_carried_exe_from_the_trailer() {
        assert_eq!(payload(&mut Cursor::new(packed(b"MZ payload"))).unwrap(), b"MZ payload");
    }

    #[test]
    fn a_stub_without_the_trailer_carries_nothing() {
        assert!(payload(&mut Cursor::new(b"MZ stub".to_vec())).is_err());
        let mut bytes = packed(b"MZ payload");
        let magic = bytes.len() - 16;
        bytes[magic] = b'X';
        assert!(payload(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn a_length_past_the_file_is_rejected() {
        let mut bytes = packed(b"MZ payload");
        let len = bytes.len() - 8;
        bytes[len..].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(payload(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn a_payload_that_is_not_an_exe_is_rejected() {
        assert!(payload(&mut Cursor::new(packed(b"nope"))).is_err());
    }
}
