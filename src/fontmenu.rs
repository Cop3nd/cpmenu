//! The font dropdown of the LOOK tab: a filter field over a list of the installed families and,
//! for a text role, rows for its size and weight above them. Pure state: settings.rs opens it,
//! window.rs turns keys and clicks into `Key`s, render.rs draws the `DropdownView`.

use toml_edit::Value;

use crate::fonts;
use crate::lang;
use crate::style::{FONT_SIZE_RANGE, Fonts, Role, Weight};

/// List entries in view at once.
pub const VISIBLE: usize = 10;
/// Longest filter that can be typed.
const MAX_FILTER: usize = 40;
const SIZE_STEP: u32 = 5;
/// A role's weight as the dropdown steps it: its built-in one first, then the four named ones.
const WEIGHT_CHOICES: [Option<Weight>; 5] = [None, Some(Weight::Light), Some(Weight::Normal), Some(Weight::Semibold), Some(Weight::Bold)];

/// Where the keyboard is. `List(i)` counts entries from the top of the whole list, not the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    Size,
    Weight,
    List(usize),
}

/// What a list entry stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entry {
    /// The role uses the global family.
    Global,
    /// An installed family, by place in the panel's list.
    Family(usize),
}

/// Keys the dropdown reacts to (typed characters go to `Dropdown::type_char`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
}

/// What a key asks of the window.
#[derive(Debug)]
pub enum Outcome {
    Stay,
    Close,
    /// Write this `[fonts]` key (`None` removes it) and stay open: size and weight apply live.
    Write(String, Option<Value>),
    /// Write it and close: a family was chosen.
    Pick(String, Option<Value>),
}

