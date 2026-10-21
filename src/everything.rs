//! File search through Everything's IPC (QUERY2 over WM_COPYDATA), as defined in the SDK's
//! `everything_ipc.h`. No DLL, no index of our own: Everything does the search and replies
//! with a WM_COPYDATA to our window.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, FindWindowW, MSGFLT_ALLOW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::{PCWSTR, w};

const IPC_WINDOW: PCWSTR = w!("EVERYTHING_TASKBAR_NOTIFICATION");
const COPYDATA_QUERY2W: usize = 18;
const REQUEST_NAME: u32 = 0x1;
const REQUEST_PATH: u32 = 0x2;
const SORT_DATE_MODIFIED_DESCENDING: u32 = 14;
const ITEM_FOLDER: u32 = 0x1;
/// `dwData` Everything echoes back on the reply; lets us tell replies from other WM_COPYDATA.
pub const REPLY_ID: usize = 0x4350_4D45;
const SEND_TIMEOUT_MS: u32 = 300;

pub struct Hit {
    pub name: String,
    pub path: String,
    pub folder: bool,
}

impl Hit {
    pub fn full_path(&self) -> String {
        format!("{}\\{}", self.path, self.name)
    }
}

/// Lets Everything's reply reach this window even if it runs at a different integrity level.
pub fn allow_replies(hwnd: HWND) {
    unsafe {
        let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None);
    }
}

fn query_message(reply_to: HWND, query: &str, max_results: u32) -> Vec<u8> {
    let header = [reply_to.0 as usize as u32, REPLY_ID as u32, 0, 0, max_results, REQUEST_NAME | REQUEST_PATH, SORT_DATE_MODIFIED_DESCENDING];
    let mut out: Vec<u8> = header.iter().flat_map(|v| v.to_le_bytes()).collect();
    out.extend(query.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    out
}

/// Sends the query; the results arrive later as WM_COPYDATA with `dwData == REPLY_ID`.
/// Returns false, sending nothing, when Everything isn't running. A running but hung
/// Everything just never replies.
pub fn query(reply_to: HWND, query: &str, max_results: u32) -> bool {
    unsafe {
        let Ok(everything) = FindWindowW(IPC_WINDOW, PCWSTR::null()) else {
            return false;
        };
        let mut message = query_message(reply_to, query, max_results);
        let data = COPYDATASTRUCT {
            dwData: COPYDATA_QUERY2W,
            cbData: message.len() as u32,
            lpData: message.as_mut_ptr() as _,
        };
        // ABORTIFHUNG: a hung Everything must never freeze the menu (seen in practice).
        SendMessageTimeoutW(
            everything,
            WM_COPYDATA,
            WPARAM(reply_to.0 as usize),
            LPARAM(&data as *const _ as isize),
            SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MS,
            None,
        );
        true
    }
}

/// Parses an EVERYTHING_IPC_LIST2 reply requested with NAME | PATH.
pub fn parse_reply(bytes: &[u8]) -> Vec<Hit> {
    fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    }
    // DWORD length in characters (no terminator), then the UTF-16 text and a terminator.
    fn text_at(bytes: &[u8], at: usize) -> Option<(String, usize)> {
        let len = u32_at(bytes, at)? as usize;
        let start = at + 4;
        let raw = bytes.get(start..start + len * 2)?;
        let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some((String::from_utf16_lossy(&units), start + (len + 1) * 2))
    }

    const LIST_HEADER: usize = 20;
    let Some(count) = u32_at(bytes, 4) else { return Vec::new() };
    let Some(flags) = u32_at(bytes, 12) else { return Vec::new() };
    if flags & (REQUEST_NAME | REQUEST_PATH) != REQUEST_NAME | REQUEST_PATH {
        return Vec::new();
    }
    (0..count as usize)
        .map_while(|i| {
            let item = LIST_HEADER + i * 8;
            let item_flags = u32_at(bytes, item)?;
            let (name, next) = text_at(bytes, u32_at(bytes, item + 4)? as usize)?;
            let (path, _) = text_at(bytes, next)?;
            Some(Hit { name, path, folder: item_flags & ITEM_FOLDER != 0 })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Vec<u8> {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut out = (units.len() as u32).to_le_bytes().to_vec();
        out.extend(units.iter().chain([&0]).flat_map(|u| u.to_le_bytes()));
        out
    }

    fn reply(items: &[(u32, &str, &str)]) -> Vec<u8> {
        let mut data = Vec::new();
        let data_start = 20 + items.len() * 8;
        let mut table = Vec::new();
        for (flags, name, path) in items {
            table.extend(flags.to_le_bytes());
            table.extend(((data_start + data.len()) as u32).to_le_bytes());
            data.extend(text(name));
            data.extend(text(path));
        }
        let n = items.len() as u32;
        let mut out: Vec<u8> = [n, n, 0, REQUEST_NAME | REQUEST_PATH, SORT_DATE_MODIFIED_DESCENDING]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        out.extend(table);
        out.extend(data);
        out
    }

    #[test]
    fn parses_names_paths_and_folder_flag() {
        let hits = parse_reply(&reply(&[(0, "plan.md", "C:\\Code"), (ITEM_FOLDER, "Zdjęcia", "D:\\")]));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].full_path(), "C:\\Code\\plan.md");
        assert!(!hits[0].folder);
        assert_eq!(hits[1].name, "Zdjęcia");
        assert!(hits[1].folder);
    }

    #[test]
    fn truncated_reply_yields_what_is_complete() {
        let bytes = reply(&[(0, "a.txt", "C:\\"), (0, "b.txt", "C:\\")]);
        assert_eq!(parse_reply(&bytes[..bytes.len() - 3]).len(), 1);
        assert!(parse_reply(&bytes[..10]).is_empty());
    }

    #[test]
    fn query_message_layout_matches_sdk() {
        let msg = query_message(HWND(0x1234 as _), "ab", 12);
        assert_eq!(&msg[0..4], &0x1234u32.to_le_bytes());
        assert_eq!(&msg[16..20], &12u32.to_le_bytes());
        assert_eq!(&msg[20..24], &(REQUEST_NAME | REQUEST_PATH).to_le_bytes());
        assert_eq!(&msg[28..], &[b'a', 0, b'b', 0, 0, 0]);
    }
}
