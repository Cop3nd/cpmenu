//! Per-element colors from `%APPDATA%\cpmenu\style.toml`. Every color the menu draws has a named
//! key; a missing or invalid key keeps the value derived from the theme roles, so without the file
//! the look is exactly the theme's. The file is only ever created by "OPEN STYLE.TOML" in settings.

use std::path::PathBuf;
use std::time::SystemTime;

use toml_edit::{DocumentMut, Item};

use crate::config::{Config, Theme};

/// A color and an opacity factor. The factor scales whatever opacity the element is drawn with
/// (rows fade, hover brightens, ...), so `#RRGGBBAA` keeps working with animations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub rgb: u32,
    pub alpha: f32,
}

impl Paint {
    fn new(rgb: u32) -> Self {
        Self { rgb, alpha: 1.0 }
    }

    /// `#RRGGBB` or `#RRGGBBAA`.
    fn parse(text: &str) -> Option<Self> {
        let digits = text.trim().strip_prefix('#')?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        match digits.len() {
            6 => Some(Self::new(u32::from_str_radix(digits, 16).ok()?)),
            8 => {
                let value = u32::from_str_radix(digits, 16).ok()?;
                Some(Self { rgb: value >> 8, alpha: (value & 0xFF) as f32 / 255.0 })
            }
            _ => None,
        }
    }

    pub fn hex(self) -> String {
        if self.alpha >= 1.0 {
            format!("#{:06X}", self.rgb)
        } else {
            format!("#{:06X}{:02X}", self.rgb, (self.alpha * 255.0).round() as u32)
        }
    }
}

impl Paint {
    /// The opacity in whole percent.
    pub fn percent(self) -> u32 {
        (self.alpha * 100.0).round() as u32
    }

    /// The opacity one 5% step from the current one in direction `dir`, landing on a multiple of 5
    /// (a 37% color steps to 40% or 35%); `None` at 0% or 100%.
    pub fn stepped_alpha(self, dir: i32) -> Option<Self> {
        let now = self.percent() as i32;
        let to = if dir > 0 { (now / 5 + 1) * 5 } else { (now + 4) / 5 * 5 - 5 }.clamp(0, 100);
        (to != now).then(|| Self { rgb: self.rgb, alpha: to as f32 / 100.0 })
    }
}

/// How corners are drawn: rounded arcs, or straight cuts (the radius is the length of the cut).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Corner {
    #[default]
    Round,
    Chamfer,
}

/// The `[shape]` table of style.toml: pixel values at 100% scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub row_radius: f32,
    pub panel_radius: f32,
    pub tag_radius: f32,
    pub icon_radius: f32,
    pub corner: Corner,
    /// Multiplier of every border's width (1 = today's borders).
    pub border_width: f32,
    pub row_height: f32,
    pub row_gap: f32,
    /// Space between a TAG chip's edge and its text, left and right.
    pub tag_padding: f32,
    /// Space between two TAG chips.
    pub tag_gap: f32,
    /// Percent of the built-in chip height.
    pub tag_height: f32,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            row_radius: 0.0,
            panel_radius: 0.0,
            tag_radius: 0.0,
            icon_radius: 0.0,
            corner: Corner::Round,
            border_width: 1.0,
            row_height: 54.0,
            row_gap: 10.0,
            tag_padding: 5.0,
            tag_gap: 5.0,
            tag_height: 100.0,
        }
    }
}

/// The `[shape]` keys, in the order the settings panel lists them.
pub const SHAPE_KEYS: [&str; 11] =
    ["row_radius", "panel_radius", "tag_radius", "icon_radius", "corner", "border_width", "row_height", "row_gap", "tag_padding", "tag_gap", "tag_height"];

impl Shape {
    /// The allowed range of a numeric key.
    fn range(key: &str) -> (f32, f32) {
        match key {
            "border_width" => (0.0, 4.0),
            "row_height" => (36.0, 90.0),
            "row_gap" => (0.0, 30.0),
            "tag_padding" => (2.0, 16.0),
            "tag_gap" => (0.0, 16.0),
            "tag_height" => (60.0, 150.0),
            _ => (0.0, 24.0),
        }
    }

    fn number_mut(&mut self, key: &str) -> Option<&mut f32> {
        Some(match key {
            "row_radius" => &mut self.row_radius,
            "panel_radius" => &mut self.panel_radius,
            "tag_radius" => &mut self.tag_radius,
            "icon_radius" => &mut self.icon_radius,
            "border_width" => &mut self.border_width,
            "row_height" => &mut self.row_height,
            "row_gap" => &mut self.row_gap,
            "tag_padding" => &mut self.tag_padding,
            "tag_gap" => &mut self.tag_gap,
            "tag_height" => &mut self.tag_height,
            _ => return None,
        })
    }

    /// The number behind a numeric key.
    pub fn number(&self, key: &str) -> Option<f32> {
        let mut copy = *self;
        copy.number_mut(key).map(|n| *n)
    }

    /// Takes `item` as the value of `key` if it is valid; numbers are clamped into their range.
    fn read(&mut self, key: &str, item: &Item) {
        if key == "corner" {
            match item.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("round") => self.corner = Corner::Round,
                Some("chamfer") => self.corner = Corner::Chamfer,
                _ => {}
            }
            return;
        }
        let (lo, hi) = Self::range(key);
        let number = item.as_float().or_else(|| item.as_integer().map(|n| n as f64)).filter(|n| n.is_finite());
        if let (Some(number), Some(slot)) = (number, self.number_mut(key)) {
            *slot = (number as f32).clamp(lo, hi);
        }
    }

    /// Applies the valid entries of the `[shape]` table of `doc`.
    fn apply(&mut self, doc: &DocumentMut) {
        let Some(table) = doc.get("shape").and_then(Item::as_table_like) else { return };
        for key in SHAPE_KEYS {
            if let Some(item) = table.get(key) {
                self.read(key, item);
            }
        }
    }

    /// The value one step from `key`'s current one in direction `dir` (-1 or 1), as it is written to
    /// style.toml; `None` at the end of the range or for an unknown key. CORNER toggles.
    pub fn stepped(&self, key: &str, dir: i32) -> Option<toml_edit::Value> {
        let dir = dir.signum();
        if key == "corner" {
            let to = if self.corner == Corner::Round { "chamfer" } else { "round" };
            return Some(to.into());
        }
        let now = self.number(key)?;
        let (lo, hi) = Self::range(key);
        if key == "border_width" {
            let tenths = (now * 10.0).round() as i32 + dir * 5;
            let to = tenths.clamp((lo * 10.0) as i32, (hi * 10.0) as i32);
            return (to != (now * 10.0).round() as i32).then(|| (to as f64 / 10.0).into());
        }
        let step = match key {
            "row_height" | "row_gap" => 2,
            "tag_height" => 5,
            _ => 1,
        };
        let from = now.round() as i32;
        let to = (from + dir * step).clamp(lo as i32, hi as i32);
        (to != from).then(|| i64::from(to).into())
    }

    /// The commented `[shape]` lines of the style.toml template, with this shape's values.
    fn template(&self) -> String {
        let n = |v: f32| if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.1}") };
        let corner = if self.corner == Corner::Round { "round" } else { "chamfer" };
        format!(
            "\n# [shape]\n\
             # Corners, borders and row size, in pixels at 100% scale. Rounded corners need a radius above 0.\n\
             # row_radius = {}       # 0-24: rows, and the settings panel's rows, tabs and buttons\n\
             # panel_radius = {}     # 0-24: the settings card and the right-click popup\n\
             # tag_radius = {}       # 0-24: the TAG chips on a row\n\
             # icon_radius = {}      # 0-24: the icon box beside a row\n\
             # corner = \"{corner}\"      # round | chamfer: chamfer cuts the corners, the radius is the cut\n\
             # border_width = {}     # 0-4: scales every border; 0 draws none\n\
             # row_height = {}       # 36-90: with automatic ROWS the rows are this tall (text scales with them)\n\
             # row_gap = {}          # 0-30: space between rows\n\
             # tag_padding = {}      # 2-16: space between a TAG chip's edge and its text\n\
             # tag_gap = {}          # 0-16: space between two chips\n\
             # tag_height = {}      # 60-150: chip height in percent\n",
            n(self.row_radius),
            n(self.panel_radius),
            n(self.tag_radius),
            n(self.icon_radius),
            format!("{:.1}", self.border_width),
            n(self.row_height),
            n(self.row_gap),
            n(self.tag_padding),
            n(self.tag_gap),
            n(self.tag_height),
        )
    }
}

/// Where a header text sits in its line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    pub const ALL: [TextAlign; 3] = [TextAlign::Left, TextAlign::Center, TextAlign::Right];

    pub fn key(self) -> &'static str {
        match self {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key().eq_ignore_ascii_case(text.trim()))
    }
}

/// The `[show]` table of style.toml: what the menu draws, how rows dim, and where the header's texts sit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Show {
    /// Unselected rows keep the icon's own colors instead of the accent line-art look.
    pub color_icons: bool,
    /// Unselected rows are dimmer than the selected one (name, letters, detail).
    pub dim_unselected: bool,
    /// Rows cut off at the list's top / bottom fade out.
    pub edge_fade: bool,
    pub caption_align: TextAlign,
    pub title_align: TextAlign,
    pub count_align: TextAlign,
    pub scanlines: bool,
    pub glow: bool,
    /// The row's name (app / file name); off leaves the icon and the second line.
    pub name: bool,
    pub tags: bool,
    pub detail: bool,
    pub hint: bool,
    pub power_bar: bool,
    pub count: bool,
    pub progress: bool,
    pub caption: bool,
    /// The list's title (`header.subtitle`) shown while nothing is typed.
    pub subtitle: bool,
    pub header_line: bool,
    pub gear: bool,
    pub open_glitch: bool,
    pub row_cascade: bool,
    pub header_backdrop: bool,
}

