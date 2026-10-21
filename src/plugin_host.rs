//! The UI thread's view of the plugin system (docs/plugins.md sections 3 and 4): which plugins are
//! installed, which may run, which keyword goes where, and the bookkeeping around the processes of
//! `plugin_proc`: the queue for queries sent before `ready`, the two-in-flight coalescing, request
//! timeouts, the validity window of `done`, and the idle sweep. Nothing here blocks on a plugin:
//! every call into a `Proc` is a non-blocking send. The window owns one `Host` and never holds its
//! borrow across anything that can pump messages.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime};

use crate::config::{PluginEntry, PluginsConfig};
use crate::lang;
use crate::menu::{Item, PluginRow, RowKind, Tag};
use crate::plugin_icon;
use crate::plugin_proc::{Crashes, Limits, Proc, Reply, Sink, Spec, next_generation};
use crate::plugin_proto::{self as proto, Effect, ToPlugin};
use crate::plugins::{self, Manifest};
use crate::settings::{PluginInfo, PluginState, SettingInfo};

/// A `done` is honored for an invocation sent at most this long ago.
pub const ACTION_WINDOW_MS: u64 = 10_000;
/// An outstanding request with no answer for this long gets the plugin killed (the supervisor
/// does the same by itself; this is the window's copy for a request it never managed to send).
pub const HARD_TIMEOUT_MS: u64 = 10_000;
/// A plugin unused for this long is shut down while the menu is hidden.
pub const IDLE_MS: u64 = 60_000;
/// At most this many unanswered queries per plugin; a third replaces the one waiting.
const IN_FLIGHT: usize = 2;
/// Lines for actions sent before the plugin said `ready`.
const QUEUED_ACTIONS: usize = 4;

struct Running {
    proc: Proc,
    generation: u16,
    ready: bool,
    /// Asked to stop; its `Exited` hasn't arrived. Never sent to again.
    stopping: bool,
    last_used: u64,
    /// The newest query sent before `ready`.
    queued: Option<(u64, String)>,
    /// The query that replaced the older one when two were already in flight.
    waiting: Option<(u64, String)>,
    /// `(id, sent at ms)` of the queries sent and not yet answered.
    outstanding: Vec<(u64, u64)>,
    queued_actions: Vec<String>,
    /// The soft timeout was reported for the oldest outstanding query.
    slow: bool,
    killed: bool,
}

struct Plugin {
    id: String,
    dir: PathBuf,
    manifest: Manifest,
    /// The keywords it answers to: the user's override or the manifest's, minus those lost to a
    /// plugin with a lower id. Empty while disabled.
    keywords: Vec<String>,
    run: Option<Running>,
}

/// An `action` the plugin owes a `done` for.
struct Pending {
    req: u64,
    slot: u16,
    generation: u16,
    sent_ms: u64,
    item_id: String,
    stay: bool,
}

#[derive(Default)]
struct Dirs {
    root: Option<PathBuf>,
    data: Option<PathBuf>,
    logs: Option<PathBuf>,
}

/// What a keyword query to a plugin runs into.
#[derive(Debug, PartialEq)]
pub enum Status {
    Ready,
    /// Enabled but the user hasn't allowed this version and command yet.
    NeedsApproval,
    /// Crashed three times within a minute; off for this session.
    Failed,
    Unavailable,
}

/// What the window does after a reply.
#[derive(Debug, PartialEq)]
pub enum Handled {
    Nothing,
    Results { slot: u16, generation: u16, id: u64, items: Vec<proto::Item> },
    QueryError { slot: u16, message: String },
    Done { slot: u16, effects: Vec<Effect>, keep_open: bool, error: Option<String>, item_id: String, stay: bool },
    Exited { slot: u16, failed: bool },
}

#[derive(Default)]
pub struct Host {
    sink: Option<Arc<dyn Sink>>,
    latest: Arc<AtomicU64>,
    next_query: u64,
    next_req: u64,
    enabled: bool,
    entries: BTreeMap<String, PluginEntry>,
    plugins: Vec<Plugin>,
    /// `(folder, error)` of the folders whose manifest is broken; the PLUGINS tab lists them.
    broken: Vec<(String, String)>,
    stamp: Option<Vec<(String, Option<SystemTime>)>>,
    crashes: BTreeMap<String, Crashes>,
    failed: BTreeSet<String>,
    pending: Vec<Pending>,
    /// The menu query the newest batch of queries was sent for.
    sent_for: String,
    dirs: Dirs,
}

/// Whether `Enter`/an action may open `target`: web and mail links, or a path that exists.
pub fn open_allowed(target: &str) -> bool {
    if !proto::open_target_ok(target) {
        return false;
    }
    let lower = target.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") || Path::new(target).exists()
}

/// Whether the menu stays open after `effects` of an action (`stay` keeps it too).
pub fn keeps_open(effects: &[Effect], stay: bool) -> bool {
    effects.iter().fold(stay, |keep, effect| match effect {
        Effect::KeepOpen(k) => *k,
        Effect::Hide => false,
        Effect::SetQuery(_) => true,
        _ => keep,
    })
}

impl Host {
    pub fn attach(&mut self, sink: Arc<dyn Sink>) {
        self.sink = Some(sink);
    }

    #[cfg(test)]
    fn with_dirs(root: PathBuf, data: PathBuf, logs: PathBuf) -> Self {
        Self { dirs: Dirs { root: Some(root), data: Some(data), logs: Some(logs) }, ..Self::default() }
    }

    /// Applies `[behavior] plugins` and the `[plugins.<id>]` tables: what may run, which keywords
    /// apply. A plugin that may no longer run is shut down.
    pub fn sync(&mut self, config: &PluginsConfig) {
        self.enabled = config.enabled;
        self.entries = config.entries.clone();
        self.recompute();
    }

    fn recompute(&mut self) {
        let master = self.enabled;
        for plugin in &mut self.plugins {
            let entry = self.entries.get(&plugin.id);
            plugin.keywords = if master && plugins::is_enabled(entry) { plugins::effective_keywords(&plugin.manifest, entry) } else { Vec::new() };
        }
        let losers = plugins::keyword_losers(self.plugins.iter().map(|p| (p.id.as_str(), p.keywords.as_slice())));
        for (id, keyword) in losers {
            if let Some(plugin) = self.plugins.iter_mut().find(|p| p.id == id) {
                plugin.keywords.retain(|k| *k != keyword);
            }
        }
        for slot in 0..self.plugins.len() {
            if self.status(slot as u16) != Status::Ready {
                self.stop(slot);
            }
        }
        // Switching a plugin off and on again is how a failed one gets another chance.
        for plugin in &self.plugins {
            if !plugins::is_enabled(self.entries.get(&plugin.id)) {
                self.failed.remove(&plugin.id);
                if let Some(crashes) = self.crashes.get_mut(&plugin.id) {
                    crashes.clear();
                }
            }
        }
    }

