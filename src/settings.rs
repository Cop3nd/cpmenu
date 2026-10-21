//! Model of the in-app settings panel: tabs, rows, and how a row's value steps. Pure data, no
//! drawing; each change is returned as the `(table, key, value)` to write to config.toml.

use toml_edit::Value;

use crate::autostart;
use crate::commands;
use crate::config::{ActionsAnchor, ActionsMode, Config, GUI_SCALE_MAX, GUI_SCALE_MIN, GUI_SCALE_STEP, ShortList, Side, THEMES};
#[cfg(test)]
use crate::hotkey::Hotkey;
use crate::plugins::{self, SettingKind, SettingValue};
use crate::fontmenu::{DropGeom, Dropdown, DropdownView, Key, Outcome, weight_label};
use crate::style::{self, Corner, Fonts, ROLES, Role, SHOW_ALIGNS, SHOW_DIMMING, SHOW_ELEMENTS, SHOW_HEADER, Shape, Show, Style};
use crate::togglemenu::{self, Toggles, TogglesView};
use crate::{fonts, lang, themes};

/// Rows the panel card has room for; longer tabs scroll.
pub const SLOTS: usize = 7;

/// Longest header title / subtitle that can be typed.
const MAX_TEXT: usize = 40;
/// Longest value that can be typed into a plugin's string setting.
const MAX_SETTING: usize = 120;
/// Characters of a plugin's name, author or setting label the panel shows.
const MAX_LABEL: usize = 17;
const MAX_HEADING: usize = 38;
/// Characters of an error text the note line shows.
const MAX_DETAIL: usize = 60;
/// Characters of typed text the value column shows; the tail, so the cursor stays visible.
const SHOWN: usize = 13;
/// Characters of a font family name a row shows.
const SHOWN_FAMILY: usize = 16;

/// The style.toml sections on the COLORS tab, in order; `settings.section.<table>` names them.
const SECTIONS: [&str; 7] = ["header", "row", "tag", "power", "panel", "popup", "overlay"];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Tab {
    #[default]
    Colors,
    /// The shape rows, the GUI scale and the fonts.
    Look,
    Layout,
    Behavior,
    Plugins,
}

pub const TABS: [Tab; 5] = [Tab::Colors, Tab::Look, Tab::Layout, Tab::Behavior, Tab::Plugins];

impl Tab {
    pub fn label(self) -> String {
        lang::t(match self {
            Tab::Colors => "settings.tab.colors",
            Tab::Look => "settings.tab.look",
            Tab::Layout => "settings.tab.layout",
            Tab::Behavior => "settings.tab.behavior",
            Tab::Plugins => "settings.tab.plugins",
        })
    }

    pub fn index(self) -> usize {
        TABS.iter().position(|&t| t == self).unwrap_or(0)
    }

    /// Everything the tab lists, in order: rows and, on COLORS, the section headings between them.
    /// On PLUGINS only the two rows that don't depend on which plugins are installed; see `Panel::lines`.
    pub fn lines(self) -> Vec<Line> {
        match self {
            Tab::Colors => {
                let mut lines = vec![Line::Row(Row::Theme), Line::Row(Row::SaveTheme)];
                for table in SECTIONS {
                    lines.push(Line::Heading(Head::Section(table)));
                    let keys = style::KEYS.iter().enumerate();
                    lines.extend(keys.filter(|(_, key)| key.split_once('.').is_some_and(|(t, _)| t == table)).map(|(i, _)| Line::Row(Row::Color(i))));
                }
                lines
            }
            Tab::Look => {
                let mut lines = vec![Line::Row(Row::GuiScale)];
                lines.extend((0..style::SHAPE_KEYS.len()).map(|i| Line::Row(Row::Shape(i))));
                lines.push(Line::Heading(Head::Section("header")));
                lines.extend([Row::Title, Row::Subtitle].map(Line::Row));
                lines.extend(SHOW_ALIGNS.map(|i| Line::Row(Row::Show(i))));
                lines.push(Line::Row(Row::HeaderToggles));
                lines.push(Line::Heading(Head::Section("dimming")));
                lines.extend(SHOW_DIMMING.map(|i| Line::Row(Row::Show(i))));
                lines.push(Line::Heading(Head::Section("elements")));
                lines.extend(SHOW_ELEMENTS.map(|i| Line::Row(Row::Show(i))));
                lines.push(Line::Heading(Head::Section("fonts")));
                lines.push(Line::Row(Row::FontFamily));
                lines.extend(ROLES.map(|role| Line::Row(Row::FontRole(role))));
                lines
            }
            Tab::Layout => {
                [Row::Side, Row::Width, Row::Rows, Row::Arc, Row::Backdrop, Row::ShortList, Row::Band, Row::Contrast, Row::Icons, Row::Actions, Row::ActionsAnchor, Row::ActionsX, Row::ActionsY]
                    .map(Line::Row)
                    .into()
            }
            Tab::Behavior => {
                [Row::Hotkey, Row::FileSearch, Row::FileResults, Row::Recent, Row::ClearHistory, Row::Language, Row::Autostart, Row::Tutorial]
                    .map(Line::Row)
                    .into()
            }
            Tab::Plugins => [Row::PluginsMaster, Row::PluginsFolder].map(Line::Row).into(),
        }
    }

    /// The selectable rows of the lines `lines` returns.
    #[cfg(test)]
    pub fn rows(self) -> Vec<Row> {
        rows_of(&self.lines())
    }
}

fn rows_of(lines: &[Line]) -> Vec<Row> {
    lines
        .iter()
        .filter_map(|line| match line {
            Line::Row(row) => Some(*row),
            Line::Heading(_) => None,
        })
        .collect()
}

/// A heading between rows: a COLORS section, or an installed plugin (by its place in `Panel::plugins`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Head {
    Section(&'static str),
    Plugin(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Line {
    Heading(Head),
    Row(Row),
}

/// What a plugin's section of the PLUGINS tab can hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginField {
    State,
    Enabled,
    /// Only while the plugin isn't approved.
    Approve,
    Keyword,
    /// Only when the manifest allows it.
    Global,
    /// By place in the manifest's `settings`.
    Setting(u8),
    Log,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum PluginState {
    #[default]
    Stopped,
    Starting,
    Running,
    /// Crashed too often; off for this session.
    Failed,
    Disabled,
    NotApproved,
    /// The manifest is broken (or needs a newer cpmenu); this is why.
    ManifestError(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingInfo {
    pub key: String,
    pub label: String,
    pub kind: SettingKind,
    pub value: SettingValue,
}

/// What the PLUGINS tab shows of one plugin folder; the window refreshes it from the plugin host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginInfo {
    /// The folder name for a broken manifest.
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    /// The command the plugin runs as, e.g. `python main.py`.
    pub run: String,
    pub state: PluginState,
    pub enabled: bool,
    pub approved: bool,
    /// The keywords it answers to: the user's override or the manifest's.
    pub keywords: Vec<String>,
    /// Another plugin holds its keyword, so it doesn't answer to it.
    pub keyword_lost: bool,
    pub can_global: bool,
    pub global: bool,
    pub settings: Vec<SettingInfo>,
}

fn plugin_lines(index: usize, p: &PluginInfo) -> Vec<Line> {
    let field = |f| Line::Row(Row::Plugin(index as u8, f));
    let mut lines = vec![Line::Heading(Head::Plugin(index as u8)), field(PluginField::State)];
    if matches!(p.state, PluginState::ManifestError(_)) {
        return lines;
    }
    lines.push(field(PluginField::Enabled));
    if !p.approved {
        lines.push(field(PluginField::Approve));
    }
    lines.push(field(PluginField::Keyword));
    if p.can_global {
        lines.push(field(PluginField::Global));
    }
    lines.extend((0..p.settings.len()).map(|i| field(PluginField::Setting(i as u8))));
    lines.push(field(PluginField::Log));
    lines
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    /// A style.toml color, by index into `style::KEYS`.
    Color(usize),
    /// A style.toml `[shape]` value, by index into `style::SHAPE_KEYS`.
    Shape(usize),
    /// A style.toml `[show]` value, by index into `style::SHOW_KEYS`.
    Show(usize),
    /// Opens a pop-out with the on/off toggles of the header's elements (`style::SHOW_HEADER`).
    HeaderToggles,
    Theme,
    /// Enter types a name and saves the current colors as a custom theme.
    SaveTheme,
    Side,
    Width,
    Rows,
    Arc,
    Backdrop,
    Hotkey,
    ShortList,
    Band,
    /// `[layout] gui_scale`.
    GuiScale,
    /// The `[fonts]` rows: the global family, then one per text role (its family, size and weight).
    /// Each opens a dropdown.
    FontFamily,
    FontRole(Role),
    Contrast,
    Icons,
    /// `[layout] actions` and the pop-out's placement.
    Actions,
    ActionsAnchor,
    ActionsX,
    ActionsY,
    FileSearch,
    FileResults,
    /// `[behavior] recent`: RECENT rows under the pinned ones.
    Recent,
    /// Wipes the launch history (usage.json); asks for a second press.
    ClearHistory,
    Language,
    /// Kept in the registry (HKCU Run), not config.toml; see `autostart`.
    Autostart,
    /// The header's caption (`header.title`) and the list's title (`header.subtitle`), typed in.
    Title,
    Subtitle,
    /// Starts the first-run wizard again.
    Tutorial,
    /// `[behavior] plugins`: nothing is scanned or started while it is off.
    PluginsMaster,
    /// Opens the plugins folder in Explorer, creating it first.
    PluginsFolder,
    /// A row of plugin `.0` (its place in `Panel::plugins`).
    Plugin(u8, PluginField),
}

type Change = (&'static str, &'static str, Value);

/// Position of `v` in the `ROWS` scale: 0 = AUTO, then 3..=30.
fn rows_index(v: u32) -> i32 {
    if v == 0 { 0 } else { v as i32 - 2 }
}

/// What a SHAPE row shows for `shape`'s value of `key`.
fn shape_value(shape: &Shape, key: &str) -> String {
    match (key, shape.number(key)) {
        (_, None) => lang::t(&format!("settings.value.{}", if shape.corner == Corner::Round { "round" } else { "chamfer" })),
        ("border_width", Some(n)) => format!("{n:.1} PX"),
        ("tag_height", Some(n)) => format!("{n:.0} %"),
        (_, Some(n)) => format!("{n:.0} PX"),
    }
}

/// What a `[show]` row shows for `show`'s value of `key`: ON / OFF, or the alignment.
fn show_value(show: &Show, key: &str) -> String {
    let word = match (show.flag(key), show.align(key)) {
        (Some(on), _) => if on { "on" } else { "off" },
        (_, Some(align)) => align.key(),
        _ => return String::new(),
    };
    lang::t(&format!("settings.value.{word}"))
}

/// `text` cut to its last `SHOWN` characters, after an ellipsis.
fn tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= SHOWN {
        return text.to_owned();
    }
    format!("\u{2026}{}", text.chars().skip(count - SHOWN + 1).collect::<String>())
}

/// What a row being typed into shows: the text and a cursor, or the hint while it is empty.
fn typed(text: &str, hint: &str) -> String {
    if text.is_empty() { format!("{}_", lang::t(hint)) } else { format!("{}_", tail(text)) }
}

impl Row {
    pub fn label(self) -> String {
        let id = match self {
            Row::Color(i) => {
                let key = style::KEYS.get(i).copied().unwrap_or_default();
                let name = key.split_once('.').map_or(key, |(_, name)| name);
                return lang::find(&format!("settings.color.{key}")).unwrap_or_else(|| name.replace('_', " ").to_uppercase());
            }
            Row::Shape(i) => {
                let key = style::SHAPE_KEYS.get(i).copied().unwrap_or_default();
                return lang::find(&format!("settings.shape.{key}")).unwrap_or_else(|| key.replace('_', " ").to_uppercase());
            }
            Row::Show(i) => {
                let key = style::SHOW_KEYS.get(i).copied().unwrap_or_default();
                return lang::find(&format!("settings.show.{key}")).unwrap_or_else(|| key.replace('_', " ").to_uppercase());
            }
            Row::HeaderToggles => "settings.row.header_toggles",
            Row::Theme => "settings.row.theme",
            Row::SaveTheme => "settings.row.save_theme",
            Row::Side => "settings.row.side",
            Row::Width => "settings.row.width",
            Row::Rows => "settings.row.rows",
            Row::Arc => "settings.row.arc",
            Row::Backdrop => "settings.row.backdrop",
            Row::Hotkey => "settings.row.hotkey",
            Row::ShortList => "settings.row.short_list",
            Row::Band => "settings.row.band",
            Row::GuiScale => "settings.row.gui_scale",
            Row::FontFamily => "settings.row.font",
            Row::FontRole(role) => return lang::t(&format!("settings.fontrole.{}", role.key())),
            Row::Contrast => "settings.row.contrast",
            Row::Icons => "settings.row.icons",
            Row::Actions => "settings.row.actions",
            Row::ActionsAnchor => "settings.row.actions_anchor",
            Row::ActionsX => "settings.row.actions_x",
            Row::ActionsY => "settings.row.actions_y",
            Row::FileSearch => "settings.row.file_search",
            Row::FileResults => "settings.row.file_results",
            Row::Recent => "settings.row.recent",
            Row::ClearHistory => "settings.row.clear_history",
            Row::Language => "settings.row.language",
            Row::Autostart => "settings.row.autostart",
            Row::Title => "settings.row.title",
            Row::Subtitle => "settings.row.subtitle",
            Row::Tutorial => "settings.row.tutorial",
            Row::PluginsMaster => "settings.row.plugins",
            Row::PluginsFolder => "settings.row.plugins_folder",
            Row::Plugin(_, field) => match field {
                PluginField::State => "settings.row.plugin_state",
                PluginField::Enabled => "settings.row.plugin_enabled",
                PluginField::Approve => "settings.row.plugin_approve",
                PluginField::Keyword => "settings.row.plugin_keyword",
                PluginField::Global => "settings.row.plugin_global",
                PluginField::Log => "settings.row.plugin_log",
                // The manifest's own label; see `Panel::plugin_label`.
                PluginField::Setting(_) => return String::new(),
            },
        };
        lang::t(id)
    }

    /// Rows that take typed text on Enter (or a double click). A plugin's string settings do too,
    /// which only `Panel::edits` knows.
    pub fn edits(self) -> bool {
        matches!(self, Row::Color(_) | Row::SaveTheme | Row::Title | Row::Subtitle | Row::Plugin(_, PluginField::Keyword))
    }

    /// Rows that open a dropdown (the fonts) or a toggle pop-out on a click and on Enter.
    pub fn opens_dropdown(self) -> bool {
        matches!(self, Row::FontFamily | Row::FontRole(_) | Row::HeaderToggles)
    }

    /// Rows that run on a click and on Enter instead of being stepped.
    pub fn presses(self) -> bool {
        matches!(self, Row::PluginsFolder | Row::ClearHistory | Row::Tutorial | Row::Plugin(_, PluginField::State | PluginField::Approve | PluginField::Log))
    }

    /// Rows without arrows: their value is typed, pressed or captured, not stepped.
    fn stepped(self) -> bool {
        !matches!(self, Row::Hotkey | Row::SaveTheme | Row::Title | Row::Subtitle | Row::PluginsFolder | Row::ClearHistory | Row::Tutorial)
            && !self.opens_dropdown()
            && !matches!(self, Row::Plugin(_, PluginField::State | PluginField::Approve | PluginField::Keyword | PluginField::Log))
    }

    /// The config value shown for the row; colors come from the style, see `Panel::view`.
    pub fn value(self, c: &Config) -> String {
        let word = |id: &str| lang::t(&format!("settings.value.{id}"));
        let on_off = |b: bool| word(if b { "on" } else { "off" });
        match self {
            // From the style, see `Panel::view`.
            Row::Color(_) | Row::Shape(_) | Row::Show(_) | Row::HeaderToggles | Row::FontFamily | Row::FontRole(_) => String::new(),
            Row::Theme => c.theme_name.to_uppercase(),
            Row::SaveTheme => word("enter"),
            Row::Side => word(if c.layout.side == Side::Left { "left" } else { "right" }),
            Row::Width => format!("{}%", c.layout.width_percent),
            Row::Rows => match c.layout.visible_rows {
                0 => word("auto"),
                n => n.to_string(),
            },
            Row::Arc => match c.layout.arc_shift {
                0 => "0".to_owned(),
                n => format!("{n:+}"),
            },
            Row::Backdrop => format!("{:.1}", c.layout.backdrop_opacity),
            Row::Hotkey => c.hotkey.display(),
            Row::ShortList => word(if c.layout.short_list == ShortList::Top { "top" } else { "center" }),
            Row::Band => match c.layout.band_percent {
                0 => word("top"),
                50 => word("mid"),
                100 => word("bottom"),
                n => format!("{n}%"),
            },
            Row::GuiScale => format!("{}%", c.layout.gui_scale),
            Row::Contrast => on_off(c.layout.high_contrast),
            Row::Icons => on_off(c.layout.icons),
            Row::Actions => word(if c.layout.actions == ActionsMode::List { "list" } else { "popout" }),
            Row::ActionsAnchor => word(match c.layout.actions_anchor {
                ActionsAnchor::Row => "row",
                ActionsAnchor::List => "list",
                ActionsAnchor::Cursor => "cursor",
            }),
            Row::ActionsX | Row::ActionsY => match if self == Row::ActionsX { c.layout.actions_x } else { c.layout.actions_y } {
                0 => "0".to_owned(),
                n => format!("{n:+}%"),
            },
            Row::FileSearch => on_off(c.file_search),
            Row::FileResults => c.file_results.to_string(),
            Row::Recent => on_off(c.recent),
            Row::ClearHistory | Row::Tutorial => word("enter"),
            Row::Language if c.language == lang::AUTO => word("auto"),
            Row::Language => c.language.to_uppercase(),
            // Read from the registry, see `Panel::view`.
            Row::Autostart => String::new(),
            Row::Title => lang::t("header.title"),
            Row::Subtitle => lang::t("header.subtitle"),
            Row::PluginsMaster => on_off(c.plugins.enabled),
            Row::PluginsFolder => word("enter"),
            // Needs the plugin; see `Panel::plugin_value`.
            Row::Plugin(..) => String::new(),
        }
    }

    /// One step of a SHAPE row in direction `dir`: the `[shape]` key and the value to write to
    /// style.toml, or `None` at the end of its range (or for any other row).
    pub fn shape_change(self, shape: &Shape, dir: i32) -> Option<(&'static str, Value)> {
        let Row::Shape(i) = self else { return None };
        let key = *style::SHAPE_KEYS.get(i)?;
        shape.stepped(key, dir).map(|value| (key, value))
    }

    /// One step of a `[show]` row in direction `dir`: the key and the value to write to style.toml.
    pub fn show_change(self, show: &Show, dir: i32) -> Option<(&'static str, Value)> {
        let Row::Show(i) = self else { return None };
        let key = *style::SHOW_KEYS.get(i)?;
        show.stepped(key, dir).map(|value| (key, value))
    }

    /// `change_in` with only the presets and no languages to pick from.
    #[cfg(test)]
    pub fn change(self, c: &Config, dir: i32) -> Option<Change> {
        self.change_in(c, dir, &[], &[])
    }

    /// One step of the row's value in direction `dir` (-1 or 1): the config entry to write, or
    /// `None` when the value is already at that end. Theme, language and the on/off rows wrap or
    /// toggle. `custom_themes` follow the presets in the THEME cycle, `languages` follow AUTO.
    pub fn change_in(self, c: &Config, dir: i32, custom_themes: &[String], languages: &[String]) -> Option<Change> {
        let dir = dir.signum();
        let moved = |from: i32, step: i32, lo: i32, hi: i32| {
            let to = (from + dir * step).clamp(lo, hi);
            (to != from).then_some(to as i64)
        };
        let cycle = |names: Vec<&str>, current: &str| {
            let at = names.iter().position(|n| n.eq_ignore_ascii_case(current)).unwrap_or(0);
            let next = (at as i32 + dir).rem_euclid(names.len() as i32) as usize;
            (next != at).then(|| Value::from(names[next]))
        };
        match self {
            // Written to style.toml, not config.toml; the fonts through their dropdown.
            Row::Color(_) | Row::Shape(_) | Row::Show(_) | Row::HeaderToggles | Row::FontFamily | Row::FontRole(_) => None,
            Row::Theme => {
                let names = THEMES.iter().copied().chain(custom_themes.iter().map(String::as_str)).collect();
                cycle(names, &c.theme_name).map(|v| ("colors", "theme", v))
            }
            Row::Language => {
                let names = std::iter::once(lang::AUTO).chain(languages.iter().map(String::as_str)).collect();
                cycle(names, &c.language).map(|v| ("behavior", "language", v))
            }
            // Typed in.
            Row::SaveTheme | Row::Title | Row::Subtitle => None,
            // Written to the registry, not config.toml; see `window::toggle_autostart`.
            Row::Autostart => None,
            Row::PluginsMaster => Some(("behavior", "plugins", (!c.plugins.enabled).into())),
            // A button, and rows written under `[plugins.<id>]`; see `Panel::plugin_step`.
            Row::PluginsFolder | Row::ClearHistory | Row::Tutorial | Row::Plugin(..) => None,
            Row::Side => {
                let side = if c.layout.side == Side::Left { "right" } else { "left" };
                Some(("layout", "side", side.into()))
            }
            Row::Width => moved(c.layout.width_percent as i32, 5, 15, 50).map(|v| ("layout", "width_percent", v.into())),
            Row::Rows => {
                let to = moved(rows_index(c.layout.visible_rows), 1, 0, 28)?;
                Some(("layout", "visible_rows", (if to == 0 { 0 } else { to + 2 }).into()))
            }
            Row::Arc => moved(c.layout.arc_shift, 10, -120, 120).map(|v| ("layout", "arc_shift", v.into())),
            Row::Backdrop => {
                let tenths = (c.layout.backdrop_opacity * 10.0).round() as i32;
                moved(tenths, 1, 0, 10).map(|v| ("layout", "backdrop_opacity", (v as f64 / 10.0).into()))
            }
            // Chosen by pressing the keys, not by stepping.
            Row::Hotkey => None,
            Row::ShortList => {
                let to = if c.layout.short_list == ShortList::Top { "center" } else { "top" };
                Some(("layout", "short_list", to.into()))
            }
            Row::Band => moved(c.layout.band_percent as i32, 10, 0, 100).map(|v| ("layout", "band_percent", v.into())),
            Row::GuiScale => {
                // From a value off the grid (the old text_size 115) to the next grid value in that direction.
                let (now, step) = (c.layout.gui_scale as i32, GUI_SCALE_STEP as i32);
                let to = if dir > 0 { (now / step + 1) * step } else { (now + step - 1) / step * step - step };
                moved(now, (to - now).abs(), GUI_SCALE_MIN as i32, GUI_SCALE_MAX as i32).map(|v| ("layout", "gui_scale", v.into()))
            }
            Row::Contrast => Some(("layout", "high_contrast", (!c.layout.high_contrast).into())),
            Row::Icons => Some(("layout", "icons", (!c.layout.icons).into())),
            Row::Actions => {
                let to = if c.layout.actions == ActionsMode::List { "popout" } else { "list" };
                Some(("layout", "actions", to.into()))
            }
            Row::ActionsAnchor => {
                let names = ["row", "list", "cursor"];
                let at = match c.layout.actions_anchor {
                    ActionsAnchor::Row => 0,
                    ActionsAnchor::List => 1,
                    ActionsAnchor::Cursor => 2,
                };
                moved(at, 1, 0, 2).map(|to| ("layout", "actions_anchor", names[to as usize].into()))
            }
            Row::ActionsX => moved(c.layout.actions_x, 5, -100, 100).map(|v| ("layout", "actions_x", v.into())),
            Row::ActionsY => moved(c.layout.actions_y, 5, -100, 100).map(|v| ("layout", "actions_y", v.into())),
            Row::FileSearch => Some(("behavior", "file_search", (!c.file_search).into())),
            Row::Recent => Some(("behavior", "recent", (!c.recent).into())),
            Row::FileResults => {
                moved(c.file_results as i32, 2, 0, 50).map(|v| ("behavior", "file_results", v.into()))
            }
        }
    }
}

/// What the panel shows in its note line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Note {
    #[default]
    None,
    HexOverrides,
    HotkeyInUse,
    /// config.toml has syntax errors.
    Error,
    /// style.toml has syntax errors.
    StyleError,
    /// The typed color wasn't `#RRGGBB` or `#RRGGBBAA`.
    InvalidHex,
    /// Enter again replaces the custom theme of that name.
    OverwriteTheme,
    /// Delete again removes the selected custom theme.
    DeleteTheme,
    /// The typed theme name is a preset (or a Windows device name).
    BuiltinName,
    EmptyName,
    /// The theme file could not be written, or config.toml / style.toml were not up to taking the switch.
    ThemeNotSaved,
    /// The HKCU Run value could not be written; the row keeps its old state.
    AutostartFailed,
    /// The typed keyword belongs to another plugin; nothing was saved.
    KeywordInUse,
    /// The typed keyword isn't 1 to 16 characters of a-z 0-9 _ -.
    KeywordInvalid,
    /// Enter again lets the plugin run.
    ApprovePlugin,
    /// The plugin's log file could not be opened.
    LogFailed,
    /// Enter again wipes the launch history.
    ClearHistory,
    /// The launch history was wiped.
    HistoryCleared,
}

impl Note {
    /// Id of the note's text.
    pub fn id(self) -> Option<&'static str> {
        Some(match self {
            Note::None => return None,
            Note::HexOverrides => "settings.note.hex_overrides",
            Note::HotkeyInUse => "settings.note.hotkey_in_use",
            Note::Error => "settings.note.config_error",
            Note::StyleError => "settings.note.style_error",
            Note::InvalidHex => "settings.note.invalid_hex",
            Note::OverwriteTheme => "settings.note.overwrite_theme",
            Note::DeleteTheme => "settings.note.delete_theme",
            Note::BuiltinName => "settings.note.builtin_name",
            Note::EmptyName => "settings.note.empty_name",
            Note::ThemeNotSaved => "settings.note.theme_not_saved",
            Note::AutostartFailed => "settings.note.autostart_failed",
            Note::KeywordInUse => "settings.note.keyword_in_use",
            Note::KeywordInvalid => "settings.note.keyword_invalid",
            Note::ApprovePlugin => "settings.note.approve_plugin",
            Note::LogFailed => "settings.note.log_failed",
            Note::ClearHistory => "settings.note.clear_history",
            Note::HistoryCleared => "settings.note.history_cleared",
        })
    }

    /// Plain information rather than something wrong.
    pub fn is_info(self) -> bool {
        matches!(self, Note::HexOverrides | Note::ApprovePlugin | Note::ClearHistory | Note::HistoryCleared)
    }
}

