# cpmenu plugins — design (implemented: phases 1-7, manifest/config, protocol, supervisor, window integration, actions/trust, PLUGINS tab, hardening)

Phase 6-7 decisions (where the code differs from or fills in the text below):
- The PLUGINS tab is the fourth tab. Rows: PLUGINS (master switch, `[behavior] plugins`), PLUGINS FOLDER (a row that acts
  as the tab-level button: creates the folder if missing, opens Explorer), then a section per folder with a
  `plugin.json`: heading `NAME vVERSION - AUTHOR`, STATE (RUNNING / STARTING / STOPPED / FAILED / DISABLED / NOT APPROVED /
  ERROR), ENABLED, APPROVE (only while not approved), KEYWORD, GLOBAL (only if the manifest has `global`), one row per
  manifest setting (string typed, bool toggled, choice stepped), OPEN LOG. A broken manifest shows only STATE = ERROR;
  the error text appears in the note line while the row is selected. With the master switch off no plugin is listed.
- Everything applies at once. ENABLED/KEYWORD/APPROVE/GLOBAL go to `[plugins.<id>]`, settings to
  `[plugins.<id>.settings]` (a running plugin gets a `settings` message). Switching a plugin off shuts it down; a changed
  or removed keyword rebuilds the routing and shuts it down (it starts again on the next query). Switching ENABLED off
  and on, or Enter on STATE, gives a FAILED plugin another chance.
- KEYWORD: Enter types (a-z 0-9 _ -, lower-cased, max 16); a keyword that another plugin answers to, or an invalid one,
  is flagged in the note line and not saved; empty or Delete returns to the manifest keyword. A keyword lost to a plugin
  with a lower id shows `kw !` and a note.
- APPROVE asks first ("ENTER AGAIN"); the second Enter or click on the same row writes `approved`. This is the only way
  to approve a global plugin. The note line shows the command it will run as the user. GLOBAL, once on, shows the
  one-line privacy note (every typed query goes to the plugin).
- `plugin check <folder>` starts the plugin the same way the menu does (job object, all limits), with a temp data dir
  and log (`%TEMP%\cpmenu-plugin-check\<id>`), default settings, `init`, `query ""` (keyword plugin) or `query "test"`
  (global-only), then `shutdown`. FAIL: bad manifest, no `ready` in 5 s, exit or kill before the answer, no answer in
  10 s, killed for flood/strikes. WARN: slow start or answer (> `query_timeout_ms`), non-JSON stdout lines, results
  without actions, ignored `shutdown` (terminated after 1 s), missing author/description, ignored icon. The exe is a
  GUI program: output goes to the launching console, or to the pipe/file if redirected. From PowerShell use
  `cpmenu.exe plugin check <folder> | Out-Host` (or `Start-Process -Wait -PassThru`) to wait for it and to see the exit code.
- Logs rotate at 1 MB: a log of 1 MB or more is moved to `<id>.log.1` (replacing the old one) when the plugin starts and
  also while it runs (host and `log` lines); stderr is a second handle to the file, so after a mid-run rotation the
  plugin's stderr keeps going to `.log.1` until that process ends.
- The job-assignment race is closed: the process is created suspended (`CreateProcessW`, `CREATE_SUSPENDED`), assigned
  to its job, then resumed. Exactly three handles are inherited (stdin, stdout, stderr, through a handle list), so a plugin
  never holds another plugin's pipe.

Phase 4-5 decisions (where the code differs from or fills in the text below):
- `src/plugin_host.rs` holds the UI-thread state (queue before `ready`, 2-in-flight coalescing, timeouts, `done`
  validity, idle sweep); `src/plugin_icon.rs` decodes row icons with WIC (LRU 64, loaded when a reply arrives, not per
  visible row).
- `plugin_proc` posts `Reply::Stale { id }` when the reader drops a stale `results`/`error`, so a coalesced waiting
  query is still released; the host corrects `Proc::set_pending` because a plugin may silently drop a superseded query.
- A keyword needs the following space (`gh `); `gh` alone is an ordinary search. Keyword rows: score desc, cap
  `max_results`. Global rows: cap `min(max_results, 5)` each, 10 total, only for plain queries of >= 2 chars.
- First-run confirmation is two list rows, IGNORE (selected) and ALLOW; an auto-repeated Enter is ignored. IGNORE clears
  the query. ALLOW writes `approved`, then re-asks the query. Global plugins never prompt.
- Timers: PLUGIN_TIMER (next soft/hard timeout, visible only), IDLE_TIMER (armed in `hide()` if a plugin runs, killed in
  `show()` which also sweeps), NOTICE_TIMER (3 s header message). The soft timeout shows "<name> is not answering" in the
  header for a keyword query; the 10 s hard timeout kills the plugin (supervisor and window both enforce it).
- Ctrl+C on a plugin row copies its `copy` text and hides the menu. Static `do` effects hide the menu unless `stay`,
  `set_query` or `keep_open`; a `done` after the menu hid only runs its effects (a `set_query` needs a visible menu).