    fn root(&self) -> Option<PathBuf> {
        self.dirs.root.clone().or_else(plugins::root)
    }

    /// Reads the plugins directory again if it or a manifest changed since the last scan (or never
    /// scanned). Returns true if the set of plugins was rebuilt. Does nothing while plugins are off.
    pub fn rescan(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(root) = self.root() else { return false };
        let stamp = plugins::stamp(&root);
        if self.stamp.as_ref() == Some(&stamp) {
            return false;
        }
        self.stamp = Some(stamp);
        let mut old = std::mem::take(&mut self.plugins);
        self.broken.clear();
        for found in plugins::discover(&root) {
            let manifest = match found.manifest {
                Ok(manifest) => manifest,
                Err(error) => {
                    self.broken.push((found.folder, error));
                    continue;
                }
            };
            let slot = self.plugins.len();
            // A running process keeps its slot (the slot is in its replies) only if nothing changed.
            let run = old.get_mut(slot).filter(|o| o.manifest == manifest && o.dir == found.dir).and_then(|o| o.run.take());
            self.plugins.push(Plugin { id: manifest.id.clone(), dir: found.dir, manifest, keywords: Vec::new(), run });
        }
        // Whatever is left in `old` is dropped here; a dropped `Proc` shuts its plugin down.
        drop(old);
        let plugins = &self.plugins;
        self.pending.retain(|p| plugins.get(p.slot as usize).and_then(|pl| pl.run.as_ref()).is_some_and(|r| r.generation == p.generation));
        self.recompute();
        true
    }

    /// What the PLUGINS tab lists, by plugin id: every folder with a `plugin.json`, the broken ones
    /// with their error. Empty while plugins are off.
    pub fn infos(&self) -> Vec<PluginInfo> {
        if !self.enabled {
            return Vec::new();
        }
        let mut infos: Vec<PluginInfo> = (0..self.plugins.len()).map(|slot| self.info(slot)).collect();
        infos.extend(self.broken.iter().map(|(folder, error)| PluginInfo {
            id: folder.clone(),
            name: folder.clone(),
            state: PluginState::ManifestError(error.clone()),
            ..PluginInfo::default()
        }));
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        infos
    }

    fn info(&self, slot: usize) -> PluginInfo {
        let plugin = &self.plugins[slot];
        let m = &plugin.manifest;
        let entry = self.entries.get(&plugin.id);
        let (enabled, approved) = (plugins::is_enabled(entry), plugins::is_approved(m, entry));
        let state = match &plugin.run {
            _ if !enabled => PluginState::Disabled,
            _ if self.failed.contains(&plugin.id) => PluginState::Failed,
            _ if !approved => PluginState::NotApproved,
            Some(run) if !run.stopping && run.proc.is_running() => {
                if run.ready {
                    PluginState::Running
                } else {
                    PluginState::Starting
                }
            }
            _ => PluginState::Stopped,
        };
        let keywords = plugins::effective_keywords(m, entry);
        let values = plugins::effective_settings(m, entry);
        PluginInfo {
            id: plugin.id.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            author: m.author.clone(),
            run: m.run.display(),
            state,
            enabled,
            approved,
            keyword_lost: enabled && self.enabled && keywords.first().is_some_and(|k| !plugin.keywords.contains(k)),
            keywords,
            can_global: m.global,
            global: entry.is_some_and(|e| e.global),
            settings: m
                .settings
                .iter()
                .map(|s| SettingInfo {
                    key: s.key.clone(),
                    label: s.label.clone(),
                    kind: s.kind.clone(),
                    value: values.get(&s.key).cloned().unwrap_or_else(|| s.default.clone()),
                })
                .collect(),
        }
    }

    /// Shuts the plugin down (its keyword or command changed); it starts again on the next query.
    pub fn stop_plugin(&mut self, id: &str) {
        if let Some(slot) = self.plugins.iter().position(|p| p.id == id) {
            self.stop(slot);
        }
    }

    /// Sends a running plugin its settings after the user edited one.
    pub fn push_settings(&mut self, id: &str) {
        let Some(slot) = self.plugins.iter().position(|p| p.id == id) else { return };
        let plugin = &mut self.plugins[slot];
        let settings = plugins::effective_settings(&plugin.manifest, self.entries.get(&plugin.id));
        if let Some(run) = plugin.run.as_mut().filter(|r| !r.stopping) {
            run.proc.send(ToPlugin::Settings { settings: &settings }.to_line());
        }
    }

    /// Gives a plugin that failed three times another chance.
    pub fn retry(&mut self, id: &str) {
        self.failed.remove(id);
        if let Some(crashes) = self.crashes.get_mut(id) {
            crashes.clear();
        }
    }

    /// `(keyword, slot)` of every plugin that answers to a keyword.
    pub fn keyword_table(&self) -> Vec<(String, u16)> {
        self.plugins.iter().enumerate().flat_map(|(slot, p)| p.keywords.iter().map(move |k| (k.clone(), slot as u16))).collect()
    }

    pub fn status(&self, slot: u16) -> Status {
        let Some(plugin) = self.plugins.get(slot as usize) else { return Status::Unavailable };
        let entry = self.entries.get(&plugin.id);
        if !self.enabled || !plugins::is_enabled(entry) {
            Status::Unavailable
        } else if self.failed.contains(&plugin.id) {
            Status::Failed
        } else if !plugins::is_approved(&plugin.manifest, entry) {
            Status::NeedsApproval
        } else {
            Status::Ready
        }
    }

    /// Plugins that receive every query: allowed by the manifest and by the user, approved, not failed.
    pub fn global_targets(&self) -> Vec<u16> {
        (0..self.plugins.len() as u16)
            .filter(|&slot| {
                let plugin = &self.plugins[slot as usize];
                plugin.manifest.global && self.entries.get(&plugin.id).is_some_and(|e| e.global) && self.status(slot) == Status::Ready
            })
            .collect()
    }

    pub fn plugin_id(&self, slot: u16) -> &str {
        self.plugins.get(slot as usize).map_or("", |p| p.id.as_str())
    }

    pub fn name(&self, slot: u16) -> &str {
        self.plugins.get(slot as usize).map_or("", |p| p.manifest.name.as_str())
    }

    pub fn max_results(&self, slot: u16) -> usize {
        self.plugins.get(slot as usize).map_or(1, |p| p.manifest.max_results)
    }

