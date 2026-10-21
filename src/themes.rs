//! Custom themes: `%APPDATA%\cpmenu\themes\<name>.toml`. A `[colors]` table holds the eight role
//! colors; the other tables are per-element colors with the same keys as style.toml. A theme file
//! is the layer between the roles and the user's style.toml.

use std::path::{Path, PathBuf};

use toml_edit::DocumentMut;

use crate::config::{THEMES, Theme, parse_hex};
use crate::style::Style;

pub const MAX_NAME: usize = 24;

/// Device names Windows won't let a file be called, with or without an extension.
const RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];

pub fn is_builtin(name: &str) -> bool {
    THEMES.iter().any(|t| t.eq_ignore_ascii_case(name.trim()))
}

/// Names a theme can't take: the presets and Windows device names.
pub fn is_reserved(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    let numbered = |prefix: &str| lower.strip_prefix(prefix).is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']));
    is_builtin(name) || RESERVED.contains(&lower.as_str()) || numbered("com") || numbered("lpt")
}

/// Characters a theme name can contain.
pub fn valid_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')
}

/// A name that is safe to use as a file name: nothing but `valid_char`, no edge spaces, not too long.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= MAX_NAME && name.chars().all(valid_char) && name == name.trim()
}

pub fn dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("cpmenu").join("themes"))
}

/// Custom theme names in `dir`, sorted by name; presets and unusable file names are left out.
pub fn list_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            path.extension()?.eq_ignore_ascii_case("toml").then_some(())?;
            let name = path.file_stem()?.to_str()?.to_owned();
            (valid_name(&name) && !is_reserved(&name)).then_some(name)
        })
        .collect();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

pub fn list() -> Vec<String> {
    dir().map(|d| list_in(&d)).unwrap_or_default()
}

pub fn read_in(dir: &Path, name: &str) -> Option<String> {
    if !valid_name(name) {
        return None;
    }
    std::fs::read_to_string(dir.join(format!("{name}.toml"))).ok()
}

pub fn read(name: &str) -> Option<String> {
    read_in(&dir()?, name)
}

pub fn write_in(dir: &Path, name: &str, text: &str) -> bool {
    valid_name(name) && std::fs::create_dir_all(dir).is_ok() && std::fs::write(dir.join(format!("{name}.toml")), text).is_ok()
}

pub fn write(name: &str, text: &str) -> bool {
    dir().is_some_and(|d| write_in(&d, name, text))
}

pub fn delete_in(dir: &Path, name: &str) -> bool {
    valid_name(name) && std::fs::remove_file(dir.join(format!("{name}.toml"))).is_ok()
}

pub fn delete(name: &str) -> bool {
    dir().is_some_and(|d| delete_in(&d, name))
}

/// The role colors of a theme file; roles it omits are the default preset's. `None` if the file
/// doesn't parse.
pub fn parse_roles(text: &str) -> Option<Theme> {
    let doc = text.parse::<DocumentMut>().ok()?;
    let base = Theme::preset("cyberpunk")?;
    let lookup = |key: &str| doc.get("colors")?.get(key)?.as_str().and_then(parse_hex);
    Some(base.overridden(lookup).0)
}

