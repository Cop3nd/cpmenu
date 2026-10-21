//! Actions for the selected row, shown in place of the list after → (Run as admin, open file location…).

use std::path::Path;

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::Shell::{
    IShellItem2, KF_FLAG_DEFAULT, SEE_MASK_INVOKEIDLIST, SHCreateItemFromParsingName, SHELLEXECUTEINFOW, SHGetKnownFolderPath,
    ShellExecuteExW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{GUID, HSTRING, PCWSTR, w};

use crate::apps;
use crate::menu::Tag;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Act {
    Run,
    RunAdmin,
    Open,
    OpenLocation,
    CopyPath,
    CopyName,
    Terminal,
    Properties,
    Uninstall,
    Pin,
    Unpin,
    /// A row of a plugin's action list; what it does is in `menu::Actions::plugin`.
    Plugin,
}

impl Act {
    pub fn label(self) -> String {
        crate::lang::t(match self {
            Act::Plugin => return String::new(),
            Act::Run => "act.run",
            Act::RunAdmin => "act.run_admin",
            Act::Open => "act.open",
            Act::OpenLocation => "act.open_location",
            Act::CopyPath => "act.copy_path",
            Act::CopyName => "act.copy_name",
            Act::Terminal => "act.terminal",
            Act::Properties => "act.properties",
            Act::Uninstall => "act.uninstall",
            Act::Pin => "act.pin",
            Act::Unpin => "act.unpin",
        })
    }
}

/// Actions for a row; `path` is the file behind it (`None` for Store apps, which have none).
pub fn for_item(tag: Tag, path: Option<&str>) -> Vec<Act> {
    let file = |acts: &mut Vec<Act>| {
        if path.is_some() {
            acts.extend([Act::OpenLocation, Act::CopyPath, Act::Properties]);
        }
    };
    let mut acts = Vec::new();
    match tag {
        Tag::App | Tag::Pinned => {
            acts.extend([Act::Run, Act::RunAdmin]);
            file(&mut acts);
            // Pinned rows are indexed apps too, so both have an entry in Installed apps.
            acts.push(Act::Uninstall);
            acts.push(if tag == Tag::Pinned { Act::Unpin } else { Act::Pin });
        }
        Tag::File => {
            acts.push(Act::Open);
            file(&mut acts);
            acts.push(Act::CopyName);
        }
        Tag::Folder => {
            acts.push(Act::Open);
            file(&mut acts);
            acts.extend([Act::CopyName, Act::Terminal]);
        }
        // Quick commands already have Enter / Ctrl+Enter.
        // A plugin's rows build their action list from the plugin's own actions (`Menu::open_actions`).
        Tag::Shell | Tag::Calc | Tag::Web | Tag::Open | Tag::Setting | Tag::Action | Tag::Plugin(_) => {}
    }
    acts
}

/// `System.Link.TargetParsingPath`: where a Start Menu shortcut points.
const LINK_TARGET: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25),
    pid: 2,
};

/// The file behind an app id: desktop shortcuts and some apps use the path itself,
/// classic apps a known-folder GUID plus a relative path, the rest a shortcut whose target the shell knows.
pub fn app_path(id: &str) -> Option<String> {
    if Path::new(id).is_absolute() {
        return Some(id.to_owned());
    }
    if let Some((guid, rest)) = id.strip_prefix('{').and_then(|s| s.split_once("}\\")) {
        let folder = unsafe { SHGetKnownFolderPath(&GUID::try_from(guid).ok()?, KF_FLAG_DEFAULT, None) }.ok()?;
        return Some(format!("{}\\{rest}", unsafe { apps::take_string(folder) }));
    }
    unsafe {
        let item: IShellItem2 = SHCreateItemFromParsingName(&HSTRING::from(format!("shell:AppsFolder\\{id}")), None).ok()?;
        let target = apps::take_string(item.GetString(&LINK_TARGET).ok()?);
        Path::new(&target).is_absolute().then_some(target)
    }
}

fn shell(verb: PCWSTR, file: &str, params: &str, dir: &str) {
    let params = HSTRING::from(params);
    let dir = HSTRING::from(dir);
    let params = if params.is_empty() { PCWSTR::null() } else { PCWSTR(params.as_ptr()) };
    let dir = if dir.is_empty() { PCWSTR::null() } else { PCWSTR(dir.as_ptr()) };
    unsafe { ShellExecuteW(None, verb, &HSTRING::from(file), params, dir, SW_SHOWNORMAL) };
}

/// Elevated launch: of the file if known, else of the AppsFolder entry.
pub fn run_admin(id: &str, path: Option<&str>) {
    match path {
        Some(path) => shell(w!("runas"), path, "", ""),
        None => shell(w!("runas"), &format!("shell:AppsFolder\\{id}"), "", ""),
    }
}

/// Explorer with the file selected in its folder.
pub fn open_location(path: &str) {
    shell(w!("open"), "explorer.exe", &format!("/select,\"{path}\""), "");
}

pub fn open_terminal(dir: &str) {
    shell(w!("open"), "cmd.exe", "", dir);
}

/// Settings > Apps > Installed apps; it can't be pointed at one app, and the user does the removal there.
pub fn uninstall() {
    shell(w!("open"), "ms-settings:appsfeatures", "", "");
}

/// The shell's Properties dialog for `path`. It has no owner window: the menu hides itself when it
/// loses focus, and an owned dialog would be hidden with it. The dialog runs on a shell thread of this process.
pub fn properties(path: &str) {
    let file = HSTRING::from(path);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        lpVerb: w!("properties"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    let _ = unsafe { ShellExecuteExW(&mut info) };
}

pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_actions_depend_on_pin_and_path() {
        assert_eq!(
            for_item(Tag::App, Some(r"C:\x.exe")),
            vec![Act::Run, Act::RunAdmin, Act::OpenLocation, Act::CopyPath, Act::Properties, Act::Uninstall, Act::Pin]
        );
        assert_eq!(
            for_item(Tag::Pinned, None),
            vec![Act::Run, Act::RunAdmin, Act::Uninstall, Act::Unpin],
            "Store app: no file, so no location or properties"
        );
    }

    #[test]
    fn files_and_folders_get_file_actions() {
        assert!(for_item(Tag::Folder, Some(r"C:\x")).contains(&Act::Terminal));
        assert!(!for_item(Tag::File, Some(r"C:\x.txt")).contains(&Act::Terminal));
        for tag in [Tag::File, Tag::Folder] {
            let acts = for_item(tag, Some(r"C:\x"));
            assert!(acts.contains(&Act::Properties) && !acts.contains(&Act::Uninstall), "{tag:?}");
        }
        assert!(!for_item(Tag::Setting, None).contains(&Act::Uninstall));
        assert!(for_item(Tag::App, None).contains(&Act::Uninstall) && !for_item(Tag::App, None).contains(&Act::Properties));
        assert!(for_item(Tag::Calc, None).is_empty());
    }

    #[test]
    fn known_folder_ids_resolve_to_paths() {
        let path = app_path(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\cmd.exe").unwrap();
        assert!(path.to_lowercase().ends_with(r"\system32\cmd.exe"), "{path}");
        assert_eq!(app_path(r"C:\a\b.lnk").as_deref(), Some(r"C:\a\b.lnk"));
        assert_eq!(file_name(r"C:\a\b.lnk"), "b.lnk");
    }
}