    pub fn debounce_ms(&self, slot: u16) -> u32 {
        self.plugins.get(slot as usize).map_or(0, |p| p.manifest.debounce_ms as u32)
    }

    /// What the user must approve, in the form `[plugins.<id>] approved` stores.
    pub fn approval_key(&self, slot: u16) -> Option<String> {
        self.plugins.get(slot as usize).map(|p| plugins::approval_key(&p.manifest))
    }

    /// What approving plugin `id` stores, in the form `[plugins.<id>] approved` takes.
    pub fn approval_key_of(&self, id: &str) -> Option<String> {
        self.plugins.iter().find(|p| p.id == id).map(|p| plugins::approval_key(&p.manifest))
    }

    /// A new query id for a batch of queries sent for `query`; replies to any older id are stale.
    pub fn new_batch(&mut self, query: &str) -> u64 {
        self.next_query += 1;
        self.latest.store(self.next_query, Ordering::Relaxed);
        self.sent_for = query.to_owned();
        self.next_query
    }

    pub fn sent_for(&self) -> &str {
        &self.sent_for
    }

    fn running(&mut self, slot: u16) -> Option<&mut Running> {
        self.plugins.get_mut(slot as usize)?.run.as_mut()
    }

    /// Starts the process if there is none (or it is on its way out). The first lines wait in the queues.
    fn ensure_started(&mut self, slot: u16, now: u64) {
        let Some(plugin) = self.plugins.get(slot as usize) else { return };
        if plugin.run.as_ref().is_some_and(|r| !r.stopping && r.proc.is_running()) {
            return;
        }
        let Some(sink) = self.sink.clone() else { return };
        let Some(data_dir) = self.dirs.data.as_ref().map(|d| d.join(&plugin.id)).or_else(|| plugins::data_dir(&plugin.id)) else { return };
        let entry = self.entries.get(&plugin.id);
        let settings = plugins::effective_settings(&plugin.manifest, entry);
        let (dir, data_text) = (plugin.dir.to_string_lossy().into_owned(), data_dir.to_string_lossy().into_owned());
        let init = ToPlugin::Init {
            protocol: plugins::HOST_PROTOCOL,
            host: &format!("cpmenu {}", env!("CARGO_PKG_VERSION")),
            lang: &lang::code(),
            plugin_dir: &dir,
            data_dir: &data_text,
            settings: &settings,
        }
        .to_line();
        let python = plugins::python_flavor(&std::env::var_os("PATH").unwrap_or_default());
        let spec = Spec {
            id: plugin.id.clone(),
            command: plugins::expand(&plugin.manifest.run, &plugin.dir, python),
            dir: plugin.dir.clone(),
            env: plugins::process_env(&plugin.id, &plugin.dir, &data_dir, plugins::HOST_PROTOCOL),
            data_dir,
            log: self.dirs.logs.as_ref().map(|d| d.join(format!("{}.log", plugin.id))).or_else(|| plugins::log_path(&plugin.id)),
            init,
            max_results: plugin.manifest.max_results,
            limits: Limits::default(),
        };
        let generation = next_generation();
        let proc = Proc::spawn(spec, slot, generation, sink, self.latest.clone());
        let run = Running {
            proc,
            generation,
            ready: false,
            stopping: false,
            last_used: now,
            queued: None,
            waiting: None,
            outstanding: Vec::new(),
            queued_actions: Vec::new(),
            slow: false,
            killed: false,
        };
        self.plugins[slot as usize].run = Some(run);
    }

    /// Sends query `id` to the plugin, starting it if needed. Before `ready` only the newest query
    /// is kept; with two in flight a newer one waits and replaces any older waiting one.
    pub fn send_query(&mut self, slot: u16, id: u64, text: &str, keyword: &str, global: bool, now: u64) {
        if self.status(slot) != Status::Ready {
            return;
        }
        self.ensure_started(slot, now);
        let max = self.max_results(slot);
        let Some(run) = self.running(slot) else { return };
        run.last_used = now;
        let line = ToPlugin::Query { id, text, keyword, global, max }.to_line();
        if !run.ready {
            run.queued = Some((id, line));
        } else if run.outstanding.len() >= IN_FLIGHT {
            run.waiting = Some((id, line));
        } else {
            run.outstanding.push((id, now));
            run.proc.send_request(line);
        }
    }

    /// Sends the plugin an `action` for `row` (starting it if needed) and remembers it so the
    /// `done` that follows is honored. `None` if the plugin may not run.
    pub fn send_action(&mut self, row: &PluginRow, action_id: &str, item_id: &str, stay: bool, now: u64) -> Option<u64> {
        let slot = row.slot;
        if self.status(slot) != Status::Ready {
            return None;
        }
        self.ensure_started(slot, now);
        self.next_req += 1;
        let req = self.next_req;
        let line = ToPlugin::Action { req, id: action_id, item: &row.item.key, data: row.item.data.as_ref(), query_id: row.query_id }.to_line();
        let run = self.running(slot)?;
        run.last_used = now;
        let generation = run.generation;
        if row.generation != generation {
            // The plugin was restarted since it produced the row; `data` carries what it needs.
            run.proc.note("action for a row from an earlier process: sending its data to the new one");
        }
        if !run.ready {
            if run.queued_actions.len() >= QUEUED_ACTIONS {
                return None;
            }
            run.queued_actions.push(line);
        } else {
            run.proc.send_request(line);
        }
        self.pending.push(Pending { req, slot, generation, sent_ms: now, item_id: item_id.to_owned(), stay });
        self.settle(slot);
        Some(req)
    }

    /// Tells the supervisor how many requests the plugin really owes now.
    fn settle(&mut self, slot: u16) {
        let actions = self.pending.iter().filter(|p| p.slot == slot).count();
        if let Some(run) = self.running(slot) {
            run.proc.set_pending((run.outstanding.len() + actions) as u32);
        }
    }

    /// Query `id` is answered (or dropped as stale): it and every older one are no longer waited
    /// for, and the waiting query, if any, goes out.
    fn answered(&mut self, slot: u16, id: u64, now: u64) {
        if let Some(run) = self.running(slot) {
            run.outstanding.retain(|(o, _)| *o > id);
            run.slow = false;
            if run.outstanding.len() < IN_FLIGHT
                && let Some((wid, line)) = run.waiting.take()
            {
                run.outstanding.push((wid, now));
                run.proc.send_request(line);
            }
        }
        self.settle(slot);
    }

