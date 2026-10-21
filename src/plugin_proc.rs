//! Plugin processes (docs/plugins.md section 3): one supervisor/writer thread and one reader thread
//! per running plugin, a Job Object around each so the whole process tree dies with the plugin or
//! with cpmenu, and replies handed to a `Sink`. The window is the only HWND-aware sink
//! (`WindowSink`); tests use a channel. None of the methods on `Proc` block on the plugin.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, PipeReader, PipeWriter, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, HWND, LPARAM, SetHandleInformation, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{PCWSTR, PWSTR};

use crate::plugin_proto::{self as proto, Done, FromPlugin, Item, Line, LogLevel, ToPlugin};
use crate::plugins::Command;

/// Posted to the menu window: `wparam` = `encode_wparam(generation, slot)`, `lparam` = `Box<Reply>`.
/// WM_APP + 1 command, + 2 capture, + 3 files are taken.
pub const WM_PLUGIN: u32 = WM_APP + 4;

/// What the reader and supervisor tell the window.
#[derive(Debug, PartialEq)]
pub enum Reply {
    Ready { protocol: Option<u32> },
    Results { id: u64, items: Vec<Item> },
    Done(Done),
    Error { id: Option<u64>, message: String },
    /// The answer to query `id` arrived but was stale and was dropped at the reader. The host still
    /// needs to know the query is no longer outstanding (it frees the coalescing slot).
    Stale { id: u64 },
    /// The last reply of a generation; nothing from it follows.
    Exited { code: Option<i32>, reason: ExitReason },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExitReason {
    /// The process ended without being asked to.
    Exited,
    /// Asked to stop (idle, disabled, quit).
    Shutdown,
    SpawnFailed(String),
    ReadyTimeout,
    HardTimeout,
    Flood,
    Strikes,
}

impl ExitReason {
    /// Counts toward the 3-crashes-in-60-s limit.
    pub fn counts_as_crash(&self) -> bool {
        !matches!(self, Self::Shutdown)
    }
}

/// Receives replies on the reader/supervisor threads. Returns false if the reply could not be delivered.
pub trait Sink: Send + Sync + 'static {
    fn deliver(&self, slot: u16, generation: u16, reply: Reply) -> bool;
}

pub fn encode_wparam(generation: u16, slot: u16) -> usize {
    ((generation as usize) << 16) | slot as usize
}

/// `(generation, slot)`
pub fn decode_wparam(wparam: usize) -> (u16, u16) {
    ((wparam >> 16) as u16, wparam as u16)
}

/// Posts replies to the menu window as `WM_PLUGIN`.
pub struct WindowSink(pub isize);

impl Sink for WindowSink {
    fn deliver(&self, slot: u16, generation: u16, reply: Reply) -> bool {
        let raw = Box::into_raw(Box::new(reply));
        let posted = unsafe { PostMessageW(Some(HWND(self.0 as _)), WM_PLUGIN, WPARAM(encode_wparam(generation, slot)), LPARAM(raw as isize)) };
        if posted.is_err() {
            drop(unsafe { Box::from_raw(raw) });
        }
        posted.is_ok()
    }
}

/// Takes ownership of the reply a `WM_PLUGIN` message carries.
///
/// # Safety
/// `lparam` must be the `lparam` of a `WM_PLUGIN` posted by `WindowSink`, and be taken once.
pub unsafe fn take_reply(lparam: LPARAM) -> Box<Reply> {
    unsafe { Box::from_raw(lparam.0 as *mut Reply) }
}

/// A new generation number for a (re)started plugin; never 0.
pub fn next_generation() -> u16 {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    loop {
        let generation = COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_add(1) as u16;
        if generation != 0 {
            return generation;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub ready_timeout: Duration,
    pub hard_timeout: Duration,
    /// How long a plugin gets to exit after `shutdown` before the job is terminated.
    pub grace: Duration,
    /// How often the supervisor checks the timeouts while one can fire.
    pub tick: Duration,
    pub line_cap: usize,
    pub messages_per_sec: u32,
    pub flood_secs: u32,
    pub max_strikes: usize,
    pub strike_window: Duration,
    pub logs_per_sec: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(5),
            hard_timeout: Duration::from_secs(10),
            grace: Duration::from_secs(1),
            tick: Duration::from_millis(100),
            line_cap: proto::MAX_LINE,
            messages_per_sec: 200,
            flood_secs: 3,
            max_strikes: 10,
            strike_window: Duration::from_secs(60),
            logs_per_sec: 20,
        }
    }
}

/// Messages-per-window counter: trips when more than `per_sec * secs` messages arrive within one
/// window, which is what "200 per second sustained over 3 s" amounts to.
pub struct Flood {
    window: Duration,
    limit: u32,
    start: Instant,
    count: u32,
}

impl Flood {
    pub fn new(per_sec: u32, secs: u32, now: Instant) -> Self {
        Self { window: Duration::from_secs(secs as u64), limit: per_sec.saturating_mul(secs), start: now, count: 0 }
    }

    /// Counts one message; true once the window's budget is exceeded.
    pub fn hit(&mut self, now: Instant) -> bool {
        if now.duration_since(self.start) >= self.window {
            self.start = now;
            self.count = 0;
        }
        self.count += 1;
        self.count > self.limit
    }
}

/// Bad lines (invalid JSON, oversized) within a sliding window.
pub struct Strikes {
    window: Duration,
    max: usize,
    hits: VecDeque<Instant>,
}

impl Strikes {
    pub fn new(max: usize, window: Duration) -> Self {
        Self { window, max, hits: VecDeque::new() }
    }

