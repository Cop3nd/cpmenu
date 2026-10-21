//! App index from the shell's AppsFolder: Win32 shortcuts and UWP/MSIX apps in one list,
//! the same set the system Start menu shows under "All apps".
//!
//! Scanning runs in a short-lived child process (`cpmenu.exe index`) that writes a cache file,
//! so the shell libraries and icon handlers it loads never stay in the resident menu process.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, HBITMAP,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, SW_SHOWNORMAL};
use windows::core::{GUID, HSTRING, Interface, PCWSTR, PWSTR, Result, w};

pub const ICON: usize = 32;
const ICON_BYTES: usize = ICON * ICON * 4;
const CACHE_MAGIC: &[u8; 4] = b"CPM2";
/// Version 1 had the same layout without settings; reading it keeps the cached icons across the upgrade.
const CACHE_MAGIC_V1: &[u8; 4] = b"CPM1";
const NO_ICON: u8 = 0;
const HAS_ICON: u8 = 1;
const IS_SETTING: u8 = 2;

/// Premultiplied BGRA, `ICON` x `ICON`, top-down.
pub struct Icon {
    pub color: Vec<u8>,
}

impl Icon {
    fn new(color: Vec<u8>) -> Self {
        Self { color }
    }
}

pub struct App {
    pub name: String,
    /// AppsFolder parsing name: an AUMID for packaged apps, a path-like id for shortcuts.
    /// For a Control Panel task (`setting`): its name in the All Tasks folder.
    pub id: String,
    pub icon: Option<Icon>,
    /// A Control Panel task rather than an app; has no icon.
    pub setting: bool,
}

pub fn launch(id: &str) {
    // Desktop shortcuts are indexed by their full path; Start Menu apps by their AppsFolder id.
    if Path::new(id).is_absolute() {
        return open_path(id);
    }
    let target = HSTRING::from(format!("shell:AppsFolder\\{id}"));
    unsafe { ShellExecuteW(None, w!("open"), &target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
}

/// Opens a file or folder with its default handler.
pub fn open_path(path: &str) {
    unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
}

/// Control Panel "All Tasks" (the "God Mode" folder). Its items can't be parsed back from a path,
/// so launching one means enumerating the folder again.
const ALL_TASKS: PCWSTR = w!("shell:::{ED7BA470-8E54-465E-825C-99712043E01C}");

/// Opens the Control Panel task `id` from a child process, so the shell libraries the enumeration
/// loads stay out of the resident menu (it takes ~150 ms).
pub fn launch_setting(id: &str) {
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(child) = std::process::Command::new(exe).arg("setting").arg(id).spawn() {
            // The child opens the dialog; without our foreground right it would land behind other windows.
            let _ = unsafe { AllowSetForegroundWindow(child.id()) };
        }
    }
}

/// Entry point of the `setting <id>` child process.
pub fn run_setting(id: &str) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let folder: IShellItem = SHCreateItemFromParsingName(ALL_TASKS, None)?;
        let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
        loop {
            let mut batch = [None];
            let mut fetched = 0;
            if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                return Ok(());
            }
            let Some(item) = batch[0].take() else { return Ok(()) };
            if item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING).map(|p| take_string(p)).ok().as_deref() != Some(id) {
                continue;
            }
            let pidl = SHGetIDListFromObject(&item)?;
            let mut info = SHELLEXECUTEINFOW {
                cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_IDLIST | SEE_MASK_NOASYNC,
                lpIDList: pidl as _,
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let result = ShellExecuteExW(&mut info);
            CoTaskMemFree(Some(pidl as _));
            return result;
        }
    }
}

const DESKTOPS: [GUID; 2] = [FOLDERID_Desktop, FOLDERID_PublicDesktop];

fn known_dirs(ids: &[GUID]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok())
        .map(|p| unsafe { take_string(p) })
        .collect()
}

/// Folders whose changes mean the app list may be stale: the Start Menu, and the desktops
/// (shortcuts there are indexed too).
pub fn watched_dirs() -> Vec<String> {
    known_dirs(&[FOLDERID_Programs, FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_PublicDesktop])
}

/// Shortcut files under `dir`, `depth` folder levels down (desktops often hold a folder of shortcuts).
fn shortcuts(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if depth > 0 {
                shortcuts(&path, depth - 1, out);
            }
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk") || e.eq_ignore_ascii_case("url")) {
            out.push(path);
        }
    }
}

