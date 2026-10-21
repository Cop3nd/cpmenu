use super::*;

fn frame_pixels(renderer: &Renderer) -> (Vec<u32>, usize) {
    let device = renderer.device.as_ref().unwrap();
    let (w, h) = (device.size.0 as usize, device.size.1 as usize);
    (unsafe { std::slice::from_raw_parts(device.bits, w * h) }.to_vec(), w)
}

/// Where row `i` of a fixture's list is, in pixels: left, top, right of its text area (up to the icon box), bottom.
fn row_text_area(l: &Layout, i: usize, count: usize) -> (usize, usize, usize, usize) {
    let top = l.row_top(i as f32, 0.0, count);
    let (left, icon) = l.row_x(i as f32);
    (left.ceil() as usize, top.ceil() as usize, icon.floor() as usize, (top + l.row_h).floor() as usize)
}

fn name_fixture(fonts: crate::style::Fonts, names: bool) -> Fixture {
    let mut f = Fixture::new(crate::style::Shape::default());
    f.style.fonts = fonts;
    if !names {
        for item in &mut f.menu.items {
            item.name = " ".to_owned();
        }
    }
    f
}

#[test]
fn a_bigger_font_changes_the_text_and_leaves_every_box_alone() {
    use crate::style::{Fonts, Role};
    let mut big = Fonts::default();
    big.roles[crate::style::ROLES.iter().position(|&r| r == Role::Name).unwrap()].size = 160;
    let (plain, bigger) = (name_fixture(Fonts::default(), true), name_fixture(big, true));
    // The layout is made without the fonts: the boxes can't move.
    let cfg = LayoutConfig::default();
    assert_eq!(Layout::new(525, 1010, 1.25, &cfg).row_h, plain.l.row_h);
    let ((a, w), (b, _)) = (frame_pixels(&plain.render()), frame_pixels(&bigger.render()));
    let changed: Vec<(usize, usize)> = (0..a.len()).filter(|&i| a[i] != b[i]).map(|i| (i % w, i / w)).collect();
    assert!(changed.len() > 200, "the names changed ({} pixels)", changed.len());
    let l = &plain.l;
    let count = plain.menu.view.len();
    // Every changed pixel is inside a row (its name, and the letters that stand in for an icon), away from its border and glow.
    let areas: Vec<_> = (0..count).map(|i| (row_text_area(l, i, count), l.row_h as usize)).map(|((left, top, right, bottom), icon)| (left, top, right + icon, bottom)).collect();
    for &(x, y) in &changed {
        let inside = areas.iter().any(|&(left, top, right, bottom)| x >= left + 3 && x < right && y >= top + 3 && y + 3 < bottom);
        assert!(inside, "pixel ({x}, {y}) outside the rows' text areas");
    }
}

/// Vertical centre of the pixels the row names draw (rows 2 and 3), found by drawing with and without them.
fn name_ink_centre(fonts: crate::style::Fonts) -> f32 {
    let (with, w) = frame_pixels(&name_fixture(fonts, true).render());
    let (without, _) = frame_pixels(&name_fixture(fonts, false).render());
    let l = name_fixture(fonts, true).l;
    let count = 20;
    let mut ys = Vec::new();
    for row in [2, 3] {
        let (left, top, right, bottom) = row_text_area(&l, row, count);
        let rows: Vec<usize> = (top..bottom).filter(|&y| (left..right).any(|x| with[y * w + x] != without[y * w + x])).collect();
        assert!(!rows.is_empty(), "row {row} drew no name");
        let centre = (*rows.first().unwrap() + *rows.last().unwrap()) as f32 / 2.0 - top as f32;
        ys.push(centre);
    }
    ys.iter().sum::<f32>() / ys.len() as f32
}

