//! User settings in `%APPDATA%\cpmenu\config.toml`, edited by hand or by the menu (Ctrl+P, settings).
//! Writes go through toml_edit so the user's comments and formatting survive.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use toml_edit::{Array, DocumentMut, Item, Table, TableLike, Value, value};

use crate::hotkey::Hotkey;
use crate::power::{Action, DEFAULT_CONFIRM};
use crate::style::{Shape, Show, Style};
use crate::plugins::SettingValue;
use crate::{lang, themes};

const TEMPLATE: &str = "\
# cpmenu settings. Changes are picked up the next time the menu opens.

# Apps shown first, in this order, by the name the menu shows.
# Ctrl+P in the menu pins or unpins the selected app.
pinned = []

# Web search for \"? terms\". Unset: the default browser searches with its own engine.
# Set a URL to use a specific engine instead; {} is replaced by the terms.
# search_url = \"https://www.google.com/search?q={}\"

# [colors]
# theme = \"cyberpunk\"      # cyberpunk | arasaka | militech | netwatch | a file name from the themes folder
# accent = \"#5EF6FF\"       # overrides: accent alert tag quick setting text text_dim panel backdrop
# SAVE AS THEME in the settings panel (Ctrl+P, COLORS) saves the current colors as
# %APPDATA%\\cpmenu\\themes\\<name>.toml: a [colors] table with the roles above, and the
# per-element tables of style.toml. Copy and edit those files to make more.

# [layout]
# side = \"left\"            # left | right
# width_percent = 25       # of the screen width, 15-50
# visible_rows = 0         # 0 = automatic (rows about 54 px tall), or 3-30
# arc_shift = 40           # px; negative bends the list the other way
# backdrop_opacity = 0.6   # 0.0-1.0
# short_list = \"top\"      # top | center: where a list that fits sits
# band_percent = 50        # 0-100: where the selection sits while scrolling
# gui_scale = 100          # 50-250 in steps of 10: percent; scales the whole GUI (text, header, rows with automatic visible_rows, power bar, settings panel)
# high_contrast = false    # true: faded text (details, unselected labels, notes, row text at the list edges) is drawn at 90% or more
# icons = true             # false: rows show no icon (or letters in its place), and the row text uses the room
# actions = \"list\"         # list | popout: the actions of a row (Right arrow, right click) replace the list, or open as a small menu beside it
# actions_anchor = \"row\"   # row | list | cursor: where a popout opens (cursor = at the mouse when right-clicked, else the row)
# actions_x = 0            # -100..100: moves the popout sideways by this percent of its own width
# actions_y = 0            # -100..100: moves the popout down by this percent of its own height

# [behavior]
# hotkey = \"win\"          # win lwin rwin capslock f1..f24 | ctrl+alt+space win+s | none
# file_search = true       # file results from Everything
# file_results = 12        # 0-50
# recent = true            # false: no RECENT rows under the pinned ones while nothing is typed (launches are still remembered for ranking)
# confirm_power = [\"sign_out\", \"restart\", \"shutdown\"]   # power buttons that ask first: lock sleep sign_out restart shutdown
# language = \"auto\"        # auto (the Windows display language) | en | pl | the name of a file in %APPDATA%\\cpmenu\\lang\\
# plugins = true           # false: plugins in %APPDATA%\\cpmenu\\plugins are never scanned or started
# onboarded = false        # true: the first-run tutorial is not shown (SETTINGS, BEHAVIOR, RUN TUTORIAL starts it again)

# Replaces any UI string by its id, on top of the language (the settings panel's TITLE and
# SUBTITLE rows write here). The full list of ids is lang/en.toml in the repo; a lang\\<code>.toml
# file in %APPDATA%\\cpmenu\\lang\\ with the same layout adds a whole language.
# [text]
# \"header.title\" = \"MY_OS // START\"     # caption above the list
# \"header.subtitle\" = \"PROGRAMS:\"       # title of the list
# \"header.actions\" = \"OPTIONS\"          # title of the action list (right arrow)
# \"power.shutdown\" = \"POWER OFF\"        # power.lock sleep sign_out restart shutdown
# \"tag.app\" = \"PROGRAM\"                 # tag.app pinned file dir cmd calc web open set exec
# \"row.detail.web\" = \"IN MY BROWSER\"    # also row.detail.shell, row.detail.calc, row.hint
# \"dialog.prompt\" = \"SURE?\"             # dialog.caption execute abort
# \"status.no_matches\" = \"NOTHING\"       # also status.scanning, popup.settings, act.* (action names)
";

/// Preset names, in the order a settings panel should list them.
pub const THEMES: &[&str] = &["cyberpunk", "arasaka", "militech", "netwatch"];
const DEFAULT_THEME: &str = "cyberpunk";

/// Colors as 0xRRGGBB. Roles: `accent` borders/names, `alert` header and dangerous actions,
/// `tag` file tags, `quick` command tags, `panel` row background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub accent: u32,
    pub alert: u32,
    pub tag: u32,
    pub quick: u32,
    /// SET tags (settings results); a hue of its own so they don't read as apps.
    pub setting: u32,
    pub text: u32,
    pub text_dim: u32,
    pub panel: u32,
    pub backdrop: u32,
}

impl Theme {
    pub fn preset(name: &str) -> Option<Theme> {
        Some(match name.to_ascii_lowercase().as_str() {
            "cyberpunk" => Theme {
                accent: 0x5EF6FF,
                alert: 0xFF5E57,
                tag: 0xF5A623,
                quick: 0xFCEE0A,
                setting: 0xFF4FD8,
                text: 0xE2FFFF,
                text_dim: 0x8FDDE6,
                panel: 0x0B1A2A,
                backdrop: 0x03070D,
            },
            "arasaka" => Theme {
                accent: 0xFF3B3B,
                alert: 0xFF9A8F,
                tag: 0xFF9F43,
                quick: 0xFFD23F,
                setting: 0x4FD6C9,
                text: 0xFFE9E6,
                text_dim: 0xD98C86,
                panel: 0x1A0B0D,
                backdrop: 0x0D0304,
            },
            "militech" => Theme {
                accent: 0xFFB300,
                alert: 0xFF5A1F,
                tag: 0xC8D64A,
                quick: 0xFFE94D,
                setting: 0x56C2E8,
                text: 0xFFF4D6,
                text_dim: 0xC9A24A,
                panel: 0x17130A,
                backdrop: 0x080602,
            },
            "netwatch" => Theme {
                accent: 0x5AB0FF,
                alert: 0xB8D8FF,
                tag: 0x7C9CFF,
                quick: 0xE8F4FF,
                setting: 0xFF7AC8,
                text: 0xF0F7FF,
                text_dim: 0x8FB0D0,
                panel: 0x0A1220,
                backdrop: 0x03060D,
            },
            _ => return None,
        })
    }

