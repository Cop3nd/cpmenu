//! `cpmenu.exe plugin check <folder>` (docs/plugins.md section 6): validates a plugin folder the way
//! the menu would load it, starts it for real (job object, limits and all), does the `init` / `ready`
//! handshake and one test query, shuts it down, and prints what it found. Exits 1 on any failure.
//! Nothing here touches `%APPDATA%`: the plugin's data directory and log live under the temp dir.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::plugin_proc::{ExitReason, Limits, Proc, Reply, Sink, Spec, next_generation};
use crate::plugin_proto::ToPlugin;
use crate::plugins::{self, Manifest};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

/// What a check found, line by line.
#[derive(Debug, Default)]
pub struct Report {
    lines: Vec<(Level, String)>,
}

impl Report {
    fn add(&mut self, level: Level, text: impl Into<String>) {
        self.lines.push((level, text.into()));
    }

    pub fn failed(&self) -> bool {
        self.lines.iter().any(|(level, _)| *level == Level::Fail)
    }

    fn count(&self, wanted: Level) -> usize {
        self.lines.iter().filter(|(level, _)| *level == wanted).count()
    }

    pub fn render(&self) -> String {
        let mut text = String::new();
        for (level, line) in &self.lines {
            let tag = match level {
                Level::Ok => "  ok    ",
                Level::Info => "        ",
                Level::Warn => "  WARN  ",
                Level::Fail => "  FAIL  ",
            };
            for (n, part) in line.lines().enumerate() {
                text.push_str(if n == 0 { tag } else { "        " });
                text.push_str(part);
                text.push('\n');
            }
        }
        let (warnings, verdict) = (self.count(Level::Warn), if self.failed() { "FAIL" } else { "PASS" });
        text.push_str(&format!("\nresult: {verdict} ({} failed, {warnings} warning{})\n", self.count(Level::Fail), if warnings == 1 { "" } else { "s" }));
        text
    }
}

struct ChannelSink(Mutex<Sender<Reply>>);

impl Sink for ChannelSink {
    fn deliver(&self, _slot: u16, _generation: u16, reply: Reply) -> bool {
        self.0.lock().is_ok_and(|tx| tx.send(reply).is_ok())
    }
}

fn why(reason: &ExitReason) -> String {
    match reason {
        ExitReason::Exited => "the process exited by itself".to_owned(),
        ExitReason::Shutdown => "the process was shut down".to_owned(),
        ExitReason::SpawnFailed(e) => format!("the process could not be started: {e}"),
        ExitReason::ReadyTimeout => "no \"ready\" within the time allowed after init (5 s)".to_owned(),
        ExitReason::HardTimeout => "no answer to the query within 10 s".to_owned(),
        ExitReason::Flood => "more than 200 messages per second for 3 s".to_owned(),
        ExitReason::Strikes => "too many lines that were not valid JSON or were over 64 KB".to_owned(),
    }
}

/// Where the check keeps the plugin's data directory and log.
fn scratch(id: &str) -> PathBuf {
    std::env::temp_dir().join("cpmenu-plugin-check").join(id)
}