- Discovery re-scan happens in `show()` (and when the settings panel opens) by `plugins::stamp`; a running plugin
  survives a re-scan only if its slot and manifest are unchanged. Broken manifests are kept for the PLUGINS tab.

Status: implemented (phases 1-7). Protocol version: **1**. Not compatible with Flow Launcher (we borrow the idea of a
manifest + keyword + query -> results, nothing else).

A plugin is a separate process that talks newline-delimited JSON (one JSON object per line, UTF-8) over its
stdin/stdout. cpmenu never loads plugin code into its own process, so a plugin cannot crash the menu, and
the menu's idle budget (~30-40 MB open, 0% CPU idle, no timers when hidden) is untouched when no plugin runs.

New dependencies: `serde` (derive) + `serde_json` (approved). Windows features added: `Win32_System_JobObjects`,
`Win32_Graphics_Imaging`, `Win32_System_Console` (plugin check output).

---------------------------------------------------------------------------------------------------

## 1. Folder layout and `plugin.json`

```
%APPDATA%\cpmenu\plugins\
  github\                    <- folder name MUST equal "id"
    plugin.json
    main.py                  <- whatever "run" points at
    icon.png                 <- optional
  %APPDATA%\cpmenu\plugin-data\github\    <- created by cpmenu; the plugin's private writable dir
  %APPDATA%\cpmenu\plugin-logs\github.log <- see section 6
```

Discovery: scan `plugins\*\plugin.json` at startup and again on show when the `plugins` directory mtime or any
manifest mtime changed (same mtime-compare pattern as `config::modified()`). No directory watcher, no timer.
Manifests are parsed once and kept (a few hundred bytes each); a broken manifest is listed in the PLUGINS tab
with its error and never loaded.

### Schema

```json
{
  "id": "github",
  "name": "GitHub",
  "version": "1.2.0",
  "author": "Jane Doe",
  "description": "Search repositories and issues",
  "protocol_min": 1,
  "keyword": "gh",
  "keywords": ["gh", "git"],
  "global": false,
  "tag": "GH",
  "icon": "icon.png",
  "run": { "interpreter": "python", "file": "main.py", "args": [] },
  "max_results": 8,
  "debounce_ms": 120,
  "query_timeout_ms": 1500,
  "settings": [
    { "key": "user",  "label": "GitHub user",  "type": "string", "default": "" },
    { "key": "forks", "label": "Include forks", "type": "bool",   "default": false },
    { "key": "sort",  "label": "Sort by",       "type": "choice", "default": "updated",
      "choices": ["updated", "stars", "name"] }
  ]
}
```

| Field | Req | Rules |
|---|---|---|
| `id` | yes | `[a-z0-9_-]{1,32}`, equals folder name. Key in `config.toml` (`[plugins.<id>]`). |
| `name`, `version`, `author`, `description` | name+version yes | Plain strings. `version` is free-form but is part of the trust key (section 5). Shown in the PLUGINS tab. |
| `protocol_min` | no (1) | Lowest protocol the plugin can speak. Host supports 1..=N; if `protocol_min` > N the plugin is listed as "needs newer cpmenu" and not started. |
| `keyword` / `keywords` | one of, or `global` | Default activation keyword(s). `[a-z0-9_-]{1,16}`, never `>` or `?` (reserved for quick commands). First keyword is the default shown in settings; user override replaces the whole list with one keyword. |
| `global` | no (false) | If true the plugin *can* receive every query (>= 2 chars) with its results merged below apps; it only does so once the user sets `global = true` in `[plugins.<id>]` (PLUGINS tab) and the plugin is enabled and approved. Independent of keywords; a plugin may have both. |
| `tag` | no (`name` upper-cased) | Tag label on rows, max 8 chars, uppercased by the host. |
| `icon` | no | Path relative to the plugin folder (png/ico/jpg, <= 256 KB). Used for the plugin's rows that give no icon of their own, and in the PLUGINS tab. |
| `run` | yes | Either `{ "file": "tool.exe", "args": [...] }` (an exe or anything `CreateProcess` can start, path relative to the folder, must resolve inside it) or `{ "interpreter": "python"\|"node"\|"powershell"\|"pwsh", "file": "main.py", "args": [...] }`. The host expands the interpreter to a fixed command line (below). No shell, no free-form command strings. |
| `max_results` | no (8) | Host clamps to 1..=20; also sent to the plugin in `query.max`. |
| `debounce_ms` | no (120) | Clamped to 0..=500. Wait after the last keystroke before querying this plugin (the existing `SEARCH_TIMER` uses 120 ms). |
| `query_timeout_ms` | no (1500) | Soft timeout (section 2.7). Clamped to 200..=5000. |
| `settings` | no | Up to 16 entries, types `string`, `bool`, `choice`. Rendered as rows in the PLUGINS tab, stored in `[plugins.<id>.settings]`, sent in `init` and in `settings` updates. No `secret` type: values are plaintext in `config.toml` (tell users so; plugins that need tokens should read them from their own data dir or env). |

Interpreter expansion (host-fixed, so a manifest cannot inject flags). `run.file` and `icon` must be relative
paths inside the plugin folder (no `..`, no drive, no `:`); the file must exist or the plugin is listed with an error:

| interpreter | command line |
|---|---|
| `python` | `python -u -X utf8 <file> <args>` (falls back to `py -3 -u -X utf8` if `python` is not on PATH, or the first one found is in a `WindowsApps` folder - the Store alias - and `py` exists) |
| `node` | `node <file> <args>` |
| `powershell` | `powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File <file> <args>` |
| `pwsh` | same with `pwsh.exe` |

Process environment: working dir = the plugin folder; extra env `CPMENU_PROTOCOL=1`, `CPMENU_PLUGIN_ID`,
`CPMENU_PLUGIN_DIR`, `CPMENU_DATA_DIR`. stdin/stdout piped, stderr appended to the plugin's log file
(a file handle, no extra thread), `CREATE_NO_WINDOW`.

### `config.toml`

```toml
[behavior]
plugins = true                 # master switch; false = no scan, nothing ever started

[plugins.github]
enabled = true                 # default true after discovery, but see "approved"
global = false                 # default false: the user opts in (PLUGINS tab); the manifest's "global" only says the plugin can
keyword = "gh"                 # optional override of the manifest keyword(s)
approved = "1.2.0|python main.py"   # written by cpmenu after the user confirms (section 5)

[plugins.github.settings]
user = "jane"
forks = false
```

Edits go through `config::set_value` (toml_edit keeps comments). Unknown plugin ids in config are ignored, not deleted.

---------------------------------------------------------------------------------------------------

## 2. Protocol (version 1)

Rules for both directions: one JSON object per line, `\n` terminated, UTF-8 (host tolerates a BOM on the first
line, which PowerShell 5.1 emits). Every object has `"type"`. Unknown types and unknown fields are ignored
(forward compatible). A line that is not valid JSON is logged and skipped (it counts as a strike, 2.7).
Plugins must flush stdout after every line.

Host -> plugin: `init`, `query`, `action`, `settings`, `shutdown`.
Plugin -> host: `ready`, `results`, `done`, `log`, `error`.

### 2.1 Handshake

Sent right after the process starts:

```json
{"type":"init","protocol":1,"host":"cpmenu 0.1.0","lang":"pl","plugin_dir":"C:\\Users\\me\\AppData\\Roaming\\cpmenu\\plugins\\github","data_dir":"C:\\Users\\me\\AppData\\Roaming\\cpmenu\\plugin-data\\github","settings":{"user":"jane","forks":false,"sort":"updated"}}
```

`protocol` is the highest version the host speaks that is >= the manifest's `protocol_min`. `lang` is the resolved
UI language code (`lang::` result, e.g. `en`, `pl`). Theme colors are deliberately not sent: plugins have no
rendering, they only supply text and an icon path (cheap to add later as an optional `theme` field).

The plugin answers within 5 s:

```json
{"type":"ready","protocol":1}
```

The plugin may reply with a lower `protocol` than offered if it only knows older ones; the host then speaks that.
Queries sent before `ready` are queued by the host (max 1 pending query, the newest) and delivered after it.

### 2.2 Query

```json
{"type":"query","id":42,"text":"cpmenu plugins","keyword":"gh","global":false,"max":8}
```

- `id`: u64, strictly increasing across all plugins (one counter per keystroke batch). The plugin echoes it in
  `results`. Plugins may drop any query that has a lower id than one already received.
- `text`: the query with the keyword and one following space removed. For a keyword plugin, typing `gh ` (keyword +
  space, nothing else) sends `text:""` so the plugin can show hints. For a global query `keyword` is `""`.
- `max`: how many items the host will keep.

The host never has more than 2 unanswered queries per plugin: if a third would be sent, the newest replaces the
waiting one (coalescing) and it is sent when a `results` for the older one arrives. This is what protects a slow
plugin from a typing burst.

### 2.3 Results

```json
{"type":"results","id":42,"items":[
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
]}
```

Item fields (all optional except `title`):

| Field | Meaning / limit |
|---|---|
| `key` | Stable id of the row within this reply. Used only to keep the selection when a replacement `results` arrives. |
| `title` | Row name, <= 200 chars. Rendered uppercase like other rows. |
| `subtitle` | Second line (same style as the file path line), <= 300 chars. |
| `tag` | Overrides the manifest tag for this row, <= 8 chars. |
| `icon` | Absolute path or path relative to the plugin dir to png/ico/jpg <= 256 KB. Loaded lazily for visible rows only, decoded with WIC into the same `Icon` type apps use, cached by path+mtime, LRU of 64. Missing/invalid -> manifest icon -> none. Remote URLs are never fetched. |
| `score` | 0..=100 (default 50). Ordering among plugin rows only (section 4). |
| `copy` | Text for "copy" (Ctrl+C on the selected row if no text is selected in the query, and the COPY row in the action list), <= 4 KB. |
| `data` | Opaque JSON <= 4 KB, returned verbatim in `action`. Lets the plugin stay stateless (it may be restarted between query and action). |
| `default` | Action id run by Enter. Absent -> first action. No actions -> Enter does nothing. |
| `alt` | Action id run by Ctrl+Enter (optional). |
| `actions` | <= 8 entries shown on Right arrow / right click, in this order. `id` (unique in the item), `label` <= 40 chars, optional `do` (a built-in effect run by the host without asking the plugin), optional `stay` (true: keep the menu open, wait for `done`). An action without `do` is sent to the plugin as an `action` message. |