/// Menu name for a shortcut file stem: drops the " - Shortcut" suffix Windows adds and a trailing ".exe".
fn shortcut_name(stem: &str) -> &str {
    let mut name = stem.trim();
    for dash in [" - ", " – ", " — "] {
        if let Some((head, tail)) = name.rsplit_once(dash) {
            let tail = tail.trim().to_lowercase();
            if tail == "skrót" || tail == "shortcut" {
                name = head.trim();
            }
        }
    }
    match name.len().checked_sub(4) {
        Some(end) if name.is_char_boundary(end) && name[end..].eq_ignore_ascii_case(".exe") => &name[..end],
        _ => name,
    }
}

fn cache_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("cpmenu").join("apps.bin"))
}

/// Last index written by the indexer; empty if there is none yet or it is unreadable.
pub fn load_cache() -> Vec<App> {
    cache_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| decode(&b)).unwrap_or_default()
}

/// Entry point of the `index` child process.
pub fn run_indexer() -> Result<()> {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    let mut known: HashMap<String, Icon> =
        load_cache().into_iter().filter_map(|app| Some((app.id, app.icon?))).collect();
    let apps = scan(&mut known)?;
    if let Some(path) = cache_path() {
        // Write-then-rename so the menu never reads a half-written file.
        let temp = path.with_extension("tmp");
        let written = path.parent().map(std::fs::create_dir_all).transpose().is_ok()
            && std::fs::write(&temp, encode(&apps)).is_ok();
        if written {
            let _ = std::fs::rename(&temp, &path);
        }
    }
    Ok(())
}

/// Icons are only extracted for ids not in `known` (~16 ms each, the bulk of a cold scan).
fn scan(known: &mut HashMap<String, Icon>) -> Result<Vec<App>> {
    unsafe {
        let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
        let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
        let mut apps = Vec::new();
        loop {
            let mut batch = [None];
            let mut fetched = 0;
            if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = batch[0].take() else { break };
            let (Ok(name), Ok(id)) =
                (item.GetDisplayName(SIGDN_NORMALDISPLAY), item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING))
            else {
                continue;
            };
            let (name, id) = (take_string(name), take_string(id));
            if name.is_empty() || id.is_empty() {
                continue;
            }
            let icon = known.remove(&id).or_else(|| extract_icon(&item));
            apps.push(App { name, id, icon, setting: false });
        }
        // Desktop shortcuts that aren't in the Start Menu (games, portable apps). A name already
        // listed wins, so a shortcut to a Start Menu app doesn't show twice.
        let mut seen: HashSet<String> = apps.iter().map(|a| a.name.to_lowercase()).collect();
        let mut found = Vec::new();
        for dir in known_dirs(&DESKTOPS) {
            shortcuts(Path::new(&dir), 2, &mut found);
        }
        for path in found {
            let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(shortcut_name) else { continue };
            if name.is_empty() || !seen.insert(name.to_lowercase()) {
                continue;
            }
            let id = path.to_string_lossy().into_owned();
            let icon = known.remove(&id).or_else(|| {
                let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(&id), None).ok()?;
                extract_icon(&item)
            });
            apps.push(App { name: name.to_owned(), id, icon, setting: false });
        }
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps.extend(settings());
        Ok(apps)
    }
}

/// Control Panel tasks with their localized names ("Edytuj zmienne środowiskowe systemu"),
/// alphabetical. Empty if the folder can't be enumerated.
fn settings() -> Vec<App> {
    let mut found: Vec<App> = Vec::new();
    unsafe {
        let folder: Result<IShellItem> = SHCreateItemFromParsingName(ALL_TASKS, None);
        let Ok(folder) = folder else { return found };
        let items: Result<IEnumShellItems> = folder.BindToHandler(None, &BHID_EnumItems);
        let Ok(items) = items else { return found };
        loop {
            let mut batch = [None];
            let mut fetched = 0;
            if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = batch[0].take() else { break };
            let (Ok(name), Ok(id)) =
                (item.GetDisplayName(SIGDN_NORMALDISPLAY), item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING))
            else {
                continue;
            };
            let (name, id) = (take_string(name), take_string(id));
            if !name.is_empty() && !id.is_empty() {
                found.push(App { name, id, icon: None, setting: true });
            }
        }
    }
    dedup_settings(found)
}

