use super::*;

/// A text format and how far down its text has to move to sit where Bahnschrift's would (the
/// offsets in the painter assume its metrics; another family, size or weight has other ones).
pub(super) struct Face {
    pub(super) format: IDWriteTextFormat,
    pub(super) dy: f32,
    /// Laid-out strings, kept between frames. Dropped with the face when the fonts change, and by
    /// `Renderer::release` while the menu is hidden.
    pub(super) layouts: RefCell<HashMap<u64, Laid>>,
}

/// A string laid out in a face, and the box it was last given.
pub(super) struct Laid {
    text: String,
    layout: IDWriteTextLayout,
    size: (f32, f32),
}

/// Strings kept per face before the cache starts over.
const LAYOUT_CAP: usize = 512;

/// What a cached layout is used for: the same string at another size must not share one that is
/// being read back for hit testing.
#[derive(Clone, Copy, Hash)]
pub(super) enum Use {
    Draw,
    HitTest,
    Measure,
}

#[cfg(test)]
thread_local! {
    /// Draw every string with `DrawText`, as before layouts were kept (the tests compare the two).
    pub(super) static FRESH_TEXT: Cell<bool> = const { Cell::new(false) };
}

impl Face {
    pub(super) fn new(format: IDWriteTextFormat, dy: f32) -> Self {
        Self { format, dy, layouts: RefCell::default() }
    }

    pub(super) fn forget_layouts(&self) {
        self.layouts.borrow_mut().clear();
    }

    /// `s` laid out in this face in a box of `size`: the one from an earlier frame, resized, or a new one.
    pub(super) fn layout(&self, dwrite: &IDWriteFactory, purpose: Use, s: &str, size: (f32, f32)) -> Option<IDWriteTextLayout> {
        let fresh = || {
            let units: Vec<u16> = s.encode_utf16().collect();
            unsafe { dwrite.CreateTextLayout(&units, &self.format, size.0, size.1) }.ok()
        };
        let key = {
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&(purpose, s), &mut hasher);
            std::hash::Hasher::finish(&hasher)
        };
        let mut layouts = self.layouts.borrow_mut();
        if let Some(laid) = layouts.get_mut(&key).filter(|laid| laid.text == s) {
            if laid.size != size {
                let resized = unsafe { laid.layout.SetMaxWidth(size.0).and_then(|()| laid.layout.SetMaxHeight(size.1)) };
                if resized.is_err() {
                    layouts.remove(&key);
                    return fresh();
                }
                laid.size = size;
            }
            return Some(laid.layout.clone());
        }
        let layout = fresh()?;
        if layouts.len() >= LAYOUT_CAP {
            layouts.clear();
        }
        layouts.insert(key, Laid { text: s.to_owned(), layout: layout.clone(), size });
        Some(layout)
    }
}

impl std::ops::Deref for Face {
    type Target = IDWriteTextFormat;

    fn deref(&self) -> &IDWriteTextFormat {
        &self.format
    }
}

/// How a `Face` is aligned in its rectangle.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Align {
    /// Left, at the top.
    Left,
    /// Right, at the top.
    Right,
    /// Left, centred vertically.
    Middle,
    /// Centred both ways.
    Centre,
}

/// Header, hint, caption and button fonts follow the text scale (DPI times the GUI scale
/// setting); name, tag, row hint, detail and glyph follow the row scale. The `[fonts]` role of each
/// is in the comment; `size` scales the font alone, family and weight replace the built-in ones.
pub(super) struct Formats {
    /// Role Caption.
    pub(super) caption: Face,
    /// Role Header: the title line.
    pub(super) header: Face,
    /// Role Query: what is typed, and the row name on the actions page.
    pub(super) query: Face,
    /// Role Name.
    pub(super) name: Face,
    /// Role Tag.
    pub(super) tag: Face,
    /// Role Hint.
    pub(super) hint: Face,
    /// Role Hint: a row's ENTER mark.
    pub(super) row_hint: Face,
    /// Role Detail.
    pub(super) detail: Face,
    /// Role Name: the letters in an icon box.
    pub(super) glyph: Face,
    /// Role Button: power bar, tabs, panel buttons, arrows.
    pub(super) button: Face,
    /// Settings panel, role Label: left-aligned row labels, popup and pop-out entries.
    pub(super) label: Face,
    /// Settings panel, role Value: centred values.
    pub(super) value: Face,
    /// Power dialog, role DialogTitle: the action name.
    pub(super) dialog_title: Face,
    /// Power dialog, role DialogText: the caption above the action name.
    pub(super) dialog_caption: Face,
    /// Power dialog, role DialogText: the CONFIRM? line.
    pub(super) dialog_prompt: Face,
}

