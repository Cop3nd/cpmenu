use super::*;


/// Design size of a row in pixels at scale 1; fonts, icons and row internals scale from it.
pub(super) const BASE_ROW_H: f32 = 54.0;
pub(super) const BASE_STEP: f32 = 64.0;
pub(super) const BASE_MARGIN: f32 = 16.0;
/// Header backdrop ends at 60; the rest is a gap so the first row's glow never touches the title.
pub(super) const BASE_HEADER: f32 = 74.0;
pub(super) const BASE_FOOTER: f32 = 52.0;
/// What stays below the list when the power bar is hidden.
pub(super) const BASE_FOOTER_HIDDEN: f32 = 8.0;
/// How much shorter the header is when it has no caption, line, progress strip or gear.
pub(super) const BASE_HEADER_TRIM: f32 = 24.0;
pub(super) const MIN_WIDTH: f32 = 420.0;
/// Gap between the window and the work-area edge.
pub(super) const BASE_OUTER_MARGIN: f32 = 8.0;
pub(super) const ROW_FILL: f32 = 0.84;
pub(super) const SHIFT_SPREAD: f32 = 1.6;
/// Rows fade out over this many list steps at the top and bottom edges.
pub(super) const EDGE_FADE_STEPS: f32 = 1.7;

/// Fraction of the remaining list slide covered per animation frame (about 170 ms to settle).
pub(super) const SLIDE_EASE: f32 = 0.4;
pub(super) const SLIDE_SNAP: f32 = 0.5;

/// Room kept right of the caret so its bar is not cut by the edge of the text area.
pub(super) const CARET_PAD: f32 = 2.0;

/// How far (px) the query text is scrolled left, as an editor does: `prev` stays unless the caret
/// (`caret` px from the text's start) would leave the `area` px wide window, then it moves just
/// far enough; text that fits is not scrolled. `text` is the full width of the text.
pub fn query_scroll(prev: f32, caret: f32, text: f32, area: f32) -> f32 {
    let most = (text + CARET_PAD - area).max(0.0);
    let mut at = prev.clamp(0.0, most);
    if caret < at {
        at = caret;
    } else if caret + CARET_PAD > at + area {
        at = caret + CARET_PAD - area;
    }
    at.clamp(0.0, most)
}

/// Top-left of a pop-out `size` px big, against `row` (the list row), `list` (the whole list) or the
/// pointer `cursor` (a row when there is none): beside the target on the side away from the screen edge
/// the menu is docked to, top aligned. Then moved by `offset` percent of its own size and kept inside `bounds`.
pub fn place_popout(
    anchor: ActionsAnchor,
    side: Side,
    row: D2D_RECT_F,
    list: D2D_RECT_F,
    cursor: Option<(f32, f32)>,
    size: (f32, f32),
    gap: f32,
    offset: (i32, i32),
    bounds: (f32, f32),
) -> (f32, f32) {
    let beside = |target: D2D_RECT_F| {
        let x = if side == Side::Left { target.right + gap } else { target.left - gap - size.0 };
        (x, target.top)
    };
    let (x, y) = match (anchor, cursor) {
        (ActionsAnchor::Cursor, Some(at)) => at,
        (ActionsAnchor::List, _) => beside(list),
        _ => beside(row),
    };
    let (x, y) = (x + offset.0 as f32 / 100.0 * size.0, y + offset.1 as f32 / 100.0 * size.1);
    (x.min(bounds.0 - size.0).max(0.0), y.min(bounds.1 - size.1).max(0.0))
}

/// All pixel geometry of the window, derived from its size, the monitor's DPI scale and the config.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    /// DPI scale (1.0 = 96 dpi): header, power bar and margins.
    pub scale: f32,
    /// `scale` times the text size setting: fonts and the metrics that hold text (header, power
    /// bar, settings panel, dialog) follow it.
    pub ts: f32,
    /// Faded text is drawn at 90% or more.
    pub(super) contrast: bool,
    /// Rows draw their icon box.
    pub(super) icons: bool,
    /// What is drawn (style.toml's `[show]`).
    pub(super) show: Show,
    /// Pixels the header lost because nothing is drawn in its top strip (see `BASE_HEADER_TRIM`).
    pub(super) head_trim: f32,
    /// Row scale relative to the 54 px design row: row internals, icons and row fonts.
    pub(super) k: f32,
    pub(super) w: f32,
    pub(super) h: f32,
    pub(super) margin: f32,
    pub(super) list_top: f32,
    pub(super) list_bottom: f32,
    pub(super) footer_top: f32,
    pub(super) button_top: f32,
    pub(super) button_h: f32,
    pub(super) button_gap: f32,
    pub(super) step: f32,
    pub(super) row_h: f32,
    /// The selected row always sits in the vertical middle of the list area.
    pub(super) band_y: f32,
    pub(super) centre_short: bool,
    pub(super) edge_fade: f32,
    pub(super) arc: f32,
    pub(super) rows: f32,
    pub(super) backdrop: f32,
    /// Pixels the list still lags behind its settled position after the number of rows or the
    /// layout changed; eased to 0 by `step_slide`.
    pub(super) slide: f32,
    /// Where the menu starts inside the window, and the settings panel's column (width 0 = closed).
    pub(super) menu_x: f32,
    pub(super) panel_x: f32,
    pub(super) panel_w: f32,
    /// The panel card shows the first-run wizard (its own buttons, no tabs).
    pub(super) wizard: bool,
    /// Width of the extra column the actions pop-out needs beside the menu (0 = none open).
    pub(super) pop_w: f32,
}

