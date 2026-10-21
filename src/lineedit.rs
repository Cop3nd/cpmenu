//! One line of text with a caret and a selection: the model behind the search query.
//! Positions are character indexes, so multibyte text never splits.

use std::ops::Deref;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
    caret: usize,
    /// The selection's fixed end; equal to `caret` when nothing is selected.
    anchor: usize,
}

impl Deref for LineEdit {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl PartialEq<&str> for LineEdit {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

/// 0 = space, 1 = letters and digits, 2 = anything else; a word is a run of one class.
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() {
        1
    } else {
        2
    }
}

/// Start of the word before `pos`: spaces first, then a run of one class.
fn prev_word(chars: &[char], pos: usize) -> usize {
    let mut i = pos;
    while i > 0 && class(chars[i - 1]) == 0 {
        i -= 1;
    }
    if i > 0 {
        let k = class(chars[i - 1]);
        while i > 0 && class(chars[i - 1]) == k {
            i -= 1;
        }
    }
    i
}

/// Start of the word after `pos`: the rest of the current run, then the spaces after it.
fn next_word(chars: &[char], pos: usize) -> usize {
    let mut i = pos;
    if i < chars.len() && class(chars[i]) != 0 {
        let k = class(chars[i]);
        while i < chars.len() && class(chars[i]) == k {
            i += 1;
        }
    }
    while i < chars.len() && class(chars[i]) == 0 {
        i += 1;
    }
    i
}

impl LineEdit {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// `(start, end)` of the selected characters, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor).then(|| (self.caret.min(self.anchor), self.caret.max(self.anchor)))
    }

    pub fn selected(&self) -> Option<&str> {
        self.selection().map(|(from, to)| &self.text[self.byte(from)..self.byte(to)])
    }

    pub fn at_end(&self) -> bool {
        self.caret == self.char_len()
    }

    /// Replaces everything; the caret goes to the end.
    pub fn set(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.caret = self.char_len();
        self.anchor = self.caret;
    }

    pub fn clear(&mut self) {
        self.set("");
    }