    /// Applies a reply from the reader/supervisor of generation `generation`. Replies of a process
    /// that is not the current one for that slot are dropped.
    pub fn handle(&mut self, slot: u16, generation: u16, reply: Reply, now: u64) -> Handled {
        let current = self.plugins.get(slot as usize).and_then(|p| p.run.as_ref()).is_some_and(|r| r.generation == generation);
        if !current {
            return Handled::Nothing;
        }
        match reply {
            Reply::Ready { .. } => {
                if let Some(run) = self.running(slot) {
                    run.ready = true;
                    if let Some((id, line)) = run.queued.take() {
                        run.outstanding.push((id, now));
                        run.proc.send_request(line);
                    }
                    for line in std::mem::take(&mut run.queued_actions) {
                        run.proc.send_request(line);
                    }
                }
                self.settle(slot);
                Handled::Nothing
            }
            Reply::Results { id, items } => {
                let asked = self.running(slot).is_some_and(|r| r.outstanding.iter().any(|(o, _)| *o == id));
                if !asked {
                    self.note(slot, &format!("results for query {id} that was not asked: discarded"));
                    return Handled::Nothing;
                }
                self.answered(slot, id, now);
                Handled::Results { slot, generation, id, items }
            }
            Reply::Error { id, message } => {
                match id {
                    Some(id) => self.answered(slot, id, now),
                    None => self.settle(slot),
                }
                Handled::QueryError { slot, message }
            }
            Reply::Stale { id } => {
                self.answered(slot, id, now);
                Handled::Nothing
            }
            Reply::Done(done) => {
                let found = self.pending.iter().position(|p| p.req == done.req && p.slot == slot && p.generation == generation);
                let Some(at) = found.filter(|&at| now.saturating_sub(self.pending[at].sent_ms) <= ACTION_WINDOW_MS) else {
                    self.note(slot, &format!("done for request {} that is unknown or older than 10 s: discarded", done.req));
                    if let Some(at) = found {
                        self.pending.remove(at);
                        self.settle(slot);
                    }
                    return Handled::Nothing;
                };
                let pending = self.pending.remove(at);
                self.settle(slot);
                Handled::Done { slot, effects: done.effects, keep_open: done.keep_open, error: done.error, item_id: pending.item_id, stay: pending.stay }
            }
            Reply::Exited { reason, .. } => {
                let id = self.plugins[slot as usize].id.clone();
                self.plugins[slot as usize].run = None;
                self.pending.retain(|p| p.slot != slot);
                let mut failed = false;
                if reason.counts_as_crash() && self.crashes.entry(id.clone()).or_default().record(Instant::now()) {
                    failed = self.failed.insert(id);
                }
                Handled::Exited { slot, failed }
            }
        }
    }

    fn note(&mut self, slot: u16, message: &str) {
        if let Some(run) = self.running(slot) {
            run.proc.note(message);
        }
    }

    /// Milliseconds until the next soft or hard timeout of an outstanding query, if one is outstanding.
    pub fn next_deadline(&self, now: u64) -> Option<u64> {
        self.plugins
            .iter()
            .filter_map(|p| {
                let run = p.run.as_ref().filter(|r| !r.killed)?;
                let (_, sent) = run.outstanding.first()?;
                let due = if run.slow { sent + HARD_TIMEOUT_MS } else { sent + p.manifest.query_timeout_ms };
                Some(due.saturating_sub(now).max(1))
            })
            .min()
    }

    /// Applies the timeouts that passed: kills a plugin that has not answered for 10 s, and returns
    /// the slots whose soft timeout (`query_timeout_ms`) passed since the last call.
    pub fn check_timeouts(&mut self, now: u64) -> Vec<u16> {
        let mut slow = Vec::new();
        for (slot, plugin) in self.plugins.iter_mut().enumerate() {
            let soft = plugin.manifest.query_timeout_ms;
            let Some(run) = plugin.run.as_mut().filter(|r| !r.killed) else { continue };
            let Some(&(_, sent)) = run.outstanding.first() else { continue };
            if now.saturating_sub(sent) >= HARD_TIMEOUT_MS {
                run.killed = true;
                run.proc.kill(crate::plugin_proc::ExitReason::HardTimeout);
            } else if now.saturating_sub(sent) >= soft && !run.slow {
                run.slow = true;
                slow.push(slot as u16);
            }
        }
        slow
    }

    pub fn any_running(&self) -> bool {
        self.plugins.iter().any(|p| p.run.is_some())
    }

    /// Shuts down the plugins unused for `IDLE_MS`. Returns the time until the next of the others
    /// expires, if any is left running.
    pub fn sweep(&mut self, now: u64) -> Option<u64> {
        let mut next: Option<u64> = None;
        for plugin in &mut self.plugins {
            let Some(run) = plugin.run.as_mut().filter(|r| !r.stopping) else { continue };
            let idle = now.saturating_sub(run.last_used);
            if idle >= IDLE_MS {
                run.proc.shutdown();
                run.stopping = true;
            } else {
                let left = IDLE_MS - idle;
                next = Some(next.map_or(left, |n| n.min(left)));
            }
        }
        next
    }

    fn stop(&mut self, slot: usize) {
        if let Some(run) = self.plugins.get_mut(slot).and_then(|p| p.run.as_mut()).filter(|r| !r.stopping) {
            run.proc.shutdown();
            run.stopping = true;
            run.outstanding.clear();
            run.waiting = None;
            run.queued = None;
        }
    }

    /// Asks every running plugin to exit (cpmenu is quitting; the job objects are the safety net).
    pub fn shutdown_all(&mut self) {
        for slot in 0..self.plugins.len() {
            self.stop(slot);
        }
    }

    /// Menu rows for a plugin's answer.
    pub fn build_rows(&self, slot: u16, generation: u16, query_id: u64, items: Vec<proto::Item>) -> Vec<Item> {
        let Some(plugin) = self.plugins.get(slot as usize) else { return Vec::new() };
        items
            .into_iter()
            .enumerate()
            .map(|(n, item)| {
                let icon = item
                    .icon
                    .as_deref()
                    .map(|i| plugin.dir.join(i))
                    .into_iter()
                    .chain(plugin.manifest.icon.as_deref().map(|i| plugin.dir.join(i)))
                    .find_map(|path| plugin_icon::load(&path));
                let key = if item.key.is_empty() { format!("#{n}") } else { item.key.clone() };
                let tag = item.tag.clone().unwrap_or_else(|| plugin.manifest.tag.clone());
                let row = PluginRow { slot, generation, plugin_id: plugin.id.clone(), query_id, tag, kind: RowKind::Result, busy: false, item };
                Item { name: row.item.title.clone(), tag: Tag::Plugin(slot as u8), id: format!("{slot}:{key}"), icon, extra: Some(Box::new(row)) }
            })
            .collect()
    }