/// The font dropdown's rectangles in window coordinates (see `Layout::dropdown_parts`).
#[derive(Default)]
pub(super) struct DropParts {
    pub(super) outer: D2D_RECT_F,
    pub(super) title: D2D_RECT_F,
    pub(super) size: D2D_RECT_F,
    pub(super) weight: D2D_RECT_F,
    pub(super) filter: D2D_RECT_F,
    pub(super) items_top: f32,
}

/// What the actions pop-out is placed from (see `Layout::popout_rect`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopoutParams {
    pub items: usize,
    /// The highlighted item.
    pub selected: usize,
    /// Pixel width, fitted to the longest label (`popout_width`).
    pub width: f32,
    /// The list row the actions belong to, the list's eased position and its row count.
    pub row: usize,
    pub scroll: f32,
    pub rows: usize,
    pub anchor: ActionsAnchor,
    pub side: Side,
    /// Where the pointer was in menu coordinates, when opened by a right click.
    pub cursor: Option<(f32, f32)>,
    /// Percent of the pop-out's own width / height.
    pub offset: (i32, i32),
}

/// What a click on the list area landed on; the pop-out is above the rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ListHit {
    Popout(usize),
    Row(usize),
    Nothing,
}

/// Where a click in the settings panel landed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelHit {
    Tab(usize),
    /// Arrow of row `.0`; `.1` is -1 (left) or 1 (right).
    Arrow(usize, i32),
    Row(usize),
    OpenConfig,
    OpenStyle,
    /// The X in the card's corner (the wizard: skip).
    Close,
    /// WSTECZ / POMIŃ / DALEJ of the wizard.
    Back,
    Skip,
    Next,
}

/// Where the pointer is over an open font dropdown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DropHit {
    /// The SIZE row: -1 / 1 its arrows, 0 the rest of it.
    Size(i32),
    Weight(i32),
    Filter,
    /// The entry this many places down the window (there may be none there in a short list).
    Item(usize),
    /// On the dropdown, on none of the above.
    Inside,
    /// Not on the dropdown but on the row it hangs from.
    Anchor,
}

/// Element under the mouse pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hover {
    Power(usize),
    /// The gear in the header.
    Gear,
    Panel(PanelHit),
    /// Part of the open font dropdown.
    Drop(DropHit),
    /// A row of the right-click popup: 0 = PASTE, 1 = SETTINGS.
    Popup(usize),
    /// Power overlay dialog buttons.
    Execute,
    Abort,
}

pub(super) const PANEL_W: f32 = 340.0;
pub(super) const PANEL_PAD: f32 = 14.0;
pub(super) const PANEL_ROW_H: f32 = 40.0;
pub(super) const PANEL_ROW_PITCH: f32 = 46.0;
pub(super) const PANEL_ROWS_TOP: f32 = 84.0;
/// Room for the tallest tab that doesn't scroll.
pub(super) const PANEL_ROW_SLOTS: f32 = SLOTS as f32;
pub(super) const PANEL_CARD_H: f32 = 476.0;
/// Window height per unit of GUI scale: the card plus its top and bottom gap.
pub(super) const PANEL_FIT_H: f32 = PANEL_CARD_H + 12.0;
pub(super) const PANEL_CONTROL_W: f32 = 150.0;
pub(super) const PANEL_ARROW_W: f32 = 26.0;
pub(super) const POPOUT_ITEM_H: f32 = 26.0;
pub(super) const POPOUT_PAD: f32 = 4.0;
/// Room the item text leaves beside its label: 10 left, 24 right (the enter hint).
pub(super) const POPOUT_TEXT_PAD: f32 = 10.0 + 24.0 + 8.0;
pub(super) const POPOUT_MIN_W: f32 = 150.0;
pub(super) const POPOUT_MAX_W: f32 = 320.0;
pub(super) const POPOUT_GAP: f32 = 6.0;
/// The font dropdown: padding, a SIZE / WEIGHT row, the filter field, the gap above the list and one entry.
pub(super) const DROP_PAD: f32 = 6.0;
/// The title line with the label of the row it belongs to.
pub(super) const DROP_TITLE_H: f32 = 22.0;
pub(super) const DROP_ROW_H: f32 = 28.0;
pub(super) const DROP_ROW_GAP: f32 = 2.0;
pub(super) const DROP_FILTER_H: f32 = 28.0;
pub(super) const DROP_GAP: f32 = 6.0;
pub(super) const DROP_ITEM_H: f32 = 26.0;
/// Size of the font each entry is set in, per unit of text scale.
pub(super) const DROP_PREVIEW_SIZE: f32 = 13.0;
/// Text formats kept for the entries' previews.
pub(super) const PREVIEW_CAP: usize = 64;
pub(super) const POPUP_W: f32 = 150.0;
pub(super) const POPUP_H: f32 = 30.0;
/// Rows of the right-click popup: PASTE, then SETTINGS.
pub const POPUP_ENTRIES: usize = 2;