A later `results` for the same `id` **replaces** that plugin's previous rows for that id, so a plugin can send a fast
partial answer and a complete one afterwards. Excess items are dropped (cap `max_results`), over-long strings are
truncated, malformed items are skipped, not fatal.

### 2.4 Action invocation

Sent when the user picks an action that has no `do` (also for `default`/`alt` that point to one):

```json
{"type":"action","req":7,"id":"star","item":"r1","data":{"repo":"cpmenu/cpmenu"},"query_id":42}
```

`req` is a host-unique u64 for this invocation. The plugin answers with `done`:

```json
{"type":"done","req":7,"do":[{"copy":"Starred cpmenu/cpmenu"}],"keep_open":false}
```

```json
{"type":"done","req":7,"set_query":"gh issues ","keep_open":true}
```

```json
{"type":"done","req":7,"error":"Not logged in"}
```

Without `stay`, the host hides the menu immediately after sending `action` (so the UI never waits on a plugin);
the `done` that follows is still executed. With `stay:true` the menu stays, the row shows a dimmed state, and the
menu closes or not according to `keep_open` (default false) when `done` arrives. `error` shows a one-line
notice in the header area (replacing the query hint for 3 s via one-shot timer only while visible) and is logged.

Validity of `done`: executed only if `req` matches an invocation sent in the last 10 s and not yet answered, and the
plugin is still running. Anything else is logged and discarded. A plugin cannot trigger effects out of the blue.

### 2.5 Built-in effects (`do`)

`do` is one effect object or an array (<= 4 effects, executed in order). Exactly one key per object:

| Effect | Example | Host behavior |
|---|---|---|
| `open` | `{"open":"https://example.com"}` / `{"open":"C:\\dir\\file.txt"}` | `ShellExecuteW` "open" on a URL (http/https/mailto only) or an existing path. Same code as quick-command OPEN. |
| `copy` | `{"copy":"text"}` | Clipboard via `commands::copy`. <= 64 KB. |
| `run` | `{"run":{"exe":"C:\\tools\\a.exe","args":["-x"],"cwd":"C:\\tools","admin":false}}` | `CreateProcess`/`ShellExecute` (no shell string, args are an array). `admin:true` uses `runas` (UAC prompt, not silent). |
| `set_query` | `{"set_query":"gh issues "}` | Replaces the query text and re-runs the search. Implies `keep_open`. |
| `keep_open` / `hide` | `{"keep_open":true}` | Only meaningful in `done`; static `do` effects on an action hide the menu unless `stay` is set. |

`run` gives a plugin nothing it did not already have (it runs as the user); it exists so a plugin can launch
things with the user's elevation prompt and normal foreground rights, which a child process of a hidden plugin
would not get.

### 2.6 Log, error, shutdown

```json
{"type":"log","level":"info","message":"fetched 8 repos in 230 ms"}
{"type":"error","id":42,"message":"HTTP 403 rate limited"}
{"type":"shutdown"}
```

- `log` (`debug|info|warn|error`): written to the plugin log file, rate-limited to 20 lines/s, message <= 2 KB.
- `error`: the plugin's answer for query `id` is "no results, because ..."; the host logs it and, for a keyword
  query, shows one disabled row `PLUGIN ERROR - <message>` (<= 100 chars) so the user sees why. For a global plugin
  the error is only logged.
- `shutdown` (host -> plugin): finish and exit within 1 s. The host closes stdin right after, and terminates the
  process tree if it is still alive after 1 s.
- `settings` (host -> plugin): `{"type":"settings","settings":{...}}` when the user edits a value in the PLUGINS
  tab while the plugin is running.

### 2.7 Limits, timeouts, flood protection

| Limit | Value | On violation |
|---|---|---|
| Line length (plugin -> host) | 64 KB | Rest of the line is skipped up to the next `\n`, logged, strike |
| `results` items per reply | `max_results` (<= 20) | Truncated silently |
| Messages per second (any type) | 200 sustained over 3 s | Kill, count as a crash |
| `log` lines | 20/s | Dropped, one summary warn line |
| Strikes (bad JSON / oversized lines) | 10 within 60 s | Kill, count as a crash |
| `ready` after spawn | 5 s | Kill, mark failed |
| Soft query timeout | `query_timeout_ms` (default 1500) | The row group simply never appears; if the reply arrives late and its `id` is still the newest, it is accepted and shown |
| Hard timeout | 10 s with an outstanding query/action and no line of any kind from the plugin | `TerminateJobObject` (kills the whole tree), count as a crash |
| Crash/restart | restart on the next query; backoff: 3 crashes within 60 s | Plugin set "failed" for this session; PLUGINS tab shows the last log line; the user can re-enable by toggling |
| Total pending output | the reader is the only consumer; it never queues more than the single newest `results` per plugin (older ones are dropped before posting) | n/a |

