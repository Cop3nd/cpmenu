//! Flow Launcher-style quick commands typed into the query:
//! `> cmd` runs in cmd, `? terms` searches the web, arithmetic is evaluated,
//! URLs and paths open directly.

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_EXECUTABLE, AssocQueryStringW, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use crate::apps;

#[derive(Debug, Clone, PartialEq)]
pub enum Quick {
    /// Command line for `cmd /k`.
    Shell(String),
    /// Expression and its formatted result.
    Calc(String, String),
    /// Search terms and the URL that searches for them; empty URL = the browser's own search engine.
    Web(String, String),
    /// URL or path to open with its default handler.
    Open(String),
    /// A plugin's keyword and a space came first: `text` is the rest, for plugin number `slot`.
    Plugin { slot: u16, text: String },
}

/// The plugin whose keyword starts `query` (keyword, then a space): its slot and the rest of the
/// query after one space. `keywords` holds `(keyword, slot)`.
fn plugin_keyword(query: &str, keywords: &[(String, u16)]) -> Option<(u16, String)> {
    let query = query.trim_start();
    keywords.iter().find_map(|(keyword, slot)| {
        let head = query.get(..keyword.len())?;
        let rest = query[keyword.len()..].strip_prefix(char::is_whitespace)?;
        head.eq_ignore_ascii_case(keyword).then(|| (*slot, rest.to_owned()))
    })
}

/// The quick command `query` asks for, if any. `search_url` (if configured) contains `{}` for the terms.
/// `keywords` are the plugin keywords as `(keyword, slot)`; `>` and `?` win over them.
pub fn detect(query: &str, search_url: Option<&str>, keywords: &[(String, u16)]) -> Option<Quick> {
    let raw = query;
    let query = query.trim();
    if let Some(command) = query.strip_prefix('>') {
        return Some(Quick::Shell(command.trim().to_owned()));
    }
    if let Some(terms) = query.strip_prefix('?') {
        let terms = terms.trim().to_owned();
        let url = search_url.map(|u| u.replace("{}", &encode(&terms))).unwrap_or_default();
        return Some(Quick::Web(terms, url));
    }
    if let Some((slot, text)) = plugin_keyword(raw, keywords) {
        return Some(Quick::Plugin { slot, text });
    }
    if let Some(target) = open_target(query) {
        return Some(Quick::Open(target));
    }
    calc(query).map(|result| Quick::Calc(query.to_owned(), result))
}

/// Percent-encodes everything but unreserved characters; spaces become `+`.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Executable stems (lowercase, no `.exe`) of browsers that treat a leading `? ` argument as a search.
const CHROMIUM: &[&str] = &[
    "chrome", "msedge", "brave", "vivaldi", "opera", "opera_gx", "launcher", "chromium", "thorium", "arc", "yandex", "browser",
];
/// Browsers that take `-search <terms>`.
const FIREFOX: &[&str] = &["firefox", "librewolf", "waterfox", "floorp", "zen", "mullvadbrowser"];

/// `arg` as one command-line argument, escaped the way `CommandLineToArgvW` reads it back.
pub fn quote_arg(arg: &str) -> String {
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                out.push(c);
            }
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                out.push(c);
            }
        }
    }
    // Backslashes right before the closing quote would escape it.
    out.extend(std::iter::repeat_n('\\', backslashes));
    out.push('"');
    out
}

/// Command-line arguments that make the browser with executable stem `stem` search for `terms`
/// with its own default engine; `None` for a browser we don't know the syntax of.
fn browser_args(stem: &str, terms: &str) -> Option<String> {
    let stem = stem.to_ascii_lowercase();
    let terms = terms.trim();
    if CHROMIUM.contains(&stem.as_str()) {
        return Some(if terms.is_empty() { String::new() } else { quote_arg(&format!("? {terms}")) });
    }
    if FIREFOX.contains(&stem.as_str()) {
        return Some(if terms.is_empty() { String::new() } else { format!("-search {}", quote_arg(terms)) });
    }
    None
}

/// Full path of the default `https` handler's executable.
fn default_browser() -> Option<String> {
    let mut buf = [0u16; 520];
    let mut len = buf.len() as u32;
    let result = unsafe {
        AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, w!("https"), w!("open"), Some(PWSTR(buf.as_mut_ptr())), &mut len)
    };
    if result.is_err() {
        return None;
    }
    // `len` counts the terminating NUL.
    Some(String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1).min(buf.len())]))
}

