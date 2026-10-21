//! Power bar at the bottom of the menu and the actions behind its buttons.

use std::os::windows::process::CommandExt;

use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::LockWorkStation;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Lock,
    Sleep,
    SignOut,
    Restart,
    Shutdown,
}

pub const ACTIONS: [Action; 5] = [Action::Lock, Action::Sleep, Action::SignOut, Action::Restart, Action::Shutdown];

impl Action {
    pub fn label(self) -> String {
        crate::lang::t(match self {
            Action::Lock => "power.lock",
            Action::Sleep => "power.sleep",
            Action::SignOut => "power.sign_out",
            Action::Restart => "power.restart",
            Action::Shutdown => "power.shutdown",
        })
    }

    /// Closes apps and ends the session: drawn in the alert color.
    pub fn is_dangerous(self) -> bool {
        matches!(self, Action::SignOut | Action::Restart | Action::Shutdown)
    }

    /// Name used in `confirm_power`.
    pub fn from_name(name: &str) -> Option<Action> {
        Some(match name.to_ascii_lowercase().as_str() {
            "lock" => Action::Lock,
            "sleep" => Action::Sleep,
            "sign_out" => Action::SignOut,
            "restart" => Action::Restart,
            "shutdown" => Action::Shutdown,
            _ => return None,
        })
    }

    pub fn run(self) {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let shutdown = |args: &[&str]| {
            let _ = std::process::Command::new("shutdown").args(args).creation_flags(CREATE_NO_WINDOW).spawn();
        };
        match self {
            Action::Lock => {
                let _ = unsafe { LockWorkStation() };
            }
            Action::Sleep => {
                unsafe { SetSuspendState(false, false, false) };
            }
            Action::SignOut => shutdown(&["/l"]),
            Action::Restart => shutdown(&["/r", "/t", "0"]),
            Action::Shutdown => shutdown(&["/s", "/t", "0"]),
        }
    }
}

/// Buttons that ask for confirmation unless `confirm_power` says otherwise.
pub const DEFAULT_CONFIRM: [Action; 3] = [Action::SignOut, Action::Restart, Action::Shutdown];

#[derive(Default)]
pub struct PowerBar {
    /// Keyboard focus is on the bar (Tab), on this button.
    pub focused: Option<usize>,
}

impl PowerBar {
    pub fn move_focus(&mut self, delta: isize) {
        let current = self.focused.unwrap_or(0) as isize;
        self.focused = Some((current + delta).clamp(0, ACTIONS.len() as isize - 1) as usize);
    }

    /// Tab (or Shift+Tab with `reverse`) through the bar: list -> first button (last in reverse) ->
    /// ... -> list again.
    pub fn tab(&mut self, reverse: bool) {
        let last = ACTIONS.len() - 1;
        self.focused = match (self.focused, reverse) {
            (None, false) => Some(0),
            (None, true) => Some(last),
            (Some(i), false) if i < last => Some(i + 1),
            (Some(i), true) if i > 0 => Some(i - 1),
            _ => None,
        };
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_focus_clamps() {
        let mut bar = PowerBar::default();
        bar.move_focus(10);
        assert_eq!(bar.focused, Some(4));
        bar.move_focus(-10);
        assert_eq!(bar.focused, Some(0));
    }

    #[test]
    fn tab_walks_the_bar_and_returns_to_the_list() {
        let mut bar = PowerBar::default();
        let mut seen = Vec::new();
        for _ in 0..=ACTIONS.len() {
            bar.tab(false);
            seen.push(bar.focused);
        }
        assert_eq!(seen, [Some(0), Some(1), Some(2), Some(3), Some(4), None]);
        seen.clear();
        for _ in 0..=ACTIONS.len() {
            bar.tab(true);
            seen.push(bar.focused);
        }
        assert_eq!(seen, [Some(4), Some(3), Some(2), Some(1), Some(0), None]);
    }

    #[test]
    fn names_map_to_actions() {
        assert_eq!(Action::from_name("sign_out"), Some(Action::SignOut));
        assert_eq!(Action::from_name("SHUTDOWN"), Some(Action::Shutdown));
        assert_eq!(Action::from_name("signout"), None);
        for action in ACTIONS {
            assert_eq!(DEFAULT_CONFIRM.contains(&action), action.is_dangerous());
        }
    }
}
