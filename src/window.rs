use std::cell::{Cell, RefCell};
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::time::SystemTime;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ScreenToClient};
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{GetCurrentProcess, INFINITE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey, VK_MENU,
    GetKeyState, ReleaseCapture, SetCapture, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F2, VK_HOME, VK_INSERT, VK_LEFT, VK_NEXT,
    VK_OEM_COMMA, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, Result, w};

use crate::Command;
use crate::actions::{self, Act};
use crate::apps;
use crate::commands;
use crate::config::{self, ActionsMode, Config, LayoutConfig, Side};
use crate::everything;
use crate::hook;
use crate::menu::{GLOBAL_EACH, GLOBAL_TOTAL, Item, Menu, PluginActKind, PluginRow, RowKind, Tag};
use crate::plugin_host::{self, Handled, Host, IDLE_MS, Status};
use crate::plugin_proc::{self, WindowSink};
use crate::plugin_proto::{self as proto, Effect};
use crate::power::{ACTIONS, Action, PowerBar};
use crate::power_overlay::{Dialog, DialogHit, Focus, Nav, Phase, Sequence, is_auto_repeat};
use crate::lineedit::LineEdit;
use crate::hotkey::{Hotkey, MOD_ALT, MOD_CONTROL, MOD_SHIFT};
use crate::fontmenu::{self, Focus as DropFocus, Outcome};
use crate::render::{DropHit, Hover, Layout, ListHit, Overlay, POPUP_ENTRIES, PanelHit, PopoutParams, PowerFrame, Renderer};
use crate::style::{self, Style};
use crate::plugins;
use crate::settings::{Advance, Button, DeleteStep, EditKind, NameCheck, Note, Panel, Pending, PluginField, PluginWrite, Row, SLOTS, Tab};
use crate::usage::{self, Usage};
use crate::winsearch;
use crate::{autostart, lang, themes};

const CLASS: PCWSTR = w!("cpmenu");
const WM_MOUSELEAVE: u32 = 675;
pub const WM_COMMAND_MSG: u32 = WM_APP + 1;
// Clicking the bar button while the menu is open first deactivates (hides) it and then
// delivers `toggle`; without this window the menu would immediately reopen.
const REOPEN_GUARD_MS: u64 = 300;
const ANIM_TIMER: usize = 1;
const SEARCH_TIMER: usize = 2;
/// Runs only while the power overlay's glitch plays.
const OVERLAY_TIMER: usize = 3;
/// One-shot: the next soft or hard timeout of a plugin request; armed only while the menu is visible.
const PLUGIN_TIMER: usize = 4;
/// One-shot, armed when the menu hides while a plugin runs: shuts down the idle ones.
const IDLE_TIMER: usize = 5;
/// One-shot: how long a plugin's error message stays in the header.
const NOTICE_TIMER: usize = 6;
const NOTICE_MS: u32 = 3000;
/// Wait for a pause in typing before asking Everything.
const SEARCH_DEBOUNCE_MS: u32 = 120;
const FILE_SEARCH_MIN_CHARS: usize = 2;
const FRAME_MS: u32 = 16;
const PAGE: isize = 5;
/// Packaged (UWP/MSIX) installs don't touch the Start Menu folders, so rescan on open after this long.
const RESCAN_AFTER_MS: u64 = 30 * 60 * 1000;

static LAST_DEACTIVATE_HIDE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    // Created on first show so an autostarted, never-opened menu stays tiny.
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
    static MENU: RefCell<Menu> = RefCell::new(Menu::default());
    /// Plugins. Never borrowed across anything that can pump messages (ShellExecute, SendMessage).
    static PLUGINS: RefCell<Host> = RefCell::new(Host::default());
    static INDEXER: RefCell<Option<Child>> = const { RefCell::new(None) };
    static INDEX_DIRTY: Cell<bool> = const { Cell::new(true) };
    static LAST_SCAN: Cell<u64> = const { Cell::new(0) };
    /// Query of the Everything request in flight; replies for anything else are stale.
    static FILE_QUERY: RefCell<String> = const { RefCell::new(String::new()) };
    static POWER: RefCell<PowerBar> = RefCell::new(PowerBar::default());
    static CONFIG: RefCell<Config> = RefCell::new(Config::default());
    /// Modification time of config.toml when it was last read; a change means it was edited by hand.
    static CONFIG_SEEN: Cell<Option<SystemTime>> = const { Cell::new(None) };
    static OPEN_START: Cell<u64> = const { Cell::new(0) };
    static FRAME_NO: Cell<u32> = const { Cell::new(0) };
    static POSITION: Cell<POINT> = const { Cell::new(POINT { x: 0, y: 0 }) };
    /// Geometry for the monitor the menu was last shown on.
    static LAYOUT: Cell<Layout> = Cell::new(Layout::default());
    /// Work area and DPI scale of that monitor.
    static WORK: Cell<RECT> = const { Cell::new(RECT { left: 0, top: 0, right: 1920, bottom: 1080 }) };
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
    /// Full rectangle of that monitor (the power overlay covers all of it, taskbar included).
    static MONITOR: Cell<RECT> = const { Cell::new(RECT { left: 0, top: 0, right: 1920, bottom: 1080 }) };
    /// The running power overlay, if any; the window then covers the whole monitor.
    static POWER_SEQ: RefCell<Option<Sequence>> = const { RefCell::new(None) };
    /// Settings panel state while it is open.
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
    /// Where the right-click popup was opened, in window coordinates.
    static POPUP: Cell<Option<(f32, f32)>> = const { Cell::new(None) };
    /// The popup row Enter runs; moved by the arrow keys and by hovering.
    static POPUP_ROW: Cell<usize> = const { Cell::new(0) };
    /// Pixel width of the open actions pop-out, fitted to its labels when it opened.
    static POPOUT_W: Cell<f32> = const { Cell::new(0.0) };
    /// Where the pointer was (menu coordinates) when the pop-out was opened by a right click.
    static POPOUT_CURSOR: Cell<Option<(f32, f32)>> = const { Cell::new(None) };
    /// Element colors, and the modification time of style.toml they were read at.
    static STYLE: Cell<Style> = Cell::new(Style::from_theme(&Config::default().colors));
    static STYLE_SEEN: Cell<Option<SystemTime>> = const { Cell::new(None) };
    static HOVER: Cell<Option<Hover>> = const { Cell::new(None) };
    /// A WM_MOUSELEAVE request is pending, so moving doesn't re-arm it every time.
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// The configured hotkey combination could not be registered.
    static HOTKEY_FAILED: Cell<bool> = const { Cell::new(false) };
    /// A double click started hex typing; its button-up must not count as another click (it would cancel the typing).
    static SKIP_BUTTON_UP: Cell<bool> = const { Cell::new(false) };
    /// The left button went down on the query text and the mouse is captured: moving selects.
    static SELECTING: Cell<bool> = const { Cell::new(false) };
}

pub fn send(command: Command) {
    unsafe {
        let Ok(hwnd) = FindWindowW(CLASS, PCWSTR::null()) else {
            return;
        };
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let _ = AllowSetForegroundWindow(pid);
        let _ = PostMessageW(Some(hwnd), WM_COMMAND_MSG, WPARAM(command as usize), LPARAM(0));
    }
}

