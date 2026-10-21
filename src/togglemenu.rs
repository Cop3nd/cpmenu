//! The pop-out of on/off toggles under a LOOK row (the header's elements): a list where Enter, a
//! click or an arrow flips the entry in focus and the list stays open. Pure state: settings.rs
//! opens it, window.rs turns keys and clicks into `Key`s, render draws the `TogglesView`.

use crate::fontmenu::Key;

/// What a key or click asks of the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    Stay,
    Close,
    /// Flip entry `.0` (a place in the list) and stay open.
    Flip(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Toggles {
    focus: usize,
}

impl Toggles {
    pub fn focus(&self) -> usize {
        self.focus
    }

    /// `count` is the number of entries; the focus wraps around its ends.
    pub fn key_press(&mut self, key: Key, count: usize) -> Act {
        if count == 0 {
            return if key == Key::Esc { Act::Close } else { Act::Stay };
        }
        match key {
            Key::Up => self.focus = (self.focus + count - 1) % count,
            Key::Down => self.focus = (self.focus + 1) % count,
            Key::Home | Key::PageUp => self.focus = 0,
            Key::End | Key::PageDown => self.focus = count - 1,
            Key::Enter | Key::Left | Key::Right => return Act::Flip(self.focus),
            Key::Esc => return Act::Close,
            Key::Backspace => {}
        }
        Act::Stay
    }

    /// A click on entry `n`: focuses it and flips it. Nothing under a click past the last entry.
    pub fn click_item(&mut self, n: usize, count: usize) -> Act {
        if n >= count {
            return Act::Stay;
        }
        self.focus = n;
        Act::Flip(n)
    }
}

/// One entry: its name and whether it is on.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub label: String,
    pub on: bool,
}

/// Everything the renderer draws of an open toggle pop-out.
#[derive(Clone, Debug, PartialEq)]
pub struct TogglesView {
    /// The label of the row it hangs from.
    pub title: String,
    /// Screen slot of that row.
    pub slot: usize,
    pub entries: Vec<Entry>,
    pub focus: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_move_the_focus_round_and_enter_flips_the_focused_entry() {
        let mut t = Toggles::default();
        assert_eq!(t.key_press(Key::Up, 6), Act::Stay);
        assert_eq!(t.focus(), 5, "wraps to the last entry");
        assert_eq!(t.key_press(Key::Down, 6), Act::Stay);
        assert_eq!(t.focus(), 0);
        t.key_press(Key::End, 6);
        assert_eq!(t.focus(), 5);
        t.key_press(Key::Home, 6);
        assert_eq!(t.focus(), 0);
        t.key_press(Key::Down, 6);
        for key in [Key::Enter, Key::Left, Key::Right] {
            assert_eq!(t.key_press(key, 6), Act::Flip(1), "{key:?}");
        }
        assert_eq!(t.key_press(Key::Esc, 6), Act::Close);
    }

    #[test]
    fn a_click_flips_its_entry_and_one_past_the_end_does_nothing() {
        let mut t = Toggles::default();
        assert_eq!(t.click_item(3, 6), Act::Flip(3));
        assert_eq!(t.focus(), 3);
        assert_eq!(t.click_item(6, 6), Act::Stay);
        assert_eq!(t.focus(), 3);
    }

    #[test]
    fn an_empty_list_only_closes() {
        let mut t = Toggles::default();
        assert_eq!(t.key_press(Key::Down, 0), Act::Stay);
        assert_eq!(t.key_press(Key::Enter, 0), Act::Stay);
        assert_eq!(t.key_press(Key::Esc, 0), Act::Close);
    }
}