    /// The role colors as (config key, color), in the order theme files list them.
    pub fn roles(&self) -> [(&'static str, u32); 9] {
        [
            ("accent", self.accent),
            ("alert", self.alert),
            ("tag", self.tag),
            ("quick", self.quick),
            ("setting", self.setting),
            ("text", self.text),
            ("text_dim", self.text_dim),
            ("panel", self.panel),
            ("backdrop", self.backdrop),
        ]
    }

    /// This theme with the roles `lookup` knows replaced; also whether any was.
    /// A new accent without a `setting` of its own takes a derived one (see `derive_setting`), so SET
    /// tags never silently match the accent.
    pub(crate) fn overridden(mut self, lookup: impl Fn(&str) -> Option<u32>) -> (Theme, bool) {
        let mut any = false;
        let slots: [(&str, &mut u32); 9] = [
            ("accent", &mut self.accent),
            ("alert", &mut self.alert),
            ("tag", &mut self.tag),
            ("quick", &mut self.quick),
            ("setting", &mut self.setting),
            ("text", &mut self.text),
            ("text_dim", &mut self.text_dim),
            ("panel", &mut self.panel),
            ("backdrop", &mut self.backdrop),
        ];
        for (key, slot) in slots {
            if let Some(hex) = lookup(key) {
                *slot = hex;
                any = true;
            }
        }
        if lookup("accent").is_some() && lookup("setting").is_none() {
            self.setting = derive_setting(self.accent);
        }
        (self, any)
    }
}

/// A color a third of the hue wheel away from `accent` (and at least that bright and saturated), for
/// themes that don't name a `setting` color.
pub(crate) fn derive_setting(accent: u32) -> u32 {
    let (r, g, b) = ((accent >> 16 & 0xFF) as f32 / 255.0, (accent >> 8 & 0xFF) as f32 / 255.0, (accent & 0xFF) as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    // A grey accent has no hue to rotate; give it a fixed one.
    let (hue, sat) = if delta == 0.0 { (5.0, 0.6) } else { ((hue + 2.0).rem_euclid(6.0), (delta / max).max(0.5)) };
    let value = max.max(0.7);
    let channel = |n: f32| {
        let k = (n + hue).rem_euclid(6.0);
        value - value * sat * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    let byte = |v: f32| (v * 255.0).round() as u32;
    byte(channel(5.0)) << 16 | byte(channel(3.0)) << 8 | byte(channel(1.0))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutConfig {
    pub side: Side,
    pub width_percent: u32,
    pub visible_rows: u32,
    pub arc_shift: i32,
    pub backdrop_opacity: f32,
    pub short_list: ShortList,
    /// Where the selected row sits in the list area while scrolling: 0 top, 50 middle, 100 bottom.
    pub band_percent: u32,
    /// Percent, `GUI_SCALE_MIN..=GUI_SCALE_MAX` in steps of `GUI_SCALE_STEP`: scales the whole GUI.
    pub gui_scale: u32,
    /// Faded text is drawn at 90% opacity or more.
    pub high_contrast: bool,
    /// Rows show their icon box; off, the box (and the letters standing in for a missing icon) is gone.
    pub icons: bool,
    pub actions: ActionsMode,
    pub actions_anchor: ActionsAnchor,
    /// Percent of the pop-out's own width / height it is moved from its anchor, -100..=100.
    pub actions_x: i32,
    pub actions_y: i32,
    /// Row size and corners. Not read from config.toml: style.toml's `[shape]` fills it in
    /// (`window::place`), so the layout code has one place to look.
    pub shape: Shape,
    /// What is drawn; likewise filled in from style.toml's `[show]`.
    pub show: Show,
}

pub const GUI_SCALE_MIN: u32 = 50;
pub const GUI_SCALE_MAX: u32 = 250;
pub const GUI_SCALE_STEP: u32 = 10;

/// `percent` clamped into the GUI scale range and rounded to the nearest step (halves go up).
pub fn snap_gui_scale(percent: i64) -> u32 {
    let step = GUI_SCALE_STEP as i64;
    let rounded = (percent + step / 2).div_euclid(step) * step;
    rounded.clamp(GUI_SCALE_MIN as i64, GUI_SCALE_MAX as i64) as u32
}

/// How the actions of a row (→ or right click) are shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActionsMode {
    /// In place of the list.
    List,
    /// A small menu beside the list; the list stays.
    Popout,
}

/// What a pop-out of actions is placed against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActionsAnchor {
    Row,
    List,
    Cursor,
}

/// Where a list that fits in the list area is placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShortList {
    Top,
    Center,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            side: Side::Left,
            width_percent: 25,
            visible_rows: 0,
            arc_shift: 40,
            backdrop_opacity: 0.6,
            short_list: ShortList::Top,
            band_percent: 50,
            gui_scale: 100,
            high_contrast: false,
            icons: true,
            actions: ActionsMode::List,
            actions_anchor: ActionsAnchor::Row,
            actions_x: 0,
            actions_y: 0,
            shape: Shape::default(),
            show: Show::default(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Config {
    pub pinned: Vec<String>,
    /// Web search URL with `{}` for the terms; `None` = the default browser's own engine.
    pub search_url: Option<String>,
    pub colors: Theme,
    /// Theme behind `colors`, before any hex overrides: an entry of `THEMES` or a custom theme's file name.
    pub theme_name: String,
    /// Text of the custom theme's file (its per-element colors are the defaults under style.toml); empty for presets.
    pub theme_elements: String,
    /// At least one valid hex override in `[colors]`; the settings panel can't show those.
    pub color_overrides: bool,
    pub layout: LayoutConfig,
    pub hotkey: Hotkey,
    pub file_search: bool,
    pub file_results: u32,
    /// RECENT rows under the pinned ones.
    pub recent: bool,
    /// The first-run wizard has been finished or skipped.
    pub onboarded: bool,
    /// Power buttons that show the full-screen confirmation before running.
    pub confirm_power: Vec<Action>,
    /// `auto`, or the code of a language (`lang::configure` resolves it).
    pub language: String,
    /// The `[text]` table: string ids replaced by the user.
    pub text: BTreeMap<String, String>,
    pub plugins: PluginsConfig,
}

/// `[behavior] plugins` and the `[plugins.<id>]` tables.
#[derive(Debug, PartialEq)]
pub struct PluginsConfig {
    /// Master switch; false = nothing is scanned or started.
    pub enabled: bool,
    /// By plugin id, including ids no installed plugin has (those are kept in the file, ignored here).
    pub entries: BTreeMap<String, PluginEntry>,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self { enabled: true, entries: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginEntry {
    /// Default true: a discovered plugin is on, but still needs `approved` to start.
    pub enabled: bool,
    /// Receives every query, not just its keyword's (the manifest has to ask for it too). Default off.
    pub global: bool,
    /// Replaces the manifest's keywords with this one.
    pub keyword: Option<String>,
    /// `plugins::approval_key` of what the user allowed.
    pub approved: Option<String>,
    /// `[plugins.<id>.settings]`: strings and booleans; other value types are ignored.
    pub settings: BTreeMap<String, SettingValue>,
}

impl Default for PluginEntry {
    fn default() -> Self {
        Self { enabled: true, global: false, keyword: None, approved: None, settings: BTreeMap::new() }
    }
}

fn parse_plugins(doc: &DocumentMut) -> PluginsConfig {
    let enabled = doc.get("behavior").and_then(|b| b.get("plugins")).and_then(Item::as_bool).unwrap_or(true);
    let entries = doc
        .get("plugins")
        .and_then(Item::as_table_like)
        .into_iter()
        .flat_map(|table| table.iter())
        .filter_map(|(id, item)| {
            let table = item.as_table_like()?;
            let text = |key: &str| table.get(key).and_then(Item::as_str).map(str::to_owned);
            let flag = |key: &str, default: bool| table.get(key).and_then(Item::as_bool).unwrap_or(default);
            let settings = table
                .get("settings")
                .and_then(Item::as_table_like)
                .into_iter()
                .flat_map(|t| t.iter())
                .filter_map(|(key, item)| {
                    let value = match (item.as_str(), item.as_bool()) {
                        (Some(s), _) => SettingValue::String(s.to_owned()),
                        (_, Some(b)) => SettingValue::Bool(b),
                        _ => return None,
                    };
                    Some((key.to_owned(), value))
                })
                .collect();
            let entry = PluginEntry { enabled: flag("enabled", true), global: flag("global", false), keyword: text("keyword"), approved: text("approved"), settings };
            Some((id.to_owned(), entry))
        })
        .collect();
    PluginsConfig { enabled, entries }
}

impl Config {
    /// The element colors the theme gives, which style.toml overrides.
    pub fn defaults(&self) -> Style {
        Style::resolve(&self.colors, &self.theme_elements)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pinned: Vec::new(),
            search_url: None,
            colors: Theme::preset(DEFAULT_THEME).unwrap(),
            theme_name: DEFAULT_THEME.to_owned(),
            theme_elements: String::new(),
            color_overrides: false,
            layout: LayoutConfig::default(),
            hotkey: Hotkey::default(),
            file_search: true,
            file_results: 12,
            recent: true,
            onboarded: false,
            confirm_power: DEFAULT_CONFIRM.to_vec(),
            language: lang::AUTO.to_owned(),
            text: BTreeMap::new(),
            plugins: PluginsConfig::default(),
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("cpmenu").join("config.toml"))
}

/// Lets the menu notice hand edits without re-reading the file on every open.
pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

pub fn load() -> Config {
    path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse_with(&t, &themes::read)).unwrap_or_default()
}

/// `#RRGGBB` only; anything else is ignored so one typo costs one key, not the file.
pub(crate) fn parse_hex(s: &str) -> Option<u32> {
    let digits = s.strip_prefix('#')?;
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u32::from_str_radix(digits, 16).ok())?
}

/// The theme `requested` names: a preset, or a custom theme whose file `read_theme` returns and
/// parses. Anything else is the default preset.
fn pick_theme(requested: Option<&str>, read_theme: &dyn Fn(&str) -> Option<String>) -> (String, Theme, String) {
    let default = || (DEFAULT_THEME.to_owned(), Theme::preset(DEFAULT_THEME).unwrap(), String::new());
    let Some(name) = requested.map(str::trim) else { return default() };
    if let Some(preset) = THEMES.iter().copied().find(|t| t.eq_ignore_ascii_case(name)) {
        return (preset.to_owned(), Theme::preset(preset).unwrap(), String::new());
    }
    match read_theme(name).and_then(|text| Some((themes::parse_roles(&text)?, text))) {
        Some((roles, text)) => (name.to_owned(), roles, text),
        None => default(),
    }
}

#[cfg(test)]
pub(crate) fn parse(text: &str) -> Config {
    parse_with(text, &|_| None)
}

/// Parses config.toml; `read_theme` supplies the text of a custom theme file by name.
pub(crate) fn parse_with(text: &str, read_theme: &dyn Fn(&str) -> Option<String>) -> Config {
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return Config::default();
    };
    let pinned = doc
        .get("pinned")
        .and_then(|item| item.as_array())
        .map(|array| array.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let search_url = doc
        .get("search_url")
        .and_then(|item| item.as_str())
        .filter(|url| url.contains("{}"))
        .map(str::to_owned);

    let get = |table: &str, key: &str| doc.get(table)?.get(key);
    let int = |table: &str, key: &str| get(table, key).and_then(Item::as_integer);
    let flag = |table: &str, key: &str, default: bool| get(table, key).and_then(Item::as_bool).unwrap_or(default);

    let (theme_name, preset, theme_elements) = pick_theme(get("colors", "theme").and_then(Item::as_str), read_theme);
    let (colors, color_overrides) = preset.overridden(|key| get("colors", key).and_then(Item::as_str).and_then(parse_hex));

    let defaults = LayoutConfig::default();
    let layout = LayoutConfig {
        side: match get("layout", "side").and_then(Item::as_str).map(str::to_ascii_lowercase).as_deref() {
            Some("right") => Side::Right,
            _ => defaults.side,
        },
        width_percent: int("layout", "width_percent").map_or(defaults.width_percent, |v| v.clamp(15, 50) as u32),
        // 0 (or "auto") sizes the rows from the screen.
        visible_rows: int("layout", "visible_rows").map_or(defaults.visible_rows, |v| if v == 0 { 0 } else { v.clamp(3, 30) as u32 }),
        arc_shift: int("layout", "arc_shift").map_or(defaults.arc_shift, |v| v.clamp(-120, 120) as i32),
        backdrop_opacity: get("layout", "backdrop_opacity")
            .and_then(|i| i.as_float().or_else(|| i.as_integer().map(|n| n as f64)))
            .filter(|v| v.is_finite())
            .map_or(defaults.backdrop_opacity, |v| v.clamp(0.0, 1.0) as f32),
        short_list: match get("layout", "short_list").and_then(Item::as_str).map(str::to_ascii_lowercase).as_deref() {
            Some("center") => ShortList::Center,
            _ => defaults.short_list,
        },
        band_percent: int("layout", "band_percent").map_or(defaults.band_percent, |v| v.clamp(0, 100) as u32),
        // `text_size` (100 | 115 | 130) is the key this setting replaced; its value is kept as it is
        // (115 stays 115) until the panel steps it onto the new grid.
        gui_scale: match (int("layout", "gui_scale"), int("layout", "text_size")) {
            (Some(v), _) => snap_gui_scale(v),
            (None, Some(v)) => v.clamp(GUI_SCALE_MIN as i64, GUI_SCALE_MAX as i64) as u32,
            (None, None) => defaults.gui_scale,
        },
        high_contrast: flag("layout", "high_contrast", defaults.high_contrast),
        icons: flag("layout", "icons", defaults.icons),
        actions: match get("layout", "actions").and_then(Item::as_str).map(str::to_ascii_lowercase).as_deref() {
            Some("popout") => ActionsMode::Popout,
            _ => defaults.actions,
        },
        actions_anchor: match get("layout", "actions_anchor").and_then(Item::as_str).map(str::to_ascii_lowercase).as_deref() {
            Some("list") => ActionsAnchor::List,
            Some("cursor") => ActionsAnchor::Cursor,
            _ => defaults.actions_anchor,
        },
        actions_x: int("layout", "actions_x").map_or(defaults.actions_x, |v| v.clamp(-100, 100) as i32),
        actions_y: int("layout", "actions_y").map_or(defaults.actions_y, |v| v.clamp(-100, 100) as i32),
        shape: defaults.shape,
        show: defaults.show,
    };

    // `win_key = false` predates `hotkey`; it still means "no hotkey" when `hotkey` isn't set.
    let hotkey = match get("behavior", "hotkey").and_then(Item::as_str) {
        Some(text) => Hotkey::parse(text).unwrap_or_default(),
        None if !flag("behavior", "win_key", true) => Hotkey::None,
        None => Hotkey::default(),
    };

    let base = Config::default();
    let confirm_power = match get("behavior", "confirm_power").and_then(Item::as_array) {
        Some(names) => parse_confirm(names.iter().filter_map(Value::as_str)),
        None => base.confirm_power,
    };
    Config {
        pinned,
        search_url,
        colors,
        theme_name,
        theme_elements,
        color_overrides,
        layout,
        hotkey,
        file_search: flag("behavior", "file_search", base.file_search),
        file_results: int("behavior", "file_results").map_or(base.file_results, |v| v.clamp(0, 50) as u32),
        recent: flag("behavior", "recent", base.recent),
        onboarded: flag("behavior", "onboarded", base.onboarded),
        confirm_power,
        language: get("behavior", "language").and_then(Item::as_str).map_or(base.language, |l| l.trim().to_ascii_lowercase()),
        text: lang::text_overrides(&doc),
        plugins: parse_plugins(&doc),
    }
}

/// Actions named in `confirm_power`; unknown names are ignored.
pub(crate) fn parse_confirm<'a>(names: impl Iterator<Item = &'a str>) -> Vec<Action> {
    names.filter_map(Action::from_name).collect()
}

/// `text` with `name` added to or removed from `pinned`; `None` if `text` isn't valid TOML,
/// so a file the user broke by hand is never overwritten.
fn toggle_in(text: &str, name: &str) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    if doc.get("pinned").and_then(|item| item.as_array()).is_none() {
        doc.insert("pinned", value(Array::new()));
    }
    let pinned = doc.get_mut("pinned")?.as_array_mut()?;
    let before = pinned.len();
    pinned.retain(|v| v.as_str() != Some(name));
    if pinned.len() == before {
        pinned.push(name);
    }
    Some(doc.to_string())
}

/// The table at `path` below `table`, created (missing parents implicit, so no empty header is
/// printed for them) where absent or not a table.
fn descend<'a>(table: &'a mut dyn TableLike, path: &[&str]) -> Option<&'a mut dyn TableLike> {
    let Some((first, rest)) = path.split_first() else { return Some(table) };
    if table.get(first).and_then(Item::as_table_like).is_none() {
        let mut created = Table::new();
        created.set_implicit(!rest.is_empty());
        table.insert(first, Item::Table(created));
    }
    descend(table.get_mut(first)?.as_table_like_mut()?, rest)
}

