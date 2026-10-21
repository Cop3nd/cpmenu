//! UI strings by stable id. Layers, first hit wins: the `[text]` table of config.toml, the active
//! language, English. A language is a built-in file (`lang/en.toml`, `lang/pl.toml`, embedded) and/or
//! `%APPDATA%\cpmenu\lang\<code>.toml`; a user file overrides the keys it defines. An id that nobody
//! defines is shown as the id itself, so a missing string is visible instead of blank.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};
use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};

const BUILTIN: [(&str, &str); 2] = [("en", include_str!("../lang/en.toml")), ("pl", include_str!("../lang/pl.toml"))];
pub const AUTO: &str = "auto";
const ENGLISH: &str = "en";

type Strings = HashMap<String, String>;

/// Adds every string under `item` to `out`, named by the path of table names and the key.
fn flatten(item: &Item, id: &str, out: &mut Strings) {
    if let Some(text) = item.as_str() {
        out.insert(id.to_owned(), text.to_owned());
    } else if let Some(table) = item.as_table_like() {
        for (key, value) in table.iter() {
            flatten(value, &format!("{id}.{key}"), out);
        }
    }
}

/// A language file's strings; empty if it doesn't parse.
fn parse_strings(text: &str) -> Strings {
    let mut out = Strings::new();
    if let Ok(doc) = text.parse::<DocumentMut>() {
        for (key, value) in doc.iter() {
            flatten(value, key, &mut out);
        }
    }
    out
}

/// The strings of config.toml's `[text]` table: `"header.title" = "..."` or nested tables.
pub fn text_overrides(doc: &DocumentMut) -> BTreeMap<String, String> {
    let mut flat = Strings::new();
    if let Some(table) = doc.get("text").and_then(Item::as_table_like) {
        for (key, value) in table.iter() {
            flatten(value, key, &mut flat);
        }
    }
    flat.into_iter().collect()
}

fn user_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("cpmenu").join("lang"))
}

fn valid_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 16 && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Codes of the `.toml` files in `dir`, lowercase and sorted.
fn user_codes_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut codes: Vec<String> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            (path.extension()?.eq_ignore_ascii_case("toml")).then_some(())?;
            let code = path.file_stem()?.to_str()?.to_ascii_lowercase();
            valid_code(&code).then_some(code)
        })
        .collect();
    codes.sort();
    codes.dedup();
    codes
}

/// Built-in languages, then the user's others.
fn languages_in(dir: Option<&Path>) -> Vec<String> {
    let mut all: Vec<String> = BUILTIN.iter().map(|(code, _)| (*code).to_owned()).collect();
    for code in dir.map(user_codes_in).unwrap_or_default() {
        if !all.contains(&code) {
            all.push(code);
        }
    }
    all
}

/// Every language the LANGUAGE row can pick (without AUTO).
pub fn languages() -> Vec<String> {
    languages_in(user_dir().as_deref())
}

/// The built-in strings of `code` with the keys of the user's file for it on top.
fn layers(code: &str, dir: Option<&Path>) -> Strings {
    let mut strings = BUILTIN.iter().find(|(c, _)| *c == code).map(|(_, text)| parse_strings(text)).unwrap_or_default();
    if let Some(text) = dir.filter(|_| valid_code(code)).and_then(|d| std::fs::read_to_string(d.join(format!("{code}.toml"))).ok()) {
        strings.extend(parse_strings(&text));
    }
    strings
}

/// "pl" from "pl-PL"; that language if `available`, English otherwise.
pub fn pick_from_locale(locale: &str, available: &[String]) -> String {
    let primary = locale.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    if available.contains(&primary) { primary } else { ENGLISH.to_owned() }
}

/// The language `setting` (config.toml's `language`) stands for.
fn resolve(setting: &str, available: &[String], system_locale: &dyn Fn() -> String) -> String {
    let setting = setting.trim().to_ascii_lowercase();
    if setting == AUTO || setting.is_empty() {
        return pick_from_locale(&system_locale(), available);
    }
    if available.contains(&setting) { setting } else { ENGLISH.to_owned() }
}

/// The user's Windows display language as a locale name ("pl-PL"); empty if it can't be read.
fn system_locale() -> String {
    let mut buf = [0u16; 85];
    let len = unsafe { LCIDToLocaleName(GetUserDefaultUILanguage() as u32, Some(&mut buf), 0) };
    String::from_utf16_lossy(&buf[..(len.max(1) as usize - 1).min(buf.len())])
}