/// A step that waits for the same step again.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    /// Enter again replaces this custom theme.
    Overwrite(String),
    /// Delete again removes this custom theme.
    Delete(String),
    /// Enter again lets this plugin (by id) run.
    Approve(String),
    /// Enter again wipes the launch history.
    ClearHistory,
}

/// What is being typed into the selected row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum EditKind {
    /// Hex digits of a color.
    #[default]
    Hex,
    /// A theme name.
    Name,
    /// Free text (header title / subtitle).
    Text,
    /// A plugin keyword: lower case letters, digits, `_` and `-`.
    Keyword,
    /// A plugin's string setting: free text, longer than the header's.
    Setting,
}

/// A change to config.toml that belongs to a plugin: `[plugins.<id>]` or `[plugins.<id>.settings]`.
#[derive(Debug)]
pub enum PluginWrite {
    Value { id: String, key: &'static str, value: Value },
    Remove { id: String, key: &'static str },
    Setting { id: String, key: String, value: Value },
}

/// The outcome of Enter on a typed theme name.
#[derive(Debug, PartialEq)]
pub enum NameCheck {
    /// Nothing usable was typed; typing continues.
    Rejected,
    /// The name is taken by a custom theme; Enter again overwrites it.
    Confirm,
    Save(String),
}

/// The outcome of Delete on the THEME row.
#[derive(Debug, PartialEq)]
pub enum DeleteStep {
    /// Not a custom theme.
    Nothing,
    Confirm,
    Remove(String),
}

/// A color row's swatch and state.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorCell {
    pub rgb: u32,
    pub alpha: f32,
    /// Set in style.toml; otherwise it follows the theme.
    pub custom: bool,
    /// Hex digits are being typed into `PanelRow::value`.
    pub editing: bool,
}

#[derive(Debug, PartialEq)]
pub struct PanelRow {
    pub label: String,
    pub value: String,
    /// Shows ◂ ▸ arrows; false for rows set by other means (the hotkey is captured).
    pub stepper: bool,
    pub color: Option<ColorCell>,
    /// Opens a dropdown (the font rows): the value is a longer text and a ▾ stands in for the arrows.
    pub dropdown: bool,
}

/// One visible line of the panel.
#[derive(Debug, PartialEq)]
pub enum Slot {
    Heading(String),
    Row(PanelRow),
}

/// The buttons under the rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Button {
    Config,
    Style,
}

pub const BUTTONS: usize = 2;

/// Everything the renderer needs to draw the panel.
#[derive(Debug, PartialEq)]
pub struct PanelView {
    pub tab: usize,
    /// The lines in view (at most `SLOTS`).
    pub slots: Vec<Slot>,
    /// Slot of the selected row, if it is in view.
    pub selected: Option<usize>,
    /// The selected button, when the selection is below the rows.
    pub button: Option<Button>,
    /// Start and end of the visible part as fractions of the whole list, when it scrolls.
    pub scroll: Option<(f32, f32)>,
    pub note: Note,
    /// A line about the selected row (a plugin's error, the command it runs), shown when there is no note.
    pub detail: Option<String>,
    /// Set while the first-run wizard is showing: the card draws it in place of the tabs and buttons.
    pub wizard: Option<WizardView>,
    /// The font dropdown, drawn over the card while it is open.
    pub dropdown: Option<DropdownView>,
    /// The header toggles' pop-out, drawn over the card while it is open.
    pub toggles: Option<TogglesView>,
}

/// Panel state while it is open.
#[derive(Debug, Default)]
pub struct Panel {
    pub tab: Tab,
    /// Index among the tab's rows; `rows.len()` and the next one are the OPEN CONFIG.TOML / OPEN STYLE.TOML buttons.
    pub selected: usize,
    /// Index of the first visible line.
    top: usize,
    /// The last write was refused because config.toml has syntax errors.
    pub error: bool,
    /// Likewise for style.toml.
    pub style_error: bool,
    /// The last typed color was rejected.
    pub invalid_hex: bool,
    /// Waiting for the key press that becomes the hotkey.
    pub capturing: bool,
    /// The wizard's HOTKEY step took a key: the row shows it instead of the prompt.
    captured: bool,
    /// Text being typed into the selected row (hex digits, a theme name, or a header string).
    pub editing: Option<String>,
    pub edit_kind: EditKind,
    pub pending: Option<Pending>,
    /// A one-off message for the note line (cleared by the next key).
    pub flash: Note,
    /// Custom theme names, as found when the panel opened or a theme was saved or deleted.
    pub themes: Vec<String>,
    /// Languages the LANGUAGE row cycles through.
    pub languages: Vec<String>,
    /// The AUTOSTART row's state, as the registry had it when the panel opened.
    pub autostart: bool,
    /// `[behavior] plugins`; while it is off the PLUGINS tab lists no plugin.
    pub plugins_on: bool,
    /// The installed plugins, refreshed by the window whenever something about them changes.
    pub plugins: Vec<PluginInfo>,
    /// `[layout] actions = "popout"`; only then does LAYOUT list the pop-out's placement rows.
    pub popout: bool,
    /// The first-run wizard, shown on the card instead of the tabs while it is `Some`.
    pub wizard: Option<Wizard>,
    /// The installed font families, as found when the panel opened.
    pub fonts: Vec<String>,
    /// The open font dropdown; it takes the keys and clicks until it closes.
    pub dropdown: Option<Dropdown>,
    /// The open toggle pop-out; it takes the keys and clicks until it closes.
    pub toggles: Option<Toggles>,
}

/// Steps of the first-run wizard.
pub const WIZARD_STEPS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Language,
    Hotkey,
    /// Side of the screen and how the actions open; the list stays visible to preview them.
    Layout,
    Theme,
    Autostart,
}

const STEPS: [Step; WIZARD_STEPS] = [Step::Language, Step::Hotkey, Step::Layout, Step::Theme, Step::Autostart];

impl Step {
    fn id(self) -> &'static str {
        match self {
            Step::Language => "language",
            Step::Hotkey => "hotkey",
            Step::Layout => "layout",
            Step::Theme => "theme",
            Step::Autostart => "autostart",
        }
    }

    /// The settings rows the step shows; each writes through the same code as in the panel.
    fn rows(self) -> &'static [Row] {
        match self {
            Step::Language => &[Row::Language],
            Step::Hotkey => &[Row::Hotkey],
            Step::Layout => &[Row::Side, Row::Actions],
            Step::Theme => &[Row::Theme],
            Step::Autostart => &[Row::Autostart],
        }
    }
}

/// What Enter / DALEJ did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Advance {
    Moved,
    /// Enter on the last step: the wizard is over.
    Finished,
}

/// State of the first-run wizard. Choices are not kept here: each is written to config.toml (or
/// the registry) as it is made, like in the panel. AUTOSTART alone is shown as ON before it is
/// touched and only written when the wizard finishes.
#[derive(Clone, Debug, PartialEq)]
pub struct Wizard {
    pub step: usize,
    pub autostart: bool,
    /// Opened because the config is new, not from RUN TUTORIAL: finishing then closes the panel.
    pub first_run: bool,
}

impl Wizard {
    pub fn new(first_run: bool) -> Self {
        Self { step: 0, autostart: true, first_run }
    }

    pub fn step(&self) -> Step {
        STEPS[self.step.min(WIZARD_STEPS - 1)]
    }

    pub fn is_last(&self) -> bool {
        self.step + 1 >= WIZARD_STEPS
    }

    /// Whether the hotkey capture should be running: on the HOTKEY step.
    pub fn captures(&self) -> bool {
        self.step() == Step::Hotkey
    }
}

/// What the renderer draws around the wizard's rows (which come as the view's slots).
#[derive(Debug, PartialEq)]
pub struct WizardView {
    /// `STEP n/5`.
    pub title: String,
    pub heading: String,
    /// Explanation under the rows, one entry per line.
    pub text: Vec<String>,
    pub can_back: bool,
    /// NEXT, or FINISH on the last step.
    pub next: String,
}

impl Panel {
    /// A panel for the files and registry state that exist now (themes, languages, autostart).
    pub fn scanned() -> Self {
        Self { themes: themes::list(), languages: lang::languages(), autostart: autostart::is_enabled(), fonts: fonts::installed_now(), ..Self::default() }
    }