/// Outcomes are equal when they ask for the same write (TOML values have no equality of their own).
impl PartialEq for Outcome {
    fn eq(&self, other: &Self) -> bool {
        let write = |o: &Outcome| match o {
            Outcome::Write(key, value) => Some((false, key.clone(), value.as_ref().map(ToString::to_string))),
            Outcome::Pick(key, value) => Some((true, key.clone(), value.as_ref().map(ToString::to_string))),
            _ => None,
        };
        std::mem::discriminant(self) == std::mem::discriminant(other) && write(self) == write(other)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dropdown {
    /// `None` is the global family.
    role: Option<Role>,
    filter: String,
    /// Places in the panel's family list that fit `filter`, best first.
    matches: Vec<usize>,
    focus: Focus,
    /// First entry in view.
    top: usize,
}

impl Dropdown {
    /// Opens on the family in use (GLOBAL for a role without its own), scrolled to show it.
    pub fn open(role: Option<Role>, families: &[String], fonts: &Fonts) -> Self {
        let mut open = Self { role, filter: String::new(), matches: (0..families.len()).collect(), focus: Focus::List(0), top: 0 };
        let own = match role {
            None => Some(fonts.family),
            Some(role) => fonts.role(role).family,
        };
        let at = own.and_then(|own| families.iter().position(|f| f.eq_ignore_ascii_case(own.as_str())));
        open.focus = Focus::List(at.map_or(0, |i| i + usize::from(open.has_global())));
        open.keep_visible();
        open
    }

    pub fn role(&self) -> Option<Role> {
        self.role
    }

    #[cfg(test)]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    #[cfg(test)]
    pub fn focus(&self) -> Focus {
        self.focus
    }

    #[cfg(test)]
    pub fn top(&self) -> usize {
        self.top
    }

    /// The GLOBAL entry heads the list of a role while nothing is typed.
    fn has_global(&self) -> bool {
        self.role.is_some() && self.filter.is_empty()
    }

    /// Entries in the list.
    pub fn len(&self) -> usize {
        self.matches.len() + usize::from(self.has_global())
    }

    pub fn entry(&self, i: usize) -> Option<Entry> {
        if self.has_global() {
            return if i == 0 { Some(Entry::Global) } else { self.matches.get(i - 1).map(|&k| Entry::Family(k)) };
        }
        self.matches.get(i).map(|&k| Entry::Family(k))
    }

    /// The `[fonts]` key of this dropdown's `field`.
    fn key(&self, field: &str) -> String {
        match self.role {
            None => field.to_owned(),
            Some(role) => format!("{}.{field}", role.key()),
        }
    }

    /// Rows above the list: SIZE and WEIGHT, for a role.
    fn header_rows(&self) -> usize {
        if self.role.is_some() { 2 } else { 0 }
    }

    /// Position of the focus among SIZE, WEIGHT and the entries, top to bottom.
    fn position(&self) -> usize {
        match self.focus {
            Focus::Size => 0,
            Focus::Weight => 1,
            Focus::List(i) => self.header_rows() + i,
        }
    }

    fn keep_visible(&mut self) {
        if let Focus::List(i) = self.focus {
            if i < self.top {
                self.top = i;
            } else if i >= self.top + VISIBLE {
                self.top = i + 1 - VISIBLE;
            }
        }
        self.top = self.top.min(self.len().saturating_sub(VISIBLE));
    }

    /// Up (-1) or down (1) by `delta` places, stopping at both ends.
    fn move_by(&mut self, delta: i32) {
        let last = self.header_rows() + self.len().max(1) - 1;
        let to = (self.position() as i32 + delta).clamp(0, last as i32) as usize;
        self.focus = match to {
            0 if self.role.is_some() => Focus::Size,
            1 if self.role.is_some() => Focus::Weight,
            _ => Focus::List(to - self.header_rows()),
        };
        self.keep_visible();
    }

    /// PageUp / PageDown: a window's worth within the list; from SIZE or WEIGHT, down enters the list.
    fn page(&mut self, dir: i32) {
        match self.focus {
            Focus::List(i) => {
                let to = (i as i32 + dir * (VISIBLE as i32 - 1)).clamp(0, self.len().max(1) as i32 - 1);
                self.focus = Focus::List(to as usize);
            }
            _ if dir > 0 => self.focus = Focus::List(0),
            _ => {}
        }
        self.keep_visible();
    }

    /// The mouse wheel: scrolls the window without moving the focus.
    pub fn scroll_by(&mut self, delta: i32) {
        let most = self.len().saturating_sub(VISIBLE);
        self.top = (self.top as i32 + delta).clamp(0, most as i32) as usize;
    }

    /// A typed character narrows the list; the first match is in front.
    pub fn type_char(&mut self, c: char, families: &[String]) {
        if c.is_control() || self.filter.chars().count() >= MAX_FILTER {
            return;
        }
        self.filter.push(c);
        self.refilter(families);
    }

    fn backspace(&mut self, families: &[String]) {
        if self.filter.pop().is_some() {
            self.refilter(families);
        }
    }

    fn refilter(&mut self, families: &[String]) {
        self.matches = fonts::matching(families, &self.filter);
        self.focus = Focus::List(0);
        self.top = 0;
    }

    /// Focus on the entry `n` places down the window (a click); false past the end of the list.
    pub fn focus_item(&mut self, n: usize) -> bool {
        let i = self.top + n;
        let fits = i < self.len();
        if fits {
            self.focus = Focus::List(i);
        }
        fits
    }

    /// Focus on the SIZE or WEIGHT row (a click on it).
    pub fn focus_row(&mut self, focus: Focus) {
        if self.role.is_some() && !matches!(focus, Focus::List(_)) {
            self.focus = focus;
        }
    }

    /// ◂ ▸ on the SIZE and WEIGHT rows: the key and value to write, `None` at the end of a size's range.
    pub fn step(&self, fonts: &Fonts, dir: i32) -> Option<(String, Option<Value>)> {
        let role = self.role?;
        let dir = dir.signum();
        match self.focus {
            Focus::Size => {
                let now = fonts.role(role).size;
                let to = if dir > 0 { (now / SIZE_STEP + 1) * SIZE_STEP } else { (now + SIZE_STEP - 1) / SIZE_STEP * SIZE_STEP - SIZE_STEP };
                let to = to.clamp(FONT_SIZE_RANGE.0, FONT_SIZE_RANGE.1);
                (to != now).then(|| (self.key("size"), Some(Value::from(i64::from(to)))))
            }
            Focus::Weight => {
                let at = WEIGHT_CHOICES.iter().position(|&w| w == fonts.role(role).weight).unwrap_or(0) as i32;
                let next = WEIGHT_CHOICES[(at + dir).rem_euclid(WEIGHT_CHOICES.len() as i32) as usize];
                Some((self.key("weight"), next.map(|w| Value::from(w.key()))))
            }
            Focus::List(_) => None,
        }
    }

    /// The family chosen by the focused entry: GLOBAL removes the role's own.
    fn choice(&self, families: &[String]) -> Option<(String, Option<Value>)> {
        let Focus::List(i) = self.focus else { return None };
        match self.entry(i)? {
            Entry::Global => Some((self.key("family"), None)),
            Entry::Family(k) => Some((self.key("family"), Some(Value::from(families.get(k)?.as_str())))),
        }
    }

    pub fn key_press(&mut self, key: Key, families: &[String], fonts: &Fonts) -> Outcome {
        match key {
            Key::Up => self.move_by(-1),
            Key::Down => self.move_by(1),
            Key::PageUp => self.page(-1),
            Key::PageDown => self.page(1),
            Key::Home => {
                self.focus = Focus::List(0);
                self.keep_visible();
            }
            Key::End => {
                self.focus = Focus::List(self.len().max(1) - 1);
                self.keep_visible();
            }
            Key::Left | Key::Right => {
                if let Some((key, value)) = self.step(fonts, if key == Key::Left { -1 } else { 1 }) {
                    return Outcome::Write(key, value);
                }
            }
            Key::Backspace => self.backspace(families),
            Key::Esc => return Outcome::Close,
            Key::Enter => {
                return match self.focus {
                    Focus::List(_) => self.choice(families).map_or(Outcome::Stay, |(key, value)| Outcome::Pick(key, value)),
                    _ => Outcome::Close,
                };
            }
        }
        Outcome::Stay
    }

    /// A click on the entry `n` places down the window.
    pub fn click_item(&mut self, n: usize, families: &[String]) -> Outcome {
        if !self.focus_item(n) {
            return Outcome::Stay;
        }
        self.choice(families).map_or(Outcome::Stay, |(key, value)| Outcome::Pick(key, value))
    }

    /// What the renderer needs, for the anchor row in screen slot `slot`.
    pub fn view(&self, slot: usize, families: &[String], fonts: &Fonts) -> DropdownView {
        let current = match self.role {
            None => Some(fonts.family),
            Some(role) => fonts.role(role).family,
        };
        let is_current = |name: &str| current.is_some_and(|c| c.as_str().eq_ignore_ascii_case(name));
        let choices = (self.top..(self.top + VISIBLE).min(self.len()))
            .filter_map(|i| match self.entry(i)? {
                Entry::Global => Some(Choice { label: lang::t("settings.value.global"), family: None, current: current.is_none() }),
                Entry::Family(k) => {
                    let name = families.get(k)?;
                    Some(Choice { label: name.clone(), family: Some(name.clone()), current: is_current(name) })
                }
            })
            .collect();
        let spot = match self.focus {
            Focus::Size => Some(Spot::Size),
            Focus::Weight => Some(Spot::Weight),
            Focus::List(i) => (self.top..self.top + VISIBLE).contains(&i).then(|| Spot::Item(i - self.top)),
        };
        let (size, weight) = match self.role {
            Some(role) => (format!("{}%", fonts.role(role).size), weight_label(fonts.role(role).weight)),
            None => Default::default(),
        };
        DropdownView { title: String::new(), slot, role: self.role.is_some(), size, weight, filter: self.filter.clone(), choices, spot, top: self.top, total: self.len() }
    }
}

/// The word for a role's weight: DEFAULT while it has none of its own.
pub fn weight_label(weight: Option<Weight>) -> String {
    lang::t(&format!("settings.weight.{}", weight.map_or("default", Weight::key)))
}

/// One visible list entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    /// The family to set the label in; `None` for GLOBAL.
    pub family: Option<String>,
    pub current: bool,
}

/// Which part of the visible dropdown has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spot {
    Size,
    Weight,
    /// The entry this many places down the window.
    Item(usize),
}