impl Default for Layout {
    fn default() -> Self {
        Self::new(480, 640, 1.0, &LayoutConfig::default())
    }
}

impl Layout {
    /// DPI scale times the GUI scale setting.
    pub fn text_scale(scale: f32, cfg: &LayoutConfig) -> f32 {
        scale * cfg.gui_scale.clamp(GUI_SCALE_MIN, GUI_SCALE_MAX) as f32 / 100.0
    }

    /// `text_scale` as far as a window `height` px tall can hold: the settings card is
    /// `PANEL_CARD_H` tall at scale 1 and must fit, so a big GUI scale on a small screen is capped.
    pub fn fitted_scale(scale: f32, cfg: &LayoutConfig, height: i32) -> f32 {
        Self::text_scale(scale, cfg).min(height.max(1) as f32 / PANEL_FIT_H)
    }

    pub fn new(width: i32, height: i32, scale: f32, cfg: &LayoutConfig) -> Self {
        let (w, h) = (width as f32, height as f32);
        let ts = Self::fitted_scale(scale, cfg, height);
        let margin = BASE_MARGIN * scale;
        let show = cfg.show;
        let head_trim = if show.caption || show.header_line || show.progress || show.gear { 0.0 } else { BASE_HEADER_TRIM * ts };
        let list_top = BASE_HEADER * ts - head_trim;
        let footer_top = h - if show.power_bar { BASE_FOOTER } else { BASE_FOOTER_HIDDEN } * ts;
        let list_bottom = footer_top - 6.0 * scale;
        let (height_px, gap_px) = (cfg.shape.row_height, cfg.shape.row_gap);
        let unit = height_px + gap_px;
        let rows = match cfg.visible_rows {
            // Automatic: rows about as tall as the design row (54 px unless [shape] says otherwise, times the text size), whatever the screen.
            0 => ((list_bottom - list_top) / (unit * ts)).round().clamp(3.0, 30.0),
            n => n as f32,
        };
        let step = (list_bottom - list_top) / rows;
        // The default shape keeps the original 0.84; a taller gap or row only changes the ratio.
        let fill = (ROW_FILL * ((height_px / unit) / (BASE_ROW_H / BASE_STEP))).min(1.0);
        let row_h = step * fill;
        Self {
            width,
            height,
            scale,
            ts,
            contrast: cfg.high_contrast,
            icons: cfg.icons,
            show,
            head_trim,
            k: row_h / BASE_ROW_H,
            w,
            h,
            margin,
            list_top,
            list_bottom,
            footer_top,
            button_top: footer_top + 12.0 * ts,
            button_h: 30.0 * ts,
            button_gap: 6.0 * scale,
            step,
            row_h,
            band_y: list_top + (list_bottom - list_top - row_h) * cfg.band_percent.min(100) as f32 / 100.0,
            centre_short: cfg.short_list == ShortList::Center,
            edge_fade: step * EDGE_FADE_STEPS,
            arc: cfg.arc_shift as f32 * scale,
            rows,
            backdrop: cfg.backdrop_opacity,
            slide: 0.0,
            menu_x: 0.0,
            panel_x: 0.0,
            panel_w: 0.0,
            wizard: false,
            pop_w: 0.0,
        }
    }

    /// Room for the settings panel next to the menu, beside the screen edge's far side.
    /// `ts` is the text scale (`text_scale`); the panel grows with it.
    pub fn panel_width(work_w: i32, menu_w: i32, scale: f32, ts: f32) -> i32 {
        let room = work_w - 2 * Self::outer_margin(scale) - menu_w;
        ((PANEL_W * ts).round() as i32).min(room).max(0)
    }

    /// The same layout with a panel column of `panel_w` px added, left of the menu or right of it.
    pub fn with_panel(mut self, panel_w: i32, on_left: bool) -> Self {
        self.width = self.w as i32 + panel_w;
        self.panel_w = panel_w as f32;
        (self.menu_x, self.panel_x) = if on_left { (self.panel_w, 0.0) } else { (0.0, self.w) };
        self
    }

    /// The same layout with the panel card showing the wizard (`on`) or the settings.
    pub fn with_wizard(mut self, on: bool) -> Self {
        self.wizard = on;
        self
    }

    pub fn has_panel(&self) -> bool {
        self.panel_w > 0.0
    }

    /// Width of the column a pop-out that sticks out `needed` px past the menu needs, as far as
    /// the work area (`work_w`) has room beside a menu `menu_w` wide.
    pub fn popout_column(needed: f32, work_w: i32, menu_w: i32, scale: f32) -> i32 {
        let room = (work_w - 2 * Self::outer_margin(scale) - menu_w).max(0);
        (needed.ceil() as i32).min(room)
    }