    /// Starts (or restarts) the wizard on this panel.
    pub fn start_wizard(&mut self, first_run: bool) {
        // Started again from the panel, AUTOSTART begins at what the registry has instead of ON.
        self.wizard = Some(Wizard { autostart: first_run || self.autostart,..Wizard::new(first_run) });
        self.reset_step();
    }

    /// DALEJ / Enter: the next step, or `Finished` on the last. A no-op without a wizard.
    pub fn wizard_next(&mut self) -> Advance {
        let Some(w) = self.wizard.as_mut() else { return Advance::Moved };
        if w.is_last() {
            return Advance::Finished;
        }
        w.step += 1;
        self.reset_step();
        Advance::Moved
    }

    /// WSTECZ / Backspace: the previous step; false on the first.
    pub fn wizard_back(&mut self) -> bool {
        let Some(w) = self.wizard.as_mut().filter(|w| w.step > 0) else { return false };
        w.step -= 1;
        self.reset_step();
        true
    }

    /// Enter or a click on the HOTKEY row: the next key press becomes the hotkey.
    pub fn begin_capture(&mut self) {
        self.capturing = true;
        self.captured = false;
    }

    /// A key was taken and written. The settings panel is done capturing; the wizard's HOTKEY step
    /// keeps capturing so the key is not live yet (pressing it again would hide the menu and the
    /// wizard with it) and a different press replaces it. Leaving the step ends the capture.
    pub fn capture_done(&mut self) {
        self.captured = true;
        self.capturing = self.wizard.as_ref().is_some_and(Wizard::captures);
    }

    /// Selection on the first row of the step that just began.
    fn reset_step(&mut self) {
        self.captured = false;
        self.selected = 0;
        self.top = 0;
        self.cancel_edit();
    }

    /// Takes the config's view of what decides which rows exist. The selection stays on the row it
    /// was on (the one just changed) when rows appear or disappear around it.
    pub fn sync_config(&mut self, c: &Config) {
        let popout = c.layout.actions == ActionsMode::Popout;
        if popout == self.popout {
            return;
        }
        let (row, button) = (self.current(), self.selected.checked_sub(self.rows().len()));
        self.popout = popout;
        let rows = self.rows();
        self.selected = match (row, button) {
            (Some(row), _) => rows.iter().position(|&r| r == row).unwrap_or(self.selected.min(rows.len().saturating_sub(1))),
            (None, Some(at)) => rows.len() + at,
            (None, None) => 0,
        };
        self.keep_in_view();
    }

    /// The selected row's next config entry in direction `dir`, from the lists the panel scanned.
    pub fn change(&self, c: &Config, dir: i32) -> Option<Change> {
        self.current()?.change_in(c, dir, &self.themes, &self.languages)
    }

    /// Everything the current tab lists, in order. On PLUGINS that is the master switch and the
    /// folder button, then a section for each installed plugin.
    pub fn lines(&self) -> Vec<Line> {
        if let Some(w) = &self.wizard {
            return w.step().rows().iter().copied().map(Line::Row).collect();
        }
        let mut lines = self.tab.lines();
        if self.tab == Tab::Layout && !self.popout {
            // Where the pop-out opens means nothing while the actions replace the list.
            lines.retain(|l| !matches!(l, Line::Row(Row::ActionsAnchor | Row::ActionsX | Row::ActionsY)));
        }
        if self.tab == Tab::Plugins {
            if self.plugins_on && self.plugins.is_empty() {
                lines.push(Line::Heading(Head::Section("plugins_none")));
            }
            for (i, plugin) in self.plugins.iter().enumerate().take(u8::MAX as usize + 1) {
                lines.extend(plugin_lines(i, plugin));
            }
        }
        lines
    }

    /// The selectable rows of `lines`.
    pub fn rows(&self) -> Vec<Row> {
        rows_of(&self.lines())
    }

    /// Takes a fresh list of plugins, keeping the selection on a row that still exists.
    pub fn set_plugins(&mut self, on: bool, plugins: Vec<PluginInfo>) {
        // A selected button stays on its button; a row stays on its number if the list still has it.
        let button = self.selected.checked_sub(self.rows().len());
        self.plugins_on = on;
        self.plugins = plugins;
        let rows = self.rows().len();
        self.selected = match button {
            Some(at) => rows + at,
            None => self.selected.min(rows.saturating_sub(1)),
        };
        self.keep_in_view();
    }

    /// The selected row, or `None` on one of the buttons.
    pub fn current(&self) -> Option<Row> {
        self.rows().get(self.selected).copied()
    }

    /// The selected button, if the selection is below the rows.
    pub fn button(&self) -> Option<Button> {
        match self.selected.checked_sub(self.rows().len()) {
            Some(0) => Some(Button::Config),
            Some(1) => Some(Button::Style),
            _ => None,
        }
    }

    pub fn switch_tab(&mut self, delta: i32) {
        let next = (self.tab.index() as i32 + delta).rem_euclid(TABS.len() as i32) as usize;
        self.select_tab(next);
    }

    pub fn select_tab(&mut self, index: usize) {
        if let Some(&tab) = TABS.get(index) {
            self.tab = tab;
            self.selected = 0;
            self.top = 0;
            self.cancel_edit();
        }
    }

    /// Drops typed text and anything waiting for a second press.
    fn cancel_edit(&mut self) {
        self.editing = None;
        self.pending = None;
    }

    /// Moves between the rows and the buttons below them, stopping at both ends.
    pub fn move_selection(&mut self, delta: i32) {
        // The wizard has no OPEN CONFIG / OPEN STYLE buttons under its rows.
        let last = (self.rows().len() + if self.wizard.is_some() { 0 } else { BUTTONS } - 1) as i32;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
        self.cancel_edit();
        self.keep_in_view();
    }

    /// Selects row `index` (as numbered by `row_at`).
    pub fn select(&mut self, index: usize) {
        // A second click on the same row is the answer to its question (APPROVE asks first).
        let again = index == self.selected;
        self.selected = index;
        self.editing = None;
        if !again {
            self.pending = None;
        }
        self.keep_in_view();
    }

    /// Scrolls the visible part by `delta` lines without moving the selection (mouse wheel).
    pub fn scroll_by(&mut self, delta: i32) {
        let most = self.lines().len().saturating_sub(SLOTS);
        self.top = (self.top as i32 + delta).clamp(0, most as i32) as usize;
    }

    /// Scrolls just enough to show the selected row, and the heading above it if it is the first of its section.
    fn keep_in_view(&mut self) {
        let lines = self.lines();
        let Some(at) = lines.iter().enumerate().filter(|(_, l)| matches!(l, Line::Row(_))).nth(self.selected).map(|(i, _)| i) else {
            return;
        };
        let first = if at > 0 && matches!(lines[at - 1], Line::Heading(_)) { at - 1 } else { at };
        if first < self.top {
            self.top = first;
        } else if at >= self.top + SLOTS {
            self.top = at + 1 - SLOTS;
        }
        self.top = self.top.min(lines.len().saturating_sub(SLOTS));
    }

    /// For each visible slot, the number of the row in it (`None` for a heading).
    pub fn row_slots(&self) -> Vec<Option<usize>> {
        let lines = self.lines();
        let before = lines[..self.top.min(lines.len())].iter().filter(|l| matches!(l, Line::Row(_))).count();
        let mut next = before;
        lines
            .iter()
            .skip(self.top)
            .take(SLOTS)
            .map(|line| match line {
                Line::Row(_) => {
                    next += 1;
                    Some(next - 1)
                }
                Line::Heading(_) => None,
            })
            .collect()
    }

    /// The row number in slot `slot`, if there is a row there.
    pub fn row_at(&self, slot: usize) -> Option<usize> {
        self.row_slots().get(slot).copied().flatten()
    }

    /// Starts typing into the selected row: a color's hex, a theme name, or (starting from the
    /// current text) the header title / subtitle.
    pub fn begin_edit(&mut self) {
        let (kind, text) = match self.current() {
            Some(Row::Color(_)) => (EditKind::Hex, String::new()),
            Some(Row::SaveTheme) => (EditKind::Name, String::new()),
            Some(Row::Title) => (EditKind::Text, lang::t("header.title")),
            Some(Row::Subtitle) => (EditKind::Text, lang::t("header.subtitle")),
            Some(Row::Plugin(_, PluginField::Keyword)) => (EditKind::Keyword, self.plugin_target().and_then(|(p, _)| p.keywords.first().cloned()).unwrap_or_default()),
            Some(Row::Plugin(_, PluginField::Setting(_))) => match self.plugin_setting() {
                Some((setting, SettingValue::String(text))) if setting.kind == SettingKind::String => (EditKind::Setting, text.clone()),
                _ => return,
            },
            _ => return,
        };
        self.edit_kind = kind;
        let cap = if kind == EditKind::Setting { MAX_SETTING } else { MAX_TEXT };
        self.editing = Some(text.chars().take(cap).collect());
        self.pending = None;
    }

    /// Whether Enter on the selected row starts typing.
    pub fn edits(&self) -> bool {
        self.edits_row(self.selected)
    }

    /// Whether row number `row` takes typing, e.g. for a double click on it.
    pub fn edits_row(&self, row: usize) -> bool {
        match self.rows().get(row) {
            Some(&Row::Plugin(i, PluginField::Setting(k))) => {
                self.plugins.get(i as usize).and_then(|p| p.settings.get(k as usize)).is_some_and(|s| s.kind == SettingKind::String)
            }
            Some(row) => row.edits(),
            None => false,
        }
    }

    /// The plugin and field of the selected row, if it is a plugin's.
    fn plugin_target(&self) -> Option<(&PluginInfo, PluginField)> {
        match self.current()? {
            Row::Plugin(i, field) => Some((self.plugins.get(i as usize)?, field)),
            _ => None,
        }
    }

    /// The manifest setting behind the selected row and its current value.
    fn plugin_setting(&self) -> Option<(&SettingInfo, &SettingValue)> {
        let (plugin, PluginField::Setting(i)) = self.plugin_target()? else { return None };
        let setting = plugin.settings.get(i as usize)?;
        Some((setting, &setting.value))
    }

    /// Enter or an arrow on a plugin row that toggles or steps: the config entry to write.
    pub fn plugin_step(&self, dir: i32) -> Option<PluginWrite> {
        let (plugin, field) = self.plugin_target()?;
        let id = plugin.id.clone();
        match field {
            PluginField::Enabled => Some(PluginWrite::Value { id, key: "enabled", value: (!plugin.enabled).into() }),
            PluginField::Global => Some(PluginWrite::Value { id, key: "global", value: (!plugin.global).into() }),
            PluginField::Setting(_) => {
                let (setting, value) = self.plugin_setting()?;
                let value: Value = match (&setting.kind, value) {
                    (SettingKind::Bool, SettingValue::Bool(on)) => (!on).into(),
                    (SettingKind::Choice(choices), SettingValue::String(now)) if !choices.is_empty() => {
                        let at = choices.iter().position(|c| c == now).unwrap_or(0) as i32;
                        choices[(at + dir.signum()).rem_euclid(choices.len() as i32) as usize].as_str().into()
                    }
                    _ => return None,
                };
                Some(PluginWrite::Setting { id, key: setting.key.clone(), value })
            }
            _ => None,
        }
    }

    /// Delete on a font row: the `[fonts]` keys to remove from style.toml (back to the theme's, else
    /// the built-in font). A role goes back whole: family, size and weight.
    pub fn font_resets(&self) -> Vec<String> {
        match self.current() {
            Some(Row::FontFamily) => vec!["family".to_owned()],
            Some(Row::FontRole(role)) => ["family", "size", "weight"].map(|field| format!("{}.{field}", role.key())).to_vec(),
            _ => Vec::new(),
        }
    }

    /// Enter or a click on a font row: opens its dropdown on the family in use.
    pub fn open_dropdown(&mut self, fonts: &Fonts) {
        let role = match self.current() {
            Some(Row::FontFamily) => None,
            Some(Row::FontRole(role)) => Some(role),
            Some(Row::HeaderToggles) => {
                self.cancel_edit();
                self.toggles = Some(Toggles::default());
                return;
            }
            _ => return,
        };
        self.cancel_edit();
        self.dropdown = Some(Dropdown::open(role, &self.fonts, fonts));
    }

    pub fn close_dropdown(&mut self) {
        self.dropdown = None;
        self.toggles = None;
    }

    /// Typing, or a dropdown over the card: either way the keys are not the panel's.
    pub fn typing(&self) -> bool {
        self.editing.is_some() || self.dropdown.is_some() || self.toggles.is_some()
    }

    /// A key for the open dropdown.
    pub fn dropdown_key(&mut self, key: Key, fonts: &Fonts) -> Outcome {
        match self.dropdown.as_mut() {
            Some(dropdown) => dropdown.key_press(key, &self.fonts, fonts),
            None => Outcome::Stay,
        }
    }

    /// A key for the open toggle pop-out; `Flip` carries the `[show]` key and the value to write.
    pub fn toggles_key(&mut self, key: Key, show: &Show) -> Option<(&'static str, toml_edit::Value)> {
        let act = self.toggles.as_mut()?.key_press(key, SHOW_HEADER.len());
        self.toggles_act(act, show)
    }

    /// A click on entry `n` of the open toggle pop-out.
    pub fn toggles_click(&mut self, n: usize, show: &Show) -> Option<(&'static str, toml_edit::Value)> {
        let act = self.toggles.as_mut()?.click_item(n, SHOW_HEADER.len());
        self.toggles_act(act, show)
    }

    fn toggles_act(&mut self, act: togglemenu::Act, show: &Show) -> Option<(&'static str, toml_edit::Value)> {
        match act {
            togglemenu::Act::Stay => None,
            togglemenu::Act::Close => {
                self.toggles = None;
                None
            }
            togglemenu::Act::Flip(n) => {
                let key = *style::SHOW_KEYS.get(SHOW_HEADER.start + n)?;
                show.stepped(key, 1).map(|value| (key, value))
            }
        }
    }

    /// Screen slot of the row the open toggle pop-out hangs from, and its number of entries.
    pub fn toggles_geom(&self) -> Option<(usize, usize)> {
        self.toggles.as_ref()?;
        let slot = self.row_slots().iter().position(|&r| r == Some(self.selected)).unwrap_or(0);
        Some((slot, SHOW_HEADER.len()))
    }

    /// A typed character for the open dropdown's filter.
    pub fn dropdown_char(&mut self, c: char) {
        if let Some(dropdown) = self.dropdown.as_mut() {
            dropdown.type_char(c, &self.fonts);
        }
    }

    /// A click on the entry `n` places down the open dropdown's window.
    pub fn dropdown_click(&mut self, n: usize) -> Outcome {
        match self.dropdown.as_mut() {
            Some(dropdown) => dropdown.click_item(n, &self.fonts),
            None => Outcome::Stay,
        }
    }

    /// Where the open dropdown hangs: under the selected row's screen slot.
    pub fn drop_geom(&self) -> Option<DropGeom> {
        let dropdown = self.dropdown.as_ref()?;
        let slot = self.row_slots().iter().position(|&r| r == Some(self.selected)).unwrap_or(0);
        Some(DropGeom { slot, role: dropdown.role().is_some() })
    }

    /// Enter after typing a keyword: nothing typed removes the override (back to the manifest's
    /// keyword); a keyword another plugin answers to, or one that isn't valid, keeps typing going
    /// with a message and writes nothing.
    pub fn plugin_keyword(&mut self) -> Option<PluginWrite> {
        let typed = self.editing.as_deref().unwrap_or_default().trim().to_lowercase();
        let (plugin, _) = self.plugin_target()?;
        let id = plugin.id.clone();
        self.flash = Note::None;
        if typed.is_empty() {
            self.cancel_edit();
            return Some(PluginWrite::Remove { id, key: "keyword" });
        }
        if !plugins::valid_keyword(&typed) {
            self.flash = Note::KeywordInvalid;
            return None;
        }
        if self.plugins.iter().any(|other| other.id != id && other.keywords.contains(&typed)) {
            self.flash = Note::KeywordInUse;
            return None;
        }
        self.cancel_edit();
        Some(PluginWrite::Value { id, key: "keyword", value: typed.into() })
    }

    /// Enter after typing a string setting.
    pub fn plugin_text(&mut self) -> Option<PluginWrite> {
        let (plugin, _) = self.plugin_target()?;
        let (id, key) = (plugin.id.clone(), self.plugin_setting()?.0.key.clone());
        let typed = self.finish_edit()?;
        Some(PluginWrite::Setting { id, key, value: typed.into() })
    }

    /// Delete on the KEYWORD row: back to the manifest's keyword.
    pub fn plugin_keyword_reset(&self) -> Option<PluginWrite> {
        let (plugin, PluginField::Keyword) = self.plugin_target()? else { return None };
        Some(PluginWrite::Remove { id: plugin.id.clone(), key: "keyword" })
    }

    /// Enter on the APPROVE row: the first press asks, the second returns the id to approve.
    pub fn plugin_approve(&mut self) -> Option<String> {
        let (plugin, PluginField::Approve) = self.plugin_target()? else { return None };
        let id = plugin.id.clone();
        let ask = Pending::Approve(id.clone());
        if self.pending.as_ref() == Some(&ask) {
            self.pending = None;
            return Some(id);
        }
        self.pending = Some(ask);
        None
    }

    /// Enter on CLEAR HISTORY: the first press asks, the second (with nothing else in between) returns true.
    pub fn clear_history(&mut self) -> bool {
        if self.current() != Some(Row::ClearHistory) {
            return false;
        }
        if self.pending == Some(Pending::ClearHistory) {
            self.pending = None;
            self.flash = Note::HistoryCleared;
            return true;
        }
        self.pending = Some(Pending::ClearHistory);
        false
    }

