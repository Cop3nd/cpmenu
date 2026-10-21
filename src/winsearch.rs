//! File search through the Windows Search index (`SystemIndex`), used when Everything isn't running.
//!
//! The query runs on a worker thread through the `Search.CollatorDSO` OLE DB provider and the result
//! is posted back to the menu window, so the UI thread never waits for the indexer.

use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Search::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{GUID, HSTRING, IUnknown, Interface, Result, w};

use crate::everything::Hit;

/// Posted to the menu window with a `Box<Reply>` as `lparam`; the window owns it from then on.
pub const WM_FILES_FOUND: u32 = WM_APP + 3;

pub struct Reply {
    pub query: String,
    pub hits: Vec<Hit>,
}

/// Number of the newest query; a worker that finds itself superseded stops without posting.
static LATEST: AtomicU64 = AtomicU64::new(0);

/// `DBGUID_DEFAULT`: the provider's native SQL dialect.
const DIALECT: GUID = GUID::from_u128(0xc8b521fb_5cf3_11ce_ade5_00aa0044773d);
/// `System.ItemType` of a folder.
const FOLDER_TYPE: &str = "Directory";

/// Searches the index for file names containing `query` in the background; the hits arrive later
/// as `WM_FILES_FOUND`. Posts nothing if Windows Search is off, fails or finds nothing.
pub fn query(reply_to: HWND, query: &str, max_results: u32) {
    let id = LATEST.fetch_add(1, Ordering::Relaxed) + 1;
    let (window, text) = (reply_to.0 as usize, query.to_owned());
    let _ = std::thread::Builder::new().name("winsearch".into()).spawn(move || {
        if LATEST.load(Ordering::Relaxed) != id {
            return;
        }
        let hits = unsafe { search(&text, max_results) }.unwrap_or_default();
        if hits.is_empty() || LATEST.load(Ordering::Relaxed) != id {
            return;
        }
        let reply = Box::into_raw(Box::new(Reply { query: text, hits }));
        let posted = unsafe { PostMessageW(Some(HWND(window as _)), WM_FILES_FOUND, WPARAM(0), LPARAM(reply as isize)) };
        if posted.is_err() {
            drop(unsafe { Box::from_raw(reply) });
        }
    });
}

/// The `SELECT` for file names containing `text`, newest first. `text` is escaped for a SQL string.
fn sql(text: &str, max_results: u32) -> String {
    let escaped = text.replace('\'', "''");
    format!(
        "SELECT TOP {max_results} System.ItemPathDisplay, System.ItemType FROM SystemIndex \
         WHERE SCOPE='file:' AND System.ItemNameDisplay LIKE '%{escaped}%' ORDER BY System.DateModified DESC"
    )
}

/// A result row as name, directory and folder flag; `None` for a path without a directory part.
fn hit(path: &str, item_type: &str) -> Option<Hit> {
    let (dir, name) = path.trim_end_matches('\\').rsplit_once('\\')?;
    (!name.is_empty()).then(|| Hit { name: name.to_owned(), path: dir.to_owned(), folder: item_type == FOLDER_TYPE })
}

/// One row as the provider hands it over: a status, a length in bytes and a pointer to the
/// provider's own UTF-16 text for each of the two selected columns.
#[repr(C)]
struct Row {
    status: [u32; COLUMNS],
    length: [usize; COLUMNS],
    text: [*const u16; COLUMNS],
}

const COLUMNS: usize = 2;
const STATUS_OK: u32 = 0;

fn binding(column: usize) -> DBBINDING {
    DBBINDING {
        iOrdinal: column + 1,
        obStatus: std::mem::offset_of!(Row, status) + column * size_of::<u32>(),
        obLength: std::mem::offset_of!(Row, length) + column * size_of::<usize>(),
        obValue: std::mem::offset_of!(Row, text) + column * size_of::<*const u16>(),
        dwPart: (DBPART_VALUE.0 | DBPART_LENGTH.0 | DBPART_STATUS.0) as u32,
        dwMemOwner: DBMEMOWNER_PROVIDEROWNED.0 as u32,
        wType: (DBTYPE_WSTR.0 | DBTYPE_BYREF.0) as u16,
        ..Default::default()
    }
}

