//! What the user launched from the list: how often, when, and after typing what. Feeds the search
//! ranking and the RECENT rows. Kept in `%LOCALAPPDATA%\cpmenu\usage.json`, next to the app index
//! cache, and written (temp file + rename) only when something is launched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::menu::Tag;

/// Entries kept; the least recently used go first.
pub const MAX_ENTRIES: usize = 500;
/// Query prefixes remembered per entry, newest first.
const MAX_PICKS: usize = 3;
/// Longer queries are not remembered as picks (they are not "prefixes" anymore).
const MAX_PICK_CHARS: usize = 24;

/// Frecency halves every two weeks without use.
const HALF_LIFE_DAYS: f64 = 14.0;
/// Most the use count and recency can add to a match score.
const USAGE_MAX: i32 = 30;
/// A remembered pick equal to the query / one the query is the start of.
const PICK_EXACT: i32 = 70;
const PICK_PREFIX: i32 = 35;
/// Most any usage can add. Smaller than the gap between match tiers in `search::score`
/// (prefix >= ~940, word start <= 800, inner <= 600), so usage reorders matches of
/// the same kind and never lifts a worse kind of match above a better one.
pub const MAX_BONUS: i32 = 100;

/// Kind letters; the first character of an entry's key.
pub const APP: char = 'a';
pub const SETTING: char = 's';
pub const FILE: char = 'f';
pub const FOLDER: char = 'd';