Timeouts are armed with **one-shot** `SetTimer`s (new `PLUGIN_TIMER`) only while the menu is visible and a request
is outstanding, then killed - the same discipline as `SEARCH_TIMER`/`OVERLAY_TIMER`. Nothing ticks while hidden
except the single idle-kill timer in section 3.

---------------------------------------------------------------------------------------------------

## 3. Process model and threading

Per running plugin:

- **Supervisor/writer thread** (named `plugin-<id>`, 128 KB stack): spawns the process (so `CreateProcess` latency
  and antivirus scans never block the UI), puts it in its own Job Object (`KILL_ON_JOB_CLOSE`, so plugins and their
  children die when cpmenu exits or crashes), sends `init`, then loops on a bounded `mpsc::sync_channel(16)` of
  outgoing lines and writes them to the child's stdin. The UI thread uses `try_send`; a full channel (plugin not
  reading) drops the message and trips the hard timeout. After `shutdown` it waits for the exit, then
  `TerminateJobObject` if needed. Exit code goes to the log.
- **Reader thread** (`plugin-<id>-rd`): reads stdout with a bounded line reader (`fill_buf` loop, 64 KB cap),
  parses with `serde_json` into `plugin_proto::FromPlugin`, applies the flood limits, and posts to the window:

```rust
pub const WM_PLUGIN: u32 = WM_APP + 4;          // +1 command, +2 capture, +3 files are taken
// wparam = (generation << 16) | slot, lparam = Box<Reply> as isize; the window owns it from then on
enum Reply { Ready, Results { id: u64, items: Vec<Item> }, Done { req: u64, .. }, Error { id: u64, message: String },
             Exited { code: Option<i32>, reason: ExitReason } }
```

Same pattern as `winsearch`: `Box::into_raw` -> `PostMessageW`, and if the post fails the reader reclaims the
box. Stale-reply handling is two-layer, reusing `winsearch`'s idea: a global `static LATEST_QUERY: AtomicU64`;
the reader drops a `Results` whose `id` != `LATEST_QUERY` before posting (no flood of dead messages), and the
window checks again on receipt (menu hidden, query changed, plugin generation changed -> drop). `generation`
increments on every (re)start so a late message from a dead process can never be applied to its successor.

The UI thread owns all plugin state in a `thread_local! PLUGINS: RefCell<Host>` next to `MENU`; the threads touch
only the channel sender, an `Arc<AtomicU64>`, the window handle and the posted boxes. As with Everything's
re-entrant replies, never hold a `RefCell` borrow across anything that can pump messages.

Lifecycle:

1. **Start on first use.** A keyword query or a global query to a plugin that is enabled and approved starts it
   if it is not running (state `Starting`; the query waits in the 1-slot queue).
2. **Kept alive while in use.** `last_used` is set on every query/action.
3. **Idle kill without polling.** In `hide()`: if any plugin is running, arm one one-shot
   `SetTimer(IDLE_TIMER, 60_000)`. In `show()`: `KillTimer(IDLE_TIMER)`. When it fires: send `shutdown` to every
   plugin with `now - last_used >= 60 s` (supervisor does the 1 s grace + terminate), and if others remain (used
   less than 60 s before hide), re-arm once for the remainder. When none run, nothing is armed. `show()` also
   sweeps expired plugins in case the timer was lost, so correctness never depends on the timer. Cost: one wake-up
   about 60 s after hiding, zero while no plugin is running. The wake-up is worth it: a Python plugin is 10-30 MB
   and a Node plugin more, outside cpmenu's budget but on the same machine.
4. **Exit of cpmenu**: the Job Object kills everything. `quit` also sends `shutdown` first.
5. **Disable/uninstall**: toggling `enabled` off in the tab shuts the plugin down immediately.

---------------------------------------------------------------------------------------------------

## 4. Integration into Menu, ranking, actions

### Menu

- `Tag::Plugin(u8)` - index into the host's plugin table (max 255 loaded). `Tag` stays `Copy`; the label comes
  from the plugin (`host.tag_label(idx)`), so the tag text is per plugin with one style key for the look:
  `tag.plugin` (and the usual text/border variants) in `style.rs`/`style.toml`, default = the theme's `quick`
  role color. Per-plugin color override: not in v1.
- `Item` currently has `name`, `tag`, `id`, `icon`. Add `extra: Option<Box<PluginRow>>` (subtitle, copy, data,
  actions, default, alt, plugin generation/slot, item key). `None` for every existing row, so non-plugin rows cost
  8 bytes more and no allocation. `Item.id` holds `"<slot>:<key>"` for plugin rows so existing id-based code
  (selection retention) keeps working.
- `Menu` gains `plugin_rows: Vec<Item>` set by `set_plugin_results(group, query_id, items)` exactly like
  `set_files(&query, hits)` (which already ignores a reply for another query). `visible()` composes:
  quick command -> pinned/apps -> **plugin rows** -> files. A keyword query (`gh foo`) behaves like `>`/`?` in
  `commands::detect`: the list is replaced by that plugin's rows only (no apps, no Everything query), header shows
  `> gh foo_`. `commands::detect` returns a new `Quick::Plugin { slot, text }` variant checked before the generic
  rules and after `>`/`?`.