/// `text` with `[table] key = new`; an empty `table` means the top level. `None` if `text`
/// isn't valid TOML. Keeps comments and the trailing comment of a replaced value.
pub(crate) fn set_in(text: &str, table_name: &str, key: &str, new: Value) -> Option<String> {
    let path = [table_name];
    set_in_path(text, if table_name.is_empty() { &[] } else { &path }, key, new)
}

/// Like `set_in` for a nested table such as `[plugins.github.settings]`, given as its path.
pub(crate) fn set_in_path(text: &str, path: &[&str], key: &str, mut new: Value) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    let target = descend(doc.as_table_mut(), path)?;
    if let Some(old) = target.get(key).and_then(Item::as_value) {
        *new.decor_mut() = old.decor().clone();
    }
    target.insert(key, Item::Value(new));
    if path == ["layout"] && key == "gui_scale" {
        target.remove("text_size");
    }
    Some(doc.to_string())
}

/// `text` without `[table] key`; `None` if `text` isn't valid TOML. A missing key changes nothing.
pub(crate) fn remove_in(text: &str, table_name: &str, key: &str) -> Option<String> {
    remove_in_path(text, &[table_name], key)
}

pub(crate) fn remove_in_path(text: &str, path: &[&str], key: &str) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    let mut table: Option<&mut dyn TableLike> = Some(doc.as_table_mut());
    for name in path {
        table = table.and_then(|t| t.get_mut(name)).and_then(Item::as_table_like_mut);
    }
    if let Some(table) = table {
        table.remove(key);
    }
    Some(doc.to_string())
}

