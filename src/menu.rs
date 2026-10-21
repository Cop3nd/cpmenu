use std::cell::Cell;

use crate::actions::{self, Act};
use crate::apps::{App, Icon};
use crate::commands::{self, Quick};
use crate::everything::Hit;
use crate::lineedit::LineEdit;
use crate::plugin_proto as proto;
use crate::search;
use crate::usage::{self, Usage};

/// Longest query a paste can make, in characters.
pub const QUERY_MAX: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tag {
    /// A plugin's row; the number is the plugin's slot (the label is in `Item::extra`).
    Plugin(u8),
    App,
    Pinned,
    File,
    Folder,
    Shell,
    Calc,
    Web,
    Open,
    /// A Control Panel task; only listed while something is typed.
    Setting,
    /// A row of the action list (→).
    Action,
}

pub struct Item {
    pub name: String,
    pub tag: Tag,
    /// What launching opens: `apps::App::id` for apps, the full path for files and folders,
    /// the command line, result, URL or path for quick commands.
    pub id: String,
    pub icon: Option<Icon>,
    /// Only plugin rows.
    pub extra: Option<Box<PluginRow>>,
}

/// What a plugin row (or the first-run prompt / error row standing in for plugin results) is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowKind {
    Result,
    /// Shown instead of results: nothing to run.
    Notice,
    /// The first-run confirmation: `true` allows the plugin, `false` ignores it.
    Approval(bool),
}

#[derive(Debug, Clone)]
pub struct PluginRow {
    pub slot: u16,
    pub generation: u16,
    pub plugin_id: String,
    /// The query id the plugin answered; handed back with an `action`.
    pub query_id: u64,
    /// Upper case label shown on the row.
    pub tag: String,
    pub kind: RowKind,
    /// An action with `stay` is waiting for the plugin's `done`.
    pub busy: bool,
    pub item: proto::Item,
}

impl PluginRow {
    /// Text of the host COPY action, if the plugin gave `copy` and no action already copies exactly that.
    pub fn host_copy(&self) -> Option<&str> {
        let copy = self.item.copy.as_deref()?;
        let covered = self.item.actions.iter().any(|a| a.effects.as_ref().is_some_and(|e| e.iter().any(|e| matches!(e, proto::Effect::Copy(c) if c == copy))));
        (!covered).then_some(copy)
    }
}

impl From<App> for Item {
    fn from(app: App) -> Self {
        let tag = if app.setting { Tag::Setting } else { Tag::App };
        Self { name: app.name, tag, id: app.id, icon: app.icon, extra: None }
    }
}

impl From<Hit> for Item {
    fn from(hit: Hit) -> Self {
        let tag = if hit.folder { Tag::Folder } else { Tag::File };
        Self { id: hit.full_path(), name: hit.name, tag, icon: None, extra: None }
    }
}

impl From<Quick> for Item {
    fn from(quick: Quick) -> Self {
        let (name, id, tag) = match quick {
            Quick::Shell(command) if command.is_empty() => ("cmd".into(), command, Tag::Shell),
            Quick::Shell(command) => (command.clone(), command, Tag::Shell),
            Quick::Calc(_, result) => (format!("= {result}"), result, Tag::Calc),
            Quick::Web(terms, url) => (terms, url, Tag::Web),
            Quick::Open(target) => (target.clone(), target, Tag::Open),
            // Never turned into a row: keyword mode shows the plugin's rows instead (`Menu::push_quick`).
            Quick::Plugin { text, .. } => (text, String::new(), Tag::Open),
        };
        Self { name, tag, id, icon: None, extra: None }
    }
}

/// File name of an app's exe without `.exe` (`…\cmd.exe` -> `cmd`); empty for other ids.
fn exe_stem(id: &str) -> &str {
    let file = id.rsplit(['\\', '/']).next().unwrap_or("");
    match file.len().checked_sub(4) {
        Some(end) if file.is_char_boundary(end) && file[end..].eq_ignore_ascii_case(".exe") => &file[..end],
        _ => "",
    }
}