    fn synthetic(&self, slot: u16, id: String, kind: RowKind, title: String, subtitle: String, score: u8) -> Item {
        let plugin = self.plugins.get(slot as usize);
        let item = proto::Item {
            key: id.clone(),
            title: title.clone(),
            subtitle,
            tag: None,
            icon: None,
            score,
            copy: None,
            data: None,
            default: None,
            alt: None,
            actions: Vec::new(),
        };
        let row = PluginRow {
            slot,
            generation: 0,
            plugin_id: plugin.map_or_else(String::new, |p| p.id.clone()),
            query_id: 0,
            tag: plugin.map_or_else(String::new, |p| p.manifest.tag.clone()),
            kind,
            busy: false,
            item,
        };
        Item { name: title, tag: Tag::Plugin(slot as u8), id: format!("{slot}:{id}"), icon: None, extra: Some(Box::new(row)) }
    }

    /// The first-run confirmation as two rows: IGNORE (first, so it is what Enter does) and ALLOW.
    pub fn approval_rows(&self, slot: u16) -> Vec<Item> {
        let Some(plugin) = self.plugins.get(slot as usize) else { return Vec::new() };
        let m = &plugin.manifest;
        let who = lang::t("plugin.who")
            .replace("{name}", &m.name)
            .replace("{version}", &m.version)
            .replace("{author}", if m.author.is_empty() { "?" } else { &m.author });
        let runs = lang::t("plugin.runs").replace("{command}", &m.run.display());
        vec![
            self.synthetic(slot, "approval:no".into(), RowKind::Approval(false), lang::t("plugin.ignore"), who, 100),
            self.synthetic(slot, "approval:yes".into(), RowKind::Approval(true), lang::t("plugin.allow").replace("{name}", &m.name), runs, 99),
        ]
    }

    /// One disabled row saying why a keyword query has no results.
    pub fn notice_row(&self, slot: u16, message: &str) -> Item {
        let shown: String = message.chars().take(100).collect();
        let title = lang::t("plugin.error").replace("{message}", &shown);
        self.synthetic(slot, "notice".into(), RowKind::Notice, title, String::new(), 100)
    }

