use super::*;
use super::shapes::hash_pixels;
use crate::style::{SHOW_ELEMENTS, SHOW_HEADER, SHOW_KEYS, Shape, Show, TextAlign};

impl Fixture {
    /// The `[show]` settings, for the style and the layout alike (call before `panel`).
    fn show(mut self, show: Show) -> Self {
        self.style.show = show;
        self.l = Layout::new(525, 1010, 1.25, &LayoutConfig { shape: self.style.shape, show, ..Default::default() });
        self
    }
}

fn with(key: &str, on: bool) -> Show {
    let mut show = Show::default();
    match show.flag(key) {
        Some(_) => match key {
            "scanlines" => show.scanlines = on,
            "glow" => show.glow = on,
            "name" => show.name = on,
            "tags" => show.tags = on,
            "detail" => show.detail = on,
            "hint" => show.hint = on,
            "power_bar" => show.power_bar = on,
            "count" => show.count = on,
            "progress" => show.progress = on,
            "caption" => show.caption = on,
            "subtitle" => show.subtitle = on,
            "header_line" => show.header_line = on,
            "gear" => show.gear = on,
            "open_glitch" => show.open_glitch = on,
            "row_cascade" => show.row_cascade = on,
            "header_backdrop" => show.header_backdrop = on,
            "color_icons" => show.color_icons = on,
            "dim_unselected" => show.dim_unselected = on,
            "edge_fade" => show.edge_fade = on,
            _ => unreachable!("{key}"),
        },
        None => unreachable!("{key}"),
    }
    show
}

/// Apps and a file, scenario 0: a typed query (count, tags, path line); 1: the menu just opened.
fn busy(show: Show, scenario: usize) -> Fixture {
    let mut f = Fixture::new(Shape::default()).show(show);
    let mut items: Vec<Item> = (0..6).map(|i| Item { name: format!("App {i}"), tag: Tag::App, id: format!("id{i}"), icon: None, extra: None }).collect();
    items.push(Item { name: "App file".into(), tag: Tag::File, id: "C:\\dir\\app.txt".into(), icon: None, extra: None });
    f.menu.set_apps(items, &[]);
    if scenario == 0 {
        f.menu.set_query("app");
    } else {
        f.open_ms = 90.0;
    }
    f
}

fn calls(f: &Fixture) -> u32 {
    DRAW_CALLS.with(|c| c.set(0));
    let _ = f.render();
    DRAW_CALLS.with(Cell::get)
}

#[test]
fn every_element_toggle_skips_its_drawing_when_off() {
    for key in &SHOW_KEYS[SHOW_HEADER.start..SHOW_ELEMENTS.end] {
        // The cascade hides rows that haven't arrived yet, so off it draws more: it just has to differ.
        let changed = |scenario| match *key {
            "row_cascade" => hash_pixels(&busy(with(key, false), scenario).render()) != hash_pixels(&busy(Show::default(), scenario).render()),
            _ => calls(&busy(with(key, false), scenario)) < calls(&busy(Show::default(), scenario)),
        };
        assert!((0..2).any(changed), "{key} off still draws the same");
    }
}

#[test]
fn defaults_draw_what_they_always_did() {
    // The pixel-hash test covers the rest; this one is about the layout.
    let l = Layout::new(525, 1010, 1.25, &LayoutConfig::default());
    assert_eq!(l.show, Show::default());
    assert_eq!((l.head_trim, l.footer_top), (0.0, 1010.0 - 52.0 * l.ts));
}

#[test]
fn the_header_line_and_the_gear_leave_no_pixels() {
    let f = busy(Show { header_line: false, gear: false, progress: false, ..Show::default() }, 0);
    let r = f.render();
    let (s, w) = (f.l.ts, f.l.w as i32);
    let right = w - f.l.margin as i32 - 4;
    assert_eq!(pixel(&r, right, (24.5 * s) as i32), pixel(&r, right, (28.0 * s) as i32), "no line");
    let gear = f.l.gear_rect();
    let mut seen = std::collections::HashSet::new();
    for y in gear.top as i32..gear.bottom as i32 {
        for x in gear.left as i32..gear.right as i32 {
            seen.insert(pixel(&r, x, y));
        }
    }
    assert_eq!(seen.len(), 1, "the gear's box is flat");
    assert!(!f.l.gear_hit(f.l.menu_x() + gear.left + 2.0, gear.top + 2.0), "and can't be clicked");
    let on = busy(Show::default(), 0).render();
    assert_ne!(pixel(&on, right, (24.5 * s) as i32), pixel(&on, right, (28.0 * s) as i32), "the default has the line");
}