impl Formats {
    /// Frees the laid-out strings of every face (the menu is hidden).
    pub(super) fn forget_layouts(&self) {
        let faces = [
            &self.caption, &self.header, &self.query, &self.name, &self.tag, &self.hint, &self.row_hint, &self.detail, &self.glyph, &self.button, &self.label, &self.value,
            &self.dialog_title, &self.dialog_caption, &self.dialog_prompt,
        ];
        faces.iter().for_each(|face| face.forget_layouts());
    }

    pub(super) fn new(dwrite: &IDWriteFactory, scale: f32, k: f32, fonts: &Fonts) -> Result<Self> {
        unsafe {
            let make = |family: &str, weight: Weight, size: f32| {
                // Bahnschrift's semi-condensed face is what the design was made with; other families have no such width.
                let stretch = if family.eq_ignore_ascii_case(DEFAULT_FAMILY) { DWRITE_FONT_STRETCH_SEMI_CONDENSED } else { DWRITE_FONT_STRETCH_NORMAL };
                dwrite
                    .CreateTextFormat(&HSTRING::from(family), None, DWRITE_FONT_WEIGHT(weight.value()), DWRITE_FONT_STYLE_NORMAL, stretch, size, w!("en-us"))
                    .and_then(|f| {
                        // Every label is one line; long names and paths are clipped, not wrapped.
                        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                        Ok(f)
                    })
            };
            let face = |role: Role, base_size: f32, base_weight: Weight, align: Align, ellipsis: bool| -> Result<Face> {
                let font = fonts.role(role);
                let family = fontlist::resolve(dwrite, fonts.family_of(role).as_str());
                let weight = font.weight.unwrap_or(base_weight);
                let size = base_size * font.size as f32 / 100.0;
                let format = make(&family, weight, size)?;
                match align {
                    Align::Left => {}
                    Align::Right => format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?,
                    Align::Middle => format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?,
                    Align::Centre => {
                        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                    }
                }
                // Translations run longer than the English the boxes were sized for: texts that
                // don't fit end in an ellipsis instead of being cut mid-letter.
                if ellipsis {
                    let sign = dwrite.CreateEllipsisTrimmingSign(&format)?;
                    let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                    format.SetTrimming(&trimming, &sign)?;
                }
                // Where the ink sits in the box, against Bahnschrift at the built-in size and weight.
                let stock = family.eq_ignore_ascii_case(DEFAULT_FAMILY) && font.size == 100 && weight == base_weight;
                let dy = if stock {
                    0.0
                } else {
                    let centred = matches!(align, Align::Middle | Align::Centre);
                    let ink = |l: fontlist::Line| l.ink_centre() - if centred { l.height / 2.0 } else { 0.0 };
                    let reference = make(DEFAULT_FAMILY, base_weight, base_size)?;
                    let now = fontlist::line(dwrite, &format, &family, weight.value(), size);
                    let then = fontlist::line(dwrite, &reference, DEFAULT_FAMILY, base_weight.value(), base_size);
                    now.zip(then).map_or(0.0, |(now, then)| ink(then) - ink(now))
                };
                Ok(Face::new(format, dy))
            };
            Ok(Self {
                caption: face(Role::Caption, 10.0 * scale, Weight::Normal, Align::Left, true)?,
                header: face(Role::Header, 16.0 * scale, Weight::Bold, Align::Left, true)?,
                query: face(Role::Query, 16.0 * scale, Weight::Bold, Align::Left, false)?,
                name: face(Role::Name, 17.0 * k, Weight::Semibold, Align::Left, true)?,
                tag: face(Role::Tag, 9.0 * k, Weight::Semibold, Align::Left, false)?,
                hint: face(Role::Hint, 11.0 * scale, Weight::Semibold, Align::Right, true)?,
                row_hint: face(Role::Hint, 11.0 * k, Weight::Semibold, Align::Right, false)?,
                detail: face(Role::Detail, 10.0 * k, Weight::Normal, Align::Left, true)?,
                glyph: face(Role::Name, 18.0 * k, Weight::Bold, Align::Centre, false)?,
                button: face(Role::Button, 11.0 * scale, Weight::Semibold, Align::Centre, true)?,
                label: face(Role::Label, 11.0 * scale, Weight::Semibold, Align::Middle, true)?,
                value: face(Role::Value, 14.0 * scale, Weight::Bold, Align::Centre, true)?,
                dialog_title: face(Role::DialogTitle, 52.0 * scale, Weight::Bold, Align::Centre, false)?,
                dialog_caption: face(Role::DialogText, 13.0 * scale, Weight::Normal, Align::Centre, false)?,
                dialog_prompt: face(Role::DialogText, 20.0 * scale, Weight::Semibold, Align::Centre, false)?,
            })
        }
    }
}

