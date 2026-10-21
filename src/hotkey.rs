//! The key that opens the menu, as written in config.toml (`hotkey = "ctrl+alt+space"`): parsing,
//! canonical formatting and building one from a key press. Pure; no Win32 calls.

pub const MOD_ALT: u32 = 1;
pub const MOD_CONTROL: u32 = 2;
pub const MOD_SHIFT: u32 = 4;
pub const MOD_WIN: u32 = 8;

pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
/// Pseudo key code for "either Win key"; not a real virtual key.
pub const ANY_WIN: u16 = 0x100;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hotkey {
    None,
    /// A key tapped alone (watched by the keyboard hook).
    Tap(u16),
    /// Modifiers plus a key (registered with `RegisterHotKey`).
    Combo { mods: u32, vk: u16 },
}

impl Default for Hotkey {
    fn default() -> Self {
        Hotkey::Tap(ANY_WIN)
    }
}

const NAMED: &[(&str, u16)] = &[
    ("space", 0x20),
    ("enter", 0x0D),
    ("tab", 0x09),
    ("capslock", 0x14),
    ("apps", 0x5D),
    ("scrolllock", 0x91),
    ("pause", 0x13),
    ("printscreen", 0x2C),
    ("numlock", 0x90),
    ("insert", 0x2D),
    ("delete", 0x2E),
    ("home", 0x24),
    ("end", 0x23),
    ("pageup", 0x21),
    ("pagedown", 0x22),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("grave", 0xC0),
    ("minus", 0xBD),
    ("equals", 0xBB),
    ("comma", 0xBC),
    ("period", 0xBE),
    ("slash", 0xBF),
    ("semicolon", 0xBA),
    ("quote", 0xDE),
    ("lbracket", 0xDB),
    ("rbracket", 0xDD),
    ("backslash", 0xDC),
];

const ALIASES: &[(&str, &str)] = &[
    ("caps", "capslock"),
    ("menu", "apps"),
    ("return", "enter"),
    ("pgup", "pageup"),
    ("pgdn", "pagedown"),
    ("ins", "insert"),
    ("del", "delete"),
    ("prtsc", "printscreen"),
    ("`", "grave"),
];

fn vk_of(name: &str) -> Option<u16> {
    let name = ALIASES.iter().find(|(alias, _)| *alias == name).map_or(name, |(_, real)| real);
    let bytes = name.as_bytes();
    if bytes.len() == 1 && bytes[0].is_ascii_lowercase() {
        return Some(bytes[0].to_ascii_uppercase() as u16);
    }
    if bytes.len() == 1 && bytes[0].is_ascii_digit() {
        return Some(bytes[0] as u16);
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()).filter(|n| (1..=24).contains(n)) {
        if !name.starts_with("f0") {
            return Some(0x70 + n - 1);
        }
    }
    NAMED.iter().find(|(n, _)| *n == name).map(|&(_, vk)| vk)
}

fn name_of(vk: u16) -> Option<String> {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => Some((vk as u8 as char).to_ascii_lowercase().to_string()),
        0x70..=0x87 => Some(format!("f{}", vk - 0x70 + 1)),
        _ => NAMED.iter().find(|&&(_, v)| v == vk).map(|&(n, _)| n.to_owned()),
    }
}

/// Keys that may be used alone as a hotkey; a lone letter or Enter would steal normal typing.
fn tappable(vk: u16) -> bool {
    matches!(vk, 0x70..=0x87 | 0x14 | 0x5D | 0x91 | 0x13 | 0x2C | 0x90)
}

fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | VK_LWIN | VK_RWIN)
}

fn modifier_flag(name: &str) -> Option<u32> {
    match name {
        "ctrl" | "control" => Some(MOD_CONTROL),
        "alt" => Some(MOD_ALT),
        "shift" => Some(MOD_SHIFT),
        "win" => Some(MOD_WIN),
        _ => None,
    }
}