/// Runs a `?` search: `url` if one is configured, else the default browser with its own engine,
/// else DuckDuckGo in whatever opens URLs.
pub fn web_search(terms: &str, url: &str) {
    if !url.is_empty() {
        return apps::open_path(url);
    }
    let launched = default_browser().and_then(|exe| {
        let stem = std::path::Path::new(&exe).file_stem()?.to_str()?.to_owned();
        let args = browser_args(&stem, terms)?;
        // Chromium forwards its working directory to the running instance, so give it a real one
        // instead of whatever the menu was started in.
        let dir = HSTRING::from(std::env::var_os("USERPROFILE").unwrap_or_default());
        let dir = if dir.is_empty() { PCWSTR::null() } else { PCWSTR(dir.as_ptr()) };
        let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(exe), &HSTRING::from(args), dir, SW_SHOWNORMAL) };
        (result.0 as isize > 32).then_some(())
    });
    if launched.is_none() {
        apps::open_path(&format!("https://duckduckgo.com/?q={}", encode(terms)));
    }
}

/// Top-level domains accepted without a scheme. A list rather than "anything dotted",
/// because `notepad.exe` or `report.pdf` must stay ordinary searches.
const TLDS: &[&str] = &[
    "com", "org", "net", "io", "dev", "app", "pl", "eu", "de", "uk", "co", "me", "tv", "gg", "ai", "info", "xyz",
];

fn open_target(query: &str) -> Option<String> {
    if query.contains(char::is_whitespace) {
        return None;
    }
    let bytes = query.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/');
    if drive || query.starts_with(r"\\") {
        return Some(query.to_owned());
    }
    let lower = query.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(query.to_owned());
    }
    let host = lower.split(['/', '?', '#']).next()?;
    let host = host.split(':').next()?;
    let labels: Vec<&str> = host.split('.').collect();
    let valid = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && TLDS.contains(labels.last()?);
    valid.then(|| format!("https://{query}"))
}

/// Result of an arithmetic expression, or `None` if `query` isn't one.
/// Needs an operator so a plain number (or an app named "7zip") isn't a calculation.
fn calc(query: &str) -> Option<String> {
    if !query.contains(['+', '-', '*', '/', '%', '^', '(']) || !query.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut parser = Parser { s: query.as_bytes(), i: 0 };
    let value = parser.sum()?;
    parser.skip_spaces();
    if parser.i != parser.s.len() || !value.is_finite() {
        return None;
    }
    Some(format_number(value))
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let text = format!("{value:.10}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Recursive descent over `+ - * / % ^ ( )`, decimal point or comma.
struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn skip_spaces(&mut self) {
        while self.s.get(self.i) == Some(&b' ') {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip_spaces();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn sum(&mut self) -> Option<f64> {
        let mut value = self.product()?;
        loop {
            if self.eat(b'+') {
                value += self.product()?;
            } else if self.eat(b'-') {
                value -= self.product()?;
            } else {
                return Some(value);
            }
        }
    }

    fn product(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        loop {
            if self.eat(b'*') {
                value *= self.power()?;
            } else if self.eat(b'/') {
                value /= self.power()?;
            } else if self.eat(b'%') {
                value %= self.power()?;
            } else {
                return Some(value);
            }
        }
    }

    /// Right-associative, binds tighter than unary minus on its left: `-2^2 = -4`.
    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.eat(b'^') { Some(base.powf(self.power()?)) } else { Some(base) }
    }

    fn unary(&mut self) -> Option<f64> {
        if self.eat(b'-') {
            return Some(-self.power()?);
        }
        if self.eat(b'(') {
            let value = self.sum()?;
            return self.eat(b')').then_some(value);
        }
        self.number()
    }

    fn number(&mut self) -> Option<f64> {
        self.skip_spaces();
        let start = self.i;
        while self.s.get(self.i).is_some_and(|b| b.is_ascii_digit() || *b == b'.' || *b == b',') {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).ok()?.replace(',', ".");
        text.parse().ok()
    }
}

/// Opens a console running `command` that stays open to show its output.
pub fn run_shell(command: &str, admin: bool) {
    let params = HSTRING::from(format!("/k {command}"));
    let dir = HSTRING::from(std::env::var_os("USERPROFILE").unwrap_or_default());
    let verb = if admin { w!("runas") } else { w!("open") };
    unsafe { ShellExecuteW(None, verb, w!("cmd.exe"), &params, &dir, SW_SHOWNORMAL) };
}

/// Starts `exe` with `args` (each quoted, no shell string); `admin` asks for elevation (UAC prompt).
pub fn run_program(exe: &str, args: &[String], cwd: Option<&str>, admin: bool) {
    let params = HSTRING::from(args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" "));
    let params = if params.is_empty() { PCWSTR::null() } else { PCWSTR(params.as_ptr()) };
    let dir = HSTRING::from(cwd.unwrap_or_default());
    let dir = if dir.is_empty() { PCWSTR::null() } else { PCWSTR(dir.as_ptr()) };
    let verb = if admin { w!("runas") } else { w!("open") };
    unsafe { ShellExecuteW(None, verb, &HSTRING::from(exe), params, dir, SW_SHOWNORMAL) };
}

const CF_UNICODETEXT: u32 = 13;

/// The clipboard's text, or `None` if it holds no text or can't be opened.
pub fn clipboard_text() -> Option<String> {
    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|handle| {
            let memory = HGLOBAL(handle.0);
            let units = GlobalSize(memory) / 2;
            let source = GlobalLock(memory) as *const u16;
            if source.is_null() {
                return None;
            }
            let wide = std::slice::from_raw_parts(source, units);
            // Clipboard text is NUL-terminated; without a NUL the whole block is the text.
            let len = wide.iter().position(|&c| c == 0).unwrap_or(units);
            let text = String::from_utf16_lossy(&wide[..len]);
            let _ = GlobalUnlock(memory);
            Some(text)
        });
        let _ = CloseClipboard();
        text
    }
}