    /// How far the pop-out for `p` sticks out of the menu's box on the side away from the screen
    /// edge, at its widest (the row at its most displaced), plus a margin; 0 if it stays within.
    /// Call it on a layout without a pop-out column.
    pub fn popout_overhang(&self, p: &PopoutParams) -> f32 {
        // Placing far from 0, so a pop-out left of the menu isn't clamped at the window edge.
        const FAR: f32 = 10_000.0;
        let (shown, _) = self.popout_window(p.items, p.selected);
        let size = (p.width, 2.0 * POPOUT_PAD * self.ts + shown as f32 * POPOUT_ITEM_H * self.ts);
        let row = rect(FAR + self.margin, 0.0, FAR + self.w - self.margin, self.row_h);
        let list = rect(FAR + self.margin, self.list_top, FAR + self.w - self.margin, self.list_bottom);
        let cursor = p.cursor.map(|(x, y)| (FAR + x, y));
        let (x, _) = place_popout(p.anchor, p.side, row, list, cursor, size, POPOUT_GAP * self.scale, p.offset, (3.0 * FAR, 3.0 * FAR));
        let (left, right) = (x - FAR, x - FAR + size.0);
        let over = if p.side == Side::Left { right - self.w } else { -left };
        if over > 0.0 { over + self.margin } else { 0.0 }
    }

    /// The same layout with a pop-out column of `col` px added, left of the menu or right of it.
    /// The settings panel and the pop-out never share the window.
    pub fn with_popout(mut self, col: i32, on_left: bool) -> Self {
        if self.has_panel() || col <= 0 {
            return self;
        }
        self.width = self.w as i32 + col;
        self.pop_w = col as f32;
        if on_left {
            self.menu_x = self.pop_w;
        }
        self
    }

    /// Width of the pop-out column this layout was made with.
    pub fn popout_col(&self) -> i32 {
        self.pop_w as i32
    }

    /// The pop-out's pixel width for the widest label `text_w` px wide.
    pub fn popout_width(text_w: f32, ts: f32) -> f32 {
        (text_w + POPOUT_TEXT_PAD * ts).clamp(POPOUT_MIN_W * ts, POPOUT_MAX_W * ts)
    }

    /// Items that fit the pop-out's height, and the first one shown while the selection is `selected`.
    pub(super) fn popout_window(&self, items: usize, selected: usize) -> (usize, usize) {
        let fit = (((self.h - 2.0 * POPOUT_PAD * self.ts) / (POPOUT_ITEM_H * self.ts)) as usize).max(1);
        let shown = items.min(fit);
        (shown, (selected + 1).saturating_sub(shown))
    }

    /// The pop-out's rectangle in window coordinates: placed against the row, the list or the
    /// pointer, moved by the offsets and kept inside the window.
    pub fn popout_rect(&self, p: &PopoutParams) -> D2D_RECT_F {
        let (shown, _) = self.popout_window(p.items, p.selected);
        let size = (p.width.min(self.width as f32), 2.0 * POPOUT_PAD * self.ts + shown as f32 * POPOUT_ITEM_H * self.ts);
        let (pl, il) = self.row_x((p.row as f32 - p.scroll).abs());
        let top = self.row_top(p.row as f32, p.scroll, p.rows);
        let row = rect(self.menu_x + pl, top, self.menu_x + il + self.row_h, top + self.row_h);
        let list = rect(self.menu_x + self.margin, self.list_top, self.menu_x + self.w - self.margin, self.list_bottom);
        let cursor = p.cursor.map(|(x, y)| (self.menu_x + x, y));
        let (x, y) = place_popout(p.anchor, p.side, row, list, cursor, size, POPOUT_GAP * self.scale, p.offset, (self.width as f32, self.h));
        rect(x, y, x + size.0, y + size.1)
    }

    /// The pop-out item under a point, for a pop-out placed from `p`.
    pub fn popout_item_at(&self, p: &PopoutParams, x: f32, y: f32) -> Option<usize> {
        let r = self.popout_rect(p);
        if !contains(r, x, y) {
            return None;
        }
        let (shown, first) = self.popout_window(p.items, p.selected);
        let n = ((y - r.top - POPOUT_PAD * self.ts) / (POPOUT_ITEM_H * self.ts)).max(0.0) as usize;
        Some(first + n.min(shown.saturating_sub(1)))
    }

    /// Pop-out item or list row under a point; the pop-out wins where it covers rows.
    pub fn list_hit(&self, popout: Option<&PopoutParams>, x: f32, y: f32, scroll: f32, count: usize) -> ListHit {
        if let Some(item) = popout.and_then(|p| self.popout_item_at(p, x, y)) {
            return ListHit::Popout(item);
        }
        self.row_at(x, y, scroll, count).map_or(ListHit::Nothing, ListHit::Row)
    }

    /// Whether the footer with the power buttons is shown.
    pub fn has_power_bar(&self) -> bool {
        self.show.power_bar
    }

    /// Window x of the menu's left edge.
    pub fn menu_x(&self) -> f32 {
        self.menu_x
    }

    /// The header strip above the list (the right-click target), in window coordinates.
    pub fn in_header(&self, x: f32, y: f32) -> bool {
        (self.menu_x..self.menu_x + self.w).contains(&x) && (0.0..self.list_top).contains(&y)
    }

    /// The gear button on the header's title line, in menu coordinates.
    pub(super) fn gear_rect(&self) -> D2D_RECT_F {
        let s = self.ts;
        rect(self.w - self.margin - 20.0 * s, 2.0 * s, self.w - self.margin, 22.0 * s)
    }

