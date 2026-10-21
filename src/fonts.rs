//! DirectWrite side of the `[fonts]` setting: which families are installed, whether a family can
//! be used, and the vertical metrics that keep text centred when the family or size changes.

use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{HSTRING, w};

use crate::style::DEFAULT_FAMILY;

fn system_fonts(dwrite: &IDWriteFactory) -> Option<IDWriteFontCollection> {
    let mut fonts = None;
    unsafe { dwrite.GetSystemFontCollection(&mut fonts, false) }.ok()?;
    fonts
}

/// The installed families, sorted, without duplicates (names in en-us, or the first the font has).
pub fn installed(dwrite: &IDWriteFactory) -> Vec<String> {
    let Some(fonts) = system_fonts(dwrite) else { return Vec::new() };
    let mut names: Vec<String> = (0..unsafe { fonts.GetFontFamilyCount() })
        .filter_map(|i| {
            let family = unsafe { fonts.GetFontFamily(i) }.ok()?;
            let strings = unsafe { family.GetFamilyNames() }.ok()?;
            let (mut at, mut found) = (0, false.into());
            if unsafe { strings.FindLocaleName(w!("en-us"), &mut at, &mut found) }.is_err() || !found.as_bool() {
                at = 0;
            }
            let mut buffer = vec![0u16; unsafe { strings.GetStringLength(at) }.ok()? as usize + 1];
            unsafe { strings.GetString(at, &mut buffer) }.ok()?;
            let name = String::from_utf16_lossy(&buffer[..buffer.len() - 1]);
            // "@Name" is the vertical-writing twin of a CJK family.
            (!name.is_empty() && !name.starts_with('@')).then_some(name)
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

/// `installed` with a factory of its own, for the settings panel.
pub fn installed_now() -> Vec<String> {
    let dwrite: windows::core::Result<IDWriteFactory> = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) };
    dwrite.map(|d| installed(&d)).unwrap_or_default()
}

/// Whether `name` is an installed family (DirectWrite compares case-insensitively).
pub fn is_installed(dwrite: &IDWriteFactory, name: &str) -> bool {
    let Some(fonts) = system_fonts(dwrite) else { return false };
    let (mut at, mut found) = (0, false.into());
    unsafe { fonts.FindFamilyName(&HSTRING::from(name), &mut at, &mut found) }.is_ok() && found.as_bool()
}

/// Whether `family` can show its own name: false for symbol and icon fonts (Wingdings, Segoe
/// MDL2 Assets...), whose letters are pictures or missing. A family that is not installed shows
/// as the fallback font, so it reads fine.
pub fn shows_name(dwrite: &IDWriteFactory, family: &str) -> bool {
    let Some(fonts) = system_fonts(dwrite) else { return true };
    let (mut at, mut found) = (0, false.into());
    if unsafe { fonts.FindFamilyName(&HSTRING::from(family), &mut at, &mut found) }.is_err() || !found.as_bool() {
        return true;
    }
    let font = unsafe { fonts.GetFontFamily(at) }.and_then(|f| unsafe { f.GetFirstMatchingFont(DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL) });
    let Ok(font) = font else { return true };
    if unsafe { font.IsSymbolFont() }.as_bool() {
        return false;
    }
    family.chars().filter(|c| !c.is_whitespace()).all(|c| unsafe { font.HasCharacter(c as u32) }.is_ok_and(|has| has.as_bool()))
}

/// `wanted` if it is installed, else Bahnschrift.
pub fn resolve(dwrite: &IDWriteFactory, wanted: &str) -> String {
    if is_installed(dwrite, wanted) { wanted.to_owned() } else { DEFAULT_FAMILY.to_owned() }
}

/// The places in `families` that fit what was typed: the exact name, then names starting with it,
/// then names containing it, each group in list order. Case-insensitive; nothing typed fits all.
pub fn matching(families: &[String], typed: &str) -> Vec<usize> {
    let typed = typed.trim().to_lowercase();
    let rank = |name: &str| {
        let name = name.to_lowercase();
        if name == typed {
            Some(0)
        } else if name.starts_with(&typed) {
            Some(1)
        } else {
            name.contains(&typed).then_some(2)
        }
    };
    let mut ranked: Vec<(u8, usize)> = families.iter().enumerate().filter_map(|(i, name)| Some((rank(name)?, i))).collect();
    ranked.sort_by_key(|&(r, i)| (r, i));
    ranked.into_iter().map(|(_, i)| i).collect()
}

/// A cache of at most `cap` values by name that drops the one used longest ago, for the text
/// formats of the dropdown's previews: only the families in view are built, and scrolling back
/// finds them again.
pub struct Lru<V> {
    cap: usize,
    /// Least recently used first.
    items: Vec<(String, V)>,
}

impl<V: Clone> Lru<V> {
    pub fn new(cap: usize) -> Self {
        Self { cap, items: Vec::new() }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// The value for `key`, made by `make` (and kept) if there is none; `None` if it can't be made.
    pub fn get_or_make(&mut self, key: &str, make: impl FnOnce() -> Option<V>) -> Option<V> {
        if let Some(at) = self.items.iter().position(|(k, _)| k == key) {
            let used = self.items.remove(at);
            let value = used.1.clone();
            self.items.push(used);
            return Some(value);
        }
        let value = make()?;
        if self.items.len() >= self.cap {
            self.items.remove(0);
        }
        self.items.push((key.to_owned(), value.clone()));
        Some(value)
    }
}

/// Where a one-line text sits in its line box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub height: f32,
    pub baseline: f32,
    /// Cap height at the format's size.
    pub cap: f32,
}

impl Line {
    /// Distance from the top of the line box to the middle of the capitals.
    pub fn ink_centre(self) -> f32 {
        self.baseline - self.cap / 2.0
    }
}

/// The line box and cap height of `format`, a format made for `family` / `weight` / `size`.
pub fn line(dwrite: &IDWriteFactory, format: &IDWriteTextFormat, family: &str, weight: i32, size: f32) -> Option<Line> {
    let units: Vec<u16> = "Hx".encode_utf16().collect();
    let layout = unsafe { dwrite.CreateTextLayout(&units, format, 4096.0, 4096.0) }.ok()?;
    let mut lines = [DWRITE_LINE_METRICS::default(); 2];
    let mut count = 0;
    unsafe { layout.GetLineMetrics(Some(&mut lines), &mut count) }.ok()?;
    let first = lines.first().filter(|_| count > 0)?;
    let font_metrics = || -> Option<DWRITE_FONT_METRICS> {
        let fonts = system_fonts(dwrite)?;
        let (mut at, mut found) = (0, false.into());
        unsafe { fonts.FindFamilyName(&HSTRING::from(family), &mut at, &mut found) }.ok()?;
        let font = unsafe { fonts.GetFontFamily(at) }.ok()?;
        let font = unsafe { font.GetFirstMatchingFont(DWRITE_FONT_WEIGHT(weight), DWRITE_FONT_STRETCH_UNDEFINED, DWRITE_FONT_STYLE_NORMAL) }.ok()?;
        let mut m = DWRITE_FONT_METRICS::default();
        unsafe { font.GetMetrics(&mut m) };
        Some(m)
    };
    let cap = match font_metrics() {
        Some(m) if m.designUnitsPerEm > 0 && m.capHeight > 0 => m.capHeight as f32 / m.designUnitsPerEm as f32 * size,
        _ => 0.7 * size,
    };
    Some(Line { height: first.height, baseline: first.baseline, cap })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::DirectWrite::DWriteCreateFactory;

    fn factory() -> IDWriteFactory {
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.unwrap()
    }

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn typing_filters_the_list_best_match_first() {
        let fonts = list(&["Arial", "Segoe UI", "Segoe UI Semibold", "Consolas", "Bahnschrift", "Lucida Sans"]);
        assert_eq!(matching(&fonts, ""), vec![0, 1, 2, 3, 4, 5], "nothing typed fits all");
        assert_eq!(matching(&fonts, "seg"), vec![1, 2]);
        assert_eq!(matching(&fonts, "SEGOE UI"), vec![1, 2], "the exact name comes first");
        assert_eq!(matching(&fonts, "sans"), vec![5], "inside a name");
        assert_eq!(matching(&fonts, "s"), vec![1, 2, 3, 4, 5], "prefixes first, then names containing it");
        assert!(matching(&fonts, "zzz").is_empty());
        assert_eq!(matching(&fonts, "  arial "), vec![0]);
    }

    #[test]
    fn the_preview_cache_keeps_its_cap_and_drops_the_longest_unused() {
        let mut cache = Lru::new(3);
        let mut made = 0;
        let mut get = |cache: &mut Lru<u32>, key: &str| {
            cache.get_or_make(key, || {
                made += 1;
                Some(made)
            })
        };
        assert_eq!([get(&mut cache, "a"), get(&mut cache, "b"), get(&mut cache, "c")], [Some(1), Some(2), Some(3)]);
        assert_eq!(get(&mut cache, "a"), Some(1), "found, not made again");
        assert_eq!(get(&mut cache, "d"), Some(4));
        assert_eq!(cache.len(), 3, "at the cap");
        assert_eq!(get(&mut cache, "a"), Some(1), "a was used last, so b went");
        assert_eq!(get(&mut cache, "b"), Some(5), "b is made again");
        for i in 0..200 {
            get(&mut cache, &format!("f{i}"));
        }
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get_or_make("x", || None), None, "a value that can't be made is not kept");
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn the_installed_list_is_sorted_and_has_the_default_font() {
        let dwrite = factory();
        let families = installed(&dwrite);
        assert!(families.len() > 5);
        assert!(families.windows(2).all(|w| w[0].to_lowercase() <= w[1].to_lowercase()));
        assert!(families.iter().any(|f| f == DEFAULT_FAMILY));
        assert!(is_installed(&dwrite, "bahnschrift"), "case-insensitive");
    }

    #[test]
    fn symbol_fonts_do_not_show_their_name_and_text_fonts_do() {
        let dwrite = factory();
        assert!(shows_name(&dwrite, "Segoe UI") && shows_name(&dwrite, "Consolas"));
        assert!(shows_name(&dwrite, "No Such Font 123"), "not installed: drawn in the fallback");
        let symbols: Vec<&str> = ["Wingdings", "Webdings", "Marlett", "Segoe MDL2 Assets", "Segoe Fluent Icons"].into_iter().filter(|f| is_installed(&dwrite, f)).collect();
        for family in &symbols {
            assert!(!shows_name(&dwrite, family), "{family}");
        }
        if symbols.is_empty() {
            eprintln!("no symbol font installed, skipped");
        }
    }

    #[test]
    fn a_missing_family_falls_back_to_the_default() {
        let dwrite = factory();
        assert_eq!(resolve(&dwrite, "No Such Font 123"), DEFAULT_FAMILY);
        assert_eq!(resolve(&dwrite, "Segoe UI"), "Segoe UI");
    }
}