    /// Records a strike; true once `max` of them fall within the window.
    pub fn strike(&mut self, now: Instant) -> bool {
        while self.hits.front().is_some_and(|t| now.duration_since(*t) > self.window) {
            self.hits.pop_front();
        }
        self.hits.push_back(now);
        self.hits.len() >= self.max
    }
}

/// Crash history of one plugin for this session: three crashes within 60 s fail it.
#[derive(Default)]
pub struct Crashes(VecDeque<Instant>);

impl Crashes {
    pub const LIMIT: usize = 3;
    pub const WINDOW: Duration = Duration::from_secs(60);

    /// Records a crash; true when the plugin should be marked failed.
    pub fn record(&mut self, now: Instant) -> bool {
        while self.0.front().is_some_and(|t| now.duration_since(*t) > Self::WINDOW) {
            self.0.pop_front();
        }
        self.0.push_back(now);
        self.0.len() >= Self::LIMIT
    }

    /// Toggling a failed plugin off and on in the PLUGINS tab gives it a fresh start.
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// Everything needed to start one plugin process.
pub struct Spec {
    pub id: String,
    pub command: Command,
    pub dir: PathBuf,
    pub data_dir: PathBuf,
    pub env: Vec<(String, String)>,
    /// stderr and host events are appended here.
    pub log: Option<PathBuf>,
    /// The `init` line, built by the caller (settings, language, protocol).
    pub init: String,
    pub max_results: usize,
    pub limits: Limits,
}

struct Job(HANDLE);

// A job handle is just a kernel object reference; any thread may terminate or close it.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    fn new() -> windows::core::Result<Job> {
        unsafe {
            let job = Job(CreateJobObjectW(None, PCWSTR::null())?);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            Ok(job)
        }
    }

    fn assign(&self, process: HANDLE) -> windows::core::Result<()> {
        unsafe { AssignProcessToJobObject(self.0, process) }
    }

    fn terminate(&self) {
        let _ = unsafe { TerminateJobObject(self.0, 1) };
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

enum Out {
    Line(String),
    Shutdown,
    ReaderDone,
}

struct Shared {
    /// Set by the supervisor before the process exists; a kill requested earlier is applied then.
    job: OnceLock<Job>,
    start: Instant,
    reason: Mutex<Option<ExitReason>>,
    ready: AtomicBool,
    pending: AtomicU32,
    activity_ms: AtomicU64,
    shutting_down: AtomicBool,
    finished: AtomicBool,
    log: Mutex<Option<LogFile>>,
}

/// Largest a plugin log grows before it moves to `<id>.log.1`.
pub const LOG_CAP: u64 = 1024 * 1024;

/// The host's handle on a plugin's log. The plugin's stderr is a second handle to the same file;
/// when the file rotates, that handle keeps writing to the `.1` file until the process ends.
struct LogFile {
    file: File,
    path: PathBuf,
}

/// `<path>.1`
fn rotated(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// Moves a log of at least `cap` bytes to `<path>.1`, replacing the one already there.
pub fn rotate_log(path: &Path, cap: u64) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() >= cap) {
        let _ = std::fs::rename(path, rotated(path));
    }
}

fn open_log(path: &Path) -> Option<LogFile> {
    let _ = std::fs::create_dir_all(path.parent()?);
    rotate_log(path, LOG_CAP);
    let file = OpenOptions::new().create(true).append(true).open(path).ok()?;
    Some(LogFile { file, path: path.to_owned() })
}

impl Shared {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn touch(&self) {
        self.activity_ms.store(self.now_ms(), Ordering::Relaxed);
    }

    fn write_log(&self, tag: &str, message: &str) {
        let Ok(mut guard) = self.log.lock() else { return };
        let Some(log) = guard.as_mut() else { return };
        let line = format!("[{tag}] {message}\n");
        // The file's real size: the plugin's stderr grows it too.
        if log.file.metadata().is_ok_and(|m| m.len() + line.len() as u64 > LOG_CAP) {
            rotate_log(&log.path, 0);
            match OpenOptions::new().create(true).append(true).open(&log.path) {
                Ok(file) => log.file = file,
                Err(_) => return,
            }
        }
        let _ = log.file.write_all(line.as_bytes());
    }

    fn event(&self, message: &str) {
        self.write_log("host", message);
    }

    /// Records why (the first reason wins) and terminates the whole process tree.
    fn kill(&self, reason: ExitReason, why: &str) {
        let first = {
            let mut slot = self.reason.lock().unwrap_or_else(|e| e.into_inner());
            let first = slot.is_none();
            slot.get_or_insert(reason);
            first
        };
        if first {
            self.event(&format!("killed: {why}"));
        }
        if let Some(job) = self.job.get() {
            job.terminate();
        }
    }

