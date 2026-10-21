//! Plugin protocol v1 (docs/plugins.md section 2): the lines the host sends, the lines it accepts
//! and the sanitizing that turns untrusted plugin output into bounded, validated values.
//! Pure: no I/O except the bounded line reader over any `BufRead`.

use std::collections::BTreeMap;
use std::io::{self, BufRead};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::plugins::SettingValue;

pub const MAX_LINE: usize = 64 * 1024;
const MAX_TITLE: usize = 200;
const MAX_SUBTITLE: usize = 300;
const MAX_TAG: usize = 8;
const MAX_COPY: usize = 4 * 1024;
const MAX_DATA: usize = 4 * 1024;
const MAX_ACTIONS: usize = 8;
const MAX_LABEL: usize = 40;
const MAX_ID: usize = 64;
const MAX_EFFECTS: usize = 4;
const MAX_CLIPBOARD: usize = 64 * 1024;
const MAX_TARGET: usize = 2048;
const MAX_MESSAGE: usize = 2000;

/// Host to plugin. One JSON object per line.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToPlugin<'a> {
    Init {
        protocol: u32,
        host: &'a str,
        lang: &'a str,
        plugin_dir: &'a str,
        data_dir: &'a str,
        settings: &'a BTreeMap<String, SettingValue>,
    },
    Query {
        id: u64,
        text: &'a str,
        keyword: &'a str,
        global: bool,
        max: usize,
    },
    Action {
        req: u64,
        id: &'a str,
        item: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<&'a Value>,
        query_id: u64,
    },
    /// Sent when a setting is edited in the PLUGINS tab while the plugin runs.
    Settings {
        settings: &'a BTreeMap<String, SettingValue>,
    },
    Shutdown,
}

impl ToPlugin<'_> {
    /// The message as one `\n` terminated line.
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).unwrap_or_default();
        line.push('\n');
        line
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// A built-in effect the host runs itself (section 2.5).
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// http, https, mailto, or a path; whether a path exists is the executor's check.
    Open(String),
    Copy(String),
    Run { exe: String, args: Vec<String>, cwd: Option<String>, admin: bool },
    SetQuery(String),
    KeepOpen(bool),
    Hide,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub id: String,
    pub label: String,
    /// `Some`: the host runs these without asking the plugin. `None`: the plugin gets an `action` message.
    pub effects: Option<Vec<Effect>>,
    pub stay: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub tag: Option<String>,
    pub icon: Option<String>,
    pub score: u8,
    pub copy: Option<String>,
    pub data: Option<Value>,
    pub default: Option<String>,
    pub alt: Option<String>,
    pub actions: Vec<Action>,
}

impl Item {
    fn action(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.id == id)
    }

    /// What Enter runs: `default`, else the first action.
    pub fn default_action(&self) -> Option<&Action> {
        self.default.as_deref().and_then(|id| self.action(id)).or(self.actions.first())
    }

    /// What Ctrl+Enter runs.
    pub fn alt_action(&self) -> Option<&Action> {
        self.alt.as_deref().and_then(|id| self.action(id))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub req: u64,
    /// In order; a top-level `set_query` is appended as `SetQuery`.
    pub effects: Vec<Effect>,
    pub keep_open: bool,
    pub error: Option<String>,
}