/// Clipboard text as query or field text: up to the first line break, control characters (tabs
/// too) as spaces, trimmed, and at most `max` characters.
pub fn clean_paste(text: &str, max: usize) -> String {
    let line = text.split(['\r', '\n']).next().unwrap_or_default();
    let spaced: String = line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    spaced.trim().chars().take(max).collect()
}

/// Puts `text` on the clipboard.
pub fn copy(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe {
        let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else { return };
        let dest = GlobalLock(memory) as *mut u16;
        if dest.is_null() {
            let _ = GlobalFree(Some(memory));
            return;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dest, wide.len());
        let _ = GlobalUnlock(memory);
        if OpenClipboard(None).is_err() {
            let _ = GlobalFree(Some(memory));
            return;
        }
        let _ = EmptyClipboard();
        // On success the clipboard owns the memory.
        if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0))).is_err() {
            let _ = GlobalFree(Some(memory));
        }
        let _ = CloseClipboard();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOGLE: Option<&str> = Some("https://www.google.com/search?q={}");

    #[test]
    fn prefixes_select_shell_and_web() {
        assert_eq!(detect(">  ipconfig /all", GOOGLE, &[]), Some(Quick::Shell("ipconfig /all".into())));
        assert_eq!(
            detect("? rust & co", GOOGLE, &[]),
            Some(Quick::Web("rust & co".into(), "https://www.google.com/search?q=rust+%26+co".into()))
        );
        assert_eq!(detect(">", GOOGLE, &[]), Some(Quick::Shell(String::new())));
    }

    fn keywords() -> Vec<(String, u16)> {
        vec![("gh".to_owned(), 3), ("calc".to_owned(), 4), (">".to_owned(), 5)]
    }

    #[test]
    fn plugin_keyword_needs_a_space_and_loses_to_prefixes() {
        let k = keywords();
        let plugin = |slot, text: &str| Some(Quick::Plugin { slot, text: text.into() });
        assert_eq!(detect("gh foo bar", GOOGLE, &k), plugin(3, "foo bar"));
        assert_eq!(detect("GH ", GOOGLE, &k), plugin(3, ""), "keyword and a space alone ask for the hints");
        assert_eq!(detect("  gh  x", GOOGLE, &k), plugin(3, " x"), "exactly one space is removed");
        assert_eq!(detect("gh", GOOGLE, &k), None, "no space yet: still an ordinary search");
        assert_eq!(detect("ghost", GOOGLE, &k), None);
        assert_eq!(detect("gz foo", GOOGLE, &k), None);
        assert_eq!(detect("> gh foo", GOOGLE, &k), Some(Quick::Shell("gh foo".into())), "> wins");
        assert_eq!(detect("? gh foo", GOOGLE, &k).map(|q| matches!(q, Quick::Web(..))), Some(true), "? wins");
        assert_eq!(detect("calc 1+1", GOOGLE, &k), plugin(4, "1+1"), "a keyword beats the calculator");
        assert_eq!(detect("żż foo", GOOGLE, &k), None, "multi-byte text before the keyword's length doesn't panic");
    }

    #[test]
    fn web_search_without_a_configured_url_has_no_url() {
        assert_eq!(detect("? rust", None, &[]), Some(Quick::Web("rust".into(), String::new())));
    }

    #[test]
    fn browser_family_picks_the_argument_style() {
        assert_eq!(browser_args("chrome", " rust lang ").as_deref(), Some("\"? rust lang\""));
        assert_eq!(browser_args("MSEdge", "a").as_deref(), Some("\"? a\""));
        assert_eq!(browser_args("firefox", "rust lang").as_deref(), Some("-search \"rust lang\""));
        assert_eq!(browser_args("librewolf", "x").as_deref(), Some("-search \"x\""));
        for stem in ["brave", "vivaldi", "opera", "launcher", "chromium", "thorium", "arc", "yandex"] {
            assert!(browser_args(stem, "x").unwrap().starts_with("\"? "), "{stem}");
        }
        for stem in ["waterfox", "floorp", "zen", "mullvadbrowser"] {
            assert!(browser_args(stem, "x").unwrap().starts_with("-search "), "{stem}");
        }
        assert_eq!(browser_args("iexplore", "x"), None);
        assert_eq!(browser_args("chrome", "").as_deref(), Some(""), "just open the browser");
    }

    #[test]
    fn quotes_in_terms_survive_the_command_line() {
        assert_eq!(quote_arg("plain"), "\"plain\"");
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_arg("a\\\"b"), "\"a\\\\\\\"b\"", "backslash before a quote is doubled");
        assert_eq!(quote_arg("dir\\"), "\"dir\\\\\"", "trailing backslash must not escape the closing quote");
        assert_eq!(browser_args("firefox", "5\" screen").as_deref(), Some("-search \"5\\\" screen\""));
    }

    #[test]
    fn arithmetic_is_evaluated() {
        let eval = |q: &str| calc(q);
        assert_eq!(eval("2*(3+4)").as_deref(), Some("14"));
        assert_eq!(eval("15,5 / 2").as_deref(), Some("7.75"));
        assert_eq!(eval("-2^2").as_deref(), Some("-4"));
        assert_eq!(eval("2^3^2").as_deref(), Some("512"));
        assert_eq!(eval("10 % 4").as_deref(), Some("2"));
        assert_eq!(eval("1/3").as_deref(), Some("0.3333333333"));
    }

    #[test]
    fn non_arithmetic_is_not_a_calculation() {
        for q in ["7", "7zip", "2+", "1/0", "(1+2", "a-b", "x264 - 1"] {
            assert_eq!(calc(q), None, "{q}");
        }
    }

    #[test]
    fn urls_and_paths_open_directly() {
        assert_eq!(open_target("github.com"), Some("https://github.com".into()));
        assert_eq!(open_target("doc.rust-lang.org/std"), Some("https://doc.rust-lang.org/std".into()));
        assert_eq!(open_target("http://localhost:8080"), Some("http://localhost:8080".into()));
        assert_eq!(open_target(r"C:\Users"), Some(r"C:\Users".into()));
        assert_eq!(open_target(r"\\nas\share"), Some(r"\\nas\share".into()));
        for q in ["notepad.exe", "main.rs", "report.pdf", "visual studio.com", "firefox"] {
            assert_eq!(open_target(q), None, "{q}");
        }
    }

    #[test]
    fn pasted_text_is_one_clean_line() {
        assert_eq!(clean_paste("first\nsecond", 256), "first");
        assert_eq!(clean_paste("first\r\nsecond", 256), "first");
        assert_eq!(clean_paste("\r\nsecond", 256), "");
        assert_eq!(clean_paste("a\tb\u{7}c\u{1b}", 256), "a b c");
        assert_eq!(clean_paste("  spaced out  ", 256), "spaced out");
        assert_eq!(clean_paste("żółw ąę", 256), "żółw ąę");
        assert_eq!(clean_paste("abcdef", 3), "abc");
        assert_eq!(clean_paste("ab\ncd", 0), "");
        assert_eq!(clean_paste("   ", 256), "");
    }

    #[test]
    fn plain_names_are_left_alone() {
        assert_eq!(detect("firefox", GOOGLE, &[]), None);
    }
}