    fn killed(&self) -> bool {
        self.reason.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    fn overdue(&self, limits: &Limits) -> Option<(ExitReason, &'static str)> {
        if !self.ready.load(Ordering::Relaxed) {
            return (self.start.elapsed() >= limits.ready_timeout).then_some((ExitReason::ReadyTimeout, "no ready in time"));
        }
        let idle = Duration::from_millis(self.now_ms().saturating_sub(self.activity_ms.load(Ordering::Relaxed)));
        (self.pending.load(Ordering::Relaxed) > 0 && idle >= limits.hard_timeout).then_some((ExitReason::HardTimeout, "hard timeout"))
    }
}

/// A running (or just ended) plugin process. Dropping it asks the plugin to shut down.
pub struct Proc {
    tx: SyncSender<Out>,
    shared: Arc<Shared>,
}

impl Proc {
    /// Starts the supervisor thread, which spawns the process; this call itself only creates two
    /// threads. `latest_query` is the host's newest query id: results for any other id are dropped
    /// by the reader before they are posted.
    pub fn spawn(spec: Spec, slot: u16, generation: u16, sink: Arc<dyn Sink>, latest_query: Arc<AtomicU64>) -> Proc {
        let (tx, rx) = sync_channel(16);
        let shared = Arc::new(Shared {
            job: OnceLock::new(),
            start: Instant::now(),
            reason: Mutex::new(None),
            ready: AtomicBool::new(false),
            pending: AtomicU32::new(0),
            activity_ms: AtomicU64::new(0),
            shutting_down: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            log: Mutex::new(None),
        });
        let ctx = Ctx { shared: shared.clone(), sink: sink.clone(), latest: latest_query, slot, generation, tx: tx.clone() };
        let started = std::thread::Builder::new().name(format!("plugin-{}", spec.id)).stack_size(128 * 1024).spawn(move || supervise(spec, ctx, rx));
        if started.is_err() {
            shared.finished.store(true, Ordering::Relaxed);
            sink.deliver(slot, generation, Reply::Exited { code: None, reason: ExitReason::SpawnFailed("could not start a thread".into()) });
        }
        Proc { tx, shared }
    }

    /// Queues a line for the plugin without blocking. False if the queue is full or the plugin is gone.
    pub fn send(&self, line: String) -> bool {
        match self.tx.try_send(Out::Line(line)) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    /// `send` for a query or action: the plugin now owes a `results`/`done`/`error`, and the hard
    /// timeout runs until it arrives. Counted even when the queue is full, so a plugin that stopped
    /// reading is killed too.
    pub fn send_request(&self, line: String) -> bool {
        if self.shared.pending.fetch_add(1, Ordering::Relaxed) == 0 {
            self.shared.touch();
        }
        self.send(line)
    }

    /// Writes a host event line to the plugin's log (e.g. why a reply was discarded).
    pub fn note(&self, message: &str) {
        self.shared.event(message);
    }

    /// Sets how many requests the plugin still owes. The host calls this once it knows better than
    /// the reader's count: a plugin may silently drop a query that a newer one superseded.
    pub fn set_pending(&self, owed: u32) {
        let before = self.shared.pending.swap(owed, Ordering::Relaxed);
        if before == 0 && owed > 0 {
            self.shared.touch();
        }
    }

    /// Asks the plugin to exit; the supervisor waits `grace`, then terminates the process tree.
    pub fn shutdown(&self) {
        self.shared.shutting_down.store(true, Ordering::Relaxed);
        if self.tx.try_send(Out::Shutdown).is_err() {
            // Queue full: the supervisor may be stuck writing to a plugin that doesn't read.
            self.shared.kill(ExitReason::Shutdown, "shutdown with a full queue");
        }
    }

    /// Terminates the process tree now (the window's hard-timeout hook).
    pub fn kill(&self, reason: ExitReason) {
        self.shared.kill(reason, "requested by the host");
    }

    /// False once the process and both threads are done (the `Exited` reply has been delivered).
    pub fn is_running(&self) -> bool {
        !self.shared.finished.load(Ordering::Relaxed)
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        if self.is_running() {
            self.shutdown();
        }
    }
}

struct Ctx {
    shared: Arc<Shared>,
    sink: Arc<dyn Sink>,
    latest: Arc<AtomicU64>,
    slot: u16,
    generation: u16,
    tx: SyncSender<Out>,
}

impl Ctx {
    fn post(&self, reply: Reply) {
        if !self.sink.deliver(self.slot, self.generation, reply) {
            self.shared.event("reply dropped: nobody to deliver it to");
        }
    }
}

fn supervise(spec: Spec, ctx: Ctx, rx: Receiver<Out>) {
    let shared = ctx.shared.clone();
    let code = run_process(&spec, &ctx, rx);
    let reason = shared.reason.lock().unwrap_or_else(|e| e.into_inner()).take();
    let reason = reason.unwrap_or(if shared.shutting_down.load(Ordering::Relaxed) { ExitReason::Shutdown } else { ExitReason::Exited });
    shared.event(&format!("exited: code {code:?}, {reason:?}"));
    shared.finished.store(true, Ordering::Relaxed);
    ctx.post(Reply::Exited { code, reason });
}

/// Runs the process to its end and returns its exit code. Spawn failures end the run early and
/// leave their reason in `Shared`.
fn run_process(spec: &Spec, ctx: &Ctx, rx: Receiver<Out>) -> Option<i32> {
    let shared = &ctx.shared;
    let limits = &spec.limits;
    let fail = |message: String| -> Option<i32> {
        shared.kill(ExitReason::SpawnFailed(message.clone()), &message);
        None
    };

    let log_file = spec.log.as_deref().and_then(open_log);
    let stderr = log_file.as_ref().and_then(|l| l.file.try_clone().ok());
    *shared.log.lock().unwrap_or_else(|e| e.into_inner()) = log_file;
    let _ = std::fs::create_dir_all(&spec.data_dir);

    let Ok(job) = Job::new() else { return fail("could not create a job object".into()) };
    let job = shared.job.get_or_init(|| job);

    let Started { child, mut stdin, stdout, main_thread } = match spawn_suspended(spec, stderr) {
        Ok(started) => started,
        Err(e) => return fail(format!("{}: {e}", spec.command.exe)),
    };
    shared.event(&format!("started pid {}: {} {}", child.pid, spec.command.exe, spec.command.args.join(" ")));
    // The process has not run a single instruction yet: it joins the job first, so nothing it
    // starts can escape, and only then is it let go.
    if let Err(e) = job.assign(child.handle()) {
        child.kill();
        return fail(format!("could not assign the process to the job: {e}"));
    }
    if shared.killed() {
        job.terminate();
    }
    unsafe { ResumeThread(HANDLE(main_thread.as_raw_handle())) };
    drop(main_thread);
    let reader_ctx = Ctx { shared: shared.clone(), sink: ctx.sink.clone(), latest: ctx.latest.clone(), slot: ctx.slot, generation: ctx.generation, tx: ctx.tx.clone() };
    let (max_results, reader_limits) = (spec.max_results, limits.clone());
    let reader = std::thread::Builder::new()
        .name(format!("plugin-{}-rd", spec.id))
        .stack_size(128 * 1024)
        .spawn(move || read_loop(stdout, reader_ctx, max_results, reader_limits));

    let mut stdin = stdin.write_all(spec.init.as_bytes()).and_then(|_| stdin.flush()).is_ok().then_some(stdin);
    loop {
        let watching = !shared.ready.load(Ordering::Relaxed) || shared.pending.load(Ordering::Relaxed) > 0;
        let next = if watching {
            match rx.recv_timeout(limits.tick) {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(Out::Shutdown),
            }
        } else {
            Some(rx.recv().unwrap_or(Out::Shutdown))
        };
        match next {
            Some(Out::Line(line)) => {
                if let Some(pipe) = stdin.as_mut()
                    && pipe.write_all(line.as_bytes()).and_then(|_| pipe.flush()).is_err()
                {
                    shared.event("stdin closed by the plugin");
                    stdin = None;
                }
            }
            Some(Out::Shutdown) | Some(Out::ReaderDone) => break,
            None => {
                if let Some((reason, why)) = shared.overdue(limits)
                    && !shared.killed()
                {
                    shared.kill(reason, why);
                }
                if child.try_wait().is_some() {
                    break;
                }
            }
        }
    }

    if shared.shutting_down.load(Ordering::Relaxed)
        && !shared.killed()
        && let Some(pipe) = stdin.as_mut()
    {
        let _ = pipe.write_all(ToPlugin::Shutdown.to_line().as_bytes()).and_then(|_| pipe.flush());
    }
    drop(stdin);
    let deadline = Instant::now() + limits.grace;
    let mut status = None;
    while !shared.killed() {
        match child.try_wait() {
            Some(code) => {
                status = Some(code);
                break;
            }
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            None => break,
        }
    }
    // Lets a reader blocked on a full queue finish.
    drop(rx);
    // Also after a clean exit: whatever the plugin left running goes with it.
    job.terminate();
    let status = status.or_else(|| child.wait());
    if let Ok(handle) = reader {
        let _ = handle.join();
    }
    status
}

/// A started plugin process; the handle is closed when this is dropped.
struct Child {
    process: OwnedHandle,
    pid: u32,
}

impl Child {
    fn handle(&self) -> HANDLE {
        HANDLE(self.process.as_raw_handle())
    }