impl Hotkey {
    /// `None` when `text` isn't a valid hotkey. `"none"` (or empty) is `Some(Hotkey::None)`.
    pub fn parse(text: &str) -> Option<Hotkey> {
        let text = text.trim().to_ascii_lowercase();
        if text.is_empty() || text == "none" {
            return Some(Hotkey::None);
        }
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        if let [single] = parts[..] {
            return match single {
                "win" => Some(Hotkey::Tap(ANY_WIN)),
                "lwin" => Some(Hotkey::Tap(VK_LWIN)),
                "rwin" => Some(Hotkey::Tap(VK_RWIN)),
                key => vk_of(key).filter(|&vk| tappable(vk)).map(Hotkey::Tap),
            };
        }
        let (key, modifiers) = parts.split_last()?;
        let mut mods = 0;
        for m in modifiers {
            let flag = modifier_flag(m)?;
            if mods & flag != 0 {
                return None;
            }
            mods |= flag;
        }
        Some(Hotkey::Combo { mods, vk: vk_of(key)? })
    }

    /// Canonical lowercase form; `parse(format())` gives the same hotkey back.
    pub fn format(self) -> String {
        match self {
            Hotkey::None => "none".to_owned(),
            Hotkey::Tap(ANY_WIN) => "win".to_owned(),
            Hotkey::Tap(VK_LWIN) => "lwin".to_owned(),
            Hotkey::Tap(VK_RWIN) => "rwin".to_owned(),
            Hotkey::Tap(vk) => name_of(vk).unwrap_or_else(|| "none".to_owned()),
            Hotkey::Combo { mods, vk } => {
                let mut out = String::new();
                for (flag, name) in [(MOD_CONTROL, "ctrl"), (MOD_ALT, "alt"), (MOD_SHIFT, "shift"), (MOD_WIN, "win")] {
                    if mods & flag != 0 {
                        out.push_str(name);
                        out.push('+');
                    }
                }
                out + &name_of(vk).unwrap_or_default()
            }
        }
    }

    pub fn display(self) -> String {
        match self {
            Hotkey::None => "NONE".to_owned(),
            other => other.format().to_uppercase(),
        }
    }

    /// The hotkey a key press makes while capturing: `mods` are the modifiers held with `vk`.
    /// `None` for a bare modifier (still being held), an unnamed key, or a plain typing key.
    pub fn from_press(vk: u16, mods: u32) -> Option<Hotkey> {
        if vk == VK_LWIN || vk == VK_RWIN {
            return (mods == 0).then_some(Hotkey::Tap(if vk == VK_LWIN { ANY_WIN } else { VK_RWIN }));
        }
        if is_modifier(vk) {
            return None;
        }
        if mods == 0 {
            return tappable(vk).then_some(Hotkey::Tap(vk));
        }
        name_of(vk).map(|_| Hotkey::Combo { mods, vk })
    }
}

/// Whether tap-binding `binding` is triggered by the physical key `vk`.
pub fn tap_matches(binding: u16, vk: u16) -> bool {
    binding == vk || (binding == ANY_WIN && (vk == VK_LWIN || vk == VK_RWIN))
}