unsafe fn search(text: &str, max_results: u32) -> Result<Vec<Hit>> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let hits = run(text, max_results);
        CoUninitialize();
        hits
    }
}

unsafe fn run(text: &str, max_results: u32) -> Result<Vec<Hit>> {
    unsafe {
        let source: IDBInitialize = CoCreateInstance(&CLSIDFromProgID(w!("Search.CollatorDSO"))?, None, CLSCTX_INPROC_SERVER)?;
        source.Initialize()?;
        let session: IDBCreateCommand = source.cast::<IDBCreateSession>()?.CreateSession(None, &IDBCreateCommand::IID)?.cast()?;
        let command: ICommandText = session.CreateCommand(None, &ICommandText::IID)?.cast()?;
        command.SetCommandText(&DIALECT, &HSTRING::from(sql(text, max_results)))?;
        let mut rowset: Option<IUnknown> = None;
        command.cast::<ICommand>()?.Execute(None, &IRowset::IID, None, None, Some(&mut rowset))?;
        let rowset: IRowset = rowset.ok_or_else(windows::core::Error::empty)?.cast()?;

        let accessor: IAccessor = rowset.cast()?;
        let bindings = [binding(0), binding(1)];
        let mut handle = HACCESSOR(0);
        accessor.CreateAccessor(DBACCESSOR_ROWDATA.0 as u32, COLUMNS, bindings.as_ptr(), size_of::<Row>(), &mut handle, None)?;

        let mut hits = Vec::new();
        loop {
            let (mut obtained, mut rows) = (0usize, [std::ptr::null_mut::<usize>(); 1]);
            rowset.GetNextRows(0, 0, &mut obtained, &mut rows)?;
            if obtained == 0 {
                break;
            }
            let row_handle = rows[0] as usize;
            let mut row: Row = std::mem::zeroed();
            let got = rowset.GetData(row_handle, handle, &mut row as *mut Row as *mut _);
            if got.is_ok() {
                let read = |column: usize| match row.text[column] {
                    p if row.status[column] == STATUS_OK && !p.is_null() => {
                        String::from_utf16_lossy(std::slice::from_raw_parts(p, row.length[column] / 2))
                    }
                    _ => String::new(),
                };
                hits.extend(hit(&read(0), &read(1)));
            }
            rowset.ReleaseRows(1, &row_handle, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut())?;
        }
        let _ = accessor.ReleaseAccessor(handle, None);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_asks_for_names_containing_the_text() {
        let q = sql("plan", 12);
        assert!(q.starts_with("SELECT TOP 12 System.ItemPathDisplay, System.ItemType FROM SystemIndex"));
        assert!(q.contains("System.ItemNameDisplay LIKE '%plan%'") && q.contains("SCOPE='file:'"));
        assert!(q.ends_with("ORDER BY System.DateModified DESC"));
    }

    #[test]
    fn quotes_in_the_query_cannot_end_the_string() {
        assert!(sql("o'brien", 5).contains("LIKE '%o''brien%'"));
        assert!(sql("'; DROP", 5).contains("'%''; DROP%'"));
    }

    #[test]
    fn rows_become_name_directory_and_folder_flag() {
        let file = hit(r"C:\Users\user\plan.md", "Markdown").unwrap();
        assert_eq!((file.name.as_str(), file.path.as_str(), file.folder), ("plan.md", r"C:\Users\user", false));
        assert_eq!(file.full_path(), r"C:\Users\user\plan.md");
        let folder = hit(r"D:\Zdjęcia\", FOLDER_TYPE).unwrap();
        assert_eq!((folder.name.as_str(), folder.path.as_str(), folder.folder), ("Zdjęcia", "D:", true));
        assert!(hit("no-separator", "x").is_none());
    }

    #[test]
    fn row_layout_matches_the_bindings() {
        let b = binding(1);
        assert_eq!(b.iOrdinal, 2);
        assert_eq!(b.obStatus, std::mem::offset_of!(Row, status) + 4);
        assert_eq!(b.obLength, std::mem::offset_of!(Row, length) + size_of::<usize>());
        assert_eq!(b.obValue, std::mem::offset_of!(Row, text) + size_of::<usize>());
        assert!(b.obValue + size_of::<usize>() <= size_of::<Row>());
    }
}