    fn exit_code(&self) -> Option<i32> {
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.handle(), &mut code) }.ok().map(|_| code as i32)
    }

    /// The exit code if the process has ended.
    fn try_wait(&self) -> Option<i32> {
        (unsafe { WaitForSingleObject(self.handle(), 0) } == WAIT_OBJECT_0).then(|| self.exit_code()).flatten()
    }

    /// Waits for the process to end; the caller has made sure it will.
    fn wait(&self) -> Option<i32> {
        unsafe { WaitForSingleObject(self.handle(), INFINITE) };
        self.exit_code()
    }

    fn kill(&self) {
        let _ = unsafe { TerminateProcess(self.handle(), 1) };
        self.wait();
    }
}

struct Started {
    child: Child,
    stdin: PipeWriter,
    stdout: PipeReader,
    /// Resumed once the process is in its job.
    main_thread: OwnedHandle,
}

/// `arg` quoted for a command line so the child's argument parser gives it back unchanged.
fn push_arg(line: &mut String, arg: &str) {
    if !line.is_empty() {
        line.push(' ');
    }
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        line.push_str(arg);
        return;
    }
    line.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                line.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        line.push(c);
    }
    line.extend(std::iter::repeat_n('\\', backslashes));
    line.push('"');
}

/// This process's environment with `extra` added (replacing same-named variables), as the
/// double-NUL terminated, name-sorted UTF-16 block `CreateProcessW` takes.
fn env_block(extra: &[(String, String)]) -> Vec<u16> {
    let wide = |s: &std::ffi::OsStr| s.encode_wide().collect::<Vec<u16>>();
    let mut vars: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
    for (name, value) in std::env::vars_os() {
        vars.insert(name.to_string_lossy().to_uppercase(), (name, value));
    }
    for (name, value) in extra {
        vars.insert(name.to_uppercase(), (name.into(), value.into()));
    }
    let mut block = Vec::new();
    for (name, value) in vars.values() {
        block.extend(wide(name));
        block.push(b'=' as u16);
        block.extend(wide(value));
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    block
}

fn set_inherit(handle: HANDLE, inherit: bool) {
    let _ = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, if inherit { HANDLE_FLAG_INHERIT } else { HANDLE_FLAGS(0) }) };
}