impl Default for Show {
    fn default() -> Self {
        Self {
            color_icons: false,
            dim_unselected: true,
            edge_fade: true,
            caption_align: TextAlign::Left,
            title_align: TextAlign::Left,
            count_align: TextAlign::Right,
            scanlines: true,
            glow: true,
            name: true,
            tags: true,
            detail: true,
            hint: true,
            power_bar: true,
            count: true,
            progress: true,
            caption: true,
            subtitle: true,
            header_line: true,
            gear: true,
            open_glitch: true,
            row_cascade: true,
            header_backdrop: true,
        }
    }
}

/// The `[show]` keys, in the order the LOOK tab lists them: dimming, header alignment, elements.
pub const SHOW_KEYS: [&str; 22] = [
    "color_icons",
    "dim_unselected",
    "edge_fade",
    "caption_align",
    "title_align",
    "count_align",
    "caption",
    "subtitle",
    "header_line",
    "gear",
    "count",
    "progress",
    "header_backdrop",
    "scanlines",
    "glow",
    "name",
    "tags",
    "detail",
    "hint",
    "power_bar",
    "open_glitch",
    "row_cascade",
];
/// Where the groups of `SHOW_KEYS` start and end.
pub const SHOW_DIMMING: std::ops::Range<usize> = 0..3;
pub const SHOW_ALIGNS: std::ops::Range<usize> = 3..6;
/// The element toggles of the header, which LOOK lists in a pop-out under HEADER.
pub const SHOW_HEADER: std::ops::Range<usize> = 6..13;
pub const SHOW_ELEMENTS: std::ops::Range<usize> = 13..22;

impl Show {
    fn flag_mut(&mut self, key: &str) -> Option<&mut bool> {
        Some(match key {
            "color_icons" => &mut self.color_icons,
            "dim_unselected" => &mut self.dim_unselected,
            "edge_fade" => &mut self.edge_fade,
            "scanlines" => &mut self.scanlines,
            "glow" => &mut self.glow,
            "name" => &mut self.name,
            "tags" => &mut self.tags,
            "detail" => &mut self.detail,
            "hint" => &mut self.hint,
            "power_bar" => &mut self.power_bar,
            "count" => &mut self.count,
            "progress" => &mut self.progress,
            "caption" => &mut self.caption,
            "subtitle" => &mut self.subtitle,
            "header_line" => &mut self.header_line,
            "gear" => &mut self.gear,
            "open_glitch" => &mut self.open_glitch,
            "row_cascade" => &mut self.row_cascade,
            "header_backdrop" => &mut self.header_backdrop,
            _ => return None,
        })
    }

    fn align_mut(&mut self, key: &str) -> Option<&mut TextAlign> {
        Some(match key {
            "caption_align" => &mut self.caption_align,
            "title_align" => &mut self.title_align,
            "count_align" => &mut self.count_align,
            _ => return None,
        })
    }

    /// The on/off value behind a toggle key.
    pub fn flag(&self, key: &str) -> Option<bool> {
        let mut copy = *self;
        copy.flag_mut(key).map(|b| *b)
    }

    /// The alignment behind an `*_align` key.
    pub fn align(&self, key: &str) -> Option<TextAlign> {
        let mut copy = *self;
        copy.align_mut(key).map(|a| *a)
    }

    fn read(&mut self, key: &str, item: &Item) {
        if let Some(slot) = self.align_mut(key) {
            if let Some(align) = item.as_str().and_then(TextAlign::parse) {
                *slot = align;
            }
        } else if let (Some(slot), Some(on)) = (self.flag_mut(key), item.as_bool()) {
            *slot = on;
        }
    }

    /// Applies the valid entries of the `[show]` table of `doc`.
    fn apply(&mut self, doc: &DocumentMut) {
        let Some(table) = doc.get("show").and_then(Item::as_table_like) else { return };
        for key in SHOW_KEYS {
            if let Some(item) = table.get(key) {
                self.read(key, item);
            }
        }
    }

    /// The value one step from `key`'s current one: a toggle flips, an alignment moves round
    /// left, center, right in direction `dir`. `None` for an unknown key.
    pub fn stepped(&self, key: &str, dir: i32) -> Option<toml_edit::Value> {
        if let Some(on) = self.flag(key) {
            return Some((!on).into());
        }
        let now = self.align(key)?;
        let at = TextAlign::ALL.iter().position(|&a| a == now)? as i32;
        Some(TextAlign::ALL[(at + dir.signum()).rem_euclid(3) as usize].key().into())
    }

    /// The commented `[show]` lines of the style.toml template.
    fn template() -> String {
        format!(
            "\n# [show]\n\
             # What the menu draws. Every key is on (true) by default except color_icons.\n\
             # color_icons = false       # true: unselected rows keep their icons' colors\n\
             # dim_unselected = true     # false: unselected rows are as bright as the selected one\n\
             # edge_fade = true          # false: rows cut off at the list's ends are not faded\n\
             # caption_align = \"left\"    # left | center | right: the NETRUNNER_OS line\n\
             # title_align = \"left\"      # the title / query line\n\
             # count_align = \"right\"     # the match count or notice, beside the title\n\
             # {}\n",
            SHOW_KEYS[SHOW_HEADER.start..SHOW_ELEMENTS.end].iter().map(|k| format!("{k} = true")).collect::<Vec<_>>().join("\n# ")
        )
    }
}

/// The font every role uses unless style.toml says otherwise; also what a family that isn't
/// installed falls back to.
pub const DEFAULT_FAMILY: &str = "Bahnschrift";

/// A font family name in a fixed buffer, so `Style` stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Family {
    len: u8,
    bytes: [u8; 63],
}

impl Family {
    /// `text` trimmed; `None` if it is empty or has control characters. Names longer than the
    /// buffer (no real family is) are cut at a character boundary.
    pub fn new(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text.chars().any(char::is_control) {
            return None;
        }
        let mut end = text.len().min(63);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0; 63];
        bytes[..end].copy_from_slice(&text.as_bytes()[..end]);
        Some(Self { len: end as u8, bytes })
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or(DEFAULT_FAMILY)
    }
}

impl std::fmt::Debug for Family {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl Default for Family {
    fn default() -> Self {
        Self::new(DEFAULT_FAMILY).unwrap_or(Self { len: 0, bytes: [0; 63] })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Light,
    Normal,
    Semibold,
    Bold,
}

pub const WEIGHTS: [Weight; 4] = [Weight::Light, Weight::Normal, Weight::Semibold, Weight::Bold];

impl Weight {
    pub fn key(self) -> &'static str {
        match self {
            Weight::Light => "light",
            Weight::Normal => "normal",
            Weight::Semibold => "semibold",
            Weight::Bold => "bold",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        WEIGHTS.into_iter().find(|w| w.key().eq_ignore_ascii_case(text.trim()))
    }

    /// The OpenType weight (`DWRITE_FONT_WEIGHT`).
    pub fn value(self) -> i32 {
        match self {
            Weight::Light => 300,
            Weight::Normal => 400,
            Weight::Semibold => 600,
            Weight::Bold => 700,
        }
    }
}

/// What a piece of text is, for the `[fonts]` table. Every text format the renderer builds belongs to one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    /// The header's title line (the list's title).
    Header,
    /// The query line (and the row name shown in its place on the actions page).
    Query,
    /// A row's name and the letters standing in for its icon.
    #[default]
    Name,
    Tag,
    /// Header notices, the pop-out's enter mark and a row's ENTER hint.
    Hint,
    /// A row's detail line.
    Detail,
    /// Small print: the header's top line and the settings panel's notes.
    Caption,
    /// Settings labels and headings, pop-out and popup entries.
    Label,
    /// Settings values and the dialog's buttons.
    Value,
    /// The power bar, the settings tabs and the panel's buttons.
    Button,
    DialogTitle,
    DialogText,
}

pub const ROLES: [Role; 12] = [
    Role::Header,
    Role::Query,
    Role::Name,
    Role::Tag,
    Role::Hint,
    Role::Detail,
    Role::Caption,
    Role::Label,
    Role::Value,
    Role::Button,
    Role::DialogTitle,
    Role::DialogText,
];

impl Role {
    pub fn key(self) -> &'static str {
        match self {
            Role::Header => "header",
            Role::Query => "query",
            Role::Name => "name",
            Role::Tag => "tag",
            Role::Hint => "hint",
            Role::Detail => "detail",
            Role::Caption => "caption",
            Role::Label => "label",
            Role::Value => "value",
            Role::Button => "button",
            Role::DialogTitle => "dialog_title",
            Role::DialogText => "dialog_text",
        }
    }

    fn index(self) -> usize {
        ROLES.iter().position(|&r| r == self).unwrap_or(0)
    }