/// The kind letter for rows that have a stable id or path; `None` for the rest (calc, web, `>`, plugins).
pub fn kind(tag: Tag) -> Option<char> {
    match tag {
        Tag::App | Tag::Pinned => Some(APP),
        Tag::Setting => Some(SETTING),
        Tag::File => Some(FILE),
        Tag::Folder => Some(FOLDER),
        _ => None,
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    /// Kind letter followed by the item's id (app id, path or task name).
    k: String,
    /// Launch count.
    n: u32,
    /// Unix seconds of the last launch.
    t: u64,
    /// Lower case queries it was launched from, newest first.
    #[serde(default)]
    p: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct File {
    v: u32,
    items: Vec<Entry>,
}

#[derive(Debug, Default)]
pub struct Usage {
    entries: HashMap<String, Entry>,
}

fn key(kind: char, id: &str) -> String {
    format!("{kind}{id}")
}

impl Usage {
    pub fn path() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("cpmenu").join("usage.json"))
    }

    /// The stored usage; empty if there is none or the file is unreadable.
    pub fn load() -> Self {
        Self::path().map(|p| Self::load_from(&p)).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Self {
        let file = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok());
        let entries = file.filter(|f| f.v == 1).map(|f| f.items).unwrap_or_default();
        Self { entries: entries.into_iter().take(MAX_ENTRIES).map(|e| (e.k.clone(), e)).collect() }
    }

    pub fn save(&self) {
        if let Some(path) = Self::path() {
            let _ = self.save_to(&path);
        }
    }

    /// Write-then-rename so a crash never leaves a half-written file.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let mut items: Vec<Entry> = self.entries.values().cloned().collect();
        items.sort_by(|a, b| b.t.cmp(&a.t).then_with(|| a.k.cmp(&b.k)));
        let text = serde_json::to_vec(&File { v: 1, items }).map_err(std::io::Error::other)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, path)
    }

    /// Forgets everything and deletes the file.
    pub fn clear(&mut self) {
        self.entries.clear();
        if let Some(path) = Self::path() {
            let _ = std::fs::remove_file(path);
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// One launch of `id` from the list, after typing `query` (empty when launched from the plain list).
    pub fn record(&mut self, kind: char, id: &str, query: &str, now: u64) {
        let entry = self.entries.entry(key(kind, id)).or_insert_with(|| Entry { k: key(kind, id), n: 0, t: now, p: Vec::new() });
        entry.n = entry.n.saturating_add(1);
        entry.t = now;
        let query = query.trim().to_lowercase();
        if !query.is_empty() && query.chars().count() <= MAX_PICK_CHARS {
            entry.p.retain(|p| *p != query);
            entry.p.insert(0, query);
            entry.p.truncate(MAX_PICKS);
        }
        while self.entries.len() > MAX_ENTRIES {
            let oldest = self.entries.values().min_by(|a, b| a.t.cmp(&b.t).then_with(|| a.k.cmp(&b.k))).map(|e| e.k.clone());
            match oldest {
                Some(k) => self.entries.remove(&k),
                None => break,
            };
        }
    }

    /// Score added to a match of `query` (lower case) against `id`:
    /// `min(MAX_BONUS, usage + pick)` where
    /// - `usage = min(30, round(8 * ln(1 + count) * 0.5^(days since last use / 14)))`
    ///   (1 launch today: 6, 10 launches: 19, 40 launches: 30 and it halves every two weeks), and
    /// - `pick = 70` if the user launched it before after typing exactly `query`, `35` if after
    ///   typing something that starts with `query`, else 0.
    pub fn bonus(&self, kind: char, id: &str, query: &str, now: u64) -> i32 {
        let Some(e) = self.entries.get(&key(kind, id)) else { return 0 };
        let days = now.saturating_sub(e.t) as f64 / 86_400.0;
        let frecency = (1.0 + f64::from(e.n)).ln() * 0.5f64.powf(days / HALF_LIFE_DAYS);
        let usage = ((8.0 * frecency).round() as i32).min(USAGE_MAX);
        let pick = e
            .p
            .iter()
            .map(|p| if p == query { PICK_EXACT } else if !query.is_empty() && p.starts_with(query) { PICK_PREFIX } else { 0 })
            .max()
            .unwrap_or(0);
        (usage + pick).min(MAX_BONUS)
    }

    /// `(kind, id)` of everything used, most recent first.
    pub fn recent(&self) -> Vec<(char, &str)> {
        let mut all: Vec<&Entry> = self.entries.values().collect();
        all.sort_by(|a, b| b.t.cmp(&a.t).then_with(|| a.k.cmp(&b.k)));
        all.into_iter().filter_map(|e| Some((e.k.chars().next()?, &e.k[1..]))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cpmenu-usage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recording_counts_and_remembers_the_time() {
        let mut u = Usage::default();
        u.record(APP, "chrome", "", 100);
        u.record(APP, "chrome", "", 200);
        assert_eq!(u.entries["achrome"].n, 2);
        assert_eq!(u.entries["achrome"].t, 200);
        assert!(u.entries["achrome"].p.is_empty(), "no query, no pick");
        assert_eq!(u.recent(), vec![(APP, "chrome")]);
    }

    #[test]
    fn frecency_prefers_often_and_recent_and_decays() {
        let mut u = Usage::default();
        let now = 100 * DAY;
        for _ in 0..10 {
            u.record(APP, "often", "", now);
        }
        u.record(APP, "once", "", now);
        for _ in 0..10 {
            u.record(APP, "stale", "", now - 56 * DAY);
        }
        let b = |id| u.bonus(APP, id, "x", now);
        assert!(b("often") > b("once") && b("once") > 0, "{} {}", b("often"), b("once"));
        assert!(b("often") > b("stale"), "four half-lives later the same count is worth a lot less");
        assert_eq!(b("unknown"), 0);
        for _ in 0..1000 {
            u.record(APP, "often", "", now);
        }
        assert!(u.bonus(APP, "often", "x", now) <= USAGE_MAX, "usage alone is bounded");
    }

    #[test]
    fn prefix_picks_are_lowercase_bounded_and_boost_the_matching_query() {
        let mut u = Usage::default();
        for q in ["C", "ch", "chr", "chro"] {
            u.record(APP, "chrome", q, 10);
        }
        assert_eq!(u.entries["achrome"].p, vec!["chro", "chr", "ch"], "three newest, lower case");
        u.record(APP, "chrome", "chr", 10);
        assert_eq!(u.entries["achrome"].p, vec!["chr", "chro", "ch"], "a repeat moves to the front");
        u.record(APP, "chrome", &"x".repeat(40), 10);
        assert_eq!(u.entries["achrome"].p.len(), 3, "long queries are not picks");
        let exact = u.bonus(APP, "chrome", "chr", 10);
        let partial = u.bonus(APP, "chrome", "c", 10);
        let none = u.bonus(APP, "chrome", "z", 10);
        assert!(exact > partial && partial > none, "{exact} {partial} {none}");
        assert!(exact <= MAX_BONUS);
    }

    #[test]
    fn the_oldest_entries_are_dropped_past_the_cap() {
        let mut u = Usage::default();
        for i in 0..MAX_ENTRIES + 20 {
            u.record(FILE, &format!("f{i}"), "", i as u64);
        }
        assert_eq!(u.entries.len(), MAX_ENTRIES);
        assert!(!u.entries.contains_key("ff0") && !u.entries.contains_key("ff19"));
        assert!(u.entries.contains_key("ff20") && u.entries.contains_key(&format!("ff{}", MAX_ENTRIES + 19)));
        u.record(FILE, "f20", "", 10_000);
        u.record(FILE, "new", "", 10_001);
        assert!(u.entries.contains_key("ff20"), "a fresh launch protects an old entry");
        assert!(!u.entries.contains_key("ff21"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("sub").join("usage.json");
        let mut u = Usage::default();
        u.record(APP, r"C:\x\a.lnk", "a", 5);
        u.record(FOLDER, r"C:\x", "", 7);
        u.save_to(&path).unwrap();
        assert!(!path.with_extension("tmp").exists(), "the temp file was renamed");
        let back = Usage::load_from(&path);
        assert_eq!(back.entries, u.entries);
        assert_eq!(back.recent(), vec![(FOLDER, r"C:\x"), (APP, r"C:\x\a.lnk")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_corrupted_file_is_empty() {
        let dir = temp_dir("corrupt");
        let path = dir.join("usage.json");
        assert!(Usage::load_from(&path).is_empty());
        for bad in ["", "not json", "{\"v\":1,\"items\":[{\"k\":5}]}", "{\"v\":2,\"items\":[]}", "\u{0}\u{1}"] {
            std::fs::write(&path, bad).unwrap();
            assert!(Usage::load_from(&path).is_empty(), "{bad:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_rows_with_a_stable_identity_have_a_kind() {
        assert_eq!(kind(Tag::App), Some(APP));
        assert_eq!(kind(Tag::Pinned), Some(APP));
        assert_eq!(kind(Tag::Setting), Some(SETTING));
        for tag in [Tag::Calc, Tag::Web, Tag::Shell, Tag::Open, Tag::Action, Tag::Plugin(0)] {
            assert_eq!(kind(tag), None, "{tag:?}");
        }
    }
}