pub fn run(initial: Option<Command>) -> Result<()> {
    unsafe {
        // Before any window exists, so coordinates are physical pixels on every monitor.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // ShellExecute (launching apps) wants an STA.
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let instance = GetModuleHandleW(None)?.into();
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
            CLASS,
            w!("cpmenu"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;

        everything::allow_replies(hwnd);
        PLUGINS.with_borrow_mut(|h| h.attach(Arc::new(WindowSink(hwnd.0 as isize))));
        MENU.with_borrow_mut(|m| m.usage = Usage::load());
        config::migrate_onboarded();
        reload_config(hwnd);
        load_index();
        start_index();
        if let Some(command) = initial {
            handle(hwnd, command);
        }
        message_loop(hwnd, &watch_start_menu());
        Ok(())
    }
}

fn watch_start_menu() -> Vec<HANDLE> {
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME;
    apps::watched_dirs()
        .iter()
        .filter_map(|dir| unsafe { FindFirstChangeNotificationW(&HSTRING::from(dir), true, filter) }.ok())
        .collect()
}

/// Blocks until a message arrives, a watched Start Menu folder changes or the indexer exits;
/// no polling.
unsafe fn message_loop(hwnd: HWND, watches: &[HANDLE]) {
    unsafe {
        let mut msg = MSG::default();
        loop {
            let indexer = INDEXER.with_borrow(|c| c.as_ref().map(|c| HANDLE(c.as_raw_handle())));
            let handles: Vec<HANDLE> = watches.iter().copied().chain(indexer).collect();
            let woke = MsgWaitForMultipleObjects(Some(&handles), false, INFINITE, QS_ALLINPUT);
            let signalled = woke.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            if let Some(&watch) = watches.get(signalled) {
                // Rescanned lazily on the next open, so a burst of installer writes costs nothing.
                INDEX_DIRTY.set(true);
                let _ = FindNextChangeNotification(watch);
                continue;
            }
            if signalled == watches.len() && indexer.is_some() {
                finish_index(hwnd);
                continue;
            }
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn start_index() {
    if INDEXER.with_borrow(Option::is_some) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    if let Ok(child) = std::process::Command::new(exe).arg("index").spawn() {
        INDEXER.set(Some(child));
        INDEX_DIRTY.set(false);
        LAST_SCAN.set(unsafe { GetTickCount64() });
    }
}

fn index_stale() -> bool {
    INDEX_DIRTY.get() || unsafe { GetTickCount64() } - LAST_SCAN.get() > RESCAN_AFTER_MS
}

fn load_index() {
    let apps: Vec<Item> = apps::load_cache().into_iter().map(Item::from).collect();
    CONFIG.with_borrow(|c| MENU.with_borrow_mut(|m| m.set_apps(apps, &c.pinned)));
    RENDERER.with_borrow(|r| r.as_ref().map(Renderer::clear_icons));
}

fn reload_config(hwnd: HWND) {
    apply_config(hwnd, config::load());
}

/// Makes `new` the live config: search URL, config timestamp, element colors and the hotkey follow it.
fn apply_config(hwnd: HWND, new: Config) {
    CONFIG.set(new);
    CONFIG.with_borrow(|c| {
        MENU.with_borrow_mut(|m| {
            m.search_url = c.search_url.clone();
            m.set_recent(c.recent);
        });
        lang::configure(&c.language, &c.text);
        PLUGINS.with_borrow_mut(|h| h.sync(&c.plugins));
        with_panel(|p| p.sync_config(c));
    });
    push_plugin_keywords();
    refresh_plugin_panel();
    CONFIG_SEEN.set(config::modified());
    refresh_style();
    apply_hotkey(hwnd);
}

/// Gives the open settings panel the plugin list as the plugin host has it now.
fn refresh_plugin_panel() {
    if PANEL.with_borrow(Option::is_none) {
        return;
    }
    let (on, infos) = (CONFIG.with_borrow(|c| c.plugins.enabled), PLUGINS.with_borrow(Host::infos));
    with_panel(|p| p.set_plugins(on, infos));
}

/// Element colors: the current theme with style.toml on top.
fn refresh_style() {
    STYLE.set(CONFIG.with_borrow(style::load));
    STYLE_SEEN.set(style::modified());
}

const HOTKEY_ID: i32 = 1;

/// Points the keyboard hook / `RegisterHotKey` at the configured hotkey (or at capturing a new one).
/// A combination another program already owns is reported in the panel and left unregistered.
fn apply_hotkey(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
    }
    HOTKEY_FAILED.set(false);
    let hotkey = if capturing() { Hotkey::None } else { CONFIG.with_borrow(|c| c.hotkey) };
    let binding = match hotkey {
        Hotkey::Tap(vk) => Some(vk),
        _ => None,
    };
    hook::configure(hwnd, binding, capturing());
    if let Hotkey::Combo { mods, vk } = hotkey {
        let registered = unsafe { RegisterHotKey(Some(hwnd), HOTKEY_ID, HOT_KEY_MODIFIERS(mods | MOD_NOREPEAT.0), vk as u32) };
        HOTKEY_FAILED.set(registered.is_err());
    }
}

/// Sets `HOTKEY_FAILED` by whether another program already owns the combination.
fn probe_hotkey(hwnd: HWND, hotkey: Hotkey) {
    let Hotkey::Combo { mods, vk } = hotkey else { return };
    let probe = unsafe { RegisterHotKey(Some(hwnd), HOTKEY_ID, HOT_KEY_MODIFIERS(mods | MOD_NOREPEAT.0), vk as u32) };
    if probe.is_ok() {
        let _ = unsafe { UnregisterHotKey(Some(hwnd), HOTKEY_ID) };
    }
    HOTKEY_FAILED.set(probe.is_err());
}

fn capturing() -> bool {
    PANEL.with_borrow(|p| p.as_ref().is_some_and(|p| p.capturing))
}

/// Enter on the HOTKEY row: the next key press becomes the hotkey.
fn begin_capture(hwnd: HWND) {
    with_panel(Panel::begin_capture);
    apply_hotkey(hwnd);
    redraw(hwnd);
}

fn end_capture(hwnd: HWND) {
    with_panel(|p| p.capturing = false);
    apply_hotkey(hwnd);
    redraw(hwnd);
}

/// Writes the captured hotkey and applies it; a refused write leaves the old one in place.
fn finish_capture(hwnd: HWND, hotkey: Hotkey) {
    with_panel(Panel::capture_done);
    match config::set_value("behavior", "hotkey", hotkey.format().into()) {
        Some(new) => {
            with_panel(|p| p.error = false);
            apply_config(hwnd, new);
            // The wizard keeps capturing, so nothing is registered yet: still report a taken combination.
            if capturing() {
                probe_hotkey(hwnd, hotkey);
            }
        }
        None => {
            with_panel(|p| p.error = true);
            apply_hotkey(hwnd);
        }
    }
    redraw(hwnd);
}

/// A key press while capturing (from the window, or reported by the hook for Win keys).
/// `mods` are the modifiers held with it. Returns false when not capturing.
fn capture_press(hwnd: HWND, vk: u16, mods: u32) -> bool {
    if !capturing() {
        return false;
    }
    if wizard_capture_key(hwnd, vk, mods) {
        return true;
    }
    if mods == 0 && vk == VK_ESCAPE.0 {
        end_capture(hwnd);
    } else if mods == 0 && vk == VK_BACK.0 {
        finish_capture(hwnd, Hotkey::None);
    } else if let Some(hotkey) = Hotkey::from_press(vk, mods) {
        finish_capture(hwnd, hotkey);
    }
    true
}

fn held_modifiers() -> u32 {
    let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    [(VK_CONTROL.0, MOD_CONTROL), (VK_MENU.0, MOD_ALT), (VK_SHIFT.0, MOD_SHIFT)]
        .into_iter()
        .filter(|&(vk, _)| down(vk))
        .fold(0, |mods, (_, flag)| mods | flag)
}

/// One entry per visible panel line: true for a settings row, false for a section heading.
fn panel_slots() -> Vec<bool> {
    PANEL.with_borrow(|p| p.as_ref().map(|p| p.row_slots().iter().map(Option::is_some).collect()).unwrap_or_default())
}

/// Mouse moved to (x, y): redraw only if the hovered element changed.
fn hover_to(hwnd: HWND, x: f32, y: f32) {
    let hover = if in_power_overlay() {
        match dialog_at(x, y) {
            Some(DialogHit::Execute) => Some(Hover::Execute),
            Some(DialogHit::Abort) => Some(Hover::Abort),
            _ => None,
        }
    } else if let Some((slot, count)) = toggles_geom() {
        match LAYOUT.get().toggles_hit(slot, count, x, y) {
            Some(DropHit::Anchor) | None => None,
            Some(hit) => Some(Hover::Drop(hit)),
        }
    } else if let Some(geom) = dropdown_geom() {
        // Under an open dropdown nothing else is hot.
        match LAYOUT.get().dropdown_hit(geom, x, y) {
            Some(DropHit::Anchor) | None => None,
            Some(hit) => Some(Hover::Drop(hit)),
        }
    } else if popout_hover(hwnd, x, y) {
        None
    } else {
        LAYOUT.get().hover_at(x, y, &panel_slots(), POPUP.get())
    };
    if let Some(Hover::Popup(row)) = hover {
        POPUP_ROW.set(row);
    }
    set_hover(hwnd, hover);
}

/// Whether (x, y) is over an item of the actions pop-out; that item then becomes the selected one.
fn popout_hover(hwnd: HWND, x: f32, y: f32) -> bool {
    let Some(item) = popout_params().and_then(|p| LAYOUT.get().popout_item_at(&p, x, y)) else { return false };
    let before = MENU.with_borrow(|m| m.popout().map(|a| a.selected));
    MENU.with_borrow_mut(|m| m.select_row(item));
    if before != Some(item) {
        redraw(hwnd);
    }
    true
}

/// What places the actions pop-out, while one is open.
fn popout_params() -> Option<PopoutParams> {
    let cfg = CONFIG.with_borrow(|c| c.layout);
    MENU.with_borrow(|m| {
        let a = m.popout()?;
        Some(PopoutParams {
            items: a.acts.len(),
            selected: a.selected,
            width: POPOUT_W.get(),
            row: m.selected,
            scroll: m.scroll,
            rows: m.view.len(),
            anchor: cfg.actions_anchor,
            side: cfg.side,
            cursor: POPOUT_CURSOR.get(),
            offset: (cfg.actions_x, cfg.actions_y),
        })
    })
}

/// Width of the column the window needs beside the menu for the open pop-out (0 = none; the
/// layered window is only as wide as the list otherwise, so the pop-out needs the room).
fn popout_column(cfg: &LayoutConfig) -> i32 {
    if PANEL.with_borrow(Option::is_some) || MENU.with_borrow(|m| m.popout().is_none()) {
        return 0;
    }
    let Some(p) = popout_params() else { return 0 };
    let (work, scale) = (WORK.get(), SCALE.get());
    let work_w = work.right - work.left;
    let (menu_w, height) = Layout::window_size(work_w, work.bottom - work.top, scale, cfg);
    // The pop-out as it would be placed in the window without the column decides how much it needs.
    let needed = Layout::new(menu_w, height, scale, cfg).popout_overhang(&p);
    Layout::popout_column(needed, work_w, menu_w, scale)
}

fn set_hover(hwnd: HWND, hover: Option<Hover>) {
    if hover != HOVER.replace(hover) {
        redraw(hwnd);
    }
}

/// Rows moved, so icons cached by item index are stale.
fn apply_pins() {
    CONFIG.with_borrow(|c| MENU.with_borrow_mut(|m| m.repin(&c.pinned)));
    RENDERER.with_borrow(|r| r.as_ref().map(Renderer::clear_icons));
}

fn toggle_pin(hwnd: HWND) {
    let name = MENU.with_borrow(|m| {
        let item = &m.items[m.current()?];
        matches!(item.tag, Tag::App | Tag::Pinned).then(|| item.name.clone())
    });
    if let Some(name) = name {
        pin(hwnd, &name);
    }
}

fn pin(hwnd: HWND, name: &str) {
    CONFIG.set(config::toggle_pin(name));
    CONFIG_SEEN.set(config::modified());
    apply_pins();
    redraw(hwnd);
}

fn open_panel(hwnd: HWND) {
    POPUP.set(None);
    MENU.with_borrow_mut(Menu::close_popout);
    if PANEL.with_borrow(Option::is_none) {
        PANEL.set(Some(Panel::scanned()));
        CONFIG.with_borrow(|c| with_panel(|p| p.sync_config(c)));
        PLUGINS.with_borrow_mut(|h| h.rescan());
        refresh_plugin_panel();
        resize(hwnd);
    } else {
        redraw(hwnd);
    }
}

fn close_panel(hwnd: HWND) {
    PANEL.set(None);
    HOVER.set(None);
    resize(hwnd);
}

/// F2, Ctrl+, and the header gear: opens the settings panel, or closes it if it is open.
fn toggle_panel(hwnd: HWND) {
    if PANEL.with_borrow(Option::is_some) {
        close_panel(hwnd);
    } else {
        open_panel(hwnd);
    }
}

/// Keys that toggle the settings panel without the right-click popup.
fn is_settings_key(key: u16, ctrl: bool) -> bool {
    key == VK_F2.0 || (key == VK_OEM_COMMA.0 && ctrl)
}

fn with_panel(change: impl FnOnce(&mut Panel)) {
    PANEL.with_borrow_mut(|p| p.as_mut().map(change));
}

/// The first-run wizard is what the panel card shows.
fn wizard_active() -> bool {
    PANEL.with_borrow(|p| p.as_ref().is_some_and(|p| p.wizard.is_some()))
}

/// Shows the wizard on the first open of a new install (`[behavior] onboarded` unset). A config.toml
/// with syntax errors is left alone: the wizard could not write its choices.
fn begin_onboarding(hwnd: HWND) {
    if CONFIG.with_borrow(|c| c.onboarded) || !config::editable() {
        return;
    }
    // A new install: the first choice would create the file without the key, and a restart before the
    // end would then count it as an old config. Writing the key first keeps the wizard pending.
    if config::modified().is_none() {
        if let Some(new) = config::set_value("behavior", "onboarded", false.into()) {
            apply_config(hwnd, new);
        }
    }
    let mut panel = Panel::scanned();
    panel.start_wizard(true);
    PANEL.set(Some(panel));
    CONFIG.with_borrow(|c| with_panel(|p| p.sync_config(c)));
}

/// Makes the hotkey capture run exactly on the HOTKEY step.
fn wizard_sync_capture(hwnd: HWND) {
    let wants = PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.wizard.as_ref()).is_some_and(|w| w.captures()));
    if wants != capturing() {
        with_panel(|p| p.capturing = wants);
        apply_hotkey(hwnd);
    }
}

/// Enter / DALEJ: the next step, or the end after the last.
fn wizard_next(hwnd: HWND) {
    if PANEL.with_borrow_mut(|p| p.as_mut().map(Panel::wizard_next)) == Some(Advance::Finished) {
        // AUTOSTART shows ON until touched; the choice is applied here.
        if let Some(on) = PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.wizard.as_ref()).map(|w| w.autostart)) {
            let _ = autostart::set(on);
            with_panel(|p| p.autostart = on);
        }
        return wizard_end(hwnd);
    }
    wizard_sync_capture(hwnd);
    redraw(hwnd);
}

fn wizard_back(hwnd: HWND) {
    if PANEL.with_borrow_mut(|p| p.as_mut().is_some_and(Panel::wizard_back)) {
        wizard_sync_capture(hwnd);
    }
    redraw(hwnd);
}

/// Esc, X and POMIŃ: the rest of the wizard is skipped. What was chosen so far stays.
fn wizard_skip(hwnd: HWND) {
    wizard_end(hwnd);
}

/// Marks the config onboarded and leaves the wizard: a first run closes the panel, one started
/// from the settings goes back to them.
fn wizard_end(hwnd: HWND) {
    if !CONFIG.with_borrow(|c| c.onboarded) {
        if let Some(new) = config::set_value("behavior", "onboarded", true.into()) {
            apply_config(hwnd, new);
        }
    }
    let first_run = PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.wizard.as_ref()).is_some_and(|w| w.first_run));
    with_panel(|p| {
        p.wizard = None;
        p.capturing = false;
    });
    apply_hotkey(hwnd);
    if first_run {
        close_panel(hwnd);
    } else {
        resize(hwnd);
    }
}

/// A key while the wizard is up (the hotkey step's own keys are in `wizard_capture_key`).
fn wizard_key(hwnd: HWND, key: u16) {
    with_panel(|p| p.flash = Note::None);
    match key {
        k if k == VK_ESCAPE.0 => wizard_skip(hwnd),
        k if k == VK_RETURN.0 => wizard_next(hwnd),
        k if k == VK_BACK.0 => wizard_back(hwnd),
        k if k == VK_LEFT.0 => change_setting(hwnd, -1),
        k if k == VK_RIGHT.0 => change_setting(hwnd, 1),
        k if k == VK_UP.0 || k == VK_DOWN.0 => {
            with_panel(|p| p.move_selection(if k == VK_UP.0 { -1 } else { 1 }));
            redraw(hwnd);
        }
        _ => {}
    }
}

/// A bare navigation key on the HOTKEY step, while keys are being captured: they move the wizard
/// instead of becoming the hotkey (Enter keeps the current one). True when handled.
fn wizard_capture_key(hwnd: HWND, vk: u16, mods: u32) -> bool {
    if mods != 0 || !wizard_active() {
        return false;
    }
    match vk {
        k if k == VK_RETURN.0 || k == VK_RIGHT.0 => wizard_next(hwnd),
        k if k == VK_BACK.0 || k == VK_LEFT.0 => wizard_back(hwnd),
        k if k == VK_ESCAPE.0 => wizard_skip(hwnd),
        _ => return false,
    }
    true
}

/// A left click on the wizard card.
fn wizard_click(hwnd: HWND, x: f32, y: f32) {
    let hit = LAYOUT.get().panel_hit(x, y, &panel_slots());
    let row_in = |slot: usize| PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.row_at(slot)));
    match hit {
        Some(PanelHit::Back) => return wizard_back(hwnd),
        Some(PanelHit::Skip | PanelHit::Close) => return wizard_skip(hwnd),
        Some(PanelHit::Next) => return wizard_next(hwnd),
        Some(PanelHit::Row(slot)) => {
            if let Some(row) = row_in(slot) {
                with_panel(|p| p.select(row));
                if PANEL.with_borrow(|p| p.as_ref().and_then(Panel::current)) == Some(Row::Hotkey) {
                    return begin_capture(hwnd);
                }
            }
        }
        Some(PanelHit::Arrow(slot, dir)) => {
            let Some(row) = row_in(slot) else { return };
            with_panel(|p| p.select(row));
            return change_setting(hwnd, dir);
        }
        _ => {}
    }
    redraw(hwnd);
}

/// Steps the selected panel row in direction `dir`. The new value is written to config.toml first
/// and only applied (theme, layout, Win-key hook) if the file accepted it.
fn change_setting(hwnd: HWND, dir: i32) {
    let Some(row) = PANEL.with_borrow(|p| p.as_ref().and_then(Panel::current)) else { return };
    if let Row::Color(index) = row {
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        return step_color(hwnd, index, dir, shift);
    }
    if let Row::Shape(_) = row {
        if let Some((key, value)) = row.shape_change(&STYLE.get().shape, dir) {
            write_shape(hwnd, key, Some(value));
        }
        return;
    }
    if let Row::Show(_) = row {
        if let Some((key, value)) = row.show_change(&STYLE.get().show, dir) {
            write_show(hwnd, &[(key, Some(value))]);
        }
        return;
    }
    // The fonts are chosen in their dropdown, not stepped.
    if row.opens_dropdown() {
        return;
    }
    if row == Row::Autostart {
        return toggle_autostart(hwnd);
    }
    if matches!(row, Row::Plugin(..)) {
        return change_plugin(hwnd, dir);
    }
    let Some((table, key, value)) = PANEL.with_borrow(|p| CONFIG.with_borrow(|c| p.as_ref().and_then(|p| p.change(c, dir)))) else {
        return;
    };
    match config::set_value(table, key, value) {
        Some(new) => {
            with_panel(|p| p.error = false);
            apply_config(hwnd, new);
            resize(hwnd);
        }
        None => {
            with_panel(|p| p.error = true);
            redraw(hwnd);
        }
    }
}