    /// Whether a point (window coordinates) is on the gear.
    pub fn gear_hit(&self, x: f32, y: f32) -> bool {
        self.show.gear && contains(self.gear_rect(), x - self.menu_x, y)
    }

    /// Whether a point (window coordinates) is on the header's query line.
    pub fn query_hit(&self, x: f32, y: f32) -> bool {
        let (left, s) = (self.query_left(), self.ts);
        (left..self.menu_x + self.w - self.margin).contains(&x) && (32.0 * s - self.head_trim..58.0 * s - self.head_trim).contains(&y)
    }

    /// Window x where the query line's text starts.
    pub fn query_left(&self) -> f32 {
        self.menu_x + self.margin
    }

    /// The X in the settings card's top-right corner, in window coordinates.
    pub(super) fn close_rect(&self) -> D2D_RECT_F {
        let (card, s) = (self.card(), self.ts);
        rect(card.right - 28.0 * s, card.top + 10.0 * s, card.right - 8.0 * s, card.top + 30.0 * s)
    }

    pub(super) fn popup_rect(&self, (x, y): (f32, f32)) -> D2D_RECT_F {
        let (pw, ph) = (POPUP_W * self.ts, POPUP_H * self.ts * POPUP_ENTRIES as f32);
        let left = x.min(self.width as f32 - pw).max(0.0);
        let top = y.min(self.h - ph).max(0.0);
        rect(left, top, left + pw, top + ph)
    }

    /// The popup row under a point, for the popup opened at `anchor`.
    pub fn popup_entry(&self, anchor: (f32, f32), x: f32, y: f32) -> Option<usize> {
        let r = self.popup_rect(anchor);
        if !contains(r, x, y) {
            return None;
        }
        let row = ((y - r.top) / (POPUP_H * self.ts)) as usize;
        Some(row.min(POPUP_ENTRIES - 1))
    }

    pub(super) fn card(&self) -> D2D_RECT_F {
        let (inset, top) = (8.0 * self.ts, 6.0 * self.ts);
        let height = (PANEL_CARD_H * self.ts).min(self.h - 2.0 * top);
        rect(self.panel_x + inset, top, self.panel_x + self.panel_w - inset, top + height)
    }

    pub(super) fn card_inner(&self) -> (f32, f32) {
        let card = self.card();
        (card.left + PANEL_PAD * self.ts, card.right - PANEL_PAD * self.ts)
    }

    pub(super) fn tab_rect(&self, index: usize) -> D2D_RECT_F {
        let (card, (left, right), s) = (self.card(), self.card_inner(), self.ts);
        let gap = 4.0 * s;
        let width = (right - left - (TABS.len() - 1) as f32 * gap) / TABS.len() as f32;
        let x = left + index as f32 * (width + gap);
        rect(x, card.top + 44.0 * s, x + width, card.top + 70.0 * s)
    }

    pub(super) fn setting_rect(&self, index: usize) -> D2D_RECT_F {
        let (card, (left, right), s) = (self.card(), self.card_inner(), self.ts);
        let top = card.top + (PANEL_ROWS_TOP + index as f32 * PANEL_ROW_PITCH) * s;
        rect(left, top, right, top + PANEL_ROW_H * s)
    }

    /// Left (-1) or right (1) arrow of a settings row.
    pub(super) fn arrow_rect(&self, index: usize, dir: i32) -> D2D_RECT_F {
        let (row, s) = (self.setting_rect(index), self.ts);
        let x = if dir < 0 { row.right - PANEL_CONTROL_W * s } else { row.right - PANEL_ARROW_W * s };
        rect(x, row.top, x + PANEL_ARROW_W * s, row.bottom)
    }

    /// Where a row's value is centred: between the arrows, or, for a row without arrows, over the
    /// whole control so the value sits at the same centre.
    pub(super) fn value_rect(&self, index: usize, stepper: bool) -> D2D_RECT_F {
        let (row, (lo, hi)) = (self.setting_rect(index), (self.arrow_rect(index, -1), self.arrow_rect(index, 1)));
        if stepper { rect(lo.right, row.top, hi.left, row.bottom) } else { rect(lo.left, row.top, row.right, row.bottom) }
    }

    pub(super) fn note_rect(&self) -> D2D_RECT_F {
        let (card, (left, right), s) = (self.card(), self.card_inner(), self.ts);
        let top = card.top + (PANEL_ROWS_TOP + PANEL_ROW_SLOTS * PANEL_ROW_PITCH + 2.0) * s;
        rect(left, top, right, top + 18.0 * s)
    }

    /// OPEN CONFIG.TOML (left half) and OPEN STYLE.TOML (right half).
    pub(super) fn config_button_rect(&self) -> D2D_RECT_F {
        self.button_half(0)
    }

    pub(super) fn style_button_rect(&self) -> D2D_RECT_F {
        self.button_half(1)
    }

    pub(super) fn button_half(&self, index: usize) -> D2D_RECT_F {
        let (card, (left, right), s) = (self.card(), self.card_inner(), self.ts);
        let top = card.top + (PANEL_ROWS_TOP + PANEL_ROW_SLOTS * PANEL_ROW_PITCH + 26.0) * s;
        let half = (right - left - 6.0 * s) / 2.0;
        let x = left + index as f32 * (half + 6.0 * s);
        rect(x, top, x + half, top + 30.0 * s)
    }