/// Runs the whole check on the plugin in `dir`.
pub fn check(dir: &Path, limits: &Limits) -> Report {
    let mut report = Report::default();
    let Ok(dir) = dir.canonicalize() else {
        report.add(Level::Fail, format!("{} does not exist", dir.display()));
        return report;
    };
    let dir = PathBuf::from(dir.to_string_lossy().trim_start_matches(r"\\?\"));
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    report.add(Level::Info, format!("plugin folder {}", dir.display()));

    let manifest = match plugins::load(&dir, &folder) {
        Ok(manifest) => manifest,
        Err(e) => {
            report.add(Level::Fail, format!("plugin.json: {e}"));
            return report;
        }
    };
    let author = if manifest.author.is_empty() { String::new() } else { format!(" by {}", manifest.author) };
    report.add(Level::Ok, format!("plugin.json is valid: {} v{}{author} (id {})", manifest.name, manifest.version, manifest.id));
    describe(&manifest, &dir, &mut report);

    let python = plugins::python_flavor(&std::env::var_os("PATH").unwrap_or_default());
    let command = plugins::expand(&manifest.run, &dir, python);
    report.add(Level::Info, format!("command: {} {}", command.exe, command.args.join(" ")));

    let work = scratch(&manifest.id);
    let (data_dir, log) = (work.join("data"), work.join("plugin.log"));
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_file(work.join("plugin.log.1"));
    let settings = plugins::effective_settings(&manifest, None);
    let init = ToPlugin::Init {
        protocol: plugins::HOST_PROTOCOL,
        host: &format!("cpmenu {} (plugin check)", env!("CARGO_PKG_VERSION")),
        lang: "en",
        plugin_dir: &dir.to_string_lossy(),
        data_dir: &data_dir.to_string_lossy(),
        settings: &settings,
    }
    .to_line();
    let spec = Spec {
        id: manifest.id.clone(),
        command,
        dir: dir.clone(),
        env: plugins::process_env(&manifest.id, &dir, &data_dir, plugins::HOST_PROTOCOL),
        data_dir,
        log: Some(log.clone()),
        init,
        max_results: manifest.max_results,
        limits: limits.clone(),
    };

    let (tx, rx) = channel();
    let latest = Arc::new(AtomicU64::new(QUERY_ID));
    let started = Instant::now();
    let proc = Proc::spawn(spec, 0, next_generation(), Arc::new(ChannelSink(Mutex::new(tx))), latest);
    let alive = handshake(&rx, started, &mut report) && query(&proc, &rx, &manifest, limits, &mut report);
    if alive {
        shut_down(&proc, &rx, limits, &mut report);
    } else {
        // Failed above: wait for the supervisor so no process is left behind.
        proc.kill(ExitReason::Shutdown);
        wait_exit(&rx, Duration::from_secs(5));
    }
    drop(proc);
    inspect_log(&log, &mut report);
    report
}

const QUERY_ID: u64 = 1;

/// What the manifest asks for, and the things worth a second look.
fn describe(m: &Manifest, dir: &Path, report: &mut Report) {
    let keywords = if m.keywords.is_empty() { "none".to_owned() } else { m.keywords.join(", ") };
    report.add(
        Level::Info,
        format!("keywords: {keywords}; global: {}; tag {}; max_results {}; debounce {} ms; query timeout {} ms; {} setting{}", m.global, m.tag, m.max_results, m.debounce_ms, m.query_timeout_ms, m.settings.len(), if m.settings.len() == 1 { "" } else { "s" }),
    );
    report.add(Level::Ok, format!("run.file {} exists", m.run.file));
    if m.author.is_empty() {
        report.add(Level::Warn, "no \"author\": the approval prompt shows a question mark instead");
    }
    if m.description.is_empty() {
        report.add(Level::Warn, "no \"description\"");
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("plugin.json"))
        && m.icon.is_none()
        && plugins::parse_manifest(&text, &m.id).is_ok_and(|raw| raw.icon.is_some())
    {
        report.add(Level::Warn, "icon is ignored: the file is missing or over 256 KB");
    }
}

/// Waits for `ready`. True if the plugin is up.
fn handshake(rx: &Receiver<Reply>, started: Instant, report: &mut Report) -> bool {
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Reply::Ready { protocol }) => {
                let ms = started.elapsed().as_millis();
                let speaks = protocol.map_or("no protocol number (assumed 1)".to_owned(), |p| format!("protocol {p}"));
                report.add(Level::Ok, format!("init answered with ready after {ms} ms, {speaks}"));
                if protocol.is_some_and(|p| p > plugins::HOST_PROTOCOL) {
                    report.add(Level::Warn, "the plugin asks for a newer protocol than this cpmenu speaks");
                }
                if ms > 2000 {
                    report.add(Level::Warn, "slow start: the first query after a start waits this long (idle plugins are stopped after 60 s)");
                }
                return true;
            }
            Ok(Reply::Exited { code, reason }) => {
                report.add(Level::Fail, format!("no ready: {} (exit code {code:?})", why(&reason)));
                return false;
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                report.add(Level::Fail, "no ready and no exit: the supervisor did not answer");
                return false;
            }
        }
    }
}