#[test]
fn without_the_top_strip_the_header_shrinks_and_the_list_grows() {
    let full = Layout::new(525, 1010, 1.25, &LayoutConfig::default());
    let some = Layout::new(525, 1010, 1.25, &LayoutConfig { show: Show { caption: false, header_line: false, progress: false, ..Show::default() }, ..Default::default() });
    assert_eq!(some.list_top, full.list_top, "the gear is still there");
    let bare = Layout::new(525, 1010, 1.25, &LayoutConfig { show: Show { caption: false, header_line: false, progress: false, gear: false, ..Show::default() }, ..Default::default() });
    assert!(bare.list_top < full.list_top && bare.head_trim > 0.0);
    assert!(bare.query_hit(bare.query_left() + 10.0, 40.0 * bare.ts - bare.head_trim));
    assert!(!bare.query_hit(bare.query_left() + 10.0, 40.0 * bare.ts + 1.0), "the line moved up");
}

#[test]
fn a_hidden_power_bar_gives_the_list_its_room_and_cannot_be_reached() {
    let on = Layout::new(525, 1010, 1.25, &LayoutConfig::default());
    let off = Layout::new(525, 1010, 1.25, &LayoutConfig { show: with("power_bar", false), ..Default::default() });
    assert!(off.list_bottom > on.list_bottom && !off.has_power_bar() && on.has_power_bar());
    let r = on.button_rect(1);
    let (x, y) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
    assert_eq!(on.button_at(x, y), Some(1));
    assert_eq!(off.button_at(x, y), None);
}

#[test]
fn without_edge_fade_rows_keep_full_opacity() {
    let on = Layout::new(525, 1010, 1.25, &LayoutConfig::default());
    let off = Layout::new(525, 1010, 1.25, &LayoutConfig { show: with("edge_fade", false), ..Default::default() });
    // A long list scrolled to the middle: the first visible row is cut at the top.
    let (scroll, count) = (10.0, 40);
    let cut: Vec<f32> = (0..count).map(|r| on.row_top(r as f32, scroll, count)).filter(|&top| on.position_alpha(top, scroll, count) < 1.0).collect();
    assert!(!cut.is_empty(), "some row fades");
    assert!(cut.iter().all(|&top| off.position_alpha(top, scroll, count) == 1.0));
    // A click on a faded row's visible part still finds it.
    let x = off.menu_x() + off.margin + 10.0;
    assert!(cut.iter().any(|&top| {
        let y = (top + off.row_h / 2.0).clamp(off.list_top + 1.0, off.list_bottom - 1.0);
        off.row_at(x, y, scroll, count).is_some()
    }));
}

#[test]
fn the_dimming_toggles_change_how_unselected_rows_look() {
    let hash = |show: Show| hash_pixels(&busy(show, 0).render());
    let base = hash(Show::default());
    assert_ne!(base, hash(with("dim_unselected", false)));
    // Colour icons: an item with an icon of its own colour.
    let icon = |on: bool| {
        let mut f = busy(with("color_icons", on), 0);
        let px: Vec<u8> = std::iter::repeat([0u8, 0, 255, 255]).take(ICON * ICON).flatten().collect();
        f.menu.items[0].icon = Some(Icon { color: px });
        f.menu.set_query("");
        f.menu.selected = 3;
        f.menu.scroll = 3.0;
        f
    };
    let (plain, colour) = (icon(false), icon(true));
    let at = |f: &Fixture| {
        let r = f.render();
        // Row 0, unselected: its icon box is at the right end of the row.
        let top = f.l.row_top(0.0, 3.0, f.menu.view.len());
        let (_, il) = f.l.row_x(3.0);
        let k = f.l.k;
        pixel(&r, (il + f.l.row_h / 2.0) as i32, (top + f.l.row_h / 2.0 + 2.0 * k) as i32)
    };
    let (a, b) = (at(&plain), at(&colour));
    let red = |p: u32| (p >> 16) & 0xFF;
    let blue = |p: u32| p & 0xFF;
    assert!(red(b) > blue(b), "the original red icon: {b:#x}");
    assert_ne!(a, b);
}