/// Everything the renderer draws of an open dropdown.
#[derive(Clone, Debug, PartialEq)]
pub struct DropdownView {
    /// The label of the row it hangs from, set by the panel.
    pub title: String,
    /// Screen slot of the row it hangs from.
    pub slot: usize,
    /// A text role's: SIZE and WEIGHT rows on top.
    pub role: bool,
    pub size: String,
    pub weight: String,
    pub filter: String,
    pub choices: Vec<Choice>,
    pub spot: Option<Spot>,
    pub top: usize,
    pub total: usize,
}

/// What places an open dropdown on the card (see `Layout::dropdown_rect`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropGeom {
    pub slot: usize,
    pub role: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Family;

    fn families(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Font {i:03}")).collect()
    }

    fn open(role: Option<Role>, n: usize) -> (Dropdown, Vec<String>) {
        let list = families(n);
        (Dropdown::open(role, &list, &Fonts::default()), list)
    }

    fn press(d: &mut Dropdown, list: &[String], key: Key) -> Outcome {
        d.key_press(key, list, &Fonts::default())
    }

    fn written(outcome: Outcome) -> Option<(String, String)> {
        match outcome {
            Outcome::Write(key, value) | Outcome::Pick(key, value) => Some((key, value.map_or_else(|| "remove".to_owned(), |v| v.to_string().trim().to_owned()))),
            _ => None,
        }
    }

    fn pair(key: &str, value: &str) -> Option<(String, String)> {
        Some((key.to_owned(), value.to_owned()))
    }

    #[test]
    fn it_opens_on_the_family_in_use_and_scrolls_to_show_it() {
        let list = families(100);
        let mut fonts = Fonts::default();
        fonts.family = Family::new("font 042").unwrap();
        let d = Dropdown::open(None, &list, &fonts);
        assert_eq!(d.focus(), Focus::List(42));
        assert_eq!(d.top(), 33, "the focused entry is the last one in view");
        let mut fonts = Fonts::default();
        fonts.roles[2].family = Family::new("Font 007");
        let d = Dropdown::open(Some(Role::Name), &list, &fonts);
        assert_eq!(d.focus(), Focus::List(8), "GLOBAL is the first entry");
        assert_eq!(Dropdown::open(Some(Role::Name), &list, &Fonts::default()).focus(), Focus::List(0), "a role without its own family opens on GLOBAL");
        assert_eq!(Dropdown::open(None, &list, &Fonts::default()).focus(), Focus::List(0), "a family that is not installed opens at the top");
    }

    #[test]
    fn typing_narrows_the_list_and_backspace_widens_it() {
        let list: Vec<String> = ["Arial", "Segoe UI", "Segoe UI Semibold", "Consolas"].map(String::from).to_vec();
        let mut d = Dropdown::open(None, &list, &Fonts::default());
        assert_eq!(d.len(), 4);
        d.type_char('s', &list);
        d.type_char('e', &list);
        assert_eq!((d.len(), d.entry(0), d.entry(1)), (2, Some(Entry::Family(1)), Some(Entry::Family(2))));
        assert_eq!(d.focus(), Focus::List(0), "the first match is in front");
        d.type_char('\u{8}', &list);
        assert_eq!(d.filter(), "se", "control characters are not typed");
        assert_eq!(press(&mut d, &list, Key::Backspace), Outcome::Stay);
        assert_eq!(d.len(), 3, "s fits Segoe UI, Segoe UI Semibold and Consolas");
        press(&mut d, &list, Key::Backspace);
        assert_eq!((d.filter(), d.len()), ("", 4));
        for c in "zzz".chars() {
            d.type_char(c, &list);
        }
        assert_eq!(d.len(), 0);
        assert_eq!(press(&mut d, &list, Key::Enter), Outcome::Stay, "nothing to pick");
        assert_eq!(press(&mut d, &list, Key::Down), Outcome::Stay);
        let mut roles = Dropdown::open(Some(Role::Name), &list, &Fonts::default());
        assert_eq!(roles.len(), 5, "GLOBAL and the four families");
        roles.type_char('c', &list);
        assert_eq!((roles.len(), roles.entry(0)), (1, Some(Entry::Family(3))), "GLOBAL only heads the unfiltered list");
        let long = "x".repeat(MAX_FILTER + 10);
        for c in long.chars() {
            d.type_char(c, &list);
        }
        assert_eq!(d.filter().chars().count(), MAX_FILTER);
    }

    #[test]
    fn navigation_clamps_at_both_ends_and_follows_the_focus() {
        let (mut d, list) = open(None, 25);
        press(&mut d, &list, Key::Up);
        assert_eq!(d.focus(), Focus::List(0), "no wrapping");
        for _ in 0..12 {
            press(&mut d, &list, Key::Down);
        }
        assert_eq!((d.focus(), d.top()), (Focus::List(12), 3));
        press(&mut d, &list, Key::PageDown);
        assert_eq!((d.focus(), d.top()), (Focus::List(21), 12));
        press(&mut d, &list, Key::PageDown);
        press(&mut d, &list, Key::PageDown);
        assert_eq!((d.focus(), d.top()), (Focus::List(24), 15), "stops at the last entry");
        press(&mut d, &list, Key::Down);
        assert_eq!(d.focus(), Focus::List(24));
        press(&mut d, &list, Key::PageUp);
        assert_eq!(d.focus(), Focus::List(15));
        press(&mut d, &list, Key::Home);
        assert_eq!((d.focus(), d.top()), (Focus::List(0), 0));
        press(&mut d, &list, Key::End);
        assert_eq!((d.focus(), d.top()), (Focus::List(24), 15));
        let (mut short, list) = open(None, 4);
        press(&mut short, &list, Key::End);
        assert_eq!((short.focus(), short.top()), (Focus::List(3), 0), "a list shorter than the window never scrolls");
    }

    #[test]
    fn the_wheel_scrolls_the_window_and_leaves_the_focus() {
        let (mut d, _) = open(None, 25);
        d.scroll_by(4);
        assert_eq!((d.top(), d.focus()), (4, Focus::List(0)));
        d.scroll_by(100);
        assert_eq!(d.top(), 15, "the last window");
        d.scroll_by(-100);
        assert_eq!(d.top(), 0);
        let (mut short, _) = open(None, 3);
        short.scroll_by(5);
        assert_eq!(short.top(), 0);
    }

    #[test]
    fn enter_picks_the_focused_family_and_esc_writes_nothing() {
        let (mut d, list) = open(None, 25);
        press(&mut d, &list, Key::Down);
        press(&mut d, &list, Key::Down);
        assert_eq!(written(press(&mut d, &list, Key::Enter)), pair("family", "\"Font 002\""));
        assert_eq!(press(&mut d, &list, Key::Esc), Outcome::Close);
        let (mut r, list) = open(Some(Role::Tag), 25);
        press(&mut r, &list, Key::Down);
        assert_eq!(written(press(&mut r, &list, Key::Enter)), pair("tag.family", "\"Font 000\""), "the entry after GLOBAL");
        press(&mut r, &list, Key::Home);
        assert_eq!(written(press(&mut r, &list, Key::Enter)), pair("tag.family", "remove"), "GLOBAL clears the role's family");
        assert!(matches!(press(&mut r, &list, Key::Enter), Outcome::Pick(..)));
        assert_eq!(written(r.click_item(3, &list)), pair("tag.family", "\"Font 002\""));
        assert_eq!(r.click_item(30, &list), Outcome::Stay, "past the end");
    }

    #[test]
    fn a_role_has_size_and_weight_rows_above_the_list() {
        let (mut d, list) = open(Some(Role::Name), 25);
        let mut fonts = Fonts::default();
        assert_eq!(press(&mut d, &list, Key::Up), Outcome::Stay);
        assert_eq!(d.focus(), Focus::Weight);
        assert_eq!(written(d.key_press(Key::Right, &list, &fonts)), pair("name.weight", "\"light\""));
        assert_eq!(written(d.key_press(Key::Left, &list, &fonts)), pair("name.weight", "\"bold\""), "wraps the other way");
        fonts.roles[2].weight = Some(Weight::Bold);
        assert_eq!(written(d.key_press(Key::Right, &list, &fonts)), pair("name.weight", "remove"), "bold wraps to DEFAULT");
        assert_eq!(written(d.key_press(Key::Left, &list, &fonts)), pair("name.weight", "\"semibold\""));
        press(&mut d, &list, Key::Up);
        assert_eq!(d.focus(), Focus::Size);
        press(&mut d, &list, Key::Up);
        assert_eq!(d.focus(), Focus::Size, "clamps on top");
        assert_eq!((written(d.key_press(Key::Right, &list, &fonts)), written(d.key_press(Key::Left, &list, &fonts))), (pair("name.size", "105"), pair("name.size", "95")));
        fonts.roles[2].size = 98;
        assert_eq!((written(d.key_press(Key::Right, &list, &fonts)), written(d.key_press(Key::Left, &list, &fonts))), (pair("name.size", "100"), pair("name.size", "95")));
        fonts.roles[2].size = 200;
        assert_eq!((written(d.key_press(Key::Right, &list, &fonts)), written(d.key_press(Key::Left, &list, &fonts))), (None, pair("name.size", "195")));
        fonts.roles[2].size = 50;
        assert_eq!((written(d.key_press(Key::Left, &list, &fonts)), written(d.key_press(Key::Right, &list, &fonts))), (None, pair("name.size", "55")));
        assert_eq!(d.key_press(Key::Enter, &list, &fonts), Outcome::Close, "Enter on a value row only closes");
        press(&mut d, &list, Key::Down);
        press(&mut d, &list, Key::Down);
        assert_eq!(d.focus(), Focus::List(0));
        assert_eq!(d.key_press(Key::Left, &list, &fonts), Outcome::Stay, "arrows do nothing on the list");
        press(&mut d, &list, Key::PageUp);
        assert_eq!(d.focus(), Focus::List(0), "page up stays in the list");
        press(&mut d, &list, Key::Up);
        press(&mut d, &list, Key::PageDown);
        assert_eq!(d.focus(), Focus::List(0), "page down from a value row enters the list");
        let (mut global, list) = open(None, 5);
        assert_eq!(global.key_press(Key::Right, &list, &Fonts::default()), Outcome::Stay, "the global dropdown has no value rows");
        press(&mut global, &list, Key::Up);
        assert_eq!(global.focus(), Focus::List(0));
    }

    #[test]
    fn the_view_has_the_visible_window_and_marks_the_current_family() {
        let list = families(30);
        let mut fonts = Fonts::default();
        fonts.roles[2].family = Family::new("Font 011");
        fonts.roles[2].size = 120;
        fonts.roles[2].weight = Some(Weight::Light);
        let mut d = Dropdown::open(Some(Role::Name), &list, &fonts);
        let view = d.view(3, &list, &fonts);
        assert_eq!((view.slot, view.role, view.top, view.total, view.choices.len()), (3, true, 3, 31, VISIBLE));
        assert_eq!(view.spot, Some(Spot::Item(9)));
        assert_eq!((view.size.as_str(), view.weight.as_str()), ("120%", "LIGHT"));
        assert_eq!(view.choices.iter().position(|c| c.current), Some(9));
        assert_eq!(view.choices[9].family.as_deref(), Some("Font 011"));
        d.scroll_by(-3);
        let view = d.view(3, &list, &fonts);
        assert_eq!((view.choices[0].family.clone(), view.choices[0].current, view.spot), (None, false, None), "GLOBAL is not current, the focus is out of view");
        let view = Dropdown::open(Some(Role::Tag), &list, &fonts).view(0, &list, &fonts);
        assert!(view.choices[0].current && view.choices[0].family.is_none(), "a role without a family of its own is on GLOBAL");
        assert_eq!(Dropdown::open(None, &list, &fonts).view(0, &list, &fonts).size, "");
        assert_eq!(weight_label(None), "DEFAULT");
    }
}