    /// The weight the role is drawn with when style.toml doesn't say. (The dialog's CONFIRM? line
    /// is semibold; its caption, which this shows, is normal.)
    #[cfg(test)]
    pub fn builtin_weight(self) -> Weight {
        match self {
            Role::Header | Role::Query | Role::Value | Role::DialogTitle => Weight::Bold,
            Role::Name | Role::Tag | Role::Hint | Role::Label | Role::Button => Weight::Semibold,
            Role::Detail | Role::Caption | Role::DialogText => Weight::Normal,
        }
    }
}

/// The range of a role's `size`, percent of the built-in size.
pub const FONT_SIZE_RANGE: (u32, u32) = (50, 200);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoleFont {
    /// `None`: the global family.
    pub family: Option<Family>,
    /// Percent; changes the text only, never the boxes around it.
    pub size: u32,
    /// `None`: the role's built-in weight.
    pub weight: Option<Weight>,
}

impl Default for RoleFont {
    fn default() -> Self {
        Self { family: None, size: 100, weight: None }
    }
}

/// The `[fonts]` table of style.toml: `family`, and per role `[fonts.<role>]` with `family`, `size`, `weight`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fonts {
    pub family: Family,
    pub roles: [RoleFont; 12],
}

impl Fonts {
    pub fn role(&self, role: Role) -> &RoleFont {
        &self.roles[role.index()]
    }

    /// The family `role` is drawn in (not yet checked against the installed fonts).
    pub fn family_of(&self, role: Role) -> Family {
        self.role(role).family.unwrap_or(self.family)
    }

    #[cfg(test)]
    pub fn weight_of(&self, role: Role) -> Weight {
        self.role(role).weight.unwrap_or(role.builtin_weight())
    }

    /// Whether `key` (`family`, or `<role>.family|size|weight`) names a `[fonts]` entry.
    pub fn valid_key(key: &str) -> bool {
        match key.split_once('.') {
            None => key == "family",
            Some((role, field)) => ROLES.iter().any(|r| r.key() == role) && matches!(field, "family" | "size" | "weight"),
        }
    }

    /// Takes `item` as the value of `key` if it is valid; a size is clamped into its range.
    fn read(&mut self, key: &str, item: &Item) {
        let Some((role, field)) = key.split_once('.') else {
            if let Some(family) = item.as_str().and_then(Family::new) {
                self.family = family;
            }
            return;
        };
        let Some(role) = ROLES.iter().find(|r| r.key() == role) else { return };
        let slot = &mut self.roles[role.index()];
        match field {
            "family" => {
                if let Some(family) = item.as_str().and_then(Family::new) {
                    slot.family = Some(family);
                }
            }
            "size" => {
                let number = item.as_float().or_else(|| item.as_integer().map(|n| n as f64)).filter(|n| n.is_finite());
                if let Some(number) = number {
                    slot.size = (number.round() as i64).clamp(FONT_SIZE_RANGE.0 as i64, FONT_SIZE_RANGE.1 as i64) as u32;
                }
            }
            "weight" => {
                if let Some(weight) = item.as_str().and_then(Weight::parse) {
                    slot.weight = Some(weight);
                }
            }
            _ => {}
        }
    }

    /// Applies the valid entries of the `[fonts]` table of `doc`.
    fn apply(&mut self, doc: &DocumentMut) {
        let Some(table) = doc.get("fonts").and_then(Item::as_table_like) else { return };
        if let Some(item) = table.get("family") {
            self.read("family", item);
        }
        for role in ROLES {
            let Some(sub) = table.get(role.key()).and_then(Item::as_table_like) else { continue };
            for field in ["family", "size", "weight"] {
                if let Some(item) = sub.get(field) {
                    self.read(&format!("{}.{field}", role.key()), item);
                }
            }
        }
    }

    /// The commented `[fonts]` lines of the style.toml template.
    fn template() -> String {
        format!(
            "\n# [fonts]\n\
             # Fonts. A family that isn't installed falls back to {DEFAULT_FAMILY}; sizes change only the text, not the boxes.\n\
             # family = \"{DEFAULT_FAMILY}\"      # every role, unless it has its own\n\
             # Roles: {}.\n\
             # Each takes family, size (50-200 percent) and weight (light, normal, semibold, bold). For example:\n\
             # [fonts.name]\n\
             # size = 100\n",
            ROLES.map(Role::key).join(" ")
        )
    }
}

macro_rules! style {
    ($($field:ident $key:literal |$t:ident| $default:expr;)*) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Style {
            $(pub $field: Paint,)*
            pub shape: Shape,
            pub fonts: Fonts,
            pub show: Show,
        }

        /// Every key, in the order the settings panel lists them.
        pub const KEYS: &[&str] = &[$($key,)*];

        impl Style {
            /// Today's colors: every element derived from the theme's roles.
            pub fn from_theme(theme: &Theme) -> Self {
                Self { $($field: { let $t = theme; Paint::new($default) },)* shape: Shape::default(), fonts: Fonts::default(), show: Show::default() }
            }

            fn entries(&self) -> Vec<(&'static str, Paint)> {
                vec![$(($key, self.$field),)*]
            }

            fn slot(&mut self, key: &str) -> Option<&mut Paint> {
                match key {
                    $($key => Some(&mut self.$field),)*
                    _ => None,
                }
            }
        }
    };
}

style! {
    header_backdrop "header.backdrop" |t| t.backdrop;
    header_caption "header.caption" |t| t.alert;
    header_line "header.line" |t| t.alert;
    header_progress "header.progress" |t| t.alert;
    header_title "header.title" |t| t.alert;
    header_glitch "header.glitch" |t| t.accent;
    header_query "header.query" |t| t.accent;
    header_count "header.count" |t| t.alert;

    row_panel "row.panel" |t| t.panel;
    row_name "row.name" |t| t.text_dim;
    row_name_selected "row.name_selected" |t| t.text;
    row_border "row.border" |t| t.accent;
    row_border_selected "row.border_selected" |t| t.accent;
    row_glow "row.glow" |t| t.accent;
    row_scanline "row.scanline" |t| t.accent;
    row_hint "row.hint" |t| t.accent;
    row_detail "row.detail" |t| t.text_dim;
    row_glyph "row.glyph" |t| t.accent;
    row_status "row.status" |t| t.accent;
    row_icon "row.icon" |t| t.accent;

    tag_app "tag.app" |t| t.accent;
    tag_pinned "tag.pinned" |t| t.tag;
    tag_file "tag.file" |t| t.tag;
    tag_folder "tag.folder" |t| t.tag;
    tag_cmd "tag.cmd" |t| t.quick;
    tag_calc "tag.calc" |t| t.quick;
    tag_web "tag.web" |t| t.quick;
    tag_open "tag.open" |t| t.quick;
    tag_setting "tag.setting" |t| t.setting;
    tag_exec "tag.exec" |t| t.accent;
    tag_plugin "tag.plugin" |t| t.quick;

    power_line "power.line" |t| t.alert;
    power_fill "power.fill" |t| t.panel;
    power_button "power.button" |t| t.accent;
    power_button_danger "power.button_danger" |t| t.alert;
    power_text "power.text" |t| t.accent;
    power_text_danger "power.text_danger" |t| t.alert;

    overlay_dim "overlay.dim" |t| t.backdrop;
    overlay_glitch_a "overlay.glitch_a" |t| t.accent;
    overlay_glitch_b "overlay.glitch_b" |t| t.alert;
    overlay_glitch_c "overlay.glitch_c" |t| t.quick;
    overlay_title "overlay.title" |t| t.alert;
    overlay_caption "overlay.caption" |t| t.accent;
    overlay_text "overlay.text" |t| t.text;
    overlay_button "overlay.button" |t| t.accent;
    overlay_button_fill "overlay.button_fill" |t| t.panel;
    overlay_button_hover "overlay.button_hover" |t| t.accent;
    overlay_button_danger "overlay.button_danger" |t| t.alert;

    panel_fill "panel.fill" |t| t.backdrop;
    panel_frame "panel.frame" |t| t.alert;
    panel_strip "panel.strip" |t| t.alert;
    panel_strip_mark "panel.strip_mark" |_t| 0xFFFFFF;
    panel_corner "panel.corner" |t| t.alert;
    panel_title "panel.title" |t| t.alert;
    panel_tab "panel.tab" |t| t.alert;
    panel_tab_fill "panel.tab_fill" |t| t.panel;
    panel_tab_text "panel.tab_text" |t| t.text_dim;
    panel_tab_active "panel.tab_active" |t| t.alert;
    panel_tab_active_text "panel.tab_active_text" |t| t.text;
    panel_row_fill "panel.row_fill" |t| t.panel;
    panel_row_border "panel.row_border" |t| t.accent;
    panel_row_glow "panel.row_glow" |t| t.accent;
    panel_label "panel.label" |t| t.text_dim;
    panel_value "panel.value" |t| t.text;
    panel_arrow "panel.arrow" |t| t.accent;
    panel_note "panel.note" |t| t.text_dim;
    panel_error "panel.error" |t| t.alert;
    panel_button "panel.button" |t| t.accent;
    panel_button_fill "panel.button_fill" |t| t.panel;

    popup_fill "popup.fill" |t| t.panel;
    popup_border "popup.border" |t| t.accent;
    popup_glow "popup.glow" |t| t.accent;
    popup_text "popup.text" |t| t.text;
    popup_hint "popup.hint" |t| t.accent;
}