/// Toggles or steps the selected plugin row (ENABLED, GLOBAL, a bool or choice setting).
fn change_plugin(hwnd: HWND, dir: i32) {
    if let Some(write) = PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.plugin_step(dir))) {
        write_plugin(hwnd, write);
    }
}

/// Writes a plugin's entry to config.toml and applies it: routing is rebuilt, a plugin that may no
/// longer run is shut down, and so is one whose keyword changed (it starts again on the next
/// query). A running plugin is sent its settings when one of them changed. A file with syntax
/// errors is left alone and the panel says so.
fn write_plugin(hwnd: HWND, write: PluginWrite) {
    let (new, keyword_of, settings_of) = match write {
        PluginWrite::Value { id, key, value } => (config::set_plugin_value(&id, key, value), (key == "keyword").then_some(id), None),
        PluginWrite::Remove { id, key } => (config::remove_plugin_value(&id, key), (key == "keyword").then_some(id), None),
        PluginWrite::Setting { id, key, value } => (config::set_plugin_setting(&id, &key, value), None, Some(id)),
    };
    match new {
        Some(new) => {
            with_panel(|p| p.error = false);
            apply_config(hwnd, new);
            PLUGINS.with_borrow_mut(|h| {
                if let Some(id) = &keyword_of {
                    h.stop_plugin(id);
                }
                if let Some(id) = &settings_of {
                    h.push_settings(id);
                }
            });
            refresh_plugin_panel();
        }
        None => with_panel(|p| p.error = true),
    }
    redraw(hwnd);
}

/// Enter or a click on a row that presses (OPEN PLUGINS FOLDER, OPEN LOG, APPROVE, STATE).
fn press_row(hwnd: HWND) {
    let Some(row) = PANEL.with_borrow(|p| p.as_ref().and_then(Panel::current)) else { return };
    let id_of = |field: PluginField| PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.plugin_id_of(field).map(str::to_owned)));
    match row {
        Row::PluginsFolder => open_plugins_folder(hwnd),
        Row::Tutorial => {
            with_panel(|p| p.start_wizard(false));
            resize(hwnd);
        }
        Row::ClearHistory => {
            // Asks first; the second press forgets everything launched so far.
            if PANEL.with_borrow_mut(|p| p.as_mut().is_some_and(Panel::clear_history)) {
                MENU.with_borrow_mut(Menu::clear_usage);
            }
            redraw(hwnd);
        }
        Row::Plugin(_, PluginField::Log) => {
            if let Some(id) = id_of(PluginField::Log) {
                open_plugin_log(hwnd, &id);
            }
        }
        Row::Plugin(_, PluginField::Approve) => {
            // Asks first; the second press (Enter or click) lets the plugin run as the user.
            if let Some(id) = PANEL.with_borrow_mut(|p| p.as_mut().and_then(Panel::plugin_approve)) {
                match PLUGINS.with_borrow(|h| h.approval_key_of(&id)) {
                    Some(key) => write_plugin(hwnd, PluginWrite::Value { id, key: "approved", value: key.into() }),
                    None => redraw(hwnd),
                }
            } else {
                redraw(hwnd);
            }
        }
        Row::Plugin(_, PluginField::State) => {
            if let Some(id) = id_of(PluginField::State) {
                PLUGINS.with_borrow_mut(|h| h.retry(&id));
                refresh_plugin_panel();
            }
            redraw(hwnd);
        }
        _ => {}
    }
}

/// Opens the plugins folder in Explorer, creating it first so the button always lands somewhere.
fn open_plugins_folder(hwnd: HWND) {
    let Some(root) = plugins::root() else { return };
    let _ = std::fs::create_dir_all(&root);
    if let Some(path) = root.to_str() {
        apps::open_path(path);
        unsafe { hide(hwnd) };
    }
}

/// OPEN LOG: the plugin's log file, created empty if the plugin never ran.
fn open_plugin_log(hwnd: HWND, id: &str) {
    let opened = plugins::log_path(id).and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
        path.to_str().map(str::to_owned)
    });
    match opened {
        Some(path) => {
            apps::open_path(&path);
            unsafe { hide(hwnd) };
        }
        None => {
            with_panel(|p| p.flash = Note::LogFailed);
            redraw(hwnd);
        }
    }
}

/// The AUTOSTART row: writes or deletes the HKCU Run value at once. On failure the panel keeps the
/// state it showed and says so.
fn toggle_autostart(hwnd: HWND) {
    // The wizard shows its own choice (ON before it is touched), which may differ from the registry.
    let Some(want) = PANEL.with_borrow(|p| p.as_ref().map(|p| p.wizard.as_ref().map_or(!p.autostart, |w| !w.autostart))) else { return };
    match autostart::set(want) {
        Ok(()) => with_panel(|p| {
            p.autostart = want;
            if let Some(w) = p.wizard.as_mut() {
                w.autostart = want;
            }
        }),
        Err(_) => with_panel(|p| p.flash = Note::AutostartFailed),
    }
    redraw(hwnd);
}

/// ←/→ on a color row: the next palette color, or with Shift (also on a click of the arrows) the
/// next 5% opacity step, written to style.toml and applied.
fn step_color(hwnd: HWND, index: usize, dir: i32, opacity: bool) {
    let Some(current) = style::KEYS.get(index).and_then(|key| STYLE.get().get(key)) else { return };
    let next = if opacity {
        let Some(next) = current.stepped_alpha(dir) else { return };
        next
    } else {
        let palette = CONFIG.with_borrow(|c| style::palette(&c.colors));
        style::step_color(current, &palette, dir)
    };
    write_color(hwnd, index, Some(next.hex()));
}

/// Sets (`Some(hex)`) or removes (`None`, back to the theme's color) one style.toml key and applies
/// it; a style.toml with syntax errors is left alone and the panel says so.
fn write_color(hwnd: HWND, index: usize, hex: Option<String>) {
    let Some(&key) = style::KEYS.get(index) else { return };
    let defaults = CONFIG.with_borrow(Config::defaults);
    let written = style::set_color(&defaults, key, hex.as_deref());
    with_panel(|p| p.style_error = !written);
    if written {
        refresh_style();
    }
    redraw(hwnd);
}

/// Sets (`Some`) or removes (`None`, back to the theme's value) one `[shape]` key of style.toml and
/// applies it; row sizes change the layout, so the window is laid out again. A style.toml with
/// syntax errors is left alone and the panel says so.
fn write_shape(hwnd: HWND, key: &'static str, value: Option<toml_edit::Value>) {
    let defaults = CONFIG.with_borrow(Config::defaults);
    let written = style::set_shape(&defaults, key, value);
    with_panel(|p| p.style_error = !written);
    if written {
        refresh_style();
        return resize(hwnd);
    }
    redraw(hwnd);
}

/// Applies `[show]` changes to style.toml in one write (`None` removes a key, back to the theme's
/// value) and lays the window out again: hidden parts change the list's room. A style.toml with
/// syntax errors is left alone and the panel says so.
fn write_show(hwnd: HWND, changes: &[(&str, Option<toml_edit::Value>)]) {
    let defaults = CONFIG.with_borrow(Config::defaults);
    let written = style::set_show(&defaults, changes);
    with_panel(|p| p.style_error = !written);
    if written {
        refresh_style();
        // With the bar gone Tab can't leave focus on it.
        if !STYLE.get().show.power_bar {
            POWER.with_borrow_mut(PowerBar::reset);
        }
        return resize(hwnd);
    }
    redraw(hwnd);
}

/// Sets (`Some`) or removes (`None`, back to the theme's value) one `[fonts]` key of style.toml and
/// applies it: the next frame builds its text formats again. A style.toml with syntax errors is
/// left alone and the panel says so.
fn write_font(hwnd: HWND, key: &str, value: Option<toml_edit::Value>) {
    let defaults = CONFIG.with_borrow(Config::defaults);
    let written = style::set_font(&defaults, key, value);
    with_panel(|p| p.style_error = !written);
    if written {
        refresh_style();
    }
    redraw(hwnd);
}

fn dropdown_open() -> bool {
    PANEL.with_borrow(|p| p.as_ref().is_some_and(|p| p.dropdown.is_some() || p.toggles.is_some()))
}

/// Screen slot and entry count of the open toggle pop-out.
fn toggles_geom() -> Option<(usize, usize)> {
    PANEL.with_borrow(|p| p.as_ref().and_then(Panel::toggles_geom))
}