- Keyword conflicts: two plugins with the same keyword -> the one with the lower id wins, the other is flagged
  "keyword in use" in the PLUGINS tab until the user changes it.

### Ranking and limits

- Keyword mode: that plugin's rows sorted by `score` desc (stable), cap `max_results`.
- Global mode: each global plugin is capped at `min(max_results, 5)` and all global plugin rows together at 10.
  Placement is fixed: after all app matches, before files. Among plugin rows: `score` desc, then plugin id.
  Global plugin rows appear only when the query has >= 2 chars and is not a quick command.
- Global queries are debounced: they go out on `SEARCH_TIMER` expiry (120 ms), or the plugin's `debounce_ms` if
  larger, together with the Everything request. App results are never delayed by plugins.
- Row arrival uses the existing `eased_list_change` so the list does not jump.

### Action list (Right arrow / right click)

`actions::for_item(tag, path)` returns `Vec<Act>` (a closed enum). For `Tag::Plugin` the window instead builds the
rows from `extra.actions` as `Item`s with `Tag::Action` and `id = "plugin:<slot>:<action id>"`, after appending a
host `COPY` row when `copy` is set and no action already copies it. Executing an action row goes through
`plugins::invoke(slot, item, action_id)`:

- `do` present -> run the effect locally (no process round-trip; works even if the plugin has died).
- no `do` -> send `action` (starting the plugin if needed, with `data` attached) and hide unless `stay`.

Enter on a plugin row -> `default`, Ctrl+Enter -> `alt`, mirroring Enter/Ctrl+Enter for quick commands.

---------------------------------------------------------------------------------------------------

## 5. Security and trust (plain terms)

- A plugin is a program that runs **as you, with all your rights**. It can read your files, use your logged-in
  accounts, access the network, read `config.toml`, and start other programs. cpmenu does **not** sandbox it. The
  Job Object is there to clean up processes, not to contain them.
- There is no download, no store, no auto-update. A plugin exists only if you copied its folder into
  `%APPDATA%\cpmenu\plugins\`. cpmenu never fetches code.
- **First run confirmation.** A discovered plugin is not allowed to start until `approved` in `config.toml` equals
  `"<version>|<run command line>"`. The first time a keyword query hits an unapproved plugin, the list shows one
  row: `PLUGIN "GitHub" v1.2.0 by Jane Doe wants to run python main.py - ENTER allow / ESC ignore`
  (uses the existing CONFIRM styling, focus defaults to ignore, key-repeat Enter ignored, as the power overlay
  does). Allow -> writes `approved`. Global plugins never prompt inside search results (that would be noise and
  an easy lure); they stay off until approved in the PLUGINS tab (NOT APPROVED -> toggle -> same confirm).
- Changing `version` or the `run` command invalidates approval. **Honest limit:** editing the plugin's script
  without changing either is not detected (no file hashing in v1). Treat the plugin folder like any program
  you installed.
- Plugin output is untrusted data: strings are length-clamped and rendered as text, `open` accepts only
  http/https/mailto or existing paths, `done`/`do` is honored only for an invocation the user just made,
  icon paths are only read, never fetched from the network.
- A plugin must not be handed secrets: `init.settings` contains only the plugin's own settings. Never sent:
  other plugins' settings, the rest of `config.toml`, the query history (there is none).
- The query text is sent to the plugin: typing a password into the menu while a global plugin is enabled leaks it
  to that plugin. This is stated in the PLUGINS tab next to the GLOBAL toggle.

---------------------------------------------------------------------------------------------------

## 6. Developer docs

### Writing a plugin

1. Make `%APPDATA%\cpmenu\plugins\<id>\` with `plugin.json` and your script/exe.
2. Read lines from stdin, write one JSON line per message to stdout, **flush every line**. Use UTF-8.
3. Answer `init` with `ready`, `query` with `results` (echoing `id`), `action` with `done`, exit on `shutdown`/EOF.
4. Never print anything else on stdout; use stderr for debugging.

### Debugging

- Logs: `%APPDATA%\cpmenu\plugin-logs\<id>.log` - plugin stderr, `log` messages, and host events (started, exit
  code, "killed: hard timeout", "bad JSON line", "oversized line"). Appended across starts; at 1 MB it moves to
  `<id>.log.1` (one old file is kept).
- By hand: `python main.py` in the plugin folder and paste JSON lines; the protocol is plain text.
- PLUGINS tab shows state (STOPPED / RUNNING / FAILED...), the error text of a broken manifest, and a row to open the log.
- `cpmenu.exe plugin check <folder>` validates `plugin.json` and does an `init`/`query ""` round-trip, printing
  violations of the limits above and exiting 1 on failure (see the phase 6-7 decisions at the top).
- Working copies of both examples below live in the repo under `plugins-examples\hello` and `plugins-examples\hellops`;
  copy a folder into `%APPDATA%\cpmenu\plugins\` to use it, or check it in place.

### Example 1 - Python

`plugins\hello\plugin.json`

```json
{
  "id": "hello", "name": "Hello", "version": "1.0.0", "author": "me",
  "description": "Greets you and demonstrates actions",
  "keyword": "hello", "tag": "HELLO",
  "run": { "interpreter": "python", "file": "main.py" }
}
```

`plugins\hello\main.py`

```python
import sys, json