/// Pins or unpins `name`, creating the file from a commented template if needed.
pub fn toggle_pin(name: &str) -> Config {
    write_with(|text| toggle_in(text, name));
    load()
}

/// Sets `[table] key = new` (empty `table` = top level) for the settings panel. `None` when the
/// file was left untouched (it has syntax errors, or can't be written).
pub fn set_value(table_name: &str, key: &str, new: Value) -> Option<Config> {
    write_with(|text| set_in(text, table_name, key, new)).then(load)
}

/// Removes `[table] key` (the settings panel's way back to a default). Same `None` cases as `set_value`.
pub fn remove_value(table_name: &str, key: &str) -> Option<Config> {
    write_with(|text| remove_in(text, table_name, key)).then(load)
}

/// Sets `[plugins.<id>] key = new` (`enabled`, `global`, `keyword`, `approved`). Same `None` cases
/// as `set_value`, and for an id that isn't a valid plugin id.
pub fn set_plugin_value(id: &str, key: &str, new: Value) -> Option<Config> {
    if !crate::plugins::valid_id(id) {
        return None;
    }
    write_with(|text| set_in_path(text, &["plugins", id], key, new)).then(load)
}

/// Sets `[plugins.<id>.settings] key = new`, a value of the plugin's own settings.
pub fn set_plugin_setting(id: &str, key: &str, new: Value) -> Option<Config> {
    if !crate::plugins::valid_id(id) {
        return None;
    }
    write_with(|text| set_in_path(text, &["plugins", id, "settings"], key, new)).then(load)
}