/// Writes a flip the toggle pop-out asked for (it stays open).
fn toggles_flip(hwnd: HWND, flip: Option<(&'static str, toml_edit::Value)>) {
    match flip {
        Some((key, value)) => write_show(hwnd, &[(key, Some(value))]),
        None => redraw(hwnd),
    }
}

/// What places the open font dropdown on the card.
fn dropdown_geom() -> Option<fontmenu::DropGeom> {
    PANEL.with_borrow(|p| p.as_ref().and_then(Panel::drop_geom))
}

/// Enter or a click on a font row.
fn open_dropdown(hwnd: HWND) {
    with_panel(|p| p.open_dropdown(&STYLE.get().fonts));
    redraw(hwnd);
}

/// Does what the dropdown asked for: size and weight are written and applied live; a family is
/// written and closes it.
fn dropdown_outcome(hwnd: HWND, outcome: Outcome) {
    match outcome {
        Outcome::Stay => redraw(hwnd),
        Outcome::Close => {
            with_panel(Panel::close_dropdown);
            redraw(hwnd);
        }
        Outcome::Write(key, value) => write_font(hwnd, &key, value),
        Outcome::Pick(key, value) => {
            with_panel(Panel::close_dropdown);
            write_font(hwnd, &key, value);
        }
    }
}

/// A key while the font dropdown is open. It takes all of them.
fn dropdown_key(hwnd: HWND, key: u16) {
    let key = match key {
        k if k == VK_UP.0 => fontmenu::Key::Up,
        k if k == VK_DOWN.0 => fontmenu::Key::Down,
        k if k == VK_PRIOR.0 => fontmenu::Key::PageUp,
        k if k == VK_NEXT.0 => fontmenu::Key::PageDown,
        k if k == VK_HOME.0 => fontmenu::Key::Home,
        k if k == VK_END.0 => fontmenu::Key::End,
        k if k == VK_LEFT.0 => fontmenu::Key::Left,
        k if k == VK_RIGHT.0 => fontmenu::Key::Right,
        k if k == VK_RETURN.0 => fontmenu::Key::Enter,
        k if k == VK_ESCAPE.0 => fontmenu::Key::Esc,
        k if k == VK_BACK.0 => fontmenu::Key::Backspace,
        _ => return,
    };
    if toggles_geom().is_some() {
        let flip = PANEL.with_borrow_mut(|p| p.as_mut().and_then(|p| p.toggles_key(key, &STYLE.get().show)));
        return toggles_flip(hwnd, flip);
    }
    let outcome = PANEL.with_borrow_mut(|p| p.as_mut().map(|p| p.dropdown_key(key, &STYLE.get().fonts)));
    dropdown_outcome(hwnd, outcome.unwrap_or(Outcome::Stay));
}

/// A left click while the dropdown is open: an entry picks it, an arrow on SIZE or WEIGHT steps
/// it, and a click outside closes the dropdown without reaching whatever is under it.
fn dropdown_click(hwnd: HWND, x: f32, y: f32) {
    if let Some((slot, count)) = toggles_geom() {
        let flip = match LAYOUT.get().toggles_hit(slot, count, x, y) {
            Some(DropHit::Item(n)) => PANEL.with_borrow_mut(|p| p.as_mut().and_then(|p| p.toggles_click(n, &STYLE.get().show))),
            Some(_) => None,
            None => {
                with_panel(Panel::close_dropdown);
                None
            }
        };
        return toggles_flip(hwnd, flip);
    }
    let Some(geom) = dropdown_geom() else { return };
    let hit = LAYOUT.get().dropdown_hit(geom, x, y);
    let fonts = STYLE.get().fonts;
    let step = |focus: DropFocus, dir: i32| {
        let step = PANEL.with_borrow_mut(|p| {
            let dropdown = p.as_mut()?.dropdown.as_mut()?;
            dropdown.focus_row(focus);
            (dir != 0).then(|| dropdown.step(&fonts, dir)).flatten()
        });
        step.map_or(Outcome::Stay, |(key, value)| Outcome::Write(key, value))
    };
    let outcome = match hit {
        Some(DropHit::Item(n)) => PANEL.with_borrow_mut(|p| p.as_mut().map(|p| p.dropdown_click(n))).unwrap_or(Outcome::Stay),
        Some(DropHit::Size(dir)) => step(DropFocus::Size, dir),
        Some(DropHit::Weight(dir)) => step(DropFocus::Weight, dir),
        // The row it hangs from, the filter and the gaps between entries.
        Some(DropHit::Anchor | DropHit::Filter | DropHit::Inside) => Outcome::Stay,
        None => Outcome::Close,
    };
    dropdown_outcome(hwnd, outcome);
}

/// Enter while typing: applies what was typed to the row it was typed into.
fn finish_edit(hwnd: HWND) {
    match PANEL.with_borrow(|p| p.as_ref().map(|p| p.edit_kind)) {
        Some(EditKind::Name) => finish_theme_name(hwnd),
        Some(EditKind::Text) => finish_header_text(hwnd),
        Some(EditKind::Keyword) => match PANEL.with_borrow_mut(|p| p.as_mut().and_then(Panel::plugin_keyword)) {
            Some(write) => write_plugin(hwnd, write),
            // Rejected: typing goes on, the note says why.
            None => redraw(hwnd),
        },
        Some(EditKind::Setting) => match PANEL.with_borrow_mut(|p| p.as_mut().and_then(Panel::plugin_text)) {
            Some(write) => write_plugin(hwnd, write),
            None => redraw(hwnd),
        },
        _ => finish_hex(hwnd),
    }
}

/// Enter while typing a hex color: stores it, or keeps the old color and flags the input.
fn finish_hex(hwnd: HWND) {
    let typed = PANEL.with_borrow_mut(|p| p.as_mut().and_then(|p| Some((p.finish_edit()?, p.current()?))));
    if let Some((text, Row::Color(index))) = typed {
        match style::normalize_hex(&text) {
            Some(hex) => return write_color(hwnd, index, Some(hex)),
            None => with_panel(|p| p.invalid_hex = true),
        }
    }
    redraw(hwnd);
}

/// Enter after typing the TITLE / SUBTITLE: stored as a `[text]` override, or removed when empty.
fn finish_header_text(hwnd: HWND) {
    let typed = PANEL.with_borrow_mut(|p| p.as_mut().and_then(|p| Some((p.finish_edit()?, p.current()?))));
    match typed {
        Some((text, row)) if !text.trim().is_empty() => set_text(hwnd, row, Some(text)),
        Some((_, row)) => set_text(hwnd, row, None),
        None => redraw(hwnd),
    }
}

/// Writes (or, with `None`, removes) the `[text]` override behind a TITLE / SUBTITLE row.
fn set_text(hwnd: HWND, row: Row, text: Option<String>) {
    let id = match row {
        Row::Title => "header.title",
        Row::Subtitle => "header.subtitle",
        _ => return,
    };
    let new = match text {
        Some(text) => config::set_value("text", id, text.into()),
        None => config::remove_value("text", id),
    };
    match new {
        Some(new) => {
            with_panel(|p| p.error = false);
            apply_config(hwnd, new);
        }
        None => with_panel(|p| p.error = true),
    }
    redraw(hwnd);
}

/// Enter after typing a theme name: saves the current colors under it, then switches to it.
/// style.toml's overrides move into the theme file, so they are cleared; nothing is touched
/// unless both config.toml and style.toml are fine to edit.
fn finish_theme_name(hwnd: HWND) {
    let Some(name) = PANEL.with_borrow_mut(|p| match p.as_mut()?.check_name() {
        NameCheck::Save(name) => Some(name),
        NameCheck::Rejected | NameCheck::Confirm => None,
    }) else {
        return redraw(hwnd);
    };
    let (roles, current) = (CONFIG.with_borrow(|c| c.colors), STYLE.get());
    let (style_ok, config_ok) = (style::editable(), config::editable());
    let saved = style_ok && config_ok && themes::write(&name, &themes::to_text(&name, &roles, &current));
    let switched = saved.then(|| config::set_value("colors", "theme", name.as_str().into())).flatten();
    with_panel(|p| {
        p.style_error = !style_ok;
        p.error = !config_ok || (saved && switched.is_none());
        if style_ok && config_ok && !saved {
            p.flash = Note::ThemeNotSaved;
        }
        p.themes = themes::list();
    });
    if let Some(new) = switched {
        // Its colors are in the theme now; leftovers in style.toml would only hide later theme edits.
        let cleared = style::clear_overrides();
        with_panel(|p| p.style_error = !cleared);
        apply_config(hwnd, new);
    }
    redraw(hwnd);
}

/// Second Delete on a custom theme: removes its file and goes back to the default theme.
fn delete_theme(hwnd: HWND, name: &str) {
    if !themes::delete(name) {
        with_panel(|p| p.flash = Note::ThemeNotSaved);
        return redraw(hwnd);
    }
    with_panel(|p| p.themes = themes::list());
    match config::set_value("colors", "theme", "cyberpunk".into()) {
        Some(new) => apply_config(hwnd, new),
        None => with_panel(|p| p.error = true),
    }
    redraw(hwnd);
}

fn open_config_file(hwnd: HWND) {
    if let Some(path) = config::ensure_file() {
        apps::open_path(&path);
        unsafe { hide(hwnd) };
    }
}

/// style.toml is only ever created here, from a template listing every key with its current value.
fn open_style_file(hwnd: HWND) {
    if let Some(path) = CONFIG.with_borrow(|c| style::ensure_file(&c.defaults())) {
        apps::open_path(&path);
        unsafe { hide(hwnd) };
    }
}

/// Keys while the right-click popup is open: it takes all of them.
unsafe fn popup_key(hwnd: HWND, key: u16) -> bool {
    if POPUP.get().is_none() {
        return false;
    }
    match key {
        k if k == VK_RETURN.0 => popup_run(hwnd, POPUP_ROW.get()),
        k if k == VK_ESCAPE.0 => {
            POPUP.set(None);
            redraw(hwnd);
        }
        k if k == VK_UP.0 || k == VK_DOWN.0 => {
            let step = if k == VK_UP.0 { POPUP_ENTRIES - 1 } else { 1 };
            let row = (POPUP_ROW.get() + step) % POPUP_ENTRIES;
            POPUP_ROW.set(row);
            set_hover(hwnd, Some(Hover::Popup(row)));
        }
        _ => {}
    }
    true
}

/// Runs the right-click popup's entry `row` (0 = PASTE, 1 = SETTINGS) and closes the popup.
fn popup_run(hwnd: HWND, row: usize) {
    POPUP.set(None);
    if row == 0 {
        paste_query(hwnd);
    } else {
        open_panel(hwnd);
    }
}

/// Ctrl+V and Shift+Insert in the list, and PASTE in the popup: the clipboard's text is typed into the query.
fn paste_query(hwnd: HWND) {
    let text = commands::clipboard_text().unwrap_or_default();
    let mut pasted = false;
    eased_list_change(hwnd, || pasted = MENU.with_borrow_mut(|m| m.paste(&text)));
    if pasted {
        query_edited(hwnd);
    } else {
        redraw(hwnd);
    }
}

/// The caret keys and mouse act on the query: something is typed and the action list isn't covering it.
fn query_editing() -> bool {
    MENU.with_borrow(|m| m.actions.is_none() && !m.query.is_empty())
}

/// Caret and selection changes: nothing is searched again.
fn caret_edit(hwnd: HWND, change: impl FnOnce(&mut LineEdit)) {
    MENU.with_borrow_mut(|m| change(&mut m.query));
    redraw(hwnd);
}

/// An edit of the query text; `change` says whether it altered anything.
fn text_edit(hwnd: HWND, change: impl FnOnce(&mut LineEdit) -> bool) {
    let mut changed = false;
    eased_list_change(hwnd, || changed = MENU.with_borrow_mut(|m| m.edit(change)));
    if changed {
        query_edited(hwnd);
    } else {
        redraw(hwnd);
    }
}

/// Ctrl+C: the selected part of the query, else the selected plugin row's copy text.
fn copy_query_or_row(hwnd: HWND) {
    let selected = MENU.with_borrow(|m| if m.actions.is_none() { m.query.selected().map(str::to_owned) } else { None });
    match selected {
        Some(text) => commands::copy(&text),
        None => copy_plugin_row(hwnd),
    }
}

/// Ctrl+X: the selected part of the query goes to the clipboard.
fn cut_query(hwnd: HWND) {
    let mut cut = None;
    text_edit(hwnd, |q| {
        cut = q.cut();
        cut.is_some()
    });
    if let Some(text) = cut {
        commands::copy(&text);
    }
}

/// Delete has something to remove: a selection, or a character after the caret.
fn can_delete_forward() -> bool {
    MENU.with_borrow(|m| m.actions.is_none() && !m.query.is_empty() && (!m.query.at_end() || m.query.selection().is_some()))
}

/// Query character under (`nearest` false) or nearest to window x `x`.
fn query_char_at(x: f32, nearest: bool) -> usize {
    let left = LAYOUT.get().query_left();
    let (query, scroll, shift) = MENU.with_borrow(|m| (m.query.as_str().to_owned(), m.query_scroll.get(), m.query_x.get()));
    RENDERER.with_borrow(|r| r.as_ref().map_or(query.chars().count(), |r| r.query_caret_at(&query, x - left - shift, nearest, scroll)))
}

/// Whether a left click at (x, y) belongs to the query text (no popup, panel typing or power overlay in the way).
fn on_query_text(x: f32, y: f32) -> bool {
    let panel_typing = PANEL.with_borrow(|p| p.as_ref().is_some_and(Panel::typing));
    !in_power_overlay() && POPUP.get().is_none() && !panel_typing && query_editing() && LAYOUT.get().query_hit(x, y)
}

/// Keys while the settings panel is open: it takes all of them.
unsafe fn panel_key(hwnd: HWND, key: u16, ctrl: bool) -> bool {
    if PANEL.with_borrow(Option::is_none) {
        return false;
    }
    if wizard_active() {
        wizard_key(hwnd, key);
        return true;
    }
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    with_panel(|p| {
        p.invalid_hex = false;
        p.flash = Note::None;
    });
    if dropdown_open() {
        dropdown_key(hwnd, key);
        return true;
    }
    if PANEL.with_borrow(|p| p.as_ref().is_some_and(|p| p.editing.is_some())) {
        match key {
            k if k == VK_ESCAPE.0 => with_panel(Panel::abort_edit),
            k if k == VK_RETURN.0 => {
                finish_edit(hwnd);
                return true;
            }
            k if k == VK_BACK.0 => with_panel(Panel::edit_backspace),
            k if (k == u16::from(b'V') && ctrl) || (k == VK_INSERT.0 && shift) => {
                if let Some(text) = commands::clipboard_text() {
                    with_panel(|p| p.paste(&text));
                }
            }
            _ => {}
        }
        redraw(hwnd);
        return true;
    }
    // A question only stands for the very next key: Delete again for a theme, Enter again for a plugin.
    with_panel(|p| {
        let asked_again = if matches!(p.pending, Some(Pending::Approve(_))) { key == VK_RETURN.0 } else { key == VK_DELETE.0 };
        if !asked_again {
            p.pending = None;
        }
    });
    let (row, button) = PANEL.with_borrow(|p| p.as_ref().map_or((None, None), |p| (p.current(), p.button())));
    match key {
        k if k == VK_ESCAPE.0 => close_panel(hwnd),
        k if k == VK_LEFT.0 => change_setting(hwnd, -1),
        k if k == VK_RIGHT.0 => change_setting(hwnd, 1),
        k if k == VK_RETURN.0 && button == Some(Button::Config) => open_config_file(hwnd),
        k if k == VK_RETURN.0 && button == Some(Button::Style) => open_style_file(hwnd),
        k if k == VK_RETURN.0 && row == Some(Row::Hotkey) => begin_capture(hwnd),
        k if k == VK_RETURN.0 && row.is_some_and(Row::opens_dropdown) => open_dropdown(hwnd),
        k if k == VK_RETURN.0 && PANEL.with_borrow(|p| p.as_ref().is_some_and(Panel::edits)) => {
            with_panel(Panel::begin_edit);
            redraw(hwnd);
        }
        k if k == VK_RETURN.0 && row.is_some_and(Row::presses) => press_row(hwnd),
        k if k == VK_RETURN.0 => change_setting(hwnd, 1),
        k if k == VK_DELETE.0 => match row {
            Some(Row::Color(index)) => write_color(hwnd, index, None),
            Some(Row::Shape(index)) => {
                if let Some(&key) = style::SHAPE_KEYS.get(index) {
                    write_shape(hwnd, key, None);
                }
            }
            Some(Row::Show(index)) => {
                if let Some(&key) = style::SHOW_KEYS.get(index) {
                    write_show(hwnd, &[(key, None)]);
                }
            }
            Some(Row::HeaderToggles) => {
                let reset: Vec<_> = style::SHOW_HEADER.map(|i| (style::SHOW_KEYS[i], None)).collect();
                write_show(hwnd, &reset);
            }
            Some(Row::FontFamily | Row::FontRole(_)) => {
                let keys = PANEL.with_borrow(|p| p.as_ref().map(Panel::font_resets).unwrap_or_default());
                for key in keys {
                    write_font(hwnd, &key, None);
                }
            }
            Some(Row::Plugin(_, PluginField::Keyword)) => {
                if let Some(write) = PANEL.with_borrow(|p| p.as_ref().and_then(Panel::plugin_keyword_reset)) {
                    write_plugin(hwnd, write);
                }
            }
            Some(text_row @ (Row::Title | Row::Subtitle)) => set_text(hwnd, text_row, None),
            Some(Row::Theme) => {
                let step = PANEL.with_borrow_mut(|p| CONFIG.with_borrow(|c| p.as_mut().map(|p| p.request_delete(c))));
                match step {
                    Some(DeleteStep::Confirm) => redraw(hwnd),
                    Some(DeleteStep::Remove(name)) => delete_theme(hwnd, &name),
                    _ => {}
                }
            }
            _ => {}
        },
        k if k == VK_PRIOR.0 || k == VK_NEXT.0 => {
            with_panel(|p| p.move_selection(if k == VK_PRIOR.0 { -(SLOTS as i32 - 1) } else { SLOTS as i32 - 1 }));
            redraw(hwnd);
        }
        k if k == VK_TAB.0 && ctrl => {
            with_panel(|p| p.switch_tab(if shift { -1 } else { 1 }));
            redraw(hwnd);
        }
        k if k == VK_UP.0 || k == VK_DOWN.0 => {
            with_panel(|p| p.move_selection(if k == VK_UP.0 { -1 } else { 1 }));
            redraw(hwnd);
        }
        _ => {}
    }
    true
}

/// A left click inside the panel card.
fn panel_click(hwnd: HWND, x: f32, y: f32) {
    if wizard_active() {
        return wizard_click(hwnd, x, y);
    }
    let hit = LAYOUT.get().panel_hit(x, y, &panel_slots());
    // A click names a screen slot; the settings row in it (headings have none) is what gets selected.
    let row_in = |slot: usize| PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.row_at(slot)));
    let is_hotkey = |row: usize| PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.rows().get(row).copied())) == Some(Row::Hotkey);
    let opens_dropdown = |row: usize| PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.rows().get(row).copied())).is_some_and(Row::opens_dropdown);
    match hit {
        Some(PanelHit::Tab(i)) => with_panel(|p| p.select_tab(i)),
        // The font rows have no arrows: a click anywhere on one opens its dropdown.
        Some(PanelHit::Row(slot) | PanelHit::Arrow(slot, _)) if row_in(slot).is_some_and(opens_dropdown) => {
            let row = row_in(slot).unwrap_or_default();
            with_panel(|p| p.select(row));
            return open_dropdown(hwnd);
        }
        Some(PanelHit::Row(slot) | PanelHit::Arrow(slot, _)) if row_in(slot).is_some_and(is_hotkey) => {
            let row = row_in(slot).unwrap_or_default();
            with_panel(|p| p.select(row));
            return begin_capture(hwnd);
        }
        Some(PanelHit::Row(slot)) => {
            if let Some(row) = row_in(slot) {
                with_panel(|p| {
                    p.select(row);
                    // A click on SAVE AS THEME is the button press: it asks for the name right away.
                    if p.current() == Some(Row::SaveTheme) {
                        p.begin_edit();
                    }
                });
                // Likewise for the rows that are buttons (OPEN LOG, APPROVE...).
                if PANEL.with_borrow(|p| p.as_ref().and_then(Panel::current)).is_some_and(Row::presses) {
                    return press_row(hwnd);
                }
            }
        }
        Some(PanelHit::Arrow(slot, dir)) => {
            let Some(row) = row_in(slot) else { return };
            with_panel(|p| p.select(row));
            return change_setting(hwnd, dir);
        }
        Some(PanelHit::Close) => return close_panel(hwnd),
        Some(PanelHit::OpenConfig) => return open_config_file(hwnd),
        Some(PanelHit::OpenStyle) => return open_style_file(hwnd),
        Some(PanelHit::Back | PanelHit::Skip | PanelHit::Next) | None => {}
    }
    redraw(hwnd);
}

