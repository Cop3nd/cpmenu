//! Plugin manifests: discovery under `%APPDATA%\cpmenu\plugins`, validation and the fixed command
//! lines a manifest's `run` expands to. Pure data and file reads; no process is started here
//! (`plugin_proc`) and nothing here talks the protocol (`plugin_proto`). See docs/plugins.md.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::PluginEntry;

/// Highest protocol version this build speaks.
pub const HOST_PROTOCOL: u32 = 1;
const MANIFEST_MAX_BYTES: u64 = 64 * 1024;
const ICON_MAX_BYTES: u64 = 256 * 1024;
const MAX_PLUGINS: usize = 255;
const MAX_SETTINGS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Interpreter {
    Python,
    Node,
    PowerShell,
    Pwsh,
}

impl Interpreter {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "python" => Self::Python,
            "node" => Self::Node,
            "powershell" => Self::PowerShell,
            "pwsh" => Self::Pwsh,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Node => "node",
            Self::PowerShell => "powershell",
            Self::Pwsh => "pwsh",
        }
    }
}

/// `run` of a manifest: a file inside the plugin folder, optionally started by a fixed interpreter.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub interpreter: Option<Interpreter>,
    pub file: String,
    pub args: Vec<String>,
}

impl Run {
    /// The command as the user sees it in the approval prompt and as part of the approval key.
    pub fn display(&self) -> String {
        let head = self.interpreter.map(|i| i.name().to_owned());
        head.into_iter().chain([self.file.clone()]).chain(self.args.iter().cloned()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SettingValue {
    String(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    String,
    Bool,
    Choice(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: String,
    pub label: String,
    pub kind: SettingKind,
    pub default: SettingValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub protocol_min: u32,
    /// Default activation keywords, first one is the default shown in settings.
    pub keywords: Vec<String>,
    pub global: bool,
    /// Upper case, at most 8 chars.
    pub tag: String,
    /// Relative to the plugin folder; `discover` drops it when the file is missing or too big.
    pub icon: Option<String>,
    pub run: Run,
    pub max_results: usize,
    pub debounce_ms: u64,
    pub query_timeout_ms: u64,
    pub settings: Vec<Setting>,
}

#[derive(Deserialize)]
struct RawRun {
    interpreter: Option<String>,
    file: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Deserialize)]
struct RawSetting {
    key: String,
    label: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    default: Option<Value>,
    choices: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Raw {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    protocol_min: Option<i64>,
    keyword: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    global: bool,
    tag: Option<String>,
    icon: Option<String>,
    run: RawRun,
    max_results: Option<i64>,
    debounce_ms: Option<i64>,
    query_timeout_ms: Option<i64>,
    #[serde(default)]
    settings: Vec<RawSetting>,
}

fn name_chars(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A plugin id and its folder name: `[a-z0-9_-]{1,32}`.
pub fn valid_id(s: &str) -> bool {
    name_chars(s, 32)
}

/// An activation keyword: `[a-z0-9_-]{1,16}`, which keeps `>` and `?` for quick commands.
pub fn valid_keyword(s: &str) -> bool {
    name_chars(s, 16)
}

fn valid_setting_key(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A path that stays inside the plugin folder: relative, no `..`, no drive or stream (`:`).
fn inside_folder(rel: &str) -> bool {
    let path = Path::new(rel);
    !rel.contains(':')
        && path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && path.components().any(|c| matches!(c, Component::Normal(_)))
}

fn clip(s: &str, max_chars: usize) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(max_chars).collect()
}

fn clamp(value: Option<i64>, default: i64, low: i64, high: i64) -> i64 {
    value.unwrap_or(default).clamp(low, high)
}

fn setting(raw: RawSetting) -> Result<Setting, String> {
    if !valid_setting_key(&raw.key) {
        return Err(format!("setting key {:?} is invalid", raw.key));
    }
    let label = raw.label.map(|l| clip(&l, 60)).filter(|l| !l.is_empty()).unwrap_or_else(|| raw.key.clone());
    let wrong = |what: &str| format!("setting {:?}: default must be {what}", raw.key);
    let (kind, default) = match raw.kind.as_str() {
        "string" => match &raw.default {
            None | Some(Value::Null) => (SettingKind::String, SettingValue::String(String::new())),
            Some(Value::String(s)) => (SettingKind::String, SettingValue::String(clip(s, 1000))),
            Some(_) => return Err(wrong("a string")),
        },
        "bool" => match &raw.default {
            None | Some(Value::Null) => (SettingKind::Bool, SettingValue::Bool(false)),
            Some(Value::Bool(b)) => (SettingKind::Bool, SettingValue::Bool(*b)),
            Some(_) => return Err(wrong("true or false")),
        },
        "choice" => {
            let choices: Vec<String> = raw.choices.unwrap_or_default().iter().map(|c| clip(c, 64)).collect();
            if choices.is_empty() || choices.len() > 32 || choices.iter().any(String::is_empty) {
                return Err(format!("setting {:?}: choices must be 1 to 32 non-empty strings", raw.key));
            }
            let default = match &raw.default {
                None | Some(Value::Null) => choices[0].clone(),
                Some(Value::String(s)) if choices.contains(s) => s.clone(),
                Some(_) => return Err(wrong("one of the choices")),
            };
            (SettingKind::Choice(choices), SettingValue::String(default))
        }
        other => return Err(format!("setting {:?}: unknown type {other:?}", raw.key)),
    };
    Ok(Setting { key: raw.key, label, kind, default })
}

/// Parses and validates `plugin.json` text for the plugin whose folder is `folder`.
pub fn parse_manifest(text: &str, folder: &str) -> Result<Manifest, String> {
    let raw: Raw = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("plugin.json: {e}"))?;
    if !valid_id(&raw.id) {
        return Err(format!("id {:?} must be 1 to 32 characters of a-z 0-9 _ -", raw.id));
    }
    if raw.id != folder {
        return Err(format!("id {:?} must equal the folder name {folder:?}", raw.id));
    }
    let name = clip(&raw.name, 64);
    let version = clip(&raw.version, 32);
    if name.is_empty() || version.is_empty() {
        return Err("name and version are required".to_owned());
    }
    let protocol_min = clamp(raw.protocol_min, 1, 1, u32::MAX as i64) as u32;
    if protocol_min > HOST_PROTOCOL {
        return Err("needs a newer cpmenu".to_owned());
    }

    let mut keywords: Vec<String> = Vec::new();
    for keyword in raw.keyword.into_iter().chain(raw.keywords) {
        if !valid_keyword(&keyword) {
            return Err(format!("keyword {keyword:?} must be 1 to 16 characters of a-z 0-9 _ -"));
        }
        if !keywords.contains(&keyword) {
            keywords.push(keyword);
        }
    }
    if keywords.len() > 8 {
        return Err("at most 8 keywords".to_owned());
    }
    if keywords.is_empty() && !raw.global {
        return Err("needs a keyword, or \"global\": true".to_owned());
    }

    let tag: String = raw.tag.as_deref().map_or_else(|| name.clone(), |t| clip(t, 64)).to_uppercase().chars().take(8).collect();
    let tag = if tag.is_empty() { name.to_uppercase().chars().take(8).collect() } else { tag };

    let icon = match raw.icon {
        Some(icon) if inside_folder(&icon) => Some(icon),
        Some(icon) => return Err(format!("icon {icon:?} must be a relative path inside the plugin folder")),
        None => None,
    };

    let interpreter = match raw.run.interpreter.as_deref() {
        None => None,
        Some(name) => Some(Interpreter::parse(name).ok_or_else(|| format!("unknown interpreter {name:?} (python, node, powershell, pwsh)"))?),
    };
    if !inside_folder(&raw.run.file) {
        return Err(format!("run.file {:?} must be a relative path inside the plugin folder", raw.run.file));
    }
    if raw.run.args.len() > 16 || raw.run.args.iter().any(|a| a.len() > 256 || a.contains('\0')) {
        return Err("run.args: at most 16 arguments of 256 characters".to_owned());
    }

    if raw.settings.len() > MAX_SETTINGS {
        return Err(format!("at most {MAX_SETTINGS} settings"));
    }
    let settings = raw.settings.into_iter().map(setting).collect::<Result<Vec<_>, _>>()?;
    let mut seen = BTreeSet::new();
    if let Some(dup) = settings.iter().find(|s| !seen.insert(s.key.as_str())) {
        return Err(format!("setting {:?} is listed twice", dup.key));
    }

    Ok(Manifest {
        id: raw.id,
        name,
        version,
        author: clip(&raw.author, 64),
        description: clip(&raw.description, 300),
        protocol_min,
        keywords,
        global: raw.global,
        tag,
        icon,
        run: Run { interpreter, file: raw.run.file, args: raw.run.args },
        max_results: clamp(raw.max_results, 8, 1, 20) as usize,
        debounce_ms: clamp(raw.debounce_ms, 120, 0, 500) as u64,
        query_timeout_ms: clamp(raw.query_timeout_ms, 1500, 200, 5000) as u64,
        settings,
    })
}

/// A program and its arguments, exactly as handed to `CreateProcess` (no shell).
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub exe: String,
    pub args: Vec<String>,
}

/// How Python is started on this machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Python {
    /// `python`
    Direct,
    /// `py -3`, when `python` is missing or is the Microsoft Store alias.
    Launcher,
}

/// The fixed command line for a manifest's `run`, for a plugin in `dir`.
pub fn expand(run: &Run, dir: &Path, python: Python) -> Command {
    let file = dir.join(&run.file).to_string_lossy().into_owned();
    let strings = |parts: &[&str]| parts.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let (exe, mut args) = match run.interpreter {
        None => return Command { exe: file, args: run.args.clone() },
        Some(Interpreter::Python) => match python {
            Python::Direct => ("python", strings(&["-u", "-X", "utf8"])),
            Python::Launcher => ("py", strings(&["-3", "-u", "-X", "utf8"])),
        },
        Some(Interpreter::Node) => ("node", Vec::new()),
        Some(Interpreter::PowerShell) => ("powershell.exe", strings(&["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])),
        Some(Interpreter::Pwsh) => ("pwsh.exe", strings(&["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])),
    };
    args.push(file);
    args.extend(run.args.iter().cloned());
    Command { exe: exe.to_owned(), args }
}

fn find_on_path(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var).map(|dir| dir.join(format!("{name}.exe"))).find(|p| p.is_file())
}

/// `py -3` when `python` is missing, or is the Store alias (a stub in `WindowsApps` that opens the
/// Store when Python isn't installed) and the launcher exists; otherwise `python`.
pub fn python_flavor(path_var: &OsStr) -> Python {
    let has_launcher = || find_on_path("py", path_var).is_some();
    match find_on_path("python", path_var) {
        None if has_launcher() => Python::Launcher,
        Some(found) if found.components().any(|c| c.as_os_str().eq_ignore_ascii_case("WindowsApps")) && has_launcher() => Python::Launcher,
        _ => Python::Direct,
    }
}

/// Extra environment every plugin process gets.
pub fn process_env(id: &str, dir: &Path, data_dir: &Path, protocol: u32) -> Vec<(String, String)> {
    vec![
        ("CPMENU_PROTOCOL".to_owned(), protocol.to_string()),
        ("CPMENU_PLUGIN_ID".to_owned(), id.to_owned()),
        ("CPMENU_PLUGIN_DIR".to_owned(), dir.to_string_lossy().into_owned()),
        ("CPMENU_DATA_DIR".to_owned(), data_dir.to_string_lossy().into_owned()),
    ]
}

/// A plugin folder as found on disk; a broken manifest keeps its error to show in the PLUGINS tab.
#[derive(Debug, PartialEq)]
pub struct Found {
    pub folder: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
}

fn appdata() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("cpmenu"))
}

/// `%APPDATA%\cpmenu\plugins`
pub fn root() -> Option<PathBuf> {
    Some(appdata()?.join("plugins"))
}

/// `%APPDATA%\cpmenu\plugin-data\<id>`: the plugin's private writable directory.
pub fn data_dir(id: &str) -> Option<PathBuf> {
    Some(appdata()?.join("plugin-data").join(id))
}

/// `%APPDATA%\cpmenu\plugin-logs\<id>.log`
pub fn log_path(id: &str) -> Option<PathBuf> {
    Some(appdata()?.join("plugin-logs").join(format!("{id}.log")))
}

/// Reads and validates `dir\plugin.json` for the plugin whose folder is `folder`, including that
/// `run.file` exists; an icon that is missing or too big is dropped.
pub fn load(dir: &Path, folder: &str) -> Result<Manifest, String> {
    let file = dir.join("plugin.json");
    let meta = std::fs::metadata(&file).map_err(|e| format!("plugin.json: {e}"))?;
    if meta.len() > MANIFEST_MAX_BYTES {
        return Err("plugin.json is larger than 64 KB".to_owned());
    }
    let text = std::fs::read_to_string(&file).map_err(|e| format!("plugin.json: {e}"))?;
    let mut manifest = parse_manifest(&text, folder)?;
    if !dir.join(&manifest.run.file).is_file() {
        return Err(format!("run.file {:?} not found", manifest.run.file));
    }
    manifest.icon = manifest.icon.filter(|icon| std::fs::metadata(dir.join(icon)).is_ok_and(|m| m.is_file() && m.len() <= ICON_MAX_BYTES));
    Ok(manifest)
}

/// Every `root\*\plugin.json`, sorted by folder name. Folders without a manifest are not plugins.
pub fn discover(root: &Path) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut found: Vec<Found> = entries
        .flatten()
        .filter(|e| e.path().join("plugin.json").exists())
        .filter_map(|e| {
            let folder = e.file_name().into_string().ok()?;
            let dir = e.path();
            let manifest = load(&dir, &folder);
            Some(Found { folder, dir, manifest })
        })
        .collect();
    found.sort_by(|a, b| a.folder.cmp(&b.folder));
    found.truncate(MAX_PLUGINS);
    found
}

/// Modification times of the plugins directory and every manifest; equal stamps mean nothing to rescan.
pub fn stamp(root: &Path) -> Vec<(String, Option<SystemTime>)> {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut stamp = vec![(String::new(), modified(root))];
    if let Ok(entries) = std::fs::read_dir(root) {
        stamp.extend(entries.flatten().filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            Some((name, modified(&e.path().join("plugin.json"))))
        }));
    }
    stamp.sort_by(|a, b| a.0.cmp(&b.0));
    stamp
}

/// The keywords a plugin answers to: the user's single override if it is valid, else the manifest's.
pub fn effective_keywords(manifest: &Manifest, entry: Option<&PluginEntry>) -> Vec<String> {
    match entry.and_then(|e| e.keyword.as_deref()).map(str::trim).filter(|k| valid_keyword(k)) {
        Some(keyword) => vec![keyword.to_owned()],
        None => manifest.keywords.clone(),
    }
}

/// `(plugin id, keyword)` for every plugin that loses a keyword to a plugin with a lower id.
pub fn keyword_losers<'a>(entries: impl IntoIterator<Item = (&'a str, &'a [String])>) -> Vec<(String, String)> {
    let mut sorted: Vec<_> = entries.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
    let mut losers = Vec::new();
    for (id, keywords) in sorted {
        for keyword in keywords {
            match owner.get(keyword.as_str()) {
                Some(first) if *first != id => losers.push((id.to_owned(), keyword.clone())),
                _ => {
                    owner.insert(keyword, id);
                }
            }
        }
    }
    losers
}

/// What the user must approve: any change of version or run command asks again.
pub fn approval_key(manifest: &Manifest) -> String {
    format!("{}|{}", manifest.version, manifest.run.display())
}

pub fn is_approved(manifest: &Manifest, entry: Option<&PluginEntry>) -> bool {
    entry.and_then(|e| e.approved.as_deref()) == Some(approval_key(manifest).as_str())
}

/// Enabled unless the user switched it off.
pub fn is_enabled(entry: Option<&PluginEntry>) -> bool {
    entry.is_none_or(|e| e.enabled)
}

/// The settings sent to the plugin: its declared defaults, replaced by the user's values that fit
/// the declared type. Values for settings the manifest doesn't declare are not sent.
pub fn effective_settings(manifest: &Manifest, entry: Option<&PluginEntry>) -> BTreeMap<String, SettingValue> {
    manifest
        .settings
        .iter()
        .map(|s| {
            let user = entry.and_then(|e| e.settings.get(&s.key));
            let value = match (&s.kind, user) {
                (SettingKind::String, Some(v @ SettingValue::String(_))) | (SettingKind::Bool, Some(v @ SettingValue::Bool(_))) => v.clone(),
                (SettingKind::Choice(choices), Some(SettingValue::String(v))) if choices.contains(v) => SettingValue::String(v.clone()),
                _ => s.default.clone(),
            };
            (s.key.clone(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = r#"{"id":"hello","name":"Hello","version":"1.0.0","keyword":"hello","run":{"interpreter":"python","file":"main.py"}}"#;

    fn ok(text: &str) -> Manifest {
        parse_manifest(text, "hello").unwrap()
    }

    fn err(text: &str) -> String {
        parse_manifest(text, "hello").unwrap_err()
    }

    fn with(field: &str) -> String {
        MIN.replacen('{', &format!("{{{field},"), 1)
    }

    #[test]
    fn minimal_manifest_gets_the_documented_defaults() {
        let m = ok(MIN);
        assert_eq!((m.protocol_min, m.max_results, m.debounce_ms, m.query_timeout_ms), (1, 8, 120, 1500));
        assert_eq!((m.tag.as_str(), m.keywords.clone(), m.global), ("HELLO", vec!["hello".to_owned()], false));
        assert_eq!(m.run, Run { interpreter: Some(Interpreter::Python), file: "main.py".to_owned(), args: vec![] });
    }

    #[test]
    fn full_manifest_from_the_design_doc() {
        let text = r#"{
          "id": "github", "name": "GitHub", "version": "1.2.0", "author": "Jane Doe",
          "description": "Search repositories and issues", "protocol_min": 1,
          "keyword": "gh", "keywords": ["gh", "git"], "global": false, "tag": "gh", "icon": "icon.png",
          "run": { "interpreter": "python", "file": "main.py", "args": [] },
          "max_results": 8, "debounce_ms": 120, "query_timeout_ms": 1500,
          "settings": [
            { "key": "user",  "label": "GitHub user",  "type": "string", "default": "" },
            { "key": "forks", "label": "Include forks", "type": "bool",   "default": false },
            { "key": "sort",  "label": "Sort by",       "type": "choice", "default": "updated", "choices": ["updated", "stars", "name"] }
          ]
        }"#;
        let m = parse_manifest(text, "github").unwrap();
        assert_eq!((m.keywords, m.tag.as_str(), m.icon.as_deref()), (vec!["gh".to_owned(), "git".to_owned()], "GH", Some("icon.png")));
        assert_eq!(m.settings.len(), 3);
        assert_eq!(m.settings[2].kind, SettingKind::Choice(vec!["updated".into(), "stars".into(), "name".into()]));
        assert_eq!(m.settings[2].default, SettingValue::String("updated".into()));
    }

    #[test]
    fn invalid_manifests_say_why() {
        let cases: &[(&str, &str)] = &[
            ("{", "plugin.json"),
            (r#"{"id":"hello"}"#, "missing field"),
            (&MIN.replace("\"hello\",\"name\"", "\"Hello\",\"name\""), "id"),
            (&MIN.replace("\"id\":\"hello\"", "\"id\":\"other\""), "folder"),
            (&MIN.replace("\"id\":\"hello\"", "\"id\":\"\""), "id"),
            (&MIN.replace("\"name\":\"Hello\"", "\"name\":\"  \""), "required"),
            (&MIN.replace("\"keyword\":\"hello\"", "\"keyword\":\">\""), "keyword"),
            (&MIN.replace("\"keyword\":\"hello\"", "\"keyword\":\"?\""), "keyword"),
            (&MIN.replace("\"keyword\":\"hello\"", "\"keyword\":\"Upper\""), "keyword"),
            (&MIN.replace("\"keyword\":\"hello\"", "\"keyword\":\"waytoolongforakeyword\""), "keyword"),
            (&MIN.replace("\"keyword\":\"hello\",", ""), "keyword"),
            (&MIN.replace("python", "ruby"), "interpreter"),
            (&MIN.replace("main.py", "..\\\\evil.py"), "run.file"),
            (&MIN.replace("main.py", "C:\\\\evil.py"), "run.file"),
            (&MIN.replace("main.py", "/evil.py"), "run.file"),
            (&MIN.replace("main.py", "a.py:stream"), "run.file"),
            (&with(r#""icon":"..\\x.png""#), "icon"),
            (&with(r#""protocol_min":2"#), "newer cpmenu"),
            (&with(r#""settings":[{"key":"a","type":"secret"}]"#), "unknown type"),
            (&with(r#""settings":[{"key":"a","type":"bool","default":"yes"}]"#), "default"),
            (&with(r#""settings":[{"key":"a","type":"choice"}]"#), "choices"),
            (&with(r#""settings":[{"key":"a","type":"choice","choices":["x"],"default":"y"}]"#), "default"),
            (&with(r#""settings":[{"key":"a","type":"bool"},{"key":"a","type":"bool"}]"#), "twice"),
            (&with(r#""settings":[{"key":"bad key","type":"bool"}]"#), "key"),
        ];
        for (text, needle) in cases {
            let message = err(text);
            assert!(message.contains(needle), "{text} -> {message}");
        }
        let many = format!(r#"{{"settings":[{}],"#, vec![r#"{"key":"k","type":"bool"}"#; 17].join(","));
        assert!(err(&MIN.replacen('{', &many, 1)).contains("at most 16"));
    }

    #[test]
    fn global_plugin_needs_no_keyword_and_a_bom_is_tolerated() {
        let text = "\u{feff}{\"id\":\"hello\",\"name\":\"Hello\",\"version\":\"1\",\"global\":true,\"run\":{\"file\":\"h.exe\"}}";
        let m = ok(text);
        assert!(m.global && m.keywords.is_empty());
        assert_eq!(m.run.interpreter, None);
    }

    #[test]
    fn numbers_are_clamped_and_tag_is_upper_cased_and_cut() {
        let m = ok(&with(r#""max_results":99,"debounce_ms":-5,"query_timeout_ms":1,"tag":"verylongtag""#));
        assert_eq!((m.max_results, m.debounce_ms, m.query_timeout_ms, m.tag.as_str()), (20, 0, 200, "VERYLONG"));
        let m = ok(&with(r#""max_results":0,"debounce_ms":9999,"query_timeout_ms":99999"#));
        assert_eq!((m.max_results, m.debounce_ms, m.query_timeout_ms), (1, 500, 5000));
    }

    #[test]
    fn keywords_merge_dedupe_and_keep_the_first_as_default() {
        let m = ok(&with(r#""keywords":["h","hello","hi"]"#));
        assert_eq!(m.keywords, vec!["hello", "h", "hi"]);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        assert_eq!(ok(&with(r#""future":{"a":1}"#)).id, "hello");
    }

    fn run(interpreter: Option<Interpreter>, args: &[&str]) -> Run {
        Run { interpreter, file: "main.py".to_owned(), args: args.iter().map(|a| (*a).to_owned()).collect() }
    }

    fn argv(c: &Command) -> Vec<&str> {
        std::iter::once(c.exe.as_str()).chain(c.args.iter().map(String::as_str)).collect()
    }

    #[test]
    fn interpreters_expand_to_the_fixed_command_lines() {
        let dir = Path::new(r"C:\p\hello");
        let file = r"C:\p\hello\main.py";
        let py = |python| expand(&run(Some(Interpreter::Python), &["a", "b"]), dir, python);
        assert_eq!(argv(&py(Python::Direct)), ["python", "-u", "-X", "utf8", file, "a", "b"]);
        assert_eq!(argv(&py(Python::Launcher)), ["py", "-3", "-u", "-X", "utf8", file, "a", "b"]);
        assert_eq!(argv(&expand(&run(Some(Interpreter::Node), &[]), dir, Python::Direct)), ["node", file]);
        let ps = ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", file, "x"];
        let mut expected = vec!["powershell.exe"];
        expected.extend(ps);
        assert_eq!(argv(&expand(&run(Some(Interpreter::PowerShell), &["x"]), dir, Python::Direct)), expected);
        expected[0] = "pwsh.exe";
        assert_eq!(argv(&expand(&run(Some(Interpreter::Pwsh), &["x"]), dir, Python::Direct)), expected);
        assert_eq!(argv(&expand(&run(None, &["-x"]), dir, Python::Direct)), [file, "-x"]);
    }

    #[test]
    fn display_is_the_interpreter_file_and_args() {
        assert_eq!(run(Some(Interpreter::Python), &[]).display(), "python main.py");
        assert_eq!(run(None, &["-x", "y"]).display(), "main.py -x y");
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cpmenu-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn python_flavor_prefers_a_real_python_and_falls_back_to_the_launcher() {
        let make = |name: &str, files: &[(&str, &str)]| {
            let root = temp(name);
            let mut dirs = Vec::new();
            for (sub, file) in files {
                let dir = root.join(sub);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(file), b"x").unwrap();
                dirs.push(dir);
            }
            std::env::join_paths(dirs).unwrap()
        };
        assert_eq!(python_flavor(&make("py-real", &[("a", "python.exe"), ("b", "py.exe")])), Python::Direct);
        assert_eq!(python_flavor(&make("py-none", &[("b", "py.exe")])), Python::Launcher);
        assert_eq!(python_flavor(&make("py-store", &[("WindowsApps", "python.exe"), ("b", "py.exe")])), Python::Launcher);
        assert_eq!(python_flavor(&make("py-store-alone", &[("WindowsApps", "python.exe")])), Python::Direct);
        assert_eq!(python_flavor(&make("py-nothing", &[("a", "other.exe")])), Python::Direct);
    }

    fn write_plugin(root: &Path, folder: &str, manifest: &str, files: &[&str]) {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
    }

    #[test]
    fn discover_lists_good_and_broken_plugins_and_skips_other_folders() {
        let root = temp("discover");
        write_plugin(&root, "hello", MIN, &["main.py"]);
        write_plugin(&root, "broken", "{ nope", &[]);
        write_plugin(&root, "nofile", &MIN.replace("hello", "nofile"), &[]);
        write_plugin(&root, "wrongid", MIN, &["main.py"]);
        write_plugin(&root, "icons", &MIN.replace("hello", "icons").replace("\"keyword\"", "\"icon\":\"i.png\",\"keyword\""), &["main.py"]);
        std::fs::create_dir_all(root.join("plain-folder")).unwrap();
        std::fs::write(root.join("stray.txt"), b"x").unwrap();

        let found = discover(&root);
        let names: Vec<_> = found.iter().map(|f| f.folder.as_str()).collect();
        assert_eq!(names, ["broken", "hello", "icons", "nofile", "wrongid"]);
        let by = |n: &str| found.iter().find(|f| f.folder == n).unwrap();
        assert!(by("hello").manifest.is_ok());
        assert!(by("broken").manifest.as_ref().unwrap_err().contains("plugin.json"));
        assert!(by("nofile").manifest.as_ref().unwrap_err().contains("not found"));
        assert!(by("wrongid").manifest.as_ref().unwrap_err().contains("folder"));
        assert_eq!(by("icons").manifest.as_ref().unwrap().icon, None, "missing icon file is dropped");
        std::fs::write(root.join("icons").join("i.png"), b"png").unwrap();
        assert_eq!(discover(&root).iter().find(|f| f.folder == "icons").unwrap().manifest.as_ref().unwrap().icon.as_deref(), Some("i.png"));
        assert!(discover(&root.join("missing")).is_empty());
    }

    #[test]
    fn stamp_changes_when_a_manifest_changes() {
        let root = temp("stamp");
        write_plugin(&root, "hello", MIN, &["main.py"]);
        let before = stamp(&root);
        assert_eq!(before, stamp(&root));
        std::thread::sleep(std::time::Duration::from_millis(30));
        write_plugin(&root, "hello", &format!("{MIN} "), &[]);
        assert_ne!(before, stamp(&root));
        write_plugin(&root, "second", &MIN.replace("hello", "second"), &[]);
        assert_eq!(stamp(&root).len(), 3);
    }

    fn entry(keyword: Option<&str>, approved: Option<&str>) -> PluginEntry {
        PluginEntry { keyword: keyword.map(str::to_owned), approved: approved.map(str::to_owned), ..PluginEntry::default() }
    }

    #[test]
    fn keyword_override_replaces_the_list_and_conflicts_go_to_the_higher_id() {
        let m = ok(&with(r#""keywords":["h","hi"]"#));
        assert_eq!(effective_keywords(&m, None), vec!["hello", "h", "hi"]);
        assert_eq!(effective_keywords(&m, Some(&entry(Some(" yo "), None))), vec!["yo"]);
        assert_eq!(effective_keywords(&m, Some(&entry(Some(">"), None))), vec!["hello", "h", "hi"], "invalid override is ignored");

        let (a, b, c) = (vec!["gh".to_owned()], vec!["gh".to_owned(), "x".to_owned()], vec!["y".to_owned()]);
        let losers = keyword_losers([("zeta", &b[..]), ("alpha", &a[..]), ("mid", &c[..])]);
        assert_eq!(losers, vec![("zeta".to_owned(), "gh".to_owned())]);
        assert!(keyword_losers([("alpha", &a[..]), ("alpha", &a[..])]).is_empty());
    }

    #[test]
    fn approval_follows_version_and_run_command() {
        let m = ok(MIN);
        assert_eq!(approval_key(&m), "1.0.0|python main.py");
        assert!(is_approved(&m, Some(&entry(None, Some("1.0.0|python main.py")))));
        assert!(!is_approved(&m, Some(&entry(None, Some("0.9|python main.py")))));
        assert!(!is_approved(&m, Some(&entry(None, Some("1.0.0|python other.py")))));
        assert!(!is_approved(&m, Some(&entry(None, None))) && !is_approved(&m, None));
        let args = ok(&MIN.replace("\"file\":\"main.py\"", "\"file\":\"main.py\",\"args\":[\"--x\"]"));
        assert_eq!(approval_key(&args), "1.0.0|python main.py --x");
    }

    #[test]
    fn enabled_by_default_and_settings_fall_back_per_type() {
        assert!(is_enabled(None) && is_enabled(Some(&PluginEntry::default())));
        assert!(!is_enabled(Some(&PluginEntry { enabled: false, ..PluginEntry::default() })));

        let m = ok(&with(
            r#""settings":[{"key":"u","type":"string","default":"d"},{"key":"f","type":"bool"},{"key":"s","type":"choice","choices":["a","b"]}]"#,
        ));
        let mut e = PluginEntry::default();
        assert_eq!(effective_settings(&m, Some(&e))["u"], SettingValue::String("d".into()));
        e.settings.insert("u".into(), SettingValue::String("jane".into()));
        e.settings.insert("f".into(), SettingValue::String("wrong type".into()));
        e.settings.insert("s".into(), SettingValue::String("zzz".into()));
        e.settings.insert("undeclared".into(), SettingValue::Bool(true));
        let s = effective_settings(&m, Some(&e));
        assert_eq!(s["u"], SettingValue::String("jane".into()));
        assert_eq!(s["f"], SettingValue::Bool(false));
        assert_eq!(s["s"], SettingValue::String("a".into()));
        assert!(!s.contains_key("undeclared"));
        e.settings.insert("s".into(), SettingValue::String("b".into()));
        assert_eq!(effective_settings(&m, Some(&e))["s"], SettingValue::String("b".into()));
    }

    #[test]
    fn process_env_has_the_four_variables() {
        let env = process_env("gh", Path::new(r"C:\p\gh"), Path::new(r"C:\d\gh"), 1);
        let names: Vec<_> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["CPMENU_PROTOCOL", "CPMENU_PLUGIN_ID", "CPMENU_PLUGIN_DIR", "CPMENU_DATA_DIR"]);
        assert_eq!(env[0].1, "1");
    }
}