/// One query with empty text for a keyword plugin (what typing `<keyword> ` sends), or a short
/// text for a global-only one. True if the plugin is still running afterwards.
fn query(proc: &Proc, rx: &Receiver<Reply>, m: &Manifest, limits: &Limits, report: &mut Report) -> bool {
    let (keyword, global, text) = match m.keywords.first() {
        Some(keyword) => (keyword.as_str(), false, ""),
        None => ("", true, "test"),
    };
    let line = ToPlugin::Query { id: QUERY_ID, text, keyword, global, max: m.max_results }.to_line();
    let sent = Instant::now();
    proc.send_request(line);
    let what = if global { "global query \"test\"".to_owned() } else { format!("query \"\" for keyword {keyword}") };
    loop {
        let left = (limits.hard_timeout + Duration::from_secs(5)).saturating_sub(sent.elapsed());
        match rx.recv_timeout(left) {
            Ok(Reply::Results { items, .. }) => {
                let ms = sent.elapsed().as_millis();
                report.add(Level::Ok, format!("{what} answered with {} result{} in {ms} ms", items.len(), if items.len() == 1 { "" } else { "s" }));
                if ms as u64 > m.query_timeout_ms {
                    report.add(Level::Warn, format!("slower than query_timeout_ms ({} ms): in the menu this answer would be reported as \"not answering\"", m.query_timeout_ms));
                }
                for item in items.iter().take(5) {
                    let sub = if item.subtitle.is_empty() { String::new() } else { format!(" - {}", item.subtitle) };
                    report.add(Level::Info, format!("  {}{sub}", item.title));
                }
                let silent = items.iter().filter(|i| i.actions.is_empty()).count();
                if silent > 0 {
                    report.add(Level::Warn, format!("{silent} result{} without actions: Enter does nothing on them", if silent == 1 { "" } else { "s" }));
                }
                return true;
            }
            Ok(Reply::Error { message, .. }) => {
                report.add(Level::Warn, format!("{what} was answered with an error: {message}"));
                return true;
            }
            Ok(Reply::Exited { code, reason }) => {
                report.add(Level::Fail, format!("{what}: {} (exit code {code:?})", why(&reason)));
                return false;
            }
            Ok(Reply::Done(_)) => report.add(Level::Warn, "a done arrived that nobody asked for (ignored by the menu)"),
            Ok(_) => {}
            Err(_) => {
                report.add(Level::Fail, format!("{what}: no answer"));
                return false;
            }
        }
    }
}

/// `shutdown`, then waits for the exit.
fn shut_down(proc: &Proc, rx: &Receiver<Reply>, limits: &Limits, report: &mut Report) {
    let asked = Instant::now();
    proc.shutdown();
    match wait_exit(rx, limits.grace + Duration::from_secs(5)) {
        Some((code, ExitReason::Shutdown)) if asked.elapsed() < limits.grace => {
            report.add(Level::Ok, format!("exited with code {code:?} {} ms after shutdown", asked.elapsed().as_millis()));
        }
        Some((code, ExitReason::Shutdown)) => {
            report.add(Level::Warn, format!("slow to exit: still running {} ms after shutdown, so cpmenu terminated it (it must exit within 1 s; code {code:?})", asked.elapsed().as_millis()));
        }
        Some((code, reason @ (ExitReason::Flood | ExitReason::Strikes))) => {
            report.add(Level::Fail, format!("cpmenu killed the plugin: {} (code {code:?})", why(&reason)));
        }
        Some((code, reason)) => report.add(Level::Warn, format!("ended with {} instead of a clean shutdown (code {code:?})", why(&reason))),
        None => report.add(Level::Fail, "the process did not end after shutdown"),
    }
}

fn wait_exit(rx: &Receiver<Reply>, within: Duration) -> Option<(Option<i32>, ExitReason)> {
    let deadline = Instant::now() + within;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Reply::Exited { code, reason }) => return Some((code, reason)),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// The host's own log lines say what the replies cannot: lines skipped for being bad JSON or too