#[test]
fn row_names_stay_vertically_centred_whatever_the_family_or_size() {
    use crate::style::{Family, Fonts, Role};
    let role = crate::style::ROLES.iter().position(|&r| r == Role::Name).unwrap();
    let stock = name_ink_centre(Fonts::default());
    let mut checked = Vec::new();
    for (family, size) in [("Segoe UI", 100), ("Segoe UI", 150), ("Consolas", 100), ("Bahnschrift", 150), ("Bahnschrift", 70), ("Times New Roman", 100)] {
        let mut fonts = Fonts::default();
        fonts.roles[role].family = Family::new(family);
        fonts.roles[role].size = size;
        let centre = name_ink_centre(fonts);
        checked.push((family, size, centre));
        assert!((centre - stock).abs() <= 2.0, "{family} {size}%: ink centre {centre} against Bahnschrift's {stock}");
    }
    // Without the correction the family change alone would show: Segoe UI's line box is taller.
    println!("stock {stock}, {checked:?}");
}

fn face_fields(f: &Formats) -> [&Face; 15] {
    [&f.caption, &f.header, &f.query, &f.name, &f.tag, &f.hint, &f.row_hint, &f.detail, &f.glyph, &f.button, &f.label, &f.value, &f.dialog_title, &f.dialog_caption, &f.dialog_prompt]
}

fn family_name(face: &Face) -> String {
    let mut buffer = vec![0u16; unsafe { face.GetFontFamilyNameLength() } as usize + 1];
    unsafe { face.GetFontFamilyName(&mut buffer) }.unwrap();
    String::from_utf16_lossy(&buffer[..buffer.len() - 1])
}

#[test]
fn names_and_the_header_end_in_an_ellipsis_instead_of_a_cut_letter() {
    let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.unwrap();
    let f = Formats::new(&dwrite, 1.0, 1.0, &crate::style::Fonts::default()).unwrap();
    for face in [&f.name, &f.header] {
        let (mut trimming, mut sign) = (DWRITE_TRIMMING::default(), None);
        unsafe { face.GetTrimming(&mut trimming, &mut sign) }.unwrap();
        assert_eq!(trimming.granularity, DWRITE_TRIMMING_GRANULARITY_CHARACTER);
        assert!(sign.is_some());
    }
}

#[test]
fn every_text_format_follows_exactly_one_font_role() {
    use crate::style::{Family, Fonts, Role::*};
    let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.unwrap();
    let build = |fonts: &Fonts| Formats::new(&dwrite, 1.0, 1.0, fonts).unwrap();
    let stock = build(&Fonts::default());
    assert!(face_fields(&stock).iter().all(|f| f.dy == 0.0), "Bahnschrift at the built-in size and weight needs no correction");
    let expected = [Caption, Header, Query, Name, Tag, Hint, Hint, Detail, Name, Button, Label, Value, DialogTitle, DialogText, DialogText];
    let sizes = |f: &Formats| face_fields(f).map(|f| unsafe { f.GetFontSize() });
    let weights = |f: &Formats| face_fields(f).map(|f| unsafe { f.GetFontWeight() }.0);
    let families = |f: &Formats| face_fields(f).map(family_name);
    for (what, change) in [
        ("size", (|fonts: &mut Fonts, i: usize| fonts.roles[i].size = 200) as fn(&mut Fonts, usize)),
        ("weight", |fonts, i| fonts.roles[i].weight = Some(crate::style::Weight::Light)),
        ("family", |fonts, i| fonts.roles[i].family = Family::new("Segoe UI")),
    ] {
        let mut owner: [Option<crate::style::Role>; 15] = [None; 15];
        for (i, role) in crate::style::ROLES.iter().enumerate() {
            let mut fonts = Fonts::default();
            change(&mut fonts, i);
            let f = build(&fonts);
            for field in 0..15 {
                let differs = match what {
                    "size" => sizes(&f)[field] != sizes(&stock)[field],
                    "weight" => weights(&f)[field] != weights(&stock)[field],
                    _ => families(&f)[field] != families(&stock)[field],
                };
                if differs {
                    assert!(owner[field].is_none(), "{what}: field {field} follows two roles");
                    owner[field] = Some(*role);
                }
            }
        }
        assert_eq!(owner, expected.map(Some), "{what}: the role each text format follows");
    }

    // The global family reaches every format; one that isn't installed is Bahnschrift.
    let segoe = build(&Fonts { family: Family::new("Segoe UI").unwrap(), ..Default::default() });
    assert!(families(&segoe).iter().all(|n| n == "Segoe UI"));
    assert!(face_fields(&segoe).iter().all(|f| unsafe { f.GetFontStretch() } == DWRITE_FONT_STRETCH_NORMAL), "only Bahnschrift is semi-condensed");
    assert!(face_fields(&stock).iter().all(|f| unsafe { f.GetFontStretch() } == DWRITE_FONT_STRETCH_SEMI_CONDENSED));
    let missing = build(&Fonts { family: Family::new("No Such Font 123").unwrap(), ..Default::default() });
    assert!(families(&missing).iter().all(|n| n == "Bahnschrift") && face_fields(&missing).iter().all(|f| f.dy == 0.0));
    let mut own = Fonts { family: Family::new("Segoe UI").unwrap(), ..Default::default() };
    own.roles[2].family = Family::new("No Such Font 123");
    let f = build(&own);
    assert_eq!((family_name(&f.name), family_name(&f.tag)), ("Bahnschrift".to_owned(), "Segoe UI".to_owned()), "a missing role family falls back to Bahnschrift");
}