/// Double click on a COLORS row: selects it and starts hex typing, like Enter. False for any other click.
fn panel_double_click(hwnd: HWND, x: f32, y: f32) -> bool {
    if POPUP.get().is_some() || dropdown_open() {
        return false;
    }
    let Some(PanelHit::Row(slot)) = LAYOUT.get().panel_hit(x, y, &panel_slots()) else { return false };
    let Some(row) = PANEL.with_borrow(|p| p.as_ref().and_then(|p| p.row_at(slot))) else { return false };
    if !PANEL.with_borrow(|p| p.as_ref().is_some_and(|p| p.edits_row(row))) {
        return false;
    }
    with_panel(|p| {
        p.select(row);
        p.begin_edit();
    });
    redraw(hwnd);
    true
}

/// → on a row: replaces the list with its actions, replaying the open animation, or with
/// `[layout] actions = "popout"` opens them as a menu beside the list (at `cursor` if the
/// anchor says so). False when the row has none.
fn open_actions(hwnd: HWND, cursor: Option<(f32, f32)>) -> bool {
    if MENU.with_borrow(|m| m.popout().is_some()) {
        return false;
    }
    let target = MENU.with_borrow(|m| m.current().map(|i| (m.items[i].tag, m.items[i].id.clone())));
    let Some((tag, id)) = target else { return false };
    // Resolved only here, not for every row: it can ask the shell for a shortcut's target.
    let path = match tag {
        Tag::App | Tag::Pinned => actions::app_path(&id),
        Tag::File | Tag::Folder => Some(id),
        _ => None,
    };
    let opened = MENU.with_borrow_mut(|m| m.open_actions(path));
    if opened && CONFIG.with_borrow(|c| c.layout.actions == ActionsMode::Popout) {
        let labels = MENU.with_borrow_mut(|m| {
            m.set_popout();
            m.popout().map(|a| a.labels()).unwrap_or_default()
        });
        let ts = LAYOUT.get().ts;
        let width = RENDERER.with_borrow(|r| r.as_ref().map(|r| r.popout_width(&labels, ts)));
        POPOUT_W.set(width.unwrap_or_else(|| Layout::popout_width(0.0, ts)));
        POPOUT_CURSOR.set(cursor);
        animate(hwnd);
        redraw(hwnd);
    } else if opened {
        OPEN_START.set(unsafe { GetTickCount64() });
        FRAME_NO.set(0);
        animate(hwnd);
    }
    opened
}

/// Right click on list row `row`: selects it and opens its actions. A row without actions
/// leaves the selection where it was.
fn open_actions_at(hwnd: HWND, row: usize, cursor: Option<(f32, f32)>) {
    let before = MENU.with_borrow(|m| m.selected);
    if MENU.with_borrow_mut(|m| m.select_row(row)) && !open_actions(hwnd, cursor) {
        MENU.with_borrow_mut(|m| m.select_row(before));
    }
}

unsafe fn run_action(hwnd: HWND, row: usize) {
    let plugin = MENU.with_borrow(|m| {
        let a = m.actions.as_ref()?;
        Some((a.row.as_deref()?.clone(), a.plugin.get(row)?.kind.clone(), a.id.clone()))
    });
    if let Some((snapshot, kind, item_id)) = plugin {
        match kind {
            PluginActKind::HostCopy => {
                if let Some(text) = snapshot.host_copy() {
                    commands::copy(text);
                }
                unsafe { hide(hwnd) };
            }
            PluginActKind::Action(id) => {
                if let Some(action) = snapshot.item.actions.iter().find(|a| a.id == id) {
                    invoke_plugin(hwnd, &snapshot, &item_id, action);
                }
            }
        }
        return;
    }
    let chosen = MENU.with_borrow(|m| {
        let a = m.actions.as_ref()?;
        Some((*a.acts.get(row)?, a.tag, a.id.clone(), a.name.clone(), a.path.clone()))
    });
    let Some((act, tag, id, name, path)) = chosen else { return };
    let path = path.unwrap_or_default();
    match act {
        Act::Run | Act::Open => return unsafe { run_item(hwnd, tag, &id, &name, false) },
        Act::Pin | Act::Unpin => {
            MENU.with_borrow_mut(Menu::close_actions);
            return pin(hwnd, &name);
        }
        Act::RunAdmin => actions::run_admin(&id, (!path.is_empty()).then_some(path.as_str())),
        Act::OpenLocation => actions::open_location(&path),
        Act::CopyPath => commands::copy(&path),
        Act::CopyName => commands::copy(actions::file_name(&path)),
        Act::Terminal => actions::open_terminal(&path),
        Act::Properties => actions::properties(&path),
        Act::Uninstall => actions::uninstall(),
        Act::Plugin => return,
    }
    unsafe { hide(hwnd) };
}

fn now() -> u64 {
    unsafe { GetTickCount64() }
}

fn in_power_overlay() -> bool {
    POWER_SEQ.with_borrow(Option::is_some)
}

fn run_power(hwnd: HWND, action: Action) {
    POWER_SEQ.set(None);
    // Hide first: the menu must not be on screen when the session locks or sleeps.
    unsafe { hide(hwnd) };
    action.run();
}

/// Presses a power button: the window grows over the whole monitor and glitches; the action then
/// either runs (no confirmation configured for it) or waits for EXECUTE in the dialog.
unsafe fn press_power(hwnd: HWND, button: usize) {
    POWER.with_borrow_mut(|p| p.focused = Some(button));
    let Some(&action) = ACTIONS.get(button) else { return };
    if in_power_overlay() {
        return;
    }
    let confirm = CONFIG.with_borrow(|c| c.confirm_power.contains(&action));
    // Without the settings panel, so the menu is one plain rectangle inside the big window.
    close_overlays();
    place();
    POWER_SEQ.set(Some(Sequence::new(action, confirm, now())));
    FRAME_NO.set(0);
    let mon = MONITOR.get();
    unsafe {
        let _ = KillTimer(Some(hwnd), ANIM_TIMER);
        let _ = KillTimer(Some(hwnd), SEARCH_TIMER);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), mon.left, mon.top, mon.right - mon.left, mon.bottom - mon.top, SWP_NOACTIVATE);
        SetTimer(Some(hwnd), OVERLAY_TIMER, FRAME_MS, None);
    }
    redraw(hwnd);
}

/// One frame of the glitch; stops the timer once the overlay settles, and acts on the end of a
/// glitch (run the action, or restore the menu after ABORT).
fn power_tick(hwnd: HWND) {
    let t = now();
    let state = POWER_SEQ.with_borrow(|s| s.as_ref().map(|s| (s.phase(t), s.action(), s.animating(t))));
    match state {
        Some((Phase::Executing, action, _)) => return run_power(hwnd, action),
        Some((Phase::Done, ..)) => return end_power(hwnd),
        Some((_, _, true)) => {
            FRAME_NO.set(FRAME_NO.get() + 1);
            redraw(hwnd);
            return;
        }
        _ => {}
    }
    // Settled in the dialog (one last static frame), or the overlay is already gone.
    unsafe {
        let _ = KillTimer(Some(hwnd), OVERLAY_TIMER);
    }
    redraw(hwnd);
}

/// Enter / EXECUTE in the dialog.
fn execute_power(hwnd: HWND) {
    let action = POWER_SEQ.with_borrow_mut(|s| {
        let s = s.as_mut()?;
        s.execute(now()).then(|| s.action())
    });
    if let Some(action) = action {
        run_power(hwnd, action);
    }
}

/// Esc / ABORT / click outside: plays the reverse glitch, then `end_power` restores the menu.
fn cancel_power(hwnd: HWND) {
    if POWER_SEQ.with_borrow_mut(|s| s.as_mut().is_some_and(|s| s.cancel(now()))) {
        HOVER.set(None);
        unsafe { SetTimer(Some(hwnd), OVERLAY_TIMER, FRAME_MS, None) };
        redraw(hwnd);
    }
}

/// Back to the normal menu window; the monitor-sized DIB is released first.
fn end_power(hwnd: HWND) {
    POWER_SEQ.set(None);
    unsafe {
        let _ = KillTimer(Some(hwnd), OVERLAY_TIMER);
    }
    HOVER.set(None);
    RENDERER.with_borrow_mut(|r| r.as_mut().map(Renderer::release));
    // The open animation may have been cut short by the overlay.
    animate(hwnd);
    resize(hwnd);
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
}

/// The dialog element under a point, while the dialog is waiting for a decision.
fn dialog_at(x: f32, y: f32) -> Option<DialogHit> {
    let waiting = POWER_SEQ.with_borrow(|s| s.as_ref().is_some_and(|s| s.phase(now()) == Phase::Confirm));
    let mon = MONITOR.get();
    waiting.then(|| Dialog::new((mon.right - mon.left) as f32, (mon.bottom - mon.top) as f32, LAYOUT.get().ts).hit(x, y))
}

/// Keys in the confirm dialog: arrows and Tab move the focus, Enter presses the focused button
/// (never on auto-repeat, so Enter still held from the power bar can't confirm), Esc aborts.
fn dialog_key(hwnd: HWND, key: u16, lparam: LPARAM) {
    let nav = match key {
        k if k == VK_LEFT.0 => Some(Nav::Left),
        k if k == VK_RIGHT.0 => Some(Nav::Right),
        k if k == VK_TAB.0 => Some(Nav::Switch),
        _ => None,
    };
    if let Some(nav) = nav {
        POWER_SEQ.with_borrow_mut(|s| s.as_mut().map(|s| s.navigate(nav)));
        redraw(hwnd);
    } else if key == VK_RETURN.0 && !is_auto_repeat(lparam.0) {
        match POWER_SEQ.with_borrow(|s| s.as_ref().map(Sequence::focus)) {
            Some(Focus::Execute) => execute_power(hwnd),
            Some(Focus::Abort) => cancel_power(hwnd),
            None => {}
        }
    } else if key == VK_ESCAPE.0 {
        cancel_power(hwnd);
    }
}

/// Keys while the power bar has focus. Returns false for keys that should go to the list instead
/// (focus then returns to the list). Tab / Shift+Tab walk the buttons and leave the bar at either end.
unsafe fn power_key(hwnd: HWND, key: u16, shift: bool) -> bool {
    match key {
        k if k == VK_LEFT.0 => POWER.with_borrow_mut(|p| p.move_focus(-1)),
        k if k == VK_RIGHT.0 => POWER.with_borrow_mut(|p| p.move_focus(1)),
        k if k == VK_TAB.0 => POWER.with_borrow_mut(|p| p.tab(shift)),
        k if k == VK_RETURN.0 => {
            if let Some(button) = POWER.with_borrow(|p| p.focused) {
                unsafe { press_power(hwnd, button) };
            }
            return true;
        }
        k if k == VK_ESCAPE.0 => POWER.with_borrow_mut(|p| p.focused = None),
        _ => {
            POWER.with_borrow_mut(PowerBar::reset);
            redraw(hwnd);
            return false;
        }
    }
    redraw(hwnd);
    true
}

unsafe fn finish_index(hwnd: HWND) {
    if let Some(mut child) = INDEXER.take() {
        let _ = child.wait();
    }
    load_index();
    unsafe {
        if IsWindowVisible(hwnd).as_bool() {
            redraw(hwnd);
        } else {
            let _ = EmptyWorkingSet(GetCurrentProcess());
        }
    }
}