def send(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()

def log(level, message):
    send({"type": "log", "level": level, "message": message})

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    kind = msg.get("type")

    if kind == "init":
        send({"type": "ready", "protocol": 1})

    elif kind == "query":
        who = msg["text"].strip() or "world"
        send({"type": "results", "id": msg["id"], "items": [{
            "key": "greet",
            "title": "Hello, " + who + "!",
            "subtitle": "Enter copies it, Right arrow for more",
            "score": 80,
            "copy": "Hello, " + who + "!",
            "data": {"who": who},
            "default": "copy",
            "actions": [
                {"id": "copy", "label": "Copy greeting", "do": {"copy": "Hello, " + who + "!"}},
                {"id": "docs", "label": "Open Python docs", "do": {"open": "https://docs.python.org/3/"}},
                {"id": "shout", "label": "Shout it", "stay": True},
            ],
        }]})

    elif kind == "action":
        if msg["id"] == "shout":
            who = msg.get("data", {}).get("who", "world")
            send({"type": "done", "req": msg["req"], "set_query": "hello " + who.upper() + "!",
                  "keep_open": True})
        else:
            send({"type": "done", "req": msg["req"]})

    elif kind == "shutdown":
        break
```

### Example 2 - PowerShell

`plugins\hellops\plugin.json`

```json
{
  "id": "hellops", "name": "Hello PS", "version": "1.0.0", "author": "me",
  "description": "PowerShell example",
  "keyword": "hps", "tag": "PS",
  "run": { "interpreter": "powershell", "file": "main.ps1" }
}
```

`plugins\hellops\main.ps1`

```powershell
# Windows PowerShell 5.1 compatible. Startup costs ~0.3-0.5 s; the host waits up to 5 s for "ready".
[Console]::InputEncoding  = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)   # a stray BOM is tolerated by the host