/// Sorted by name; the folder lists a few tasks under several categories with the same name,
/// and one is enough.
fn dedup_settings(mut found: Vec<App>) -> Vec<App> {
    found.sort_by_key(|a| a.name.to_lowercase());
    found.dedup_by_key(|a| a.name.to_lowercase());
    found
}

pub(crate) unsafe fn take_string(p: PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

fn extract_icon(item: &IShellItem) -> Option<Icon> {
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let bitmap = factory.GetImage(SIZE { cx: ICON as i32, cy: ICON as i32 }, SIIGBF_ICONONLY).ok()?;
        let pixels = read_pixels(bitmap);
        let _ = DeleteObject(bitmap.into());
        Some(Icon::new(premultiply(pixels?)))
    }
}

unsafe fn read_pixels(bitmap: HBITMAP) -> Option<Vec<u8>> {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: ICON as i32,
            biHeight: -(ICON as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; ICON_BYTES];
    unsafe {
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(dc, bitmap, 0, ICON as u32, Some(pixels.as_mut_ptr() as _), &mut info, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        (lines == ICON as i32).then_some(pixels)
    }
}

/// The shell hands out straight or premultiplied alpha depending on the icon, and old icons
/// with no alpha channel at all; normalise everything to premultiplied.
fn premultiply(mut px: Vec<u8>) -> Vec<u8> {
    let no_alpha = px.chunks_exact(4).all(|p| p[3] == 0);
    if no_alpha {
        px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
        return px;
    }
    let premultiplied = px.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
    if !premultiplied {
        for p in px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = (*c as u32 * a / 255) as u8;
            }
        }
    }
    px
}

/// Monochrome cyan version, so unselected rows read like the in-game line icons.
/// Single-colour line-art version of premultiplied BGRA pixels, in `accent` (0xRRGGBB).
pub fn tint(px: &[u8], accent: u32) -> Vec<u8> {
    let bgr = [(accent & 0xFF) as f32, ((accent >> 8) & 0xFF) as f32, ((accent >> 16) & 0xFF) as f32];
    let mut out = px.to_vec();
    for p in out.chunks_exact_mut(4) {
        let a = p[3] as f32;
        let luminance = 0.11 * p[0] as f32 + 0.59 * p[1] as f32 + 0.3 * p[2] as f32;
        let level = (0.4 * a + 0.6 * luminance) / 255.0;
        for (c, cyan) in p[..3].iter_mut().zip(bgr) {
            *c = (cyan * level).min(a) as u8;
        }
    }
    out
}

// Cache layout: magic, u32 count, then per app: u16 len + UTF-8 name, u16 len + UTF-8 id,
// u8 kind (0 no icon, 1 icon, 2 setting), [ICON_BYTES] colour pixels if kind is 1.
// Little-endian. The tinted look is derived at draw time.
fn encode(apps: &[App]) -> Vec<u8> {
    let mut out = CACHE_MAGIC.to_vec();
    out.extend((apps.len() as u32).to_le_bytes());
    for app in apps {
        for s in [&app.name, &app.id] {
            let bytes = &s.as_bytes()[..s.len().min(u16::MAX as usize)];
            out.extend((bytes.len() as u16).to_le_bytes());
            out.extend(bytes);
        }
        match &app.icon {
            _ if app.setting => out.push(IS_SETTING),
            Some(icon) => {
                out.push(HAS_ICON);
                out.extend(&icon.color);
            }
            None => out.push(NO_ICON),
        }
    }
    out
}