/// `elevated` (Ctrl+Enter) runs a `>` command as administrator.
unsafe fn launch(hwnd: HWND, index: Option<usize>, elevated: bool) {
    // Ctrl+Enter on a plugin row is its `alt` action.
    let plugin = MENU.with_borrow(|m| {
        let i = index?;
        Some((m.plugin_row(i)?.clone(), m.items[i].id.clone()))
    });
    if let Some((row, id)) = plugin {
        return plugin_enter(hwnd, &row, &id, elevated);
    }
    let target = MENU.with_borrow(|m| index.and_then(|i| m.items.get(i)).map(|item| (item.tag, item.id.clone(), item.name.clone())));
    if let Some((tag, id, name)) = target {
        unsafe { run_item(hwnd, tag, &id, &name, elevated) };
    }
}

/// Remembers a launch from the list for ranking and RECENT; rows without a stable id are skipped.
fn record_launch(tag: Tag, id: &str) {
    let Some(kind) = usage::kind(tag) else { return };
    MENU.with_borrow_mut(|m| {
        let query = m.query.as_str().to_owned();
        m.usage.record(kind, id, &query, usage::now());
        m.usage.save();
    });
}

unsafe fn run_item(hwnd: HWND, tag: Tag, id: &str, name: &str, elevated: bool) {
    record_launch(tag, id);
    // Launch while still foreground so the new app is allowed to take focus.
    match tag {
        Tag::App | Tag::Pinned => apps::launch(id),
        Tag::File | Tag::Folder | Tag::Open => apps::open_path(id),
        Tag::Web => commands::web_search(name, id),
        Tag::Setting => apps::launch_setting(id),
        Tag::Shell => commands::run_shell(id, elevated),
        Tag::Calc => commands::copy(id),
        Tag::Action | Tag::Plugin(_) => return,
    }
    unsafe { hide(hwnd) };
}

/// Selection position and row count of the list on screen (the action list while it is open).
fn list_state() -> (f32, usize) {
    MENU.with_borrow(|m| match m.list_actions() {
        Some(a) => (a.scroll, a.acts.len()),
        None => (m.scroll, m.view.len()),
    })
}

/// Runs `change`; if it changed how many rows the list has, the list keeps drawing where it was
/// and eases to its new position (short lists are centred, band positions depend on the count).
fn eased_list_change(hwnd: HWND, change: impl FnOnce()) {
    let (_, old_count) = list_state();
    change();
    let (scroll, count) = list_state();
    if count != old_count {
        let mut layout = LAYOUT.get();
        let sliding = layout.retarget(old_count, scroll, count);
        LAYOUT.set(layout);
        if sliding {
            animate(hwnd);
        }
    }
}

/// Called after every query edit: redraw now, ask Everything and the plugins once typing pauses.
fn query_edited(hwnd: HWND) {
    redraw(hwnd);
    unsafe { SetTimer(Some(hwnd), SEARCH_TIMER, search_delay(), None) };
}

/// The typing pause before searching: a keyword plugin's own `debounce_ms`, else 120 ms or the
/// slowest global plugin's debounce if that is larger.
fn search_delay() -> u32 {
    let keyword = MENU.with_borrow(|m| m.keyword().map(|(slot, _)| slot));
    PLUGINS.with_borrow(|h| match keyword {
        Some(slot) => h.debounce_ms(slot).max(10),
        None => h.global_targets().into_iter().map(|slot| h.debounce_ms(slot)).fold(SEARCH_DEBOUNCE_MS, u32::max),
    })
}

fn push_plugin_keywords() {
    let table = PLUGINS.with_borrow(Host::keyword_table);
    MENU.with_borrow_mut(|m| m.set_plugin_keywords(table));
}

fn is_visible(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

/// Puts `rows` of `slot` into the list for `query`, easing the list like any other result arrival.
fn show_plugin_rows(hwnd: HWND, query: &str, slot: u16, rows: Vec<Item>, each: usize, total: usize) {
    eased_list_change(hwnd, || MENU.with_borrow_mut(|m| m.set_plugin_rows(query, slot, rows, each, total)));
    // Row indexes moved, and the renderer caches icons by index.
    RENDERER.with_borrow(|r| r.as_ref().map(Renderer::clear_icons));
    redraw(hwnd);
}

/// SEARCH_TIMER: sends the query to the keyword plugin, or to the global plugins, or shows the
/// first-run confirmation / failure row. The UI thread only queues lines; nothing waits.
fn search_plugins(hwnd: HWND) {
    let (query, keyword, plain) = MENU.with_borrow(|m| (m.query.as_str().to_owned(), m.keyword().map(|(slot, text)| (slot, text.to_owned())), m.is_plain()));
    let t = now();
    if let Some((slot, text)) = keyword {
        match PLUGINS.with_borrow(|h| h.status(slot)) {
            Status::Ready => {
                let typed = query.split_whitespace().next().unwrap_or("").to_lowercase();
                PLUGINS.with_borrow_mut(|h| {
                    let id = h.new_batch(&query);
                    h.send_query(slot, id, &text, &typed, false, t);
                });
                arm_plugin_timer(hwnd);
            }
            Status::NeedsApproval => {
                let rows = PLUGINS.with_borrow(|h| h.approval_rows(slot));
                show_plugin_rows(hwnd, &query, slot, rows, 2, 2);
            }
            Status::Failed => {
                let rows = vec![PLUGINS.with_borrow(|h| h.failed_row(slot))];
                show_plugin_rows(hwnd, &query, slot, rows, 1, 1);
            }
            Status::Unavailable => {}
        }
        return;
    }
    if !plain || query.chars().count() < FILE_SEARCH_MIN_CHARS {
        return;
    }
    let targets = PLUGINS.with_borrow(Host::global_targets);
    if targets.is_empty() {
        return;
    }
    PLUGINS.with_borrow_mut(|h| {
        let id = h.new_batch(&query);
        for slot in targets {
            h.send_query(slot, id, &query, "", true, t);
        }
    });
    arm_plugin_timer(hwnd);
}

/// (Re)arms the one-shot timer for the next request timeout, or stops it when none is pending.
/// Only while the menu is visible: hidden, the supervisor threads enforce the hard timeout alone.
fn arm_plugin_timer(hwnd: HWND) {
    unsafe {
        let next = if is_visible(hwnd) { PLUGINS.with_borrow(|h| h.next_deadline(now())) } else { None };
        match next {
            Some(ms) => {
                SetTimer(Some(hwnd), PLUGIN_TIMER, ms.clamp(10, 10_000) as u32, None);
            }
            None => {
                let _ = KillTimer(Some(hwnd), PLUGIN_TIMER);
            }
        }
    }
}

/// PLUGIN_TIMER: a soft timeout passed (say so for the keyword plugin) or a hard one (kill it).
fn plugin_timer(hwnd: HWND) {
    let slow = PLUGINS.with_borrow_mut(|h| h.check_timeouts(now()));
    let keyword = MENU.with_borrow(|m| m.keyword().map(|(slot, _)| slot));
    if let Some(slot) = keyword.filter(|slot| slow.contains(slot)) {
        let text = lang::t("plugin.slow").replace("{name}", &PLUGINS.with_borrow(|h| h.name(slot).to_uppercase()));
        MENU.with_borrow_mut(|m| m.notice = Some(text));
        redraw(hwnd);
    }
    arm_plugin_timer(hwnd);
}

/// IDLE_TIMER (armed in `hide`): shuts down the plugins unused for a minute, re-arms for the rest.
fn idle_tick(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(Some(hwnd), IDLE_TIMER);
        if is_visible(hwnd) {
            return;
        }
        if let Some(left) = PLUGINS.with_borrow_mut(|h| h.sweep(now())) {
            SetTimer(Some(hwnd), IDLE_TIMER, left.clamp(10, IDLE_MS) as u32, None);
        }
    }
}

/// A one-line message in the header for `NOTICE_MS`.
fn show_notice(hwnd: HWND, text: &str) {
    MENU.with_borrow_mut(|m| m.notice = Some(text.chars().take(80).collect()));
    unsafe { SetTimer(Some(hwnd), NOTICE_TIMER, NOTICE_MS, None) };
    redraw(hwnd);
}

fn end_notice(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(Some(hwnd), NOTICE_TIMER);
    }
    MENU.with_borrow_mut(|m| m.notice = None);
    if is_visible(hwnd) {
        redraw(hwnd);
    }
}

/// WM_PLUGIN: a reply from a plugin's reader thread. Applied only if its generation is the
/// plugin's current one (checked by the host), the menu is showing, and it answers the query the
/// user is looking at.
unsafe fn plugin_reply(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let (generation, slot) = plugin_proc::decode_wparam(wparam.0);
    let reply = *unsafe { plugin_proc::take_reply(lparam) };
    let handled = PLUGINS.with_borrow_mut(|h| h.handle(slot, generation, reply, now()));
    let visible = is_visible(hwnd);
    match handled {
        Handled::Nothing => {}
        Handled::Results { slot, generation, id, items } if visible => apply_results(hwnd, slot, generation, id, items),
        Handled::QueryError { slot, message } if visible => apply_query_error(hwnd, slot, &message),
        Handled::Results { .. } | Handled::QueryError { .. } => {}
        Handled::Done { effects, keep_open, error, item_id, stay, .. } => plugin_done(hwnd, &effects, keep_open, error, &item_id, stay),
        Handled::Exited { slot, failed } => {
            MENU.with_borrow_mut(Menu::clear_busy);
            let showing = MENU.with_borrow(|m| m.keyword().is_some_and(|(s, _)| s == slot));
            if visible && failed && showing {
                let (query, row) = (MENU.with_borrow(|m| m.query.as_str().to_owned()), PLUGINS.with_borrow(|h| h.failed_row(slot)));
                show_plugin_rows(hwnd, &query, slot, vec![row], 1, 1);
            } else if visible {
                redraw(hwnd);
            }
        }
    }
    if visible {
        arm_plugin_timer(hwnd);
    }
    // RUNNING, STARTING and FAILED in the PLUGINS tab follow the processes.
    if PANEL.with_borrow(|p| p.as_ref().is_some_and(|p| p.tab == Tab::Plugins)) {
        refresh_plugin_panel();
        if visible {
            redraw(hwnd);
        }
    }
}

/// Whether plugin `slot`'s answer belongs on screen: it answers the newest batch, which was sent
/// for the query now typed, and the query is that plugin's keyword (or, without a keyword, a plain
/// query that the plugin is a global target for).
fn answers_current_query(slot: u16) -> Option<(String, bool)> {
    let (query, keyword, plain) = MENU.with_borrow(|m| (m.query.as_str().to_owned(), m.keyword().map(|(s, _)| s), m.is_plain()));
    PLUGINS.with_borrow(|h| {
        let current = h.sent_for() == query;
        let belongs = match keyword {
            Some(k) => k == slot,
            None => plain && h.global_targets().contains(&slot),
        };
        (current && belongs).then_some((query, keyword.is_some()))
    })
}

fn apply_results(hwnd: HWND, slot: u16, generation: u16, id: u64, items: Vec<proto::Item>) {
    let Some((query, keyword_mode)) = answers_current_query(slot) else { return };
    let (rows, max) = PLUGINS.with_borrow(|h| (h.build_rows(slot, generation, id, items), h.max_results(slot)));
    let (each, total) = if keyword_mode { (max, max) } else { (max.min(GLOBAL_EACH), GLOBAL_TOTAL) };
    if keyword_mode {
        MENU.with_borrow_mut(|m| m.notice = None);
    }
    show_plugin_rows(hwnd, &query, slot, rows, each, total);
}

/// A plugin's `error` for a keyword query becomes one disabled row; for a global query it is only
/// in the plugin's log.
fn apply_query_error(hwnd: HWND, slot: u16, message: &str) {
    let Some((query, true)) = answers_current_query(slot) else { return };
    let row = PLUGINS.with_borrow(|h| h.notice_row(slot, message));
    show_plugin_rows(hwnd, &query, slot, vec![row], 1, 1);
}

/// Runs built-in effects. `visible`: whether the menu is on screen (a hidden menu can't have its
/// query set).
fn run_effects(hwnd: HWND, effects: &[Effect], visible: bool) {
    for effect in effects {
        match effect {
            Effect::Open(target) if plugin_host::open_allowed(target) => apps::open_path(target),
            Effect::Copy(text) => commands::copy(text),
            Effect::Run { exe, args, cwd, admin } => commands::run_program(exe, args, cwd.as_deref(), *admin),
            Effect::SetQuery(query) if visible => {
                eased_list_change(hwnd, || MENU.with_borrow_mut(|m| m.set_query(query)));
                query_edited(hwnd);
            }
            _ => {}
        }
    }
}

/// A `done` the host accepted. Without `stay` the menu was hidden when the action was sent, so
/// only the effects run; with `stay` the row stops being dimmed and the menu closes unless asked
/// to stay open or the plugin reported an error.
fn plugin_done(hwnd: HWND, effects: &[Effect], keep_open: bool, error: Option<String>, item_id: &str, stay: bool) {
    let visible = is_visible(hwnd);
    MENU.with_borrow_mut(|m| m.set_busy(item_id, false));
    run_effects(hwnd, effects, visible);
    if let Some(error) = &error
        && visible
    {
        show_notice(hwnd, error);
    }
    if stay && visible && !keep_open && error.is_none() {
        unsafe { hide(hwnd) };
    } else if visible {
        redraw(hwnd);
    }
}