    /// WSTECZ (0), POMIŃ (1) and DALEJ (2) of the wizard: thirds of the row the settings buttons use.
    pub(super) fn wizard_button(&self, index: usize) -> D2D_RECT_F {
        let (card, (left, right), s) = (self.card(), self.card_inner(), self.ts);
        let top = card.top + (PANEL_ROWS_TOP + PANEL_ROW_SLOTS * PANEL_ROW_PITCH + 26.0) * s;
        let gap = 6.0 * s;
        let third = (right - left - 2.0 * gap) / 3.0;
        let x = left + index as f32 * (third + gap);
        rect(x, top, x + third, top + 30.0 * s)
    }

    /// The parts of the font dropdown for `g`: under its row if the card has room, else pushed up
    /// to end inside the card. Rows whose rectangles are empty (no SIZE / WEIGHT) are for a role only.
    pub(super) fn dropdown_parts(&self, g: DropGeom) -> DropParts {
        let (anchor, card, s) = (self.setting_rect(g.slot), self.card(), self.ts);
        let pad = DROP_PAD * s;
        let rows = if g.role { 2.0 * (DROP_ROW_H + DROP_ROW_GAP) } else { 0.0 };
        let height = (2.0 * DROP_PAD + DROP_TITLE_H + rows + DROP_FILTER_H + DROP_GAP + VISIBLE as f32 * DROP_ITEM_H) * s;
        let top = (anchor.bottom + 3.0 * s).min(card.bottom - pad - height).max(card.top + pad);
        let (left, right) = (anchor.left + pad, anchor.right - pad);
        let mut y = top + pad;
        let mut next = |h: f32, gap: f32| {
            let r = rect(left, y, right, y + h * s);
            y += (h + gap) * s;
            r
        };
        let title = next(DROP_TITLE_H, 0.0);
        let (size, weight) = if g.role { (next(DROP_ROW_H, DROP_ROW_GAP), next(DROP_ROW_H, DROP_ROW_GAP)) } else { Default::default() };
        let filter = next(DROP_FILTER_H, DROP_GAP);
        DropParts { outer: rect(anchor.left, top, anchor.right, top + height), title, size, weight, filter, items_top: filter.bottom + DROP_GAP * s }
    }

    /// The window's rectangle for the entry `n` places down the dropdown's list.
    pub(super) fn dropdown_item(&self, parts: &DropParts, n: usize) -> D2D_RECT_F {
        let top = parts.items_top + n as f32 * DROP_ITEM_H * self.ts;
        rect(parts.filter.left, top, parts.filter.right, top + DROP_ITEM_H * self.ts)
    }

    /// Left and right arrow of a SIZE / WEIGHT row, placed like a settings row's.
    pub(super) fn drop_arrows(&self, r: D2D_RECT_F) -> (D2D_RECT_F, D2D_RECT_F) {
        let s = self.ts;
        let arrow = |x: f32| rect(x, r.top, x + PANEL_ARROW_W * s, r.bottom);
        (arrow(r.right - PANEL_CONTROL_W * s), arrow(r.right - PANEL_ARROW_W * s))
    }

    /// The parts of the toggle pop-out of `count` entries hanging from screen slot `slot`: placed
    /// like the font dropdown. `filter` is the strip the entries span, so `dropdown_item` serves both.
    pub(super) fn toggles_parts(&self, slot: usize, count: usize) -> DropParts {
        let (anchor, card, s) = (self.setting_rect(slot), self.card(), self.ts);
        let pad = DROP_PAD * s;
        let height = (2.0 * DROP_PAD + DROP_TITLE_H + count as f32 * DROP_ITEM_H) * s;
        let top = (anchor.bottom + 3.0 * s).min(card.bottom - pad - height).max(card.top + pad);
        let (left, right) = (anchor.left + pad, anchor.right - pad);
        let title = rect(left, top + pad, right, top + pad + DROP_TITLE_H * s);
        DropParts { outer: rect(anchor.left, top, anchor.right, top + height), title, filter: rect(left, title.bottom, right, title.bottom), items_top: title.bottom, ..Default::default() }
    }

    /// What a point is over, for the toggle pop-out; `None` outside both it and its row.
    pub fn toggles_hit(&self, slot: usize, count: usize, x: f32, y: f32) -> Option<DropHit> {
        let parts = self.toggles_parts(slot, count);
        if !contains(parts.outer, x, y) {
            return contains(self.setting_rect(slot), x, y).then_some(DropHit::Anchor);
        }
        let n = ((y - parts.items_top) / (DROP_ITEM_H * self.ts)).floor();
        if n >= 0.0 && (n as usize) < count && (parts.filter.left..=parts.filter.right).contains(&x) {
            return Some(DropHit::Item(n as usize));
        }
        Some(DropHit::Inside)
    }