    fn byte(&self, index: usize) -> usize {
        self.text.char_indices().nth(index).map_or(self.text.len(), |(b, _)| b)
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    fn replace(&mut self, from: usize, to: usize, with: &str) {
        let (a, b) = (self.byte(from), self.byte(to));
        self.text.replace_range(a..b, with);
        self.caret = from + with.chars().count();
        self.anchor = self.caret;
    }

    /// Types `s` over the selection, or at the caret.
    pub fn insert(&mut self, s: &str) -> bool {
        let (from, to) = self.selection().unwrap_or((self.caret, self.caret));
        if s.is_empty() && from == to {
            return false;
        }
        self.replace(from, to, s);
        true
    }

    /// Removes the selection and returns it.
    pub fn cut(&mut self) -> Option<String> {
        let selected = self.selected()?.to_owned();
        self.insert("");
        Some(selected)
    }

    pub fn backspace(&mut self) -> bool {
        match self.selection() {
            Some(_) => self.insert(""),
            None if self.caret > 0 => {
                self.replace(self.caret - 1, self.caret, "");
                true
            }
            None => false,
        }
    }

    pub fn delete(&mut self) -> bool {
        match self.selection() {
            Some(_) => self.insert(""),
            None if self.caret < self.char_len() => {
                self.replace(self.caret, self.caret + 1, "");
                true
            }
            None => false,
        }
    }

    /// Ctrl+Backspace: the selection, else the word before the caret.
    pub fn delete_word_back(&mut self) -> bool {
        if self.selection().is_some() {
            return self.insert("");
        }
        let from = prev_word(&self.chars(), self.caret);
        if from == self.caret {
            return false;
        }
        self.replace(from, self.caret, "");
        true
    }

    /// Ctrl+Delete: the selection, else the word after the caret.
    pub fn delete_word_forward(&mut self) -> bool {
        if self.selection().is_some() {
            return self.insert("");
        }
        let to = next_word(&self.chars(), self.caret);
        if to == self.caret {
            return false;
        }
        self.replace(self.caret, to, "");
        true
    }

    /// Moves the caret; `extend` keeps the anchor so the selection grows, else it collapses.
    pub fn move_to(&mut self, pos: usize, extend: bool) {
        self.caret = pos.min(self.char_len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// A plain Left with a selection collapses to its start instead of moving.
    pub fn move_left(&mut self, extend: bool, word: bool) {
        match self.selection() {
            Some((from, _)) if !extend => self.move_to(from, false),
            _ if word => self.move_to(prev_word(&self.chars(), self.caret), extend),
            _ => self.move_to(self.caret.saturating_sub(1), extend),
        }
    }

    pub fn move_right(&mut self, extend: bool, word: bool) {
        match self.selection() {
            Some((_, to)) if !extend => self.move_to(to, false),
            _ if word => self.move_to(next_word(&self.chars(), self.caret), extend),
            _ => self.move_to(self.caret + 1, extend),
        }
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.char_len();
    }

    /// Selects the run of letters/digits, symbols or spaces around `pos`.
    pub fn select_word(&mut self, pos: usize) {
        let chars = self.chars();
        if chars.is_empty() {
            return;
        }
        let at = pos.min(chars.len() - 1);
        let k = class(chars[at]);
        let mut from = at;
        while from > 0 && class(chars[from - 1]) == k {
            from -= 1;
        }
        let mut to = at + 1;
        while to < chars.len() && class(chars[to]) == k {
            to += 1;
        }
        self.anchor = from;
        self.caret = to;
    }
}

/// The query as drawn (upper case) and, per character, the UTF-16 offset where it starts in that
/// text (one extra entry for the end). Upper-casing can change lengths ("ß" is "SS").
pub fn shouted(text: &str) -> (String, Vec<u32>) {
    let mut shown = String::with_capacity(text.len());
    let mut offsets = Vec::with_capacity(text.len() + 1);
    let mut units = 0;
    for c in text.chars() {
        offsets.push(units);
        for u in c.to_uppercase() {
            shown.push(u);
            units += u.len_utf16() as u32;
        }
    }
    offsets.push(units);
    (shown, offsets)
}

/// The character whose edge is nearest to UTF-16 offset `unit` of the text `shouted` returned.
pub fn char_at_unit(offsets: &[u32], unit: u32) -> usize {
    let last = offsets.len().saturating_sub(1);
    let before = offsets.partition_point(|&o| o <= unit).saturating_sub(1).min(last);
    match offsets.get(before + 1) {
        Some(&next) if next - unit.min(next) < unit.saturating_sub(offsets[before]) => before + 1,
        _ => before,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> LineEdit {
        let mut l = LineEdit::default();
        l.set(text);
        l
    }

    #[test]
    fn insert_and_delete_at_the_caret_keep_multibyte_text_whole() {
        let mut l = line("zażółć gęślą");
        assert_eq!(l.caret(), 12);
        l.move_to(3, false);
        assert!(l.insert("Ł"));
        assert_eq!((l.as_str(), l.caret()), ("zażŁółć gęślą", 4));
        assert!(l.backspace());
        assert_eq!((l.as_str(), l.caret()), ("zażółć gęślą", 3));
        assert!(l.delete());
        assert_eq!((l.as_str(), l.caret()), ("zażłć gęślą", 3));
    }

    #[test]
    fn backspace_and_delete_stop_at_the_ends() {
        let mut l = line("ą");
        assert!(l.backspace() && !l.backspace());
        assert!(!l.delete());
        assert_eq!(l, "");
        l.insert("ęś");
        l.move_to(0, false);
        assert!(l.delete() && !l.backspace());
        assert_eq!(l, "ś");
    }

    #[test]
    fn selection_is_replaced_or_deleted_by_every_edit() {
        let mut l = line("zażółć gęślą");
        l.move_to(2, false);
        l.move_to(6, true);
        assert_eq!((l.selection(), l.selected()), (Some((2, 6)), Some("żółć")));
        assert!(l.insert("o"));
        assert_eq!((l.as_str(), l.caret(), l.selection()), ("zao gęślą", 3, None));
        l.select_all();
        assert_eq!(l.cut().as_deref(), Some("zao gęślą"));
        assert_eq!((l.as_str(), l.caret()), ("", 0));
        assert_eq!(l.cut(), None);
        l.set("gęś");
        l.move_to(3, false);
        l.move_to(1, true);
        assert!(l.backspace());
        assert_eq!(l, "g");
        l.select_all();
        assert!(l.delete());
        l.set("ab");
        l.select_all();
        assert!(l.delete_word_back());
        assert_eq!(l, "");
    }

    #[test]
    fn arrows_move_and_collapse_the_selection() {
        let mut l = line("gęślą");
        l.move_left(false, false);
        assert_eq!(l.caret(), 4);
        l.move_left(true, false);
        l.move_left(true, false);
        assert_eq!((l.selection(), l.caret()), (Some((2, 4)), 2));
        l.move_right(true, false);
        assert_eq!(l.selection(), Some((3, 4)));
    }

    #[test]
    fn plain_arrows_collapse_to_the_nearer_edge() {
        let mut l = line("abcdef");
        l.move_to(4, false);
        l.move_to(1, true);
        l.move_left(false, false);
        assert_eq!((l.caret(), l.selection()), (1, None), "Left goes to the start");
        l.move_to(4, true);
        l.move_right(false, false);
        assert_eq!((l.caret(), l.selection()), (4, None), "Right goes to the end");
        l.move_to(0, false);
        l.move_left(false, false);
        assert_eq!(l.caret(), 0);
        l.move_to(6, false);
        l.move_right(false, false);
        assert!(l.at_end());
    }

    #[test]
    fn word_movement_and_deletion() {
        let mut l = line("zażółć gęślą /all");
        l.move_left(false, true);
        assert_eq!(l.caret(), 14, "before the word");
        l.move_left(false, true);
        assert_eq!(l.caret(), 13, "the symbol run is a word of its own");
        l.move_left(false, true);
        assert_eq!(l.caret(), 7);
        l.move_left(true, true);
        assert_eq!((l.caret(), l.selection()), (0, Some((0, 7))));
        l.move_to(0, false);
        l.move_right(false, true);
        assert_eq!(l.caret(), 7, "lands at the start of the next word");
        assert!(l.delete_word_forward());
        assert_eq!(l, "zażółć /all");
        assert!(l.delete_word_back());
        assert_eq!((l.as_str(), l.caret()), ("/all", 0));
        assert!(!l.delete_word_back());
        assert!(l.delete_word_forward());
        assert!(l.delete_word_forward());
        assert_eq!(l, "");
    }

    #[test]
    fn ctrl_backspace_at_the_end_matches_the_old_behaviour() {
        let mut l = line("> ipconfig /all  ");
        let mut steps = Vec::new();
        while l.delete_word_back() {
            steps.push(l.as_str().to_owned());
        }
        assert_eq!(steps, vec!["> ipconfig /", "> ipconfig ", "> ", ""]);
    }

    #[test]
    fn double_click_selects_the_run_under_the_point() {
        let mut l = line("zażółć gęślą");
        l.select_word(2);
        assert_eq!(l.selected(), Some("zażółć"));
        l.select_word(6);
        assert_eq!(l.selected(), Some(" "));
        l.select_word(99);
        assert_eq!(l.selected(), Some("gęślą"), "past the end picks the last run");
        let mut empty = LineEdit::default();
        empty.select_word(0);
        assert_eq!(empty.selection(), None);
    }

    #[test]
    fn shouted_text_maps_characters_to_utf16_offsets() {
        let (shown, offsets) = shouted("aß😀ę");
        assert_eq!(shown, "ASS😀Ę");
        assert_eq!(offsets, vec![0, 1, 3, 5, 6]);
        assert_eq!(char_at_unit(&offsets, 0), 0);
        assert_eq!(char_at_unit(&offsets, 1), 1);
        assert_eq!(char_at_unit(&offsets, 2), 1, "inside the SS: the nearer edge");
        assert_eq!(char_at_unit(&offsets, 6), 4);
        assert_eq!(char_at_unit(&offsets, 99), 4);
        assert_eq!(char_at_unit(&shouted("").1, 5), 0);
    }
}