/// Runs `action` of a plugin row: its own effects at once, or an `action` message to the plugin.
fn invoke_plugin(hwnd: HWND, row: &PluginRow, item_id: &str, action: &proto::Action) {
    match &action.effects {
        Some(effects) => {
            // Effects run while the menu is still foreground so launched programs may take focus.
            run_effects(hwnd, effects, true);
            if plugin_host::keeps_open(effects, action.stay) {
                MENU.with_borrow_mut(Menu::close_actions);
                redraw(hwnd);
            } else {
                unsafe { hide(hwnd) };
            }
        }
        None => {
            let sent = PLUGINS.with_borrow_mut(|h| h.send_action(row, &action.id, item_id, action.stay, now()));
            match sent {
                None => {
                    let text = lang::t("plugin.failed").replace("{name}", &row.tag);
                    show_notice(hwnd, &text);
                }
                Some(_) if action.stay => {
                    MENU.with_borrow_mut(|m| {
                        m.close_actions();
                        m.set_busy(item_id, true);
                    });
                    redraw(hwnd);
                }
                Some(_) => unsafe { hide(hwnd) },
            }
        }
    }
}

/// Enter (or Ctrl+Enter with `alt`) on a plugin row.
fn plugin_enter(hwnd: HWND, row: &PluginRow, item_id: &str, alt: bool) {
    match row.kind {
        RowKind::Notice => {}
        RowKind::Approval(true) => approve_plugin(hwnd, row.slot),
        RowKind::Approval(false) => {
            // Ignore: back to an empty query; the prompt returns the next time the keyword is typed.
            eased_list_change(hwnd, || {
                MENU.with_borrow_mut(Menu::clear_query);
            });
            query_edited(hwnd);
        }
        RowKind::Result => {
            let action = if alt { row.item.alt_action() } else { row.item.default_action() };
            if let Some(action) = action {
                invoke_plugin(hwnd, row, item_id, action);
            }
        }
    }
}

/// ALLOW: stores what was shown (version and command) as `approved` in config.toml, then asks the
/// plugin the query that triggered the prompt.
fn approve_plugin(hwnd: HWND, slot: u16) {
    let target = PLUGINS.with_borrow(|h| Some((h.plugin_id(slot).to_owned(), h.approval_key(slot)?)));
    let Some((id, key)) = target else { return };
    match config::set_plugin_value(&id, "approved", key.into()) {
        Some(new) => {
            apply_config(hwnd, new);
            let query = MENU.with_borrow(|m| m.query.as_str().to_owned());
            eased_list_change(hwnd, || MENU.with_borrow_mut(|m| m.set_query(&query)));
            query_edited(hwnd);
        }
        None => show_notice(hwnd, &lang::t("settings.note.config_error")),
    }
}

/// Ctrl+C on the selected plugin row: copies its `copy` text.
fn copy_plugin_row(hwnd: HWND) {
    let text = MENU.with_borrow(|m| m.current().and_then(|i| m.plugin_row(i)).and_then(|r| r.item.copy.clone()));
    if let Some(text) = text {
        commands::copy(&text);
        unsafe { hide(hwnd) };
    }
}

/// The selected row is a first-run confirmation (its Enter must not repeat).
fn approval_selected() -> bool {
    MENU.with_borrow(|m| m.current().and_then(|i| m.plugin_row(i)).is_some_and(|r| matches!(r.kind, RowKind::Approval(_))))
}

fn search_files(hwnd: HWND) {
    let (enabled, results) = CONFIG.with_borrow(|c| (c.file_search, c.file_results));
    let (query, wanted) = MENU.with_borrow(|m| (m.query.as_str().to_owned(), m.wants_files()));
    if !enabled || results == 0 || !wanted || query.chars().count() < FILE_SEARCH_MIN_CHARS {
        return;
    }
    FILE_QUERY.set(query.clone());
    // The reply may arrive re-entrantly inside this call (Everything answers with SendMessage),
    // so no RefCell borrow may be held here.
    if !everything::query(hwnd, &query, results) {
        winsearch::query(hwnd, &query, results);
    }
}

unsafe fn windows_search_found(hwnd: HWND, reply: *mut winsearch::Reply) {
    let winsearch::Reply { query, hits } = *unsafe { Box::from_raw(reply) };
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    let set = || MENU.with_borrow_mut(|m| m.set_files(&query, hits));
    if visible {
        eased_list_change(hwnd, set);
        redraw(hwnd);
    } else {
        set();
    }
}

unsafe fn files_found(hwnd: HWND, data: &COPYDATASTRUCT) {
    let bytes = unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize) };
    let hits = everything::parse_reply(bytes);
    let query = FILE_QUERY.with_borrow(Clone::clone);
    eased_list_change(hwnd, || MENU.with_borrow_mut(|m| m.set_files(&query, hits)));
    redraw(hwnd);
}

fn handle(hwnd: HWND, command: Command) {
    unsafe {
        match command {
            Command::Toggle if IsWindowVisible(hwnd).as_bool() => hide(hwnd),
            Command::Toggle => {
                let since_hide = GetTickCount64() - LAST_DEACTIVATE_HIDE.load(Ordering::Relaxed);
                if since_hide > REOPEN_GUARD_MS {
                    show(hwnd);
                }
            }
            Command::Show => show(hwnd),
            Command::Hide => hide(hwnd),
            Command::Quit => {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

fn open_ms() -> f32 {
    (unsafe { GetTickCount64() } - OPEN_START.get()) as f32
}

/// A frame of the full-screen power overlay: the menu sits where it was, on a monitor-sized surface.
fn redraw_power(hwnd: HWND) {
    let now = now();
    let state = POWER_SEQ.with_borrow(|s| s.as_ref().map(|s| (s.phase(now), s.action(), s.confirm(), s.focus())));
    let Some((phase, action, confirm, focus)) = state else { return };
    let (mon, position) = (MONITOR.get(), POSITION.get());
    let frame = PowerFrame {
        look: phase.look(confirm),
        action,
        screen: (mon.right - mon.left, mon.bottom - mon.top),
        origin: ((position.x - mon.left) as f32, (position.y - mon.top) as f32),
        hover: HOVER.get(),
        focus,
        frame_no: FRAME_NO.get(),
    };
    RENDERER.with_borrow_mut(|renderer| {
        let layout = LAYOUT.get();
        if renderer.is_none() {
            *renderer = Renderer::new(&layout).ok();
        }
        let Some(renderer) = renderer else { return };
        let drawn = POWER.with_borrow(|power| MENU.with_borrow(|menu| renderer.draw_power(menu, power, &STYLE.get(), &layout, &frame)));
        if drawn.is_ok() {
            let _ = renderer.present(hwnd, POINT { x: mon.left, y: mon.top });
        }
    });
}

fn redraw(hwnd: HWND) {
    if in_power_overlay() {
        return redraw_power(hwnd);
    }
    // The pop-out opened or closed: the window grows or shrinks by its column first.
    if is_visible(hwnd) && LAYOUT.get().popout_col() != CONFIG.with_borrow(|c| popout_column(&c.layout)) {
        return resize(hwnd);
    }
    RENDERER.with_borrow_mut(|renderer| {
        let layout = LAYOUT.get();
        if renderer.is_none() {
            *renderer = Renderer::new(&layout).ok();
        }
        let Some(renderer) = renderer else { return };
        let style = STYLE.get();
        let view = PANEL.with_borrow(|p| p.as_ref().map(|p| CONFIG.with_borrow(|c| p.view(c, &style, HOTKEY_FAILED.get()))));
        let overlay = Overlay { panel: view.as_ref(), popup: POPUP.get(), hover: HOVER.get(), popout: popout_params() };
        let drawn = POWER.with_borrow(|power| {
            MENU.with_borrow(|menu| renderer.draw(menu, power, &style, &layout, &overlay, open_ms(), FRAME_NO.get()))
        });
        if drawn.is_ok() {
            let _ = renderer.present(hwnd, POSITION.get());
        }
    });
}

/// (Re)arms the frame timer; it stops itself once nothing moves.
fn animate(hwnd: HWND) {
    unsafe { SetTimer(Some(hwnd), ANIM_TIMER, FRAME_MS, None) };
}

/// Computes the window rectangle for the current monitor, config and panel state (menu at the
/// configured edge, the panel column on its inner side) and stores `LAYOUT` / `POSITION`.
fn place() -> (POINT, i32, i32) {
    let (work, scale) = (WORK.get(), SCALE.get());
    let mut cfg = CONFIG.with_borrow(|c| c.layout);
    cfg.shape = STYLE.get().shape;
    cfg.show = STYLE.get().show;
    let work_w = work.right - work.left;
    let (menu_w, height) = Layout::window_size(work_w, work.bottom - work.top, scale, &cfg);
    let margin = Layout::outer_margin(scale);
    let panel_w = if PANEL.with_borrow(Option::is_some) { Layout::panel_width(work_w, menu_w, scale, Layout::fitted_scale(scale, &cfg, height)) } else { 0 };
    let menu_left = match cfg.side {
        Side::Left => work.left + margin,
        Side::Right => work.right - margin - menu_w,
    };
    let layout = Layout::new(menu_w, height, scale, &cfg).with_panel(panel_w, cfg.side == Side::Right).with_wizard(wizard_active()).with_popout(popout_column(&cfg), cfg.side == Side::Right);
    let position = POINT { x: menu_left - layout.menu_x() as i32, y: work.top + margin };
    POSITION.set(position);
    LAYOUT.set(layout);
    (position, layout.width, height)
}

/// Drops the panel, the right-click popup and the actions pop-out without touching the window size.
fn close_overlays() {
    PANEL.set(None);
    MENU.with_borrow_mut(Menu::close_popout);
    POPUP.set(None);
    HOVER.set(None);
    TRACKING.set(false);
}

/// Applies a changed window rectangle to the visible window (panel opened/closed, layout edited).
fn resize(hwnd: HWND) {
    let old = LAYOUT.get();
    let (position, width, height) = place();
    // A layout change (band, short list, rows ...) moves the list: ease it instead of jumping.
    let (scroll, count) = list_state();
    let mut layout = LAYOUT.get();
    let sliding = layout.carry_slide(&old, scroll, count);
    LAYOUT.set(layout);
    if sliding {
        animate(hwnd);
    }
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), position.x, position.y, width, height, SWP_NOACTIVATE);
    }
    redraw(hwnd);
}

unsafe fn show(hwnd: HWND) {
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        WORK.set(info.rcWork);
        MONITOR.set(info.rcMonitor);
        close_overlays();

        OPEN_START.set(GetTickCount64());
        FRAME_NO.set(0);
        MENU.with_borrow_mut(Menu::reset);
        POWER.with_borrow_mut(PowerBar::reset);
        if config::modified() != CONFIG_SEEN.get() {
            reload_config(hwnd);
            apply_pins();
        } else if style::modified() != STYLE_SEEN.get() {
            refresh_style();
        }
        begin_onboarding(hwnd);
        // The idle one-shot is only for the hidden time; the sweep covers a lost timer.
        let _ = KillTimer(Some(hwnd), IDLE_TIMER);
        PLUGINS.with_borrow_mut(|h| {
            h.rescan();
            h.sweep(now());
        });
        push_plugin_keywords();

        let (mut dpi_x, mut dpi_y) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        SCALE.set(dpi_x as f32 / 96.0);
        let (position, width, height) = place();
        if index_stale() {
            start_index();
        }
        redraw(hwnd);

        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), position.x, position.y, width, height, SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(hwnd);
        animate(hwnd);
    }
}