    /// The id of the plugin whose selected row is the given kind, e.g. for OPEN LOG.
    pub fn plugin_id_of(&self, wanted: PluginField) -> Option<&str> {
        let (plugin, field) = self.plugin_target()?;
        (field == wanted).then_some(plugin.id.as_str())
    }

    /// Adds a typed character if the kind of text being typed takes it: `#` first and up to 8 hex
    /// digits, theme name characters up to `themes::MAX_NAME`, or any printable character.
    pub fn edit_char(&mut self, c: char) {
        let Some(text) = &mut self.editing else { return };
        let length = text.chars().count();
        let digits = text.chars().filter(char::is_ascii_hexdigit).count();
        let accepted = match self.edit_kind {
            EditKind::Hex => (c == '#' && text.is_empty()) || (c.is_ascii_hexdigit() && digits < 8),
            EditKind::Name => themes::valid_char(c) && length < themes::MAX_NAME,
            EditKind::Text => !c.is_control() && length < MAX_TEXT,
            EditKind::Keyword => (c.is_ascii_alphanumeric() || c == '-' || c == '_') && length < 16,
            EditKind::Setting => !c.is_control() && length < MAX_SETTING,
        };
        if accepted {
            text.push(if self.edit_kind == EditKind::Keyword { c.to_ascii_lowercase() } else { c });
            self.pending = None;
        }
    }

    /// Ctrl+V while a field is typed into: the clipboard's text goes through `edit_char`, so the
    /// field's own character rules and length limit apply.
    pub fn paste(&mut self, text: &str) {
        for c in commands::clean_paste(text, MAX_SETTING).chars() {
            self.edit_char(c);
        }
    }

    pub fn edit_backspace(&mut self) {
        if let Some(text) = &mut self.editing {
            text.pop();
            self.pending = None;
        }
    }

    /// Ends typing and returns what was typed.
    pub fn finish_edit(&mut self) -> Option<String> {
        self.pending = None;
        self.editing.take()
    }

    /// Ends typing without applying it (Esc).
    pub fn abort_edit(&mut self) {
        self.cancel_edit();
    }

    /// Enter on a typed theme name. A name that is empty or a preset keeps typing going with a
    /// message; one a custom theme already has needs Enter once more.
    pub fn check_name(&mut self) -> NameCheck {
        let name = self.editing.as_deref().unwrap_or_default().trim().to_owned();
        self.flash = Note::None;
        if name.is_empty() {
            self.flash = Note::EmptyName;
            return NameCheck::Rejected;
        }
        if themes::is_reserved(&name) {
            self.flash = Note::BuiltinName;
            return NameCheck::Rejected;
        }
        let taken = self.themes.iter().any(|t| t.eq_ignore_ascii_case(&name));
        let confirmed = self.pending == Some(Pending::Overwrite(name.clone()));
        if taken && !confirmed {
            self.pending = Some(Pending::Overwrite(name));
            return NameCheck::Confirm;
        }
        self.cancel_edit();
        NameCheck::Save(name)
    }

    /// Delete on the THEME row: the first press asks, the second (with nothing else in between)
    /// removes the active custom theme. `c.theme_name` is the theme the row shows.
    pub fn request_delete(&mut self, c: &Config) -> DeleteStep {
        if self.current() != Some(Row::Theme) || themes::is_builtin(&c.theme_name) {
            return DeleteStep::Nothing;
        }
        let ask = Pending::Delete(c.theme_name.clone());
        if self.pending.as_ref() == Some(&ask) {
            self.pending = None;
            return DeleteStep::Remove(c.theme_name.clone());
        }
        self.pending = Some(ask);
        DeleteStep::Confirm
    }

    /// `hotkey_failed`: the configured combination could not be registered (another program has it).
    /// `style` is the colors in use; the theme's own colors give the defaults to compare against.
    pub fn view(&self, c: &Config, style: &Style, hotkey_failed: bool) -> PanelView {
        if let Some(w) = &self.wizard {
            return self.wizard_view(w, c, hotkey_failed);
        }
        let waiting = self.pending.as_ref().map(|p| match p {
            Pending::Overwrite(_) => Note::OverwriteTheme,
            Pending::Delete(_) => Note::DeleteTheme,
            Pending::Approve(_) => Note::ApprovePlugin,
            Pending::ClearHistory => Note::ClearHistory,
        });
        let note = if self.error {
            Note::Error
        } else if self.style_error {
            Note::StyleError
        } else if self.flash != Note::None {
            self.flash
        } else if let Some(waiting) = waiting {
            waiting
        } else if self.invalid_hex {
            Note::InvalidHex
        } else if self.tab == Tab::Behavior && hotkey_failed {
            Note::HotkeyInUse
        } else if self.tab == Tab::Colors && c.color_overrides {
            Note::HexOverrides
        } else {
            Note::None
        };
        let defaults = c.defaults();
        let row_view = |row: Row| {
            let editing = self.editing.as_ref().filter(|_| self.current() == Some(row));
            match row {
                Row::Color(i) => {
                    let key = style::KEYS.get(i).copied().unwrap_or_default();
                    let (now, default) = (style.get(key).unwrap_or(defaults.row_name), defaults.get(key));
                    PanelRow {
                        label: row.label(),
                        value: editing.map_or_else(|| format!("#{:06X}", now.rgb), |text| typed(text, "settings.value.type_hex")),
                        stepper: true,
                        color: Some(ColorCell { rgb: now.rgb, alpha: now.alpha, custom: default != Some(now), editing: editing.is_some() }),
                        dropdown: false,
                    }
                }
                Row::Shape(i) => {
                    let key = style::SHAPE_KEYS.get(i).copied().unwrap_or_default();
                    PanelRow { label: row.label(), value: shape_value(&style.shape, key), stepper: true, color: None, dropdown: false }
                }
                Row::Show(i) => {
                    let key = style::SHOW_KEYS.get(i).copied().unwrap_or_default();
                    PanelRow { label: row.label(), value: show_value(&style.show, key), stepper: true, color: None, dropdown: false }
                }
                Row::HeaderToggles => PanelRow { label: row.label(), value: String::new(), stepper: false, color: None, dropdown: true },
                Row::FontFamily | Row::FontRole(_) => {
                    let role = if let Row::FontRole(role) = row { Some(role) } else { None };
                    PanelRow { label: row.label(), value: font_row_text(&style.fonts, role), stepper: false, color: None, dropdown: true }
                }
                Row::Plugin(i, field) => self.plugin_row(i, field, editing),
                _ => {
                    let hint = if row == Row::SaveTheme { "settings.value.type_name" } else { "settings.value.type_text" };
                    PanelRow {
                        label: row.label(),
                        value: match editing {
                            Some(text) => typed(text, hint),
                            None if self.capturing && row == Row::Hotkey => lang::t("settings.value.press_keys"),
                            None if row == Row::Autostart => lang::t(if self.autostart { "settings.value.on" } else { "settings.value.off" }),
                            None => row.value(c),
                        },
                        stepper: row.stepped(),
                        color: None,
                        dropdown: false,
                    }
                }
            }
        };
        let lines = self.lines();
        let slots: Vec<Slot> = lines
            .iter()
            .skip(self.top)
            .take(SLOTS)
            .map(|line| match *line {
                Line::Heading(Head::Section(table)) => Slot::Heading(lang::t(&format!("settings.section.{table}"))),
                Line::Heading(Head::Plugin(i)) => Slot::Heading(self.plugin_heading(i)),
                Line::Row(row) => Slot::Row(row_view(row)),
            })
            .collect();
        let selected = self.row_slots().iter().position(|&r| r == Some(self.selected));
        let scroll = (lines.len() > SLOTS).then(|| {
            let total = lines.len() as f32;
            (self.top as f32 / total, (self.top + SLOTS).min(lines.len()) as f32 / total)
        });
        let dropdown = self.dropdown.as_ref().zip(self.drop_geom()).map(|(dropdown, geom)| DropdownView { title: self.current().map(Row::label).unwrap_or_default(), ..dropdown.view(geom.slot, &self.fonts, &style.fonts) });
        let toggles = self.toggles.as_ref().zip(self.toggles_geom()).map(|(toggles, (slot, _))| TogglesView {
            title: self.current().map(Row::label).unwrap_or_default(),
            slot,
            entries: SHOW_HEADER.map(|i| togglemenu::Entry { label: Row::Show(i).label(), on: style.show.flag(style::SHOW_KEYS[i]).unwrap_or(true) }).collect(),
            focus: toggles.focus(),
        });
        PanelView { tab: self.tab.index(), slots, selected, button: self.button(), scroll, note, detail: self.detail(), wizard: None, dropdown, toggles }
    }

    /// The wizard's step: its rows as slots, and the texts around them.
    fn wizard_view(&self, w: &Wizard, c: &Config, hotkey_failed: bool) -> PanelView {
        let step = w.step().id();
        let slots = w
            .step()
            .rows()
            .iter()
            .map(|&row| {
                let value = match row {
                    Row::Hotkey if self.capturing && !self.captured => lang::t("settings.value.press_keys"),
                    Row::Autostart => lang::t(if w.autostart { "settings.value.on" } else { "settings.value.off" }),
                    _ => row.value(c),
                };
                Slot::Row(PanelRow { label: row.label(), value, stepper: row.stepped(), color: None, dropdown: false })
            })
            .collect();
        let mut text: Vec<String> = lang::t(&format!("wizard.text.{step}")).split('\n').map(str::to_owned).collect();
        text.extend([String::new(), lang::t("wizard.keys")]);
        let note = if self.error {
            Note::Error
        } else if self.flash != Note::None {
            self.flash
        } else if w.step() == Step::Hotkey && hotkey_failed {
            Note::HotkeyInUse
        } else {
            Note::None
        };
        let title = lang::t("wizard.title").replace("{n}", &(w.step + 1).to_string()).replace("{total}", &WIZARD_STEPS.to_string());
        let wizard = WizardView {
            title,
            heading: lang::t(&format!("wizard.step.{step}")),
            text,
            can_back: w.step > 0,
            next: lang::t(if w.is_last() { "wizard.finish" } else { "wizard.next" }),
        };
        PanelView { tab: 0, slots, selected: Some(self.selected), button: None, scroll: None, note, detail: None, wizard: Some(wizard), dropdown: None, toggles: None }
    }

    /// `NAME V1.2 - AUTHOR`, upper case like every other heading.
    fn plugin_heading(&self, index: u8) -> String {
        let Some(p) = self.plugins.get(index as usize) else { return String::new() };
        let version = if p.version.is_empty() { String::new() } else { format!(" v{}", p.version) };
        let author = if p.author.is_empty() { String::new() } else { format!(" - {}", p.author) };
        clip(&format!("{}{version}{author}", p.name).to_uppercase(), MAX_HEADING)
    }

    /// A line about the selected plugin row, for the note line when there is no note.
    fn detail(&self) -> Option<String> {
        if matches!(self.current(), Some(Row::Color(_))) {
            return Some(lang::t("settings.detail.color_keys"));
        }
        if self.current() == Some(Row::HeaderToggles) {
            return Some(lang::t(if self.toggles.is_some() { "settings.detail.toggles" } else { "settings.detail.toggles_keys" }));
        }
        if self.current().is_some_and(Row::opens_dropdown) {
            return Some(lang::t(match &self.dropdown {
                Some(dropdown) if dropdown.role().is_some() => "settings.detail.dropdown_role",
                Some(_) => "settings.detail.dropdown",
                None => "settings.detail.font_keys",
            }));
        }
        let (plugin, field) = self.plugin_target()?;
        let text = match field {
            PluginField::State => match &plugin.state {
                PluginState::ManifestError(error) => error.clone(),
                PluginState::Failed => lang::t("settings.detail.failed"),
                PluginState::NotApproved => lang::t("settings.detail.not_approved"),
                _ => return None,
            },
            PluginField::Approve => lang::t("settings.detail.runs").replace("{command}", &plugin.run),
            PluginField::Keyword if plugin.keyword_lost => lang::t("settings.detail.keyword_lost"),
            PluginField::Global if plugin.global => lang::t("settings.detail.global_privacy"),
            _ => return None,
        };
        Some(clip(&text, MAX_DETAIL))
    }

    /// The row for a plugin's field. `editing` is the text being typed into it, if any.
    fn plugin_row(&self, index: u8, field: PluginField, editing: Option<&String>) -> PanelRow {
        let row = Row::Plugin(index, field);
        let Some(p) = self.plugins.get(index as usize) else {
            return PanelRow { label: row.label(), value: String::new(), stepper: false, color: None, dropdown: false };
        };
        let word = |id: &str| lang::t(&format!("settings.value.{id}"));
        let on_off = |b: bool| word(if b { "on" } else { "off" });
        let setting = match field {
            PluginField::Setting(k) => p.settings.get(k as usize),
            _ => None,
        };
        let label = match setting {
            Some(s) => clip(&s.label.to_uppercase(), MAX_LABEL),
            None => row.label(),
        };
        let shown = match (field, editing) {
            (_, Some(text)) => typed(text, "settings.value.type_text"),
            (PluginField::State, _) => match &p.state {
                PluginState::Stopped => word("stopped"),
                PluginState::Starting => word("starting"),
                PluginState::Running => word("running"),
                PluginState::Failed => word("failed"),
                PluginState::Disabled => word("disabled"),
                PluginState::NotApproved => word("not_approved"),
                PluginState::ManifestError(_) => word("manifest_error"),
            },
            (PluginField::Enabled, _) => on_off(p.enabled),
            (PluginField::Approve | PluginField::Log, _) => word("enter"),
            (PluginField::Keyword, _) => match (p.keywords.first(), p.keyword_lost) {
                (Some(k), true) => clip(&format!("{k} !"), SHOWN),
                (Some(k), false) => clip(k, SHOWN),
                (None, _) => "-".to_owned(),
            },
            (PluginField::Global, _) => on_off(p.global),
            (PluginField::Setting(_), _) => match setting.map(|s| &s.value) {
                Some(SettingValue::Bool(on)) => on_off(*on),
                Some(SettingValue::String(text)) => {
                    let choice = matches!(setting.map(|s| &s.kind), Some(SettingKind::Choice(_)));
                    clip(&if choice { text.to_uppercase() } else { text.clone() }, SHOWN)
                }
                None => String::new(),
            },
        };
        PanelRow { label, value: shown, stepper: row.stepped() && self.plugin_steps(p, field), color: None, dropdown: false }
    }

    /// Whether a plugin row has arrows: toggles and choices have, typed and pressed rows haven't.
    fn plugin_steps(&self, p: &PluginInfo, field: PluginField) -> bool {
        match field {
            PluginField::Setting(k) => p.settings.get(k as usize).is_some_and(|s| s.kind != SettingKind::String),
            _ => true,
        }
    }
}

/// What a font row shows: the global family, or for a role its family (GLOBAL when it has none of
/// its own), its size and, once set, its weight: `CONSOLAS · 120% · BOLD`.
pub fn font_row_text(fonts: &Fonts, role: Option<Role>) -> String {
    let family = |name: &str| clip(&name.to_uppercase(), SHOWN_FAMILY);
    let Some(role) = role else { return family(fonts.family.as_str()) };
    let own = fonts.role(role);
    let mut parts = vec![own.family.map_or_else(|| lang::t("settings.value.global"), |f| family(f.as_str())), format!("{}%", own.size)];
    if own.weight.is_some() {
        parts.push(weight_label(own.weight));
    }
    parts.join(" \u{B7} ")
}