/// Removes `[plugins.<id>] key`, e.g. a keyword override.
pub fn remove_plugin_value(id: &str, key: &str) -> Option<Config> {
    if !crate::plugins::valid_id(id) {
        return None;
    }
    write_with(|text| remove_in_path(text, &["plugins", id], key)).then(load)
}

/// Whether config.toml can be edited: missing, or valid TOML.
pub fn editable() -> bool {
    match path().map(std::fs::read_to_string) {
        Some(Ok(text)) => text.parse::<DocumentMut>().is_ok(),
        Some(Err(e)) => e.kind() == std::io::ErrorKind::NotFound,
        None => false,
    }
}

/// Path of config.toml, written from the template first if it doesn't exist yet.
pub fn ensure_file() -> Option<String> {
    let path = path()?;
    if !path.exists() {
        let _ = std::fs::create_dir_all(path.parent()?);
        std::fs::write(&path, TEMPLATE).ok()?;
    }
    path.to_str().map(str::to_owned)
}

/// Whether the edit produced a file that was written.
fn write_with(edit: impl FnOnce(&str) -> Option<String>) -> bool {
    let Some(path) = path() else { return false };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| TEMPLATE.to_owned());
    let Some(updated) = edit(&text) else { return false };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, updated).is_ok()
}

/// `text` with `[behavior] onboarded = true` added when it is an existing config (any content) that
/// predates the key; `None` when there is nothing to do (empty, already has the key, or not valid TOML).
fn onboarded_in(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let doc = text.parse::<DocumentMut>().ok()?;
    if doc.get("behavior").and_then(|b| b.get("onboarded")).is_some() {
        return None;
    }
    set_in(text, "behavior", "onboarded", Value::from(true))
}