unsafe fn hide(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(Some(hwnd), ANIM_TIMER);
        let _ = KillTimer(Some(hwnd), SEARCH_TIMER);
        let _ = KillTimer(Some(hwnd), OVERLAY_TIMER);
        let _ = KillTimer(Some(hwnd), PLUGIN_TIMER);
        let _ = KillTimer(Some(hwnd), NOTICE_TIMER);
        if PLUGINS.with_borrow(Host::any_running) {
            SetTimer(Some(hwnd), IDLE_TIMER, IDLE_MS as u32, None);
        }
        POWER_SEQ.set(None);
        let _ = ShowWindow(hwnd, SW_HIDE);
        let was_capturing = capturing();
        close_overlays();
        if was_capturing {
            apply_hotkey(hwnd);
        }
        MENU.with_borrow_mut(Menu::reset);
        RENDERER.with_borrow_mut(|r| r.as_mut().map(Renderer::release));
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

fn navigate(hwnd: HWND, change: impl FnOnce(&mut Menu)) {
    MENU.with_borrow_mut(change);
    animate(hwnd);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND_MSG => {
                if let Some(command) = Command::from_wparam(wparam.0) {
                    handle(hwnd, command);
                }
                LRESULT(0)
            }
            WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
                if IsWindowVisible(hwnd).as_bool() {
                    LAST_DEACTIVATE_HIDE.store(GetTickCount64(), Ordering::Relaxed);
                    hide(hwnd);
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == SEARCH_TIMER => {
                let _ = KillTimer(Some(hwnd), SEARCH_TIMER);
                search_files(hwnd);
                search_plugins(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == PLUGIN_TIMER => {
                plugin_timer(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == IDLE_TIMER => {
                idle_tick(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == NOTICE_TIMER => {
                end_notice(hwnd);
                LRESULT(0)
            }
            plugin_proc::WM_PLUGIN => {
                plugin_reply(hwnd, wparam, lparam);
                LRESULT(0)
            }
            WM_COPYDATA => {
                let data = &*(lparam.0 as *const COPYDATASTRUCT);
                if data.dwData != everything::REPLY_ID {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                files_found(hwnd, data);
                LRESULT(1)
            }
            WM_TIMER if wparam.0 == OVERLAY_TIMER => {
                power_tick(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == ANIM_TIMER => {
                FRAME_NO.set(FRAME_NO.get() + 1);
                let scrolling = MENU.with_borrow_mut(Menu::step);
                let mut layout = LAYOUT.get();
                let sliding = layout.step_slide();
                LAYOUT.set(layout);
                redraw(hwnd);
                if !scrolling && !sliding && open_ms() >= layout.open_ms() {
                    let _ = KillTimer(Some(hwnd), ANIM_TIMER);
                }
                LRESULT(0)
            }
            winsearch::WM_FILES_FOUND => {
                windows_search_found(hwnd, lparam.0 as *mut winsearch::Reply);
                LRESULT(0)
            }
            WM_HOTKEY => {
                handle(hwnd, Command::Toggle);
                LRESULT(0)
            }
            hook::WM_CAPTURE_MSG => {
                capture_press(hwnd, wparam.0 as u16, lparam.0 as u32);
                LRESULT(0)
            }
            WM_SYSKEYDOWN => {
                // Alt+<key> arrives as a system key; only the hotkey capture wants it.
                if capture_press(hwnd, wparam.0 as u16, held_modifiers()) {
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_MOUSEMOVE => {
                if !TRACKING.get() {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TRACKING.set(TrackMouseEvent(&mut track).is_ok());
                }
                let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
                if SELECTING.get() {
                    // MK_LBUTTON: a button-up lost to another window ends the drag too.
                    if wparam.0 & 1 == 0 {
                        SELECTING.set(false);
                        let _ = ReleaseCapture();
                    } else {
                        let at = query_char_at(x, true);
                        caret_edit(hwnd, |q| q.move_to(at, true));
                    }
                }
                hover_to(hwnd, x, y);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                TRACKING.set(false);
                set_hover(hwnd, None);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let key = wparam.0 as u16;
                // A held Enter that moved the wizard onto the HOTKEY step must not skip it at once.
                if key == VK_RETURN.0 && is_auto_repeat(lparam.0) && wizard_active() {
                    return LRESULT(0);
                }
                if capture_press(hwnd, key, held_modifiers()) {
                    return LRESULT(0);
                }
                if in_power_overlay() {
                    dialog_key(hwnd, key, lparam);
                    return LRESULT(0);
                }
                let (ctrl, shift) = (GetKeyState(VK_CONTROL.0 as i32) < 0, GetKeyState(VK_SHIFT.0 as i32) < 0);
                let typing = PANEL.with_borrow(|p| p.as_ref().is_some_and(Panel::typing));
                if !typing && !wizard_active() && is_settings_key(key, ctrl) {
                    POWER.with_borrow_mut(PowerBar::reset);
                    toggle_panel(hwnd);
                    return LRESULT(0);
                }
                if POWER.with_borrow(|p| p.focused.is_some()) && power_key(hwnd, key, shift) {
                    return LRESULT(0);
                }
                if popup_key(hwnd, key) || panel_key(hwnd, key, ctrl) {
                    return LRESULT(0);
                }
                match key {
                    // Without the power bar there is nothing for Tab to focus.
                    k if k == VK_TAB.0 && LAYOUT.get().has_power_bar() => {
                        POWER.with_borrow_mut(|p| p.tab(shift));
                        redraw(hwnd);
                    }
                    k if k == u16::from(b'V') && ctrl => paste_query(hwnd),
                    k if k == VK_INSERT.0 && shift => paste_query(hwnd),
                    k if k == u16::from(b'P') && ctrl => toggle_pin(hwnd),
                    k if k == u16::from(b'C') && ctrl => copy_query_or_row(hwnd),
                    k if k == u16::from(b'X') && ctrl && query_editing() => cut_query(hwnd),
                    k if k == u16::from(b'A') && ctrl && query_editing() => caret_edit(hwnd, LineEdit::select_all),
                    // A held Enter from typing the keyword must not answer the first-run question.
                    k if k == VK_RETURN.0 && is_auto_repeat(lparam.0) && approval_selected() => {}
                    // → moves the caret until it rests at the end of the query; there it opens the actions.
                    k if k == VK_RIGHT.0 => {
                        let at_rest = MENU.with_borrow(|m| m.query.at_end() && m.query.selection().is_none());
                        if query_editing() && (shift || !at_rest) {
                            caret_edit(hwnd, |q| q.move_right(shift, ctrl));
                        } else {
                            open_actions(hwnd, None);
                        }
                    }
                    k if k == VK_LEFT.0 => {
                        let movable = MENU.with_borrow(|m| m.query.caret() > 0 || m.query.selection().is_some());
                        if query_editing() && movable {
                            caret_edit(hwnd, |q| q.move_left(shift, ctrl));
                        } else if MENU.with_borrow_mut(Menu::close_actions) {
                            redraw(hwnd);
                        }
                    }
                    k if k == VK_HOME.0 && !ctrl && query_editing() => caret_edit(hwnd, |q| q.move_to(0, shift)),
                    k if k == VK_END.0 && !ctrl && query_editing() => caret_edit(hwnd, |q| q.move_to(usize::MAX, shift)),
                    k if k == VK_DELETE.0 && can_delete_forward() => {
                        text_edit(hwnd, |q| if ctrl { q.delete_word_forward() } else { q.delete() });
                    }
                    k if k == VK_ESCAPE.0 && MENU.with_borrow_mut(Menu::close_actions) => redraw(hwnd),
                    k if k == VK_ESCAPE.0 => {
                        let mut cleared = false;
                        eased_list_change(hwnd, || cleared = MENU.with_borrow_mut(Menu::clear_query));
                        if !cleared {
                            hide(hwnd);
                        } else {
                            query_edited(hwnd);
                        }
                    }
                    k if k == VK_RETURN.0 => match MENU.with_borrow(|m| m.actions.as_ref().map(|a| a.selected)) {
                        Some(row) => run_action(hwnd, row),
                        None => launch(hwnd, MENU.with_borrow(Menu::current), ctrl),
                    },
                    k if k == VK_BACK.0 => text_edit(hwnd, |q| if ctrl { q.delete_word_back() } else { q.backspace() }),
                    k if k == VK_UP.0 => navigate(hwnd, |m| m.move_by(-1)),
                    k if k == VK_DOWN.0 => navigate(hwnd, |m| m.move_by(1)),
                    k if k == VK_PRIOR.0 => navigate(hwnd, |m| m.move_by(-PAGE)),
                    k if k == VK_NEXT.0 => navigate(hwnd, |m| m.move_by(PAGE)),
                    k if k == VK_HOME.0 => navigate(hwnd, Menu::select_first),
                    k if k == VK_END.0 => navigate(hwnd, Menu::select_last),
                    _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
                }
                LRESULT(0)
            }
            WM_CHAR => {
                // Control characters (Backspace, Enter, Esc, Ctrl+key) are handled in WM_KEYDOWN.
                if dropdown_open() {
                    if let Some(c) = char::from_u32(wparam.0 as u32).filter(|c| !c.is_control()) {
                        with_panel(|p| p.dropdown_char(c));
                        redraw(hwnd);
                    }
                    return LRESULT(0);
                }
                if PANEL.with_borrow(|p| p.as_ref().is_some_and(|p| p.editing.is_some())) {
                    if let Some(c) = char::from_u32(wparam.0 as u32).filter(|c| !c.is_control()) {
                        with_panel(|p| p.edit_char(c));
                        redraw(hwnd);
                    }
                    return LRESULT(0);
                }
                let overlay_open = POPUP.get().is_some() || PANEL.with_borrow(Option::is_some) || in_power_overlay();
                if let Some(c) = char::from_u32(wparam.0 as u32).filter(|c| !c.is_control() && !overlay_open) {
                    eased_list_change(hwnd, || MENU.with_borrow_mut(|m| m.type_char(c)));
                    query_edited(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL if in_power_overlay() => LRESULT(0),
            WM_RBUTTONUP if in_power_overlay() => LRESULT(0),
            WM_MOUSEWHEEL => {
                let delta = (wparam.0 >> 16) as u16 as i16 as isize;
                let notches = (delta.abs() / WHEEL_DELTA as isize).max(1);
                // The wheel's position is in screen coordinates.
                let mut at = POINT { x: lparam.0 as u16 as i16 as i32, y: (lparam.0 >> 16) as u16 as i16 as i32 };
                let _ = ScreenToClient(hwnd, &mut at);
                if dropdown_open() {
                    with_panel(|p| {
                        if let Some(dropdown) = p.dropdown.as_mut() {
                            dropdown.scroll_by(-delta.signum() as i32 * notches as i32 * 3);
                        }
                    });
                    hover_to(hwnd, at.x as f32, at.y as f32);
                    redraw(hwnd);
                    return LRESULT(0);
                }
                if LAYOUT.get().in_panel(at.x as f32, at.y as f32) && PANEL.with_borrow(Option::is_some) {
                    with_panel(|p| p.scroll_by(-delta.signum() as i32 * notches as i32 * 2));
                    hover_to(hwnd, at.x as f32, at.y as f32);
                    redraw(hwnd);
                    return LRESULT(0);
                }
                navigate(hwnd, |m| m.move_by(-delta.signum() * notches));
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
                if dropdown_open() {
                    with_panel(Panel::close_dropdown);
                    redraw(hwnd);
                    return LRESULT(0);
                }
                let layout = LAYOUT.get();
                if layout.in_header(x, y) {
                    POPUP.set(Some((x, y)));
                    POPUP_ROW.set(0);
                    HOVER.set(Some(Hover::Popup(0)));
                    redraw(hwnd);
                    return LRESULT(0);
                }
                POPUP.set(None);
                let cursor = Some((x - layout.menu_x(), y));
                if let Some(p) = popout_params() {
                    // On the pop-out nothing happens; elsewhere it closes, and a row gets its own.
                    if layout.popout_item_at(&p, x, y).is_none() {
                        let row = MENU.with_borrow(|m| layout.row_at(x, y, m.scroll, m.view.len()));
                        MENU.with_borrow_mut(Menu::close_popout);
                        if let Some(row) = row {
                            open_actions_at(hwnd, row, cursor);
                        }
                    }
                    redraw(hwnd);
                    return LRESULT(0);
                }
                // Right click on a row opens its actions (like →); on the action list it goes back.
                let row = MENU.with_borrow(|m| layout.row_at(x, y, m.scroll, m.view.len()));
                if MENU.with_borrow_mut(Menu::close_actions) {
                    redraw(hwnd);
                } else if let Some(row) = row {
                    open_actions_at(hwnd, row, cursor);
                } else {
                    redraw(hwnd);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
                if !on_query_text(x, y) {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                let (at, shift) = (query_char_at(x, true), GetKeyState(VK_SHIFT.0 as i32) < 0);
                caret_edit(hwnd, |q| q.move_to(at, shift));
                SELECTING.set(true);
                SetCapture(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
                if on_query_text(x, y) {
                    let at = query_char_at(x, false);
                    caret_edit(hwnd, |q| q.select_word(at));
                    SKIP_BUTTON_UP.set(true);
                    return LRESULT(0);
                }
                SKIP_BUTTON_UP.set(!in_power_overlay() && panel_double_click(hwnd, x, y));
                LRESULT(0)
            }
            WM_LBUTTONUP if SELECTING.replace(false) => {
                let _ = ReleaseCapture();
                LRESULT(0)
            }
            WM_LBUTTONUP if SKIP_BUTTON_UP.replace(false) => LRESULT(0),
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
                if in_power_overlay() {
                    match dialog_at(x, y) {
                        Some(DialogHit::Execute) => execute_power(hwnd),
                        Some(DialogHit::Abort | DialogHit::Outside) => cancel_power(hwnd),
                        _ => {}
                    }
                    return LRESULT(0);
                }
                if dropdown_open() {
                    dropdown_click(hwnd, x, y);
                    return LRESULT(0);
                }
                let layout = LAYOUT.get();
                if let Some(anchor) = POPUP.take() {
                    match layout.popup_entry(anchor, x, y) {
                        Some(row) => popup_run(hwnd, row),
                        None => redraw(hwnd),
                    }
                    return LRESULT(0);
                }
                if let Some(p) = popout_params() {
                    // A click outside the pop-out only closes it (on a row, that row is selected, not launched).
                    let (scroll, count) = MENU.with_borrow(|m| (m.scroll, m.view.len()));
                    match layout.list_hit(Some(&p), x, y, scroll, count) {
                        ListHit::Popout(item) => run_action(hwnd, item),
                        ListHit::Row(row) => {
                            navigate(hwnd, |m| {
                                m.close_popout();
                                m.select_row(row);
                            });
                            redraw(hwnd);
                        }
                        ListHit::Nothing => {
                            MENU.with_borrow_mut(Menu::close_popout);
                            redraw(hwnd);
                        }
                    }
                    return LRESULT(0);
                }
                if layout.in_panel(x, y) {
                    panel_click(hwnd, x, y);
                    return LRESULT(0);
                }
                if layout.gear_hit(x, y) {
                    toggle_panel(hwnd);
                    return LRESULT(0);
                }
                if let Some(button) = layout.button_at(x, y) {
                    press_power(hwnd, button);
                    return LRESULT(0);
                }
                let action = MENU.with_borrow(|m| m.actions.as_ref().map(|a| layout.row_at(x, y, a.scroll, a.acts.len())));
                if let Some(row) = action {
                    if let Some(row) = row {
                        run_action(hwnd, row);
                    }
                    return LRESULT(0);
                }
                let hit = MENU.with_borrow(|m| layout.row_at(x, y, m.scroll, m.view.len()).map(|row| m.view[row]));
                if hit.is_some() {
                    launch(hwnd, hit, false);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PLUGINS.with_borrow_mut(Host::shutdown_all);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f2_and_ctrl_comma_open_settings() {
        assert!(is_settings_key(VK_F2.0, false) && is_settings_key(VK_F2.0, true));
        assert!(is_settings_key(VK_OEM_COMMA.0, true));
        assert!(!is_settings_key(VK_OEM_COMMA.0, false), "a bare comma is typed into the query");
        assert!(!is_settings_key(VK_TAB.0, true));
    }
}