    /// What a point is over, for the dropdown `g`; `None` outside both it and its row.
    pub fn dropdown_hit(&self, g: DropGeom, x: f32, y: f32) -> Option<DropHit> {
        let parts = self.dropdown_parts(g);
        if !contains(parts.outer, x, y) {
            return contains(self.setting_rect(g.slot), x, y).then_some(DropHit::Anchor);
        }
        if g.role {
            for (row, hit) in [(parts.size, DropHit::Size as fn(i32) -> DropHit), (parts.weight, DropHit::Weight)] {
                if contains(row, x, y) {
                    let (left, right) = self.drop_arrows(row);
                    let dir = if contains(left, x, y) { -1 } else if contains(right, x, y) { 1 } else { 0 };
                    return Some(hit(dir));
                }
            }
        }
        if contains(parts.filter, x, y) {
            return Some(DropHit::Filter);
        }
        let n = ((y - parts.items_top) / (DROP_ITEM_H * self.ts)).floor();
        if n >= 0.0 && (n as usize) < VISIBLE && (parts.filter.left..=parts.filter.right).contains(&x) {
            return Some(DropHit::Item(n as usize));
        }
        Some(DropHit::Inside)
    }

    /// What the mouse is over, for hover highlights: the popup first, then the panel, then the power bar.
    pub fn hover_at(&self, x: f32, y: f32, slots: &[bool], popup: Option<(f32, f32)>) -> Option<Hover> {
        if let Some(row) = popup.and_then(|anchor| self.popup_entry(anchor, x, y)) {
            return Some(Hover::Popup(row));
        }
        if let Some(hit) = self.panel_hit(x, y, slots) {
            return Some(Hover::Panel(hit));
        }
        if self.gear_hit(x, y) {
            return Some(Hover::Gear);
        }
        self.button_at(x, y).map(Hover::Power)
    }

    /// Panel element under a point in window coordinates. `slots` has one entry per visible
    /// line of the panel: true for a settings row, false for a section heading.
    pub fn panel_hit(&self, x: f32, y: f32, slots: &[bool]) -> Option<PanelHit> {
        if !self.has_panel() || !contains(self.card(), x, y) {
            return None;
        }
        if contains(self.close_rect(), x, y) {
            return Some(PanelHit::Close);
        }
        if self.wizard {
            if let Some(hit) = [PanelHit::Back, PanelHit::Skip, PanelHit::Next].into_iter().enumerate().find_map(|(i, hit)| contains(self.wizard_button(i), x, y).then_some(hit)) {
                return Some(hit);
            }
        } else {
            if let Some(tab) = (0..TABS.len()).find(|&i| contains(self.tab_rect(i), x, y)) {
                return Some(PanelHit::Tab(tab));
            }
            if contains(self.config_button_rect(), x, y) {
                return Some(PanelHit::OpenConfig);
            }
            if contains(self.style_button_rect(), x, y) {
                return Some(PanelHit::OpenStyle);
            }
        }
        slots.iter().enumerate().filter(|(_, row)| **row).find_map(|(i, _)| {
            if !contains(self.setting_rect(i), x, y) {
                return None;
            }
            let arrow = [-1, 1].into_iter().find(|&dir| contains(self.arrow_rect(i, dir), x, y));
            Some(arrow.map_or(PanelHit::Row(i), |dir| PanelHit::Arrow(i, dir)))
        })
    }

    /// Whether a point is on the panel card at all (clicks there never reach the menu).
    pub fn in_panel(&self, x: f32, y: f32) -> bool {
        self.has_panel() && contains(self.card(), x, y)
    }

    /// Window size on a work area of `work_w` x `work_h` px.
    pub fn window_size(work_w: i32, work_h: i32, scale: f32, cfg: &LayoutConfig) -> (i32, i32) {
        let margin = Self::outer_margin(scale);
        let widest = (work_w - 2 * margin).max(1);
        let height = (work_h - 2 * margin).max(1);
        let wanted = (work_w as f32 * cfg.width_percent as f32 / 100.0).round() as i32;
        // The content grows with the GUI scale (as far as the height lets it), so does the least room
        // it gets; a scale below 100% leaves the 100% minimum.
        let narrowest = (MIN_WIDTH * Self::fitted_scale(scale, cfg, height).max(scale)).round() as i32;
        (wanted.max(narrowest).min(widest), height)
    }

    pub fn outer_margin(scale: f32) -> i32 {
        (BASE_OUTER_MARGIN * scale).round() as i32
    }

    /// Longest open animation: last visible row's delay plus its slide-in, as far as the rows
    /// cascade, the header glitches and the power bar fades in.
    pub fn open_ms(&self) -> f32 {
        let cascade = if self.show.row_cascade { (self.rows + 3.0) * ROW_DELAY_MS + ROW_IN_MS } else { 0.0 };
        let glitch = if self.show.open_glitch { GLITCH_MS } else { 0.0 };
        let power = if self.show.power_bar { ROW_IN_MS } else { 0.0 };
        cascade.max(glitch).max(power)
    }

    /// Sideways offset of a row `distance` rows from the selection: a bell curve, so the list reads as an arc.
    pub(super) fn shift(&self, distance: f32) -> f32 {
        self.arc * (-(distance * distance) / (2.0 * SHIFT_SPREAD * SHIFT_SPREAD)).exp()
    }

    /// (panel left, icon left) of a row; the most displaced row ends flush with the margins.
    pub(super) fn row_x(&self, distance: f32) -> (f32, f32) {
        let x = self.shift(distance);
        let lo = self.arc.min(0.0);
        (self.margin + x - lo, self.w - self.margin - self.arc.abs() - self.row_h + x - lo)
    }