#[test]
fn the_query_scroll_keeps_the_caret_in_view() {
    // Text shorter than the area never scrolls.
    assert_eq!(query_scroll(0.0, 40.0, 40.0, 200.0), 0.0);
    assert_eq!(query_scroll(30.0, 40.0, 40.0, 200.0), 0.0, "a stale offset is dropped when the text fits");
    // The caret at the end of a long text shows the end.
    let end = query_scroll(0.0, 500.0, 500.0, 200.0);
    assert_eq!(end, 500.0 + CARET_PAD - 200.0);
    // Moved back to the start: the offset follows it to 0.
    assert_eq!(query_scroll(end, 0.0, 500.0, 200.0), 0.0);
    // Inside the visible part: unchanged, whichever way it moves there.
    assert_eq!(query_scroll(end, 320.0, 500.0, 200.0), end);
    assert_eq!(query_scroll(end, 450.0, 500.0, 200.0), end);
    // Just left of it: scrolls only as far as the caret.
    assert_eq!(query_scroll(end, 290.0, 500.0, 200.0), 290.0);
    // A caret right of the window (the text grew): the window moves just enough.
    assert_eq!(query_scroll(100.0, 400.0, 600.0, 200.0), 400.0 + CARET_PAD - 200.0);
    // Never past the end of the text.
    let most = 500.0 + CARET_PAD - 200.0;
    assert_eq!(query_scroll(900.0, 480.0, 500.0, 200.0), most, "a stale offset past the end is clamped");
    assert_eq!(query_scroll(900.0, 100.0, 500.0, 200.0), 100.0, "then the caret decides");
}

#[test]
fn a_long_query_scrolls_with_the_caret_and_the_offset_is_stored() {
    let mut f = Fixture::new(crate::style::Shape::default());
    f.menu.set_query(&"w".repeat(120));
    let mut renderer = f.render();
    let at_end = f.menu.query_scroll.get();
    assert!(at_end > 100.0, "the end of the text is shown: {at_end}");
    f.draw(&mut renderer);
    assert_eq!(f.menu.query_scroll.get(), at_end, "stable from frame to frame");
    f.menu.query.move_to(0, false);
    f.draw(&mut renderer);
    assert_eq!(f.menu.query_scroll.get(), 0.0, "the caret at the start brings the start back");
    f.menu.query.move_to(usize::MAX, false);
    f.draw(&mut renderer);
    assert_eq!(f.menu.query_scroll.get(), at_end);
    f.menu.query.move_to(117, false);
    f.draw(&mut renderer);
    assert_eq!(f.menu.query_scroll.get(), at_end, "a caret still in the visible part leaves the offset alone");
    f.menu.set_query("w");
    f.draw(&mut renderer);
    assert_eq!(f.menu.query_scroll.get(), 0.0, "a short query is not scrolled");
    f.menu.set_query(&"w".repeat(120));
    f.draw(&mut renderer);
    f.menu.clear_query();
    f.draw(&mut renderer);
    assert_eq!(f.menu.query_scroll.get(), 0.0, "an empty query resets it");
}