/// Whether the binding is a Win key, which needs the Start-menu masking trick.
pub fn is_win_binding(binding: u16) -> bool {
    matches!(binding, ANY_WIN | VK_LWIN | VK_RWIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_taps_combos_and_none() {
        assert_eq!(Hotkey::parse("win"), Some(Hotkey::Tap(ANY_WIN)));
        assert_eq!(Hotkey::parse(" LWin "), Some(Hotkey::Tap(VK_LWIN)));
        assert_eq!(Hotkey::parse("rwin"), Some(Hotkey::Tap(VK_RWIN)));
        assert_eq!(Hotkey::parse("CapsLock"), Some(Hotkey::Tap(0x14)));
        assert_eq!(Hotkey::parse("f1"), Some(Hotkey::Tap(0x70)));
        assert_eq!(Hotkey::parse("f24"), Some(Hotkey::Tap(0x87)));
        assert_eq!(Hotkey::parse("none"), Some(Hotkey::None));
        assert_eq!(Hotkey::parse(""), Some(Hotkey::None));
        assert_eq!(Hotkey::parse("ctrl+alt+space"), Some(Hotkey::Combo { mods: MOD_CONTROL | MOD_ALT, vk: 0x20 }));
        assert_eq!(Hotkey::parse("Alt + Space"), Some(Hotkey::Combo { mods: MOD_ALT, vk: 0x20 }));
        assert_eq!(Hotkey::parse("win+s"), Some(Hotkey::Combo { mods: MOD_WIN, vk: 0x53 }));
        assert_eq!(Hotkey::parse("ctrl+shift+f12"), Some(Hotkey::Combo { mods: MOD_CONTROL | MOD_SHIFT, vk: 0x7B }));
        assert_eq!(Hotkey::parse("shift+ctrl+1"), Some(Hotkey::Combo { mods: MOD_CONTROL | MOD_SHIFT, vk: 0x31 }));
    }

    #[test]
    fn rejects_invalid_hotkeys() {
        for bad in ["a", "enter", "space", "ctrl", "f25", "f0", "f01", "ctrl+", "+s", "ctrl+ctrl+s", "ctrl+win", "bogus", "ctrl+bogus", "s+ctrl"] {
            assert_eq!(Hotkey::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn format_round_trips_and_is_canonical() {
        for text in ["win", "lwin", "rwin", "capslock", "apps", "f1", "f13", "f24", "scrolllock", "pause", "none",
                     "ctrl+alt+space", "alt+space", "win+s", "ctrl+shift+f12", "ctrl+alt+shift+win+a", "ctrl+grave", "alt+0"] {
            let parsed = Hotkey::parse(text).unwrap();
            assert_eq!(parsed.format(), text, "{text}");
            assert_eq!(Hotkey::parse(&parsed.format()), Some(parsed));
        }
        assert_eq!(Hotkey::parse("Shift+Ctrl+Space").unwrap().format(), "ctrl+shift+space", "modifier order");
        assert_eq!(Hotkey::parse("caps").unwrap().format(), "capslock", "alias");
        assert_eq!(Hotkey::Combo { mods: MOD_ALT, vk: 0x20 }.display(), "ALT+SPACE");
        assert_eq!(Hotkey::None.display(), "NONE");
    }

    #[test]
    fn key_presses_become_hotkeys() {
        assert_eq!(Hotkey::from_press(0x70, 0), Some(Hotkey::Tap(0x70)), "F1 alone");
        assert_eq!(Hotkey::from_press(0x14, 0), Some(Hotkey::Tap(0x14)), "caps lock alone");
        assert_eq!(Hotkey::from_press(0x41, 0), None, "a typing key alone");
        assert_eq!(Hotkey::from_press(0x20, MOD_ALT), Some(Hotkey::Combo { mods: MOD_ALT, vk: 0x20 }));
        assert_eq!(Hotkey::from_press(0x11, 0), None, "bare ctrl");
        assert_eq!(Hotkey::from_press(0x12, MOD_ALT), None, "bare alt");
        assert_eq!(Hotkey::from_press(VK_LWIN, 0), Some(Hotkey::Tap(ANY_WIN)));
        assert_eq!(Hotkey::from_press(VK_RWIN, 0), Some(Hotkey::Tap(VK_RWIN)));
        assert_eq!(Hotkey::from_press(VK_LWIN, MOD_CONTROL), None);
        assert_eq!(Hotkey::from_press(0xFF, MOD_CONTROL), None, "unnamed key");
        assert_eq!(Hotkey::from_press(0x53, MOD_WIN), Some(Hotkey::Combo { mods: MOD_WIN, vk: 0x53 }));
    }

    #[test]
    fn tap_binding_matching() {
        assert!(tap_matches(ANY_WIN, VK_LWIN) && tap_matches(ANY_WIN, VK_RWIN));
        assert!(tap_matches(VK_LWIN, VK_LWIN) && !tap_matches(VK_LWIN, VK_RWIN));
        assert!(tap_matches(0x14, 0x14) && !tap_matches(0x14, 0x70));
        assert!(is_win_binding(ANY_WIN) && is_win_binding(VK_RWIN) && !is_win_binding(0x14));
    }
}
