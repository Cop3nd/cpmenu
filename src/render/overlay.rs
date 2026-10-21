use super::*;

/// Glitches the menu's pixels in the DIB between two D2D passes: a red/blue split of the whole
/// menu, then bands of rows thrown sideways. `origin` / `size` are the menu's rectangle on the
/// screen; the band is padded so shifted pixels have room.
pub(super) fn glitch_menu(device: &Device, origin: (f32, f32), size: (i32, i32), look: Look, frame_no: u32, scale: f32) {
    let (sw, sh) = (device.size.0, device.size.1);
    let pad = (100.0 * scale) as i32;
    let (ox, oy) = (origin.0 as i32, origin.1 as i32);
    let columns = (ox - pad).clamp(0, sw) as usize..(ox + size.0 + pad).clamp(0, sw) as usize;
    let rows = oy.clamp(0, sh) as usize..(oy + size.1).clamp(0, sh) as usize;
    if columns.is_empty() || rows.is_empty() {
        return;
    }
    // Pending drawing must be in the DIB before it is touched directly.
    unsafe {
        let _ = GdiFlush();
    }
    let pixels = unsafe { std::slice::from_raw_parts_mut(device.bits, sw as usize * sh as usize) };
    let stride = sw as usize;
    let mut scratch = Vec::new();
    let split = (look.glitch * 7.0 * scale).round() as usize;
    if split > 0 {
        ov::rgb_split(pixels, stride, rows.clone(), columns.start, columns.end, split, &mut scratch);
    }
    for slice in ov::slices(frame_no, look.glitch, rows.len(), scale) {
        let top = rows.start + slice.y;
        ov::shift_rows(pixels, stride, top..(top + slice.h).min(rows.end), columns.start, columns.end, slice.dx);
    }
}

