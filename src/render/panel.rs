use super::*;

/// Inset of a hover highlight inside the rounded settings row (arrow) or popup entry.
pub(super) const ARROW_INSET: f32 = 3.0;
pub(super) const POPUP_INSET: f32 = 4.0;

impl Painter<'_> {
    /// The actions of the selected row as a small menu beside the list; window coordinates.
    pub(super) unsafe fn popout(&self, p: &PopoutParams, labels: &[String]) {
        let (f, l, st) = (self.formats, self.l, self.s);
        let (r, s) = (l.popout_rect(p), l.ts);
        let (shown, first) = l.popout_window(p.items, p.selected);
        // Items sit POPOUT_PAD inside the frame, so their corners stay concentric with it.
        let panel_radius = st.shape.panel_radius * s;
        let item_radius = self.inner_corner(panel_radius, POPOUT_PAD * s);
        unsafe {
            self.fill_shape(grow(r, 3.0 * s), self.corner_after(panel_radius, 3.0 * s), self.c(st.popup_glow, 0.08));
            self.fill_shape(r, panel_radius, self.c(st.popup_fill, 0.96));
            for n in 0..shown {
                let index = first + n;
                let top = r.top + (POPOUT_PAD + n as f32 * POPOUT_ITEM_H) * s;
                let item = rect(r.left + POPOUT_PAD * s, top, r.right - POPOUT_PAD * s, top + POPOUT_ITEM_H * s);
                let selected = index == p.selected;
                if selected {
                    self.fill_shape(item, item_radius, self.c(st.popup_glow, 0.22));
                }
                let label = labels.get(index).map_or("", String::as_str);
                self.text(label, &f.label, rect(item.left + 10.0 * s, item.top, item.right - 24.0 * s, item.bottom), self.c(st.popup_text, 1.0));
                if selected {
                    self.text("\u{21B5}", &f.hint, rect(item.left, item.top + 5.0 * s, item.right - 8.0 * s, item.bottom), self.c(st.popup_hint, self.hc(0.8)));
                }
            }
            self.frame(r, panel_radius, self.c(st.popup_border, 1.0), 0.5 * s, 1.5 * s * st.shape.border_width);
        }
    }

    /// Small right-click entries (PASTE, then SETTINGS); window coordinates.
    pub(super) unsafe fn popup(&self, anchor: (f32, f32)) {
        let (f, l, st) = (self.formats, self.l, self.s);
        let (r, s) = (l.popup_rect(anchor), l.ts);
        let line = |i: usize| rect(r.left, r.top + i as f32 * POPUP_H * s, r.right, r.top + (i + 1) as f32 * POPUP_H * s);
        let radius = st.shape.panel_radius * s;
        let inset = if radius > 0.0 { POPUP_INSET * s } else { 0.0 };
        unsafe {
            self.fill_shape(grow(r, 3.0 * s), self.corner_after(radius, 3.0 * s), self.c(st.popup_glow, 0.08));
            self.fill_shape(r, radius, self.c(st.popup_fill, 0.96));
            for i in 0..POPUP_ENTRIES {
                if self.hovered(Hover::Popup(i)) {
                    self.fill_shape(grow(line(i), -inset), self.inner_corner(radius, inset), self.c(st.popup_glow, 0.14));
                }
            }
            self.frame(r, radius, self.c(st.popup_border, 1.0), 0.5 * s, 1.5 * s * st.shape.border_width);
            for (i, key) in ["popup.paste", "popup.settings"].into_iter().enumerate() {
                let row = line(i);
                self.text(&t(key), &f.label, rect(row.left + 14.0 * s, row.top, row.right - 30.0 * s, row.bottom), self.c(st.popup_text, 1.0));
                if self.hovered(Hover::Popup(i)) {
                    self.text("\u{21B5}", &f.hint, rect(row.left, row.top + 7.0 * s, row.right - 10.0 * s, row.bottom), self.c(st.popup_hint, self.hc(0.8)));
                }
            }
        }
    }

    /// The text format for an entry of the font dropdown, set in `family` (built on first use).
    /// Also whether the family can show its name: symbol fonts cannot, so their entries use the UI font.
    pub(super) fn preview(&self, family: &str) -> Option<(Rc<Face>, bool)> {
        let size = DROP_PREVIEW_SIZE * self.l.ts;
        let make = || unsafe {
            let format = self
                .dwrite
                .CreateTextFormat(&HSTRING::from(family), None, DWRITE_FONT_WEIGHT(Weight::Normal.value()), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))
                .ok()?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok()?;
            let sign = self.dwrite.CreateEllipsisTrimmingSign(&format).ok()?;
            let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            format.SetTrimming(&trimming, &sign).ok()?;
            Some((Rc::new(Face::new(format, 0.0)), fontlist::shows_name(self.dwrite, family)))
        };
        self.device.previews.borrow_mut().get_or_make(&format!("{size:.1}|{family}"), make)
    }

    /// The font dropdown over the card: SIZE and WEIGHT for a role, the filter, then the families
    /// in view, each in its own font.
    pub(super) unsafe fn dropdown(&self, view: &DropdownView) {
        let (f, l, st) = (self.formats, self.l, self.s);
        let s = l.ts;
        let parts = l.dropdown_parts(DropGeom { slot: view.slot, role: view.role });
        let (radius, bw) = (st.shape.panel_radius * s, st.shape.border_width);
        // Everything inside sits DROP_PAD in, so its corners stay concentric with the frame.
        let inner = self.inner_corner(radius, DROP_PAD * s);
        unsafe {
            self.fill_shape(grow(parts.outer, 3.0 * s), self.corner_after(radius, 3.0 * s), self.c(st.popup_glow, 0.08));
            self.fill_shape(parts.outer, radius, self.c(st.popup_fill, 0.98));
            self.text(&view.title, &f.label, rect(parts.title.left + 10.0 * s, parts.title.top, parts.title.right - 10.0 * s, parts.title.bottom), self.c(st.popup_hint, 1.0));
            if view.role {
                let rows = [(parts.size, "settings.dropdown.size", &view.size, Spot::Size, DropHit::Size as fn(i32) -> DropHit), (parts.weight, "settings.dropdown.weight", &view.weight, Spot::Weight, DropHit::Weight)];
                for (r, label, value, spot, hit) in rows {
                    let focused = view.spot == Some(spot);
                    let hovered = (-1..=1).any(|dir| self.hovered(Hover::Drop(hit(dir))));
                    if focused || hovered {
                        self.fill_shape(r, inner, self.c(st.popup_glow, if focused { 0.22 } else { 0.12 }));
                    }
                    let (lo, hi) = l.drop_arrows(r);
                    self.text(&t(label), &f.label, rect(r.left + 10.0 * s, r.top, lo.left, r.bottom), self.c(st.popup_text, 1.0));
                    self.text(value, &f.value, rect(lo.right, r.top, hi.left, r.bottom), self.c(st.popup_text, 1.0));
                    for (dir, arrow, glyph) in [(-1, lo, "\u{25C2}"), (1, hi, "\u{25B8}")] {
                        if self.hovered(Hover::Drop(hit(dir))) {
                            let inset = if inner > 0.0 { ARROW_INSET * s } else { 0.0 };
                            self.fill_shape(grow(arrow, -inset), self.inner_corner(inner, inset), self.c(st.popup_glow, 0.2));
                        }
                        self.text(glyph, &f.button, arrow, self.c(st.popup_hint, if focused { 1.0 } else { self.hc(0.7) }));
                    }
                }
            }
            // The filter: what is typed (its end, so the cursor stays in view), or the hint.
            self.fill_shape(parts.filter, inner, self.c(st.popup_glow, 0.1));
            self.frame(parts.filter, inner, self.c(st.popup_border, 0.6), 0.5 * s, s * bw);
            let field = rect(parts.filter.left + 10.0 * s, parts.filter.top, parts.filter.right - 10.0 * s, parts.filter.bottom);
            if view.filter.is_empty() {
                self.text(&t("settings.dropdown.filter"), &f.label, field, self.c(st.popup_hint, self.hc(0.55)));
            } else {
                let count = view.filter.chars().count();
                let tail: String = view.filter.chars().skip(count.saturating_sub(24)).collect();
                self.text(&format!("{}{tail}_", if count > 24 { "\u{2026}" } else { "" }), &f.label, field, self.c(st.popup_text, 1.0));
            }
            if view.total == 0 {
                self.text(&t("settings.dropdown.none"), &f.label, l.dropdown_item(&parts, 0), self.c(st.popup_hint, self.hc(0.7)));
            }
            for (n, choice) in view.choices.iter().enumerate() {
                let item = l.dropdown_item(&parts, n);
                let focused = view.spot == Some(Spot::Item(n));
                let hovered = self.hovered(Hover::Drop(DropHit::Item(n)));
                if focused || hovered {
                    self.fill_shape(item, inner, self.c(st.popup_glow, if focused { 0.22 } else { 0.12 }));
                }
                if choice.current {
                    let bar = rect(item.left + 3.0 * s, item.top + 6.0 * s, item.left + 6.0 * s, item.bottom - 6.0 * s);
                    self.fill_shape(bar, 1.5 * s, self.c(st.popup_hint, 1.0));
                }
                let preview = choice.family.as_deref().and_then(|family| self.preview(family));
                let symbol = preview.as_ref().is_some_and(|(_, readable)| !readable);
                let face = preview.as_ref().filter(|(_, readable)| *readable).map_or(&f.label, |(face, _)| &**face);
                let alpha = if focused || hovered || choice.current { 1.0 } else { self.hc(0.85) };
                let text = rect(item.left + 14.0 * s, item.top, item.right - 12.0 * s, item.bottom);
                if symbol {
                    // Its letters are pictures: the name in the UI font, dimmed, with a mark.
                    self.text(&choice.label, face, text, self.c(st.popup_text, alpha * 0.6));
                    self.text(&t("settings.dropdown.symbol"), &f.hint, rect(text.left, item.top + 5.0 * s, text.right - 8.0 * s, item.bottom), self.c(st.popup_hint, 0.6));
                } else {
                    self.text(&choice.label, face, text, self.c(st.popup_text, alpha));
                }
            }
            if view.total > VISIBLE {
                let (top, bottom) = (parts.items_top, parts.items_top + VISIBLE as f32 * DROP_ITEM_H * s);
                let x = parts.outer.right - 5.0 * s;
                let at = |share: f32| top + (bottom - top) * share;
                self.fill(rect(x, top, x + 2.0 * s, bottom), self.c(st.popup_border, 0.25));
                self.fill(rect(x, at(view.top as f32 / view.total as f32), x + 2.0 * s, at((view.top + VISIBLE) as f32 / view.total as f32)), self.c(st.popup_hint, 0.8));
            }
            self.frame(parts.outer, radius, self.c(st.popup_border, 1.0), 0.5 * s, 1.5 * s * bw);
        }
    }

    /// The header toggles' pop-out over the card: a title, then one line per toggle with its state.
    pub(super) unsafe fn toggles(&self, view: &TogglesView) {
        let (f, l, st) = (self.formats, self.l, self.s);
        let s = l.ts;
        let parts = l.toggles_parts(view.slot, view.entries.len());
        let (radius, bw) = (st.shape.panel_radius * s, st.shape.border_width);
        let inner = self.inner_corner(radius, DROP_PAD * s);
        unsafe {
            self.fill_shape(grow(parts.outer, 3.0 * s), self.corner_after(radius, 3.0 * s), self.c(st.popup_glow, 0.08));
            self.fill_shape(parts.outer, radius, self.c(st.popup_fill, 0.98));
            self.text(&view.title, &f.label, rect(parts.title.left + 10.0 * s, parts.title.top, parts.title.right - 10.0 * s, parts.title.bottom), self.c(st.popup_hint, 1.0));
            for (n, entry) in view.entries.iter().enumerate() {
                let item = l.dropdown_item(&parts, n);
                let focused = view.focus == n;
                if focused || self.hovered(Hover::Drop(DropHit::Item(n))) {
                    self.fill_shape(item, inner, self.c(st.popup_glow, if focused { 0.22 } else { 0.12 }));
                }
                let alpha = if focused || entry.on { 1.0 } else { self.hc(0.7) };
                self.text(&entry.label, &f.label, rect(item.left + 14.0 * s, item.top, item.right - 60.0 * s, item.bottom), self.c(st.popup_text, alpha));
                let state = t(if entry.on { "settings.value.on" } else { "settings.value.off" });
                self.text(&state, &f.value, rect(item.right - 56.0 * s, item.top, item.right - 8.0 * s, item.bottom), self.c(st.popup_hint, alpha));
            }
            self.frame(parts.outer, radius, self.c(st.popup_border, 1.0), 0.5 * s, 1.5 * s * bw);
        }
    }

    /// One of the two buttons under the rows.
    pub(super) unsafe fn panel_button(&self, b: D2D_RECT_F, label: &str, selected: bool, hit: PanelHit) {
        let (f, l, st) = (self.formats, self.l, self.s);
        let s = l.ts;
        let hovered = self.hovered(Hover::Panel(hit));
        let (radius, bw) = (st.shape.row_radius * s, st.shape.border_width);
        unsafe {
            if selected {
                for (by, glow) in [(4.0, 0.08), (2.0, 0.16)] {
                    self.halo(b, radius, by * s, self.c(st.panel_button, glow), 2.0 * s);
                }
            }
            self.fill_shape(b, radius, self.c(st.panel_button_fill, 0.88));
            if hovered && !selected {
                self.fill_shape(b, radius, self.c(st.panel_button, 0.08));
            }
            let border = if selected { 1.0 } else if hovered { 0.85 } else { 0.45 };
            self.frame(b, radius, self.c(st.panel_button, border), 0.5 * s, if selected { 1.5 * s } else { s } * bw);
            let text = if selected { 1.0 } else if hovered { 0.95 } else { self.hc(0.75) };
            self.text(label, &f.button, b, self.c(st.panel_button, text));
        }
    }

    /// The settings card: red tab strip, thin red frame with corner accents, tabs, rows, note, buttons.
    pub(super) unsafe fn panel(&self, view: &PanelView) {
        let (f, l, st) = (self.formats, self.l, self.s);
        let s = l.ts;
        let card = l.card();
        let (card_radius, row_radius, bw) = (st.shape.panel_radius * s, st.shape.row_radius * s, st.shape.border_width);
        unsafe {
            self.fill_shape(card, card_radius, self.c(st.panel_fill, 0.92));
            self.frame(card, card_radius, self.c(st.panel_frame, 0.7), 0.5 * s, s * bw);
            // The strip stays between the rounded corners.
            let (strip_left, strip_right) = self.top_strip(card, card_radius);
            self.fill(rect(strip_left, card.top, strip_right, card.top + 6.0 * s), self.c(st.panel_strip, 0.95));
            let mark_right = (card.right - 30.0 * s).min(strip_right);
            self.fill(rect((card.right - 70.0 * s).min(mark_right), card.top, mark_right, card.top + 6.0 * s), self.c(st.panel_strip_mark, 0.35));
            let (len, thick) = (16.0 * s, 2.0 * s);
            let outer = grow(card, 2.0 * s);
            // Square corner marks don't fit a rounded card.
            let marks: &[(f32, f32, f32, f32)] = if card_radius > 0.0 {
                &[]
            } else {
                &[(outer.left, 1.0, outer.top, 1.0), (outer.right, -1.0, outer.top, 1.0), (outer.left, 1.0, outer.bottom, -1.0), (outer.right, -1.0, outer.bottom, -1.0)]
            };
            for &(x, sx, y, sy) in marks {
                let (x2, y2) = (x + sx * len, y + sy * len);
                self.fill(rect(x.min(x2), y.min(y + sy * thick), x.max(x2), y.max(y + sy * thick)), self.c(st.panel_corner, 1.0));
                self.fill(rect(x.min(x + sx * thick), y.min(y2), x.max(x + sx * thick), y.max(y2)), self.c(st.panel_corner, 1.0));
            }

            let (left, right) = l.card_inner();
            let title = view.wizard.as_ref().map_or_else(|| t("settings.title"), |w| w.title.clone());
            self.shadowed(&title, &f.header, rect(left, card.top + 12.0 * s, right, card.top + 38.0 * s), self.c(st.panel_title, 1.0));
            let close = l.close_rect();
            let close_hovered = self.hovered(Hover::Panel(PanelHit::Close));
            if close_hovered {
                self.fill_shape(close, row_radius, self.c(st.panel_arrow, 0.16));
            }
            let (centre, reach) = (((close.left + close.right) / 2.0, (close.top + close.bottom) / 2.0), 5.0 * s);
            let x_color = self.c(st.panel_arrow, if close_hovered { 1.0 } else { self.hc(0.7) });
            self.brush.SetColor(&x_color);
            for (dx, dy) in [(-reach, -reach), (-reach, reach)] {
                let (from, to) = (Vector2::new(centre.0 + dx, centre.1 + dy), Vector2::new(centre.0 - dx, centre.1 - dy));
                self.rt.DrawLine(from, to, self.brush, 1.6 * s, None);
            }

            if let Some(w) = &view.wizard {
                self.text(&w.heading, &f.label, rect(left, card.top + 44.0 * s, right, card.top + 70.0 * s), self.c(st.panel_arrow, 1.0));
            }
            for (i, tab) in TABS.iter().enumerate().filter(|_| view.wizard.is_none()) {
                let r = l.tab_rect(i);
                let active = view.tab == i;
                let hovered = self.hovered(Hover::Panel(PanelHit::Tab(i))) && !active;
                if active {
                    self.fill_shape(r, row_radius, self.c(st.panel_tab_active, 0.35));
                } else {
                    self.fill_shape(r, row_radius, self.c(st.panel_tab_fill, 0.7));
                    if hovered {
                        self.fill_shape(r, row_radius, self.c(st.panel_tab, 0.14));
                    }
                }
                let border = if active { 1.0 } else if hovered { 0.8 } else { 0.4 };
                self.frame(r, row_radius, self.c(st.panel_tab, border), 0.5 * s, s * bw);
                let (paint, alpha) = if active { (st.panel_tab_active_text, 1.0) } else { (st.panel_tab_text, if hovered { 1.0 } else { self.hc(0.8) }) };
                self.text(&tab.label(), &f.button, r, self.c(paint, alpha));
            }

            for (i, slot) in view.slots.iter().enumerate() {
                let r = l.setting_rect(i);
                let row = match slot {
                    Slot::Heading(title) => {
                        self.text(title, &f.label, rect(r.left + 2.0 * s, r.top + 8.0 * s, r.right, r.bottom), self.c(st.panel_title, 0.9));
                        self.fill(rect(r.left, r.bottom - 4.0 * s, r.right, r.bottom - 3.0 * s), self.c(st.panel_frame, 0.4));
                        continue;
                    }
                    Slot::Row(row) => row,
                };
                let selected = view.selected == Some(i);
                let row_hover = self.hovered(Hover::Panel(PanelHit::Row(i)))
                    || (row.dropdown && [-1, 1].into_iter().any(|dir| self.hovered(Hover::Panel(PanelHit::Arrow(i, dir)))));
                if selected {
                    for (by, glow) in [(4.0, 0.06), (2.0, 0.14)] {
                        self.halo(r, row_radius, by * s, self.c(st.panel_row_glow, glow), 2.0 * s);
                    }
                }
                self.fill_shape(r, row_radius, self.c(st.panel_row_fill, 0.88));
                if selected {
                    self.fill_shape(r, row_radius, self.c(st.panel_row_glow, 0.08));
                }
                let border = if selected { 1.0 } else if row_hover { 0.6 } else { 0.3 };
                self.frame(r, row_radius, self.c(st.panel_row_border, border), 0.5 * s, if selected { 1.5 * s } else { s } * bw);
                let (lo, hi) = (l.arrow_rect(i, -1), l.arrow_rect(i, 1));
                let label_right = lo.left;
                let value = l.value_rect(i, row.stepper);
                let (value_left, value_right) = (value.left, value.right);
                let label_alpha = if selected { 1.0 } else { self.hc(0.75) };
                if let Some(cell) = &row.color {
                    // Name and whether it follows the theme on two lines, the swatch beside the arrows.
                    let swatch = rect(lo.left - 22.0 * s, r.top + 12.0 * s, lo.left - 6.0 * s, r.top + 28.0 * s);
                    let swatch_radius = st.shape.icon_radius * s;
                    // A checker behind the color shows how transparent it is; only its middle, clear of rounded corners.
                    self.fill_shape(swatch, swatch_radius, rgba(0xA0A0A0, 1.0));
                    let mid = ((swatch.left + swatch.right) / 2.0, (swatch.top + swatch.bottom) / 2.0);
                    let square = 4.0 * s;
                    for (dx, dy) in [(-1.0, -1.0), (0.0, 0.0)] {
                        self.fill(rect(mid.0 + dx * square, mid.1 + dy * square, mid.0 + (dx + 1.0) * square, mid.1 + (dy + 1.0) * square), rgba(0x404040, 1.0));
                    }
                    self.fill_shape(swatch, swatch_radius, self.c(Paint { rgb: cell.rgb, alpha: cell.alpha }, 1.0));
                    self.frame(swatch, swatch_radius, self.c(st.panel_label, 0.6), 0.5 * s, s * bw);
                    let label_right = swatch.left - 4.0 * s;
                    self.text(&row.label, &f.label, rect(r.left + 10.0 * s, r.top + 2.0 * s, label_right, r.top + 24.0 * s), self.c(st.panel_label, label_alpha));
                    let (state, paint, alpha) =
                        if cell.custom { (t("settings.state.custom"), st.panel_arrow, 0.9) } else { (t("settings.state.theme_default"), st.panel_note, self.hc(0.55)) };
                    self.text(&state, &f.caption, rect(r.left + 10.0 * s, r.top + 24.0 * s, label_right, r.bottom), self.c(paint, alpha));
                    let (paint, alpha) = if cell.editing { (st.panel_arrow, 1.0) } else { (st.panel_value, if cell.custom { 1.0 } else { self.hc(0.6) }) };
                    // The hex on the first line and the opacity under it, like the label and its state.
                    self.text(&row.value, &f.value, rect(value_left, r.top, value_right, r.top + (r.bottom - r.top) * 0.62), self.c(paint, alpha));
                    self.text(&format!("{}%", Paint { rgb: 0, alpha: cell.alpha }.percent()), &f.button, rect(value_left, r.top + (r.bottom - r.top) * 0.55, value_right, r.bottom), self.c(paint, alpha * 0.8));
                } else if row.dropdown {
                    // A longer value (family, size, weight) than the control has room for, then a ▾ instead of arrows.
                    let split = r.left + (r.right - r.left) * 0.36;
                    let mark = rect(r.right - 28.0 * s, r.top, r.right - 4.0 * s, r.bottom);
                    self.text(&row.label, &f.label, rect(r.left + 10.0 * s, r.top, split, r.bottom), self.c(st.panel_label, label_alpha));
                    self.text(&row.value, &f.button, rect(split, r.top, mark.left, r.bottom), self.c(st.panel_value, 1.0));
                    self.text("\u{25BE}", &f.button, mark, self.c(st.panel_arrow, if selected || row_hover { 1.0 } else { self.hc(0.55) }));
                } else {
                    self.text(&row.label, &f.label, rect(r.left + 10.0 * s, r.top, label_right, r.bottom), self.c(st.panel_label, label_alpha));
                    self.text(&row.value, &f.value, rect(value_left, r.top, value_right, r.bottom), self.c(st.panel_value, 1.0));
                }
                if row.stepper {
                    for (dir, arrow_rect, glyph) in [(-1, lo, "\u{25C2}"), (1, hi, "\u{25B8}")] {
                        let hovered = self.hovered(Hover::Panel(PanelHit::Arrow(i, dir)));
                        if hovered {
                            // Inside the row's rounded edge, not flush with it.
                            let inset = if row_radius > 0.0 { ARROW_INSET * s } else { 0.0 };
                            self.fill_shape(grow(arrow_rect, -inset), self.inner_corner(row_radius, inset), self.c(st.panel_arrow, 0.16));
                        }
                        let alpha = if selected || hovered { 1.0 } else { self.hc(0.55) };
                        self.text(glyph, &f.button, arrow_rect, self.c(st.panel_arrow, alpha));
                    }
                }
            }

            if let Some((from, to)) = view.scroll {
                let (top, bottom) = (l.setting_rect(0).top, l.setting_rect(SLOTS - 1).bottom);
                let x = card.right - 8.0 * s;
                // Only the straight part of the card's right edge.
                let y = |y: f32| y.max(card.top + card_radius).min(card.bottom - card_radius);
                self.fill(rect(x, y(top), x + 2.0 * s, y(bottom)), self.c(st.panel_frame, 0.25));
                self.fill(rect(x, y(top + (bottom - top) * from), x + 2.0 * s, y(top + (bottom - top) * to)), self.c(st.panel_arrow, 0.8));
            }

            if let Some(id) = view.note.id() {
                let (paint, alpha) = if view.note.is_info() { (st.panel_note, self.hc(0.7)) } else { (st.panel_error, 1.0) };
                self.text(&t(id), &f.caption, l.note_rect(), self.c(paint, alpha));
            } else if let Some(detail) = &view.detail {
                self.text(detail, &f.caption, l.note_rect(), self.c(st.panel_note, self.hc(0.7)));
            }

            if let Some(w) = &view.wizard {
                let top = l.setting_rect(view.slots.len()).top + 4.0 * s;
                for (n, line) in w.text.iter().enumerate() {
                    self.text(line, &f.caption, rect(left, top + n as f32 * 16.0 * s, right, top + (n + 1) as f32 * 16.0 * s), self.c(st.panel_note, self.hc(0.85)));
                }
                if w.can_back {
                    self.panel_button(l.wizard_button(0), &t("wizard.back"), false, PanelHit::Back);
                }
                self.panel_button(l.wizard_button(1), &t("wizard.skip"), false, PanelHit::Skip);
                self.panel_button(l.wizard_button(2), &w.next, true, PanelHit::Next);
                return;
            }
            self.panel_button(l.config_button_rect(), &t("settings.button.config"), view.button == Some(Button::Config), PanelHit::OpenConfig);
            self.panel_button(l.style_button_rect(), &t("settings.button.style"), view.button == Some(Button::Style), PanelHit::OpenStyle);
        }
    }
}