#[test]
fn query_pixels_never_reach_the_match_count() {
    let shape = crate::style::Shape::default();
    let frame = |query: &str, notice: Option<&str>| {
        let mut f = Fixture::new(shape);
        f.menu.set_query(query);
        f.menu.notice = notice.map(str::to_owned);
        (f.render(), f.l)
    };
    for notice in [None, Some("PLUGIN FAILED TO ANSWER IN TIME")] {
        let (long, l) = frame(&"W".repeat(150), notice);
        let (short, _) = frame("x", notice);
        let side = notice.map_or_else(|| lang::plural("header.matches", 0), str::to_owned);
        let width = text_width(&long.dwrite, &long.formats.hint, &side);
        let right = l.menu_x + l.w - l.margin;
        let left = right - width.min(if notice.is_some() { right - l.w * 0.4 } else { right - l.margin });
        let gap_start = left - QUERY_GAP * l.ts;
        let (top, bottom) = ((32.0 * l.ts) as i32, (58.0 * l.ts) as i32);
        // Everything from the gap on is the same whatever the query says: the text ends before it.
        for y in top..bottom {
            for x in gap_start.ceil() as i32..(l.w as i32) {
                assert_eq!(pixel(&long, x, y), pixel(&short, x, y), "{notice:?}: the query reached ({x},{y}) past x = {gap_start}");
            }
        }
        // And it does fill the room it has.
        let near = (gap_start - 30.0 * l.ts) as i32..(gap_start - 2.0) as i32;
        assert!(near.clone().any(|x| (top..bottom).any(|y| pixel(&long, x, y) != pixel(&short, x, y))), "{notice:?}: the long query is drawn up to the gap");
    }
}

#[test]
fn clicks_on_a_scrolled_query_find_the_character_under_the_pointer() {
    let renderer = Fixture::new(crate::style::Shape::default()).render();
    let query = "abcdefghijklmnopqrstuvwxyz".repeat(8);
    let (shown, offsets) = lineedit::shouted(&query);
    let layout = query_layout(&renderer.dwrite, &renderer.formats.query, &format!("> {shown}")).unwrap();
    let prompt = prompt_width(&layout);
    let x_of = |k: usize| unit_position(&layout, PROMPT_UNITS + offsets[k]).unwrap().0;
    // Unscrolled: the character whose left edge is just before the point.
    for k in [0, 3, 9] {
        assert_eq!(renderer.query_caret_at(&query, x_of(k) + 1.0, false, 0.0), k);
    }
    // Scrolled by 400 px: the same character is 400 px further left.
    for k in [40, 77, 150] {
        assert_eq!(renderer.query_caret_at(&query, x_of(k) - 400.0 + 1.0, false, 400.0), k, "char {k}");
        assert_eq!(renderer.query_caret_at(&query, x_of(k) - 400.0 - 1.0, true, 400.0), k, "nearest: just before char {k}");
    }
    // The same point is a later character once the text is scrolled.
    let x = prompt + 60.0;
    assert!(renderer.query_caret_at(&query, x, true, 400.0) > renderer.query_caret_at(&query, x, true, 0.0) + 20);
    // The "> " does not scroll.
    assert_eq!(renderer.query_caret_at(&query, prompt / 2.0, true, 400.0), 0);
    assert_eq!(renderer.query_caret_at("", 10.0, true, 0.0), 0);
}