/// `text` cut to `max` characters, ending in an ellipsis if it was longer.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    format!("{}\u{2026}", text.chars().take(max.saturating_sub(1)).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(f: impl FnOnce(&mut Config)) -> Config {
        let mut c = Config::default();
        f(&mut c);
        c
    }

    fn toml_of(change: Option<Change>) -> Option<(String, String, String)> {
        change.map(|(t, k, v)| (t.into(), k.into(), v.to_string().trim().to_owned()))
    }

    fn step(row: Row, c: &Config, dir: i32) -> Option<(String, String, String)> {
        toml_of(row.change(c, dir))
    }

    fn t(table: &str, key: &str, value: &str) -> Option<(String, String, String)> {
        Some((table.into(), key.into(), value.into()))
    }

    #[test]
    fn theme_cycles_through_the_presets_and_wraps() {
        let c = Config::default();
        assert_eq!(step(Row::Theme, &c, 1), t("colors", "theme", "\"arasaka\""));
        assert_eq!(step(Row::Theme, &c, -1), t("colors", "theme", "\"netwatch\""));
        let last = cfg(|c| c.theme_name = "netwatch".into());
        assert_eq!(step(Row::Theme, &last, 1), t("colors", "theme", "\"cyberpunk\""));
        assert_eq!(Row::Theme.value(&last), "NETWATCH");
    }

    #[test]
    fn side_and_switches_toggle_either_way() {
        let c = Config::default();
        assert_eq!(step(Row::Side, &c, 1), t("layout", "side", "\"right\""));
        assert_eq!(step(Row::Side, &c, -1), t("layout", "side", "\"right\""));
        assert_eq!(step(Row::FileSearch, &cfg(|c| c.file_search = false), -1), t("behavior", "file_search", "true"));
    }

    #[test]
    fn width_steps_by_five_and_clamps() {
        let c = Config::default();
        assert_eq!(step(Row::Width, &c, 1), t("layout", "width_percent", "30"));
        assert_eq!(step(Row::Width, &c, -1), t("layout", "width_percent", "20"));
        assert_eq!(step(Row::Width, &cfg(|c| c.layout.width_percent = 15), -1), None);
        assert_eq!(step(Row::Width, &cfg(|c| c.layout.width_percent = 50), 1), None);
        assert_eq!(step(Row::Width, &cfg(|c| c.layout.width_percent = 48), 1), t("layout", "width_percent", "50"));
    }

    #[test]
    fn rows_go_auto_then_three_to_thirty() {
        let auto = Config::default();
        assert_eq!(Row::Rows.value(&auto), "AUTO");
        assert_eq!(step(Row::Rows, &auto, -1), None);
        assert_eq!(step(Row::Rows, &auto, 1), t("layout", "visible_rows", "3"));
        let three = cfg(|c| c.layout.visible_rows = 3);
        assert_eq!(step(Row::Rows, &three, -1), t("layout", "visible_rows", "0"));
        assert_eq!(step(Row::Rows, &three, 1), t("layout", "visible_rows", "4"));
        let top = cfg(|c| c.layout.visible_rows = 30);
        assert_eq!(step(Row::Rows, &top, 1), None);
        assert_eq!(step(Row::Rows, &top, -1), t("layout", "visible_rows", "29"));
    }

    #[test]
    fn arc_steps_by_ten_with_sign_and_clamps() {
        let c = Config::default();
        assert_eq!(Row::Arc.value(&c), "+40");
        assert_eq!(step(Row::Arc, &cfg(|c| c.layout.arc_shift = 0), -1), t("layout", "arc_shift", "-10"));
        assert_eq!(Row::Arc.value(&cfg(|c| c.layout.arc_shift = 0)), "0");
        assert_eq!(Row::Arc.value(&cfg(|c| c.layout.arc_shift = -30)), "-30");
        assert_eq!(step(Row::Arc, &cfg(|c| c.layout.arc_shift = 120), 1), None);
        assert_eq!(step(Row::Arc, &cfg(|c| c.layout.arc_shift = -120), -1), None);
    }

    #[test]
    fn backdrop_steps_by_tenths_without_float_noise() {
        let c = Config::default();
        assert_eq!(Row::Backdrop.value(&c), "0.6");
        assert_eq!(step(Row::Backdrop, &c, 1), t("layout", "backdrop_opacity", "0.7"));
        assert_eq!(step(Row::Backdrop, &c, -1), t("layout", "backdrop_opacity", "0.5"));
        assert_eq!(step(Row::Backdrop, &cfg(|c| c.layout.backdrop_opacity = 1.0), 1), None);
        assert_eq!(step(Row::Backdrop, &cfg(|c| c.layout.backdrop_opacity = 0.0), -1), None);
    }

    #[test]
    fn file_results_step_by_two_and_clamp() {
        let c = Config::default();
        assert_eq!(step(Row::FileResults, &c, 1), t("behavior", "file_results", "14"));
        assert_eq!(step(Row::FileResults, &cfg(|c| c.file_results = 0), -1), None);
        assert_eq!(step(Row::FileResults, &cfg(|c| c.file_results = 50), 1), None);
        assert_eq!(step(Row::FileResults, &cfg(|c| c.file_results = 1), -1), t("behavior", "file_results", "0"));
    }

    #[test]
    fn icons_row_toggles_layout_icons() {
        let c = Config::default();
        assert_eq!(Row::Icons.value(&c), "ON");
        assert_eq!(step(Row::Icons, &c, 1), t("layout", "icons", "false"));
        let off = cfg(|c| c.layout.icons = false);
        assert_eq!((Row::Icons.value(&off), Row::Icons.label().as_str()), ("OFF".to_owned(), "ICONS"));
        assert_eq!(step(Row::Icons, &off, -1), t("layout", "icons", "true"));
        assert!(Tab::Layout.rows().contains(&Row::Icons));
    }

    #[test]
    fn actions_rows_write_the_layout_keys() {
        let c = Config::default();
        assert_eq!((Row::Actions.value(&c).as_str(), Row::ActionsAnchor.value(&c).as_str()), ("LIST", "ROW"));
        assert_eq!(step(Row::Actions, &c, 1), t("layout", "actions", "\"popout\""));
        let popout = cfg(|c| c.layout.actions = ActionsMode::Popout);
        assert_eq!((Row::Actions.value(&popout).as_str(), step(Row::Actions, &popout, -1)), ("POPOUT", t("layout", "actions", "\"list\"")));
        assert_eq!(step(Row::ActionsAnchor, &c, 1), t("layout", "actions_anchor", "\"list\""));
        assert_eq!(step(Row::ActionsAnchor, &c, -1), None);
        let list = cfg(|c| c.layout.actions_anchor = ActionsAnchor::List);
        assert_eq!(step(Row::ActionsAnchor, &list, 1), t("layout", "actions_anchor", "\"cursor\""));
        assert_eq!(step(Row::ActionsAnchor, &list, -1), t("layout", "actions_anchor", "\"row\""));
        let cursor = cfg(|c| c.layout.actions_anchor = ActionsAnchor::Cursor);
        assert_eq!((Row::ActionsAnchor.value(&cursor).as_str(), step(Row::ActionsAnchor, &cursor, 1)), ("CURSOR", None));
        assert_eq!((Row::ActionsX.value(&c).as_str(), Row::ActionsX.label().as_str()), ("0", "OFFSET X"));
        assert_eq!(step(Row::ActionsX, &c, 1), t("layout", "actions_x", "5"));
        assert_eq!(step(Row::ActionsY, &c, -1), t("layout", "actions_y", "-5"));
        let far = cfg(|c| (c.layout.actions_x, c.layout.actions_y) = (100, -100));
        assert_eq!((step(Row::ActionsX, &far, 1), step(Row::ActionsY, &far, -1)), (None, None));
        assert_eq!(step(Row::ActionsX, &far, -1), t("layout", "actions_x", "95"));
        assert_eq!((Row::ActionsX.value(&far).as_str(), Row::ActionsY.value(&far).as_str()), ("+100%", "-100%"));
        assert!(Tab::Layout.rows().contains(&Row::ActionsY));
    }

    fn shape_step(key: &str, shape: &Shape, dir: i32) -> Option<String> {
        let i = style::SHAPE_KEYS.iter().position(|&k| k == key).unwrap();
        Row::Shape(i).shape_change(shape, dir).map(|(k, v)| {
            assert_eq!(k, key);
            v.to_string().trim().to_owned()
        })
    }

    #[test]
    fn look_tab_lists_the_scale_every_shape_key_once_then_the_header_and_the_fonts() {
        let mut expected = vec![Row::GuiScale];
        expected.extend((0..style::SHAPE_KEYS.len()).map(Row::Shape));
        expected.extend([Row::Title, Row::Subtitle]);
        expected.extend(style::SHOW_ALIGNS.map(Row::Show));
        expected.push(Row::HeaderToggles);
        expected.extend(style::SHOW_DIMMING.map(Row::Show));
        expected.extend(style::SHOW_ELEMENTS.map(Row::Show));
        expected.push(Row::FontFamily);
        expected.extend(ROLES.map(Row::FontRole));
        assert_eq!(Tab::Look.rows(), expected);
        assert_eq!(Tab::Look.lines().iter().filter(|l| matches!(l, Line::Heading(Head::Section("fonts")))).count(), 1);
        assert_eq!(Tab::Look.lines().iter().filter(|l| matches!(l, Line::Heading(Head::Section("header")))).count(), 1);
        assert_eq!(Tab::Look.label(), "LOOK");
        assert!(!Tab::Layout.rows().iter().any(|r| *r == Row::GuiScale), "moved to LOOK");
    }

    #[test]
    fn shape_rows_step_inside_the_ranges() {
        assert_eq!(Row::Shape(0).label(), "ROW RADIUS");
        let d = Shape::default();
        assert_eq!(shape_step("row_radius", &d, 1).as_deref(), Some("1"));
        assert_eq!(shape_step("row_radius", &d, -1), None, "0 is the lowest radius");
        assert_eq!(shape_step("panel_radius", &Shape { panel_radius: 24.0, ..d }, 1), None);
        assert_eq!(shape_step("corner", &d, 1).as_deref(), Some("\"chamfer\""));
        assert_eq!(shape_step("corner", &Shape { corner: Corner::Chamfer, ..d }, -1).as_deref(), Some("\"round\""));
        assert_eq!(shape_step("border_width", &d, 1).as_deref(), Some("1.5"));
        assert_eq!(shape_step("border_width", &d, -1).as_deref(), Some("0.5"));
        assert_eq!(shape_step("border_width", &Shape { border_width: 4.0, ..d }, 1), None);
        assert_eq!(shape_step("row_height", &d, 1).as_deref(), Some("56"));
        assert_eq!(shape_step("row_height", &Shape { row_height: 36.0, ..d }, -1), None);
        assert_eq!(shape_step("row_height", &Shape { row_height: 89.0, ..d }, 1).as_deref(), Some("90"));
        assert_eq!(shape_step("row_gap", &d, -1).as_deref(), Some("8"));
        assert_eq!(shape_step("row_gap", &Shape { row_gap: 30.0, ..d }, 1), None);
        assert!(Row::Side.shape_change(&d, 1).is_none());
    }

    #[test]
    fn shape_rows_show_the_styles_values_and_write_the_right_keys() {
        let c = Config::default();
        let shape = Shape { row_radius: 6.0, corner: Corner::Chamfer, border_width: 2.5, row_height: 60.0, ..Shape::default() };
        let mut style = Style::from_theme(&c.colors);
        style.shape = shape;
        // Scrolled past GUI SCALE, which comes first on the tab.
        let p = Panel { tab: Tab::Look, top: 1, ..Default::default() };
        let v = p.view(&c, &style, false);
        let value = |i: usize| match &v.slots[i] {
            Slot::Row(row) => (row.label.clone(), row.value.clone(), row.stepper),
            other => panic!("{other:?}"),
        };
        assert_eq!(value(0), ("ROW RADIUS".to_owned(), "6 PX".to_owned(), true));
        assert_eq!(value(4), ("CORNER".to_owned(), "CHAMFER".to_owned(), true));
        assert_eq!(value(5), ("BORDER WIDTH".to_owned(), "2.5 PX".to_owned(), true));
        assert_eq!(value(6).1, "60 PX");
        // What a step writes parses back to the stepped value.
        let (key, stepped) = Row::Shape(0).shape_change(&shape, 1).unwrap();
        let doc = format!("[shape]\n{key} = {}\n", stepped.to_string().trim());
        assert_eq!(Style::resolve(&c.colors, &doc).shape.row_radius, 7.0);
    }

    #[test]
    fn every_change_survives_a_round_trip_through_the_config_writer() {
        let mut c = Config::default();
        for tab in TABS {
            for row in tab.rows() {
                for dir in [1, -1] {
                    if let Some((table, key, value)) = row.change(&c, dir) {
                        let text = crate::config::set_in("", table, key, value).unwrap();
                        let parsed = crate::config::parse(&text);
                        assert_ne!(row.value(&parsed), row.value(&c), "{row:?} {dir}");
                        c = parsed;
                    }
                }
            }
        }
    }

    #[test]
    fn selection_moves_over_rows_and_the_buttons() {
        let mut p = Panel { tab: Tab::Layout, ..Default::default() };
        assert_eq!(p.current(), Some(Row::Side));
        p.move_selection(-1);
        assert_eq!(p.selected, 0);
        p.move_selection(20);
        assert_eq!(p.selected, 11, "10 rows (no pop-out placement), then 2 buttons");
        assert_eq!((p.current(), p.button()), (None, Some(Button::Style)));
        p.move_selection(-1);
        assert_eq!(p.button(), Some(Button::Config));
        p.move_selection(-1);
        assert_eq!((p.current(), p.button()), (Some(Row::Actions), None));
        p.switch_tab(1);
        assert_eq!((p.tab, p.selected), (Tab::Behavior, 0));
        p.switch_tab(1);
        assert_eq!(p.tab, Tab::Plugins);
        p.switch_tab(1);
        assert_eq!(p.tab, Tab::Colors);
        p.switch_tab(-1);
        assert_eq!(p.tab, Tab::Plugins);
    }

    #[test]
    fn view_lists_rows_and_picks_the_note() {
        let mut p = Panel::default();
        let c = Config::default();
        let st = |c: &Config| Style::from_theme(&c.colors);
        let v = p.view(&c, &st(&c), false);
        assert_eq!((v.tab, v.slots.len(), v.note), (0, SLOTS, Note::None));
        assert_eq!(
            v.slots[0],
            Slot::Row(PanelRow { label: "THEME".to_owned(), value: "CYBERPUNK".to_owned(), stepper: true, color: None, dropdown: false })
        );
        assert_eq!(v.slots[1], Slot::Row(PanelRow { label: "SAVE AS THEME".to_owned(), value: "ENTER".to_owned(), stepper: false, color: None, dropdown: false }));
        assert_eq!(v.slots[2], Slot::Heading("HEADER".to_owned()));
        let overridden = cfg(|c| c.color_overrides = true);
        assert_eq!(p.view(&overridden, &st(&overridden), false).note, Note::HexOverrides);
        p.select_tab(1);
        assert_eq!(p.view(&overridden, &st(&overridden), false).note, Note::None, "only on the colors tab");
        p.select_tab(3);
        assert_eq!(p.view(&c, &st(&c), true).note, Note::HotkeyInUse);
        assert_eq!(p.view(&c, &st(&c), false).note, Note::None);
        p.invalid_hex = true;
        assert_eq!(p.view(&c, &st(&c), true).note, Note::InvalidHex);
        p.style_error = true;
        assert_eq!(p.view(&c, &st(&c), true).note, Note::StyleError);
        p.error = true;
        assert_eq!(p.view(&c, &st(&c), true).note, Note::Error, "errors win");
    }

    #[test]
    fn hotkey_row_shows_the_key_and_the_capture_prompt() {
        let mut p = Panel { tab: Tab::Behavior, ..Default::default() };
        let c = cfg(|c| c.hotkey = Hotkey::parse("ctrl+alt+space").unwrap());
        let s = Style::from_theme(&c.colors);
        let Slot::Row(row) = &p.view(&c, &s, false).slots[0] else { panic!("row expected") };
        assert_eq!((row.label.as_str(), row.value.as_str(), row.stepper), ("HOTKEY", "CTRL+ALT+SPACE", false));
        p.capturing = true;
        let Slot::Row(row) = &p.view(&c, &s, false).slots[0] else { panic!("row expected") };
        assert_eq!(row.value, "PRESS KEYS...");
        assert!(Row::Hotkey.change(&c, 1).is_none(), "not a stepper");
        assert_eq!(Row::Hotkey.value(&Config::default()), "WIN");
    }

    #[test]
    fn short_list_and_band_rows() {
        let c = Config::default();
        assert_eq!(Row::ShortList.value(&c), "TOP");
        assert_eq!(step(Row::ShortList, &c, 1), t("layout", "short_list", "\"center\""));
        let centred = cfg(|c| c.layout.short_list = ShortList::Center);
        assert_eq!(step(Row::ShortList, &centred, -1), t("layout", "short_list", "\"top\""));
        assert_eq!(Row::Band.value(&c), "MID");
        assert_eq!(step(Row::Band, &c, 1), t("layout", "band_percent", "60"));
        let bottom = cfg(|c| c.layout.band_percent = 100);
        assert_eq!((Row::Band.value(&bottom), step(Row::Band, &bottom, 1)), ("BOTTOM".to_owned(), None));
        let top = cfg(|c| c.layout.band_percent = 0);
        assert_eq!((Row::Band.value(&top), step(Row::Band, &top, -1)), ("TOP".to_owned(), None));
        assert_eq!(Row::Band.value(&cfg(|c| c.layout.band_percent = 30)), "30%");
    }

    #[test]
    fn gui_scale_steps_by_ten_and_contrast_toggles() {
        let c = Config::default();
        assert_eq!((Row::GuiScale.value(&c).as_str(), Row::Contrast.value(&c).as_str()), ("100%", "OFF"));
        assert_eq!(Row::GuiScale.label(), "GUI SCALE");
        assert_eq!(step(Row::GuiScale, &c, -1), t("layout", "gui_scale", "90"));
        assert_eq!(step(Row::GuiScale, &c, 1), t("layout", "gui_scale", "110"));
        let at = |v: u32| cfg(|c| c.layout.gui_scale = v);
        assert_eq!(step(Row::GuiScale, &at(50), -1), None);
        assert_eq!(step(Row::GuiScale, &at(50), 1), t("layout", "gui_scale", "60"));
        assert_eq!(step(Row::GuiScale, &at(250), 1), None);
        assert_eq!(step(Row::GuiScale, &at(250), -1), t("layout", "gui_scale", "240"));
        // The old text_size 115 steps onto the grid.
        assert_eq!((step(Row::GuiScale, &at(115), 1), step(Row::GuiScale, &at(115), -1)), (t("layout", "gui_scale", "120"), t("layout", "gui_scale", "110")));
        assert_eq!(step(Row::Contrast, &c, 1), t("layout", "high_contrast", "true"));
        assert_eq!(step(Row::Contrast, &cfg(|c| c.layout.high_contrast = true), -1), t("layout", "high_contrast", "false"));
    }

    #[test]
    fn colors_tab_lists_theme_then_every_style_key_under_headings() {
        let lines = Tab::Colors.lines();
        assert_eq!(lines[0], Line::Row(Row::Theme));
        let headings: Vec<_> = lines.iter().filter_map(|l| if let Line::Heading(Head::Section(t)) = l { Some(*t) } else { None }).collect();
        assert_eq!(headings, vec!["header", "row", "tag", "power", "panel", "popup", "overlay"]);
        let mut colors: Vec<usize> = lines.iter().filter_map(|l| if let Line::Row(Row::Color(i)) = l { Some(*i) } else { None }).collect();
        colors.sort();
        assert_eq!(colors, (0..style::KEYS.len()).collect::<Vec<_>>(), "each key exactly once");
        assert_eq!(Tab::Colors.rows().len(), 2 + style::KEYS.len());
        assert_eq!(lines[1], Line::Row(Row::SaveTheme), "right under THEME");
        // Headings come right before their own keys.
        let mut section = "";
        for line in &lines {
            match line {
                Line::Heading(Head::Section(t)) => section = t,
                Line::Heading(Head::Plugin(_)) => unreachable!(),
                Line::Row(Row::Color(i)) => assert_eq!(style::KEYS[*i].split('.').next().unwrap(), section),
                Line::Row(_) => {}
            }
        }
        assert_eq!(Row::Color(style::KEYS.iter().position(|&k| k == "row.name_selected").unwrap()).label(), "NAME SELECTED");
    }

    fn color_index(key: &str) -> usize {
        style::KEYS.iter().position(|&k| k == key).unwrap()
    }

    #[test]
    fn selection_scrolls_the_window_and_headings_are_not_selectable() {
        let mut p = Panel::default();
        assert_eq!(p.row_slots(), vec![Some(0), Some(1), None, Some(2), Some(3), Some(4), Some(5)]);
        p.move_selection(2);
        assert_eq!(p.current(), Some(Row::Color(0)), "skips the heading");
        for _ in 0..40 {
            p.move_selection(1);
            let slot = p.row_slots().iter().position(|&r| r == Some(p.selected));
            assert!(slot.is_some(), "selected row {} is in view", p.selected);
        }
        let rows = Tab::Colors.rows().len();
        p.move_selection(1000);
        assert_eq!(p.button(), Some(Button::Style));
        assert_eq!(p.selected, rows + 1);
        p.move_selection(-2);
        let last = p.current().unwrap();
        assert_eq!(last, *Tab::Colors.rows().last().unwrap());
        assert!(p.row_slots().contains(&Some(rows - 1)));
        p.move_selection(-1000);
        assert_eq!((p.selected, p.row_at(0)), (0, Some(0)));
        assert_eq!(p.row_at(2), None, "heading slot");
    }

    #[test]
    fn heading_stays_visible_above_the_first_row_of_a_section() {
        let mut p = Panel::default();
        p.move_selection(40);
        // Walk up to the first row of TAG: its heading must be in the window.
        while p.current() != Some(Row::Color(color_index("tag.app"))) {
            p.move_selection(-1);
        }
        let c = Config::default();
        let view = p.view(&c, &Style::from_theme(&c.colors), false);
        assert!(view.slots.contains(&Slot::Heading("TAG".to_owned())));
        assert!(view.selected.is_some());
    }

    #[test]
    fn wheel_scrolls_without_moving_the_selection_and_clamps() {
        let mut p = Panel::default();
        p.scroll_by(-5);
        assert_eq!(p.row_at(0), Some(0));
        p.scroll_by(1000);
        let lines = Tab::Colors.lines().len();
        let last = p.row_slots();
        assert_eq!(last.len(), SLOTS);
        assert_eq!(last.last().copied().flatten(), Some(Tab::Colors.rows().len() - 1));
        assert_eq!(p.selected, 0);
        let c = Config::default();
        let v = p.view(&c, &Style::from_theme(&c.colors), false);
        assert_eq!(v.selected, None, "selection scrolled out of view");
        let (a, b) = v.scroll.unwrap();
        assert!(a > 0.8 && (b - 1.0).abs() < 1e-6, "{a} {b} of {lines}");
        p.move_selection(0);
        assert!(p.view(&c, &Style::from_theme(&c.colors), false).selected.is_some(), "a key press brings it back");
        let mut short = Panel { tab: Tab::Plugins, ..Default::default() };
        short.scroll_by(5);
        assert!(short.view(&c, &Style::from_theme(&c.colors), false).scroll.is_none());
    }

    #[test]
    fn color_rows_show_swatch_hex_and_whether_they_follow_the_theme() {
        let c = Config::default();
        let default = Style::from_theme(&c.colors);
        let mut custom = default;
        custom.row_border = style::Paint { rgb: 0x112233, alpha: 0.5 };
        let mut p = Panel::default();
        while p.current() != Some(Row::Color(color_index("row.border"))) {
            p.move_selection(1);
        }
        let on_default = selected_row(&p.view(&c, &default, false));
        assert_eq!((on_default.label.as_str(), on_default.value.as_str()), ("BORDER", "#5EF6FF"));
        assert_eq!(on_default.color, Some(ColorCell { rgb: 0x5EF6FF, alpha: 1.0, custom: false, editing: false }));
        let on_custom = selected_row(&p.view(&c, &custom, false));
        assert_eq!(on_custom.value, "#112233");
        assert_eq!(on_custom.color.as_ref().unwrap().alpha, 0.5);
        assert!(p.view(&c, &custom, false).detail.unwrap().contains("OPACITY"));
        assert!(on_custom.color.as_ref().unwrap().custom);
    }

    fn selected_row(view: &PanelView) -> PanelRow {
        match &view.slots[view.selected.expect("selected row in view")] {
            Slot::Row(r) => PanelRow { label: r.label.clone(), value: r.value.clone(), stepper: r.stepper, color: r.color.clone(), dropdown: r.dropdown },
            Slot::Heading(_) => panic!("heading selected"),
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn change_in(row: Row, c: &Config, dir: i32, custom: &[&str], languages: &[&str]) -> Option<(String, String, String)> {
        toml_of(row.change_in(c, dir, &names(custom), &names(languages)))
    }

    #[test]
    fn theme_row_cycles_presets_then_custom_themes_in_order() {
        let custom = ["Alpha", "zeta"];
        let mut c = Config::default();
        let mut seen = Vec::new();
        for _ in 0..6 {
            let (_, _, v) = change_in(Row::Theme, &c, 1, &custom, &[]).unwrap();
            c.theme_name = v.trim_matches('"').to_owned();
            seen.push(c.theme_name.clone());
        }
        assert_eq!(seen, ["arasaka", "militech", "netwatch", "Alpha", "zeta", "cyberpunk"]);
        let on_zeta = cfg(|c| c.theme_name = "zeta".into());
        assert_eq!(change_in(Row::Theme, &on_zeta, -1, &custom, &[]), t("colors", "theme", "\"Alpha\""));
        assert_eq!(change_in(Row::Theme, &Config::default(), -1, &custom, &[]), t("colors", "theme", "\"zeta\""), "wraps backwards onto the last custom theme");
        let deleted = cfg(|c| c.theme_name = "gone".into());
        assert_eq!(change_in(Row::Theme, &deleted, 1, &custom, &[]), t("colors", "theme", "\"arasaka\""), "an unknown theme counts as the first");
    }

    #[test]
    fn language_row_cycles_auto_then_the_available_languages() {
        let langs = ["en", "pl", "de"];
        let auto = Config::default();
        assert_eq!(Row::Language.value(&auto), "AUTO");
        assert_eq!(change_in(Row::Language, &auto, 1, &[], &langs), t("behavior", "language", "\"en\""));
        assert_eq!(change_in(Row::Language, &auto, -1, &[], &langs), t("behavior", "language", "\"de\""));
        let pl = cfg(|c| c.language = "pl".into());
        assert_eq!(Row::Language.value(&pl), "PL");
        assert_eq!(change_in(Row::Language, &pl, 1, &[], &langs), t("behavior", "language", "\"de\""));
        let last = cfg(|c| c.language = "de".into());
        assert_eq!(change_in(Row::Language, &last, 1, &[], &langs), t("behavior", "language", "\"auto\""), "wraps onto AUTO");
        assert_eq!(change_in(Row::Language, &auto, 1, &[], &[]), None, "nothing to pick");
    }

    #[test]
    fn behavior_tab_lists_language_and_look_tab_the_header_texts() {
        assert_eq!(
            Tab::Behavior.rows(),
            vec![Row::Hotkey, Row::FileSearch, Row::FileResults, Row::Recent, Row::ClearHistory, Row::Language, Row::Autostart, Row::Tutorial]
        );
        assert!(Tab::Look.rows().windows(3).any(|w| w == [Row::Title, Row::Subtitle, Row::Show(3)]));
        let c = Config::default();
        assert_eq!((Row::Title.value(&c).as_str(), Row::Subtitle.value(&c).as_str()), ("NETRUNNER_OS // START", "AVAILABLE PROGRAMS:"));
        for row in [Row::Title, Row::Subtitle, Row::SaveTheme] {
            assert!(row.edits() && row.change(&c, 1).is_none() && !row.stepped(), "{row:?}");
        }
    }

    #[test]
    fn autostart_row_shows_the_registry_state_and_writes_no_config() {
        let c = Config::default();
        let st = Style::from_theme(&c.colors);
        let mut p = Panel { tab: Tab::Behavior, selected: 6, autostart: true, ..Default::default() };
        assert_eq!(p.current(), Some(Row::Autostart));
        assert_eq!(selected_row(&p.view(&c, &st, false)).value, "ON");
        p.autostart = false;
        let row = selected_row(&p.view(&c, &st, false));
        assert_eq!((row.value.as_str(), row.stepper), ("OFF", true));
        assert_eq!(step(Row::Autostart, &c, 1), None, "toggled through the registry, not config.toml");
    }

    #[test]
    fn title_rows_start_from_the_current_text_and_cap_the_length() {
        let mut p = Panel { tab: Tab::Look, selected: 1 + style::SHAPE_KEYS.len(), ..Default::default() };
        assert_eq!(p.current(), Some(Row::Title));
        p.move_selection(0);
        p.begin_edit();
        assert_eq!((p.editing.as_deref(), p.edit_kind), (Some("NETRUNNER_OS // START"), EditKind::Text));
        for c in "ąć é".chars() {
            p.edit_char(c);
        }
        p.edit_char('\u{7}');
        assert_eq!(p.editing.as_deref(), Some("NETRUNNER_OS // STARTąć é"), "any printable character, no control characters");
        for _ in 0..60 {
            p.edit_char('x');
        }
        assert_eq!(p.editing.as_ref().unwrap().chars().count(), MAX_TEXT);
        let c = Config::default();
        let view = p.view(&c, &Style::from_theme(&c.colors), false);
        let row = selected_row(&view);
        assert!(row.value.starts_with('\u{2026}') && row.value.ends_with("x_") && row.value.chars().count() == SHOWN + 1, "{}", row.value);
        p.select(2 + style::SHAPE_KEYS.len());
        p.begin_edit();
        assert_eq!(p.editing.as_deref(), Some("AVAILABLE PROGRAMS:"));
        p.editing = Some(String::new());
        assert_eq!(selected_row(&p.view(&c, &Style::from_theme(&c.colors), false)).value, "TYPE TEXT_");
        assert_eq!(p.finish_edit().as_deref(), Some(""));
    }

    #[test]
    fn theme_names_take_only_name_characters_and_stop_at_the_limit() {
        let mut p = Panel { selected: 1, ..Default::default() };
        assert_eq!(p.current(), Some(Row::SaveTheme));
        p.begin_edit();
        assert_eq!((p.editing.as_deref(), p.edit_kind), (Some(""), EditKind::Name));
        let c = Config::default();
        assert_eq!(selected_row(&p.view(&c, &Style::from_theme(&c.colors), false)).value, "TYPE NAME_");
        for ch in "My_Theme-2 a/b.c\\ł".chars() {
            p.edit_char(ch);
        }
        assert_eq!(p.editing.as_deref(), Some("My_Theme-2 abc"));
        for _ in 0..40 {
            p.edit_char('z');
        }
        assert_eq!(p.editing.as_ref().unwrap().chars().count(), themes::MAX_NAME);
    }

    #[test]
    fn saving_a_name_checks_it_and_asks_before_overwriting() {
        let mut p = Panel { selected: 1, themes: names(&["Mine", "other"]), ..Default::default() };
        let typed = |p: &mut Panel, text: &str| {
            p.begin_edit();
            p.editing = Some(text.to_owned());
        };
        typed(&mut p, "  ");
        assert_eq!((p.check_name(), p.flash), (NameCheck::Rejected, Note::EmptyName));
        assert!(p.editing.is_some(), "typing goes on");
        p.flash = Note::None;
        typed(&mut p, "Arasaka");
        assert_eq!((p.check_name(), p.flash), (NameCheck::Rejected, Note::BuiltinName));
        p.flash = Note::None;
        typed(&mut p, "NUL");
        assert_eq!(p.check_name(), NameCheck::Rejected);
        typed(&mut p, "fresh");
        assert_eq!(p.check_name(), NameCheck::Save("fresh".into()));
        assert_eq!((p.editing.as_deref(), p.pending.as_ref()), (None, None));

        typed(&mut p, " mine ");
        assert_eq!(p.check_name(), NameCheck::Confirm);
        assert_eq!(p.pending, Some(Pending::Overwrite("mine".into())));
        let c = Config::default();
        assert_eq!(p.view(&c, &Style::from_theme(&c.colors), false).note, Note::OverwriteTheme);
        p.edit_char('x');
        assert_eq!(p.pending, None, "editing the name withdraws the question");
        p.edit_backspace();
        assert_eq!(p.check_name(), NameCheck::Confirm);
        assert_eq!(p.check_name(), NameCheck::Save("mine".into()), "Enter again confirms");
        typed(&mut p, "Mine");
        assert_eq!(p.check_name(), NameCheck::Confirm);
        p.abort_edit();
        assert_eq!((p.editing.as_deref(), p.pending.as_ref()), (None, None), "Esc cancels everything");
    }

    #[test]
    fn deleting_a_custom_theme_needs_two_presses_and_a_custom_theme() {
        let mut p = Panel::default();
        let preset = Config::default();
        assert_eq!(p.request_delete(&preset), DeleteStep::Nothing, "presets can't be deleted");
        let c = cfg(|c| c.theme_name = "Mine".into());
        assert_eq!(p.request_delete(&c), DeleteStep::Confirm);
        assert_eq!(p.view(&c, &Style::from_theme(&c.colors), false).note, Note::DeleteTheme);
        assert_eq!(p.request_delete(&c), DeleteStep::Remove("Mine".into()));
        assert_eq!(p.pending, None);
        assert_eq!(p.request_delete(&c), DeleteStep::Confirm, "asks again");
        p.move_selection(1);
        assert_eq!(p.pending, None, "moving away withdraws it");
        assert_eq!(p.request_delete(&c), DeleteStep::Nothing, "not on the THEME row");
        p.select(0);
        assert_eq!(p.request_delete(&c), DeleteStep::Confirm);
        let other = cfg(|c| c.theme_name = "Other".into());
        assert_eq!(p.request_delete(&other), DeleteStep::Confirm, "a different theme is a new question");
    }

    #[test]
    fn notes_have_text_and_errors_win_over_questions() {
        let c = Config::default();
        let st = Style::from_theme(&c.colors);
        let mut p = Panel { pending: Some(Pending::Delete("x".into())), flash: Note::ThemeNotSaved, ..Default::default() };
        assert_eq!(p.view(&c, &st, false).note, Note::ThemeNotSaved, "a message beats a question");
        p.style_error = true;
        assert_eq!(p.view(&c, &st, false).note, Note::StyleError);
        for note in [Note::HexOverrides, Note::HotkeyInUse, Note::Error, Note::StyleError, Note::InvalidHex, Note::OverwriteTheme, Note::DeleteTheme, Note::BuiltinName, Note::EmptyName, Note::ThemeNotSaved, Note::AutostartFailed, Note::KeywordInUse, Note::KeywordInvalid, Note::ApprovePlugin, Note::LogFailed, Note::ClearHistory, Note::HistoryCleared] {
            assert!(lang::find(note.id().unwrap()).is_some(), "{note:?}");
        }
        assert_eq!(Note::None.id(), None);
    }

    #[test]
    fn typing_a_hex_color_accepts_a_hash_then_digits_and_stops_at_eight() {
        let mut p = Panel::default();
        p.begin_edit();
        assert_eq!(p.editing, None, "THEME is not a color row");
        p.move_selection(2);
        p.begin_edit();
        for c in "#12G34zz5678901".chars() {
            p.edit_char(c);
        }
        assert_eq!(p.editing.as_deref(), Some("#12345678"));
        p.edit_char('#');
        p.edit_backspace();
        assert_eq!(p.editing.as_deref(), Some("#1234567"));
        let c = Config::default();
        let row = selected_row(&p.view(&c, &Style::from_theme(&c.colors), false));
        assert_eq!(row.value, "#1234567_");
        assert!(row.color.unwrap().editing);
        assert_eq!(p.finish_edit().as_deref(), Some("#1234567"));
        assert_eq!(p.editing, None);
        p.begin_edit();
        p.move_selection(1);
        assert_eq!(p.editing, None, "moving away cancels");
    }

    fn info(id: &str) -> PluginInfo {
        PluginInfo {
            id: id.to_owned(),
            name: format!("{id} plugin"),
            version: "1.2.0".to_owned(),
            author: "Jane".to_owned(),
            run: "python main.py".to_owned(),
            state: PluginState::Running,
            enabled: true,
            approved: true,
            keywords: vec![id[..2].to_owned()],
            ..PluginInfo::default()
        }
    }

    fn setting(key: &str, kind: SettingKind, value: SettingValue) -> SettingInfo {
        SettingInfo { key: key.to_owned(), label: format!("{key} label"), kind, value }
    }

    fn plugins_panel(list: Vec<PluginInfo>) -> Panel {
        let mut p = Panel { tab: Tab::Plugins, ..Default::default() };
        p.set_plugins(true, list);
        p
    }

    /// Moves the selection onto `row`.
    fn goto(p: &mut Panel, row: Row) {
        let at = p.rows().iter().position(|&r| r == row).unwrap_or_else(|| panic!("{row:?} is not listed"));
        p.select(at);
    }

    fn write_text(write: Option<PluginWrite>) -> String {
        match write {
            Some(PluginWrite::Value { id, key, value }) => format!("[{id}] {key} = {}", value.to_string().trim()),
            Some(PluginWrite::Remove { id, key }) => format!("[{id}] remove {key}"),
            Some(PluginWrite::Setting { id, key, value }) => format!("[{id}.settings] {key} = {}", value.to_string().trim()),
            None => "nothing".to_owned(),
        }
    }

    #[test]
    fn plugins_tab_lists_master_folder_then_a_section_per_plugin() {
        let mut gh = info("github");
        gh.can_global = true;
        gh.settings = vec![
            setting("user", SettingKind::String, SettingValue::String("jane".into())),
            setting("forks", SettingKind::Bool, SettingValue::Bool(false)),
            setting("sort", SettingKind::Choice(vec!["updated".into(), "stars".into()]), SettingValue::String("updated".into())),
        ];
        let mut raw = info("raw");
        raw.approved = false;
        raw.state = PluginState::NotApproved;
        let broken = PluginInfo { id: "bad".into(), name: "bad".into(), state: PluginState::ManifestError("id must equal the folder name".into()), ..PluginInfo::default() };
        let p = plugins_panel(vec![broken, gh, raw]);
        let field = |i, f| Row::Plugin(i, f);
        use PluginField::*;
        assert_eq!(
            p.rows(),
            vec![
                Row::PluginsMaster,
                Row::PluginsFolder,
                field(0, State),
                field(1, State),
                field(1, Enabled),
                field(1, Keyword),
                field(1, Global),
                field(1, Setting(0)),
                field(1, Setting(1)),
                field(1, Setting(2)),
                field(1, Log),
                field(2, State),
                field(2, Enabled),
                field(2, Approve),
                field(2, Keyword),
                field(2, Log),
            ],
            "a broken manifest has only its state; GLOBAL needs the manifest's permission; APPROVE only until approved"
        );
        let headings = p.lines().iter().filter(|l| matches!(l, Line::Heading(_))).count();
        assert_eq!(headings, 3);
        assert_eq!(p.plugin_heading(1), "GITHUB PLUGIN V1.2.0 - JANE");
    }

    #[test]
    fn no_plugins_says_so_and_plugins_off_lists_nothing() {
        let mut p = plugins_panel(Vec::new());
        assert_eq!(p.lines().last(), Some(&Line::Heading(Head::Section("plugins_none"))));
        p.set_plugins(false, Vec::new());
        assert_eq!(p.rows(), vec![Row::PluginsMaster, Row::PluginsFolder]);
        assert!(!p.lines().iter().any(|l| matches!(l, Line::Heading(_))));
        let on = Config::default();
        assert_eq!(step(Row::PluginsMaster, &on, 1), t("behavior", "plugins", "false"));
        let off = cfg(|c| c.plugins.enabled = false);
        assert_eq!((Row::PluginsMaster.value(&off).as_str(), step(Row::PluginsMaster, &off, -1)), ("OFF", t("behavior", "plugins", "true")));
    }

    #[test]
    fn state_rows_show_the_state_and_the_error_text() {
        let broken = PluginInfo { id: "bad".into(), name: "bad".into(), state: PluginState::ManifestError("run.file \"x.py\" not found".into()), ..PluginInfo::default() };
        let mut failed = info("fail");
        failed.state = PluginState::Failed;
        let mut p = plugins_panel(vec![broken, failed]);
        let (c, st) = (Config::default(), Style::from_theme(&Config::default().colors));
        goto(&mut p, Row::Plugin(0, PluginField::State));
        let view = p.view(&c, &st, false);
        assert_eq!(selected_row(&view).value, "ERROR");
        assert_eq!(view.detail.as_deref(), Some("run.file \"x.py\" not found"));
        assert_eq!(view.slots[2], Slot::Heading("BAD".to_owned()));
        goto(&mut p, Row::Plugin(1, PluginField::State));
        let view = p.view(&c, &st, false);
        assert_eq!(selected_row(&view).value, "FAILED");
        assert!(view.detail.unwrap().contains("ENTER"));
        for (state, word) in [
            (PluginState::Running, "RUNNING"),
            (PluginState::Starting, "STARTING"),
            (PluginState::Stopped, "STOPPED"),
            (PluginState::Disabled, "DISABLED"),
            (PluginState::NotApproved, "NOT APPROVED"),
        ] {
            let mut one = info("one");
            one.state = state;
            let mut p = plugins_panel(vec![one]);
            goto(&mut p, Row::Plugin(0, PluginField::State));
            assert_eq!(selected_row(&p.view(&c, &st, false)).value, word);
        }
    }

    #[test]
    fn enabled_and_global_toggle_under_the_plugins_table() {
        let mut gh = info("github");
        gh.can_global = true;
        let mut p = plugins_panel(vec![gh]);
        goto(&mut p, Row::Plugin(0, PluginField::Enabled));
        assert_eq!(write_text(p.plugin_step(1)), "[github] enabled = false");
        assert_eq!(write_text(p.plugin_step(-1)), "[github] enabled = false", "a toggle either way");
        goto(&mut p, Row::Plugin(0, PluginField::Global));
        assert_eq!(write_text(p.plugin_step(1)), "[github] global = true");
        let (c, st) = (Config::default(), Style::from_theme(&Config::default().colors));
        assert_eq!(p.view(&c, &st, false).detail, None, "no privacy note while it is off");
        p.plugins[0].global = true;
        let view = p.view(&c, &st, false);
        assert_eq!(selected_row(&view).value, "ON");
        assert_eq!(view.detail.as_deref(), Some("EVERY QUERY YOU TYPE IS SENT TO THIS PLUGIN"));
        goto(&mut p, Row::Plugin(0, PluginField::Log));
        assert_eq!(write_text(p.plugin_step(1)), "nothing");
    }

    #[test]
    fn a_keyword_taken_by_another_plugin_is_flagged_and_not_saved() {
        let mut p = plugins_panel(vec![info("github"), info("gist")]);
        goto(&mut p, Row::Plugin(0, PluginField::Keyword));
        p.begin_edit();
        assert_eq!((p.edit_kind, p.editing.as_deref()), (EditKind::Keyword, Some("gi")));
        for c in " GIst!".chars() {
            p.edit_char(c);
        }
        assert_eq!(p.editing.as_deref(), Some("gigist"), "lower case letters, digits, _ and - only");
        p.editing = Some("gi".into());
        let conflict = write_text(p.plugin_keyword());
        assert_eq!(conflict, "nothing", "gist also answers to gi");
        assert_eq!((p.flash, p.editing.as_deref()), (Note::KeywordInUse, Some("gi")), "typing goes on");
        assert_eq!(p.view(&Config::default(), &Style::from_theme(&Config::default().colors), false).note, Note::KeywordInUse);
        p.flash = Note::None;
        p.editing = Some("a b".into());
        assert_eq!(write_text(p.plugin_keyword()), "nothing");
        assert_eq!(p.flash, Note::KeywordInvalid);
        p.editing = Some("hub".into());
        assert_eq!(write_text(p.plugin_keyword()), "[github] keyword = \"hub\"");
        assert_eq!(p.editing, None);
        p.begin_edit();
        p.editing = Some("  ".into());
        assert_eq!(write_text(p.plugin_keyword()), "[github] remove keyword", "empty = back to the manifest's keyword");
        assert_eq!(write_text(p.plugin_keyword_reset()), "[github] remove keyword", "Delete does the same");
    }

    #[test]
    fn a_keyword_lost_to_another_plugin_shows_a_mark_and_a_note() {
        let mut dup = info("github");
        dup.keyword_lost = true;
        let mut p = plugins_panel(vec![dup]);
        goto(&mut p, Row::Plugin(0, PluginField::Keyword));
        let (c, st) = (Config::default(), Style::from_theme(&Config::default().colors));
        let view = p.view(&c, &st, false);
        assert_eq!(selected_row(&view).value, "gi !");
        assert!(view.detail.unwrap().contains("ANOTHER PLUGIN"));
    }

    #[test]
    fn settings_toggle_step_and_are_typed_into_the_settings_table() {
        let mut gh = info("github");
        gh.settings = vec![
            setting("user", SettingKind::String, SettingValue::String("jane".into())),
            setting("forks", SettingKind::Bool, SettingValue::Bool(false)),
            setting("sort", SettingKind::Choice(vec!["updated".into(), "stars".into(), "name".into()]), SettingValue::String("updated".into())),
        ];
        let mut p = plugins_panel(vec![gh]);
        let (c, st) = (Config::default(), Style::from_theme(&Config::default().colors));
        goto(&mut p, Row::Plugin(0, PluginField::Setting(1)));
        assert_eq!(write_text(p.plugin_step(1)), "[github.settings] forks = true");
        let row = selected_row(&p.view(&c, &st, false));
        assert_eq!((row.label.as_str(), row.value.as_str(), row.stepper), ("FORKS LABEL", "OFF", true));
        goto(&mut p, Row::Plugin(0, PluginField::Setting(2)));
        assert_eq!(write_text(p.plugin_step(1)), "[github.settings] sort = \"stars\"");
        assert_eq!(write_text(p.plugin_step(-1)), "[github.settings] sort = \"name\"", "wraps backwards");
        assert_eq!(selected_row(&p.view(&c, &st, false)).value, "UPDATED");
        goto(&mut p, Row::Plugin(0, PluginField::Setting(0)));
        assert!(p.edits() && p.edits_row(p.selected) && !p.edits_row(p.selected + 1));
        assert!(!selected_row(&p.view(&c, &st, false)).stepper, "a string is typed, not stepped");
        assert_eq!(write_text(p.plugin_step(1)), "nothing");
        p.begin_edit();
        assert_eq!((p.edit_kind, p.editing.as_deref()), (EditKind::Setting, Some("jane")));
        for ch in " the 2nd".chars() {
            p.edit_char(ch);
        }
        assert_eq!(write_text(p.plugin_text()), "[github.settings] user = \"jane the 2nd\"");
        assert_eq!(p.editing, None);
        for _ in 0..200 {
            p.begin_edit();
            p.edit_char('x');
        }
        p.begin_edit();
        for _ in 0..200 {
            p.edit_char('y');
        }
        assert_eq!(p.editing.as_ref().unwrap().chars().count(), MAX_SETTING);
    }

    #[test]
    fn plugin_writes_land_in_the_plugins_tables_of_config_toml() {
        let text = crate::config::set_in_path("", &["plugins", "github", "settings"], "forks", Value::from(true)).unwrap();
        let text = crate::config::set_in_path(&text, &["plugins", "github"], "keyword", Value::from("hub")).unwrap();
        let entry = &crate::config::parse(&text).plugins.entries["github"];
        assert_eq!((entry.keyword.as_deref(), entry.settings["forks"].clone()), (Some("hub"), SettingValue::Bool(true)));
    }

    #[test]
    fn approving_asks_first_and_only_the_same_row_confirms() {
        let mut raw = info("raw");
        raw.approved = false;
        let mut p = plugins_panel(vec![raw]);
        goto(&mut p, Row::Plugin(0, PluginField::Approve));
        let (c, st) = (Config::default(), Style::from_theme(&Config::default().colors));
        assert_eq!(p.view(&c, &st, false).detail.as_deref(), Some("RUNS AS YOU: python main.py"));
        assert_eq!(p.plugin_approve(), None);
        assert_eq!(p.view(&c, &st, false).note, Note::ApprovePlugin);
        p.select(p.selected);
        assert_eq!(p.plugin_approve(), Some("raw".to_owned()), "the second press on the same row approves");
        assert_eq!(p.pending, None);
        assert_eq!(p.plugin_approve(), None, "and asks again");
        p.move_selection(1);
        assert_eq!(p.pending, None, "moving away withdraws the question");
    }

    #[test]
    fn recent_row_writes_behavior_recent_and_clear_history_asks_twice() {
        assert_eq!(step(Row::Recent, &Config::default(), 1), t("behavior", "recent", "false"));
        assert_eq!(step(Row::Recent, &cfg(|c| c.recent = false), -1), t("behavior", "recent", "true"));
        assert_eq!((Row::Recent.value(&Config::default()).as_str(), Row::Recent.value(&cfg(|c| c.recent = false)).as_str()), ("ON", "OFF"));
        assert!(Row::ClearHistory.presses() && !Row::ClearHistory.stepped() && step(Row::ClearHistory, &Config::default(), 1).is_none());
        let c = Config::default();
        let mut p = Panel { tab: Tab::Behavior, selected: 4, ..Default::default() };
        assert_eq!(p.current(), Some(Row::ClearHistory));
        assert!(!p.clear_history(), "the first press only asks");
        assert_eq!(p.view(&c, &Style::from_theme(&c.colors), false).note, Note::ClearHistory);
        p.move_selection(1);
        p.move_selection(-1);
        assert!(!p.clear_history(), "moving away withdrew the question");
        assert!(p.clear_history(), "the second press on the same row clears");
        assert_eq!((p.pending.clone(), p.view(&c, &Style::from_theme(&c.colors), false).note), (None, Note::HistoryCleared));
        p.move_selection(-1);
        assert!(!p.clear_history() && p.pending.is_none(), "other rows ignore it");
    }

    #[test]
    fn button_rows_press_and_the_rest_do_not() {
        for row in [Row::PluginsFolder, Row::Plugin(0, PluginField::Log), Row::Plugin(0, PluginField::Approve), Row::Plugin(0, PluginField::State)] {
            assert!(row.presses() && !row.stepped(), "{row:?}");
        }
        assert!(Row::PluginsMaster.stepped() && Row::Plugin(0, PluginField::Enabled).stepped() && !Row::PluginsMaster.presses());
        assert!(!Row::Plugin(0, PluginField::Keyword).stepped() && Row::Plugin(0, PluginField::Keyword).edits());
        assert_eq!(Row::PluginsFolder.value(&Config::default()), "ENTER");
    }

    #[test]
    fn a_refresh_keeps_the_selection_on_a_row_that_still_exists() {
        let mut p = plugins_panel(vec![info("github"), info("gist")]);
        let last = p.rows().len() - 1;
        p.select(last);
        p.set_plugins(true, vec![info("github")]);
        assert_eq!(p.selected, p.rows().len() - 1, "clamped when the list shrank");
        p.set_plugins(true, Vec::new());
        assert!(p.selected < p.rows().len() + BUTTONS);
        assert!(p.row_slots().iter().any(|r| r == &Some(p.selected)) || p.button().is_some());
    }

    #[test]
    fn pop_out_placement_rows_exist_only_in_pop_out_mode_and_the_selection_follows_the_row() {
        let list = Config::default();
        let popout = cfg(|c| c.layout.actions = ActionsMode::Popout);
        let mut p = Panel { tab: Tab::Layout, ..Default::default() };
        let rows = |p: &Panel| p.rows();
        assert_eq!(rows(&p).len(), 10);
        assert!(!rows(&p).iter().any(|r| matches!(r, Row::ActionsAnchor | Row::ActionsX | Row::ActionsY)));
        assert!(rows(&p).contains(&Row::Band), "BAND depends on the list overflowing, not on SHORT LIST");
        goto(&mut p, Row::Actions);
        p.sync_config(&popout);
        assert_eq!((rows(&p).len(), p.current()), (13, Some(Row::Actions)), "the changed row stays selected");
        goto(&mut p, Row::ActionsY);
        p.sync_config(&list);
        assert_eq!(p.current(), Some(Row::Actions), "a vanished row leaves the selection on the last one");
        assert!(p.row_slots().contains(&Some(p.selected)));
        p.move_selection(5);
        assert_eq!(p.button(), Some(Button::Style));
        p.sync_config(&popout);
        assert_eq!(p.button(), Some(Button::Style), "a button stays on its button");
        p.sync_config(&popout);
        assert_eq!(p.rows().len(), 13, "no change when nothing differs");
        let mut other = Panel { tab: Tab::Behavior, selected: 3, ..Default::default() };
        other.sync_config(&popout);
        assert_eq!(other.current(), Some(Row::Recent), "other tabs don't move");
    }

    #[test]
    fn tutorial_row_is_a_button_that_restarts_the_wizard() {
        assert!(Row::Tutorial.presses() && !Row::Tutorial.stepped() && step(Row::Tutorial, &Config::default(), 1).is_none());
        assert_eq!((Row::Tutorial.label().as_str(), Row::Tutorial.value(&Config::default()).as_str()), ("RUN TUTORIAL", "ENTER"));
        let mut p = Panel { tab: Tab::Behavior, selected: 7, autostart: false, ..Default::default() };
        assert_eq!(p.current(), Some(Row::Tutorial));
        p.start_wizard(false);
        let w = p.wizard.clone().unwrap();
        assert_eq!((w.step, w.autostart, w.first_run, p.selected), (0, false, false, 0), "starts from the registry's state");
        p.start_wizard(true);
        assert!(p.wizard.as_ref().unwrap().autostart, "a first run proposes ON");
    }

    #[test]
    fn wizard_walks_five_steps_back_and_forth_and_finishes_on_the_last() {
        let mut p = Panel::default();
        p.start_wizard(true);
        let at = |p: &Panel| p.wizard.as_ref().unwrap().step();
        assert_eq!(at(&p), Step::Language);
        assert!(!p.wizard_back(), "nothing before the first step");
        let mut seen = vec![at(&p)];
        for _ in 0..4 {
            assert_eq!(p.wizard_next(), Advance::Moved);
            seen.push(at(&p));
        }
        assert_eq!(seen, [Step::Language, Step::Hotkey, Step::Layout, Step::Theme, Step::Autostart]);
        assert!(p.wizard.as_ref().unwrap().is_last());
        assert_eq!(p.wizard_next(), Advance::Finished);
        assert_eq!(at(&p), Step::Autostart, "finishing doesn't move on");
        assert!(p.wizard_back() && at(&p) == Step::Theme);
        // Only the HOTKEY step captures keys.
        for (step, captures) in [(Step::Language, false), (Step::Hotkey, true), (Step::Layout, false)] {
            p.wizard.as_mut().unwrap().step = STEPS.iter().position(|&s| s == step).unwrap();
            assert_eq!(p.wizard.as_ref().unwrap().captures(), captures, "{step:?}");
        }
    }

    #[test]
    fn wizard_rows_are_the_settings_rows_and_a_step_starts_on_its_first() {
        let mut p = Panel::default();
        p.start_wizard(true);
        assert_eq!(p.rows(), vec![Row::Language]);
        p.wizard_next();
        p.wizard_next();
        assert_eq!(p.rows(), vec![Row::Side, Row::Actions]);
        p.move_selection(5);
        assert_eq!((p.selected, p.button()), (1, None), "no buttons under the wizard's rows");
        p.wizard_next();
        assert_eq!((p.rows(), p.selected), (vec![Row::Theme], 0));
        assert!(p.change(&Config::default(), 1).is_some(), "the THEME row steps through the panel's own code");
        p.wizard_back();
        assert_eq!((p.selected, p.row_slots()), (0, vec![Some(0), Some(1)]));
        assert_eq!(p.change(&Config::default(), 1).map(|(t, k, _)| (t, k)), Some(("layout", "side")));
    }

    #[test]
    fn wizard_view_follows_the_language_and_shows_the_capture_prompt_and_default_autostart() {
        let c = Config::default();
        let st = Style::from_theme(&c.colors);
        let mut p = Panel::default();
        p.start_wizard(true);
        let view = p.view(&c, &st, false);
        let w = view.wizard.as_ref().unwrap();
        assert_eq!((w.title.as_str(), w.heading.as_str(), w.next.as_str(), w.can_back), ("STEP 1/5", "LANGUAGE", "NEXT", false));
        assert_eq!(selected_row(&view).value, "AUTO");
        lang::use_builtin("pl");
        let view = p.view(&c, &st, false);
        let w = view.wizard.as_ref().unwrap();
        assert_eq!((w.title.as_str(), w.heading.as_str(), w.next.as_str()), ("KROK 1/5", "JĘZYK", "DALEJ"));
        assert_eq!(w.text.first().map(String::as_str), Some("WYBIERZ JĘZYK MENU."));
        lang::use_builtin("en");
        p.wizard_next();
        let hotkey = Config::default();
        assert_eq!(selected_row(&p.view(&hotkey, &st, false)).value, "WIN", "the current hotkey is shown");
        p.capturing = true;
        assert_eq!(selected_row(&p.view(&hotkey, &st, false)).value, "PRESS KEYS...");
        assert_eq!(p.view(&hotkey, &st, true).note, Note::HotkeyInUse);
        p.wizard_next();
        p.wizard_next();
        p.wizard_next();
        let last = p.view(&c, &st, false);
        let (row, w) = (selected_row(&last), last.wizard.as_ref().unwrap());
        assert_eq!((row.label.as_str(), row.value.as_str(), row.stepper), ("AUTOSTART", "ON", true));
        assert_eq!((w.title.as_str(), w.next.as_str(), w.can_back), ("STEP 5/5", "FINISH", true));
        assert!(last.scroll.is_none() && last.button.is_none());
    }

    #[test]
    fn wizard_hotkey_step_keeps_capturing_after_a_key_and_shows_it() {
        let c = cfg(|c| c.hotkey = Hotkey::parse("ctrl+alt+space").unwrap());
        let st = Style::from_theme(&c.colors);
        let mut p = Panel::default();
        p.start_wizard(true);
        p.wizard_next();
        p.begin_capture();
        assert_eq!(selected_row(&p.view(&c, &st, false)).value, "PRESS KEYS...");
        p.capture_done();
        assert!(p.capturing, "the new key must not go live while the step is up: pressing it would hide the wizard");
        assert_eq!(selected_row(&p.view(&c, &st, false)).value, "CTRL+ALT+SPACE", "the step shows what was taken");
        p.begin_capture();
        assert_eq!(selected_row(&p.view(&c, &st, false)).value, "PRESS KEYS...", "clicking the row asks again");
        let mut settings = Panel { capturing: true, ..Default::default() };
        settings.capture_done();
        assert!(!settings.capturing, "the settings panel stops after one key");
    }

    #[test]
    fn the_plugins_tab_scrolls_with_many_rows() {
        let mut many = Vec::new();
        for n in 0..6 {
            let mut p = info(&format!("pl{n}"));
            p.settings = vec![setting("a", SettingKind::Bool, SettingValue::Bool(true))];
            many.push(p);
        }
        let mut p = plugins_panel(many);
        for _ in 0..200 {
            p.move_selection(1);
            assert!(p.button().is_some() || p.row_slots().contains(&Some(p.selected)), "row {} in view", p.selected);
        }
        let (c, st) = (Config::default(), Style::from_theme(&Config::default().colors));
        assert!(p.view(&c, &st, false).scroll.is_some());
    }

    fn font_panel() -> Panel {
        let fonts = ["Arial", "Bahnschrift", "Consolas", "Segoe UI"].map(String::from).to_vec();
        Panel { tab: Tab::Look, fonts, ..Default::default() }
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

    fn key(p: &mut Panel, fonts: &Fonts, key: Key) -> Outcome {
        p.dropdown_key(key, fonts)
    }

    #[test]
    fn enter_on_a_font_row_opens_its_dropdown_and_a_pick_writes_the_right_key() {
        let mut p = font_panel();
        let fonts = Fonts::default();
        goto(&mut p, Row::FontFamily);
        assert!(p.current().is_some_and(Row::opens_dropdown) && !p.edits(), "no typing into the row any more");
        assert!(!p.typing());
        p.open_dropdown(&fonts);
        assert!(p.typing() && p.dropdown.is_some());
        assert_eq!(p.drop_geom().map(|g| g.role), Some(false));
        // Opens on Bahnschrift, the family in use.
        key(&mut p, &fonts, Key::Down);
        assert_eq!(written(key(&mut p, &fonts, Key::Enter)), pair("family", "\"Consolas\""));
        // Esc leaves it without a write.
        p.open_dropdown(&fonts);
        key(&mut p, &fonts, Key::Down);
        assert_eq!(written(key(&mut p, &fonts, Key::Esc)), None);
        p.close_dropdown();
        assert!(!p.typing());

        // A role: GLOBAL on top, SIZE and WEIGHT above the list.
        goto(&mut p, Row::FontRole(Role::Tag));
        p.open_dropdown(&fonts);
        assert_eq!(p.drop_geom().map(|g| g.role), Some(true));
        assert_eq!(written(key(&mut p, &fonts, Key::Enter)), pair("tag.family", "remove"), "GLOBAL clears the family");
        key(&mut p, &fonts, Key::Up);
        assert_eq!(written(key(&mut p, &fonts, Key::Right)), pair("tag.weight", "\"light\""));
        key(&mut p, &fonts, Key::Up);
        assert_eq!(written(key(&mut p, &fonts, Key::Right)), pair("tag.size", "105"));
        assert_eq!(written(key(&mut p, &fonts, Key::Left)), pair("tag.size", "95"));
        // Typing filters, and the first match is the pick.
        for c in "seg".chars() {
            p.dropdown_char(c);
        }
        assert_eq!(written(key(&mut p, &fonts, Key::Enter)), pair("tag.family", "\"Segoe UI\""));
        assert_eq!(written(p.dropdown_click(0)), pair("tag.family", "\"Segoe UI\""), "a click on the first entry picks it too");
        // The steps never reach the config.
        assert!(Row::FontRole(Role::Tag).change(&Config::default(), 1).is_none() && Row::FontFamily.change(&Config::default(), 1).is_none());
    }

    #[test]
    fn delete_on_a_font_row_names_the_keys_to_reset() {
        let mut p = font_panel();
        goto(&mut p, Row::FontFamily);
        assert_eq!(p.font_resets(), ["family"]);
        goto(&mut p, Row::FontRole(Role::Hint));
        assert_eq!(p.font_resets(), ["hint.family", "hint.size", "hint.weight"], "a role goes back whole");
        goto(&mut p, Row::GuiScale);
        assert!(p.font_resets().is_empty());
    }

    #[test]
    fn font_rows_read_family_size_and_weight() {
        let mut fonts = Fonts::default();
        assert_eq!(font_row_text(&fonts, None), "BAHNSCHRIFT");
        assert_eq!(font_row_text(&fonts, Some(Role::Name)), "GLOBAL \u{B7} 100%");
        fonts.roles[2].size = 120;
        fonts.roles[2].family = style::Family::new("Consolas");
        fonts.roles[2].weight = Some(style::Weight::Bold);
        assert_eq!(font_row_text(&fonts, Some(Role::Name)), "CONSOLAS \u{B7} 120% \u{B7} BOLD");
        fonts.roles[2].family = style::Family::new("A Very Long Family Name Indeed");
        assert_eq!(font_row_text(&fonts, Some(Role::Name)), "A VERY LONG FAM\u{2026} \u{B7} 120% \u{B7} BOLD");
        lang::use_builtin("pl");
        fonts.roles[2].family = None;
        fonts.roles[2].weight = None;
        fonts.roles[2].size = 100;
        assert_eq!(font_row_text(&fonts, Some(Role::Name)), "GLOBALNA \u{B7} 100%");
        lang::use_builtin("en");
    }

    #[test]
    fn look_tab_font_rows_show_the_fonts_in_use() {
        let c = Config::default();
        let mut style = Style::from_theme(&c.colors);
        style.fonts.family = style::Family::new("Segoe UI").unwrap();
        style.fonts.roles[2].size = 120;
        style.fonts.roles[2].weight = Some(style::Weight::Light);
        style.fonts.roles[2].family = style::Family::new("Consolas");
        // Scrolled to the FONTS heading.
        let p = Panel { tab: Tab::Look, top: 33, ..Default::default() };
        let row = |v: &PanelView, i: usize| match &v.slots[i] {
            Slot::Row(r) => (r.label.clone(), r.value.clone(), r.stepper, r.dropdown),
            other => panic!("{other:?}"),
        };
        let v = p.view(&c, &style, false);
        assert_eq!(v.slots[0], Slot::Heading("FONTS".to_owned()));
        assert_eq!(row(&v, 1), ("FONT".to_owned(), "SEGOE UI".to_owned(), false, true));
        assert_eq!(row(&v, 2), ("HEADER".to_owned(), "GLOBAL \u{B7} 100%".to_owned(), false, true));
        assert_eq!(row(&v, 4), ("NAME".to_owned(), "CONSOLAS \u{B7} 120% \u{B7} LIGHT".to_owned(), false, true));
        assert!(v.dropdown.is_none());
        // Every text role has its own row after the global one, and a name in both languages' files.
        let rows = Tab::Look.rows();
        let at = rows.iter().position(|&r| r == Row::FontFamily).unwrap();
        assert_eq!(&rows[at + 1..], &ROLES.map(Row::FontRole));
        assert_eq!(ROLES.len(), 12);
        for code in ["en", "pl"] {
            lang::use_builtin(code);
            for role in ROLES {
                assert!(lang::find(&format!("settings.fontrole.{}", role.key())).is_some(), "{code} {role:?}");
            }
            for id in ["default", "light", "normal", "semibold", "bold"] {
                assert!(lang::find(&format!("settings.weight.{id}")).is_some(), "{code} {id}");
            }
            for id in ["dropdown.size", "dropdown.weight", "dropdown.filter", "dropdown.none", "detail.dropdown", "detail.dropdown_role", "detail.font_keys"] {
                assert!(lang::find(&format!("settings.{id}")).is_some(), "{code} {id}");
            }
        }
        lang::use_builtin("en");
    }

    #[test]
    fn the_open_dropdown_is_in_the_view_and_hangs_from_the_selected_rows_slot() {
        let c = Config::default();
        let style = Style::from_theme(&c.colors);
        let mut p = font_panel();
        goto(&mut p, Row::FontRole(Role::Name));
        p.open_dropdown(&style.fonts);
        let v = p.view(&c, &style, false);
        let dropdown = v.dropdown.expect("open");
        assert_eq!(Some(dropdown.slot), v.selected);
        assert!(dropdown.role && dropdown.choices.len() == 5, "GLOBAL and four families");
        assert_eq!(v.detail.as_deref(), Some("\u{25C2} \u{25B8} SIZE, WEIGHT   ENTER OK   ESC CLOSE"));
        p.close_dropdown();
        assert!(p.view(&c, &style, false).dropdown.is_none());
        let wizard = Panel { wizard: Some(Wizard::new(false)), dropdown: p.dropdown.clone(), ..Default::default() };
        assert!(wizard.view(&c, &style, false).dropdown.is_none());
    }

    #[test]
    fn the_header_toggles_hang_off_one_row_under_header_and_flip_without_closing() {
        let rows = Tab::Look.rows();
        let at = rows.iter().position(|&r| r == Row::HeaderToggles).unwrap();
        assert_eq!(rows[at - 1], Row::Show(5), "after the header's alignments");
        for i in style::SHOW_HEADER {
            assert!(!rows.contains(&Row::Show(i)), "{} has no row of its own", style::SHOW_KEYS[i]);
        }
        assert_eq!(style::SHOW_HEADER.map(|i| style::SHOW_KEYS[i]).collect::<Vec<_>>(), ["caption", "subtitle", "header_line", "gear", "count", "progress", "header_backdrop"]);
        let mut p = Panel { tab: Tab::Look, ..Default::default() };
        goto(&mut p, Row::HeaderToggles);
        assert!(!p.typing());
        p.open_dropdown(&Fonts::default());
        assert!(p.typing() && p.toggles.is_some() && p.dropdown.is_none());
        let show = Show::default();
        assert!(p.toggles_key(Key::Down, &show).is_none(), "moving writes nothing");
        assert_eq!(p.toggles_key(Key::Enter, &show).map(|(k, v)| (k, v.as_bool())), Some(("subtitle", Some(false))));
        assert!(p.toggles.is_some(), "it stays open");
        assert_eq!(p.toggles_click(6, &show).map(|(k, v)| (k, v.as_bool())), Some(("header_backdrop", Some(false))));
        assert!(p.toggles_click(7, &show).is_none());
        // The view lists the six with their state.
        let style = Style::from_theme(&Config::default().colors);
        let view = p.view(&Config::default(), &style, false).toggles.unwrap();
        assert_eq!((view.entries.len(), view.focus), (7, 6));
        assert_eq!(view.entries[0], togglemenu::Entry { label: "TITLE".into(), on: true });
        assert!(p.toggles_key(Key::Esc, &show).is_none());
        assert!(p.toggles.is_none() && !p.typing());
    }

    #[test]
    fn minimalism_rows_are_in_look_with_text_in_both_languages() {
        assert_eq!(TABS.len(), 5, "no tab of its own");
        let lines = Tab::Look.lines();
        for section in ["dimming", "elements"] {
            assert_eq!(lines.iter().filter(|l| matches!(l, Line::Heading(Head::Section(s)) if *s == section)).count(), 1, "{section}");
        }
        for code in ["en", "pl"] {
            lang::use_builtin(code);
            for key in style::SHOW_KEYS {
                assert!(lang::find(&format!("settings.show.{key}")).is_some(), "{code} {key}");
            }
            for key in style::SHAPE_KEYS {
                assert!(lang::find(&format!("settings.shape.{key}")).is_some(), "{code} {key}");
            }
            for id in ["left", "center", "right", "on", "off"] {
                assert!(lang::find(&format!("settings.value.{id}")).is_some(), "{code} {id}");
            }
            for section in ["dimming", "elements", "header"] {
                assert!(lang::find(&format!("settings.section.{section}")).is_some(), "{code} {section}");
            }
        }
        lang::use_builtin("pl");
        lang::use_builtin("en");
        assert_eq!(Row::Show(0).label(), "COLOR ICONS");
    }

    #[test]
    fn show_rows_toggle_and_cycle() {
        let d = Show::default();
        assert_eq!(show_value(&d, "glow"), "ON");
        assert_eq!(show_value(&d, "color_icons"), "OFF");
        assert_eq!(show_value(&d, "count_align"), "RIGHT");
        let glow = style::SHOW_KEYS.iter().position(|&k| k == "glow").unwrap();
        let (key, value) = Row::Show(glow).show_change(&d, 1).unwrap();
        assert_eq!((key, value.as_bool()), ("glow", Some(false)));
        let title = style::SHOW_KEYS.iter().position(|&k| k == "title_align").unwrap();
        assert_eq!(Row::Show(title).show_change(&d, 1).unwrap().1.as_str(), Some("center"));
        assert!(Row::Side.show_change(&d, 1).is_none());
    }
}