    /// Top of row 0 for the eased selection index `scroll`: the selected row sits in the band
    /// (`band_percent` of the list area), except near the ends, where the list stays flush with the
    /// top / bottom of the list area. A list that fits entirely starts at the top or is centred.
    /// Includes the `slide` still to ease out, so drawing and hit testing agree.
    pub fn offset(&self, scroll: f32, count: usize) -> f32 {
        self.settled_offset(scroll, count) + self.slide
    }

    pub(super) fn settled_offset(&self, scroll: f32, count: usize) -> f32 {
        let first = self.list_top;
        let last = self.list_bottom - self.row_h - count.saturating_sub(1) as f32 * self.step;
        if last >= first {
            return if self.centre_short { first + (last - first) / 2.0 } else { first };
        }
        (self.band_y - scroll * self.step).clamp(last, first)
    }

    /// The list went from `old_count` to `count` rows at selection `scroll`: it keeps drawing where
    /// it was and eases to the new settled position.
    /// Returns whether a slide is now running.
    pub fn retarget(&mut self, old_count: usize, scroll: f32, count: usize) -> bool {
        self.slide += self.settled_offset(scroll, old_count) - self.settled_offset(scroll, count);
        self.snap_slide()
    }

    /// This (new) layout replaces `old` while the list is shown: the list eases from where `old` drew it.
    /// Returns whether a slide is now running.
    pub fn carry_slide(&mut self, old: &Layout, scroll: f32, count: usize) -> bool {
        self.slide = old.offset(scroll, count) - self.settled_offset(scroll, count);
        self.snap_slide()
    }

    /// One animation frame of the slide; true while it is still moving.
    pub fn step_slide(&mut self) -> bool {
        self.slide *= 1.0 - SLIDE_EASE;
        self.snap_slide()
    }

    /// Drops a slide too small to see; true if one remains.
    pub(super) fn snap_slide(&mut self) -> bool {
        if self.slide.abs() < SLIDE_SNAP * self.scale {
            self.slide = 0.0;
        }
        self.slide != 0.0
    }

    pub(super) fn row_top(&self, row: f32, scroll: f32, count: usize) -> f32 {
        self.offset(scroll, count) + row * self.step
    }

    /// Opacity of a row from its position: fades out towards the top and bottom edges, but only
    /// where rows are actually cut off there (the list start / end stays fully visible).
    pub(super) fn position_alpha(&self, top: f32, scroll: f32, count: usize) -> f32 {
        if !self.show.edge_fade {
            return 1.0;
        }
        let offset = self.offset(scroll, count);
        let last_bottom = offset + count.saturating_sub(1) as f32 * self.step + self.row_h;
        let cut_top = ((self.list_top - offset) / self.step).clamp(0.0, 1.0);
        let cut_bottom = ((last_bottom - self.list_bottom) / self.step).clamp(0.0, 1.0);
        let from_top = ((top - self.list_top) / self.edge_fade).clamp(0.0, 1.0);
        let from_bottom = ((self.list_bottom - (top + self.row_h)) / self.edge_fade).clamp(0.0, 1.0);
        let top_alpha = 1.0 - cut_top * (1.0 - from_top);
        let bottom_alpha = 1.0 - cut_bottom * (1.0 - from_bottom);
        top_alpha.min(bottom_alpha)
    }

    /// Row under a point in window coordinates, once the open animation has settled.
    pub fn row_at(&self, x: f32, y: f32, scroll: f32, count: usize) -> Option<usize> {
        let x = x - self.menu_x;
        if !(self.margin..=self.w - self.margin).contains(&x) || !(self.list_top..self.list_bottom).contains(&y) {
            return None;
        }
        let index = ((y - self.offset(scroll, count)) / self.step).floor();
        let top = self.row_top(index, scroll, count);
        let inside = y - top <= self.row_h;
        let visible = self.position_alpha(top, scroll, count) > 0.3;
        (inside && visible && index >= 0.0 && (index as usize) < count).then_some(index as usize)
    }

    pub(super) fn button_rect(&self, index: usize) -> D2D_RECT_F {
        let count = ACTIONS.len() as f32;
        let width = (self.w - 2.0 * self.margin - self.button_gap * (count - 1.0)) / count;
        let left = self.margin + index as f32 * (width + self.button_gap);
        rect(left, self.button_top, left + width, self.button_top + self.button_h)
    }

    /// Power bar button under a point in window coordinates.
    pub fn button_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.show.power_bar {
            return None;
        }
        let x = x - self.menu_x;
        (0..ACTIONS.len()).find(|&i| {
            let r = self.button_rect(i);
            (r.left..=r.right).contains(&x) && (r.top..=r.bottom).contains(&y)
        })
    }
}

/// Share of the header bar filled for the eased selection index `scroll` in a list of `count`.
pub(super) fn progress(scroll: f32, count: usize) -> f32 {
    if count == 0 { 0.0 } else { ((scroll + 1.0) / count as f32).clamp(0.0, 1.0) }
}

pub(super) fn ease_out(p: f32) -> f32 {
    1.0 - (1.0 - p.clamp(0.0, 1.0)).powi(3)
}
