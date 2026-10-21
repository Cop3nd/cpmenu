use super::*;

/// UTF-16 length of the "> " in front of the query in the header.
pub(super) const PROMPT_UNITS: u32 = 2;

/// The header's query line ("> " and `shown`, the upper-cased query) laid out for hit testing.
pub(super) fn query_layout(dwrite: &IDWriteFactory, face: &Face, shown: &str) -> Option<IDWriteTextLayout> {
    face.layout(dwrite, Use::HitTest, shown, (4096.0, 256.0))
}

/// Left edge, top and height of the caret position in front of UTF-16 unit `unit` of `layout`.
pub(super) fn unit_position(layout: &IDWriteTextLayout, unit: u32) -> Option<(f32, f32, f32)> {
    let (mut x, mut y, mut m) = (0.0, 0.0, DWRITE_HIT_TEST_METRICS::default());
    unsafe { layout.HitTestTextPosition(unit, false, &mut x, &mut y, &mut m) }.ok().map(|_| (x, y, m.height))
}

/// Width of the "> " in front of the query, which stays put while the text scrolls.
pub(super) fn prompt_width(layout: &IDWriteTextLayout) -> f32 {
    unit_position(layout, PROMPT_UNITS).map_or(0.0, |p| p.0)
}

/// Pixel width of `s` set in `face`.
pub(super) fn text_width(dwrite: &IDWriteFactory, face: &Face, s: &str) -> f32 {
    let mut metrics = DWRITE_TEXT_METRICS::default();
    let measured = face.layout(dwrite, Use::Measure, s, (4096.0, 64.0)).and_then(|layout| unsafe { layout.GetMetrics(&mut metrics) }.ok());
    if measured.is_some() { metrics.widthIncludingTrailingWhitespace } else { 0.0 }
}

/// Gap between the query text and the match count or notice at its right, per unit of scale.
pub(super) const QUERY_GAP: f32 = 8.0;

/// Room added to a text's box where its width was measured, so the last pixel isn't clipped.
const LINE_SLACK: f32 = 2.0;

/// A text `text_w` px wide placed in `area` per `align`: the box to draw it in.
fn aligned_in(area: D2D_RECT_F, text_w: f32, align: TextAlign) -> D2D_RECT_F {
    let room = area.right - area.left;
    if text_w >= room {
        return area;
    }
    let x = area.left + match align {
        TextAlign::Left => 0.0,
        TextAlign::Center => (room - text_w) / 2.0,
        TextAlign::Right => room - text_w,
    };
    rect(x, area.top, x + text_w + LINE_SLACK, area.bottom)
}

/// Where the title and the count start on a line `avail` px wide (relative to its left edge), for a
/// title `title_w` wide and a count `count_w` wide (0 = none), with `gap` between them. The title
/// goes where `title` says. The count takes the free side the title leaves (`count` Left prefers
/// the left, Right the right, Center the roomier one), aligned inside that space as `count` says.
/// If neither side has room (a centred title), the title moves aside so the count fits. The two
/// never overlap, as long as `title_w + gap + count_w <= avail`.
pub(super) fn line_slots(avail: f32, title_w: f32, count_w: f32, gap: f32, title: TextAlign, count: TextAlign) -> (f32, f32) {
    let mut title_x = match title {
        TextAlign::Left => 0.0,
        TextAlign::Center => (avail - title_w) / 2.0,
        TextAlign::Right => avail - title_w,
    };
    if count_w <= 0.0 {
        return (title_x, 0.0);
    }
    let (left, right) = ((0.0, title_x - gap), (title_x + title_w + gap, avail));
    let roomy = |zone: (f32, f32)| zone.1 - zone.0 >= count_w;
    let order = match count {
        TextAlign::Left => [left, right],
        TextAlign::Right => [right, left],
        TextAlign::Center if left.1 - left.0 > right.1 - right.0 => [left, right],
        TextAlign::Center => [right, left],
    };
    let Some(zone) = order.into_iter().find(|&z| roomy(z)) else {
        return if count == TextAlign::Left {
            title_x = title_x.max(count_w + gap);
            (title_x, 0.0)
        } else {
            title_x = title_x.min(avail - count_w - gap - title_w);
            (title_x, avail - count_w)
        };
    };
    let count_x = match count {
        TextAlign::Left => zone.0,
        TextAlign::Center => zone.0 + (zone.1 - zone.0 - count_w) / 2.0,
        TextAlign::Right => zone.1 - count_w,
    };
    (title_x, count_x)
}