function Send($obj) {
    [Console]::Out.WriteLine((ConvertTo-Json -InputObject $obj -Compress -Depth 8))
    [Console]::Out.Flush()
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    try { $msg = $line | ConvertFrom-Json } catch { continue }

    switch ($msg.type) {
        'init'  { Send @{ type = 'ready'; protocol = 1 } }
        'query' {
            $who = if ($msg.text.Trim()) { $msg.text.Trim() } else { 'world' }
            $greeting = "Hello, $who!"
            Send @{
                type  = 'results'
                id    = $msg.id
                items = @(
                    @{
                        key      = 'greet'
                        title    = $greeting
                        subtitle = 'Enter copies it'
                        score    = 80
                        default  = 'copy'
                        actions  = @(
                            @{ id = 'copy'; label = 'Copy greeting'; do = @{ copy = $greeting } },
                            @{ id = 'time'; label = 'Copy current time'; stay = $false }
                        )
                    }
                )
            }
        }
        'action' {
            if ($msg.id -eq 'time') {
                Send @{ type = 'done'; req = $msg.req; do = @(@{ copy = (Get-Date -Format o) }) }
            } else {
                Send @{ type = 'done'; req = $msg.req }
            }
        }
        'shutdown' { exit 0 }
    }
}
```

(`-InputObject` and the hashtable-inside-hashtable form keep single-element arrays as arrays; piping a
one-element array to `ConvertTo-Json` would unwrap it.)

---------------------------------------------------------------------------------------------------

## 7. Implementation plan

Flat module layout like the rest of `src/` (no `src/plugin/` directory, the repo has none). Tests run in the
project's usual `cargo test`; after each phase run the full check from the project CLAUDE.md / `cargo test`.

**Phase 1 - manifest, discovery, config (pure, no processes)**
- Add `serde`, `serde_json` to `Cargo.toml`. New `plugins.rs`: `Manifest` (serde), validation (id, keyword
  charset, reserved `>`/`?`, clamps), `discover()` over `%APPDATA%\cpmenu\plugins`, mtime change check,
  `run` -> command line expansion (pure function returning exe + args).
- `config.rs`: parse `[behavior] plugins`, `[plugins.<id>]` (enabled, keyword, approved, settings) into
  `Config.plugins`; keep unknown keys; `set_plugin_value` using the existing `set_value`.
- Tests: valid/invalid manifests table, id vs folder mismatch, keyword conflicts, interpreter expansion exact argv,
  config round-trip keeps comments, unknown ids ignored.

**Phase 2 - protocol types and bounded reader (pure)**
- New `plugin_proto.rs`: `ToPlugin`/`FromPlugin` serde enums, item sanitizer (truncate strings, clamp score, cap
  counts, drop bad items), `do` effect parsing/validation (url scheme allowlist), `read_bounded_line(reader, cap)`.
- Tests: oversized line skipped to next `\n` and next line still read, BOM on first line, unknown type/field
  ignored, malformed item skipped but others kept, `do` with two keys rejected, every example JSON in section 2
  parses (copy them into the test).

**Phase 3 - process supervisor**
- New `plugin_proc.rs`: spawn with `CREATE_NO_WINDOW`, Job Object (`Win32_System_JobObjects`), supervisor+writer
  thread, reader thread, `WM_PLUGIN` posting with generation, flood/strike counters, hard-timeout kill hook,
  shutdown with 1 s grace.
- Test helper: `examples/fake_plugin.rs` (a configurable plugin: echo, never read stdin, flood stdout, hang, crash,
  oversized line) so `cargo test` builds it; tests drive it without a window by giving `plugin_proc` a
  `Sender`-style sink trait instead of an `HWND` (the window glue is the only HWND-aware part).
- Tests: round trip init/ready/query/results, stale id dropped at reader, flood killed, hang killed by hard
  timeout, process tree gone after `TerminateJobObject`, restart gets a new generation.

**Phase 4 - window and Menu integration (keyword + global search)**
- `menu.rs`: `Tag::Plugin(u8)`, `Item.extra`, `plugin_rows`, `set_plugin_results`, ordering in `visible()`.
  `commands.rs`: `Quick::Plugin`. `window.rs`: `WM_PLUGIN` handler (visible + id + generation checks, then
  `eased_list_change`), sending queries from the `SEARCH_TIMER` path, `PLUGIN_TIMER` (one-shot soft/hard timeout
  while visible), `IDLE_TIMER` armed in `hide()`, killed in `show()`. `render.rs`/`style.rs`: tag drawing with
  per-plugin label, `tag.plugin` style key, subtitle line, plugin icon loading.
- Tests: `Menu` ordering (apps < plugin < files), caps and score sort, keyword mode replaces list, stale
  `query_id` ignored, selection retained across a replacement `results`, `commands::detect` precedence.
- Manual: measure RAM/CPU with a plugin running, then hidden 70 s later.

**Phase 5 - actions, effects, trust**
- `actions.rs`: plugin action rows; `plugins::invoke`; effect executor reusing `commands::copy`,
  `run_shell`-style launch and the OPEN code; `done` validity window (req match, 10 s); `stay`/`keep_open`;
  header error notice. First-run confirmation row (reuses power-overlay confirm look/focus rules);
  `approved` written via config.
- Tests: `done` with unknown/expired `req` discarded; `open` rejects `file:`/`javascript:`; approval string
  invalidates on version or run change; `default`/`alt` resolution; action rows built from `actions`.

**Phase 6 - PLUGINS tab**
- `settings.rs`: `Tab::Plugins` (list rows: name, version, state, ENABLED toggle, KEYWORD edit via the existing hex-edit
  row editor, GLOBAL toggle, plugin settings rows, NOT APPROVED -> approve, OPEN LOG, OPEN PLUGINS FOLDER).
  `lang/en.toml`, `lang/pl.toml`: new ids (`settings.tab.plugins`, `plugin.*`). Enabling/disabling or changing a
  keyword rebuilds the routing table and shuts the plugin down.
- Tests: tab row model (like the existing `Tab::Colors.rows()` tests), keyword conflict flagged, setting edit
  writes `[plugins.<id>.settings]` and sends `settings` to a running plugin.

**Phase 7 - hardening and docs**
- `cpmenu.exe plugin check <folder>`, log rotation, measure exe size and RAM, `plugins-examples/hello` and
  `hellops` from section 6, update `plan.md` (module table + log). Stress run: 1 000 keystrokes into a slow plugin.

### The failure modes that matter

1. **UI thread stalls or re-enters.** Any blocking pipe write, spawn, or RefCell borrow held across
   `PostMessage`/`SendMessage` freezes the menu (Everything already taught this once). Mitigation: all plugin I/O
   on supervisor/reader threads, `try_send` only, 2-in-flight query coalescing. Detect: the phase 3 fake plugin that
   never reads stdin and one that floods stdout, with a test asserting the typing path stays under a few ms
   (timestamp around the send call), plus a debug-build check that `plugin_proc` I/O functions panic if called on
   the UI thread.
2. **Orphaned or never-dying plugin processes / idle drain.** A lost timer, a plugin ignoring `shutdown`, or a
   cpmenu crash leaves Python/Node processes eating RAM, breaking the "0% idle, 30-40 MB" promise machine-wide.
   Mitigation: per-plugin Job Object with `KILL_ON_JOB_CLOSE`, `TerminateJobObject` after the 1 s grace, idle sweep
   also in `show()`. Detect: scripted check - hide, wait 65 s, `Get-Process python` has no child of cpmenu; kill
   cpmenu with `taskkill /f` and confirm the plugin is gone; `Get-Process cpmenu` CPU time unchanged over 60 s
   while hidden with no plugin running.
3. **Wrong or hostile results applied.** Late replies from a previous query, from a killed process, or `done`
   messages nobody asked for would show stale rows or trigger `open`/`run`. Mitigation: global query id +
   generation number checked at the reader and again in the window handler, `req` validity window, scheme
   allowlist, first-run approval. Detect: unit tests that feed out-of-order / wrong-generation / unsolicited `done`
   messages into the handler and assert no row and no effect; the log records every discarded message with the reason.