// Every plain rectangle at least 5 px both ways that the painter drew, with the line that asked
// for it: the shape tests check that boxes go through the shaped path.
#[cfg(test)]
thread_local! {
    pub(super) static PLAIN_BOXES: RefCell<Vec<&'static std::panic::Location<'static>>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
#[track_caller]
pub(super) fn record_box(r: D2D_RECT_F) {
    if r.right - r.left > 4.0 && r.bottom - r.top > 4.0 {
        let at = std::panic::Location::caller();
        PLAIN_BOXES.with(|boxes| boxes.borrow_mut().push(at));
    }
}

// How many rectangles, outlines and strings were drawn: the tests check that a hidden element is skipped, not drawn invisibly.
#[cfg(test)]
thread_local! {
    pub(super) static DRAW_CALLS: Cell<u32> = const { Cell::new(0) };
}

#[cfg(test)]
pub(super) fn count_draw() {
    DRAW_CALLS.with(|c| c.set(c.get() + 1));
}

pub(super) struct Painter<'a> {
    pub(super) factory: &'a ID2D1Factory,
    pub(super) dwrite: &'a IDWriteFactory,
    pub(super) device: &'a Device,
    pub(super) rt: &'a ID2D1DCRenderTarget,
    pub(super) brush: &'a ID2D1SolidColorBrush,
    pub(super) formats: &'a Formats,
    pub(super) l: &'a Layout,
    pub(super) s: &'a Style,
    pub(super) hover: Option<Hover>,
}

