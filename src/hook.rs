//! The keyboard hook behind single-key hotkeys: a key tapped alone toggles the menu. For the Win
//! key the Start menu is suppressed and Win+<key> shortcuts still go to Windows. Other keys (caps
//! lock, F-keys, ...) are swallowed entirely so they don't also do their usual job. While capturing
//! (settings panel), Win taps and Win+<key> are reported instead of acted on. A low-level hook on
//! the menu thread, which pumps messages.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WM_APP, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::Command;
use crate::hotkey::{ANY_WIN, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, is_win_binding, tap_matches};
use crate::window::WM_COMMAND_MSG;

/// Posted while capturing: `wparam` is the virtual key, `lparam` the held modifiers (`MOD_*`).
pub const WM_CAPTURE_MSG: u32 = WM_APP + 2;

/// Marks the keys we inject so the hook doesn't react to them.
const MARKER: usize = 0x4350_4D45;
/// Unassigned virtual key. Windows opens Start only when Win is released with no key pressed
/// in between, so sending this before the Win release keeps Start closed.
const MASK_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

/// Tracks one press of Win: a tap is Win down and up with no other key in between.
#[derive(Default)]
struct Tap {
    win_down: bool,
    other: bool,
}

impl Tap {
    /// Returns true when this event completes a tap.
    fn event(&mut self, is_win: bool, down: bool) -> bool {
        match (is_win, down) {
            (true, true) => {
                // Auto-repeat while held sends more downs; only the first one starts a press.
                if !self.win_down {
                    *self = Tap { win_down: true, other: false };
                }
                false
            }
            (true, false) => {
                let tap = self.win_down && !self.other;
                *self = Tap::default();
                tap
            }
            (false, true) => {
                self.other |= self.win_down;
                false
            }
            (false, false) => false,
        }
    }
}

// The hook runs on the thread that installed it.
thread_local! {
    static TARGET: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    static TAP: RefCell<Tap> = RefCell::new(Tap::default());
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
    /// Key (or `ANY_WIN`) whose tap toggles the menu.
    static BINDING: Cell<Option<u16>> = const { Cell::new(None) };
    static CAPTURE: Cell<bool> = const { Cell::new(false) };
    /// The bound non-Win key is down and its events are being swallowed.
    static HELD: Cell<bool> = const { Cell::new(false) };
    /// While capturing, a key pressed with Win held was taken; Win's release must still be masked.
    static COMBO: Cell<bool> = const { Cell::new(false) };
}

/// Sets what the hook watches for and installs or removes it accordingly: `binding` is the key
/// whose tap toggles the menu window `hwnd` (`ANY_WIN` for either Win key), `capture` reports
/// Win taps and Win+<key> to the window instead. With neither, the hook is removed.
pub fn configure(hwnd: HWND, binding: Option<u16>, capture: bool) {
    TARGET.set(hwnd);
    BINDING.set(binding);
    CAPTURE.set(capture);
    HELD.set(false);
    COMBO.set(false);
    TAP.with_borrow_mut(|t| *t = Tap::default());
    if binding.is_none() && !capture {
        if let Some(hook) = HOOK.take() {
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }
    } else if HOOK.get().is_none() {
        HOOK.set(unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), None, 0) }.ok());
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: MARKER } },
    }
}

fn pressed(vk: VIRTUAL_KEY) -> bool {
    (unsafe { GetAsyncKeyState(vk.0 as i32) }) < 0
}

/// `MOD_*` flags of the modifiers physically held now (not counting Win).
fn held_mods() -> u32 {
    [(VK_CONTROL, MOD_CONTROL), (VK_MENU, MOD_ALT), (VK_SHIFT, MOD_SHIFT)]
        .into_iter()
        .filter(|&(vk, _)| pressed(vk))
        .fold(0, |mods, (_, flag)| mods | flag)
}

fn post(message: u32, wparam: usize, lparam: isize) {
    unsafe {
        let _ = PostMessageW(Some(TARGET.get()), message, WPARAM(wparam), LPARAM(lparam));
    }
}

fn toggle() {
    post(WM_COMMAND_MSG, Command::Toggle as usize, 0);
}

/// Whether the hook swallows this key event.
fn handle(vk: u16, down: bool) -> bool {
    let capture = CAPTURE.get();
    let Some(binding) = (if capture { Some(ANY_WIN) } else { BINDING.get() }) else { return false };
    if is_win_binding(binding) { win_event(binding, vk, down, capture) } else { single_event(binding, vk, down) }
}

fn win_event(binding: u16, vk: u16, down: bool, capture: bool) -> bool {
    let is_win = vk == VK_LWIN.0 || vk == VK_RWIN.0;
    // Capturing Win+<key>: take the key so Windows doesn't act on it.
    if capture && down && !is_win && TAP.with_borrow(|t| t.win_down) {
        post(WM_CAPTURE_MSG, vk as usize, (held_mods() | MOD_WIN) as isize);
        COMBO.set(true);
        TAP.with_borrow_mut(|t| t.other = true);
        return true;
    }
    let tap = TAP.with_borrow_mut(|t| t.event(tap_matches(binding, vk), down));
    let combo_done = tap_matches(binding, vk) && !down && COMBO.take();
    if !tap && !combo_done {
        return false;
    }
    // Swallow this Win release and replay it after the mask key, so Windows sees Win+<mask> and
    // leaves Start closed. Injected keys queue after this event.
    let inputs = [key(MASK_KEY, false), key(MASK_KEY, true), key(VIRTUAL_KEY(vk), true)];
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if tap {
        if capture {
            post(WM_CAPTURE_MSG, vk as usize, 0);
        } else {
            toggle();
        }
    }
    true
}

fn single_event(binding: u16, vk: u16, down: bool) -> bool {
    if vk != binding {
        return false;
    }
    if !down {
        return HELD.take();
    }
    if HELD.get() {
        return true; // auto-repeat
    }
    // Ctrl+F1 and friends still reach their program.
    if held_mods() != 0 || pressed(VK_LWIN) || pressed(VK_RWIN) {
        return false;
    }
    HELD.set(true);
    toggle();
    true
}

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if event.dwExtraInfo != MARKER {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            if handle(event.vkCode as u16, down) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::Tap;

    #[test]
    fn win_alone_is_a_tap() {
        let mut tap = Tap::default();
        assert!(!tap.event(true, true));
        assert!(!tap.event(true, true), "auto-repeat");
        assert!(tap.event(true, false));
    }

    #[test]
    fn win_with_another_key_is_a_shortcut() {
        let mut tap = Tap::default();
        tap.event(true, true);
        tap.event(false, true);
        tap.event(false, false);
        assert!(!tap.event(true, false));
        tap.event(true, true);
        assert!(tap.event(true, false), "the next press starts clean");
    }

    #[test]
    fn keys_before_win_dont_count() {
        let mut tap = Tap::default();
        tap.event(false, true);
        tap.event(true, true);
        assert!(tap.event(true, false));
        assert!(!tap.event(true, false), "release without press");
    }
}