/// long, rate limits, and what the plugin wrote to stderr.
fn inspect_log(log: &Path, report: &mut Report) {
    let Ok(text) = std::fs::read_to_string(log) else { return };
    let count = |needle: &str| text.lines().filter(|l| l.contains(needle)).count();
    let (bad, long, dropped) = (count("bad JSON line"), count("oversized line"), count("log lines dropped"));
    if bad > 0 {
        report.add(Level::Warn, format!("{bad} line{} on stdout were not JSON and were skipped: stdout is for protocol lines only, write debugging output to stderr", if bad == 1 { "" } else { "s" }));
    }
    if long > 0 {
        report.add(Level::Warn, format!("{long} line{} over 64 KB were skipped", if long == 1 { "" } else { "s" }));
    }
    if dropped > 0 {
        report.add(Level::Warn, "log messages above 20 per second are dropped");
    }
    let shown: Vec<&str> = text.lines().filter(|l| !l.starts_with("[host]")).take(10).collect();
    if !shown.is_empty() {
        report.add(Level::Info, format!("log ({}):\n{}", log.display(), shown.join("\n")));
    } else {
        report.add(Level::Info, format!("log: {}", log.display()));
    }
}

/// Where the report goes: stdout if the launcher gave this process one (a pipe, a file, a console),
/// else the console of the program that started it. The exe is a GUI program, so it has none of its own.
fn output() -> Box<dyn Write> {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    let inherited = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
    if inherited {
        return Box::new(std::io::stdout());
    }
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    match std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        Ok(console) => Box::new(console),
        Err(_) => Box::new(std::io::sink()),
    }
}

const USAGE: &str = "usage: cpmenu.exe plugin check <plugin folder>\n";

/// `cpmenu.exe plugin <args>`; returns the process exit code.
pub fn cli(args: &[String]) -> i32 {
    run(args, &mut *output())
}