impl Painter<'_> {
    /// `paint` at the element's own opacity `a`, scaled by the style's opacity factor.
    pub(super) fn c(&self, paint: Paint, a: f32) -> D2D1_COLOR_F {
        rgba(paint.rgb, a * paint.alpha)
    }

    #[cfg_attr(test, track_caller)]
    pub(super) unsafe fn fill(&self, r: D2D_RECT_F, color: D2D1_COLOR_F) {
        #[cfg(test)]
        {
            record_box(r);
            count_draw();
        }
        unsafe {
            self.brush.SetColor(&color);
            self.rt.FillRectangle(&r, self.brush);
        }
    }

    #[cfg_attr(test, track_caller)]
    pub(super) unsafe fn stroke(&self, r: D2D_RECT_F, color: D2D1_COLOR_F, width: f32) {
        #[cfg(test)]
        {
            record_box(r);
            count_draw();
        }
        unsafe {
            self.brush.SetColor(&color);
            self.rt.DrawRectangle(&r, self.brush, width, None);
        }
    }

    /// The corner size to use after growing a shape by `by` px (negative: shrinking): round corners
    /// stay concentric, a cut keeps its diagonal parallel. A square shape stays square.
    pub(super) fn corner_after(&self, radius: f32, by: f32) -> f32 {
        if radius <= 0.0 {
            return 0.0;
        }
        let factor = if self.s.shape.corner == Corner::Chamfer { 2.0 - std::f32::consts::SQRT_2 } else { 1.0 };
        (radius + by * factor).max(0.0)
    }

    /// Corner of a highlight inset `inset` px inside a parent with corner `radius`: concentric, but never
    /// below half the parent's, so small radii don't leave the highlight looking square.
    pub(super) fn inner_corner(&self, radius: f32, inset: f32) -> f32 {
        self.corner_after(radius, -inset).max(radius * 0.5)
    }

    /// The octagon-like outline of `r` with its corners cut `c` px.
    pub(super) unsafe fn chamfered(&self, r: D2D_RECT_F, c: f32) -> Option<ID2D1PathGeometry> {
        let p = Vector2::new;
        unsafe {
            let geometry = self.factory.CreatePathGeometry().ok()?;
            let sink = geometry.Open().ok()?;
            sink.BeginFigure(p(r.left + c, r.top), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLines(&[
                p(r.right - c, r.top),
                p(r.right, r.top + c),
                p(r.right, r.bottom - c),
                p(r.right - c, r.bottom),
                p(r.left + c, r.bottom),
                p(r.left, r.bottom - c),
                p(r.left, r.top + c),
            ]);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            Some(geometry)
        }
    }

    /// How far in from the left and right edge of a scanline `y..y+h` the outline of `r` (corners
    /// of size `radius`) is, so a thin line can stop at the shape instead of poking out of a corner.
    pub(super) fn corner_inset(&self, r: D2D_RECT_F, radius: f32, y: f32, h: f32) -> f32 {
        let radius = radius.min((r.right - r.left).min(r.bottom - r.top) / 2.0);
        if radius < 0.5 {
            return 0.0;
        }
        let dy = (y - r.top).min(r.bottom - (y + h)).max(0.0);
        if dy >= radius {
            return 0.0;
        }
        match self.s.shape.corner {
            Corner::Round => radius - (radius * radius - (radius - dy).powi(2)).max(0.0).sqrt(),
            Corner::Chamfer => radius - dy,
        }
    }

    /// The x range of a strip along the top edge of `r` that stays clear of its rounded corners.
    pub(super) fn top_strip(&self, r: D2D_RECT_F, radius: f32) -> (f32, f32) {
        let inset = radius.min((r.right - r.left) / 2.0);
        (r.left + inset, r.right - inset)
    }

    /// `fill` with rounded or cut corners of size `radius` (0 = square).
    #[cfg_attr(test, track_caller)]
    pub(super) unsafe fn fill_shape(&self, r: D2D_RECT_F, radius: f32, color: D2D1_COLOR_F) {
        let radius = radius.min((r.right - r.left).min(r.bottom - r.top) / 2.0);
        if radius < 0.5 {
            return unsafe { self.fill(r, color) };
        }
        #[cfg(test)]
        count_draw();
        unsafe {
            self.brush.SetColor(&color);
            match self.s.shape.corner {
                Corner::Round => self.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, self.brush),
                Corner::Chamfer => {
                    if let Some(outline) = self.chamfered(r, radius) {
                        self.rt.FillGeometry(&outline, self.brush, None);
                    }
                }
            }
        }
    }

    /// `stroke` with rounded or cut corners of size `radius`; the line is centred on `r`.
    #[cfg_attr(test, track_caller)]
    pub(super) unsafe fn stroke_shape(&self, r: D2D_RECT_F, radius: f32, color: D2D1_COLOR_F, width: f32) {
        if width <= 0.0 {
            return;
        }
        let radius = radius.min((r.right - r.left).min(r.bottom - r.top) / 2.0);
        if radius < 0.5 {
            return unsafe { self.stroke(r, color, width) };
        }
        #[cfg(test)]
        count_draw();
        unsafe {
            self.brush.SetColor(&color);
            match self.s.shape.corner {
                Corner::Round => self.rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, self.brush, width, None),
                Corner::Chamfer => {
                    if let Some(outline) = self.chamfered(r, radius) {
                        self.rt.DrawGeometry(&outline, self.brush, width, None);
                    }
                }
            }
        }
    }

    /// A border just inside `r`: `width` wide (already scaled by the shape's border width), its
    /// centre line `inset` px in.
    #[cfg_attr(test, track_caller)]
    pub(super) unsafe fn frame(&self, r: D2D_RECT_F, radius: f32, color: D2D1_COLOR_F, inset: f32, width: f32) {
        unsafe { self.stroke_shape(grow(r, -inset), self.corner_after(radius, -inset), color, width) }
    }

    /// A glow ring `by` px outside `r`.
    #[cfg_attr(test, track_caller)]
    pub(super) unsafe fn halo(&self, r: D2D_RECT_F, radius: f32, by: f32, color: D2D1_COLOR_F, width: f32) {
        unsafe { self.stroke_shape(grow(r, by), self.corner_after(radius, by), color, width) }
    }

    pub(super) unsafe fn text(&self, s: &str, face: &Face, r: D2D_RECT_F, color: D2D1_COLOR_F) {
        let r = rect(r.left, r.top + face.dy, r.right, r.bottom + face.dy);
        #[cfg(test)]
        let kept = !FRESH_TEXT.get();
        #[cfg(not(test))]
        let kept = true;
        #[cfg(test)]
        count_draw();
        unsafe {
            self.brush.SetColor(&color);
            // What `DrawText` does, minus laying the string out again every frame.
            let layout = if kept { face.layout(self.dwrite, Use::Draw, s, (r.right - r.left, r.bottom - r.top)) } else { None };
            match layout {
                Some(layout) => self.rt.DrawTextLayout(Vector2::new(r.left, r.top), &layout, self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP),
                None => {
                    let s: Vec<u16> = s.encode_utf16().collect();
                    self.rt.DrawText(&s, &face.format, &r, self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                }
            }
        }
    }

    /// Text with a dark drop shadow so it stays readable over bright wallpapers.
    pub(super) unsafe fn shadowed(&self, s: &str, format: &Face, r: D2D_RECT_F, color: D2D1_COLOR_F) {
        let d = self.l.scale;
        unsafe {
            let offset = rect(r.left + d, r.top + d, r.right + d, r.bottom + d);
            self.text(s, format, offset, rgba(0x000000, 0.85 * color.a));
            self.text(s, format, r, color);
        }
    }

    pub(super) fn hovered(&self, target: Hover) -> bool {
        self.hover == Some(target)
    }

    /// Opacity of faded text: raised to at least 0.9 with high contrast.
    pub(super) fn hc(&self, alpha: f32) -> f32 {
        lifted(self.l.contrast, alpha)
    }

    pub(super) fn row_text_alpha(&self, a: f32) -> f32 {
        row_text_lifted(self.l.contrast, a)
    }
}