/// Pinned apps first, in `pinned` order (matched by display name), the rest alphabetical
/// (the index's order, so an unpinned app goes straight back to its place).
pub fn arrange(mut apps: Vec<Item>, pinned: &[String]) -> Vec<Item> {
    for app in &mut apps {
        if app.tag == Tag::Pinned {
            app.tag = Tag::App;
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    let mut front = 0;
    for name in pinned {
        if let Some(offset) = apps[front..].iter().position(|a| a.tag == Tag::App && &a.name == name) {
            let mut app = apps.remove(front + offset);
            app.tag = Tag::Pinned;
            apps.insert(front, app);
            front += 1;
        }
    }
    apps
}

/// Fraction of the remaining distance covered per animation frame.
const SCROLL_EASE: f32 = 0.28;
const SCROLL_SNAP: f32 = 0.002;

/// The action list shown in place of the results after → on a row.
pub struct Actions {
    /// The row the actions are for, copied so file results arriving meanwhile can't change it.
    pub name: String,
    pub tag: Tag,
    pub id: String,
    /// The file behind the row, if any.
    pub path: Option<String>,
    pub acts: Vec<Act>,
    pub selected: usize,
    pub scroll: f32,
    /// For a plugin row: a copy of the row and, per entry of `acts` (all `Act::Plugin`), what it does.
    pub row: Option<Box<PluginRow>>,
    pub plugin: Vec<PluginAct>,
    /// Shown as a small menu beside the list (which stays) instead of replacing it.
    pub popout: bool,
}

impl Actions {
    /// The text of each entry, in order.
    pub fn labels(&self) -> Vec<String> {
        self.acts.iter().enumerate().map(|(i, act)| self.plugin.get(i).map_or_else(|| act.label(), |p| p.label.clone())).collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginActKind {
    /// An action id of the plugin's item.
    Action(String),
    /// The host's COPY row for the item's `copy` text.
    HostCopy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginAct {
    pub kind: PluginActKind,
    pub label: String,
}

/// RECENT rows shown under the pinned ones.
pub const RECENT_MAX: usize = 5;

/// Plugin rows all together never exceed this when several plugins answer a global query.
pub const GLOBAL_TOTAL: usize = 10;
/// A global plugin contributes at most this many rows.
pub const GLOBAL_EACH: usize = 5;

#[derive(Default)]
pub struct Menu {
    /// `(keyword, slot)` of the plugins that answer to a keyword; set by the window.
    plugin_keywords: Vec<(String, u16)>,
    /// Keyword mode: the plugin and the query text after its keyword.
    keyword: Option<(u16, String)>,
    /// Rows of plugins in `items`, right after the apps and the quick command row.
    plugin_len: usize,
    /// One-line message replacing the match count in the header (a plugin error).
    pub notice: Option<String>,
    /// Apps first (`..apps_len`), then the quick command row for the query (if any),
    /// then file results for the query.
    pub items: Vec<Item>,
    /// Apps and Control Panel tasks (`settings_start..`), the indexed part of `items`.
    apps_len: usize,
    settings_start: usize,
    /// `{}` in it is replaced by the terms of a `?` web search; unset = the browser's own engine.
    pub search_url: Option<String>,
    pub query: LineEdit,
    /// How far the header scrolled the query text to the left, in px. The renderer keeps the caret
    /// in view and stores it here (a `Cell`, as it draws from `&Menu`); the mouse hit test reads it.
    pub query_scroll: Cell<f32>,
    /// Where the query line starts, in px right of the margin: 0 unless the title is centred or
    /// right aligned. Stored by the renderer like `query_scroll`; the mouse hit test reads it.
    pub query_x: Cell<f32>,
    /// Indexes into `items` that match `query`, best first. `selected` and `scroll` refer to this.
    pub view: Vec<usize>,
    pub selected: usize,
    /// Animated position of the list, in rows; converges to `selected`.
    pub scroll: f32,
    /// Open action list; navigation and Enter go to it instead of the results.
    pub actions: Option<Actions>,
    /// What was launched before; ranks matches and fills RECENT.
    pub usage: Usage,
    /// `[behavior] recent`: show RECENT rows while nothing is typed.
    pub recent: bool,
    /// Indexes into `items` of the RECENT rows in `view` (empty query only).
    recent_rows: Vec<usize>,
}

impl Menu {
    /// `items` are apps followed by Control Panel tasks.
    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.apps_len = self.items.len();
        self.settings_start = self.items.iter().position(|i| i.tag == Tag::Setting).unwrap_or(self.apps_len);
        self.push_quick();
        self.refilter();
        self.clamp_selection();
        self.scroll = self.selected as f32;
    }

    /// Apps are arranged (pins first, then alphabetical); tasks follow them in the given order,
    /// so an app wins a tie in the filter.
    pub fn set_apps(&mut self, items: Vec<Item>, pinned: &[String]) {
        let (settings, apps): (Vec<Item>, Vec<Item>) = items.into_iter().partition(|i| i.tag == Tag::Setting);
        let mut items = arrange(apps, pinned);
        items.extend(settings);
        self.set_items(items);
    }

    /// Re-applies pins after they changed, keeping the selection on the same item.
    pub fn repin(&mut self, pinned: &[String]) {
        let selected_id = self.current().map(|i| self.items[i].id.clone());
        self.items.truncate(self.apps_len);
        let items = std::mem::take(&mut self.items);
        self.set_apps(items, pinned);
        if let Some(row) = self.view.iter().position(|&i| Some(&self.items[i].id) == selected_id.as_ref()) {
            self.selected = row;
            self.scroll = row as f32;
        }
    }

    /// Ignored unless `query` is still what the user has typed.
    pub fn set_files(&mut self, query: &str, hits: Vec<Hit>) {
        if query != self.query.as_str() {
            return;
        }
        self.items.truncate(self.fixed_len());
        self.items.extend(hits.into_iter().map(Item::from));
        self.refilter();
        self.clamp_selection();
    }

    pub fn reset(&mut self) {
        self.actions = None;
        self.query.clear();
        self.items.truncate(self.apps_len);
        self.plugin_len = 0;
        self.keyword = None;
        self.notice = None;
        self.refilter();
        self.selected = 0;
        self.scroll = 0.0;
    }

    /// Item index of the selected row.
    pub fn current(&self) -> Option<usize> {
        self.view.get(self.selected).copied()
    }

    /// Runs a text edit on the query; the search refreshes if it changed anything. While the
    /// action list hides the query, edits still apply at its end.
    pub fn edit(&mut self, change: impl FnOnce(&mut LineEdit) -> bool) -> bool {
        if self.actions.is_some() {
            self.query.move_to(usize::MAX, false);
        }
        let changed = change(&mut self.query);
        if changed {
            self.query_changed();
        }
        changed
    }

    pub fn type_char(&mut self, c: char) {
        self.edit(|q| q.insert(c.encode_utf8(&mut [0; 4])));
    }

    #[cfg(test)]
    pub fn backspace(&mut self) {
        self.edit(LineEdit::backspace);
    }

    #[cfg(test)]
    pub fn delete_word(&mut self) {
        self.edit(LineEdit::delete_word_back);
    }

    /// Types clipboard `text` over the selection (or at the caret), cleaned and capped so the query
    /// stays within `QUERY_MAX`. Returns false if nothing was added.
    pub fn paste(&mut self, text: &str) -> bool {
        let room = (QUERY_MAX + self.query.selection().map_or(0, |(from, to)| to - from)).saturating_sub(self.query.char_len());
        let text = commands::clean_paste(text, room);
        !text.is_empty() && self.edit(|q| q.insert(&text))
    }

    /// Returns false if there was nothing to clear.
    pub fn clear_query(&mut self) -> bool {
        if self.query.is_empty() {
            return false;
        }
        self.query.clear();
        self.query_changed();
        true
    }

    fn query_changed(&mut self) {
        self.actions = None;
        self.notice = None;
        self.push_quick();
        self.refilter();
        self.selected = 0;
        self.scroll = 0.0;
    }

    /// Replaces the query (a plugin's `set_query`), as if it had been typed.
    pub fn set_query(&mut self, query: &str) {
        self.query.set(query);
        self.query_changed();
    }

    /// Plugin keywords as `(keyword, slot)`; the current query is checked against them again.
    pub fn set_plugin_keywords(&mut self, keywords: Vec<(String, u16)>) {
        if keywords == self.plugin_keywords {
            return;
        }
        self.plugin_keywords = keywords;
        if !self.query.is_empty() {
            self.push_quick();
            self.refilter();
            self.clamp_selection();
        }
    }

    /// The plugin and the text after its keyword, while the query starts with a plugin keyword.
    pub fn keyword(&self) -> Option<(u16, &str)> {
        self.keyword.as_ref().map(|(slot, text)| (*slot, text.as_str()))
    }

    /// True for a plain query: no quick command and no keyword (what global plugins answer).
    pub fn is_plain(&self) -> bool {
        self.keyword.is_none() && self.quick().is_none()
    }

    /// Drops results for the previous query and adds the quick command row for this one.
    fn push_quick(&mut self) {
        self.items.truncate(self.apps_len);
        self.plugin_len = 0;
        self.keyword = None;
        match commands::detect(&self.query, self.search_url.as_deref(), &self.plugin_keywords) {
            Some(Quick::Plugin { slot, text }) => self.keyword = Some((slot, text)),
            Some(quick) => self.items.push(quick.into()),
            None => {}
        }
    }

    fn quick(&self) -> Option<&Item> {
        self.items.get(self.apps_len).filter(|i| matches!(i.tag, Tag::Shell | Tag::Calc | Tag::Web | Tag::Open))
    }

    /// Apps plus the quick command row: the part before the plugin rows.
    fn base_len(&self) -> usize {
        self.apps_len + usize::from(self.quick().is_some())
    }

    /// Everything that isn't a file result.
    fn fixed_len(&self) -> usize {
        self.base_len() + self.plugin_len
    }

    /// `>`, `?` and plugin keywords take over the list, so neither apps nor files are searched.
    pub fn wants_files(&self) -> bool {
        self.keyword.is_none() && !self.quick().is_some_and(|i| matches!(i.tag, Tag::Shell | Tag::Web))
    }

    fn refilter(&mut self) {
        let (base, fixed) = (self.base_len(), self.fixed_len());
        self.view = (self.apps_len..base).collect();
        self.recent_rows.clear();
        if self.keyword.is_some() {
            self.view.extend(base..fixed);
        } else if self.wants_files() && self.query.is_empty() {
            // Control Panel tasks only turn up when searching, not in the plain app list.
            self.items.truncate(fixed);
            let list = self.plain_list();
            self.view.extend(list);
            self.view.extend(base..fixed);
        } else if self.wants_files() {
            let q = self.query.as_str().to_lowercase();
            let now = usage::now();
            let bonus = |i: &Item| usage::kind(i.tag).map_or(0, |k| self.usage.bonus(k, &i.id, &q, now));
            let apps = self.items[..self.apps_len].iter().map(|i| (i.name.as_str(), exe_stem(&i.id), bonus(i)));
            let found = search::filter_with(&self.query, apps);
            self.view.extend(found);
            self.view.extend(base..self.items.len());
        }
    }

    /// The list for an empty query: pinned apps, then RECENT (if on), then the other apps.
    fn plain_list(&mut self) -> Vec<usize> {
        let listed = self.settings_start;
        let mut recent: Vec<usize> = Vec::new();
        if self.recent {
            for (kind, id) in self.usage.recent() {
                if recent.len() == RECENT_MAX {
                    break;
                }
                match kind {
                    usage::APP | usage::SETTING => {
                        let found = self.items[..self.apps_len]
                            .iter()
                            .position(|i| i.id == id && usage::kind(i.tag) == Some(kind) && i.tag != Tag::Pinned);
                        recent.extend(found);
                    }
                    usage::FILE | usage::FOLDER if std::path::Path::new(id).exists() => {
                        let tag = if kind == usage::FOLDER { Tag::Folder } else { Tag::File };
                        self.items.push(Item { name: actions::file_name(id).to_owned(), tag, id: id.to_owned(), icon: None, extra: None });
                        recent.push(self.items.len() - 1);
                    }
                    _ => {}
                }
            }
        }
        let mut list: Vec<usize> = (0..listed).filter(|&i| self.items[i].tag == Tag::Pinned).collect();
        list.extend(&recent);
        list.extend((0..listed).filter(|&i| self.items[i].tag != Tag::Pinned && !recent.contains(&i)));
        self.recent_rows = recent;
        list
    }

    pub fn set_recent(&mut self, on: bool) {
        if self.recent != on {
            self.recent = on;
            self.refilter();
            self.clamp_selection();
        }
    }

    /// Forgets the launch history (CLEAR HISTORY).
    pub fn clear_usage(&mut self) {
        self.usage.clear();
        self.refilter();
        self.clamp_selection();
    }

    /// Whether item `index` is shown as a RECENT row.
    pub fn is_recent(&self, index: usize) -> bool {
        self.recent_rows.contains(&index)
    }

    /// Sets one plugin's rows for `query` (ignored if the user has typed on). Rows are ordered by score,
    /// a plugin keeps at most `each` of them and all plugins together at most `total`, ties by plugin id.
    /// The selection stays on the same row if it is still there.
    pub fn set_plugin_rows(&mut self, query: &str, slot: u16, mut rows: Vec<Item>, each: usize, total: usize) {
        if query != self.query.as_str() {
            return;
        }
        let score = |i: &Item| i.extra.as_ref().map_or(0, |e| e.item.score);
        let kept_id = self.current().map(|i| self.items[i].id.clone());
        rows.sort_by_key(|i| std::cmp::Reverse(score(i)));
        rows.truncate(each);
        let base = self.base_len();
        let files = self.items.split_off(base + self.plugin_len);
        let mut plugin = self.items.split_off(base);
        plugin.retain(|i| i.extra.as_ref().is_none_or(|e| e.slot != slot));
        plugin.extend(rows);
        let key = |i: &Item| i.extra.as_ref().map(|e| (std::cmp::Reverse(e.item.score), e.plugin_id.clone()));
        plugin.sort_by_key(key);
        plugin.truncate(total);
        self.plugin_len = plugin.len();
        self.items.extend(plugin);
        self.items.extend(files);
        self.refilter();
        match kept_id.and_then(|id| self.view.iter().position(|&i| self.items[i].id == id)) {
            Some(row) => self.selected = row,
            None => self.clamp_selection(),
        }
    }

    /// Marks (or unmarks) the plugin row `id` as waiting for its plugin.
    pub fn set_busy(&mut self, id: &str, busy: bool) {
        for item in self.items.iter_mut().filter(|i| i.id == id) {
            if let Some(extra) = item.extra.as_mut() {
                extra.busy = busy;
            }
        }
    }

    /// Clears every busy mark (the plugin died or restarted).
    pub fn clear_busy(&mut self) {
        for extra in self.items.iter_mut().filter_map(|i| i.extra.as_mut()) {
            extra.busy = false;
        }
    }

    /// The plugin row at `index`, if it is one.
    pub fn plugin_row(&self, index: usize) -> Option<&PluginRow> {
        self.items.get(index)?.extra.as_deref()
    }

    /// Opens the action list for the selected row; false if it has no actions.
    /// `path` is the file behind the row, for the actions that need one.
    pub fn open_actions(&mut self, path: Option<String>) -> bool {
        let Some(item) = self.current().map(|i| &self.items[i]) else { return false };
        if let Some(row) = item.extra.as_deref().filter(|r| r.kind == RowKind::Result) {
            return self.open_plugin_actions(row.clone());
        }
        let acts = actions::for_item(item.tag, path.as_deref());
        if acts.is_empty() {
            return false;
        }
        let (name, tag, id) = (item.name.clone(), item.tag, item.id.clone());
        self.actions = Some(Actions { name, tag, id, path, acts, selected: 0, scroll: 0.0, row: None, plugin: Vec::new(), popout: false });
        true
    }

    /// The plugin's own actions in its order, then the host COPY row.
    fn open_plugin_actions(&mut self, row: PluginRow) -> bool {
        let mut plugin: Vec<PluginAct> =
            row.item.actions.iter().map(|a| PluginAct { kind: PluginActKind::Action(a.id.clone()), label: a.label.clone() }).collect();
        if row.host_copy().is_some() {
            plugin.push(PluginAct { kind: PluginActKind::HostCopy, label: crate::lang::t("act.copy") });
        }
        if plugin.is_empty() {
            return false;
        }
        let name = row.item.title.clone();
        let id = self.current().map(|i| self.items[i].id.clone()).unwrap_or_default();
        let tag = Tag::Plugin(row.slot as u8);
        let acts = vec![Act::Plugin; plugin.len()];
        self.actions = Some(Actions { name, tag, id, path: None, acts, selected: 0, scroll: 0.0, row: Some(Box::new(row)), plugin, popout: false });
        true
    }

    /// Returns false if no action list was open.
    pub fn close_actions(&mut self) -> bool {
        self.actions.take().is_some()
    }

    /// The action list while it replaces the results.
    pub fn list_actions(&self) -> Option<&Actions> {
        self.actions.as_ref().filter(|a| !a.popout)
    }

    /// The action list while it is a pop-out beside the results.
    pub fn popout(&self) -> Option<&Actions> {
        self.actions.as_ref().filter(|a| a.popout)
    }

    /// Makes the open action list a pop-out.
    pub fn set_popout(&mut self) {
        if let Some(a) = &mut self.actions {
            a.popout = true;
        }
    }

    /// Closes the action list if it is a pop-out; returns whether one was open.
    pub fn close_popout(&mut self) -> bool {
        self.popout().is_some() && self.close_actions()
    }

    /// The selected row or action and the animated scroll of whichever list is shown.
    fn active(&mut self) -> (&mut usize, &mut f32, usize) {
        match &mut self.actions {
            Some(a) => (&mut a.selected, &mut a.scroll, a.acts.len()),
            None => (&mut self.selected, &mut self.scroll, self.view.len()),
        }
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.view.len().saturating_sub(1));
    }

    pub fn move_by(&mut self, delta: isize) {
        let (selected, _, len) = self.active();
        let last = len.saturating_sub(1) as isize;
        *selected = (*selected as isize + delta).clamp(0, last) as usize;
    }

    /// Selects `row` of the list or action list on screen (a right click); false if there is no such row.
    pub fn select_row(&mut self, row: usize) -> bool {
        let (selected, _, len) = self.active();
        let exists = row < len;
        if exists {
            *selected = row;
        }
        exists
    }

    pub fn select_first(&mut self) {
        *self.active().0 = 0;
    }

    pub fn select_last(&mut self) {
        let (selected, _, len) = self.active();
        *selected = len.saturating_sub(1);
    }

    /// Advances the scroll animation by one frame; returns true while still moving.
    pub fn step(&mut self) -> bool {
        // A pop-out leaves the list on screen, so the list's own scroll follows its selection too.
        let list = self.popout().is_some() && ease(self.selected, &mut self.scroll);
        let (selected, scroll, _) = self.active();
        ease(*selected, scroll) | list
    }
}

/// One frame of `scroll` easing toward row `selected`; true while still moving.
fn ease(selected: usize, scroll: &mut f32) -> bool {
    let target = selected as f32;
    let remaining = target - *scroll;
    if remaining.abs() < SCROLL_SNAP {
        *scroll = target;
        false
    } else {
        *scroll += remaining * SCROLL_EASE;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Hit, Item, Menu, PluginActKind, PluginRow, QUERY_MAX, RECENT_MAX, RowKind, Tag, Usage, arrange, proto, usage};

    #[test]
    fn popout_keeps_the_list_scrolling_to_the_right_clicked_row() {
        let mut menu = menu(10);
        assert!(menu.open_actions(None));
        menu.set_popout();
        // Right click on row 5 while a pop-out is open: it closes, then the new row opens its own.
        assert!(menu.close_popout() && menu.select_row(5) && menu.open_actions(None));
        menu.set_popout();
        let mut frames = 0;
        while menu.step() {
            frames += 1;
            assert!(frames < 500);
        }
        assert_eq!((menu.selected, menu.scroll), (5, 5.0), "the highlighted row is the one the pop-out belongs to");
        assert_eq!(menu.popout().unwrap().id, "id5");
    }

    fn menu(count: usize) -> Menu {
        let items = (0..count)
            .map(|i| Item { name: format!("App {i}"), tag: Tag::App, id: format!("id{i}"), icon: None, extra: None })
            .collect();
        named(items)
    }

    fn named(items: Vec<Item>) -> Menu {
        let mut menu = Menu::default();
        menu.set_items(items);
        menu
    }

    #[test]
    fn move_by_clamps_to_list() {
        let mut menu = menu(10);
        menu.move_by(-3);
        assert_eq!(menu.selected, 0);
        menu.move_by(1000);
        assert_eq!(menu.selected, 9);
    }

    #[test]
    fn scroll_converges_to_selection() {
        let mut menu = menu(10);
        menu.move_by(5);
        let mut frames = 0;
        while menu.step() {
            frames += 1;
            assert!(frames < 60, "scroll animation never settles");
        }
        assert_eq!(menu.scroll, 5.0);
    }

    #[test]
    fn shrinking_list_keeps_selection_in_range() {
        let mut menu = menu(10);
        menu.select_last();
        menu.set_items(Vec::new());
        assert_eq!(menu.selected, 0);
        assert!(menu.current().is_none());
    }

    #[test]
    fn paste_appends_like_typing_and_stops_at_the_limit() {
        let mut menu = named(vec![Item { name: "Discord".into(), tag: Tag::App, id: "d".into(), icon: None, extra: None }]);
        menu.type_char('d');
        assert!(menu.paste("isc\tord\nnext line"));
        assert_eq!(menu.query, "disc ord", "up to the first line break, tab as a space");
        assert!(!menu.paste("\n"), "nothing usable on the clipboard");
        assert_eq!(menu.query, "disc ord");
        menu.paste(&"x".repeat(1000));
        assert_eq!(menu.query.chars().count(), QUERY_MAX);
        assert!(!menu.paste("y"), "the query is full");
    }

    #[test]
    fn edits_apply_at_the_caret_and_replace_the_selection() {
        let mut menu = named(apps(&["Alpha", "Beta"]));
        menu.set_query("zażółć");
        menu.query.move_to(2, false);
        menu.type_char('x');
        assert_eq!((menu.query.as_str(), menu.query.caret()), ("zaxżółć", 3));
        menu.query.select_all();
        assert!(menu.paste("be"));
        assert_eq!((menu.query.as_str(), menu.query.selection()), ("be", None), "the selection is replaced");
        menu.query.select_all();
        menu.paste(&"x".repeat(1000));
        assert_eq!(menu.query.char_len(), QUERY_MAX, "room counts the replaced selection");
        menu.set_query("alp");
        assert!(menu.open_actions(None));
        menu.query.move_to(0, false);
        menu.backspace();
        assert!(menu.actions.is_none());
        assert_eq!(menu.query, "al", "edits under the action list hit the end");
    }

    #[test]
    fn typing_filters_and_selects_best_match() {
        let names = ["Discord", "Firefox", "Windows Terminal"];
        let mut menu = named(
            names.iter().map(|n| Item { name: n.to_string(), tag: Tag::App, id: n.to_string(), icon: None, extra: None }).collect(),
        );
        menu.move_by(2);
        for c in "term".chars() {
            menu.type_char(c);
        }
        assert_eq!(menu.view, vec![2]);
        assert_eq!(menu.current(), Some(2));
        menu.backspace();
        assert_eq!(menu.query, "ter");
        assert!(menu.clear_query());
        assert!(!menu.clear_query());
        assert_eq!(menu.view, vec![0, 1, 2]);
    }

    #[test]
    fn file_hits_follow_apps_and_stale_ones_are_dropped() {
        let mut menu = menu(3);
        menu.type_char('a');
        let hit = |n: &str| Hit { name: n.into(), path: r"C:\x".into(), folder: false };
        menu.set_files("old query", vec![hit("stale.txt")]);
        assert_eq!(menu.items.len(), 3);
        menu.set_files("a", vec![hit("a.txt")]);
        assert_eq!(menu.view, vec![0, 1, 2, 3]);
        assert_eq!(menu.items[3].tag, Tag::File);
        menu.type_char('b');
        assert_eq!(menu.items.len(), 3, "typing drops results for the previous query");
    }

    #[test]
    fn select_row_picks_a_row_that_exists() {
        let mut menu = named(apps(&["Alpha", "Beta", "Gamma"]));
        assert!(menu.select_row(2) && menu.selected == 2);
        assert!(!menu.select_row(3) && menu.selected == 2, "out of range is ignored");
        assert!(menu.open_actions(None));
        assert!(menu.select_row(1));
        assert_eq!((menu.actions.as_ref().unwrap().selected, menu.selected), (1, 2), "the shown list is the one that moves");
    }

    #[test]
    fn action_list_takes_over_navigation_and_closes_on_typing() {
        let mut menu = named(apps(&["Alpha", "Beta"]));
        menu.move_by(1);
        assert!(menu.open_actions(Some(r"C:/b.exe".into())));
        let a = menu.actions.as_ref().unwrap();
        assert_eq!((a.name.as_str(), a.acts.len()), ("Beta", 7));
        menu.move_by(2);
        menu.select_last();
        assert_eq!(menu.actions.as_ref().unwrap().selected, 6);
        assert_eq!(menu.selected, 1, "list selection is kept underneath");
        while menu.step() {}
        assert_eq!(menu.actions.as_ref().unwrap().scroll, 6.0);
        assert!(menu.close_actions());
        assert!(!menu.close_actions());
        menu.open_actions(None);
        menu.type_char('a');
        assert!(menu.actions.is_none());
    }

    #[test]
    fn a_popout_leaves_the_list_in_place_and_closes_on_its_own() {
        let mut menu = menu(3);
        assert!(menu.open_actions(None));
        assert!(menu.list_actions().is_some() && menu.popout().is_none());
        assert!(!menu.close_popout(), "the list-mode action list is not a pop-out");
        menu.set_popout();
        assert!(menu.list_actions().is_none() && menu.popout().is_some());
        menu.move_by(1);
        assert_eq!((menu.popout().unwrap().selected, menu.selected), (1, 0), "arrows move in the pop-out, the row stays");
        assert!(menu.close_popout() && menu.actions.is_none());
        assert!(menu.open_actions(None));
        menu.set_popout();
        menu.type_char('x');
        assert!(menu.actions.is_none(), "typing closes it");
    }

    #[test]
    fn quick_rows_have_no_actions() {
        let mut menu = named(apps(&["Alpha"]));
        for c in "1+1".chars() {
            menu.type_char(c);
        }
        assert!(!menu.open_actions(None));
    }

    #[test]
    fn delete_word_removes_one_word_at_a_time() {
        let mut menu = menu(1);
        for c in "> ipconfig /all  ".chars() {
            menu.type_char(c);
        }
        let mut steps = Vec::new();
        while !menu.query.is_empty() {
            menu.delete_word();
            steps.push(menu.query.as_str().to_owned());
        }
        assert_eq!(steps, vec!["> ipconfig /", "> ipconfig ", "> ", ""]);
        menu.delete_word();
        assert_eq!(menu.query, "");
        for c in "zażółć gęślą".chars() {
            menu.type_char(c);
        }
        menu.delete_word();
        assert_eq!(menu.query, "zażółć ");
    }

    #[test]
    fn exe_stem_comes_from_path_ids_only() {
        assert_eq!(super::exe_stem(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\cmd.exe"), "cmd");
        assert_eq!(super::exe_stem("C:/x/Code.EXE"), "Code");
        assert_eq!(super::exe_stem("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), "");
        assert_eq!(super::exe_stem(r"C:\Users\a\Desktop\Riot Client.lnk"), "");
    }

    #[test]
    fn quick_command_row_comes_first() {
        let mut menu = named(apps(&["Calculator", "Firefox"]));
        for c in "2+2".chars() {
            menu.type_char(c);
        }
        assert_eq!(menu.items[menu.current().unwrap()].tag, Tag::Calc);
        assert_eq!(menu.items[menu.current().unwrap()].id, "4");
        let hit = Hit { name: "2+2.txt".into(), path: r"C:\x".into(), folder: false };
        menu.set_files("2+2", vec![hit]);
        assert_eq!(menu.view, vec![2, 3], "quick row survives file results");
        menu.clear_query();
        assert_eq!(menu.items.len(), 2);
    }

    #[test]
    fn shell_prefix_hides_apps_and_files() {
        let mut menu = named(apps(&["Discord"]));
        for c in ">dir".chars() {
            menu.type_char(c);
        }
        assert!(!menu.wants_files());
        assert_eq!(menu.view, vec![1]);
        assert_eq!(menu.items[1].tag, Tag::Shell);
        assert_eq!(menu.items[1].id, "dir");
        menu.repin(&["Discord".to_string()]);
        assert_eq!(menu.view, vec![1], "re-pinning keeps the quick row");
    }

    fn setting(name: &str) -> Item {
        Item { name: name.into(), tag: Tag::Setting, id: format!("{{{name}}}"), icon: None, extra: None }
    }

    #[test]
    fn settings_show_only_while_searching_and_rank_below_apps() {
        let mut items = apps(&["Zmienne aplikacji", "Notepad"]);
        items.extend([setting("Edytuj zmienne środowiskowe systemu"), setting("Zmień zmienne")]);
        let mut menu = Menu::default();
        menu.set_apps(items, &[]);
        assert_eq!(menu.view.len(), 2, "plain list has apps only");
        for c in "zmienne".chars() {
            menu.type_char(c);
        }
        let found: Vec<_> = menu.view.iter().map(|&i| (menu.items[i].name.as_str(), menu.items[i].tag)).collect();
        assert_eq!(found[0], ("Zmienne aplikacji", Tag::App), "prefix match on an app wins");
        assert!(found.iter().any(|f| f.1 == Tag::Setting));
        assert_eq!(found.len(), 3);
        menu.clear_query();
        assert_eq!(menu.view.len(), 2);
    }

    #[test]
    fn settings_survive_repinning() {
        let mut items = apps(&["B", "A"]);
        items.push(setting("Siec"));
        let mut menu = Menu::default();
        menu.set_apps(items, &[]);
        menu.repin(&["B".to_string()]);
        assert_eq!(names(&menu.items), vec!["B", "A", "Siec"]);
        assert_eq!(menu.items[2].tag, Tag::Setting);
        menu.type_char('s');
        assert!(menu.view.iter().any(|&i| menu.items[i].tag == Tag::Setting));
    }

    fn proto_item(key: &str, score: u8) -> proto::Item {
        proto::Item {
            key: key.into(),
            title: key.to_uppercase(),
            subtitle: String::new(),
            tag: None,
            icon: None,
            score,
            copy: None,
            data: None,
            default: None,
            alt: None,
            actions: Vec::new(),
        }
    }

    fn plugin_rows(plugin: &str, slot: u16, rows: &[(&str, u8)]) -> Vec<Item> {
        rows.iter()
            .map(|(key, score)| Item {
                name: key.to_uppercase(),
                tag: Tag::Plugin(slot as u8),
                id: format!("{slot}:{key}"),
                icon: None,
                extra: Some(Box::new(PluginRow {
                    slot,
                    generation: 1,
                    plugin_id: plugin.into(),
                    query_id: 1,
                    tag: "TAG".into(),
                    kind: RowKind::Result,
                    busy: false,
                    item: proto_item(key, *score),
                })),
            })
            .collect()
    }

    fn typed(menu: &mut Menu, text: &str) {
        for c in text.chars() {
            menu.type_char(c);
        }
    }

    fn view_names(menu: &Menu) -> Vec<String> {
        menu.view.iter().map(|&i| menu.items[i].name.clone()).collect()
    }

    #[test]
    fn global_plugin_rows_sit_between_apps_and_files() {
        let mut menu = named(apps(&["Alpha", "Alps"]));
        typed(&mut menu, "al");
        menu.set_plugin_rows("al", 0, plugin_rows("p", 0, &[("low", 10), ("high", 90)]), 5, 10);
        let hit = Hit { name: "al.txt".into(), path: r"C:\x".into(), folder: false };
        menu.set_files("al", vec![hit]);
        assert_eq!(view_names(&menu)[2..], ["HIGH", "LOW", "al.txt"], "apps < plugin rows (by score) < files");
        menu.set_plugin_rows("al", 0, plugin_rows("p", 0, &[("only", 50)]), 5, 10);
        assert_eq!(view_names(&menu)[2..], ["ONLY", "al.txt"], "a new reply replaces that plugin's rows, files stay");
        menu.set_files("al", vec![]);
        assert_eq!(view_names(&menu)[2..], ["ONLY"], "new files keep the plugin rows");
    }

    #[test]
    fn global_caps_and_ties_by_plugin_id() {
        let mut menu = named(apps(&["Alpha"]));
        typed(&mut menu, "al");
        let many: Vec<(String, u8)> = (0..8).map(|i| (format!("a{i}"), 50)).collect();
        let many: Vec<(&str, u8)> = many.iter().map(|(k, s)| (k.as_str(), *s)).collect();
        menu.set_plugin_rows("al", 1, plugin_rows("zeta", 1, &many), 5, 10);
        assert_eq!(menu.view.len(), 1 + 5, "one plugin contributes at most 5");
        menu.set_plugin_rows("al", 2, plugin_rows("alpha", 2, &many), 5, 10);
        assert_eq!(menu.view.len(), 1 + 10, "all plugins together at most 10");
        let ids: Vec<_> = menu.view[1..].iter().map(|&i| menu.items[i].extra.as_ref().unwrap().plugin_id.clone()).collect();
        assert_eq!(&ids[..5], ["alpha"; 5], "equal scores: lower plugin id first");
    }

    #[test]
    fn plugin_rows_for_an_older_query_are_ignored() {
        let mut menu = named(apps(&["Alpha"]));
        typed(&mut menu, "al");
        menu.set_plugin_rows("a", 0, plugin_rows("p", 0, &[("x", 50)]), 5, 10);
        assert_eq!(menu.items.len(), 1);
        menu.set_plugin_rows("al", 0, plugin_rows("p", 0, &[("x", 50)]), 5, 10);
        assert_eq!(menu.items.len(), 2);
        menu.type_char('p');
        assert_eq!(menu.items.len(), 1, "typing drops the plugin rows of the previous query");
    }

    #[test]
    fn keyword_mode_replaces_the_list_with_that_plugins_rows() {
        let mut menu = named(apps(&["Github Desktop", "Notepad"]));
        menu.set_plugin_keywords(vec![("gh".into(), 4)]);
        typed(&mut menu, "gh foo");
        assert_eq!(menu.keyword(), Some((4, "foo")));
        assert!(!menu.wants_files() && menu.view.is_empty() && !menu.is_plain(), "no apps, no files, nothing until the plugin answers");
        menu.set_plugin_rows("gh foo", 4, plugin_rows("github", 4, &[("a", 10), ("b", 20), ("c", 30)]), 2, 2);
        assert_eq!(view_names(&menu), ["C", "B"], "sorted by score and capped at max_results");
        menu.backspace();
        menu.backspace();
        menu.backspace();
        menu.backspace();
        assert_eq!(menu.keyword(), None, "'gh' without the space is an ordinary search again");
        assert!(menu.wants_files());
    }

    #[test]
    fn selection_stays_on_the_same_row_when_a_reply_replaces_the_rows() {
        let mut menu = named(apps(&["Alpha"]));
        menu.set_plugin_keywords(vec![("gh".into(), 4)]);
        typed(&mut menu, "gh x");
        menu.set_plugin_rows("gh x", 4, plugin_rows("p", 4, &[("a", 90), ("b", 80), ("c", 70)]), 8, 8);
        menu.move_by(2);
        assert_eq!(menu.items[menu.current().unwrap()].id, "4:c");
        menu.set_plugin_rows("gh x", 4, plugin_rows("p", 4, &[("n", 99), ("a", 90), ("b", 80), ("c", 70)]), 8, 8);
        assert_eq!(menu.items[menu.current().unwrap()].id, "4:c", "a row inserted above doesn't move the selection off c");
        menu.set_plugin_rows("gh x", 4, plugin_rows("p", 4, &[("z", 99)]), 8, 8);
        assert_eq!(menu.current().map(|i| menu.items[i].id.as_str()), Some("4:z"), "selection falls back into range");
    }

    #[test]
    fn plugin_action_list_has_its_actions_then_a_copy_row() {
        let mut menu = named(apps(&["Alpha"]));
        menu.set_plugin_keywords(vec![("gh".into(), 4)]);
        typed(&mut menu, "gh x");
        let mut rows = plugin_rows("p", 4, &[("a", 50)]);
        {
            let extra = rows[0].extra.as_mut().unwrap();
            extra.item.copy = Some("https://x".into());
            extra.item.actions = vec![
                proto::Action { id: "open".into(), label: "Open".into(), effects: Some(vec![proto::Effect::Open("https://x".into())]), stay: false },
                proto::Action { id: "star".into(), label: "Star".into(), effects: None, stay: true },
            ];
        }
        menu.set_plugin_rows("gh x", 4, rows, 8, 8);
        assert!(menu.open_actions(None));
        let actions = menu.actions.as_ref().unwrap();
        let labels: Vec<_> = actions.plugin.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Open", "Star", "COPY"]);
        assert_eq!(actions.plugin[2].kind, PluginActKind::HostCopy);
        assert_eq!(actions.acts.len(), 3);
        // An action that already copies the same text makes the host row redundant.
        menu.close_actions();
        let at = menu.current().unwrap();
        let extra = menu.items[at].extra.as_mut().unwrap();
        extra.item.actions[0].effects = Some(vec![proto::Effect::Copy("https://x".into())]);
        assert!(menu.open_actions(None));
        assert_eq!(menu.actions.as_ref().unwrap().plugin.len(), 2);
        // A row with neither actions nor copy text has no list; error rows have none either.
        menu.close_actions();
        let at = menu.current().unwrap();
        let extra = menu.items[at].extra.as_mut().unwrap();
        extra.item.actions.clear();
        extra.item.copy = None;
        assert!(!menu.open_actions(None));
    }

    fn recent_menu(names: &[&str], pinned: &[&str]) -> Menu {
        let mut menu = Menu { recent: true, ..Menu::default() };
        let pinned: Vec<String> = pinned.iter().map(|p| p.to_string()).collect();
        menu.set_apps(apps(names), &pinned);
        menu
    }

    #[test]
    fn usage_lifts_a_match_within_its_kind_and_a_remembered_pick_more() {
        let mut menu = named(apps(&["Edge", "Edit", "Notepad Editor"]));
        menu.usage.record(usage::APP, "Edit", "", usage::now());
        typed(&mut menu, "ed");
        assert_eq!(view_names(&menu), ["Edit", "Edge", "Notepad Editor"], "the used prefix match first, the word-start match stays last");
        let mut menu = named(apps(&["Edge", "Edit", "Notepad Editor"]));
        for _ in 0..500 {
            menu.usage.record(usage::APP, "Notepad Editor", "ed", usage::now());
        }
        typed(&mut menu, "ed");
        assert_eq!(view_names(&menu), ["Edge", "Edit", "Notepad Editor"], "heavy use plus an exact pick still loses to a better kind of match");
        let mut menu = named(apps(&["Calendar", "Chrome", "Calculator"]));
        menu.usage.record(usage::APP, "Chrome", "c", usage::now());
        typed(&mut menu, "c");
        assert_eq!(menu.items[menu.current().unwrap()].name, "Chrome", "c -> Chrome is remembered");
    }

    #[test]
    fn recent_rows_follow_the_pinned_ones_and_skip_pinned_and_missing() {
        let mut menu = recent_menu(&["Alpha", "Beta", "Gamma", "Delta"], &["Delta"]);
        for (i, id) in ["Gamma", "Delta", "Ghost", "Beta"].iter().enumerate() {
            menu.usage.record(usage::APP, id, "", 100 + i as u64);
        }
        menu.reset();
        assert_eq!(view_names(&menu), ["Delta", "Beta", "Gamma", "Alpha"], "pinned, then newest first, then the rest");
        let recent: Vec<_> = menu.view.iter().map(|&i| menu.is_recent(i)).collect();
        assert_eq!(recent, [false, true, true, false], "a pinned app and an app that is gone are not RECENT");
        menu.type_char('a');
        assert!(menu.view.iter().all(|&i| !menu.is_recent(i)), "only while nothing is typed");
        menu.clear_query();
        assert!(menu.is_recent(menu.view[1]));
    }

    #[test]
    fn recent_is_capped_off_when_switched_off_and_cleared_with_the_history() {
        let names: Vec<String> = (0..8).map(|i| format!("App {i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut menu = recent_menu(&names, &[]);
        for (i, n) in names.iter().enumerate() {
            menu.usage.record(usage::APP, n, "", 100 + i as u64);
        }
        menu.reset();
        assert_eq!(menu.view.iter().filter(|&&i| menu.is_recent(i)).count(), RECENT_MAX);
        assert_eq!(view_names(&menu)[..2], ["App 7", "App 6"]);
        assert_eq!(menu.view.len(), 8, "nothing listed twice");
        menu.set_recent(false);
        assert_eq!(view_names(&menu), names, "off: the plain alphabetical list");
        menu.set_recent(true);
        menu.usage = Usage::default();
        menu.reset();
        assert_eq!(view_names(&menu), names);
    }

    #[test]
    fn recent_files_and_folders_must_still_exist() {
        let dir = std::env::temp_dir().join(format!("cpmenu-recent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "x").unwrap();
        let (file, folder, gone) = (file.to_str().unwrap().to_owned(), dir.to_str().unwrap().to_owned(), dir.join("gone.txt").to_str().unwrap().to_owned());
        let mut menu = recent_menu(&["Alpha"], &[]);
        menu.usage.record(usage::FILE, &file, "", 10);
        menu.usage.record(usage::FOLDER, &folder, "", 11);
        menu.usage.record(usage::FILE, &gone, "", 12);
        menu.reset();
        let shown: Vec<_> = menu.view.iter().map(|&i| (menu.items[i].name.as_str(), menu.items[i].tag)).collect();
        assert_eq!(shown[..2], [(dir.file_name().unwrap().to_str().unwrap(), Tag::Folder), ("notes.txt", Tag::File)]);
        assert_eq!(shown[2], ("Alpha", Tag::App));
        assert_eq!(shown.len(), 3);
        menu.type_char('a');
        assert_eq!(menu.items.len(), 1, "recent files leave the item list when typing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn apps(names: &[&str]) -> Vec<Item> {
        names.iter().map(|n| Item { name: n.to_string(), tag: Tag::App, id: n.to_string(), icon: None, extra: None }).collect()
    }

    fn names(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.name.as_str()).collect()
    }

    #[test]
    fn pinned_apps_come_first_in_config_order() {
        let pinned = ["Zed".to_string(), "Discord".to_string(), "Missing".to_string()];
        let arranged = arrange(apps(&["Discord", "Firefox", "Zed"]), &pinned);
        assert_eq!(names(&arranged), vec!["Zed", "Discord", "Firefox"]);
        assert_eq!(arranged[0].tag, Tag::Pinned);
        assert_eq!(arranged[2].tag, Tag::App);
    }

    #[test]
    fn repin_keeps_selection_on_the_toggled_item() {
        let mut menu = Menu::default();
        menu.set_apps(apps(&["A", "B", "C"]), &[]);
        menu.move_by(2);
        menu.repin(&["C".to_string()]);
        assert_eq!(names(&menu.items), vec!["C", "A", "B"]);
        assert_eq!(menu.current().map(|i| menu.items[i].name.as_str()), Some("C"));
        menu.repin(&[]);
        assert_eq!(names(&menu.items), vec!["A", "B", "C"], "unpinned app returns to its alphabetical place");
        assert_eq!(menu.items[0].tag, Tag::App);
    }
}