impl Style {
    /// The theme's colors with the valid entries of `text` (style.toml) applied on top. A file
    /// that doesn't parse changes nothing; so does any key that is missing or not a color.
    pub fn resolve(theme: &Theme, text: &str) -> Self {
        Self::resolve_layers(theme, &[text])
    }

    /// The theme's colors with each of `layers` (a theme file, then style.toml) applied in order.
    pub fn resolve_layers(theme: &Theme, layers: &[&str]) -> Self {
        let mut style = Self::from_theme(theme);
        for text in layers {
            let Ok(doc) = text.parse::<DocumentMut>() else { continue };
            style.shape.apply(&doc);
            style.fonts.apply(&doc);
            style.show.apply(&doc);
            for (key, _) in style.entries() {
                let Some((table, name)) = key.split_once('.') else { continue };
                let paint = doc.get(table).and_then(|t| t.get(name)).and_then(Item::as_str).and_then(Paint::parse);
                if let (Some(paint), Some(slot)) = (paint, style.slot(key)) {
                    *slot = paint;
                }
            }
        }
        style
    }

    /// The (key, color) pairs that differ from `base`.
    pub fn changed_from(&self, base: &Style) -> Vec<(&'static str, Paint)> {
        self.entries().into_iter().filter(|(key, paint)| base.get(key) != Some(*paint)).collect()
    }

    /// A style.toml listing every key, commented out, with this style's current value.
    pub fn template(&self) -> String {
        let mut out = String::from(
            "# cpmenu element colors. A key you leave commented out follows the theme (the value shown).\n\
             # \"#RRGGBB\" sets a color; \"#RRGGBBAA\" also scales that element's opacity by AA/255.\n\
             # Changes are picked up the next time the menu opens.\n",
        );
        let mut group = "";
        for (key, paint) in self.entries() {
            let (table, name) = key.split_once('.').unwrap_or(("", key));
            if table != group {
                out.push_str(&format!("\n# [{table}]\n"));
                group = table;
            }
            out.push_str(&format!("# {name} = \"{}\"\n", paint.hex()));
        }
        out.push_str(&self.shape.template());
        out.push_str(&Show::template());
        out.push_str(&Fonts::template());
        out
    }
}

impl Style {
    /// The color of `key` (`"row.border"`), if there is such a key.
    pub fn get(&self, key: &str) -> Option<Paint> {
        let mut copy = *self;
        copy.slot(key).map(|p| *p)
    }
}

/// Typed color input: `#RRGGBB` or `#RRGGBBAA`, the `#` optional. Returns the text to store.
pub fn normalize_hex(input: &str) -> Option<String> {
    let input = input.trim();
    let text = if input.starts_with('#') { input.to_owned() } else { format!("#{input}") };
    Paint::parse(&text).map(Paint::hex)
}

/// Colors the panel steps through after the theme's own roles.
const EXTRA_COLORS: [u32; 12] = [
    0x00F0FF, 0xFF2D2D, 0xFCEE0A, 0xFF2BD6, 0x39FF14, 0xFF8A00, 0xFFFFFF, 0x000000, 0x202020, 0x555555, 0x9A9A9A, 0xD0D0D0,
];

/// The theme's role colors, then `EXTRA_COLORS`, without duplicates.
pub fn palette(theme: &Theme) -> Vec<u32> {
    let roles = [theme.accent, theme.alert, theme.tag, theme.quick, theme.setting, theme.text, theme.text_dim, theme.panel, theme.backdrop];
    let mut out: Vec<u32> = Vec::new();
    for color in roles.into_iter().chain(EXTRA_COLORS) {
        if !out.contains(&color) {
            out.push(color);
        }
    }
    out
}

/// The palette color `dir` steps from `current` (wrapping), keeping its opacity. A color that isn't
/// in the palette steps to the first entry going right and the last going left.
pub fn step_color(current: Paint, palette: &[u32], dir: i32) -> Paint {
    let len = palette.len() as i32;
    let next = match palette.iter().position(|&c| c == current.rgb) {
        Some(at) => (at as i32 + dir.signum()).rem_euclid(len),
        None if dir < 0 => len - 1,
        None => 0,
    };
    Paint { rgb: palette[next as usize], alpha: current.alpha }
}

/// `text` with `key` ("row.border") set to `hex`, or removed when `hex` is `None`. `None` if `text`
/// isn't valid TOML or the key's table is something else, so the user's file is never clobbered.
pub(crate) fn set_in(text: &str, key: &str, hex: Option<&str>) -> Option<String> {
    set_value_in(text, key, hex.map(toml_edit::Value::from))
}

/// `set_in` for any value: a color string or a `[shape]` number.
fn set_value_in(text: &str, key: &str, value: Option<toml_edit::Value>) -> Option<String> {
    let (table_name, name) = key.split_once('.')?;
    let mut doc = text.parse::<DocumentMut>().ok()?;
    match value {
        Some(new) => {
            match doc.get(table_name) {
                None => {
                    doc.insert(table_name, toml_edit::table());
                }
                Some(item) if item.as_table_like().is_none() => return None,
                Some(_) => {}
            }
            let table = doc.get_mut(table_name)?.as_table_like_mut()?;
            let mut new = new;
            if let Some(old) = table.get(name).and_then(Item::as_value) {
                *new.decor_mut() = old.decor().clone();
            }
            table.insert(name, Item::Value(new));
        }
        None => {
            if let Some(table) = doc.get_mut(table_name).and_then(Item::as_table_like_mut) {
                table.remove(name);
            }
        }
    }
    Some(doc.to_string())
}

/// `hex` unless it is the color `theme` already gives `key`: such an entry would show as following
/// the theme while pinning today's color, and go stale on the next theme switch. Not storing it
/// keeps "in style.toml" and "differs from the theme" the same thing.
fn explicit_hex<'a>(defaults: &Style, key: &str, hex: Option<&'a str>) -> Option<&'a str> {
    let default = defaults.get(key)?;
    hex.filter(|h| Paint::parse(h) != Some(default))
}

/// Writes `key` into style.toml (`Some(hex)`) or removes it (`None`), creating the file from the
/// template if it doesn't exist. `defaults` are the theme's own element colors. False when nothing
/// was written: the file has syntax errors, or can't be read or written.
pub fn set_color(defaults: &Style, key: &str, hex: Option<&str>) -> bool {
    let Some(path) = path() else { return false };
    if !KEYS.contains(&key) {
        return false;
    }
    let hex = explicit_hex(defaults, key, hex);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if hex.is_none() {
                return true;
            }
            defaults.template()
        }
        Err(_) => return false,
    };
    let Some(updated) = set_in(&text, key, hex) else { return false };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, updated).is_ok()
}

/// `value` unless it gives `key` the value the theme layer already has (stored, it would pin
/// today's value like an explicit color does).
fn explicit_shape(defaults: &Shape, key: &str, value: toml_edit::Value) -> Option<toml_edit::Value> {
    let mut probe = *defaults;
    probe.read(key, &Item::Value(value.clone()));
    (probe != *defaults).then_some(value)
}

/// Writes `shape.<key>` into style.toml, or removes it for `None` or a value equal to the theme's
/// (`defaults`); the file is created from the template if missing. False when nothing was written.
pub fn set_shape(defaults: &Style, key: &str, value: Option<toml_edit::Value>) -> bool {
    let Some(path) = path() else { return false };
    if !SHAPE_KEYS.contains(&key) {
        return false;
    }
    let value = value.and_then(|v| explicit_shape(&defaults.shape, key, v));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if value.is_none() {
                return true;
            }
            defaults.template()
        }
        Err(_) => return false,
    };
    let Some(updated) = set_value_in(&text, &format!("shape.{key}"), value) else { return false };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, updated).is_ok()
}

/// `value` unless it gives `key` the value the theme layer already has.
fn explicit_show(defaults: &Show, key: &str, value: toml_edit::Value) -> Option<toml_edit::Value> {
    let mut probe = *defaults;
    probe.read(key, &Item::Value(value.clone()));
    (probe != *defaults).then_some(value)
}

/// `text` with every `[show]` change applied: `Some` sets the key, `None` removes it, and a value
/// equal to the theme's (`defaults`) is removed rather than stored. `None` if `text` isn't valid
/// TOML or `show` is something other than a table.
fn set_show_in(text: &str, defaults: &Show, changes: &[(&str, Option<toml_edit::Value>)]) -> Option<String> {
    let mut text = text.to_owned();
    for (key, value) in changes {
        if !SHOW_KEYS.contains(key) {
            return None;
        }
        let value = value.clone().and_then(|v| explicit_show(defaults, key, v));
        text = set_value_in(&text, &format!("show.{key}"), value)?;
    }
    Some(text)
}

/// Writes `[show]` keys into style.toml in one go, or removes them (`None`); the file is created
/// from the template if missing. False when nothing was written.
pub fn set_show(defaults: &Style, changes: &[(&str, Option<toml_edit::Value>)]) -> bool {
    let Some(path) = path() else { return false };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if changes.iter().all(|(_, value)| value.is_none()) {
                return true;
            }
            defaults.template()
        }
        Err(_) => return false,
    };
    let Some(updated) = set_show_in(&text, &defaults.show, changes) else { return false };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, updated).is_ok()
}