    /// The row for a plugin that is off for this session.
    pub fn failed_row(&self, slot: u16) -> Item {
        let title = lang::t("plugin.failed").replace("{name}", self.name(slot));
        self.synthetic(slot, "notice".into(), RowKind::Notice, title, String::new(), 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_proc::ExitReason;
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    struct ChannelSink(Mutex<Sender<(u16, u16, Reply)>>);

    impl Sink for ChannelSink {
        fn deliver(&self, slot: u16, generation: u16, reply: Reply) -> bool {
            self.0.lock().unwrap().send((slot, generation, reply)).is_ok()
        }
    }

    fn fake_plugin_exe() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let path = exe.parent().unwrap().parent().unwrap().join("examples").join("fake_plugin.exe");
        assert!(path.exists(), "{} is built by cargo test", path.display());
        path
    }

    /// A temp tree with one plugin `fake` that runs the fake plugin in `mode`.
    struct Rig {
        host: Host,
        rx: Receiver<(u16, u16, Reply)>,
        base: PathBuf,
    }

    impl Rig {
        fn new(name: &str, mode: &str) -> Rig {
            Rig::with_args(name, &[mode])
        }

        fn with_args(name: &str, args: &[&str]) -> Rig {
            let base = std::env::temp_dir().join(format!("cpmenu-test-host-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let folder = base.join("plugins").join("fake");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::copy(fake_plugin_exe(), folder.join("fake.exe")).unwrap();
            let args = args.iter().map(|a| format!("{a:?}")).collect::<Vec<_>>().join(",");
            let manifest = format!(
                r#"{{"id":"fake","name":"Fake","version":"1","keyword":"fk","global":true,"query_timeout_ms":200,"run":{{"file":"fake.exe","args":[{args}]}},
                    "settings":[{{"key":"user","label":"User","type":"string","default":"a"}}]}}"#
            );
            std::fs::write(folder.join("plugin.json"), manifest).unwrap();
            let (tx, rx) = channel();
            let mut host = Host::with_dirs(base.join("plugins"), base.join("data"), base.join("logs"));
            host.attach(Arc::new(ChannelSink(Mutex::new(tx))));
            Rig { host, rx, base }
        }

        /// Scanned, and approved as the user would approve it.
        fn ready(name: &str, mode: &str, global: bool) -> Rig {
            Rig::ready_with(name, &[mode], global)
        }

        fn ready_with(name: &str, args: &[&str], global: bool) -> Rig {
            let mut rig = Rig::with_args(name, args);
            let key = {
                rig.host.sync(&PluginsConfig { enabled: true, entries: BTreeMap::new() });
                assert!(rig.host.rescan());
                rig.host.approval_key(0).unwrap()
            };
            let entry = PluginEntry { approved: Some(key), global, ..Default::default() };
            rig.host.sync(&PluginsConfig { enabled: true, entries: [("fake".to_owned(), entry)].into() });
            rig
        }

        fn next(&self) -> (u16, u16, Reply) {
            self.rx.recv_timeout(Duration::from_secs(10)).expect("a reply")
        }

        /// Applies replies until `pick` accepts the result of `handle`.
        fn until(&mut self, now: u64, pick: impl Fn(&Handled) -> bool) -> Handled {
            loop {
                let (slot, generation, reply) = self.next();
                let handled = self.host.handle(slot, generation, reply, now);
                if pick(&handled) {
                    return handled;
                }
            }
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            self.host.shutdown_all();
            std::thread::sleep(Duration::from_millis(200));
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn plugin_needs_approval_and_keyword_before_it_runs() {
        let mut rig = Rig::new("approval", "echo");
        rig.host.sync(&PluginsConfig { enabled: true, entries: BTreeMap::new() });
        assert!(rig.host.rescan() && !rig.host.rescan(), "the second scan finds nothing changed");
        assert_eq!(rig.host.keyword_table(), vec![("fk".to_owned(), 0)]);
        assert_eq!(rig.host.status(0), Status::NeedsApproval);
        let id = rig.host.new_batch("fk x");
        rig.host.send_query(0, id, "x", "fk", false, 0);
        assert!(!rig.host.any_running(), "an unapproved plugin never starts");
        let rows = rig.host.approval_rows(0);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].extra.as_ref().unwrap().kind, RowKind::Approval(false), "ignore comes first: it is the default");
        assert_eq!(rows[1].extra.as_ref().unwrap().kind, RowKind::Approval(true));
    }

    #[test]
    fn approval_goes_stale_when_version_or_command_change() {
        let mut rig = Rig::ready("stale-approval", "echo", false);
        assert_eq!(rig.host.status(0), Status::Ready);
        let folder = rig.base.join("plugins").join("fake");
        let text = std::fs::read_to_string(folder.join("plugin.json")).unwrap();
        std::fs::write(folder.join("plugin.json"), text.replace(r#""version":"1""#, r#""version":"2""#)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(rig.host.rescan());
        assert_eq!(rig.host.status(0), Status::NeedsApproval, "version changed");
        rig.host.sync(&PluginsConfig { enabled: true, entries: BTreeMap::new() });
        assert_eq!(rig.host.status(0), Status::NeedsApproval);
        rig.host.sync(&PluginsConfig { enabled: false, entries: BTreeMap::new() });
        assert_eq!(rig.host.status(0), Status::Unavailable);
        assert!(rig.host.keyword_table().is_empty(), "plugins off: no keyword is claimed");
    }

    #[test]
    fn round_trip_answers_the_query_and_a_late_one_is_not_asked_for() {
        let mut rig = Rig::ready("roundtrip", "echo", true);
        assert_eq!(rig.host.global_targets(), vec![0]);
        let id = rig.host.new_batch("hello");
        rig.host.send_query(0, id, "hello", "", true, 0);
        match rig.until(0, |h| matches!(h, Handled::Results { .. })) {
            Handled::Results { slot, id: got, items, .. } => {
                assert_eq!((slot, got), (0, id));
                assert_eq!(items[0].title, "echo: hello");
                let rows = rig.host.build_rows(slot, 7, id, items);
                assert_eq!((rows[0].id.as_str(), rows[0].tag), ("0:e", Tag::Plugin(0)));
                assert_eq!(rows[0].extra.as_ref().unwrap().tag, "FAKE", "manifest tag");
            }
            _ => unreachable!(),
        }
        let stale = rig.host.handle(0, 12345, Reply::Results { id, items: Vec::new() }, 0);
        assert_eq!(stale, Handled::Nothing, "a reply of another generation is dropped");
        let unasked = rig.host.handle(0, rig.host.plugins[0].run.as_ref().unwrap().generation, Reply::Results { id: 999, items: Vec::new() }, 0);
        assert_eq!(unasked, Handled::Nothing, "results nobody asked for are dropped");
    }

    #[test]
    fn queries_wait_for_ready_and_only_the_newest_is_kept() {
        let mut rig = Rig::ready("queue", "echo", false);
        let (a, b) = (rig.host.new_batch("a"), rig.host.new_batch("b"));
        rig.host.send_query(0, a, "a", "fk", false, 0);
        rig.host.send_query(0, b, "b", "fk", false, 0);
        let run = rig.host.plugins[0].run.as_ref().unwrap();
        assert!(!run.ready && run.queued.as_ref().map(|q| q.0) == Some(b) && run.outstanding.is_empty());
        match rig.until(0, |h| matches!(h, Handled::Results { .. })) {
            Handled::Results { id, items, .. } => assert_eq!((id, items[0].title.as_str()), (b, "echo: b")),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_third_query_replaces_the_waiting_one_and_goes_out_when_an_answer_arrives() {
        let mut rig = Rig::ready("coalesce", "hang", false);
        let ids: Vec<u64> = (0..4).map(|n| rig.host.new_batch(&n.to_string())).collect();
        rig.host.send_query(0, ids[0], "0", "fk", false, 0);
        let (slot, generation, reply) = rig.next();
        assert_eq!(rig.host.handle(slot, generation, reply, 0), Handled::Nothing, "ready");
        let outstanding = |rig: &Rig| rig.host.plugins[0].run.as_ref().unwrap().outstanding.iter().map(|o| o.0).collect::<Vec<_>>();
        assert_eq!(outstanding(&rig), vec![ids[0]], "the queued query went out on ready");
        for (n, id) in ids.iter().enumerate().skip(1) {
            rig.host.send_query(0, *id, &n.to_string(), "fk", false, 0);
        }
        assert_eq!(outstanding(&rig), vec![ids[0], ids[1]], "two in flight");
        assert_eq!(rig.host.plugins[0].run.as_ref().unwrap().waiting.as_ref().map(|w| w.0), Some(ids[3]), "the newest waits, 2 was replaced");
        assert_eq!(rig.host.handle(0, generation, Reply::Stale { id: ids[0] }, 5), Handled::Nothing);
        assert_eq!(outstanding(&rig), vec![ids[1], ids[3]], "an answer frees a slot for the waiting query");
        assert!(rig.host.plugins[0].run.as_ref().unwrap().waiting.is_none());
    }

    #[test]
    fn done_is_honored_once_for_a_recent_request_only() {
        let mut rig = Rig::ready("done", "echo", false);
        let id = rig.host.new_batch("x");
        rig.host.send_query(0, id, "x", "fk", false, 0);
        let Handled::Results { generation, items, .. } = rig.until(0, |h| matches!(h, Handled::Results { .. })) else { unreachable!() };
        let row = rig.host.build_rows(0, generation, id, items).remove(0);
        let row = row.extra.unwrap();
        let req = rig.host.send_action(&row, "star", "0:e", true, 1000).unwrap();
        let done = |req| Reply::Done(proto::Done { req, effects: vec![Effect::Copy("x".into())], keep_open: false, error: None });
        assert_eq!(rig.host.handle(0, generation, done(req + 5), 1500), Handled::Nothing, "unknown request");
        match rig.host.handle(0, generation, done(req), 1500) {
            Handled::Done { effects, item_id, stay, .. } => assert_eq!((effects, item_id.as_str(), stay), (vec![Effect::Copy("x".into())], "0:e", true)),
            other => panic!("{other:?}"),
        }
        assert_eq!(rig.host.handle(0, generation, done(req), 1600), Handled::Nothing, "answered requests are used up");
        let late = rig.host.send_action(&row, "star", "0:e", false, 2000).unwrap();
        assert_eq!(rig.host.handle(0, generation, done(late), 2000 + ACTION_WINDOW_MS + 1), Handled::Nothing, "older than 10 s");
        // A plugin that died takes its requests with it.
        let again = rig.host.send_action(&row, "star", "0:e", false, 3000).unwrap();
        rig.host.handle(0, generation, Reply::Exited { code: Some(1), reason: ExitReason::Exited }, 3000);
        assert!(rig.host.pending.is_empty(), "{again}");
    }

    #[test]
    fn three_crashes_fail_the_plugin_for_the_session() {
        let mut rig = Rig::ready("crashes", "crash", false);
        for attempt in 0..3 {
            let id = rig.host.new_batch("x");
            assert_eq!(rig.host.status(0), Status::Ready);
            rig.host.send_query(0, id, "x", "fk", false, 0);
            let Handled::Exited { failed, .. } = rig.until(0, |h| matches!(h, Handled::Exited { .. })) else { unreachable!() };
            assert_eq!(failed, attempt == 2);
        }
        assert_eq!(rig.host.status(0), Status::Failed);
        let id = rig.host.new_batch("y");
        rig.host.send_query(0, id, "y", "fk", false, 0);
        assert!(!rig.host.any_running(), "a failed plugin is not started again");
    }

    #[test]
    fn soft_timeout_is_reported_once_then_the_plugin_is_killed_at_the_hard_one() {
        let mut rig = Rig::ready("timeouts", "hang", false);
        let id = rig.host.new_batch("x");
        rig.host.send_query(0, id, "x", "fk", false, 0);
        let (slot, generation, reply) = rig.next();
        rig.host.handle(slot, generation, reply, 0);
        assert_eq!(rig.host.next_deadline(100), Some(100), "soft timeout of 200 ms from the send");
        assert!(rig.host.check_timeouts(100).is_empty());
        assert_eq!(rig.host.check_timeouts(250), vec![0]);
        assert!(rig.host.check_timeouts(260).is_empty(), "reported once");
        assert_eq!(rig.host.next_deadline(250), Some(HARD_TIMEOUT_MS - 250));
        assert!(rig.host.check_timeouts(HARD_TIMEOUT_MS).is_empty());
        let Handled::Exited { failed, .. } = rig.until(0, |h| matches!(h, Handled::Exited { .. })) else { unreachable!() };
        assert!(!failed, "one kill is one crash");
        assert_eq!(rig.host.next_deadline(0), None);
    }

    #[test]
    fn idle_plugins_are_swept_after_a_minute() {
        let mut rig = Rig::ready("idle", "echo", false);
        let id = rig.host.new_batch("x");
        rig.host.send_query(0, id, "x", "fk", false, 1000);
        rig.until(1000, |h| matches!(h, Handled::Results { .. }));
        assert!(rig.host.any_running());
        assert_eq!(rig.host.sweep(1000 + IDLE_MS - 1000), Some(1000), "still has a second to go");
        assert_eq!(rig.host.sweep(1000 + IDLE_MS), None);
        let Handled::Exited { failed, .. } = rig.until(0, |h| matches!(h, Handled::Exited { .. })) else { unreachable!() };
        assert!(!failed, "an idle shutdown is not a crash");
        assert!(!rig.host.any_running());
    }

    fn state_of(rig: &Rig, id: &str) -> PluginState {
        rig.host.infos().into_iter().find(|i| i.id == id).unwrap().state
    }

    #[test]
    fn infos_list_every_folder_with_its_state_and_broken_manifests_with_their_error() {
        let mut rig = Rig::new("infos", "echo");
        assert!(rig.host.infos().is_empty(), "nothing before the first scan");
        rig.host.sync(&PluginsConfig { enabled: true, entries: BTreeMap::new() });
        let bad = rig.base.join("plugins").join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("plugin.json"), r#"{"id":"other","name":"x","version":"1","keyword":"x","run":{"file":"a.py"}}"#).unwrap();
        assert!(rig.host.rescan());
        let infos = rig.host.infos();
        assert_eq!(infos.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["bad", "fake"], "sorted by folder");
        assert!(matches!(&infos[0].state, PluginState::ManifestError(e) if e.contains("must equal the folder name")), "{:?}", infos[0].state);
        assert_eq!((infos[1].name.as_str(), infos[1].version.as_str(), infos[1].keywords.clone()), ("Fake", "1", vec!["fk".to_owned()]));
        assert_eq!(infos[1].state, PluginState::NotApproved);
        assert!(infos[1].can_global && !infos[1].global && infos[1].enabled && !infos[1].approved);
        assert_eq!(infos[1].settings[0].value, plugins::SettingValue::String("a".into()), "the manifest's default");

        let key = rig.host.approval_key_of("fake").unwrap();
        assert_eq!(key, rig.host.approval_key(0).unwrap(), "slots count only the plugins that loaded");
        let entry = PluginEntry { approved: Some(key), ..Default::default() };
        let config = |entry: PluginEntry| PluginsConfig { enabled: true, entries: [("fake".to_owned(), entry)].into() };
        rig.host.sync(&config(entry.clone()));
        assert_eq!(state_of(&rig, "fake"), PluginState::Stopped);
        let id = rig.host.new_batch("x");
        rig.host.send_query(0, id, "x", "fk", false, 0);
        assert_eq!(state_of(&rig, "fake"), PluginState::Starting);
        rig.until(0, |h| matches!(h, Handled::Results { .. }));
        assert_eq!(state_of(&rig, "fake"), PluginState::Running);
        rig.host.sync(&config(PluginEntry { enabled: false, ..entry }));
        assert_eq!(state_of(&rig, "fake"), PluginState::Disabled, "switching it off wins over everything");
        rig.until(0, |h| matches!(h, Handled::Exited { .. }));
        rig.host.sync(&PluginsConfig { enabled: false, entries: BTreeMap::new() });
        assert!(rig.host.infos().is_empty(), "plugins off: the tab lists none");
    }

    #[test]
    fn a_keyword_held_by_another_plugin_is_reported_as_lost() {
        let mut rig = Rig::new("lost", "echo");
        let second = rig.base.join("plugins").join("fake2");
        std::fs::create_dir_all(&second).unwrap();
        std::fs::copy(fake_plugin_exe(), second.join("fake.exe")).unwrap();
        let manifest = r#"{"id":"fake2","name":"Two","version":"1","keyword":"fk","run":{"file":"fake.exe"}}"#;
        std::fs::write(second.join("plugin.json"), manifest).unwrap();
        rig.host.sync(&PluginsConfig { enabled: true, entries: BTreeMap::new() });
        assert!(rig.host.rescan());
        let lost: Vec<_> = rig.host.infos().into_iter().map(|i| (i.id, i.keyword_lost)).collect();
        assert_eq!(lost, [("fake".to_owned(), false), ("fake2".to_owned(), true)], "the lower id keeps the keyword");
        let entry = PluginEntry { keyword: Some("two".into()), ..Default::default() };
        rig.host.sync(&PluginsConfig { enabled: true, entries: [("fake2".to_owned(), entry)].into() });
        assert!(rig.host.infos().iter().all(|i| !i.keyword_lost), "an override resolves it");
        assert_eq!(rig.host.keyword_table(), vec![("fk".to_owned(), 0), ("two".to_owned(), 1)]);
    }

    #[test]
    fn a_changed_keyword_stops_the_plugin_and_a_changed_setting_reaches_it() {
        let mut rig = Rig::ready("settings", "echo", false);
        let id = rig.host.new_batch("x");
        rig.host.send_query(0, id, "x", "fk", false, 0);
        rig.until(0, |h| matches!(h, Handled::Results { .. }));
        let key = rig.host.approval_key(0).unwrap();
        let mut entry = PluginEntry { approved: Some(key), ..Default::default() };
        entry.settings.insert("user".into(), plugins::SettingValue::String("bob".into()));
        rig.host.sync(&PluginsConfig { enabled: true, entries: [("fake".to_owned(), entry.clone())].into() });
        rig.host.push_settings("fake");
        rig.host.push_settings("nobody");
        entry.keyword = Some("zz".into());
        rig.host.sync(&PluginsConfig { enabled: true, entries: [("fake".to_owned(), entry)].into() });
        assert!(rig.host.any_running(), "a keyword change alone doesn't stop it");
        rig.host.stop_plugin("fake");
        assert_eq!(state_of(&rig, "fake"), PluginState::Stopped, "stopping shows at once");
        let Handled::Exited { failed, .. } = rig.until(0, |h| matches!(h, Handled::Exited { .. })) else { unreachable!() };
        assert!(!failed);
        let log = std::fs::read_to_string(rig.base.join("logs").join("fake.log")).unwrap();
        assert!(log.contains(r#"settings: {"user":"bob"}"#), "{log}");
        assert_eq!(rig.host.keyword_table(), vec![("zz".to_owned(), 0)]);
    }

    #[test]
    fn switching_a_failed_plugin_off_and_on_gives_it_another_chance() {
        let mut rig = Rig::ready("retry", "crash", false);
        for _ in 0..3 {
            let id = rig.host.new_batch("x");
            rig.host.send_query(0, id, "x", "fk", false, 0);
            rig.until(0, |h| matches!(h, Handled::Exited { .. }));
        }
        assert_eq!(state_of(&rig, "fake"), PluginState::Failed);
        let key = rig.host.approval_key(0).unwrap();
        let config = |enabled| PluginsConfig { enabled: true, entries: [("fake".to_owned(), PluginEntry { enabled, approved: Some(key.clone()), ..Default::default() })].into() };
        rig.host.sync(&config(false));
        rig.host.sync(&config(true));
        assert_eq!(rig.host.status(0), Status::Ready);
        assert_eq!(state_of(&rig, "fake"), PluginState::Stopped);
        // The STATE row's Enter does the same without the round trip.
        for _ in 0..3 {
            let id = rig.host.new_batch("x");
            rig.host.send_query(0, id, "x", "fk", false, 0);
            rig.until(0, |h| matches!(h, Handled::Exited { .. }));
        }
        assert_eq!(rig.host.status(0), Status::Failed);
        rig.host.retry("fake");
        assert_eq!(rig.host.status(0), Status::Ready);
    }

    fn alive(pid: u32) -> bool {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code).is_ok();
            let _ = CloseHandle(handle);
            ok && code == 259
        }
    }

    /// 1 000 keystrokes into a plugin that needs 30 ms per query: the typing path (new batch +
    /// send) never waits for it, at most two queries are in flight, and when it is over no process
    /// is left.
    #[test]
    fn a_thousand_rapid_queries_into_a_slow_plugin_never_stall_the_typing_path() {
        let pidfile = std::env::temp_dir().join(format!("cpmenu-test-slow-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        let mut rig = Rig::ready_with("stress", &["slow", pidfile.to_str().unwrap()], false);
        // Typing starts with the plugin up, so what is measured is the slow plugin, not its launch.
        let first = rig.host.new_batch("warm up");
        rig.host.send_query(0, first, "warm up", "fk", false, 0);
        rig.until(0, |h| matches!(h, Handled::Results { .. }));
        let mut spent = Vec::with_capacity(1000);
        let mut answered = 0;
        for n in 0..1000 {
            let text = format!("query number {n}");
            let t = Instant::now();
            let id = rig.host.new_batch(&text);
            rig.host.send_query(0, id, &text, "fk", false, n);
            spent.push(t.elapsed());
            while let Ok((slot, generation, reply)) = rig.rx.try_recv() {
                if let Handled::Results { .. } = rig.host.handle(slot, generation, reply, n) {
                    answered += 1;
                }
            }
            let run = rig.host.plugins[0].run.as_ref().unwrap();
            assert!(run.outstanding.len() <= IN_FLIGHT);
            if n % 100 == 99 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        spent.sort();
        let (median, p99, worst) = (spent[500], spent[989], spent[999]);
        assert!(p99 < Duration::from_millis(5), "median {median:?}, p99 {p99:?}, worst {worst:?}");
        assert!(worst < Duration::from_millis(100), "worst {worst:?}");
        let pid: u32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert!(alive(pid), "the slow plugin is up while it works");
        // Whatever is still in flight drains, and the newest query is answered last.
        while rig.host.plugins[0].run.as_ref().is_some_and(|r| !r.outstanding.is_empty() || r.waiting.is_some()) {
            if let Handled::Results { .. } = rig.until(2000, |h| matches!(h, Handled::Results { .. } | Handled::Nothing)) {
                answered += 1;
            }
        }
        assert!(answered >= 1, "at least the final query is answered");
        rig.host.shutdown_all();
        rig.until(0, |h| matches!(h, Handled::Exited { .. }));
        let deadline = Instant::now() + Duration::from_secs(3);
        while alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(pid), "plugin process {pid} survived the shutdown");
        assert!(!rig.host.any_running());
        let _ = std::fs::remove_file(&pidfile);
    }

    #[test]
    fn effects_open_only_links_and_existing_paths() {
        assert!(open_allowed("https://example.com") && open_allowed("mailto:a@b.c"));
        assert!(!open_allowed("file:///C:/Windows/notepad.exe") && !open_allowed("javascript:alert(1)") && !open_allowed("ms-settings:network"));
        assert!(open_allowed(&std::env::temp_dir().to_string_lossy()));
        assert!(!open_allowed(r"C:\definitely\not\here\x.txt"));
        assert!(!open_allowed(""));
    }

    #[test]
    fn menu_stays_open_for_stay_set_query_and_keep_open_only() {
        assert!(!keeps_open(&[Effect::Copy("x".into())], false));
        assert!(keeps_open(&[Effect::Copy("x".into())], true));
        assert!(keeps_open(&[Effect::SetQuery("q".into())], false));
        assert!(keeps_open(&[Effect::KeepOpen(true)], false));
        assert!(!keeps_open(&[Effect::KeepOpen(true), Effect::Hide], false));
        assert!(!keeps_open(&[], false));
    }
}