/// Starts the plugin's process suspended, with pipes for stdin/stdout and `stderr` (or the null
/// device) as its stderr. Exactly those three handles are inherited, so a plugin never holds a
/// pipe end of another one, and no other thread's spawn can hand them on.
fn spawn_suspended(spec: &Spec, stderr: Option<File>) -> io::Result<Started> {
    let (child_in, parent_in) = io::pipe()?;
    let (parent_out, child_out) = io::pipe()?;
    let stderr = match stderr {
        Some(file) => file,
        None => OpenOptions::new().write(true).open("NUL")?,
    };
    let handle_of = |h: &dyn AsRawHandle| HANDLE(h.as_raw_handle());
    let handles = [handle_of(&child_in), handle_of(&child_out), handle_of(&stderr)];
    for handle in handles {
        set_inherit(handle, true);
    }
    set_inherit(handle_of(&parent_in), false);
    set_inherit(handle_of(&parent_out), false);

    let mut line = String::new();
    push_arg(&mut line, &spec.command.exe);
    for arg in &spec.command.args {
        push_arg(&mut line, arg);
    }
    let mut line: Vec<u16> = line.encode_utf16().chain([0]).collect();
    let env = env_block(&spec.env);
    let dir: Vec<u16> = spec.dir.as_os_str().encode_wide().chain([0]).collect();

    let mut size = 0usize;
    // Asks how much room one attribute needs; fails by design.
    let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut size) };
    let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
    let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
    unsafe {
        InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)?;
        let listed = UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, Some(handles.as_ptr().cast()), size_of_val(&handles), None, None);
        let startup = STARTUPINFOEXW {
            StartupInfo: STARTUPINFOW {
                cb: size_of::<STARTUPINFOEXW>() as u32,
                dwFlags: STARTF_USESTDHANDLES,
                hStdInput: handles[0],
                hStdOutput: handles[1],
                hStdError: handles[2],
                ..Default::default()
            },
            lpAttributeList: list,
        };
        let mut info = PROCESS_INFORMATION::default();
        let created = listed.and_then(|_| {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                true,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                Some(env.as_ptr().cast()),
                PCWSTR(dir.as_ptr()),
                (&startup as *const STARTUPINFOEXW).cast::<STARTUPINFOW>(),
                &mut info,
            )
        });
        let error = created.is_err().then(io::Error::last_os_error);
        DeleteProcThreadAttributeList(list);
        if let Some(error) = error {
            return Err(error);
        }
        Ok(Started {
            child: Child { process: OwnedHandle::from_raw_handle(info.hProcess.0), pid: info.dwProcessId },
            stdin: parent_in,
            stdout: parent_out,
            main_thread: OwnedHandle::from_raw_handle(info.hThread.0),
        })
    }
}

/// Per-second budget for `log` lines; excess lines are dropped and summarized once.
struct LogBudget {
    per_sec: u32,
    second: Instant,
    used: u32,
    dropped: u32,
}

impl LogBudget {
    fn new(per_sec: u32, now: Instant) -> Self {
        Self { per_sec, second: now, used: 0, dropped: 0 }
    }

    /// Whether this line may be written; `summary` is how many were dropped in the second just ended.
    fn allow(&mut self, now: Instant) -> (bool, u32) {
        let mut summary = 0;
        if now.duration_since(self.second) >= Duration::from_secs(1) {
            summary = std::mem::take(&mut self.dropped);
            self.second = now;
            self.used = 0;
        }
        if self.used < self.per_sec {
            self.used += 1;
            (true, summary)
        } else {
            self.dropped += 1;
            (false, summary)
        }
    }
}

fn read_loop(stdout: PipeReader, ctx: Ctx, max_results: usize, limits: Limits) {
    let shared = &ctx.shared;
    let mut reader = BufReader::with_capacity(8 * 1024, stdout);
    let started = Instant::now();
    let mut flood = Flood::new(limits.messages_per_sec, limits.flood_secs, started);
    let mut strikes = Strikes::new(limits.max_strikes, limits.strike_window);
    let mut logs = LogBudget::new(limits.logs_per_sec, started);
    loop {
        let line = match proto::read_bounded_line(&mut reader, limits.line_cap) {
            Ok(Line::Text(line)) => line,
            Ok(Line::Oversized) => {
                shared.touch();
                shared.event(&format!("oversized line (over {} bytes) skipped", limits.line_cap));
                if strikes.strike(Instant::now()) {
                    shared.kill(ExitReason::Strikes, "too many bad lines");
                }
                continue;
            }
            Ok(Line::Eof) | Err(_) => break,
        };
        let now = Instant::now();
        shared.touch();
        if flood.hit(now) {
            shared.kill(ExitReason::Flood, "too many messages");
            continue;
        }
        let message = match proto::parse_line(&line, max_results) {
            Ok(Some(message)) => message,
            Ok(None) => continue,
            Err(e) => {
                shared.event(&format!("bad JSON line: {e}"));
                if strikes.strike(now) {
                    shared.kill(ExitReason::Strikes, "too many bad lines");
                }
                continue;
            }
        };
        match message {
            FromPlugin::Ready { protocol } => {
                shared.ready.store(true, Ordering::Relaxed);
                ctx.post(Reply::Ready { protocol });
            }
            FromPlugin::Results { id, items } => {
                answered(shared);
                if id == ctx.latest.load(Ordering::Relaxed) {
                    ctx.post(Reply::Results { id, items });
                } else {
                    shared.event(&format!("stale results for query {id} dropped"));
                    ctx.post(Reply::Stale { id });
                }
            }
            FromPlugin::Done(done) => {
                answered(shared);
                ctx.post(Reply::Done(done));
            }
            FromPlugin::Error { id, message } => {
                answered(shared);
                shared.write_log("error", &message);
                match id {
                    Some(id) if id != ctx.latest.load(Ordering::Relaxed) => ctx.post(Reply::Stale { id }),
                    _ => ctx.post(Reply::Error { id, message }),
                }
            }
            FromPlugin::Log { level, message } => {
                let (allowed, summary) = logs.allow(now);
                if summary > 0 {
                    shared.write_log("warn", &format!("{summary} log lines dropped (limit {} per second)", limits.logs_per_sec));
                }
                if allowed {
                    let tag = match level {
                        LogLevel::Debug => "debug",
                        LogLevel::Info => "info",
                        LogLevel::Warn => "warn",
                        LogLevel::Error => "error",
                    };
                    shared.write_log(tag, &message);
                }
            }
            FromPlugin::Unknown => {}
        }
    }
    // Wakes the supervisor if it is waiting for work; if the queue is full it is busy writing and
    // sees the exit when that write fails.
    let _ = ctx.tx.send(Out::ReaderDone);
}