fn decode(bytes: &[u8]) -> Option<Vec<App>> {
    fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        let (head, rest) = bytes.split_at_checked(n)?;
        *bytes = rest;
        Some(head)
    }
    fn string(bytes: &mut &[u8]) -> Option<String> {
        let len = u16::from_le_bytes(take(bytes, 2)?.try_into().ok()?) as usize;
        String::from_utf8(take(bytes, len)?.to_vec()).ok()
    }

    let mut bytes = bytes;
    let magic = take(&mut bytes, 4)?;
    if magic != CACHE_MAGIC && magic != CACHE_MAGIC_V1 {
        return None;
    }
    let count = u32::from_le_bytes(take(&mut bytes, 4)?.try_into().ok()?) as usize;
    let mut apps = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let name = string(&mut bytes)?;
        let id = string(&mut bytes)?;
        let (icon, setting) = match take(&mut bytes, 1)?[0] {
            NO_ICON => (None, false),
            IS_SETTING => (None, true),
            _ => (Some(Icon::new(take(&mut bytes, ICON_BYTES)?.to_vec())), false),
        };
        apps.push(App { name, id, icon, setting });
    }
    Some(apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_names_drop_windows_suffixes() {
        assert_eq!(shortcut_name("Riot Client"), "Riot Client");
        assert_eq!(shortcut_name("Obsidian.exe – skrót "), "Obsidian");
        assert_eq!(shortcut_name("DziennikApp.exe"), "DziennikApp");
        assert_eq!(shortcut_name("Steam - Shortcut"), "Steam");
        assert_eq!(shortcut_name("Folding @Home"), "Folding @Home");
    }

    #[test]
    fn straight_alpha_gets_premultiplied() {
        assert_eq!(premultiply(vec![200, 100, 50, 128]), vec![100, 50, 25, 128]);
    }

    #[test]
    fn premultiplied_pixels_are_left_alone() {
        assert_eq!(premultiply(vec![10, 20, 30, 128, 0, 0, 0, 0]), vec![10, 20, 30, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn icons_without_alpha_become_opaque() {
        assert_eq!(premultiply(vec![1, 2, 3, 0]), vec![1, 2, 3, 255]);
    }

    #[test]
    fn tint_keeps_alpha_and_stays_premultiplied() {
        let out = tint(&[255, 255, 255, 255, 0, 0, 0, 0, 40, 40, 40, 64], 0x5EF6FF);
        assert_eq!(out[3], 255);
        assert_eq!(&out[4..8], &[0, 0, 0, 0]);
        assert!(out[8..11].iter().all(|&c| c <= 64));
    }

    #[test]
    fn tint_follows_the_accent_colour() {
        let white = [255, 255, 255, 255];
        assert_eq!(&tint(&white, 0x5EF6FF)[..3], &[255, 246, 94], "BGR of the cyan accent");
        assert_eq!(&tint(&white, 0xFF3B3B)[..3], &[59, 59, 255], "red accent");
    }

    #[test]
    fn cache_roundtrip() {
        let apps = vec![
            App { name: "Zażółć".into(), id: "a!b".into(), icon: Some(Icon::new(vec![7; ICON_BYTES])), setting: false },
            App { name: "Terminal".into(), id: "C:\\t.lnk".into(), icon: None, setting: false },
            App { name: "Edytuj zmienne środowiskowe systemu".into(), id: "{E2394C16}".into(), icon: None, setting: true },
        ];
        let decoded = decode(&encode(&apps)).unwrap();
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].name, "Zażółć");
        assert_eq!(decoded[0].icon.as_ref().unwrap().color, vec![7; ICON_BYTES]);
        assert_eq!(decoded[1].id, "C:\\t.lnk");
        assert!(decoded[1].icon.is_none() && !decoded[1].setting);
        assert!(decoded[2].setting && decoded[2].icon.is_none());
        assert_eq!(decoded[2].name, "Edytuj zmienne środowiskowe systemu");
    }

    #[test]
    fn version_one_cache_still_reads() {
        let apps = vec![App { name: "A".into(), id: "a".into(), icon: Some(Icon::new(vec![1; ICON_BYTES])), setting: false }];
        let mut bytes = encode(&apps);
        bytes[..4].copy_from_slice(CACHE_MAGIC_V1);
        let decoded = decode(&bytes).unwrap();
        assert_eq!((decoded.len(), decoded[0].setting), (1, false));
        assert!(decoded[0].icon.is_some(), "icons survive the upgrade");
    }

    #[test]
    fn settings_with_the_same_name_collapse_and_sort() {
        let setting = |n: &str, id: &str| App { name: n.into(), id: id.into(), icon: None, setting: true };
        let sorted = dedup_settings(vec![setting("Zeta", "1"), setting("alfa", "2"), setting("ALFA", "3")]);
        assert_eq!(sorted.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["alfa", "Zeta"]);
    }

    #[test]
    fn truncated_cache_is_rejected() {
        let apps = vec![App { name: "A".into(), id: "a".into(), icon: Some(Icon::new(vec![1; ICON_BYTES])), setting: false }];
        let bytes = encode(&apps);
        assert!(decode(&bytes[..bytes.len() - 1]).is_none());
        assert!(decode(b"XXXX").is_none());
    }
}