struct Table {
    text: Strings,
    active: Strings,
    english: Strings,
    code: String,
}

impl Table {
    fn build(code: &str, dir: Option<&Path>, text: &BTreeMap<String, String>) -> Self {
        Table {
            code: code.to_owned(),
            text: text.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            active: if code == ENGLISH { Strings::new() } else { layers(code, dir) },
            english: layers(ENGLISH, dir),
        }
    }

    fn get(&self, id: &str) -> Option<&str> {
        [&self.text, &self.active, &self.english].into_iter().find_map(|layer| layer.get(id)).map(String::as_str)
    }
}

thread_local! {
    static TABLE: RefCell<Table> = RefCell::new(Table::build(ENGLISH, None, &BTreeMap::new()));
}

/// Makes `setting` (a code, or `auto` for the system language) and the `[text]` overrides the live strings.
pub fn configure(setting: &str, text: &BTreeMap<String, String>) {
    let dir = user_dir();
    let code = resolve(setting, &languages_in(dir.as_deref()), &system_locale);
    TABLE.set(Table::build(&code, dir.as_deref(), text));
}

/// Makes the built-in language `code` live on this thread, ignoring the user's files.
#[cfg(test)]
pub fn use_builtin(code: &str) {
    TABLE.set(Table::build(code, None, &BTreeMap::new()));
}

/// Every built-in Polish string, to check the font can draw them.
#[cfg(test)]
pub fn polish_text() -> String {
    layers("pl", None).into_values().collect::<Vec<_>>().join(" ") + " ĄĆĘŁŃÓŚŹŻ…"
}

/// The resolved language code (`en`, `pl`, ...), as sent to plugins.
pub fn code() -> String {
    TABLE.with_borrow(|t| t.code.clone())
}

/// The string for `id`, or `None` if no layer defines it.
pub fn find(id: &str) -> Option<String> {
    TABLE.with_borrow(|t| t.get(id).map(str::to_owned))
}

pub fn t(id: &str) -> String {
    find(id).unwrap_or_else(|| id.to_owned())
}