impl Painter<'_> {
    /// The settings button in the header: a toothed ring drawn from lines (Bahnschrift has no gear glyph).
    pub(super) unsafe fn gear(&self, r: D2D_RECT_F) {
        let (st, s) = (self.s, self.l.ts);
        let hovered = self.hovered(Hover::Gear);
        let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
        let radius = (r.right - r.left).min(r.bottom - r.top) / 2.0 - 2.0 * s;
        #[cfg(test)]
        count_draw();
        unsafe {
            if hovered {
                self.fill_shape(r, st.shape.row_radius * s, self.c(st.header_line, 0.22));
            }
            let color = self.c(st.header_caption, if hovered { 1.0 } else { self.hc(0.8) });
            self.brush.SetColor(&color);
            let ring = D2D1_ELLIPSE { point: Vector2::new(cx, cy), radiusX: radius * 0.6, radiusY: radius * 0.6 };
            self.rt.DrawEllipse(&ring, self.brush, 1.6 * s, None);
            for tooth in 0..8 {
                let angle = tooth as f32 * std::f32::consts::FRAC_PI_4;
                let (dx, dy) = (angle.cos(), angle.sin());
                let (from, to) = (Vector2::new(cx + dx * radius * 0.75, cy + dy * radius * 0.75), Vector2::new(cx + dx * radius, cy + dy * radius));
                self.rt.DrawLine(from, to, self.brush, 2.4 * s, None);
            }
        }
    }

    /// The title line keeps the layout it always had: title left, count right.
    fn default_line(&self) -> bool {
        self.l.show.title_align == TextAlign::Left && self.l.show.count_align == TextAlign::Right
    }

    /// The boxes of the title line's two texts, `title_w` and `count_w` px wide (0 = no count),
    /// and how far right of the margin the title starts. The count goes where the title leaves
    /// room (see `line_slots`). With the default alignment these are `plain`, the boxes the line
    /// has always used, and nothing is measured: callers pass widths only otherwise.
    fn line_boxes(&self, title_w: f32, count_w: f32, plain: (D2D_RECT_F, D2D_RECT_F)) -> (D2D_RECT_F, D2D_RECT_F, f32) {
        if self.default_line() {
            return (plain.0, plain.1, 0.0);
        }
        let (l, s) = (self.l, self.l.ts);
        let avail = l.w - 2.0 * l.margin;
        let gap = QUERY_GAP * s;
        let title_w = title_w.min(avail - if count_w > 0.0 { count_w + gap } else { 0.0 }).max(0.0);
        let (tx, cx) = line_slots(avail, title_w, count_w, gap, l.show.title_align, l.show.count_align);
        let title = rect(l.margin + tx, plain.0.top, l.margin + tx + title_w + LINE_SLACK, plain.0.bottom);
        // The count's face is right aligned: its extra room goes on the left.
        let count = rect(l.margin + cx - LINE_SLACK, plain.1.top, l.margin + cx + count_w, plain.1.bottom);
        (title, count, tx)
    }

    pub(super) unsafe fn header(&self, menu: &Menu, open_ms: f32, frame_no: u32) {
        let (f, l, st) = (self.formats, self.l, self.s);
        let (s, m, w, trim, show) = (l.ts, l.margin, l.w, l.head_trim, l.show);
        let plain = self.default_line();
        unsafe {
            // The window is transparent: a light backdrop keeps the header and the typed query
            // readable over bright wallpapers or windows.
            if show.header_backdrop {
                self.fill(rect(m - 8.0 * s, 0.0, w - m + 8.0 * s, 60.0 * s - trim), self.c(st.header_backdrop, l.backdrop));
            }
            if show.caption {
                let caption = t("header.title");
                let mut area = rect(m, 4.0 * s, w - m - if show.gear { 24.0 * s } else { 0.0 }, 20.0 * s);
                if show.caption_align != TextAlign::Left {
                    area = aligned_in(area, text_width(self.dwrite, &f.caption, &caption), show.caption_align);
                }
                self.shadowed(&caption, &f.caption, area, self.c(st.header_caption, self.hc(0.8)));
            }
            if show.gear {
                self.gear(l.gear_rect());
            }
            if show.header_line {
                self.fill(rect(m, 24.0 * s, w - m, 25.0 * s), self.c(st.header_line, 0.6));
            }
            if show.progress {
                let (scroll, count) = match menu.list_actions() {
                    Some(actions) => (actions.scroll, actions.acts.len()),
                    None => (menu.scroll, menu.view.len()),
                };
                self.fill(rect(m, 22.0 * s, m + (w - 2.0 * m) * progress(scroll, count), 25.0 * s), self.c(st.header_progress, 1.0));
            }

            let title = rect(m, 32.0 * s - trim, w, 58.0 * s - trim);
            let sub = rect(m, 37.0 * s - trim, w - m, 56.0 * s - trim);
            let subtitle = t("header.subtitle");
            let to_the_right = |hint: &str, cap: f32| text_width(self.dwrite, &f.hint, hint).min(cap);
            if let Some(actions) = menu.list_actions() {
                menu.query_scroll.set(0.0);
                menu.query_x.set(0.0);
                let name = format!("< {}", actions.name.to_uppercase());
                // A notice (a plugin error) always shows; the count's place holds a hint here.
                let side = match &menu.notice {
                    Some(notice) => Some((notice.clone(), st.header_title, 1.0)),
                    None => show.count.then(|| (t("header.actions"), st.header_count, 0.9)),
                };
                let name_box = rect(title.left, title.top, w - m - 70.0 * s, title.bottom);
                let (name_box, side_box) = if plain {
                    (name_box, sub)
                } else {
                    let count_w = side.as_ref().map_or(0.0, |(text, ..)| to_the_right(text, sub.right - sub.left));
                    let (name_box, side_box, _) = self.line_boxes(text_width(self.dwrite, &f.query, &name), count_w, (name_box, sub));
                    (name_box, side_box)
                };
                self.shadowed(&name, &f.query, name_box, self.c(st.header_query, 1.0));
                if let Some((text, paint, alpha)) = side {
                    self.shadowed(&text, &f.hint, side_box, self.c(paint, alpha));
                }
            } else if menu.query.is_empty() {
                menu.query_scroll.set(0.0);
                menu.query_x.set(0.0);
                let notice_box = rect(w * 0.45, sub.top, sub.right, sub.bottom);
                let (title_box, notice_box) = if plain || (menu.notice.is_none() && show.title_align == TextAlign::Left) {
                    (title, notice_box)
                } else {
                    let count_w = menu.notice.as_ref().map_or(0.0, |notice| to_the_right(notice, notice_box.right - notice_box.left));
                    let (title_box, notice_box, _) = self.line_boxes(if show.subtitle { text_width(self.dwrite, &f.header, &subtitle) } else { 0.0 }, count_w, (title, notice_box));
                    (title_box, notice_box)
                };
                if open_ms < GLITCH_MS && show.open_glitch && show.subtitle {
                    let k = 1.0 - open_ms / GLITCH_MS;
                    let shifted = |dx: f32| rect(title_box.left + dx, title_box.top, title_box.right + dx, title_box.bottom);
                    self.text(&subtitle, &f.header, shifted(-7.0 * s * k), self.c(st.header_glitch, 0.7 * k));
                    self.text(&subtitle, &f.header, shifted(7.0 * s * k), rgba(0xFFFFFF, 0.5 * k));
                    self.glitch_slices(k, frame_no);
                }
                if show.subtitle {
                    self.shadowed(&subtitle, &f.header, title_box, self.c(st.header_title, 1.0));
                }
                if let Some(notice) = &menu.notice {
                    self.shadowed(notice, &f.hint, notice_box, self.c(st.header_title, 1.0));
                }
            } else {
                menu.query_x.set(0.0);
                let (shown, offsets) = lineedit::shouted(&menu.query);
                let query = format!("> {shown}");
                let layout = query_layout(self.dwrite, &f.query, &query);
                // The count or notice on the right; the query never runs into it. A notice always
                // shows, the match count only with `count`.
                let side = match &menu.notice {
                    Some(notice) => Some((notice.clone(), rect(w * 0.4, sub.top, sub.right, sub.bottom), st.header_title, 1.0)),
                    None => show.count.then(|| (lang::plural("header.matches", menu.view.len()), sub, st.header_count, 0.9)),
                };
                let side_w = side.as_ref().map(|(text, side_box, ..)| to_the_right(text, side_box.right - side_box.left));
                // Position (left, top, height) in the layout of the caret before character `i`.
                let at = |i: usize| layout.as_ref().and_then(|l| unit_position(l, PROMPT_UNITS + offsets[i.min(offsets.len() - 1)]));
                let prompt_w = layout.as_ref().map_or(0.0, prompt_width);
                let text_left = m + prompt_w;
                let area_right = match side_w {
                    Some(side_w) => (w - m - side_w - QUERY_GAP * s).max(text_left + 24.0 * s),
                    None => w - m,
                };
                let text_end = at(usize::MAX).map_or(0.0, |p| p.0 - prompt_w);
                let caret_x = at(menu.query.caret()).map_or(0.0, |p| p.0 - prompt_w);
                let area_w = area_right - text_left;
                let scroll = query_scroll(menu.query_scroll.get(), caret_x, text_end, area_w);
                menu.query_scroll.set(scroll);
                // "> " and the visible part of the text are placed as one block.
                let (_, side_box, shift) = if plain {
                    (title, side.as_ref().map_or(sub, |(_, side_box, ..)| *side_box), 0.0)
                } else {
                    let block = prompt_w + (text_end + CARET_PAD + (1.5 * s).max(1.0)).min(area_w);
                    let plain_side = side.as_ref().map_or(sub, |(_, side_box, ..)| *side_box);
                    self.line_boxes(block, side_w.unwrap_or(0.0), (title, plain_side))
                };
                menu.query_x.set(shift);
                let left = m + shift;
                let text_left = left + prompt_w;
                let origin = left - scroll;
                let on_screen = |p: (f32, f32, f32)| (origin + p.0, title.top + f.query.dy + p.1, p.2);
                self.rt.PushAxisAlignedClip(&rect(text_left, title.top, text_left + area_w, title.bottom), D2D1_ANTIALIAS_MODE_ALIASED);
                if let Some((a, b)) = menu.query.selection() {
                    if let (Some((left, top, height)), Some((right, ..))) = (at(a).map(on_screen), at(b).map(on_screen)) {
                        self.fill_shape(rect(left, top, right, top + height), st.shape.tag_radius * s, self.c(st.header_query, 0.3));
                    }
                }
                self.shadowed(&query, &f.query, rect(origin, title.top, origin + 4096.0, title.bottom), self.c(st.header_query, 1.0));
                if let Some((x, top, height)) = at(menu.query.caret()).map(on_screen) {
                    self.fill(rect(x, top, x + (1.5 * s).max(1.0), top + height), self.c(st.header_query, 1.0));
                }
                self.rt.PopAxisAlignedClip();
                self.shadowed("> ", &f.query, rect(left, title.top, title.right, title.bottom), self.c(st.header_query, 1.0));
                if let Some((text, _, paint, alpha)) = side {
                    self.shadowed(&text, &f.hint, side_box, self.c(paint, alpha));
                }
            }
        }
    }

    pub(super) unsafe fn glitch_slices(&self, strength: f32, frame_no: u32) {
        let (s, w) = (self.l.ts, self.l.w);
        let mut seed = frame_no.wrapping_mul(2_654_435_761) | 1;
        let mut random = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 10_000) as f32 / 10_000.0
        };
        for _ in 0..4 {
            let y = random() * 58.0 * s - self.l.head_trim;
            let height = (1.0 + random() * 5.0) * s;
            let x = random() * w * 0.6;
            let paint = if random() < 0.5 { self.s.header_line } else { self.s.header_glitch };
            unsafe { self.fill(rect(x, y, x + (40.0 + random() * 160.0) * s, y + height), self.c(paint, 0.6 * strength)) };
        }
    }
}