/// The text of a theme file: the roles, then every element of `style` that differs from what the
/// roles alone would give (the theme file's own elements and the user's style.toml overrides).
pub fn to_text(name: &str, roles: &Theme, style: &Style) -> String {
    let mut out = format!(
        "# cpmenu theme \"{name}\". [colors] holds the role colors; the other tables are per-element\n\
         # colors with the keys of style.toml. An element left out follows the roles.\n\n[colors]\n"
    );
    for (key, rgb) in roles.roles() {
        out.push_str(&format!("{key} = \"#{rgb:06X}\"\n"));
    }
    let mut table = "";
    for (key, paint) in style.changed_from(&Style::from_theme(roles)) {
        let (group, element) = key.split_once('.').unwrap_or(("", key));
        if group != table {
            out.push_str(&format!("\n[{group}]\n"));
            table = group;
        }
        out.push_str(&format!("{element} = \"{}\"\n", paint.hex()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cpmenu-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn names_are_checked_for_characters_length_and_reserved_words() {
        for good in ["mine", "My Theme-2_x", "a", &"x".repeat(MAX_NAME)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", " lead", "trail ", "a/b", "..", "a\\b", "c:x", "dot.dot", "ł", &"x".repeat(MAX_NAME + 1)] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(is_builtin("Cyberpunk") && is_builtin(" netwatch ") && !is_builtin("mine"));
        assert!(is_reserved("CON") && is_reserved("com1") && is_reserved("LPT9") && is_reserved("arasaka"));
        assert!(!is_reserved("com10") && !is_reserved("com0") && !is_reserved("console"));
    }

    #[test]
    fn theme_file_round_trips_roles_and_changed_elements() {
        let roles = Theme { accent: 0x112233, ..Theme::preset("militech").unwrap() };
        let style = Style::resolve(&roles, "[row]\nname = \"#010203\"\n[tag]\nfile = \"#AABBCC80\"\n[popup]\ntext = \"#FFFFFF\"\n");
        let text = to_text("Mine", &roles, &style);
        assert!(text.starts_with("# cpmenu theme \"Mine\""));
        assert_eq!(parse_roles(&text), Some(roles));
        assert_eq!(Style::resolve(&roles, &text), style, "the file reproduces the style");
        assert!(!text.contains("[header]"), "unchanged elements are not written:\n{text}");
        assert!(text.contains("[row]\nname = \"#010203\"") && text.contains("file = \"#AABBCC80\""), "{text}");
    }

    #[test]
    fn a_plain_theme_has_only_roles() {
        let roles = Theme::preset("netwatch").unwrap();
        let text = to_text("plain", &roles, &Style::from_theme(&roles));
        assert_eq!(text.lines().filter(|l| l.starts_with('[')).count(), 1, "{text}");
        assert_eq!(parse_roles(&text), Some(roles));
    }

    #[test]
    fn roles_a_file_omits_come_from_cyberpunk_and_broken_files_are_rejected() {
        let cyber = Theme::preset("cyberpunk").unwrap();
        let derived = crate::config::derive_setting(1);
        assert_eq!(parse_roles("[colors]\naccent = \"#000001\"\nalert = \"red\"\n"), Some(Theme { accent: 1, setting: derived, ..cyber }));
        assert_eq!(parse_roles(""), Some(cyber));
        assert_eq!(parse_roles("[colors\naccent = "), None);
    }

    #[test]
    fn a_theme_file_without_a_setting_color_gets_one_unlike_its_accent() {
        let theme = parse_roles("[colors]\naccent = \"#FF0000\"\n").unwrap();
        assert_ne!(theme.setting, theme.accent);
        assert_ne!(theme.setting, Theme::preset("cyberpunk").unwrap().setting, "derived from the file's accent, not inherited");
        assert_eq!(theme.setting, crate::config::derive_setting(0xFF0000));
        // Without an accent of its own the file keeps cyberpunk's pair; an explicit setting always wins.
        assert_eq!(parse_roles("[colors]\nalert = \"#112233\"\n").unwrap().setting, Theme::preset("cyberpunk").unwrap().setting);
        assert_eq!(parse_roles("[colors]\naccent = \"#FF0000\"\nsetting = \"#00FF00\"\n").unwrap().setting, 0x00FF00);
    }

    #[test]
    fn the_setting_role_round_trips_through_a_theme_file() {
        let roles = Theme { setting: 0x123456, ..Theme::preset("netwatch").unwrap() };
        let text = to_text("s", &roles, &Style::from_theme(&roles));
        assert!(text.contains("setting = \"#123456\""), "{text}");
        assert_eq!(parse_roles(&text), Some(roles));
    }

    #[test]
    fn files_are_listed_sorted_read_written_and_deleted() {
        let dir = temp_dir("themes");
        assert!(list_in(&dir).is_empty(), "missing folder");
        assert!(write_in(&dir, "zeta", "[colors]\n"));
        assert!(write_in(&dir, "Alpha", "[colors]\n"));
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join("cyberpunk.toml"), "x").unwrap();
        std::fs::write(dir.join("bad.name.toml"), "x").unwrap();
        assert_eq!(list_in(&dir), vec!["Alpha", "zeta"]);
        assert_eq!(read_in(&dir, "zeta").as_deref(), Some("[colors]\n"));
        assert_eq!(read_in(&dir, "../zeta"), None);
        assert!(!write_in(&dir, "../escape", "x") && !dir.parent().unwrap().join("escape.toml").exists());
        assert!(delete_in(&dir, "zeta") && !delete_in(&dir, "zeta"));
        assert_eq!(list_in(&dir), vec!["Alpha"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