/// "{n} MATCH" style string for `n` by the language's plural forms (`<id>.one`, `.few`, `.many`).
pub fn plural(id: &str, n: usize) -> String {
    let category = match (n % 10, n % 100) {
        _ if n == 1 => "one",
        (2..=4, rest) if !(12..=14).contains(&rest) => "few",
        _ => "many",
    };
    let text = find(&format!("{id}.{category}")).or_else(|| find(&format!("{id}.many"))).unwrap_or_else(|| id.to_owned());
    text.replace("{n}", &n.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cpmenu-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn nested_tables_and_dotted_keys_flatten_to_the_same_ids() {
        let strings = parse_strings("[a.b]\nc = \"1\"\n[d]\n\"e.f\" = \"2\"\ng = 5\n");
        assert_eq!(strings.get("a.b.c").map(String::as_str), Some("1"));
        assert_eq!(strings.get("d.e.f").map(String::as_str), Some("2"));
        assert!(!strings.contains_key("d.g"), "only strings");
        assert!(parse_strings("a = [").is_empty());
        let doc: DocumentMut = "[text]\n\"header.title\" = \"X\"\n[text.header]\nsubtitle = \"Y\"\n".parse().unwrap();
        let text = text_overrides(&doc);
        assert_eq!((text.get("header.title").map(String::as_str), text.get("header.subtitle").map(String::as_str)), (Some("X"), Some("Y")));
    }

    #[test]
    fn builtin_languages_define_the_same_ids() {
        let en = layers("en", None);
        let pl = layers("pl", None);
        assert!(en.len() > 80);
        for id in en.keys() {
            assert!(pl.contains_key(id), "pl is missing {id}");
        }
        for (id, text) in &pl {
            assert!(en.contains_key(id) || id.starts_with("settings.color.") || id.ends_with(".few"), "pl has unknown id {id}");
            assert!(!text.is_empty(), "{id}");
        }
        assert_eq!(pl["power.shutdown"], "WYŁĄCZ");
    }

    #[test]
    fn every_polish_color_label_matches_a_style_key() {
        let pl = layers("pl", None);
        for key in crate::style::KEYS {
            assert!(pl.contains_key(&format!("settings.color.{key}")), "{key}");
        }
        assert_eq!(pl.keys().filter(|k| k.starts_with("settings.color.")).count(), crate::style::KEYS.len());
    }

    #[test]
    fn layers_resolve_text_then_language_then_english() {
        let text: BTreeMap<_, _> = [("header.title".to_owned(), "MINE".to_owned())].into();
        let table = Table::build("pl", None, &text);
        assert_eq!(table.get("header.title"), Some("MINE"));
        assert_eq!(table.get("power.lock"), Some("BLOKUJ"));
        assert_eq!(table.get("header.matches.few"), Some("{n} WYNIKI"));
        let english = Table::build("en", None, &BTreeMap::new());
        assert_eq!(english.get("power.lock"), Some("LOCK"));
        assert_eq!(english.get("header.matches.few"), None);
        assert_eq!(english.get("no.such.id"), None);
    }

    #[test]
    fn user_files_add_languages_override_builtins_and_fall_back_to_english() {
        let dir = temp_dir("lang");
        std::fs::write(dir.join("de.toml"), "[power]\nlock = \"SPERREN\"\n").unwrap();
        std::fs::write(dir.join("PL.toml"), "[power]\nlock = \"ZABLOKUJ\"\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join("bad name.toml"), "x = 1").unwrap();
        assert_eq!(languages_in(Some(&dir)), vec!["en", "pl", "de"]);
        let de = Table::build("de", Some(&dir), &BTreeMap::new());
        assert_eq!((de.get("power.lock"), de.get("power.sleep")), (Some("SPERREN"), Some("SLEEP")));
        let pl = Table::build("pl", Some(&dir), &BTreeMap::new());
        assert_eq!((pl.get("power.lock"), pl.get("power.sleep")), (Some("ZABLOKUJ"), Some("UŚPIJ")), "user file overrides only its keys");
        std::fs::write(dir.join("en.toml"), "[power]\nlock = \"LOCK IT\"\n").unwrap();
        assert_eq!(Table::build("de", Some(&dir), &BTreeMap::new()).get("power.sleep"), Some("SLEEP"));
        assert_eq!(Table::build("en", Some(&dir), &BTreeMap::new()).get("power.lock"), Some("LOCK IT"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn auto_follows_the_system_language_when_it_is_available() {
        let all = ["en".to_owned(), "pl".to_owned(), "de".to_owned()];
        assert_eq!(pick_from_locale("pl-PL", &all), "pl");
        assert_eq!(pick_from_locale("de-AT", &all), "de");
        assert_eq!(pick_from_locale("fr-FR", &all), "en");
        assert_eq!(pick_from_locale("", &all), "en");
        let system = || "pl-PL".to_owned();
        assert_eq!(resolve("auto", &all, &system), "pl");
        assert_eq!(resolve("", &all, &system), "pl");
        assert_eq!(resolve("EN", &all, &system), "en");
        assert_eq!(resolve("de", &all, &system), "de");
        assert_eq!(resolve("xx", &all, &system), "en", "unknown language");
        assert_eq!(resolve("auto", &["en".to_owned()], &system), "en");
    }

    #[test]
    fn plural_forms_follow_the_language() {
        TABLE.set(Table::build("pl", None, &BTreeMap::new()));
        let forms: Vec<String> = [1, 2, 4, 5, 12, 14, 22, 25, 0].into_iter().map(|n| plural("header.matches", n)).collect();
        assert_eq!(forms, ["1 WYNIK", "2 WYNIKI", "4 WYNIKI", "5 WYNIKÓW", "12 WYNIKÓW", "14 WYNIKÓW", "22 WYNIKI", "25 WYNIKÓW", "0 WYNIKÓW"]);
        TABLE.set(Table::build("en", None, &BTreeMap::new()));
        assert_eq!((plural("header.matches", 1), plural("header.matches", 3)), ("1 MATCH".to_owned(), "3 MATCHES".to_owned()));
    }

    #[test]
    fn unknown_ids_show_themselves() {
        assert_eq!(t("no.such.id"), "no.such.id");
        assert_eq!(t("header.title"), "NETRUNNER_OS // START");
    }
}