/// `text` with `[fonts]` entry `key` (`family`, or `<role>.size`...) set, or removed for `None`
/// (an emptied role table goes with it). `None` if `text` isn't valid TOML or `fonts` / the role
/// is something other than a table.
fn set_font_in(text: &str, key: &str, value: Option<toml_edit::Value>) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    let (role, name) = match key.split_once('.') {
        Some((role, name)) => (Some(role), name),
        None => (None, key),
    };
    match value {
        Some(mut new) => {
            match doc.get("fonts") {
                None => {
                    doc.insert("fonts", toml_edit::table());
                }
                Some(item) if item.as_table_like().is_none() => return None,
                Some(_) => {}
            }
            let fonts = doc.get_mut("fonts")?.as_table_like_mut()?;
            if let Some(role) = role {
                match fonts.get(role) {
                    None => {
                        fonts.insert(role, toml_edit::table());
                    }
                    Some(item) if item.as_table_like().is_none() => return None,
                    Some(_) => {}
                }
            }
            let table = match role {
                Some(role) => fonts.get_mut(role)?.as_table_like_mut()?,
                None => fonts,
            };
            if let Some(old) = table.get(name).and_then(Item::as_value) {
                *new.decor_mut() = old.decor().clone();
            }
            table.insert(name, Item::Value(new));
        }
        None => {
            let Some(fonts) = doc.get_mut("fonts").and_then(Item::as_table_like_mut) else { return Some(doc.to_string()) };
            match role {
                None => {
                    fonts.remove(name);
                }
                Some(role) => {
                    if let Some(sub) = fonts.get_mut(role).and_then(Item::as_table_like_mut) {
                        sub.remove(name);
                        if sub.is_empty() {
                            fonts.remove(role);
                        }
                    }
                }
            }
        }
    }
    Some(doc.to_string())
}

/// `value` unless it gives `key` the value the theme layer already has.
fn explicit_font(defaults: &Fonts, key: &str, value: toml_edit::Value) -> Option<toml_edit::Value> {
    let mut probe = *defaults;
    probe.read(key, &Item::Value(value.clone()));
    (probe != *defaults).then_some(value)
}

/// Writes `fonts.<key>` into style.toml, or removes it for `None` or a value equal to the theme's
/// (`defaults`); the file is created from the template if missing. False when nothing was written.
pub fn set_font(defaults: &Style, key: &str, value: Option<toml_edit::Value>) -> bool {
    let Some(path) = path() else { return false };
    if !Fonts::valid_key(key) {
        return false;
    }
    let value = value.and_then(|v| explicit_font(&defaults.fonts, key, v));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if value.is_none() {
                return true;
            }
            defaults.template()
        }
        Err(_) => return false,
    };
    let Some(updated) = set_font_in(&text, key, value) else { return false };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, updated).is_ok()
}

/// `text` with every color key removed (comments, other keys and the tables stay).
/// `None` if `text` isn't valid TOML.
pub(crate) fn clear_in(text: &str) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    for key in KEYS {
        let (table, name) = key.split_once('.')?;
        if let Some(table) = doc.get_mut(table).and_then(Item::as_table_like_mut) {
            table.remove(name);
        }
    }
    Some(doc.to_string())
}

/// Whether style.toml can be edited: missing, or valid TOML.
pub fn editable() -> bool {
    match path().map(std::fs::read_to_string) {
        Some(Ok(text)) => text.parse::<DocumentMut>().is_ok(),
        Some(Err(e)) => e.kind() == std::io::ErrorKind::NotFound,
        None => false,
    }
}

/// Removes all color overrides from style.toml (they moved into a saved theme). A file that
/// doesn't parse is left alone and reported as false.
pub fn clear_overrides() -> bool {
    let Some(path) = path() else { return false };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => return e.kind() == std::io::ErrorKind::NotFound,
    };
    clear_in(&text).is_some_and(|cleared| std::fs::write(&path, cleared).is_ok())
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("cpmenu").join("style.toml"))
}

/// Lets the menu notice hand edits without re-reading the file on every open.
pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

/// The style of `config`'s theme, with style.toml applied if it exists.
pub fn load(config: &Config) -> Style {
    let text = path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    Style::resolve_layers(&config.colors, &[&config.theme_elements, &text])
}