/// Plugin to host, sanitized.
#[derive(Debug, PartialEq)]
pub enum FromPlugin {
    Ready { protocol: Option<u32> },
    Results { id: u64, items: Vec<Item> },
    Done(Done),
    Log { level: LogLevel, message: String },
    Error { id: Option<u64>, message: String },
    /// A type this host doesn't know; ignored (forward compatible).
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Wire {
    Ready {
        protocol: Option<u32>,
    },
    Results {
        id: u64,
        #[serde(default)]
        items: Value,
    },
    Done {
        req: u64,
        #[serde(rename = "do")]
        effects: Option<Value>,
        #[serde(default)]
        keep_open: bool,
        set_query: Option<String>,
        error: Option<String>,
    },
    Log {
        #[serde(default)]
        level: String,
        #[serde(default)]
        message: String,
    },
    Error {
        id: Option<u64>,
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Unknown,
}

/// Control characters become spaces, then the text is cut to `max` chars and trimmed.
fn clean(s: &str, max: usize) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect::<String>().trim().to_owned()
}

/// Cut at a char boundary to at most `max` bytes.
fn clip_bytes(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

fn text(o: &Map<String, Value>, key: &str, max: usize) -> Option<String> {
    Some(clean(o.get(key)?.as_str()?, max)).filter(|s| !s.is_empty())
}

/// http, https and mailto links, or anything that isn't a URL (a path); `file:`, `javascript:` and
/// every other scheme are refused. A single letter before `:` is a drive.
pub fn open_target_ok(target: &str) -> bool {
    if target.is_empty() || target.len() > MAX_TARGET || target.chars().any(char::is_control) {
        return false;
    }
    match target.split_once(':') {
        Some((scheme, rest)) if scheme.len() > 1 && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) => {
            match scheme.to_ascii_lowercase().as_str() {
                "http" | "https" => rest.strip_prefix("//").is_some_and(|host| !host.is_empty()),
                "mailto" => !rest.is_empty(),
                _ => false,
            }
        }
        _ => true,
    }
}

fn effect(value: &Value) -> Result<Effect, String> {
    let object = value.as_object().ok_or("an effect must be an object")?;
    let mut keys = object.iter();
    let (Some((name, arg)), None) = (keys.next(), keys.next()) else {
        return Err("an effect object has exactly one key".to_owned());
    };
    let string = |max: usize| arg.as_str().filter(|s| s.len() <= max).map(str::to_owned).ok_or_else(|| format!("{name}: expected a string of at most {max} bytes"));
    match name.as_str() {
        "open" => string(MAX_TARGET).and_then(|t| if open_target_ok(&t) { Ok(Effect::Open(t)) } else { Err("open: only http, https, mailto or a path".to_owned()) }),
        "copy" => string(MAX_CLIPBOARD).map(Effect::Copy),
        "set_query" => string(500).map(Effect::SetQuery),
        "keep_open" => arg.as_bool().map(Effect::KeepOpen).ok_or_else(|| "keep_open: expected true or false".to_owned()),
        "hide" => arg.as_bool().filter(|b| *b).map(|_| Effect::Hide).ok_or_else(|| "hide: expected true".to_owned()),
        "run" => {
            let run = arg.as_object().ok_or("run: expected an object")?;
            let exe = run.get("exe").and_then(Value::as_str).filter(|e| !e.is_empty() && e.len() <= 520 && !e.chars().any(char::is_control));
            let args = match run.get("args") {
                None | Some(Value::Null) => Some(Vec::new()),
                Some(Value::Array(a)) if a.len() <= 32 => a.iter().map(|v| v.as_str().filter(|s| s.len() <= MAX_TARGET && !s.contains('\0')).map(str::to_owned)).collect(),
                _ => None,
            };
            let cwd = match run.get("cwd") {
                None | Some(Value::Null) => Some(None),
                Some(Value::String(s)) if s.len() <= 520 && !s.contains('\0') => Some(Some(s.clone())),
                _ => None,
            };
            let admin = match run.get("admin") {
                None | Some(Value::Null) => Some(false),
                Some(Value::Bool(b)) => Some(*b),
                _ => None,
            };
            match (exe, args, cwd, admin) {
                (Some(exe), Some(args), Some(cwd), Some(admin)) => Ok(Effect::Run { exe: exe.to_owned(), args, cwd, admin }),
                _ => Err("run: needs a non-empty exe, an args array of strings, an optional cwd string and an optional admin flag".to_owned()),
            }
        }
        other => Err(format!("unknown effect {other:?}")),
    }
}

/// `do`: one effect object or an array of at most 4. Anything invalid rejects all of it, so a half
/// executed list can't happen.
pub fn parse_effects(value: &Value) -> Result<Vec<Effect>, String> {
    let list = match value {
        Value::Array(list) => list.as_slice(),
        single => std::slice::from_ref(single),
    };
    if list.is_empty() || list.len() > MAX_EFFECTS {
        return Err(format!("do: 1 to {MAX_EFFECTS} effects"));
    }
    list.iter().map(effect).collect()
}

fn action(value: &Value) -> Option<Action> {
    let o = value.as_object()?;
    let id = text(o, "id", MAX_ID)?;
    let label = text(o, "label", MAX_LABEL).unwrap_or_else(|| id.clone());
    let effects = match o.get("do") {
        None | Some(Value::Null) => None,
        Some(v) => Some(parse_effects(v).ok()?),
    };
    Some(Action { id, label, effects, stay: o.get("stay").and_then(Value::as_bool).unwrap_or(false) })
}

fn item(value: &Value) -> Option<Item> {
    let o = value.as_object()?;
    let title = text(o, "title", MAX_TITLE)?;
    let mut actions: Vec<Action> = Vec::new();
    for a in o.get("actions").and_then(Value::as_array).into_iter().flatten().filter_map(action) {
        if actions.len() < MAX_ACTIONS && !actions.iter().any(|x| x.id == a.id) {
            actions.push(a);
        }
    }
    let known = |key: &str| text(o, key, MAX_ID).filter(|id| actions.iter().any(|a| a.id == *id));
    let score = o.get("score").and_then(Value::as_f64).map_or(50, |s| s.round().clamp(0.0, 100.0) as u8);
    Some(Item {
        key: text(o, "key", MAX_ID).unwrap_or_default(),
        title,
        subtitle: text(o, "subtitle", MAX_SUBTITLE).unwrap_or_default(),
        tag: text(o, "tag", MAX_TAG).map(|t| t.to_uppercase()),
        icon: o.get("icon").and_then(Value::as_str).filter(|p| !p.is_empty() && p.len() <= 520 && !p.contains('\0')).map(str::to_owned),
        score,
        copy: o.get("copy").and_then(Value::as_str).filter(|c| !c.is_empty()).map(|c| clip_bytes(c, MAX_COPY)),
        data: o.get("data").filter(|d| !d.is_null() && d.to_string().len() <= MAX_DATA).cloned(),
        default: known("default"),
        alt: known("alt"),
        actions,
    })
}

/// The valid items of a `results.items` value, at most `max` of them, in order. A malformed item
/// is skipped; the others are kept.
pub fn sanitize_items(items: &Value, max: usize) -> Vec<Item> {
    items.as_array().into_iter().flatten().filter_map(item).take(max).collect()
}

fn done(req: u64, effects: Option<Value>, keep_open: bool, set_query: Option<String>, error: Option<String>) -> Result<Done, String> {
    let mut effects = effects.map(|v| parse_effects(&v)).transpose()?.unwrap_or_default();
    let mut keep_open = keep_open;
    for e in &effects {
        match e {
            Effect::KeepOpen(keep) => keep_open = *keep,
            Effect::Hide => keep_open = false,
            Effect::SetQuery(_) => keep_open = true,
            _ => {}
        }
    }
    if let Some(query) = set_query {
        effects.push(Effect::SetQuery(clip_bytes(&query, 500)));
        keep_open = true;
    }
    Ok(Done { req, effects, keep_open, error: error.map(|e| clean(&e, MAX_MESSAGE)).filter(|e| !e.is_empty()) })
}

/// Parses one line of plugin output. `Ok(None)` for a blank line; `Err` for anything that is not a
/// JSON object with a valid `type`, or whose `done` carries an invalid `do` (the caller logs it and
/// counts a strike). A leading BOM (PowerShell 5.1) is ignored. At most `max_items` result items are kept.
pub fn parse_line(line: &str, max_items: usize) -> Result<Option<FromPlugin>, String> {
    let line = line.trim_start_matches('\u{feff}').trim();
    if line.is_empty() {
        return Ok(None);
    }
    let wire: Wire = serde_json::from_str(line).map_err(|e| e.to_string())?;
    Ok(Some(match wire {
        Wire::Ready { protocol } => FromPlugin::Ready { protocol },
        Wire::Results { id, items } => FromPlugin::Results { id, items: sanitize_items(&items, max_items) },
        Wire::Done { req, effects, keep_open, set_query, error } => FromPlugin::Done(done(req, effects, keep_open, set_query, error)?),
        Wire::Log { level, message } => FromPlugin::Log {
            level: match level.as_str() {
                "debug" => LogLevel::Debug,
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                _ => LogLevel::Info,
            },
            message: clean(&message, MAX_MESSAGE),
        },
        Wire::Error { id, message } => FromPlugin::Error { id, message: clean(&message, MAX_MESSAGE) },
        Wire::Unknown => FromPlugin::Unknown,
    }))
}

/// What `read_bounded_line` found.
#[derive(Debug, PartialEq)]
pub enum Line {
    /// A line without its `\n` (and `\r`); invalid UTF-8 is replaced.
    Text(String),
    /// A line longer than the cap: its bytes were discarded up to the next `\n`.
    Oversized,
    Eof,
}

/// Reads one line of at most `cap` bytes. Memory use is bounded by `cap` plus the reader's buffer
/// however long the line is. A last line without `\n` counts as a line.
pub fn read_bounded_line<R: BufRead>(reader: &mut R, cap: usize) -> io::Result<Line> {
    let mut line: Vec<u8> = Vec::new();
    let mut skipping = false;
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(match (skipping, line.is_empty()) {
                (true, _) => Line::Oversized,
                (false, true) => Line::Eof,
                (false, false) => finish(line),
            });
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let part = &chunk[..newline.unwrap_or(chunk.len())];
        if !skipping {
            if line.len() + part.len() > cap {
                skipping = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(part);
            }
        }
        let used = newline.map_or(chunk.len(), |i| i + 1);
        reader.consume(used);
        if newline.is_some() {
            return Ok(if skipping { Line::Oversized } else { finish(line) });
        }
    }
}