/// Startup migration: a config.toml from before the first-run wizard counts as onboarded, so
/// existing users never see it. A missing file stays missing (a new install shows the wizard).
pub fn migrate_onboarded() {
    let Some(path) = path() else { return };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    if let Some(updated) = onboarded_in(&text) {
        let _ = std::fs::write(&path, updated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_configs_count_as_onboarded_but_new_installs_do_not() {
        let migrated = onboarded_in("pinned = [\"A\"]\n").unwrap();
        assert!(parse(&migrated).onboarded && parse(&migrated).pinned == ["A"]);
        assert!(onboarded_in(TEMPLATE).is_some_and(|t| parse(&t).onboarded), "the commented template is an existing file too");
        assert_eq!(onboarded_in(""), None, "an empty file is a new install");
        assert_eq!(onboarded_in("  \n"), None);
        assert_eq!(onboarded_in("[behavior]\nonboarded = false\n"), None, "an explicit false keeps the wizard");
        assert_eq!(onboarded_in("[behavior]\nonboarded = true\n"), None);
        assert_eq!(onboarded_in("pinned = ["), None, "a broken file is left alone");
        assert!(!parse("").onboarded && !parse("[behavior]\nonboarded = false\n").onboarded);
    }

    #[test]
    fn template_parses_to_empty_config() {
        assert_eq!(parse(TEMPLATE), Config::default());
    }

    #[test]
    fn toggle_adds_then_removes_and_keeps_comments() {
        let added = toggle_in(TEMPLATE, "Firefox").unwrap();
        assert_eq!(parse(&added).pinned, vec!["Firefox"]);
        assert!(added.contains("# Ctrl+P in the menu"));
        let removed = toggle_in(&added, "Firefox").unwrap();
        assert!(parse(&removed).pinned.is_empty());
    }

    #[test]
    fn toggle_keeps_order_and_other_keys() {
        let text = "pinned = [\"A\", \"B\"]\nother = 1\n";
        let updated = toggle_in(text, "C").unwrap();
        assert_eq!(parse(&updated).pinned, vec!["A", "B", "C"]);
        assert!(updated.contains("other = 1"));
    }

    #[test]
    fn broken_file_is_not_overwritten() {
        assert!(toggle_in("pinned = [", "A").is_none());
        assert_eq!(parse("pinned = ["), Config::default());
    }

    #[test]
    fn search_url_needs_a_placeholder() {
        assert_eq!(parse("search_url = \"https://duckduckgo.com/?q={}\"").search_url.as_deref(), Some("https://duckduckgo.com/?q={}"));
        assert_eq!(parse("search_url = \"https://example.com\"").search_url, None);
        assert_eq!(parse("").search_url, None, "unset = the browser's own engine");
    }

    #[test]
    fn confirm_power_defaults_parses_and_ignores_unknown_names() {
        use crate::power::Action::*;
        assert_eq!(parse("").confirm_power, vec![SignOut, Restart, Shutdown]);
        assert_eq!(parse("[behavior]\nconfirm_power = [\"lock\", \"nope\", \"shutdown\"]\n").confirm_power, vec![Lock, Shutdown]);
        assert_eq!(parse("[behavior]\nconfirm_power = []\n").confirm_power, vec![], "empty list = never ask");
        assert_eq!(parse("[behavior]\nconfirm_power = \"shutdown\"\n").confirm_power, vec![SignOut, Restart, Shutdown], "wrong type = default");
    }

    #[test]
    fn wrong_type_is_replaced_by_a_list() {
        assert_eq!(parse(&toggle_in("pinned = \"A\"\n", "B").unwrap()).pinned, vec!["B"]);
    }

    #[test]
    fn every_listed_theme_exists_and_cyberpunk_is_the_default() {
        for name in THEMES {
            assert!(Theme::preset(name).is_some(), "{name}");
        }
        assert!(Theme::preset("nope").is_none());
        assert_eq!(Theme::preset("CyberPunk"), Some(Config::default().colors));
        assert_eq!(Config::default().colors.accent, 0x5EF6FF);
    }

    #[test]
    fn theme_preset_and_overrides() {
        let c = parse("[colors]\ntheme = \"arasaka\"\naccent = \"#00ff7f\"\n");
        let arasaka = Theme::preset("arasaka").unwrap();
        assert_eq!(c.colors.accent, 0x00FF7F);
        assert_eq!(c.colors.alert, arasaka.alert);
        assert_eq!(parse("[colors]\ntheme = \"bogus\"\n").colors, Config::default().colors);
    }

    #[test]
    fn theme_name_and_override_flag_are_reported() {
        let plain = parse("[colors]\ntheme = \"Militech\"\n");
        assert_eq!((plain.theme_name.as_str(), plain.color_overrides), ("militech", false));
        let tweaked = parse("[colors]\ntheme = \"netwatch\"\ntext = \"#ffffff\"\n");
        assert_eq!((tweaked.theme_name.as_str(), tweaked.color_overrides), ("netwatch", true));
        assert!(!parse("[colors]\naccent = \"nope\"\n").color_overrides, "invalid hex is not an override");
        assert_eq!(parse("[colors]\ntheme = \"bogus\"\n").theme_name, "cyberpunk");
    }

    #[test]
    fn invalid_hex_falls_back_per_key() {
        let text = "[colors]\naccent = \"red\"\nalert = \"#12345\"\ntag = \"#GGGGGG\"\nquick = 5\ntext = \"#ffffff\"\n";
        let c = parse(text).colors;
        let base = Config::default().colors;
        assert_eq!((c.accent, c.alert, c.tag, c.quick), (base.accent, base.alert, base.tag, base.quick));
        assert_eq!(c.text, 0xFFFFFF);
    }

    #[test]
    fn actions_mode_anchor_and_offsets_parse_and_clamp() {
        let c = parse("").layout;
        assert_eq!((c.actions, c.actions_anchor, c.actions_x, c.actions_y), (ActionsMode::List, ActionsAnchor::Row, 0, 0));
        let c = parse("[layout]
actions = \"Popout\"
actions_anchor = \"cursor\"
actions_x = 250
actions_y = -250
").layout;
        assert_eq!((c.actions, c.actions_anchor, c.actions_x, c.actions_y), (ActionsMode::Popout, ActionsAnchor::Cursor, 100, -100));
        assert_eq!(parse("[layout]
actions_anchor = \"list\"
actions_x = -40
actions_y = 15
").layout.actions_anchor, ActionsAnchor::List);
        let c = parse("[layout]
actions = \"menu\"
actions_anchor = 3
actions_x = \"left\"
actions_y = 1.5
").layout;
        assert_eq!((c.actions, c.actions_anchor, c.actions_x, c.actions_y), (ActionsMode::List, ActionsAnchor::Row, 0, 0));
    }

    #[test]
    fn layout_values_are_clamped_and_bad_types_ignored() {
        let c = parse("[layout]\nside = \"right\"\nwidth_percent = 99\nvisible_rows = 99\narc_shift = -500\nbackdrop_opacity = 7\n").layout;
        assert_eq!(c, LayoutConfig { side: Side::Right, width_percent: 50, visible_rows: 30, arc_shift: -120, backdrop_opacity: 1.0, ..LayoutConfig::default() });
        let c = parse("[layout]\nside = \"middle\"\nwidth_percent = 1\nvisible_rows = \"many\"\nbackdrop_opacity = -1\n").layout;
        assert_eq!(c.side, Side::Left);
        assert_eq!((c.width_percent, c.visible_rows, c.backdrop_opacity), (15, 0, 0.0));
        assert_eq!(parse("[layout]\nbackdrop_opacity = 0.25\n").layout.backdrop_opacity, 0.25);
    }

    #[test]
    fn visible_rows_zero_or_absent_means_automatic() {
        assert_eq!(parse("").layout.visible_rows, 0);
        assert_eq!(parse("[layout]\nvisible_rows = 0\n").layout.visible_rows, 0);
        assert_eq!(parse("[layout]\nvisible_rows = \"auto\"\n").layout.visible_rows, 0);
        assert_eq!(parse("[layout]\nvisible_rows = 1\n").layout.visible_rows, 3);
        assert_eq!(parse("[layout]\nvisible_rows = 12\n").layout.visible_rows, 12);
    }

    #[test]
    fn hotkey_parsing_defaults_and_migration() {
        use crate::hotkey::{ANY_WIN, MOD_ALT};
        assert_eq!(parse("").hotkey, Hotkey::Tap(ANY_WIN));
        assert_eq!(parse("[behavior]\nhotkey = \"alt+space\"\n").hotkey, Hotkey::Combo { mods: MOD_ALT, vk: 0x20 });
        assert_eq!(parse("[behavior]\nhotkey = \"none\"\n").hotkey, Hotkey::None);
        assert_eq!(parse("[behavior]\nhotkey = \"nonsense\"\n").hotkey, Hotkey::Tap(ANY_WIN), "invalid falls back");
        assert_eq!(parse("[behavior]\nhotkey = 5\n").hotkey, Hotkey::Tap(ANY_WIN));
        assert_eq!(parse("[behavior]\nwin_key = false\n").hotkey, Hotkey::None, "old setting");
        assert_eq!(parse("[behavior]\nwin_key = true\n").hotkey, Hotkey::Tap(ANY_WIN));
        let both = parse("[behavior]\nwin_key = false\nhotkey = \"f1\"\n");
        assert_eq!(both.hotkey, Hotkey::Tap(0x70), "hotkey wins over the old setting");
    }

    #[test]
    fn short_list_and_band_percent() {
        assert_eq!(parse("").layout.short_list, ShortList::Top);
        assert_eq!(parse("[layout]\nshort_list = \"Center\"\n").layout.short_list, ShortList::Center);
        assert_eq!(parse("[layout]\nshort_list = \"middle\"\n").layout.short_list, ShortList::Top);
        assert_eq!(parse("").layout.band_percent, 50);
        assert_eq!(parse("[layout]\nband_percent = 150\n").layout.band_percent, 100);
        assert_eq!(parse("[layout]\nband_percent = -5\n").layout.band_percent, 0);
        assert_eq!(parse("[layout]\nband_percent = \"x\"\n").layout.band_percent, 50);
    }

    #[test]
    fn gui_scale_clamps_and_steps_and_high_contrast_is_a_flag() {
        let scale = |v: &str| parse(&format!("[layout]
gui_scale = {v}
")).layout.gui_scale;
        assert_eq!((scale("50"), scale("100"), scale("250")), (50, 100, 250));
        assert_eq!((scale("104"), scale("105"), scale("96"), scale("500"), scale("-5"), scale("0")), (100, 110, 100, 250, 50, 50));
        assert_eq!(scale("\"big\""), 100, "wrong type = default");
        assert_eq!((parse("").layout.gui_scale, parse("").layout.high_contrast), (100, false));
        assert!(parse("[layout]
high_contrast = true
").layout.high_contrast);
        assert!(!parse("[layout]
high_contrast = \"yes\"
").layout.high_contrast);
    }

    #[test]
    fn the_old_text_size_key_is_read_until_gui_scale_is_written() {
        let scale = |text: &str| parse(text).layout.gui_scale;
        assert_eq!((scale("[layout]
text_size = 115
"), scale("[layout]
text_size = 130
")), (115, 130));
        assert_eq!(scale("[layout]
text_size = 115
gui_scale = 70
"), 70, "gui_scale wins");
        let out = set_in("[layout]
text_size = 130 # old
side = \"left\"
", "layout", "gui_scale", 130_i64.into()).unwrap();
        assert!(!out.contains("text_size") && out.contains("gui_scale = 130") && out.contains("side"));
        assert_eq!(scale(&out), 130);
        let untouched = set_in("[layout]
text_size = 130
", "layout", "side", "right".into()).unwrap();
        assert!(untouched.contains("text_size"));
    }

    #[test]
    fn icons_default_on_and_the_flag_is_read() {
        assert!(parse("").layout.icons);
        assert!(!parse("[layout]\nicons = false\n").layout.icons);
        assert!(parse("[layout]\nicons = \"no\"\n").layout.icons, "wrong type = default");
        assert!(parse("[layout]\nicons = true\n").layout.icons);
    }

    #[test]
    fn every_preset_has_a_setting_color_that_differs_from_the_accent() {
        for name in THEMES {
            let t = Theme::preset(name).unwrap();
            assert_ne!(t.setting, t.accent, "{name}");
            assert!(t.roles().contains(&("setting", t.setting)));
        }
    }

    #[test]
    fn the_setting_role_is_a_colors_key_with_a_derived_fallback() {
        let c = parse("[colors]\nsetting = \"#010203\"\n");
        assert_eq!((c.colors.setting, c.color_overrides), (0x010203, true));
        // A new accent without a setting color gets a derived one, never the accent itself.
        let c = parse("[colors]\ntheme = \"arasaka\"\naccent = \"#00AA00\"\n");
        assert_eq!(c.colors.setting, derive_setting(0x00AA00));
        assert_ne!(c.colors.setting, c.colors.accent);
        // Other overrides leave the preset's own setting color alone.
        assert_eq!(parse("[colors]\nalert = \"#FFFFFF\"\n").colors.setting, Theme::preset("cyberpunk").unwrap().setting);
    }

    #[test]
    fn derived_setting_colors_differ_from_the_accent_for_any_accent() {
        for accent in [0x000000, 0xFFFFFF, 0x808080, 0xFF0000, 0x00FF00, 0x0000FF, 0x5EF6FF, 0xFFB300, 0x000001] {
            let derived = derive_setting(accent);
            assert_ne!(derived, accent, "{accent:06X}");
        }
        assert_eq!(derive_setting(0xFF0000), 0x00FF00, "a third of the wheel from red is green");
    }

    #[test]
    fn recent_is_on_unless_switched_off() {
        assert!(parse("").recent);
        assert!(!parse("[behavior]\nrecent = false\n").recent);
        assert!(parse("[behavior]\nrecent = \"no\"\n").recent, "wrong type = default");
    }

    #[test]
    fn behavior_defaults_and_clamps() {
        let c = parse("[behavior]\nfile_search = false\nfile_results = 500\n");
        assert!(!c.file_search);
        assert_eq!(c.file_results, 50);
        assert_eq!(parse("[behavior]\nfile_results = -3\n").file_results, 0);
    }

    #[test]
    fn set_value_creates_table_and_keeps_comments() {
        let text = set_in(TEMPLATE, "layout", "side", Value::from("right")).unwrap();
        assert_eq!(parse(&text).layout.side, Side::Right);
        assert!(text.contains("# Ctrl+P in the menu") && text.contains("# visible_rows = 0"));
        let text = set_in(&text, "layout", "visible_rows", Value::from(12)).unwrap();
        let c = parse(&text).layout;
        assert_eq!((c.side, c.visible_rows), (Side::Right, 12));
    }

    #[test]
    fn set_value_replaces_in_place_with_trailing_comment() {
        let text = "[behavior]\nfile_search = true # keep me\nother = 1\n";
        let updated = set_in(text, "behavior", "file_search", Value::from(false)).unwrap();
        assert!(updated.contains("file_search = false # keep me") && updated.contains("other = 1"), "{updated}");
        assert!(!parse(&updated).file_search);
    }

    #[test]
    fn set_value_top_level_and_wrong_shaped_table() {
        let text = set_in("pinned = []\n", "", "search_url", Value::from("https://a.b/?q={}")).unwrap();
        assert_eq!(parse(&text).search_url.as_deref(), Some("https://a.b/?q={}"));
        let text = set_in("colors = 3\n", "colors", "theme", Value::from("netwatch")).unwrap();
        assert_eq!(parse(&text).colors, Theme::preset("netwatch").unwrap());
    }

    fn mine(name: &str) -> Option<String> {
        (name == "mine").then(|| "[colors]\naccent = \"#112233\"\npanel = \"#445566\"\n[row]\nname = \"#778899\"\n".to_owned())
    }

    #[test]
    fn custom_theme_supplies_roles_and_element_defaults() {
        let c = parse_with("[colors]\ntheme = \"mine\"\n", &mine);
        assert_eq!(c.theme_name, "mine");
        assert_eq!((c.colors.accent, c.colors.panel), (0x112233, 0x445566));
        assert_eq!(c.colors.alert, Config::default().colors.alert, "roles the file omits come from the default preset");
        assert!(!c.color_overrides);
        let defaults = c.defaults();
        assert_eq!(defaults.get("row.name").unwrap().rgb, 0x778899);
        assert_eq!(defaults.get("row.panel").unwrap().rgb, 0x445566, "elements the file omits follow its roles");
        let tweaked = parse_with("[colors]\ntheme = \"mine\"\naccent = \"#ffffff\"\n", &mine);
        assert_eq!((tweaked.colors.accent, tweaked.color_overrides), (0xFFFFFF, true), "config hex still wins");
    }

    #[test]
    fn missing_or_broken_custom_theme_falls_back_to_cyberpunk() {
        for text in ["[colors]\ntheme = \"gone\"\n", "[colors]\ntheme = \"broken\"\n", "[colors]\ntheme = \"../escape\"\n"] {
            let c = parse_with(text, &|name| (name != "gone").then(|| "[colors\naccent = ".to_owned()));
            assert_eq!(c, Config::default(), "{text}");
        }
        assert_eq!(parse_with("[colors]\ntheme = \"mine\"\n", &|_| None), Config::default());
        let preset = parse_with("[colors]\ntheme = \"ARASAKA\"\n", &mine);
        assert_eq!((preset.theme_name.as_str(), preset.theme_elements.as_str()), ("arasaka", ""), "presets win over files of the same name");
    }

    #[test]
    fn language_and_text_overrides() {
        let c = parse("");
        assert_eq!((c.language.as_str(), c.text.len()), ("auto", 0));
        let c = parse("[behavior]\nlanguage = \" PL \"\n[text]\n\"header.title\" = \"MY_OS\"\nbad = 5\n[text.header]\nsubtitle = \"LIST\"\n");
        assert_eq!(c.language, "pl");
        assert_eq!(c.text.get("header.title").map(String::as_str), Some("MY_OS"));
        assert_eq!(c.text.get("header.subtitle").map(String::as_str), Some("LIST"));
        assert_eq!(c.text.len(), 2, "only strings");
        assert_eq!(parse("[behavior]\nlanguage = 5\n").language, "auto");
    }

    #[test]
    fn text_overrides_are_written_quoted_and_removed_again() {
        let text = set_in(TEMPLATE, "text", "header.title", Value::from("MY_OS // START")).unwrap();
        assert!(text.contains("\"header.title\" = \"MY_OS // START\""), "{text}");
        assert!(text.contains("# Ctrl+P in the menu"));
        assert_eq!(parse(&text).text.get("header.title").map(String::as_str), Some("MY_OS // START"));
        let text = set_in(&text, "text", "header.title", Value::from("TWO")).unwrap();
        assert_eq!(parse(&text).text.len(), 1);
        let text = remove_in(&text, "text", "header.title").unwrap();
        assert!(parse(&text).text.is_empty());
        assert!(text.contains("# Ctrl+P in the menu"));
        assert_eq!(remove_in("a = 1\n", "text", "x").as_deref(), Some("a = 1\n"));
        assert!(remove_in("[text\n", "text", "x").is_none());
    }

    #[test]
    fn plugins_default_to_on_with_no_entries() {
        let c = parse("");
        assert!(c.plugins.enabled && c.plugins.entries.is_empty());
        assert!(!parse("[behavior]\nplugins = false\n").plugins.enabled);
        assert!(parse("[behavior]\nplugins = \"no\"\n").plugins.enabled, "wrong type = default");
    }

    #[test]
    fn plugin_entries_are_parsed_and_unknown_ids_kept() {
        let text = "[plugins.github]\nenabled = false\nglobal = true\nkeyword = \"gh\"\napproved = \"1.2.0|python main.py\"\nfuture = 1\n\
                    [plugins.github.settings]\nuser = \"jane\"\nforks = true\ncount = 5\n[plugins.ghost]\n[plugins]\nstray = 3\n";
        let c = parse(text).plugins;
        let github = &c.entries["github"];
        assert!(!github.enabled && github.global);
        assert_eq!((github.keyword.as_deref(), github.approved.as_deref()), (Some("gh"), Some("1.2.0|python main.py")));
        assert_eq!(github.settings.len(), 2, "only strings and booleans");
        assert_eq!(github.settings["user"], SettingValue::String("jane".into()));
        assert_eq!(github.settings["forks"], SettingValue::Bool(true));
        assert_eq!(c.entries["ghost"], PluginEntry::default(), "unknown ids are parsed like any other");
        assert_eq!(c.entries.len(), 2, "non-table values under [plugins] are not plugins");
    }

    #[test]
    fn plugin_values_round_trip_and_keep_comments() {
        let text = set_in_path(TEMPLATE, &["plugins", "github"], "approved", Value::from("1|python main.py")).unwrap();
        let text = set_in_path(&text, &["plugins", "github"], "enabled", Value::from(false)).unwrap();
        let text = set_in_path(&text, &["plugins", "github", "settings"], "user", Value::from("jane")).unwrap();
        assert!(text.contains("# Ctrl+P in the menu") && text.contains("# plugins = true"));
        assert!(!text.contains("[plugins]\n"), "the parent stays implicit: {text}");
        let c = parse(&text).plugins.entries.remove("github").unwrap();
        assert_eq!((c.approved.as_deref(), c.enabled), (Some("1|python main.py"), false));
        assert_eq!(c.settings["user"], SettingValue::String("jane".into()));

        let text = set_in_path(&text, &["plugins", "github"], "enabled", Value::from(true)).unwrap();
        assert!(parse(&text).plugins.entries["github"].enabled);
        let text = set_in_path(&text, &["plugins", "other"], "keyword", Value::from("o")).unwrap();
        assert_eq!(parse(&text).plugins.entries.len(), 2);

        let text = remove_in_path(&text, &["plugins", "github"], "approved").unwrap();
        assert_eq!(parse(&text).plugins.entries["github"].approved, None);
        assert_eq!(parse(&text).plugins.entries["github"].settings.len(), 1);
        assert_eq!(remove_in_path("a = 1\n", &["plugins", "x"], "k").as_deref(), Some("a = 1\n"));
    }

    #[test]
    fn plugin_edit_keeps_a_hand_written_unknown_key_and_unknown_plugin() {
        let text = "[plugins.old]\nkeep = \"me\" # note\n\n[plugins.github]\nenabled = true\n";
        let updated = set_in_path(text, &["plugins", "github"], "enabled", Value::from(false)).unwrap();
        assert!(updated.contains("keep = \"me\" # note") && updated.contains("[plugins.old]"), "{updated}");
        assert!(set_in_path("[plugins\n", &["plugins", "x"], "k", Value::from(1)).is_none());
        assert!(set_plugin_value("Bad Id", "enabled", Value::from(true)).is_none(), "ids are validated before any file is touched");
    }

    #[test]
    fn set_value_refuses_broken_file() {
        assert!(set_in("[layout\nside = ", "layout", "side", Value::from("right")).is_none());
    }
}