/// Path of style.toml, created from the template for the theme's colors `defaults` if it doesn't exist yet.
pub fn ensure_file(defaults: &Style) -> Option<String> {
    let path = path()?;
    if !path.exists() {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, defaults.template()).ok()?;
    }
    path.to_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::THEMES;

    fn theme(name: &str) -> Theme {
        Theme::preset(name).unwrap()
    }

    #[test]
    fn empty_style_is_exactly_the_themes_colors() {
        for name in THEMES {
            let t = theme(name);
            let s = Style::resolve(&t, "");
            assert_eq!(s, Style::from_theme(&t));
            assert!(s.entries().iter().all(|(_, p)| p.alpha == 1.0));
        }
        // Spot-check against the colors that were hard-coded before this file existed.
        let s = Style::from_theme(&theme("cyberpunk"));
        assert_eq!(s.row_border.rgb, 0x5EF6FF);
        assert_eq!(s.row_name.rgb, 0x8FDDE6);
        assert_eq!(s.row_name_selected.rgb, 0xE2FFFF);
        assert_eq!(s.row_panel.rgb, 0x0B1A2A);
        assert_eq!(s.header_title.rgb, 0xFF5E57);
        assert_eq!(s.header_backdrop.rgb, 0x03070D);
        assert_eq!((s.tag_file.rgb, s.tag_cmd.rgb, s.tag_app.rgb), (0xF5A623, 0xFCEE0A, 0x5EF6FF));
        assert_eq!(s.power_button_danger.rgb, 0xFF5E57);
        assert_eq!((s.overlay_dim.rgb, s.overlay_title.rgb, s.overlay_glitch_c.rgb), (0x03070D, 0xFF5E57, 0xFCEE0A));
    }

    #[test]
    fn invalid_or_unknown_keys_fall_back_individually() {
        let t = theme("cyberpunk");
        let text = "[row]\nname = \"#112233\"\nborder = \"red\"\npanel = \"#12345\"\nglow = 5\nnope = \"#000000\"\n[tag]\nfile = \"#abcdef\"\n";
        let s = Style::resolve(&t, text);
        let base = Style::from_theme(&t);
        assert_eq!(s.row_name.rgb, 0x112233);
        assert_eq!(s.tag_file.rgb, 0xABCDEF);
        assert_eq!((s.row_border, s.row_panel, s.row_glow), (base.row_border, base.row_panel, base.row_glow));
        assert_eq!(Style::resolve(&t, "[row\nname = "), base, "broken file changes nothing");
    }

    #[test]
    fn eight_digit_colors_set_the_opacity_factor() {
        let p = Paint::parse("#FF000080").unwrap();
        assert_eq!(p.rgb, 0xFF0000);
        assert!((p.alpha - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(Paint::parse("#FF0000").unwrap().alpha, 1.0);
        assert!(Paint::parse("FF0000").is_none() && Paint::parse("#FF00").is_none() && Paint::parse("#GG0000").is_none());
        let s = Style::resolve(&theme("cyberpunk"), "[header]\nbackdrop = \"#00000040\"\n");
        assert_eq!((s.header_backdrop.rgb, (s.header_backdrop.alpha * 255.0).round() as u32), (0, 0x40));
    }

    #[test]
    fn dotted_keys_work_like_tables() {
        let s = Style::resolve(&theme("cyberpunk"), "popup.text = \"#010203\"\n");
        assert_eq!(s.popup_text.rgb, 0x010203);
    }

    #[test]
    fn keys_are_unique_and_the_template_lists_all_of_them() {
        let s = Style::from_theme(&theme("arasaka"));
        let keys: Vec<_> = s.entries().iter().map(|e| e.0).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "duplicate key");
        let template = s.template();
        for (key, paint) in s.entries() {
            let name = key.split_once('.').unwrap().1;
            assert!(template.contains(&format!("# {name} = \"{}\"", paint.hex())), "{key}");
        }
        assert!(template.contains("# [header]") && template.contains("# [popup]") && template.contains("# [overlay]"));
    }

    #[test]
    fn template_with_every_line_enabled_resolves_to_the_same_style() {
        let t = theme("militech");
        let base = Style::from_theme(&t);
        let enabled: String = base
            .template()
            .lines()
            .map(|l| if l.starts_with("# [") || (l.starts_with("# ") && l.contains(" = ")) { &l[2..] } else { l })
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(Style::resolve(&t, &enabled), base);
        // And it really did set the values rather than silently falling back to defaults.
        let changed = enabled.replace("name = \"#C9A24A\"", "name = \"#000001\"");
        assert_eq!(Style::resolve(&t, &changed).row_name.rgb, 0x000001);
    }

    #[test]
    fn typed_hex_accepts_both_lengths_with_or_without_hash() {
        assert_eq!(normalize_hex("5ef6ff").as_deref(), Some("#5EF6FF"));
        assert_eq!(normalize_hex(" #5ef6ff80 ").as_deref(), Some("#5EF6FF80"));
        assert_eq!(normalize_hex("#abcdefff").as_deref(), Some("#ABCDEF"), "full opacity is the plain form");
        for bad in ["", "#", "12345", "#GG0000", "1234567", "red"] {
            assert_eq!(normalize_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn palette_has_the_theme_roles_first_and_no_duplicates() {
        for name in THEMES {
            let t = theme(name);
            let p = palette(&t);
            assert_eq!(&p[..2], &[t.accent, t.alert]);
            let mut sorted = p.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), p.len(), "{name}");
            assert!(p.contains(&0xFFFFFF) && p.contains(&0x000000));
        }
        let mut t = theme("cyberpunk");
        t.tag = t.accent;
        assert_eq!(palette(&t).iter().filter(|&&c| c == t.accent).count(), 1);
    }

    #[test]
    fn stepping_wraps_keeps_opacity_and_handles_unknown_colors() {
        let p = [1, 2, 3];
        let at = |rgb, alpha| Paint { rgb, alpha };
        assert_eq!(step_color(at(1, 1.0), &p, 1).rgb, 2);
        assert_eq!(step_color(at(3, 1.0), &p, 1).rgb, 1, "wraps forward");
        assert_eq!(step_color(at(1, 1.0), &p, -1).rgb, 3, "wraps back");
        assert_eq!(step_color(at(9, 1.0), &p, 1).rgb, 1);
        assert_eq!(step_color(at(9, 1.0), &p, -1).rgb, 3);
        assert_eq!(step_color(at(2, 0.5), &p, 1), at(3, 0.5));
    }

    #[test]
    fn writing_a_color_keeps_comments_and_other_keys() {
        let t = theme("cyberpunk");
        let base = Style::from_theme(&t);
        let text = "# mine\n[row]\nname = \"#112233\" # keep\nborder = \"#445566\"\n";
        let out = set_in(text, "row.name", Some("#ABCDEF")).unwrap();
        assert!(out.contains("# mine") && out.contains("# keep"));
        let s = Style::resolve(&t, &out);
        assert_eq!((s.row_name.rgb, s.row_border.rgb), (0xABCDEF, 0x445566));
        let out = set_in(&out, "popup.text", Some("#010203")).unwrap();
        assert_eq!(Style::resolve(&t, &out).popup_text.rgb, 0x010203);
        let out = set_in(&out, "row.name", None).unwrap();
        let s = Style::resolve(&t, &out);
        assert_eq!((s.row_name, s.row_border.rgb, s.popup_text.rgb), (base.row_name, 0x445566, 0x010203), "reset restores the default");
        assert_eq!(set_in("", "row.name", None).as_deref(), Some(""), "removing from an empty file is a no-op");
    }

    #[test]
    fn a_removed_key_follows_whatever_theme_is_active() {
        let (old, new) = (theme("cyberpunk"), theme("arasaka"));
        let mut text = Style::from_theme(&old).template();
        for (key, hex) in [("row.name", "#112233"), ("tag.file", "#445566")] {
            text = set_in(&text, key, Some(hex)).unwrap();
        }
        // Reset the way the panel does: the key goes away, whatever theme wrote the file.
        for key in ["row.name", "tag.file"] {
            text = set_in(&text, key, None).unwrap();
        }
        assert_eq!(Style::resolve(&new, &text), Style::from_theme(&new));
        assert!(!text.contains("112233") && !text.contains("445566"), "{text}");
    }

    #[test]
    fn a_hex_equal_to_the_theme_color_is_not_stored() {
        let t = Style::from_theme(&theme("cyberpunk"));
        assert_eq!(explicit_hex(&t, "row.border", Some("#5EF6FF")), None, "same as the theme: follow the theme");
        assert_eq!(explicit_hex(&t, "row.border", Some("#5ef6ff")), None);
        assert_eq!(explicit_hex(&t, "row.border", Some("#5EF6FF80")), Some("#5EF6FF80"), "other opacity is a change");
        assert_eq!(explicit_hex(&t, "row.border", Some("#000001")), Some("#000001"));
        assert_eq!(explicit_hex(&t, "row.border", None), None);
        assert_eq!(explicit_hex(&t, "nope.x", Some("#000001")), None);
    }

    #[test]
    fn writing_into_the_template_creates_a_table_and_never_clobbers_broken_files() {
        let t = theme("arasaka");
        let out = set_in(&Style::from_theme(&t).template(), "tag.file", Some("#00FF00")).unwrap();
        assert!(out.contains("# cpmenu element colors") && out.contains("[tag]"));
        assert_eq!(Style::resolve(&t, &out).tag_file.rgb, 0x00FF00);
        assert_eq!(set_in("[row\nname = ", "row.name", Some("#000000")), None);
        assert_eq!(set_in("row = 5\n", "row.name", Some("#000000")), None, "a non-table in the way is left alone");
    }

    #[test]
    fn every_key_resolves_through_get_and_the_setting_tag_has_a_key() {
        let s = Style::from_theme(&theme("cyberpunk"));
        for key in KEYS {
            assert!(s.get(key).is_some(), "{key}");
        }
        assert_eq!(s.get("nope.x"), None);
        assert_eq!(s.get("tag.setting").unwrap().rgb, 0xFF4FD8);
    }

    #[test]
    fn setting_tags_follow_the_setting_role_not_the_accent() {
        for name in THEMES {
            let t = theme(name);
            let s = Style::from_theme(&t);
            assert_eq!(s.tag_setting.rgb, t.setting);
            assert_ne!(s.tag_setting.rgb, s.tag_app.rgb, "{name}: SET and APP tags look alike");
        }
    }

    #[test]
    fn no_shape_table_is_exactly_todays_look() {
        let t = theme("cyberpunk");
        assert_eq!(Style::from_theme(&t).shape, Shape::default());
        assert_eq!(Style::resolve(&t, "").shape, Shape::default());
        let d = Shape::default();
        assert_eq!((d.row_radius, d.panel_radius, d.tag_radius, d.icon_radius), (0.0, 0.0, 0.0, 0.0));
        assert_eq!((d.corner, d.border_width, d.row_height, d.row_gap), (Corner::Round, 1.0, 54.0, 10.0));
    }

    #[test]
    fn shape_values_parse_clamp_and_fall_back_individually() {
        let t = theme("cyberpunk");
        let text = "[shape]\nrow_radius = 8\npanel_radius = 99\ntag_radius = -3\nicon_radius = 4.5\ncorner = \"Chamfer\"\n\
                    border_width = 9\nrow_height = 20\nrow_gap = 100\n";
        let s = Style::resolve(&t, text).shape;
        assert_eq!((s.row_radius, s.panel_radius, s.tag_radius, s.icon_radius), (8.0, 24.0, 0.0, 4.5));
        assert_eq!((s.corner, s.border_width, s.row_height, s.row_gap), (Corner::Chamfer, 4.0, 36.0, 30.0));
        let s = Style::resolve(&t, "[shape]\nrow_height = 90\nrow_radius = \"big\"\ncorner = \"blob\"\nborder_width = 0\nrow_gap = 0\n").shape;
        assert_eq!((s.row_height, s.row_radius, s.corner, s.border_width, s.row_gap), (90.0, 0.0, Corner::Round, 0.0, 0.0));
        assert_eq!(Style::resolve(&t, "shape = 5\n").shape, Shape::default(), "a non-table is ignored");
        assert_eq!(Style::resolve(&t, "[shape\nrow_radius = ").shape, Shape::default());
    }

    #[test]
    fn shape_layers_stack_like_colors() {
        let t = theme("cyberpunk");
        let theme_file = "[shape]\nrow_radius = 6\ncorner = \"chamfer\"\n";
        let style_toml = "[shape]\nrow_radius = 2\n";
        let s = Style::resolve_layers(&t, &[theme_file, style_toml]).shape;
        assert_eq!((s.row_radius, s.corner), (2.0, Corner::Chamfer));
    }

    #[test]
    fn the_template_lists_the_shape_keys_and_enabling_them_changes_nothing() {
        let template = Style::from_theme(&theme("arasaka")).template();
        assert!(template.contains("# [shape]"));
        for key in SHAPE_KEYS {
            assert!(template.contains(&format!("# {key} = ")), "{key}");
        }
        assert!(template.contains("# corner = \"round\"") && template.contains("# border_width = 1.0") && template.contains("# row_height = 54"));
        let custom = Style { shape: Shape { row_radius: 5.0, corner: Corner::Chamfer, row_gap: 12.0, ..Shape::default() }, ..Style::from_theme(&theme("arasaka")) };
        let enabled: String = custom.template().lines().map(|l| if l.starts_with("# [") || (l.starts_with("# ") && l.contains(" = ")) { &l[2..] } else { l }).map(|l| format!("{l}\n")).collect();
        assert_eq!(Style::resolve(&theme("arasaka"), &enabled).shape, custom.shape);
    }

    #[test]
    fn writing_a_shape_value_keeps_the_file_and_defaults_are_not_stored() {
        let defaults = Shape::default();
        let text = "# mine\n[shape]\nrow_radius = 4 # keep\n";
        let out = set_value_in(text, "shape.row_gap", Some(14_i64.into())).unwrap();
        assert!(out.contains("# mine") && out.contains("# keep"));
        let s = Style::resolve(&theme("cyberpunk"), &out).shape;
        assert_eq!((s.row_radius, s.row_gap), (4.0, 14.0));
        let out = set_value_in(&out, "shape.corner", Some("chamfer".into())).unwrap();
        assert_eq!(Style::resolve(&theme("cyberpunk"), &out).shape.corner, Corner::Chamfer);
        let out = set_value_in(&out, "shape.row_gap", None).unwrap();
        assert_eq!(Style::resolve(&theme("cyberpunk"), &out).shape.row_gap, 10.0);
        assert!(explicit_shape(&defaults, "row_gap", 10_i64.into()).is_none(), "the default isn't stored");
        assert!(explicit_shape(&defaults, "row_gap", 12_i64.into()).is_some());
        assert!(explicit_shape(&defaults, "corner", "round".into()).is_none());
        assert!(explicit_shape(&defaults, "corner", "chamfer".into()).is_some());
        let themed = Shape { row_radius: 6.0, ..defaults };
        assert!(explicit_shape(&themed, "row_radius", 0_i64.into()).is_some(), "0 differs from the theme's 6");
        assert!(explicit_shape(&themed, "row_radius", 6_i64.into()).is_none());
        assert_eq!(set_value_in("[shape\n", "shape.row_gap", Some(1_i64.into())), None);
    }

    #[test]
    fn shape_steps_stay_in_range() {
        let d = Shape::default();
        let step = |s: &Shape, key: &str, dir| s.stepped(key, dir).map(|v| v.to_string().trim().to_owned());
        assert_eq!(step(&d, "row_radius", 1).as_deref(), Some("1"));
        assert_eq!(step(&d, "row_radius", -1), None);
        assert_eq!(step(&Shape { tag_radius: 24.0, ..d }, "tag_radius", 1), None);
        assert_eq!(step(&d, "border_width", 1).as_deref(), Some("1.5"));
        assert_eq!(step(&Shape { border_width: 0.0, ..d }, "border_width", -1), None);
        assert_eq!(step(&d, "row_height", 1).as_deref(), Some("56"));
        assert_eq!(step(&Shape { row_height: 90.0, ..d }, "row_height", 1), None);
        assert_eq!(step(&d, "nope", 1), None);
    }

    #[test]
    fn later_layers_win_and_broken_layers_are_skipped() {
        let t = theme("cyberpunk");
        let file = "[row]\nname = \"#111111\"\nborder = \"#222222\"\n";
        let overrides = "[row]\nname = \"#333333\"\n";
        let s = Style::resolve_layers(&t, &[file, overrides]);
        assert_eq!((s.row_name.rgb, s.row_border.rgb), (0x333333, 0x222222));
        let s = Style::resolve_layers(&t, &[file, "[row\nname = "]);
        assert_eq!((s.row_name.rgb, s.row_border.rgb), (0x111111, 0x222222), "a broken top layer changes nothing");
        assert_eq!(Style::resolve_layers(&t, &[]), Style::from_theme(&t));
    }

    #[test]
    fn changed_from_lists_only_differing_elements() {
        let t = theme("arasaka");
        let base = Style::from_theme(&t);
        assert!(base.changed_from(&base).is_empty());
        let s = Style::resolve(&t, "[row]\nname = \"#010203\"\n[tag]\nfile = \"#FFFFFF80\"\nfolder = \"#FF9F43\"\n");
        let changed = s.changed_from(&base);
        let keys: Vec<_> = changed.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec!["row.name", "tag.file"], "folder equals the default");
        assert_eq!(changed[1].1.hex(), "#FFFFFF80");
    }

    #[test]
    fn clearing_removes_color_keys_but_keeps_comments_and_other_keys() {
        let t = theme("cyberpunk");
        let mut text = Style::from_theme(&t).template();
        text = set_in(&text, "row.name", Some("#112233")).unwrap();
        text = set_in(&text, "popup.text", Some("#445566")).unwrap();
        text = text.replacen("[row]\n", "[row]\nmine = 1\n", 1);
        assert!(text.contains("mine = 1"), "{text}");
        let cleared = clear_in(&text).unwrap();
        assert!(cleared.contains("# cpmenu element colors") && cleared.contains("mine = 1"), "{cleared}");
        assert!(!cleared.contains("112233") && !cleared.contains("445566"), "{cleared}");
        assert_eq!(Style::resolve(&t, &cleared), Style::from_theme(&t));
        assert_eq!(clear_in("").as_deref(), Some(""));
        assert!(clear_in("[row\nname = ").is_none(), "a file that doesn't parse is never rewritten");
    }

    #[test]
    fn hex_round_trips() {
        for text in ["#5EF6FF", "#000000", "#FFFFFF", "#12345680"] {
            assert_eq!(Paint::parse(text).unwrap().hex(), text);
        }
    }

    #[test]
    fn opacity_steps_by_five_percent_clamped_and_keeps_the_color() {
        let at = |alpha| Paint { rgb: 0x123456, alpha };
        assert_eq!(at(1.0).stepped_alpha(1), None);
        assert_eq!(at(1.0).stepped_alpha(-1), Some(at(0.95)));
        assert_eq!(at(0.0).stepped_alpha(-1), None);
        assert_eq!(at(0.0).stepped_alpha(1), Some(at(0.05)));
        assert_eq!(at(0.37).stepped_alpha(1).unwrap().percent(), 40);
        assert_eq!(at(0.37).stepped_alpha(-1).unwrap().percent(), 35);
        assert_eq!(at(0.5).stepped_alpha(1).unwrap().rgb, 0x123456);
        let mut paint = at(1.0);
        for _ in 0..30 {
            paint = paint.stepped_alpha(-1).unwrap_or(paint);
        }
        assert_eq!(paint.percent(), 0);
    }

    #[test]
    fn opacity_is_written_as_rrggbbaa_below_100_percent() {
        assert_eq!(Paint { rgb: 0xFF5E57, alpha: 1.0 }.hex(), "#FF5E57");
        assert_eq!(Paint { rgb: 0xFF5E57, alpha: 0.5 }.hex(), "#FF5E5780");
        for percent in (0..=100).step_by(5) {
            let paint = Paint { rgb: 1, alpha: percent as f32 / 100.0 };
            assert_eq!(Paint::parse(&paint.hex()).unwrap().percent(), percent);
        }
    }

    #[test]
    fn removing_a_key_resets_color_and_opacity() {
        let t = theme("cyberpunk");
        let text = set_in(&Style::from_theme(&t).template(), "row.border", Some("#11223380")).unwrap();
        assert_eq!(Style::resolve(&t, &text).row_border.percent(), 50);
        let reset = set_in(&text, "row.border", None).unwrap();
        assert_eq!(Style::resolve(&t, &reset).row_border, Style::from_theme(&t).row_border);
    }

    #[test]
    fn no_fonts_table_is_exactly_todays_fonts() {
        let t = theme("cyberpunk");
        assert_eq!(Style::from_theme(&t).fonts, Fonts::default());
        assert_eq!(Style::resolve(&t, "").fonts, Fonts::default());
        let f = Fonts::default();
        assert_eq!(f.family.as_str(), "Bahnschrift");
        for role in ROLES {
            assert_eq!((f.family_of(role).as_str(), f.role(role).size, f.weight_of(role)), ("Bahnschrift", 100, role.builtin_weight()), "{role:?}");
        }
        assert_eq!(ROLES.len(), 12);
        assert_eq!(ROLES.iter().map(|r| r.key()).collect::<std::collections::HashSet<_>>().len(), 12, "keys are distinct");
    }

    #[test]
    fn fonts_parse_clamp_and_fall_back_individually() {
        let t = theme("cyberpunk");
        let text = "[fonts]\nfamily = \"Segoe UI\"\n[fonts.name]\nfamily = \"Consolas\"\nsize = 500\nweight = \"Light\"\n\
                    [fonts.tag]\nsize = 10\nweight = \"heavy\"\n[fonts.hint]\nsize = 62.6\nfamily = 5\n[fonts.dialog_title]\nsize = \"big\"\n";
        let f = Style::resolve(&t, text).fonts;
        assert_eq!(f.family.as_str(), "Segoe UI");
        assert_eq!((f.family_of(Role::Name).as_str(), f.role(Role::Name).size, f.weight_of(Role::Name)), ("Consolas", 200, Weight::Light));
        assert_eq!((f.role(Role::Tag).size, f.weight_of(Role::Tag)), (50, Weight::Semibold), "clamped; an unknown weight is ignored");
        assert_eq!((f.role(Role::Hint).size, f.family_of(Role::Hint).as_str()), (63, "Segoe UI"), "a number rounds; a wrong type is ignored");
        assert_eq!(f.role(Role::DialogTitle).size, 100);
        assert_eq!(f.family_of(Role::Header).as_str(), "Segoe UI", "roles without their own follow the global family");
        // Dotted keys are the same thing.
        let dotted = Style::resolve(&t, "[fonts]\nvalue.size = 80\nvalue.weight = \"bold\"\n").fonts;
        assert_eq!((dotted.role(Role::Value).size, dotted.weight_of(Role::Value)), (80, Weight::Bold));
        assert_eq!(Style::resolve(&t, "fonts = 5\n").fonts, Fonts::default(), "a non-table is ignored");
        assert_eq!(Style::resolve(&t, "[fonts\nfamily = ").fonts, Fonts::default());
        assert_eq!(Style::resolve(&t, "[fonts]\nfamily = \"\"\n").fonts.family.as_str(), "Bahnschrift", "an empty name is ignored");
    }

    #[test]
    fn font_layers_stack_like_colors() {
        let t = theme("cyberpunk");
        let theme_file = "[fonts]\nfamily = \"Consolas\"\n[fonts.name]\nsize = 120\nweight = \"bold\"\n";
        let style_toml = "[fonts.name]\nsize = 90\n[fonts]\nfamily = \"Segoe UI\"\n";
        let f = Style::resolve_layers(&t, &[theme_file, style_toml]).fonts;
        assert_eq!(f.family.as_str(), "Segoe UI");
        assert_eq!((f.role(Role::Name).size, f.weight_of(Role::Name)), (90, Weight::Bold));
        assert_eq!(Style::resolve_layers(&t, &[theme_file, ""]).fonts.family.as_str(), "Consolas");
    }

    #[test]
    fn long_or_odd_family_names_are_kept_whole_or_cut_cleanly() {
        assert_eq!(Family::new("  Segoe UI  ").unwrap().as_str(), "Segoe UI");
        assert!(Family::new("   ").is_none() && Family::new("a\nb").is_none());
        let long = "Ż".repeat(60);
        let cut = Family::new(&long).unwrap();
        assert!(cut.as_str().len() <= 63 && long.starts_with(cut.as_str()), "cut on a character boundary");
    }

    #[test]
    fn the_template_lists_the_fonts_and_enabling_them_changes_nothing() {
        let template = Style::from_theme(&theme("arasaka")).template();
        assert!(template.contains("# [fonts]") && template.contains("# family = \"Bahnschrift\""));
        for role in ROLES {
            assert!(template.contains(role.key()), "{role:?}");
        }
        let enabled: String = template.lines().map(|l| if l.starts_with("# [") || (l.starts_with("# ") && l.contains(" = ")) { &l[2..] } else { l }).map(|l| format!("{l}\n")).collect();
        assert_eq!(Style::resolve(&theme("arasaka"), &enabled).fonts, Fonts::default());
    }

    #[test]
    fn writing_a_font_value_keeps_the_file_and_theme_values_are_not_stored() {
        let t = theme("cyberpunk");
        let text = "# mine\n[fonts]\nfamily = \"Consolas\" # keep\n";
        let out = set_font_in(text, "name.size", Some(120_i64.into())).unwrap();
        assert!(out.contains("# mine") && out.contains("# keep"));
        let f = Style::resolve(&t, &out).fonts;
        assert_eq!((f.family.as_str(), f.role(Role::Name).size), ("Consolas", 120));
        let out = set_font_in(&out, "name.weight", Some("bold".into())).unwrap();
        let out = set_font_in(&out, "family", Some("Segoe UI".into())).unwrap();
        let f = Style::resolve(&t, &out).fonts;
        assert_eq!((f.family.as_str(), f.weight_of(Role::Name)), ("Segoe UI", Weight::Bold));
        // Removing the last key of a role drops its table; the others stay.
        let out = set_font_in(&out, "name.size", None).unwrap();
        assert!(out.contains("[fonts.name]"));
        let out = set_font_in(&out, "name.weight", None).unwrap();
        assert!(!out.contains("fonts.name") && out.contains("Segoe UI"));
        assert_eq!(Style::resolve(&t, &out).fonts.role(Role::Name), &RoleFont::default());
        // A file from a text that has no [fonts] at all, or removing from one without it.
        assert_eq!(set_font_in("", "tag.size", None).as_deref(), Some(""));
        assert!(set_font_in("", "tag.size", Some(80_i64.into())).unwrap().contains("[fonts.tag]"));
        assert_eq!(set_font_in("[fonts\n", "family", Some("X".into())), None, "invalid TOML is left alone");
        assert_eq!(set_font_in("fonts = 5\n", "family", Some("X".into())), None, "so is a non-table");
        // The theme layer's value (or the default) is not stored.
        let defaults = Fonts::default();
        assert!(explicit_font(&defaults, "family", "Bahnschrift".into()).is_none());
        assert!(explicit_font(&defaults, "family", "Segoe UI".into()).is_some());
        assert!(explicit_font(&defaults, "name.size", 100_i64.into()).is_none());
        assert!(explicit_font(&defaults, "name.size", 101_i64.into()).is_some());
        assert!(explicit_font(&defaults, "name.weight", "bold".into()).is_some());
        let themed = Style::resolve(&t, "[fonts.name]\nweight = \"bold\"\nsize = 120\n").fonts;
        assert!(explicit_font(&themed, "name.weight", "bold".into()).is_none());
        assert!(explicit_font(&themed, "name.size", 100_i64.into()).is_some(), "100 differs from the theme's 120");
        assert!(Fonts::valid_key("family") && Fonts::valid_key("dialog_text.weight"));
        assert!(!Fonts::valid_key("nope.size") && !Fonts::valid_key("name.colour") && !Fonts::valid_key("name"));
    }

    #[test]
    fn no_show_table_is_exactly_todays_look() {
        let t = theme("cyberpunk");
        let show = Style::resolve(&t, "").show;
        assert_eq!(show, Show::default());
        assert!(show.dim_unselected && show.edge_fade && !show.color_icons && show.power_bar);
        assert_eq!((show.caption_align, show.title_align, show.count_align), (TextAlign::Left, TextAlign::Left, TextAlign::Right));
    }

    #[test]
    fn show_values_parse_and_fall_back_individually_and_layers_stack() {
        let t = theme("cyberpunk");
        let doc = "[show]\nglow = false\ntags = 3\ntitle_align = \"CENTER\"\ncount_align = \"middle\"\ncolor_icons = true\nunknown = false\n";
        let show = Style::resolve(&t, doc).show;
        assert!(!show.glow && show.tags && show.color_icons, "a number is not a flag");
        assert_eq!((show.title_align, show.count_align), (TextAlign::Center, TextAlign::Right), "case is ignored, nonsense falls back");
        let layered = Style::resolve_layers(&t, &["[show]\nglow = false\nhint = false\n", "[show]\nglow = true\n"]).show;
        assert!(layered.glow && !layered.hint, "style.toml wins over the theme, the rest stays");
        assert_eq!(Style::resolve(&t, "[show\n").show, Show::default());
    }

    #[test]
    fn the_template_lists_the_show_keys_and_enabling_them_changes_nothing() {
        let t = theme("cyberpunk");
        let template = Style::from_theme(&t).template();
        for key in SHOW_KEYS {
            assert!(template.contains(&format!("# {key} = ")) || template.contains(&format!("{key} = true")), "{key}");
        }
        let enabled: String = template.lines().map(|l| l.strip_prefix("# ").unwrap_or(l)).collect::<Vec<_>>().join("\n");
        assert_eq!(Style::resolve(&t, &enabled).show, Show::default());
    }

    #[test]
    fn show_steps_toggle_flags_and_cycle_alignments() {
        let d = Show::default();
        assert_eq!(d.stepped("glow", 1).unwrap().as_bool(), Some(false));
        assert_eq!(d.stepped("color_icons", -1).unwrap().as_bool(), Some(true));
        assert_eq!(d.stepped("count_align", 1).unwrap().as_str(), Some("left"), "right wraps to left");
        assert_eq!(d.stepped("count_align", -1).unwrap().as_str(), Some("center"));
        assert_eq!(d.stepped("title_align", 1).unwrap().as_str(), Some("center"));
        assert!(d.stepped("nope", 1).is_none());
    }

    #[test]
    fn show_changes_write_in_one_go_and_values_equal_to_the_theme_are_not_stored() {
        let t = theme("cyberpunk");
        let defaults = Style::from_theme(&t);
        let off = [("glow", Some(false.into())), ("tags", Some(false.into())), ("power_bar", Some(true.into()))];
        let text = set_show_in("# keep me
", &defaults.show, &off).unwrap();
        assert!(text.contains("# keep me"));
        let show = Style::resolve(&t, &text).show;
        assert!(!show.glow && !show.tags && show.power_bar);
        assert!(!text.contains("power_bar"), "a value equal to the theme's is not stored");
        // `None` removes a key and leaves the others.
        let with_align = set_show_in(&text, &defaults.show, &[("title_align", Some("center".into()))]).unwrap();
        let back = set_show_in(&with_align, &defaults.show, &[("glow", None), ("tags", None)]).unwrap();
        assert!(!back.contains("glow") && !back.contains("tags"));
        assert_eq!(Style::resolve(&t, &back).show, Show { title_align: TextAlign::Center, ..Show::default() });
        // A broken file is left alone.
        assert_eq!(set_show_in("[show
", &defaults.show, &off), None);
        assert_eq!(set_show_in("show = 1
", &defaults.show, &off), None);
        assert_eq!(set_show_in("", &defaults.show, &[("nope", None)]), None);
    }

    #[test]
    fn tag_spacing_keys_clamp_and_step() {
        let t = theme("cyberpunk");
        let s = Style::resolve(&t, "[shape]\ntag_padding = 40\ntag_gap = -3\ntag_height = 10\n").shape;
        assert_eq!((s.tag_padding, s.tag_gap, s.tag_height), (16.0, 0.0, 60.0));
        let d = Shape::default();
        assert_eq!((d.tag_padding, d.tag_gap, d.tag_height), (5.0, 5.0, 100.0));
        assert_eq!(d.stepped("tag_height", 1).unwrap().as_integer(), Some(105));
        assert!(Shape { tag_padding: 2.0, ..d }.stepped("tag_padding", -1).is_none());
        assert!(Shape { tag_gap: 16.0, ..d }.stepped("tag_gap", 1).is_none());
        let defaults = Style::from_theme(&t);
        let out = set_value_in("", "shape.tag_gap", explicit_shape(&defaults.shape, "tag_gap", 5_i64.into())).unwrap();
        assert!(!out.contains("tag_gap"), "the default is not stored");
    }
}