fn finish(mut line: Vec<u8>) -> Line {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Line::Text(String::from_utf8_lossy(&line).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, Cursor};

    fn parse(line: &str) -> FromPlugin {
        parse_line(line, 20).unwrap().unwrap()
    }

    fn lines(bytes: &[u8], cap: usize) -> Vec<Line> {
        let mut reader = BufReader::with_capacity(7, Cursor::new(bytes.to_vec()));
        let mut out = Vec::new();
        loop {
            match read_bounded_line(&mut reader, cap).unwrap() {
                Line::Eof => return out,
                line => out.push(line),
            }
        }
    }

    fn text(s: &str) -> Line {
        Line::Text(s.to_owned())
    }

    #[test]
    fn reader_splits_lines_and_handles_crlf_and_a_last_line_without_newline() {
        assert_eq!(lines(b"one\ntwo\r\n\nthree", 100), vec![text("one"), text("two"), text(""), text("three")]);
        assert_eq!(lines(b"", 10), vec![]);
        assert_eq!(lines(b"\n", 10), vec![text("")]);
    }

    #[test]
    fn oversized_line_is_skipped_to_the_next_newline_and_the_next_line_is_read() {
        let mut data = vec![b'x'; 5000];
        data.extend_from_slice(b"\nok\n");
        assert_eq!(lines(&data, 100), vec![Line::Oversized, text("ok")]);
        assert_eq!(lines(b"exactly10!\n", 10), vec![text("exactly10!")]);
        assert_eq!(lines(b"eleven_chrs\nok\n", 10), vec![Line::Oversized, text("ok")]);
        assert_eq!(lines(&vec![b'y'; 300], 100), vec![Line::Oversized], "no newline before EOF");
        let mut two = vec![b'a'; 200];
        two.extend_from_slice(b"\n");
        two.extend(vec![b'b'; 200]);
        two.extend_from_slice(b"\nfine\n");
        assert_eq!(lines(&two, 100), vec![Line::Oversized, Line::Oversized, text("fine")]);
    }

    #[test]
    fn invalid_utf8_is_replaced_not_fatal() {
        assert_eq!(lines(b"a\xFFb\n", 10), vec![text("a\u{FFFD}b")]);
    }

    #[test]
    fn bom_blank_lines_unknown_types_and_fields_are_tolerated() {
        assert_eq!(parse("\u{feff}{\"type\":\"ready\",\"protocol\":1}"), FromPlugin::Ready { protocol: Some(1) });
        assert_eq!(parse_line("   ", 5), Ok(None));
        assert_eq!(parse(r#"{"type":"telemetry","x":1}"#), FromPlugin::Unknown);
        assert_eq!(parse(r#"{"type":"ready","extra":{"a":1}}"#), FromPlugin::Ready { protocol: None });
    }

    #[test]
    fn bad_lines_are_errors() {
        for line in ["not json", "{", "5", "[]", r#"{"no_type":1}"#, r#"{"type":"results"}"#, r#"{"type":"results","id":"x"}"#, r#"{"type":5}"#] {
            assert!(parse_line(line, 5).is_err(), "{line}");
        }
    }

    #[test]
    fn host_messages_serialize_as_in_the_design_doc() {
        let settings = BTreeMap::from([("user".to_owned(), SettingValue::String("jane".into())), ("forks".to_owned(), SettingValue::Bool(false))]);
        let init = ToPlugin::Init { protocol: 1, host: "cpmenu 0.1.0", lang: "pl", plugin_dir: r"C:\p\github", data_dir: r"C:\d\github", settings: &settings };
        let expected: Value = serde_json::from_str(
            r#"{"type":"init","protocol":1,"host":"cpmenu 0.1.0","lang":"pl","plugin_dir":"C:\\p\\github","data_dir":"C:\\d\\github","settings":{"user":"jane","forks":false}}"#,
        )
        .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&init.to_line()).unwrap(), expected);
        assert!(init.to_line().ends_with("}\n") && init.to_line().matches('\n').count() == 1);

        let query = ToPlugin::Query { id: 42, text: "cpmenu plugins", keyword: "gh", global: false, max: 8 };
        assert_eq!(
            serde_json::from_str::<Value>(&query.to_line()).unwrap(),
            json!({"type":"query","id":42,"text":"cpmenu plugins","keyword":"gh","global":false,"max":8})
        );
        let data = json!({"repo":"cpmenu/cpmenu"});
        let action = ToPlugin::Action { req: 7, id: "star", item: "r1", data: Some(&data), query_id: 42 };
        assert_eq!(
            serde_json::from_str::<Value>(&action.to_line()).unwrap(),
            json!({"type":"action","req":7,"id":"star","item":"r1","data":{"repo":"cpmenu/cpmenu"},"query_id":42})
        );
        let bare = ToPlugin::Action { req: 1, id: "a", item: "", data: None, query_id: 0 };
        assert!(!bare.to_line().contains("data"));
        assert_eq!(ToPlugin::Shutdown.to_line(), "{\"type\":\"shutdown\"}\n");
        assert_eq!(
            serde_json::from_str::<Value>(&ToPlugin::Settings { settings: &settings }.to_line()).unwrap(),
            json!({"type":"settings","settings":{"user":"jane","forks":false}})
        );
    }

    #[test]
    fn the_results_example_from_the_design_doc_parses() {
        let line = r#"{"type":"results","id":42,"items":[
          {"key":"r1","title":"cpmenu/cpmenu","subtitle":"Cyberpunk start menu - 120 stars",
           "tag":"REPO","icon":"C:\\Users\\me\\AppData\\Roaming\\cpmenu\\plugin-data\\github\\r1.png",
           "score":90,"copy":"https://github.com/cpmenu/cpmenu",
           "data":{"repo":"cpmenu/cpmenu"},
           "default":"open",
           "alt":"copy",
           "actions":[
             {"id":"open","label":"Open on GitHub","do":{"open":"https://github.com/cpmenu/cpmenu"}},
             {"id":"copy","label":"Copy URL","do":{"copy":"https://github.com/cpmenu/cpmenu"}},
             {"id":"star","label":"Star","stay":true}
           ]}
        ]}"#
        .replace('\n', "");
        let FromPlugin::Results { id, items } = parse(&line) else { panic!() };
        assert_eq!(id, 42);
        let item = &items[0];
        assert_eq!((item.key.as_str(), item.title.as_str(), item.tag.as_deref(), item.score), ("r1", "cpmenu/cpmenu", Some("REPO"), 90));
        assert_eq!(item.copy.as_deref(), Some("https://github.com/cpmenu/cpmenu"));
        assert_eq!(item.data, Some(json!({"repo":"cpmenu/cpmenu"})));
        assert_eq!(item.actions.len(), 3);
        assert_eq!(item.actions[0].effects, Some(vec![Effect::Open("https://github.com/cpmenu/cpmenu".into())]));
        assert_eq!((item.actions[2].effects.clone(), item.actions[2].stay), (None, true));
        assert_eq!(item.default_action().unwrap().id, "open");
        assert_eq!(item.alt_action().unwrap().id, "copy");
    }

    #[test]
    fn the_done_error_log_and_ready_examples_from_the_design_doc_parse() {
        assert_eq!(parse(r#"{"type":"ready","protocol":1}"#), FromPlugin::Ready { protocol: Some(1) });
        assert_eq!(
            parse(r#"{"type":"done","req":7,"do":[{"copy":"Starred cpmenu/cpmenu"}],"keep_open":false}"#),
            FromPlugin::Done(Done { req: 7, effects: vec![Effect::Copy("Starred cpmenu/cpmenu".into())], keep_open: false, error: None })
        );
        assert_eq!(
            parse(r#"{"type":"done","req":7,"set_query":"gh issues ","keep_open":true}"#),
            FromPlugin::Done(Done { req: 7, effects: vec![Effect::SetQuery("gh issues ".into())], keep_open: true, error: None })
        );
        assert_eq!(
            parse(r#"{"type":"done","req":7,"error":"Not logged in"}"#),
            FromPlugin::Done(Done { req: 7, effects: vec![], keep_open: false, error: Some("Not logged in".into()) })
        );
        assert_eq!(
            parse(r#"{"type":"log","level":"info","message":"fetched 8 repos in 230 ms"}"#),
            FromPlugin::Log { level: LogLevel::Info, message: "fetched 8 repos in 230 ms".into() }
        );
        assert_eq!(
            parse(r#"{"type":"error","id":42,"message":"HTTP 403 rate limited"}"#),
            FromPlugin::Error { id: Some(42), message: "HTTP 403 rate limited".into() }
        );
        assert_eq!(parse(r#"{"type":"log","level":"loud","message":"x\ny"}"#), FromPlugin::Log { level: LogLevel::Info, message: "x y".into() });
    }

    #[test]
    fn the_example_plugins_output_parses() {
        let py = r#"{"type": "results", "id": 3, "items": [{"key": "greet", "title": "Hello, world!", "subtitle": "Enter copies it, Right arrow for more", "score": 80, "copy": "Hello, world!", "data": {"who": "world"}, "default": "copy", "actions": [{"id": "copy", "label": "Copy greeting", "do": {"copy": "Hello, world!"}}, {"id": "docs", "label": "Open Python docs", "do": {"open": "https://docs.python.org/3/"}}, {"id": "shout", "label": "Shout it", "stay": true}]}]}"#;
        let FromPlugin::Results { items, .. } = parse(py) else { panic!() };
        assert_eq!(items[0].actions.len(), 3);
        let ps = r#"{"type":"done","req":9,"do":[{"copy":"2026-10-15T12:00:00.0000000+02:00"}]}"#;
        assert!(matches!(parse(ps), FromPlugin::Done(d) if d.effects.len() == 1));
        let single = r#"{"type":"results","id":1,"items":{"title":"unwrapped single item"}}"#;
        assert_eq!(parse(single), FromPlugin::Results { id: 1, items: vec![] }, "PowerShell unwraps one-element arrays into an object; that is not an array of items");
    }

    #[test]
    fn malformed_items_are_skipped_and_the_rest_kept() {
        let line = json!({"type":"results","id":1,"items":[
            {"title":"good"}, 5, "str", null, {"subtitle":"no title"}, {"title":""}, {"title":"   "}, {"title":"also good","score":"high"}
        ]})
        .to_string();
        let FromPlugin::Results { items, .. } = parse(&line) else { panic!() };
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["good", "also good"]);
        assert_eq!(items[1].score, 50, "a non-number score is the default");
    }

    #[test]
    fn items_are_capped_truncated_and_clamped() {
        let many: Vec<Value> = (0..30).map(|i| json!({"title": format!("t{i}")})).collect();
        let FromPlugin::Results { items, .. } = parse_line(&json!({"type":"results","id":1,"items":many}).to_string(), 8).unwrap().unwrap() else { panic!() };
        assert_eq!((items.len(), items[0].title.as_str(), items[7].title.as_str()), (8, "t0", "t7"));

        let long = "é".repeat(5000);
        let raw = json!({"type":"results","id":1,"items":[{
            "title": long, "subtitle": long, "tag": "toolongtag", "copy": long, "score": 500, "key": long,
            "data": {"blob": long}, "icon": ""
        }]});
        let FromPlugin::Results { items, .. } = parse(&raw.to_string()) else { panic!() };
        let i = &items[0];
        assert_eq!((i.title.chars().count(), i.subtitle.chars().count()), (200, 300));
        assert_eq!((i.tag.as_deref(), i.score), (Some("TOOLONGT"), 100));
        assert!(i.copy.as_ref().unwrap().len() <= 4096 && i.copy.as_ref().unwrap().chars().all(|c| c == 'é'), "cut on a char boundary");
        assert_eq!(i.key.chars().count(), 64);
        assert_eq!((i.data.clone(), i.icon.clone()), (None, None), "oversized data and an empty icon are dropped");
        let negative = json!({"type":"results","id":1,"items":[{"title":"a","score":-5},{"title":"b","score":49.6}]});
        let FromPlugin::Results { items, .. } = parse(&negative.to_string()) else { panic!() };
        assert_eq!((items[0].score, items[1].score), (0, 50));
    }

    #[test]
    fn control_characters_cannot_reach_the_row_text() {
        let raw = json!({"type":"results","id":1,"items":[{"title":"a\u{0}b\nc\u{202}","subtitle":"\t x \r"}]});
        let FromPlugin::Results { items, .. } = parse(&raw.to_string()) else { panic!() };
        assert_eq!((items[0].title.as_str(), items[0].subtitle.as_str()), ("a b c\u{202}", "x"));
    }

    #[test]
    fn actions_are_capped_unique_and_default_and_alt_must_exist() {
        let actions: Vec<Value> = (0..12).map(|i| json!({"id": format!("a{i}"), "label": format!("Action {i}")})).collect();
        let mut with_dup = actions.clone();
        with_dup.insert(1, json!({"id":"a0","label":"again"}));
        let raw = json!({"type":"results","id":1,"items":[{"title":"x","actions":with_dup,"default":"a2","alt":"nope"}]});
        let FromPlugin::Results { items, .. } = parse(&raw.to_string()) else { panic!() };
        let item = &items[0];
        assert_eq!(item.actions.len(), 8);
        assert_eq!(item.actions[0].label, "Action 0", "the first of a duplicated id wins");
        assert_eq!((item.default.as_deref(), item.alt.clone()), (Some("a2"), None));
        assert_eq!(item.default_action().unwrap().id, "a2");
        assert!(item.alt_action().is_none());

        let raw = json!({"type":"results","id":1,"items":[{"title":"x","default":"gone","actions":[{"id":"first","label":"F"},{"id":"bad","do":{"open":"file:///c:/x"}},{"label":"no id"},{"id":"nolabel"}]}]});
        let FromPlugin::Results { items, .. } = parse(&raw.to_string()) else { panic!() };
        let ids: Vec<_> = items[0].actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["first", "nolabel"], "an action with an invalid do or no id is dropped");
        assert_eq!(items[0].actions[1].label, "nolabel", "a missing label is the id");
        assert_eq!(items[0].default_action().unwrap().id, "first", "unknown default = the first action");
        let none = json!({"type":"results","id":1,"items":[{"title":"x"}]});
        let FromPlugin::Results { items, .. } = parse(&none.to_string()) else { panic!() };
        assert!(items[0].default_action().is_none());
    }

    fn fx(value: Value) -> Result<Vec<Effect>, String> {
        parse_effects(&value)
    }

    #[test]
    fn open_allows_web_links_mail_and_paths_only() {
        for ok in ["https://example.com", "HTTP://example.com/a?b=c", "mailto:a@b.c", r"C:\dir\file.txt", r"\\server\share\f", "relative\\p", "notes.txt", r"\\?\C:\x", "c:/x"] {
            assert!(open_target_ok(ok), "{ok}");
            assert_eq!(fx(json!({"open": ok})), Ok(vec![Effect::Open(ok.to_owned())]));
        }
        for bad in ["file:///c:/windows/system32/calc.exe", "javascript:alert(1)", "FILE:///x", "ms-settings:network", "ftp://x", "vbscript:x", "data:text/html,x", "http:nohost", "https://", "mailto:", "", "http://a\nb"] {
            assert!(!open_target_ok(bad), "{bad:?}");
            assert!(fx(json!({"open": bad})).is_err(), "{bad:?}");
        }
        assert!(fx(json!({"open": 5})).is_err());
    }

    #[test]
    fn effects_take_exactly_one_key_and_at_most_four() {
        assert!(fx(json!({"copy":"a","open":"https://x.y"})).is_err(), "two keys");
        assert!(fx(json!({})).is_err());
        assert!(fx(json!({"teleport":"x"})).is_err());
        assert!(fx(json!([])).is_err());
        assert!(fx(json!("copy")).is_err());
        assert!(fx(json!([{"copy":"1"},{"copy":"2"},{"copy":"3"},{"copy":"4"}])).is_ok());
        assert!(fx(json!([{"copy":"1"},{"copy":"2"},{"copy":"3"},{"copy":"4"},{"copy":"5"}])).is_err());
        assert!(fx(json!([{"copy":"1"},{"copy":"2","open":"https://x.y"}])).is_err(), "one bad effect rejects the list");
        assert_eq!(fx(json!([{"copy":"a"},{"set_query":"q"},{"hide":true}])), Ok(vec![Effect::Copy("a".into()), Effect::SetQuery("q".into()), Effect::Hide]));
        assert!(fx(json!({"hide":false})).is_err());
        assert_eq!(fx(json!({"keep_open":true})), Ok(vec![Effect::KeepOpen(true)]));
        assert!(fx(json!({"copy":"x".repeat(MAX_CLIPBOARD + 1)})).is_err());
        assert!(fx(json!({"copy":"x".repeat(MAX_CLIPBOARD)})).is_ok());
    }

    #[test]
    fn run_effect_validation() {
        assert_eq!(
            fx(json!({"run":{"exe":r"C:\tools\a.exe","args":["-x"],"cwd":r"C:\tools","admin":false}})),
            Ok(vec![Effect::Run { exe: r"C:\tools\a.exe".into(), args: vec!["-x".into()], cwd: Some(r"C:\tools".into()), admin: false }])
        );
        assert_eq!(fx(json!({"run":{"exe":"a.exe"}})), Ok(vec![Effect::Run { exe: "a.exe".into(), args: vec![], cwd: None, admin: false }]));
        for bad in [json!({"run":{}}), json!({"run":{"exe":""}}), json!({"run":{"exe":"a","args":"-x"}}), json!({"run":{"exe":"a","args":[1]}}), json!({"run":{"exe":"a","admin":"yes"}}), json!({"run":"a.exe"})] {
            assert!(fx(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn done_combines_top_level_and_listed_flow_effects() {
        let d = |s: &str| match parse(s) {
            FromPlugin::Done(d) => d,
            other => panic!("{other:?}"),
        };
        assert!(d(r#"{"type":"done","req":1}"#).effects.is_empty());
        let both = d(r#"{"type":"done","req":1,"do":{"copy":"a"},"set_query":"q"}"#);
        assert_eq!((both.effects, both.keep_open), (vec![Effect::Copy("a".into()), Effect::SetQuery("q".into())], true));
        assert!(d(r#"{"type":"done","req":1,"do":{"keep_open":true}}"#).keep_open);
        assert!(!d(r#"{"type":"done","req":1,"keep_open":true,"do":{"hide":true}}"#).keep_open);
        assert!(d(r#"{"type":"done","req":1,"do":{"set_query":"x"}}"#).keep_open, "set_query implies keep_open");
        assert_eq!(d(r#"{"type":"done","req":1,"error":""}"#).error, None);
        assert!(parse_line(r#"{"type":"done","req":1,"do":{"open":"javascript:x"}}"#, 5).is_err());
        assert!(parse_line(r#"{"type":"done","req":1,"do":{"copy":"a","hide":true}}"#, 5).is_err());
        assert!(parse_line(r#"{"type":"done"}"#, 5).is_err(), "req is required");
    }
}