#[test]
fn the_dropdown_sits_over_the_rows_and_hits_before_them() {
    let f = Fixture::new(crate::style::Shape::default()).dropdown(Some(Role::Tag));
    let (l, g) = (f.l, f.drop_geom());
    let view = f.view.as_ref().unwrap();
    let slots: Vec<bool> = view.slots.iter().map(|s| matches!(s, Slot::Row(_))).collect();
    let parts = l.dropdown_parts(g);
    assert!(within(parts.outer, l.card()), "{:?} inside {:?}", parts.outer, l.card());
    // Where it covers rows below its own, the dropdown gets the point, not the row.
    let under = (0..slots.len()).find(|&k| k != g.slot && slots[k] && contains(parts.outer, center(l.setting_rect(k)).0, center(l.setting_rect(k)).1));
    let covered = center(l.setting_rect(under.expect("a row sits under the dropdown")));
    assert!(l.panel_hit(covered.0, covered.1, &slots).is_some());
    assert!(l.dropdown_hit(g, covered.0, covered.1).is_some_and(|hit| hit != DropHit::Anchor));
    // Each part answers for its own place.
    let at = |r: D2D_RECT_F| center(r);
    let (size_lo, size_hi) = l.drop_arrows(parts.size);
    let value = ((size_lo.right + size_hi.left) / 2.0, center(parts.size).1);
    assert_eq!(l.dropdown_hit(g, value.0, value.1), Some(DropHit::Size(0)));
    assert_eq!(l.dropdown_hit(g, parts.size.left + 4.0, value.1), Some(DropHit::Size(0)), "the label is the row too");
    let (lo, hi) = l.drop_arrows(parts.weight);
    assert_eq!(l.dropdown_hit(g, at(lo).0, at(lo).1), Some(DropHit::Weight(-1)));
    assert_eq!(l.dropdown_hit(g, at(hi).0, at(hi).1), Some(DropHit::Weight(1)));
    assert_eq!(l.dropdown_hit(g, at(parts.filter).0, at(parts.filter).1), Some(DropHit::Filter));
    for n in 0..VISIBLE {
        let item = at(l.dropdown_item(&parts, n));
        assert_eq!(l.dropdown_hit(g, item.0, item.1), Some(DropHit::Item(n)));
    }
    assert_eq!(l.dropdown_hit(g, parts.outer.left + 1.0, parts.outer.top + 1.0), Some(DropHit::Inside), "its padding is the dropdown too");
    // Its own row is the anchor (unless the dropdown was pushed up over it); anything else is outside.
    let anchor = l.setting_rect(g.slot);
    if parts.outer.top > anchor.bottom {
        assert_eq!(l.dropdown_hit(g, at(anchor).0, at(anchor).1), Some(DropHit::Anchor));
    }
    assert_eq!(l.dropdown_hit(g, 5.0, 5.0), None, "the list is outside");
    let tab = at(l.tab_rect(0));
    assert_eq!(l.dropdown_hit(g, tab.0, tab.1), None, "so are the tabs: a click there only closes it");
    // Every slot's dropdown stays in the card, with or without the role rows.
    for slot in 0..SLOTS {
        for role in [false, true] {
            let parts = l.dropdown_parts(DropGeom { slot, role });
            assert!(within(parts.outer, l.card()), "slot {slot} role {role}: {:?}", parts.outer);
            let last = l.dropdown_item(&parts, VISIBLE - 1);
            assert!(within(last, parts.outer), "slot {slot} role {role}: last entry {last:?}");
            if role {
                assert!(parts.size.bottom <= parts.weight.top && parts.weight.bottom <= parts.filter.top);
            }
        }
    }
}