fn answered(shared: &Shared) {
    let _ = shared.pending.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    struct ChannelSink(Mutex<Sender<(u16, u16, Reply)>>);

    impl Sink for ChannelSink {
        fn deliver(&self, slot: u16, generation: u16, reply: Reply) -> bool {
            self.0.lock().unwrap().send((slot, generation, reply)).is_ok()
        }
    }

    fn sink() -> (Arc<dyn Sink>, Receiver<(u16, u16, Reply)>) {
        let (tx, rx) = channel();
        (Arc::new(ChannelSink(Mutex::new(tx))), rx)
    }

    fn fake_plugin() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let path = exe.parent().unwrap().parent().unwrap().join("examples").join("fake_plugin.exe");
        assert!(path.exists(), "{} is built by cargo test; run it, not the test binary alone", path.display());
        path
    }

    fn spec(id: &str, args: &[&str], limits: Limits) -> Spec {
        let dir = std::env::temp_dir();
        let settings = Default::default();
        let init = ToPlugin::Init { protocol: 1, host: "test", lang: "en", plugin_dir: "d", data_dir: "d", settings: &settings }.to_line();
        Spec {
            id: id.to_owned(),
            command: Command { exe: fake_plugin().to_string_lossy().into_owned(), args: args.iter().map(|a| (*a).to_owned()).collect() },
            data_dir: dir.join(format!("cpmenu-test-data-{id}-{}", std::process::id())),
            dir,
            env: Vec::new(),
            log: None,
            init,
            max_results: 8,
            limits,
        }
    }

    fn fast() -> Limits {
        Limits { tick: Duration::from_millis(20), grace: Duration::from_millis(300), ..Limits::default() }
    }

    fn query(id: u64, text: &str) -> String {
        ToPlugin::Query { id, text, keyword: "", global: false, max: 8 }.to_line()
    }

    fn start(args: &[&str], limits: Limits) -> (Proc, Receiver<(u16, u16, Reply)>, Arc<AtomicU64>) {
        let (sink, rx) = sink();
        let latest = Arc::new(AtomicU64::new(0));
        let name = format!("t{}", next_generation());
        (Proc::spawn(spec(&name, args, limits), 3, next_generation(), sink, latest.clone()), rx, latest)
    }

    fn next(rx: &Receiver<(u16, u16, Reply)>) -> Reply {
        rx.recv_timeout(Duration::from_secs(10)).expect("a reply").2
    }

    /// Replies up to and including `Exited`.
    fn until_exit(rx: &Receiver<(u16, u16, Reply)>) -> (Vec<Reply>, Option<i32>, ExitReason) {
        let mut seen = Vec::new();
        loop {
            match next(rx) {
                Reply::Exited { code, reason } => return (seen, code, reason),
                other => seen.push(other),
            }
        }
    }

    #[test]
    fn round_trip_init_ready_query_results_and_graceful_shutdown() {
        let (proc, rx, latest) = start(&["echo"], fast());
        assert_eq!(next(&rx), Reply::Ready { protocol: Some(1) });
        latest.store(42, Ordering::Relaxed);
        assert!(proc.send_request(query(42, "hello")));
        let Reply::Results { id, items } = next(&rx) else { panic!() };
        assert_eq!((id, items[0].title.as_str()), (42, "echo: hello"));

        let action = ToPlugin::Action { req: 7, id: "x", item: "k", data: None, query_id: 42 }.to_line();
        assert!(proc.send_request(action));
        assert!(matches!(next(&rx), Reply::Done(d) if d.req == 7));

        let t = Instant::now();
        proc.shutdown();
        let (rest, code, reason) = until_exit(&rx);
        assert!(rest.is_empty());
        assert_eq!((code, reason), (Some(0), ExitReason::Shutdown));
        assert!(t.elapsed() < Duration::from_millis(1500));
        assert!(!proc.is_running());
    }

    #[test]
    fn results_for_an_old_query_are_dropped_at_the_reader() {
        let (proc, rx, latest) = start(&["echo"], fast());
        assert!(matches!(next(&rx), Reply::Ready { .. }));
        latest.store(5, Ordering::Relaxed);
        proc.send_request(query(4, "old"));
        proc.send_request(query(5, "new"));
        assert_eq!(next(&rx), Reply::Stale { id: 4 }, "the reply to query 4 reaches the window only as a notice");
        let Reply::Results { id, .. } = next(&rx) else { panic!() };
        assert_eq!(id, 5);
        proc.shutdown();
        assert!(until_exit(&rx).0.is_empty());
    }

    #[test]
    fn stderr_and_log_messages_go_to_the_log_file() {
        let log = std::env::temp_dir().join(format!("cpmenu-test-log-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let (sink, rx) = sink();
        let mut s = spec("logger", &["echo"], fast());
        s.log = Some(log.clone());
        let proc = Proc::spawn(s, 0, next_generation(), sink, Arc::new(AtomicU64::new(0)));
        assert!(matches!(next(&rx), Reply::Ready { .. }));
        proc.shutdown();
        until_exit(&rx);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("hello from stderr"), "{text}");
        assert!(text.contains("[info] fake plugin up"), "{text}");
        assert!(text.contains("[host] started pid") && text.contains("[host] exited: code Some(0)"), "{text}");
    }

    #[test]
    fn oversized_line_is_skipped_and_the_next_message_still_arrives() {
        let (proc, rx, latest) = start(&["oversized"], fast());
        assert!(matches!(next(&rx), Reply::Ready { .. }));
        latest.store(1, Ordering::Relaxed);
        proc.send_request(query(1, "x"));
        let Reply::Results { id, items } = next(&rx) else { panic!() };
        assert_eq!((id, items.len()), (1, 1));
        proc.shutdown();
        until_exit(&rx);
    }

    #[test]
    fn a_stream_of_bad_lines_is_killed_as_strikes() {
        let (_proc, rx, _) = start(&["badjson"], fast());
        let (_, _, reason) = until_exit(&rx);
        assert_eq!(reason, ExitReason::Strikes);
        assert!(reason.counts_as_crash());
    }

    #[test]
    fn a_flooding_plugin_is_killed() {
        let (_proc, rx, _) = start(&["flood"], fast());
        let t = Instant::now();
        let (_, _, reason) = until_exit(&rx);
        assert_eq!(reason, ExitReason::Flood);
        assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    }

    #[test]
    fn a_plugin_that_never_answers_dies_at_the_hard_timeout() {
        let (proc, rx, latest) = start(&["hang"], Limits { hard_timeout: Duration::from_millis(400), ..fast() });
        assert!(matches!(next(&rx), Reply::Ready { .. }));
        std::thread::sleep(Duration::from_millis(600));
        assert!(proc.is_running(), "no request outstanding: idle is fine");
        latest.store(1, Ordering::Relaxed);
        let t = Instant::now();
        proc.send_request(query(1, "x"));
        let (_, _, reason) = until_exit(&rx);
        assert_eq!(reason, ExitReason::HardTimeout);
        assert!(t.elapsed() >= Duration::from_millis(350) && t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_plugin_that_never_says_ready_dies_at_the_ready_timeout() {
        let (_proc, rx, _) = start(&["silent"], Limits { ready_timeout: Duration::from_millis(300), ..fast() });
        assert_eq!(until_exit(&rx).2, ExitReason::ReadyTimeout);
    }

    #[test]
    fn the_host_can_kill_a_plugin_with_the_hook() {
        let (proc, rx, _) = start(&["echo"], fast());
        assert!(matches!(next(&rx), Reply::Ready { .. }));
        proc.kill(ExitReason::HardTimeout);
        assert_eq!(until_exit(&rx).2, ExitReason::HardTimeout);
    }

    #[test]
    fn an_unrequested_exit_is_reported_with_its_code() {
        let (_proc, rx, _) = start(&["crash"], fast());
        let (_, code, reason) = until_exit(&rx);
        assert_eq!((code, reason.clone()), (Some(3), ExitReason::Exited));
        assert!(reason.counts_as_crash());
    }

    #[test]
    fn a_missing_executable_is_a_spawn_failure() {
        let (sink, rx) = sink();
        let mut s = spec("missing", &[], fast());
        s.command.exe = r"C:\no\such\plugin.exe".to_owned();
        let _proc = Proc::spawn(s, 0, next_generation(), sink, Arc::new(AtomicU64::new(0)));
        let (_, code, reason) = until_exit(&rx);
        assert_eq!(code, None);
        assert!(matches!(reason, ExitReason::SpawnFailed(_)));
    }

    #[test]
    fn a_plugin_that_ignores_shutdown_is_terminated_after_the_grace() {
        let (proc, rx, _) = start(&["deaf"], fast());
        assert!(matches!(next(&rx), Reply::Ready { .. }));
        let t = Instant::now();
        proc.shutdown();
        let (_, _, reason) = until_exit(&rx);
        assert_eq!(reason, ExitReason::Shutdown);
        assert!(t.elapsed() >= Duration::from_millis(250) && t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[test]
    fn sending_never_blocks_even_if_the_plugin_does_not_read() {
        let (proc, rx, _) = start(&["silent"], Limits { ready_timeout: Duration::from_secs(30), ..fast() });
        let padding = "x".repeat(900);
        let t = Instant::now();
        let accepted = (0..300).filter(|i| proc.send_request(query(*i, &padding))).count();
        assert!(t.elapsed() < Duration::from_secs(1), "the typing path took {:?}", t.elapsed());
        assert!(accepted < 300, "a full queue refuses instead of waiting");
        proc.shutdown();
        let (_, _, reason) = until_exit(&rx);
        assert_eq!(reason, ExitReason::Shutdown, "the supervisor stuck in a write is released by the kill");
    }

    fn alive(pid: u32) -> bool {
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code).is_ok();
            let _ = CloseHandle(handle);
            ok && code == 259
        }
    }

    #[test]
    fn the_whole_process_tree_is_gone_after_the_job_is_terminated() {
        let pidfile = std::env::temp_dir().join(format!("cpmenu-test-child-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        let (proc, rx, _) = start(&["tree", pidfile.to_str().unwrap()], fast());
        assert!(matches!(next(&rx), Reply::Ready { .. }));
        let child: u32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert!(alive(child), "the grandchild runs while the plugin does");
        proc.kill(ExitReason::HardTimeout);
        until_exit(&rx);
        let deadline = Instant::now() + Duration::from_secs(3);
        while alive(child) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(child), "grandchild {child} survived");
    }

    #[test]
    fn a_restart_gets_a_new_generation_and_replies_carry_their_own() {
        let (sink, rx) = sink();
        let first = next_generation();
        let one = Proc::spawn(spec("gen1", &["echo"], fast()), 7, first, sink.clone(), Arc::new(AtomicU64::new(0)));
        let (slot, generation, reply) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!((slot, generation, reply), (7, first, Reply::Ready { protocol: Some(1) }));
        one.shutdown();
        loop {
            let (_, g, reply) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(g, first);
            if matches!(reply, Reply::Exited { .. }) {
                break;
            }
        }
        let second = next_generation();
        assert_ne!(first, second);
        assert_ne!(second, 0);
        let two = Proc::spawn(spec("gen2", &["echo"], fast()), 7, second, sink, Arc::new(AtomicU64::new(0)));
        let (_, generation, reply) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!((generation, reply), (second, Reply::Ready { protocol: Some(1) }));
        two.shutdown();
        let (_, g, _) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(g, second);
    }

    #[test]
    fn dropping_the_handle_shuts_the_plugin_down() {
        let (proc, rx, _) = start(&["echo"], fast());
        assert!(matches!(next(&rx), Reply::Ready { .. }));
        drop(proc);
        assert_eq!(until_exit(&rx).2, ExitReason::Shutdown);
    }

    #[test]
    fn arguments_are_quoted_so_the_child_parses_them_back() {
        let mut line = String::new();
        for arg in ["plain", "with space", "", r"C:\a b\", r#"say "hi""#, r"back\slash"] {
            push_arg(&mut line, arg);
        }
        assert_eq!(line, r#"plain "with space" "" "C:\a b\\" "say \"hi\"" back\slash"#);
    }

    #[test]
    fn the_environment_block_overrides_and_ends_with_two_nuls() {
        let block = env_block(&[("CPMENU_TEST_VAR".to_owned(), "x y".to_owned())]);
        let text = String::from_utf16_lossy(&block);
        assert!(text.contains("CPMENU_TEST_VAR=x y\0"), "{text:?}");
        assert!(text.ends_with("\0\0"));
    }

    #[test]
    fn a_log_over_the_cap_moves_to_dot_one_and_a_small_one_stays() {
        let dir = std::env::temp_dir().join(format!("cpmenu-test-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("p.log");
        std::fs::write(&log, "small").unwrap();
        rotate_log(&log, 100);
        assert!(log.exists() && !dir.join("p.log.1").exists());
        std::fs::write(&log, "x".repeat(100)).unwrap();
        std::fs::write(dir.join("p.log.1"), "older").unwrap();
        rotate_log(&log, 100);
        assert!(!log.exists());
        assert_eq!(std::fs::read_to_string(dir.join("p.log.1")).unwrap().len(), 100, "the previous .1 is replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_events_rotate_the_log_while_the_plugin_runs() {
        let dir = std::env::temp_dir().join(format!("cpmenu-test-rotate-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("live.log");
        std::fs::write(&path, "x".repeat(LOG_CAP as usize - 10)).unwrap();
        let shared = Shared {
            job: OnceLock::new(),
            start: Instant::now(),
            reason: Mutex::new(None),
            ready: AtomicBool::new(false),
            pending: AtomicU32::new(0),
            activity_ms: AtomicU64::new(0),
            shutting_down: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            log: Mutex::new(None),
        };
        let file = OpenOptions::new().create(true).append(true).open(&path).unwrap();
        *shared.log.lock().unwrap() = Some(LogFile { file, path: path.clone() });
        shared.event("this line does not fit");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[host] this line does not fit\n");
        assert_eq!(std::fs::metadata(dir.join("live.log.1")).unwrap().len(), LOG_CAP - 10);
        *shared.log.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wparam_packs_generation_and_slot() {
        assert_eq!(decode_wparam(encode_wparam(0xABCD, 0x0012)), (0xABCD, 0x0012));
        assert_eq!(decode_wparam(encode_wparam(1, 255)), (1, 255));
    }

    #[test]
    fn flood_trips_only_above_the_sustained_rate() {
        let t0 = Instant::now();
        let mut flood = Flood::new(200, 3, t0);
        assert!((0..600).all(|_| !flood.hit(t0)), "600 in 3 s is exactly at the limit");
        assert!(flood.hit(t0 + Duration::from_secs(1)));
        let mut flood = Flood::new(200, 3, t0);
        for window in 0..5 {
            let at = t0 + Duration::from_secs(3 * window);
            assert!((0..600).all(|_| !flood.hit(at)), "the budget resets every window");
        }
    }

    #[test]
    fn strikes_expire_and_trip_at_the_limit() {
        let t0 = Instant::now();
        let mut strikes = Strikes::new(10, Duration::from_secs(60));
        assert!((0..9).all(|i| !strikes.strike(t0 + Duration::from_secs(i))));
        assert!(strikes.strike(t0 + Duration::from_secs(9)));
        let mut slow = Strikes::new(10, Duration::from_secs(60));
        assert!((0..40).all(|i| !slow.strike(t0 + Duration::from_secs(i * 10))), "one per 10 s never adds up");
    }

    #[test]
    fn three_crashes_in_a_minute_fail_the_plugin() {
        let t0 = Instant::now();
        let mut crashes = Crashes::default();
        assert!(!crashes.record(t0) && !crashes.record(t0 + Duration::from_secs(10)));
        assert!(crashes.record(t0 + Duration::from_secs(20)));
        crashes.clear();
        assert!(!crashes.record(t0 + Duration::from_secs(30)));
        let mut spread = Crashes::default();
        assert!((0..10).all(|i| !spread.record(t0 + Duration::from_secs(i * 40))));
    }

    #[test]
    fn log_budget_drops_excess_and_reports_once() {
        let t0 = Instant::now();
        let mut budget = LogBudget::new(20, t0);
        assert!((0..20).all(|_| budget.allow(t0).0));
        assert!(!budget.allow(t0).0 && !budget.allow(t0).0);
        assert_eq!(budget.allow(t0 + Duration::from_secs(1)), (true, 2));
        assert_eq!(budget.allow(t0 + Duration::from_secs(1)), (true, 0));
    }
}