impl Painter<'_> {
    /// Full-screen scanlines, a flash, and the flickering colored blocks.
    pub(super) unsafe fn effects(&self, look: Look, frame_no: u32, (w, h): (i32, i32)) {
        let (s, st) = (self.l.scale, self.s);
        let (w, h) = (w as f32, h as f32);
        let mut rng = Rng::new(frame_no, 4);
        unsafe {
            if look.glitch > 0.0 && rng.unit() < 0.25 * look.glitch {
                self.fill(rect(0.0, 0.0, w, h), rgba(0, 0.35));
            }
            if look.dim > 0.0 {
                let alpha = (0.04 + 0.06 * look.glitch) * look.dim;
                let mut y = 0.0;
                while y < h {
                    self.fill(rect(0.0, y, w, y + s.max(1.0)), self.c(st.overlay_glitch_a, alpha));
                    y += 3.0 * s;
                }
            }
            for block in ov::blocks(frame_no, look.glitch, w, h, s) {
                let paint = [st.overlay_glitch_a, st.overlay_glitch_b, st.overlay_glitch_c][block.color];
                self.fill(block.rect, self.c(paint, 0.3 + 0.3 * rng.unit()));
            }
        }
    }

    /// The centred confirm dialog.
    pub(super) unsafe fn dialog(&self, f: &PowerFrame, d: &Dialog) {
        let (fm, st, s) = (self.formats, self.s, self.l.ts);
        let a = f.look.dialog;
        let jitter = (Rng::new(f.frame_no, 5).unit() - 0.5) * 24.0 * s * f.look.glitch;
        let at = |r: D2D_RECT_F| rect(r.left + jitter, r.top, r.right + jitter, r.bottom);
        let card = at(d.card);
        let radius = st.shape.panel_radius * s;
        unsafe {
            self.fill_shape(grow(card, 3.0 * s), self.corner_after(radius, 3.0 * s), self.c(st.overlay_title, 0.07 * a));
            self.fill_shape(card, radius, self.c(st.overlay_dim, 0.95 * a));
            self.frame(card, radius, self.c(st.overlay_title, 0.7 * a), 0.5 * s, s * st.shape.border_width);
            // The strip stays between the rounded corners.
            let (strip_left, strip_right) = self.top_strip(card, radius);
            self.fill(rect(strip_left, card.top, strip_right, card.top + 6.0 * s), self.c(st.overlay_title, 0.95 * a));
            self.text(&t("dialog.caption"), &fm.dialog_caption, at(d.caption), self.c(st.overlay_caption, 0.9 * a));
            let title = at(d.title);
            let ghost = (2.0 + 4.0 * f.look.glitch) * s;
            let label = f.action.label();
            for (paint, dx) in [(st.overlay_glitch_a, -ghost), (st.overlay_glitch_b, ghost)] {
                self.text(&label, &fm.dialog_title, rect(title.left + dx, title.top, title.right + dx, title.bottom), self.c(paint, 0.5 * a));
            }
            self.text(&label, &fm.dialog_title, title, self.c(st.overlay_title, a));
            self.text(&t("dialog.prompt"), &fm.dialog_prompt, at(d.prompt), self.c(st.overlay_text, a));
            let (execute, abort) = (self.hovered(Hover::Execute), self.hovered(Hover::Abort));
            self.dialog_button(at(d.execute), &t("dialog.execute"), st.overlay_button_danger, execute, f.focus == ov::Focus::Execute, a);
            self.dialog_button(at(d.abort), &t("dialog.abort"), st.overlay_button, abort, f.focus == ov::Focus::Abort, a);
        }
    }

    /// Hover is a glow and a brighter fill; keyboard focus is a separate solid ring outside the
    /// button plus a full-strength border, so the two never look alike.
    pub(super) unsafe fn dialog_button(&self, r: D2D_RECT_F, label: &str, paint: Paint, hovered: bool, focused: bool, a: f32) {
        let (fm, st, s) = (self.formats, self.s, self.l.ts);
        let radius = st.shape.row_radius * s;
        unsafe {
            if focused {
                self.halo(r, radius, 5.0 * s, self.c(paint, a), 2.0 * s);
                self.fill_shape(r, radius, self.c(paint, 0.12 * a));
            }
            if hovered {
                for (by, glow) in [(4.0, 0.08), (2.0, 0.16)] {
                    self.halo(r, radius, by * s, self.c(paint, glow * a), 2.0 * s);
                }
            }
            self.fill_shape(r, radius, self.c(st.overlay_button_fill, 0.9 * a));
            if hovered {
                self.fill_shape(r, radius, self.c(st.overlay_button_hover, 0.18 * a));
            }
            let lit = hovered || focused;
            self.frame(r, radius, self.c(paint, if lit { 1.0 } else { 0.6 } * a), 0.5 * s, if lit { 1.5 * s } else { s } * st.shape.border_width);
            self.text(label, &fm.value, r, self.c(paint, if lit { 1.0 } else { 0.85 } * a));
        }
    }

    pub(super) unsafe fn power_bar(&self, power: &PowerBar, open_ms: f32) {
        let a = ease_out(open_ms / ROW_IN_MS);
        let (f, l, st) = (self.formats, self.l, self.s);
        let (s, m, w, top) = (l.ts, l.margin, l.w, l.footer_top);
        let (radius, bw) = (st.shape.row_radius * s, st.shape.border_width);
        unsafe {
            self.fill(rect(m, top, w - m, top + s), self.c(st.power_line, 0.6 * a));
            self.fill(rect(w - m - 52.0 * s, top - 2.0 * s, w - m, top + s), self.c(st.power_line, a));
            for (i, action) in ACTIONS.iter().enumerate() {
                let r = l.button_rect(i);
                let focused = power.focused == Some(i);
                let hovered = self.hovered(Hover::Power(i));
                let danger = action.is_dangerous();
                let button = if danger { st.power_button_danger } else { st.power_button };
                if focused {
                    for (by, glow) in [(4.0, 0.08), (2.0, 0.16)] {
                        self.halo(r, radius, by * s, self.c(button, glow * a), 2.0 * s);
                    }
                }
                self.fill_shape(r, radius, self.c(st.power_fill, 0.88 * a));
                // While the keyboard has the bar, a hovered button is only a hint next to the focus.
                let (hover_fill, hover_border, hover_text) = if power.focused.is_some() { (0.04, 0.6, 0.8) } else { (0.08, 0.8, 0.9) };
                if hovered && !focused {
                    self.fill_shape(r, radius, self.c(button, hover_fill * a));
                }
                let border = if focused { 1.0 } else if hovered { hover_border } else { 0.45 };
                self.frame(r, radius, self.c(button, border * a), 0.5 * s, if focused { 1.5 * s } else { s } * bw);
                let text = if danger { st.power_text_danger } else { st.power_text };
                let alpha = if focused { 1.0 } else if hovered { self.hc(hover_text) } else { self.hc(0.75) };
                self.text(&action.label(), &f.button, r, self.c(text, alpha * a));
            }
        }
    }
}