#[test]
fn the_title_line_never_overlaps_for_any_alignment() {
    for avail in [300.0f32, 480.0] {
        for (title_w, count_w) in [(10.0, 40.0), (120.0, 80.0), (200.0, 60.0), (avail - 80.0 - 8.0, 80.0), (60.0, 0.0)] {
            for title in TextAlign::ALL {
                for count in TextAlign::ALL {
                    let (tx, cx) = header::line_slots(avail, title_w, count_w, 8.0, title, count);
                    assert!(tx >= -0.01 && tx + title_w <= avail + 0.01, "{title:?}/{count:?} title out: {tx}");
                    if count_w > 0.0 {
                        assert!(cx >= -0.01 && cx + count_w <= avail + 0.01, "{title:?}/{count:?} count out: {cx}");
                        let apart = if tx < cx { cx - (tx + title_w) } else { tx - (cx + count_w) };
                        assert!(apart >= 8.0 - 0.01, "{title:?}/{count:?} {title_w}+{count_w}: {apart}");
                    }
                }
            }
        }
    }
    // The default is the old layout: title at the left edge, count flush right.
    assert_eq!(header::line_slots(400.0, 100.0, 50.0, 8.0, TextAlign::Left, TextAlign::Right), (0.0, 350.0));
}

#[test]
fn all_nine_alignments_draw_and_a_centred_query_keeps_the_mouse_in_step() {
    for title in TextAlign::ALL {
        for count in TextAlign::ALL {
            for caption in TextAlign::ALL {
                let show = Show { title_align: title, count_align: count, caption_align: caption, ..Show::default() };
                let mut f = busy(show, 0);
                f.menu.set_query("app");
                f.menu.query.select_all();
                let r = f.render();
                let shift = f.menu.query_x.get();
                match title {
                    TextAlign::Left => assert_eq!(shift, 0.0, "{title:?}/{count:?}"),
                    _ => assert!(shift >= 0.0 && shift < f.l.w, "{title:?}/{count:?} {shift}"),
                }
                if title == TextAlign::Right {
                    assert!(shift > f.l.w / 3.0, "right aligned query starts at {shift}");
                }
                // The caret position under the mouse: a click at the start of the block is char 0.
                let start = f.l.query_left() + shift;
                assert_eq!(r.query_caret_at("app", 0.5, true, 0.0), 0);
                assert!(start > 0.0);
            }
        }
    }
}

#[test]
fn a_query_longer_than_the_line_still_scrolls_when_centred() {
    let show = Show { title_align: TextAlign::Center, ..Show::default() };
    let mut f = busy(show, 0);
    f.menu.set_query(&"w".repeat(80));
    let _ = f.render();
    assert!(f.menu.query_scroll.get() > 0.0);
    assert_eq!(f.menu.query_x.get(), 0.0, "a full line has no room to move");
}

#[test]
fn a_notice_shows_even_with_the_count_off() {
    let mut off = busy(with("count", false), 0);
    let mut on = busy(Show::default(), 0);
    let quiet = calls(&off);
    assert!(quiet < calls(&on));
    off.menu.notice = Some("PLUGIN ERROR".into());
    on.menu.notice = Some("PLUGIN ERROR".into());
    assert_eq!(calls(&off), calls(&on));
}

/// Ink columns (first, last) of `label` drawn alone in the tag face at x = 40.
fn ink_columns(label: &str) -> (f32, f32, f32) {
    let f = Fixture::new(Shape::default());
    let mut renderer = f.render();
    let (dwrite, factory) = (renderer.dwrite.clone(), renderer.factory.clone());
    let device = renderer.device.as_ref().unwrap();
    let painter = Painter { factory: &factory, dwrite: &dwrite, device, rt: &device.target, brush: &device.brush, formats: &renderer.formats, l: &f.l, s: &f.style, hover: None };
    let width = text_width(&dwrite, &renderer.formats.tag, label);
    let y = 100.0;
    unsafe {
        device.target.BeginDraw();
        device.target.Clear(Some(&rgba(0, 0.0)));
        painter.text(label, &renderer.formats.tag, rect(40.0, y, 40.0 + width + 4.0, y + 30.0), rgba(0xFFFFFF, 1.0));
        device.target.EndDraw(None, None).unwrap();
    }
    let (mut first, mut last) = (i32::MAX, i32::MIN);
    for row in y as i32..y as i32 + 30 {
        for x in 0..device.size.0 {
            if (pixel(&renderer, x, row) >> 24) > 24 {
                first = first.min(x);
                last = last.max(x);
            }
        }
    }
    renderer.release();
    (first as f32 - 40.0, 40.0 + width - (last + 1) as f32, width)
}

