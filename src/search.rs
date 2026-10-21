//! Fuzzy matching of app names: prefix > word start > substring > scattered letters ("vsc").

fn lower(s: &str) -> Vec<char> {
    s.chars().flat_map(char::to_lowercase).collect()
}

fn is_word_start(name: &[char], i: usize) -> bool {
    i == 0 || !name[i - 1].is_alphanumeric()
}

/// Higher is better; `None` when the letters of `query` don't all appear in order.
pub fn score(query: &str, name: &str) -> Option<i32> {
    let (q, n) = (lower(query), lower(name));
    if q.is_empty() {
        return Some(0);
    }
    if n.starts_with(&q) {
        return Some(1000 - n.len() as i32);
    }
    let found = (0..n.len()).filter(|&i| n[i..].starts_with(&q));
    let (mut word, mut inner) = (None, None);
    for i in found {
        if is_word_start(&n, i) {
            word = word.or(Some(i));
        } else {
            inner = inner.or(Some(i));
        }
    }
    if let Some(i) = word {
        return Some(800 - i as i32);
    }
    if let Some(i) = inner {
        return Some(600 - i as i32);
    }

    // Scattered letters only count as initials or word prefixes ("vsc" -> Visual Studio Code):
    // each letter either continues the previous match or starts a word. Anything looser was noise.
    let mut total = 300;
    let mut next = 0;
    for c in q {
        let i = if n.get(next) == Some(&c) && next > 0 {
            total += 10;
            next
        } else {
            let i = (next..n.len()).find(|&i| n[i] == c && is_word_start(&n, i))?;
            total += 15 - (i - next) as i32;
            i
        };
        next = i + 1;
    }
    Some(total)
}

/// Indexes of matching items, best first; ties keep the input (alphabetical) order.
/// Each item is a name and an alias (the exe name, so "cmd" finds "Wiersz polecenia"; empty if none);
/// an item scores as its better match.
#[cfg(test)]
pub fn filter<'a>(query: &str, items: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<usize> {
    filter_with(query, items.map(|(name, alias)| (name, alias, 0)))
}

/// `filter` with a third value per item: a usage bonus added to its match score. It is clamped to
/// `usage::MAX_BONUS`, which is smaller than the gap between match kinds, so it only reorders
/// matches of the same kind.
pub fn filter_with<'a>(query: &str, items: impl Iterator<Item = (&'a str, &'a str, i32)>) -> Vec<usize> {
    let best = |name, alias: &str| {
        let by_alias = if alias.is_empty() { None } else { score(query, alias) };
        score(query, name).max(by_alias)
    };
    let mut hits: Vec<(i32, usize)> = items
        .enumerate()
        .filter_map(|(i, (name, alias, bonus))| Some((best(name, alias)? + bonus.clamp(0, crate::usage::MAX_BONUS), i)))
        .collect();
    hits.sort_by_key(|&(s, _)| std::cmp::Reverse(s));
    hits.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 6] = ["Discord", "Firefox", "Visual Studio Code", "Windows Terminal", "Ustawienia", "VLC"];

    fn top(query: &str) -> Vec<&'static str> {
        filter(query, NAMES.iter().map(|n| (*n, ""))).into_iter().map(|i| NAMES[i]).collect()
    }

    #[test]
    fn empty_query_keeps_everything_in_order() {
        assert_eq!(top(""), NAMES.to_vec());
    }

    #[test]
    fn prefix_beats_everything() {
        assert_eq!(top("fi")[0], "Firefox");
        assert_eq!(top("V")[0], "VLC", "shorter prefix match first");
    }

    #[test]
    fn word_start_beats_inner_substring() {
        assert_eq!(top("term")[0], "Windows Terminal");
        assert_eq!(top("co"), vec!["Visual Studio Code", "Discord"]);
    }

    #[test]
    fn initials_match_scattered_letters() {
        assert_eq!(top("vsc")[0], "Visual Studio Code");
    }

    #[test]
    fn case_and_non_ascii_are_handled() {
        assert_eq!(top("USTAW"), vec!["Ustawienia"]);
        assert_eq!(score("ŻÓŁ", "Zażółć"), Some(600 - 2));
    }

    #[test]
    fn letters_scattered_mid_word_do_not_match() {
        assert!(score("plan", "Prism Launcher").is_none());
        assert!(score("plan", "Panel Sterowania").is_none());
        assert!(score("wt", "Windows Terminal").is_some());
        assert!(score("wterm", "Windows Terminal").is_some());
    }

    #[test]
    fn alias_matches_and_ranks_by_the_better_score() {
        let items = [("Git CMD", ""), ("Wiersz polecenia", "cmd"), ("Notatnik", "notepad")];
        assert_eq!(filter("cmd", items.iter().copied()), vec![1, 0]);
        assert_eq!(filter("wiersz", items.iter().copied()), vec![1]);
    }

    #[test]
    fn usage_bonus_reorders_the_same_kind_of_match_but_never_a_worse_one_above_a_better_one() {
        // "ed": Edge and Edit are prefix matches, Notepad Editor only a word start.
        let names = [("Edge", "", 0), ("Notepad Editor", "", crate::usage::MAX_BONUS), ("Edit", "", 0)];
        assert_eq!(filter_with("ed", names.iter().copied()), vec![0, 2, 1], "a word-start match stays below prefix matches");
        let used = [("Edge", "", 0), ("Edit", "", 30)];
        assert_eq!(filter_with("ed", used.iter().copied()), vec![1, 0], "among prefix matches usage decides");
        let huge = [("Edge", "", 0), ("Notepad Editor", "", 100_000)];
        assert_eq!(filter_with("ed", huge.iter().copied()), vec![0, 1], "the bonus is clamped");
        assert_eq!(filter_with("zz", used.iter().copied()), Vec::<usize>::new(), "no match, no bonus");
    }

    #[test]
    fn missing_letters_do_not_match() {
        assert!(top("xyz").is_empty());
        assert!(score("xf", "Firefox").is_none());
        assert!(score("fx", "Firefox").is_none(), "x is mid-word, not an initial");
    }
}
