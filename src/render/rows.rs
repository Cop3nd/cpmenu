use super::*;

impl Painter<'_> {
    pub(super) unsafe fn bitmap(&self, pixels: &[u8]) -> Option<ID2D1Bitmap> {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let size = D2D_SIZE_U { width: ICON as u32, height: ICON as u32 };
        unsafe { self.rt.CreateBitmap(size, Some(pixels.as_ptr() as _), (ICON * 4) as u32, &props) }.ok()
    }

    /// Accent line-art look for normal rows, crossfading to full colour as the row becomes selected.
    pub(super) unsafe fn icon(&self, index: usize, icon: &Icon, area: D2D_RECT_F, selected: f32, a: f32) -> bool {
        let mut cache = self.device.icons.borrow_mut();
        // The accent look is only made when it is used.
        let tinted = !self.l.show.color_icons;
        if !cache.get(&index).is_some_and(|(accent, _)| accent.is_some() || !tinted) {
            let accent = if tinted { unsafe { self.bitmap(&tint(&icon.color, self.s.row_icon.rgb)) } } else { None };
            let color = match cache.get(&index) {
                Some((_, color)) => Some(color.clone()),
                None => unsafe { self.bitmap(&icon.color) },
            };
            let Some(color) = color.filter(|_| accent.is_some() || !tinted) else { return false };
            cache.insert(index, (accent, color));
        }
        let (accent, color) = &cache[&index];
        let half = ICON as f32 * self.l.k / 2.0;
        let (cx, cy) = ((area.left + area.right) / 2.0, (area.top + area.bottom) / 2.0);
        let dest = rect(cx - half, cy - half, cx + half, cy + half);
        let mode = D2D1_BITMAP_INTERPOLATION_MODE_LINEAR;
        let tint_alpha = self.s.row_icon.alpha;
        unsafe {
            let Some(accent) = accent else {
                self.rt.DrawBitmap(color, Some(&dest), a, mode, None);
                return true;
            };
            if selected < 1.0 {
                self.rt.DrawBitmap(accent, Some(&dest), (1.0 - selected) * 0.9 * a * tint_alpha, mode, None);
            }
            if selected > 0.0 {
                self.rt.DrawBitmap(color, Some(&dest), selected * a, mode, None);
            }
        }
        true
    }

    /// Left and right edge of each TAG chip for `labels`, the first starting at `x`: every chip is its
    /// text's width plus the padding twice, and `tag_gap` apart.
    pub(super) fn chip_spans(&self, labels: &[&str], mut x: f32) -> Vec<(f32, f32)> {
        let (pad, gap) = (self.s.shape.tag_padding * self.l.k, self.s.shape.tag_gap * self.l.k);
        labels
            .iter()
            .map(|label| {
                let right = x + text_width(self.dwrite, &self.formats.tag, label) + 2.0 * pad;
                let span = (x, right);
                x = right + gap;
                span
            })
            .collect()
    }

    /// `row` is the position in the list of `count` rows, `scroll` the eased selection index.
    pub(super) unsafe fn row(&self, index: usize, item: &Item, recent: bool, (row, scroll, count): (usize, f32, usize), open_ms: f32) {
        let (l, st, f) = (self.l, self.s, self.formats);
        let (k, show) = (l.k, l.show);
        let rel = row as f32 - scroll;
        let settled_top = l.row_top(row as f32, scroll, count);
        // Cascade by screen position, so rows still slide in from the top down.
        let order = ((settled_top - l.list_top) / l.step).max(0.0);
        let appear = if show.row_cascade { ease_out((open_ms - order * ROW_DELAY_MS) / ROW_IN_MS) } else { 1.0 };
        let top = settled_top - 16.0 * l.scale * (1.0 - appear);
        if top + l.row_h < l.list_top - l.step || top > l.h {
            return;
        }
        let busy = item.extra.as_ref().is_some_and(|e| e.busy);
        let a = l.position_alpha(settled_top, scroll, count) * appear * if busy { 0.45 } else { 1.0 };
        if a <= 0.01 {
            return;
        }
        let ta = self.row_text_alpha(a);
        let distance = rel.abs();
        let s = (1.0 - distance).max(0.0);

        let (panel_left, icon_left) = l.row_x(distance);
        // Without icons the panel takes over the icon box's place.
        let panel_right = if l.icons { icon_left - 6.0 * k } else { icon_left + l.row_h };
        let panel = rect(panel_left, top, panel_right, top + l.row_h);
        let icon = rect(icon_left, top, icon_left + l.row_h, top + l.row_h);
        let border = mix(st.row_border, st.row_border_selected, s, (0.35 + 0.65 * s) * a);
        let border_w = (1.0 + 0.5 * s) * k * st.shape.border_width;
        let boxes: &[(D2D_RECT_F, f32)] = if l.icons { &[(panel, st.shape.row_radius * k), (icon, st.shape.icon_radius * k)] } else { &[(panel, st.shape.row_radius * k)] };

        unsafe {
            if s > 0.0 && show.glow {
                for (by, glow) in [(6.0, 0.05), (4.0, 0.1), (2.0, 0.18)] {
                    for &(r, radius) in boxes {
                        self.halo(r, radius, by * k, self.c(st.row_glow, glow * s * a), 2.0 * k);
                    }
                }
            }
            for &(r, radius) in boxes {
                self.fill_shape(r, radius, self.c(st.row_panel, (0.86 + 0.08 * s) * a));
                if show.glow {
                    self.fill_shape(r, radius, self.c(st.row_glow, 0.07 * s * a));
                }
                let mut y = r.top + 2.0 * k;
                while show.scanlines && y < r.bottom {
                    let inset = self.corner_inset(r, radius, y, k);
                    if inset * 2.0 < r.right - r.left {
                        self.fill(rect(r.left + inset, y, r.right - inset, y + k), self.c(st.row_scanline, 0.035 * a));
                    }
                    y += 3.0 * k;
                }
                self.frame(r, radius, border, 0.5 * k, border_w);
            }

            // Command lines and paths are shown as typed: case can matter there.
            let name = if matches!(item.tag, Tag::Shell | Tag::Open) { item.name.clone() } else { item.name.to_uppercase() };
            let (name_top, dy) = line_offsets(&show);
            if show.name {
                let name_color = if show.dim_unselected { mix(st.row_name, st.row_name_selected, s, ta) } else { self.c(st.row_name_selected, ta) };
                self.text(&name, &f.name, rect(panel.left + 14.0 * k, top + name_top * k, panel.right - 60.0 * k, top + (name_top + 23.0) * k), name_color);
            }
            let line = top + dy * k;

            let mut x = panel.left + 14.0 * k;
            if show.tags {
                let mut tags: Vec<(String, Paint)> = match item.tag {
                    Tag::App => vec![(t("tag.app"), st.tag_app)],
                    Tag::Pinned => vec![(t("tag.app"), st.tag_app), (t("tag.pinned"), st.tag_pinned)],
                    Tag::File => vec![(t("tag.file"), st.tag_file)],
                    Tag::Folder => vec![(t("tag.dir"), st.tag_folder)],
                    Tag::Shell => vec![(t("tag.cmd"), st.tag_cmd)],
                    Tag::Calc => vec![(t("tag.calc"), st.tag_calc)],
                    Tag::Web => vec![(t("tag.web"), st.tag_web)],
                    Tag::Open => vec![(t("tag.open"), st.tag_open)],
                    Tag::Setting => vec![(t("tag.set"), st.tag_setting)],
                    Tag::Action => vec![(t("tag.exec"), st.tag_exec)],
                    Tag::Plugin(_) => vec![(item.extra.as_ref().map_or_else(|| t("tag.exec"), |e| e.tag.clone()), st.tag_plugin)],
                };
                if recent {
                    tags.push((t("tag.recent"), st.tag_exec));
                }
                let pad = st.shape.tag_padding * k;
                // Grows from the middle of the line the chips sit on.
                let half = 7.0 * k * st.shape.tag_height / 100.0;
                let labels: Vec<&str> = tags.iter().map(|(label, _)| label.as_str()).collect();
                let spans = self.chip_spans(&labels, x);
                for ((label, paint), &(left, right)) in tags.iter().zip(&spans) {
                    let tag = rect(left, line + 40.0 * k - half, right, line + 40.0 * k + half);
                    let radius = st.shape.tag_radius * k;
                    self.fill_shape(tag, radius, self.c(*paint, 0.12 * a));
                    self.frame(tag, radius, self.c(*paint, 0.8 * a), 0.5 * k, k * st.shape.border_width);
                    self.text(label, &f.tag, rect(tag.left + pad, line + 34.5 * k, tag.right, tag.bottom), self.c(*paint, ta));
                }
                x = spans.last().map_or(x, |&(_, right)| right + st.shape.tag_gap * k) + 2.0 * k;
            }
            let detail = match item.tag {
                Tag::File | Tag::Folder => item.id.rsplit_once('\\').map_or("",|(dir, _)| dir).to_owned(),
                Tag::Shell => t("row.detail.shell"),
                Tag::Calc => t("row.detail.calc"),
                Tag::Web if item.id.is_empty() => t("row.detail.web"),
                Tag::Web => item.id.clone(),
                Tag::Plugin(_) => item.extra.as_ref().map_or_else(String::new, |e| e.item.subtitle.clone()),
                _ => String::new(),
            };
            if show.detail && !detail.is_empty() {
                let alpha = if show.dim_unselected { self.hc(0.55) } else { 1.0 };
                self.text(&detail, &f.detail, rect(x, line + 33.0 * k, panel.right - 10.0 * k, line + 48.0 * k), self.c(st.row_detail, alpha * ta));
            }

            if show.hint && s > 0.5 {
                self.text(&t("row.hint"), &f.row_hint, rect(panel.left, top + 19.0 * k, panel.right - 12.0 * k, top + 36.0 * k), self.c(st.row_hint, (s - 0.5) * 2.0 * ta));
            }

            let drawn = !l.icons || item.icon.as_ref().is_some_and(|i| self.icon(index, i, icon, s, a));
            if !drawn {
                let label: String = match item.tag {
                    Tag::App | Tag::Pinned => name.chars().filter(|c| c.is_alphanumeric()).take(2).collect(),
                    Tag::File => name.rsplit_once('.').map_or(t("tag.file"), |(_, ext)| ext.to_owned()).chars().take(3).collect(),
                    Tag::Folder => t("tag.dir").chars().take(3).collect(),
                    Tag::Shell => ">_".into(),
                    Tag::Calc => "=".into(),
                    Tag::Web => "WWW".into(),
                    Tag::Open => "->".into(),
                    Tag::Setting => t("tag.set").chars().take(3).collect(),
                    Tag::Action => "\u{BB}".into(),
                    Tag::Plugin(_) => item.extra.as_ref().map_or_else(String::new, |e| e.tag.chars().take(3).collect()),
                };
                let paint = match item.tag {
                    Tag::File => st.tag_file,
                    Tag::Folder => st.tag_folder,
                    Tag::Shell => st.tag_cmd,
                    Tag::Calc => st.tag_calc,
                    Tag::Web => st.tag_web,
                    Tag::Open => st.tag_open,
                    Tag::Setting => st.tag_setting,
                    Tag::Plugin(_) => st.tag_plugin,
                    Tag::App | Tag::Pinned | Tag::Action => st.row_glyph,
                };
                let alpha = if show.dim_unselected { self.hc(0.45 + 0.55 * s) } else { 1.0 };
                self.text(&label, &f.glyph, icon, self.c(paint, alpha * ta));
            }
        }
    }
}

/// Where a row's two lines sit: the name's top and how far the second line (tags, path) moves.
/// With either line hidden, the other one sits in the middle of the row.
pub(super) fn line_offsets(show: &Show) -> (f32, f32) {
    let name_top = if show.tags || show.detail { 5.0 } else { 17.0 };
    (name_top, if show.name { 0.0 } else { -11.5 })
}