#[test]
fn tag_chips_hug_their_text_with_equal_padding_in_both_languages() {
    for code in ["en", "pl"] {
        lang::use_builtin(code);
        for id in ["app", "pinned", "file", "dir", "cmd", "calc", "web", "open", "set", "exec", "recent"] {
            let label = t(&format!("tag.{id}"));
            let (ink_left, ink_right, width) = ink_columns(&label);
            // The chip is the text's width plus the padding twice, so the padding on the two
            // sides differs only by the glyphs' own side bearings.
            assert!(width > 0.0);
            assert!((ink_left - ink_right).abs() <= 1.5, "{code} {label}: left {ink_left} right {ink_right}");
        }
    }
    lang::use_builtin("en");
}

#[test]
fn chips_are_text_plus_padding_wide_and_tag_gap_apart() {
    for code in ["en", "pl"] {
        lang::use_builtin(code);
        for (padding, gap) in [(5.0, 5.0), (2.0, 0.0), (9.0, 16.0)] {
            let shape = Shape { tag_padding: padding, tag_gap: gap, ..Shape::default() };
            let f = Fixture::new(shape);
            let renderer = f.render();
            let device = renderer.device.as_ref().unwrap();
            let painter = Painter { factory: &renderer.factory, dwrite: &renderer.dwrite, device, rt: &device.target, brush: &device.brush, formats: &renderer.formats, l: &f.l, s: &f.style, hover: None };
            let labels = [t("tag.app"), t("tag.pinned"), t("tag.recent")];
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let spans = painter.chip_spans(&labels, 20.0);
            let k = f.l.k;
            for (i, (label, &(left, right))) in labels.iter().zip(&spans).enumerate() {
                let text = text_width(&renderer.dwrite, &renderer.formats.tag, label);
                assert!((right - left - (text + 2.0 * padding * k)).abs() < 0.01, "{code} {label}");
                if i > 0 {
                    assert!((left - spans[i - 1].1 - gap * k).abs() < 0.01, "{code} gap {gap}");
                }
            }
        }
    }
    lang::use_builtin("en");
}

#[test]
fn tag_height_scales_around_the_same_middle() {
    let mut taller = busy(Show::default(), 0);
    taller.style.shape.tag_height = 150.0;
    let mut shorter = busy(Show::default(), 0);
    shorter.style.shape.tag_height = 60.0;
    assert_ne!(hash_pixels(&taller.render()), hash_pixels(&shorter.render()));
}

#[test]
fn a_hidden_name_centres_the_second_line_and_bare_icons_still_draw() {
    use crate::render::rows::line_offsets;
    assert_eq!(line_offsets(&Show::default()), (5.0, 0.0), "defaults stay where they were");
    // The name alone is centred on the row's middle; the chips alone must land on the same line.
    let (name_top, _) = line_offsets(&Show { tags: false, detail: false, ..Show::default() });
    let (_, dy) = line_offsets(&Show { name: false, ..Show::default() });
    assert_eq!(name_top + 23.0 / 2.0, 40.0 + dy);
    // Name, tags and path all off: each row is its icon (here the fallback letters) and nothing else.
    let bare = Show { name: false, tags: false, detail: false, ..Show::default() };
    let named = Show { tags: false, detail: false, ..Show::default() };
    let mut empty = busy(bare, 0);
    empty.menu.set_query("no such app");
    assert!(calls(&busy(bare, 0)) < calls(&busy(named, 0)));
    assert!(calls(&busy(bare, 0)) > calls(&empty), "the rows themselves are still drawn");
}