#[test]
fn the_dropdown_draws_a_300_font_list_in_both_languages_and_keeps_its_preview_cache_capped() {
    use crate::config::Config;
    let style = Style::from_theme(&Config::default().colors);
    let mut f = Fixture::new(crate::style::Shape { row_radius: 12.0, panel_radius: 12.0, tag_radius: 6.0, icon_radius: 6.0, ..Default::default() }).panel(1);
    let mut renderer = Renderer::new(&f.l).unwrap();
    for code in ["en", "pl"] {
        crate::lang::use_builtin(code);
        for role in [None, Some(Role::Query)] {
            let mut panel = dropdown_panel(role, font_list(300), &style);
            // Through the whole list, a window at a time, with the filter empty and then typed.
            for step in 0..32 {
                f.view = Some(panel.view(&Config::default(), &style, false));
                f.hover = Some(Hover::Drop(DropHit::Item(step % VISIBLE)));
                f.draw(&mut renderer);
                panel.dropdown.as_mut().unwrap().scroll_by(10);
            }
            for c in "segoe".chars() {
                panel.dropdown_char(c);
            }
            f.view = Some(panel.view(&Config::default(), &style, false));
            f.draw(&mut renderer);
            for c in "zzzz".chars() {
                panel.dropdown_char(c);
            }
            f.view = Some(panel.view(&Config::default(), &style, false));
            assert_eq!(f.view.as_ref().unwrap().dropdown.as_ref().unwrap().total, 0);
            f.draw(&mut renderer);
            let cached = renderer.device.as_ref().unwrap().previews.borrow().len();
            assert!(cached > 0 && cached <= PREVIEW_CAP, "{cached} preview formats cached");
        }
    }
    crate::lang::use_builtin("en");
}

#[test]
fn dropdown_entries_are_set_in_their_own_font() {
    let f = Fixture::new(crate::style::Shape::default());
    let renderer = f.render();
    let device = renderer.device.as_ref().unwrap();
    let painter = Painter { factory: &renderer.factory, dwrite: &renderer.dwrite, device, rt: &device.target, brush: &device.brush, formats: &renderer.formats, l: &f.l, s: &f.style, hover: None };
    let family = |name: &str| painter.preview(name).map(|(face, _)| family_name(&face));
    for symbol in ["Wingdings", "Webdings", "Marlett"].into_iter().filter(|f| crate::fonts::is_installed(&renderer.dwrite, f)) {
        assert_eq!(painter.preview(symbol).map(|(_, readable)| readable), Some(false), "{symbol}");
    }
    assert_eq!(painter.preview("Consolas").map(|(_, readable)| readable), Some(true));
    assert_eq!(family("Consolas").as_deref(), Some("Consolas"));
    assert_eq!(family("Segoe UI").as_deref(), Some("Segoe UI"));
    let before = device.previews.borrow().len();
    family("Consolas");
    assert_eq!(device.previews.borrow().len(), before, "built once per family and size");
}

#[test]
fn the_header_toggles_popout_draws_hits_its_entries_and_stays_inside_the_card() {
    let f = Fixture::new(crate::style::Shape::default());
    let mut panel = crate::settings::Panel::default();
    panel.select_tab(1);
    let at = panel.rows().iter().position(|&r| r == crate::settings::Row::HeaderToggles).unwrap();
    panel.select(at);
    panel.open_dropdown(&f.style.fonts);
    let mut f = f.panel(1);
    let view = panel.view(&crate::config::Config::default(), &f.style, false);
    let toggles = view.toggles.as_ref().expect("the pop-out is open");
    assert_eq!(toggles.entries.len(), 7);
    let (slot, count) = (toggles.slot, toggles.entries.len());
    f.view = Some(view);
    let l = &f.l;
    let parts = l.toggles_parts(slot, count);
    assert!(parts.outer.bottom <= l.card().bottom && parts.outer.top >= l.card().top);
    for n in 0..count {
        let item = l.dropdown_item(&parts, n);
        let (x, y) = ((item.left + item.right) / 2.0, (item.top + item.bottom) / 2.0);
        assert_eq!(l.toggles_hit(slot, count, x, y), Some(DropHit::Item(n)));
    }
    assert_eq!(l.toggles_hit(slot, count, parts.outer.left + 1.0, parts.title.top), Some(DropHit::Inside));
    assert_eq!(l.toggles_hit(slot, count, -50.0, -50.0), None);
    assert_ne!(super::shapes::hash_pixels(&f.render()), super::shapes::hash_pixels(&Fixture::new(crate::style::Shape::default()).panel(1).render()), "the pop-out is drawn over the card");
}