fn run(args: &[String], out: &mut dyn Write) -> i32 {
    match args {
        [check_word, folder] if check_word == "check" => {
            let report = check(Path::new(folder), &Limits::default());
            let _ = write!(out, "cpmenu plugin check\n{}", report.render());
            let _ = out.flush();
            i32::from(report.failed())
        }
        _ => {
            let _ = out.write_all(USAGE.as_bytes());
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_plugin() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let path = exe.parent().unwrap().parent().unwrap().join("examples").join("fake_plugin.exe");
        assert!(path.exists(), "{} is built by cargo test", path.display());
        path
    }

    /// A plugin folder named `id` that runs the fake plugin with `args`; `manifest_extra` goes into plugin.json.
    fn folder(id: &str, args: &[&str], manifest_extra: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cpmenu-test-check-{}", std::process::id())).join(id);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(fake_plugin(), dir.join("fake.exe")).unwrap();
        let args = args.iter().map(|a| format!("{a:?}")).collect::<Vec<_>>().join(",");
        let manifest = format!(
            r#"{{"id":"{id}","name":"Check {id}","version":"1.0","author":"me","description":"d","keyword":"ck","run":{{"file":"fake.exe","args":[{args}]}}{manifest_extra}}}"#
        );
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();
        dir
    }

    fn fast() -> Limits {
        Limits { tick: Duration::from_millis(20), grace: Duration::from_millis(400), ready_timeout: Duration::from_millis(600), hard_timeout: Duration::from_millis(800), ..Limits::default() }
    }

    #[test]
    fn a_good_plugin_passes_with_ready_results_and_a_clean_exit() {
        let report = check(&folder("good", &["echo"], ""), &fast());
        let text = report.render();
        assert!(!report.failed(), "{text}");
        for wanted in ["plugin.json is valid: Check good v1.0 by me", "init answered with ready", "query \"\" for keyword ck answered with 1 result", "echo:", "exited with code Some(0)", "result: PASS", "hello from stderr"] {
            assert!(text.contains(wanted), "missing {wanted:?} in\n{text}");
        }
        assert!(text.contains("without actions"), "the echo result has none: {text}");
    }

    #[test]
    fn a_plugin_that_dies_at_once_fails() {
        let report = check(&folder("dies", &["crash"], ""), &fast());
        let text = report.render();
        assert!(report.failed() && text.contains("no ready: the process exited by itself (exit code Some(3))") && text.contains("result: FAIL"), "{text}");
    }

    #[test]
    fn a_plugin_that_never_says_ready_fails_at_the_timeout() {
        let report = check(&folder("mute", &["silent"], ""), &fast());
        assert!(report.failed() && report.render().contains("no \"ready\" within the time allowed"), "{}", report.render());
    }

    #[test]
    fn a_plugin_that_never_answers_the_query_fails() {
        let report = check(&folder("stuck", &["hang"], ""), &fast());
        let text = report.render();
        assert!(report.failed() && text.contains("init answered with ready") && text.contains("no answer to the query within 10 s"), "{text}");
    }

    #[test]
    fn stdout_noise_is_reported_and_fails_when_it_is_a_flood() {
        let report = check(&folder("noisy", &["badjson"], ""), &fast());
        let text = report.render();
        assert!(report.failed() && text.contains("too many lines that were not valid JSON"), "{text}");
        assert!(text.contains("were not JSON and were skipped"), "{text}");
    }

    #[test]
    fn a_plugin_that_ignores_shutdown_is_a_warning() {
        let report = check(&folder("stubborn", &["stubborn"], ""), &fast());
        let text = report.render();
        assert!(!report.failed() && text.contains("WARN") && text.contains("slow to exit"), "{text}");
    }

    #[test]
    fn a_bad_manifest_fails_without_starting_anything() {
        let dir = folder("badmanifest", &["echo"], "");
        std::fs::write(dir.join("plugin.json"), r#"{"id":"someone-else","name":"x","version":"1","keyword":"k","run":{"file":"fake.exe"}}"#).unwrap();
        let report = check(&dir, &fast());
        assert!(report.failed() && report.render().contains("must equal the folder name"), "{}", report.render());
        let missing = check(&dir.join("nope"), &fast());
        assert!(missing.failed() && missing.render().contains("does not exist"));
        let no_file = folder("nofile", &["echo"], "");
        std::fs::remove_file(no_file.join("fake.exe")).unwrap();
        assert!(check(&no_file, &fast()).render().contains("run.file \"fake.exe\" not found"));
    }

    #[test]
    fn a_global_only_plugin_gets_a_global_query() {
        let dir = folder("globalonly", &["echo"], "");
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"id":"globalonly","name":"G","version":"1","author":"me","description":"d","global":true,"run":{"file":"fake.exe","args":["echo"]}}"#,
        )
        .unwrap();
        let text = check(&dir, &fast()).render();
        assert!(text.contains("global query \"test\" answered with 1 result") && text.contains("echo: test"), "{text}");
    }

    #[test]
    fn warnings_cover_missing_author_description_and_icon() {
        let dir = folder("sparse", &["echo"], r#","icon":"missing.png""#);
        let manifest = std::fs::read_to_string(dir.join("plugin.json")).unwrap().replace(r#""author":"me","#, "").replace(r#""description":"d","#, "");
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();
        let text = check(&dir, &fast()).render();
        for wanted in ["no \"author\"", "no \"description\"", "icon is ignored"] {
            assert!(text.contains(wanted), "{wanted}: {text}");
        }
    }

    #[test]
    fn usage_for_anything_but_check_with_one_folder() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let mut out = Vec::new();
        assert_eq!(run(&args(&[]), &mut out), 2);
        assert_eq!(run(&args(&["check"]), &mut out), 2);
        assert_eq!(run(&args(&["frobnicate", "x"]), &mut out), 2);
        assert!(String::from_utf8(out).unwrap().starts_with("usage: cpmenu.exe plugin check"));
        let mut out = Vec::new();
        assert_eq!(run(&args(&["check", r"C:\no\such\folder"]), &mut out), 1);
        assert!(String::from_utf8(out).unwrap().contains("does not exist"));
    }
}
