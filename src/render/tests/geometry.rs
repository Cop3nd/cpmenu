use super::*;

#[test]
fn shift_peaks_at_selection_and_falls_off() {
    let l = Layout::default();
    assert_eq!(l.shift(0.0), 40.0);
    assert!(l.shift(0.0) > l.shift(1.0) && l.shift(1.0) > l.shift(2.0) && l.shift(2.0) > l.shift(3.0));
    assert!(l.shift(6.0) < 0.1);
    assert_eq!(layout(600, 900, 1.5, 9, 40).shift(0.0), 60.0, "arc scales with DPI");
    assert_eq!(layout(600, 900, 1.0, 9, -30).shift(0.0), -30.0);
}

#[test]
fn rows_fill_the_list_area() {
    for (h, scale, rows) in [(640, 1.0, 9), (1328, 1.25, 7), (2100, 2.0, 12), (500, 1.0, 3)] {
        let l = layout(500, h, scale, rows, 40);
        assert!((l.step * rows as f32 - (l.list_bottom - l.list_top)).abs() < 0.01);
        assert!((l.row_h - l.step * ROW_FILL).abs() < 0.01);
        assert_eq!(l.list_top, BASE_HEADER * scale);
        assert!(l.list_bottom < l.footer_top && l.button_top + l.button_h < h as f32);
    }
}

#[test]
fn first_row_sits_at_the_list_top_and_last_at_the_bottom() {
    let l = layout(600, 1000, 1.25, 9, 40);
    assert_eq!(l.row_top(0.0, 0.0, 50), l.list_top);
    assert_eq!(l.position_alpha(l.list_top, 0.0, 50), 1.0, "start of the list is not faded");
    let last_top = l.row_top(49.0, 49.0, 50);
    assert!((last_top + l.row_h - l.list_bottom).abs() < 0.01);
    assert_eq!(l.position_alpha(last_top, 49.0, 50), 1.0, "end of the list is not faded");
    // The cut-off side still fades.
    assert_eq!(l.position_alpha(l.list_bottom - l.row_h / 2.0, 0.0, 50), 0.0);
    assert_eq!(l.position_alpha(l.list_top - l.step, 49.0, 50), 0.0);
}

#[test]
fn selection_is_centred_in_the_middle_and_follows_scrolling_smoothly() {
    let l = layout(600, 1000, 1.25, 9, 40);
    let centre = (l.list_top + l.list_bottom) / 2.0;
    let mid = l.row_top(25.0, 25.0, 50);
    assert!((mid + l.row_h / 2.0 - centre).abs() < 0.5);
    assert_eq!(l.position_alpha(mid, 25.0, 50), 1.0);
    let above = l.position_alpha(mid - 3.0 * l.step, 25.0, 50);
    let below = l.position_alpha(mid + 3.0 * l.step, 25.0, 50);
    assert!(above < 1.0 && (above - below).abs() < 0.01, "symmetric fade");
    // No jumps: the offset never changes faster than the scroll itself.
    let mut previous = l.offset(0.0, 50);
    for i in 1..=4900 {
        let now = l.offset(i as f32 / 100.0, 50);
        assert!((now - previous).abs() <= l.step / 100.0 + 0.001);
        previous = now;
    }
}

fn layout_with(cfg: LayoutConfig) -> Layout {
    Layout::new(600, 1000, 1.0, &LayoutConfig { visible_rows: 9, ..cfg })
}

#[test]
fn band_percent_places_the_selection_while_scrolling() {
    let mid_list = |band: u32| {
        let l = layout_with(LayoutConfig { band_percent: band, ..Default::default() });
        (l, l.row_top(25.0, 25.0, 50))
    };
    let (l, top) = mid_list(0);
    assert!((top - l.list_top).abs() < 0.01, "0 = selection at the top of the list area");
    let (l, top) = mid_list(100);
    assert!((top + l.row_h - l.list_bottom).abs() < 0.01, "100 = at the bottom");
    let (l, top) = mid_list(50);
    assert!((top + l.row_h / 2.0 - (l.list_top + l.list_bottom) / 2.0).abs() < 0.5, "50 = middle");
    let (l, top) = mid_list(25);
    let expected = l.list_top + (l.list_bottom - l.list_top - l.row_h) * 0.25;
    assert!((top - expected).abs() < 0.01);
}

#[test]
fn band_keeps_rows_filling_to_the_edges_at_both_ends() {
    for band in [0, 30, 50, 100] {
        let l = layout_with(LayoutConfig { band_percent: band, ..Default::default() });
        assert_eq!(l.row_top(0.0, 0.0, 50), l.list_top, "start, band {band}");
        let last = l.row_top(49.0, 49.0, 50);
        assert!((last + l.row_h - l.list_bottom).abs() < 0.01, "end, band {band}");
        // Selection moves down to the band before the list starts scrolling.
        let selection_y = |scroll: f32| l.row_top(scroll, scroll, 50);
        assert!(selection_y(1.0) > selection_y(0.0) || band == 0);
    }
}

#[test]
fn short_list_can_be_centred() {
    let top = layout_with(LayoutConfig::default());
    let centred = layout_with(LayoutConfig { short_list: ShortList::Center, ..Default::default() });
    assert_eq!(top.row_top(0.0, 0.0, 3), top.list_top);
    let block = 2.0 * centred.step + centred.row_h;
    let first = centred.row_top(0.0, 0.0, 3);
    let free_above = first - centred.list_top;
    let free_below = centred.list_bottom - (first + block);
    assert!(free_above > 0.0 && (free_above - free_below).abs() < 0.01);
    // Scrolling a short list never moves it.
    assert_eq!(centred.row_top(0.0, 2.0, 3), first);
    // Hit-testing follows the centred position.
    assert_eq!(centred.row_at(200.0, first + 1.0, 0.0, 3), Some(0));
    assert_eq!(centred.row_at(200.0, centred.list_top + 1.0, 0.0, 3), None);
}

#[test]
fn list_eases_to_its_new_position_when_the_row_count_changes() {
    let centred = layout_with(LayoutConfig { short_list: ShortList::Center, ..Default::default() });
    // (old rows, new rows): shorter, longer, and across the fits / doesn't-fit boundary both ways.
    for (old, new) in [(3, 1), (1, 3), (30, 2), (2, 30)] {
        let mut l = centred;
        let (from, to) = (l.offset(0.0, old), l.offset(0.0, new));
        assert!(l.retarget(old, 0.0, new) == ((from - to).abs() >= 0.5), "{old} -> {new}");
        assert!((l.offset(0.0, new) - from).abs() < 0.01, "starts where the old list was drawn");
        let mut frames = 0;
        let mut previous = (from - to).abs();
        while l.step_slide() {
            let distance = (l.offset(0.0, new) - to).abs();
            assert!(distance < previous, "moves monotonically towards the target");
            (previous, frames) = (distance, frames + 1);
            // Hit testing follows what is drawn.
            let y = l.row_top(0.0, 0.0, new) + 1.0;
            if y >= l.list_top && y < l.list_bottom {
                assert_eq!(l.row_at(200.0, y, 0.0, new), Some(0), "{old} -> {new}");
            }
        }
        assert!(frames <= 12, "settles within about 180 ms, took {frames} frames");
        assert_eq!(l.offset(0.0, new), to);
    }
    // Nothing to ease when the position does not depend on the count.
    let mut top = layout_with(LayoutConfig::default());
    assert!(!top.retarget(1, 0.0, 3) && !top.step_slide());
}

#[test]
fn replacing_the_layout_keeps_the_list_where_it_was_drawn() {
    let before = layout_with(LayoutConfig { band_percent: 20, ..Default::default() });
    let mut after = layout_with(LayoutConfig { band_percent: 80, ..Default::default() });
    assert!(after.carry_slide(&before, 10.0, 40));
    assert!((after.offset(10.0, 40) - before.offset(10.0, 40)).abs() < 0.01);
    while after.step_slide() {}
    assert_eq!(after.offset(10.0, 40), layout_with(LayoutConfig { band_percent: 80, ..Default::default() }).offset(10.0, 40));
}

#[test]
fn hover_targets_follow_the_pointer() {
    let l = layout(480, 1000, 1.0, 9, 40).with_panel(340, false);
    let b = l.button_rect(1);
    let power = (center(b).0, center(b).1);
    assert_eq!(l.hover_at(power.0, power.1, &[true], None), Some(Hover::Power(1)));
    let tab = center(l.tab_rect(2));
    assert_eq!(l.hover_at(tab.0, tab.1, &[true], None), Some(Hover::Panel(PanelHit::Tab(2))));
    let arrow = center(l.arrow_rect(0, 1));
    assert_eq!(l.hover_at(arrow.0, arrow.1, &[true], None), Some(Hover::Panel(PanelHit::Arrow(0, 1))));
    assert_eq!(l.hover_at(arrow.0, arrow.1, &[false], None), None, "a heading has no arrows");
    let style = center(l.style_button_rect());
    assert_eq!(l.hover_at(style.0, style.1, &[true], None), Some(Hover::Panel(PanelHit::OpenStyle)));
    let config = center(l.config_button_rect());
    assert_eq!(l.hover_at(config.0, config.1, &[true], None), Some(Hover::Panel(PanelHit::OpenConfig)));
    assert!(l.config_button_rect().right < l.style_button_rect().left, "buttons don't overlap");
    assert_eq!(l.hover_at(200.0, 300.0, &[true], None), None, "list area");
    let popup = (300.0, 20.0);
    assert_eq!(l.hover_at(310.0, 30.0, &[true], Some(popup)), Some(Hover::Popup(0)));
    assert_eq!(l.hover_at(310.0, 50.0, &[true], Some(popup)), Some(Hover::Popup(1)));
    assert_eq!(l.hover_at(power.0, power.1, &[true], Some(popup)), Some(Hover::Power(1)), "away from the popup");
}

#[test]
fn short_list_starts_at_the_top() {
    let l = layout(600, 1000, 1.0, 9, 40);
    for scroll in [0.0, 1.0, 2.0] {
        assert_eq!(l.row_top(0.0, scroll, 3), l.list_top);
    }
    assert_eq!(l.row_top(0.0, 0.0, 0), l.list_top);
}

#[test]
fn rows_stop_above_the_power_bar() {
    let l = Layout::default();
    assert_eq!(l.row_at(100.0, l.list_bottom + 1.0, 0.0, 100), None);
    assert_eq!(l.position_alpha(l.list_bottom - l.row_h / 2.0, 50.0, 100), 0.0);
}

#[test]
fn row_at_agrees_with_drawing_everywhere_in_the_list() {
    let l = layout(540, 1000, 1.5, 7, 40);
    for (count, scroll) in [(30, 0.0), (30, 15.0), (30, 29.0), (30, 14.4), (3, 1.0), (30, 1.0)] {
        for row in 0..count {
            let top = l.row_top(row as f32, scroll, count);
            if l.position_alpha(top, scroll, count) <= 0.3 || top < l.list_top || top + l.row_h > l.list_bottom {
                continue;
            }
            let found = |y| l.row_at(200.0, y, scroll, count);
            assert_eq!(found(top + 1.0), Some(row), "count {count} scroll {scroll} row {row}");
            assert_eq!(found(top + l.row_h - 1.0), Some(row));
            assert_eq!(found(top + l.row_h + 1.0), None, "gap");
        }
    }
    assert_eq!(l.row_at(200.0, l.list_top + 1.0, 0.0, 30), Some(0));
    assert_eq!(l.row_at(200.0, l.list_bottom - 1.0, 29.0, 30), Some(29));
    assert_eq!(l.row_at(200.0, l.list_top + 1.0, 0.0, 0), None);
    assert_eq!(l.row_at(2.0, l.list_top + 1.0, 0.0, 30), None, "left of the rows");
    assert_eq!(l.row_at(200.0, l.list_top - 1.0, 0.0, 30), None, "header");
    assert_eq!(l.row_at(200.0, l.list_top + 4.0 * l.step + 1.0, 0.0, 3), None, "past the end");
}

#[test]
fn gear_and_close_are_hit_targets() {
    let l = layout(480, 1000, 1.0, 9, 40).with_panel(340, true);
    let gear = center(l.gear_rect());
    assert!(gear.1 < l.list_top, "on the header's title line");
    let at = (gear.0 + l.menu_x(), gear.1);
    assert!(l.gear_hit(at.0, at.1) && l.in_header(at.0, at.1));
    assert!(!l.gear_hit(gear.0, gear.1), "the panel column to the left shifts the menu");
    assert_eq!(l.hover_at(at.0, at.1, &[true], None), Some(Hover::Gear));
    assert!(!l.gear_hit(at.0 - 40.0, at.1) && !l.gear_hit(at.0, l.list_top + 2.0));

    let l = layout(480, 1000, 1.5, 9, 40).with_panel(510, false);
    let close = center(l.close_rect());
    assert_eq!(l.panel_hit(close.0, close.1, &[true; 5]), Some(PanelHit::Close));
    assert_eq!(l.hover_at(close.0, close.1, &[true], None), Some(Hover::Panel(PanelHit::Close)));
    let (card, r) = (l.card(), l.close_rect());
    assert!(r.right <= card.right && r.top >= card.top && r.bottom <= l.tab_rect(0).top, "inside the card, above the tabs");
    assert!(!l.in_panel(close.0, close.1 + 1000.0) && !Layout::default().with_panel(0, false).gear_hit(10.0, 10.0));
}

#[test]
fn gui_scale_scales_text_metrics_and_the_automatic_row_count_follows() {
    let at = |size: u32| Layout::new(480, 1012, 1.0, &LayoutConfig { gui_scale: size, ..Default::default() });
    let (a, b, c) = (at(100), at(110), at(130));
    assert_eq!((a.ts, b.ts, c.ts), (1.0, 1.1, 1.3));
    assert!(a.list_top < b.list_top && b.list_top < c.list_top, "header grows");
    assert!(a.button_h < b.button_h && b.button_h < c.button_h, "power bar grows");
    assert!(a.row_h < b.row_h && b.row_h < c.row_h, "rows grow");
    assert!(a.rows > c.rows, "fewer, bigger rows: {} vs {}", a.rows, c.rows);
    for l in [a, b, c] {
        assert!(l.list_bottom < l.footer_top && l.button_top + l.button_h < 1012.0);
        assert!((l.step * l.rows - (l.list_bottom - l.list_top)).abs() < 0.01);
        assert!(l.setting_rect(0).top > l.tab_rect(0).bottom && l.config_button_rect().bottom <= l.card().bottom, "panel fits its card");
    }
    assert!(c.card().bottom - c.card().top > a.card().bottom - a.card().top, "panel grows");
    let scaled = Layout::new(480, 1012, 1.5, &LayoutConfig { gui_scale: 130, ..Default::default() });
    assert!((scaled.ts - 1.95).abs() < 1e-6);
    assert_eq!(Layout::new(480, 1600, 1.0, &LayoutConfig { gui_scale: 999, ..Default::default() }).ts, 2.5, "out-of-range sizes are clamped");
    assert_eq!(Layout::new(480, 1600, 1.0, &LayoutConfig { gui_scale: 5, ..Default::default() }).ts, 0.5);
}

#[test]
fn panel_column_goes_beside_the_menu_and_hit_testing_follows_it() {
    let base = layout(480, 1000, 1.0, 9, 40);
    for (on_left, menu_x) in [(false, 0.0), (true, 340.0)] {
        let l = base.with_panel(340, on_left);
        assert_eq!(l.width, 820);
        assert_eq!(l.menu_x(), menu_x);
        let card = l.card();
        assert!(card.left >= 0.0 && card.right <= 820.0 && card.bottom <= 1000.0);
        // The card never overlaps the menu's columns.
        if on_left {
            assert!(card.right <= menu_x);
        } else {
            assert!(card.left >= 480.0);
        }
        // The menu keeps working, shifted by the panel when that sits on its left.
        let middle = l.band_y + l.row_h / 2.0;
        assert_eq!(l.row_at(menu_x + 200.0, middle, 8.0, 30), Some(8));
        let b = l.button_rect(0);
        assert_eq!(l.button_at(menu_x + b.left + 1.0, l.button_top + 2.0), Some(0));
        assert!(l.in_header(menu_x + 100.0, 10.0) && !l.in_header(menu_x + 100.0, l.list_top + 1.0));
        assert!(!l.in_header(if on_left { 100.0 } else { 600.0 }, 10.0), "panel column is not header");
    }
    assert!(!base.has_panel() && base.panel_hit(10.0, 10.0, &[true; 5]).is_none());
}

const ROW: D2D_RECT_F = D2D_RECT_F { left: 220.0, top: 300.0, right: 400.0, bottom: 350.0 };
const LIST: D2D_RECT_F = D2D_RECT_F { left: 216.0, top: 80.0, right: 404.0, bottom: 900.0 };

fn placed(anchor: ActionsAnchor, side: Side, cursor: Option<(f32, f32)>, offset: (i32, i32)) -> (f32, f32) {
    place_popout(anchor, side, ROW, LIST, cursor, (150.0, 100.0), 6.0, offset, (800.0, 1000.0))
}

#[test]
fn popout_goes_to_the_far_side_of_the_row_the_list_or_the_pointer() {
    use ActionsAnchor::*;
    assert_eq!(placed(Row, Side::Left, None, (0, 0)), (406.0, 300.0), "right of the row, top aligned");
    assert_eq!(placed(Row, Side::Right, None, (0, 0)), (220.0 - 6.0 - 150.0, 300.0), "left of the row");
    assert_eq!(placed(List, Side::Left, None, (0, 0)), (410.0, 80.0), "beside the whole list, level with its top");
    assert_eq!(placed(List, Side::Right, None, (0, 0)), (216.0 - 6.0 - 150.0, 80.0));
    assert_eq!(placed(Cursor, Side::Left, Some((200.0, 500.0)), (0, 0)), (200.0, 500.0), "at the pointer");
    assert_eq!(placed(Cursor, Side::Right, Some((200.0, 500.0)), (0, 0)), (200.0, 500.0));
    assert_eq!(placed(Cursor, Side::Left, None, (0, 0)), placed(Row, Side::Left, None, (0, 0)), "keyboard: falls back to the row");
}

#[test]
fn popout_offsets_are_percent_of_its_own_size_and_it_stays_in_the_window() {
    use ActionsAnchor::*;
    assert_eq!(placed(Row, Side::Left, None, (50, 20)), (406.0 + 75.0, 300.0 + 20.0));
    assert_eq!(placed(Row, Side::Left, None, (-100, -100)), (256.0, 200.0));
    assert_eq!(placed(Row, Side::Left, None, (100, 0)).0, 556.0);
    // Clamped to the window on every edge.
    assert_eq!(placed(Row, Side::Left, None, (-100, -100)), (256.0, 200.0));
    assert_eq!(placed(List, Side::Right, None, (-100, -100)), (0.0, 0.0));
    assert_eq!(placed(Cursor, Side::Left, Some((790.0, 990.0)), (0, 0)), (650.0, 900.0));
    assert_eq!(placed(Cursor, Side::Left, Some((790.0, 990.0)), (100, 100)), (650.0, 900.0));
    assert_eq!(placed(Cursor, Side::Left, Some((-50.0, -50.0)), (0, 0)), (0.0, 0.0));
    // A window narrower than the pop-out pins it to the left edge instead of going negative.
    assert_eq!(place_popout(Row, Side::Left, ROW, LIST, None, (150.0, 100.0), 6.0, (0, 0), (100.0, 1000.0)).0, 0.0);
}

#[test]
fn popout_column_keeps_the_popout_visible_on_either_side() {
    let base = layout(480, 1000, 1.0, 9, 40);
    let col = Layout::popout_column(176.0, 1920, 480, 1.0);
    assert_eq!(col, 176);
    assert_eq!(Layout::popout_column(176.0, 600, 480, 1.0), 600 - 16 - 480, "capped by the room in the work area");
    assert_eq!(Layout::popout_column(176.0, 480, 480, 1.0), 0);
    for (side, on_left) in [(Side::Left, false), (Side::Right, true)] {
        let l = base.with_popout(col, on_left);
        assert_eq!((l.width, l.popout_col(), l.has_panel()), (480 + 176, 176, false));
        assert_eq!(l.menu_x(), if on_left { 176.0 } else { 0.0 });
        for anchor in [ActionsAnchor::Row, ActionsAnchor::List] {
            let r = l.popout_rect(&popout(anchor, side, (0, 0)));
            assert!(r.left >= 0.0 && r.right <= l.width as f32 && r.top >= 0.0 && r.bottom <= 1000.0, "{anchor:?} {r:?}");
            // Beside the whole list it sits in the added column, clear of the list's own box.
            if anchor == ActionsAnchor::List {
                let (menu_left, menu_right) = (l.menu_x(), l.menu_x() + 480.0);
                if on_left {
                    assert!(r.right <= menu_left + 16.0, "{r:?}");
                } else {
                    assert!(r.left >= menu_right - 16.0, "{r:?}");
                }
            }
        }
        // An offset that would push it out is held at the window edge.
        let r = l.popout_rect(&popout(ActionsAnchor::Row, side, (100, 100)));
        assert!(r.right <= l.width as f32 && r.bottom <= 1000.0);
    }
    // No room for a column: the pop-out still lies inside the window, over the list.
    let tight = base.with_popout(0, false);
    let r = tight.popout_rect(&popout(ActionsAnchor::Row, Side::Left, (0, 0)));
    assert!(r.right <= 480.0 && r.left >= 0.0);
    // A panel and a pop-out never share the window.
    assert_eq!(base.with_panel(340, false).with_popout(176, false).popout_col(), 0);
}

#[test]
fn the_placed_popout_lies_inside_the_window_for_both_sides_and_arcs() {
    for arc in [40, 120, -120] {
        for scale in [1.0, 1.25, 1.5] {
            for side in [Side::Left, Side::Right] {
                for anchor in [ActionsAnchor::Row, ActionsAnchor::List] {
                    for offset in [(0, 0), (30, 0)] {
                        let cfg = LayoutConfig { visible_rows: 9, arc_shift: arc, ..LayoutConfig::default() };
                        let menu_w = (420.0 * scale) as i32;
                        let base = Layout::new(menu_w, 1000, scale, &cfg);
                        let on_left = side == Side::Right;
                        // Wide labels, and every row distance the eased selection can have.
                        for scroll in [3.0, 3.4, 2.0] {
                            let p = PopoutParams { width: 250.0 * scale, scroll, ..popout(anchor, side, offset) };
                            let col = Layout::popout_column(base.popout_overhang(&p), 2560, menu_w, scale);
                            assert!(col > 0, "arc {arc} {side:?} {anchor:?}");
                            let l = base.with_popout(col, on_left);
                            let r = l.popout_rect(&p);
                            assert!((r.right - r.left - p.width).abs() < 0.01, "not squeezed");
                            assert!(r.left >= 0.0 && r.right <= l.width as f32 + 0.01, "arc {arc} {side:?} {anchor:?} {offset:?}: {r:?} in {}", l.width);
                            // The list keeps its own width and place; only the pop-out uses the column.
                            assert_eq!(l.w, base.w);
                            assert_eq!(l.menu_x(), if on_left { col as f32 } else { 0.0 });
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn popout_items_are_hit_before_the_rows_beneath_it() {
    let l = layout(480, 1000, 1.0, 9, 40).with_popout(176, false);
    let p = popout(ActionsAnchor::Row, Side::Left, (-100, 0));
    let r = l.popout_rect(&p);
    let (x, first_y) = (r.left + 20.0, r.top + 4.0 + 5.0);
    // Moved left over the list, the pop-out covers the selected row and the rows around it.
    assert_eq!(l.row_at(x, first_y, 2.0, 10), Some(2), "a row is there");
    assert_eq!(l.list_hit(Some(&p), x, first_y, 2.0, 10), ListHit::Popout(0));
    assert_eq!(l.list_hit(Some(&p), x, first_y + 26.0 * 2.0, 2.0, 10), ListHit::Popout(2));
    assert_eq!(l.list_hit(Some(&p), x, r.bottom - 1.0, 2.0, 10), ListHit::Popout(4));
    assert_eq!(l.list_hit(Some(&p), x, r.top + 1.0, 2.0, 10), ListHit::Popout(0), "its padding counts as the pop-out");
    // Beyond its rectangle the rows answer again, and with no pop-out they always do.
    let below = l.row_top(4.0, 2.0, 10) + 5.0;
    assert_eq!(l.list_hit(Some(&p), x, below, 2.0, 10), ListHit::Row(4));
    assert_eq!(l.list_hit(None, x, first_y, 2.0, 10), ListHit::Row(2));
    assert_eq!(l.list_hit(Some(&p), 1.0, 1.0, 2.0, 10), ListHit::Nothing);
}

#[test]
fn popout_width_is_fitted_with_a_minimum_and_a_maximum() {
    assert_eq!(Layout::popout_width(10.0, 1.0), 150.0);
    assert_eq!(Layout::popout_width(200.0, 1.0), 242.0);
    assert_eq!(Layout::popout_width(900.0, 1.0), 320.0);
    assert_eq!(Layout::popout_width(900.0, 1.5), 480.0);
}

#[test]
fn panel_hit_finds_tabs_arrows_rows_and_the_button() {
    let l = layout(480, 1000, 1.5, 9, 40).with_panel(510, false);
    for i in 0..TABS.len() {
        let (x, y) = center(l.tab_rect(i));
        assert_eq!(l.panel_hit(x, y, &[true; 5]), Some(PanelHit::Tab(i)));
    }
    for i in 0..5 {
        for dir in [-1, 1] {
            let (x, y) = center(l.arrow_rect(i, dir));
            assert_eq!(l.panel_hit(x, y, &[true; 5]), Some(PanelHit::Arrow(i, dir)));
        }
        let r = l.setting_rect(i);
        assert_eq!(l.panel_hit(r.left + 3.0, (r.top + r.bottom) / 2.0, &[true; 5]), Some(PanelHit::Row(i)));
    }
    let (x, y) = center(l.config_button_rect());
    assert_eq!(l.panel_hit(x, y, &[true]), Some(PanelHit::OpenConfig));
    let (x, y) = center(l.setting_rect(3));
    assert_eq!(l.panel_hit(x, y, &[true]), None, "slot 3 doesn't exist on a one-row tab");
    assert!(l.in_panel(x, y) && !l.in_panel(100.0, 100.0), "menu area is not the card");
    let rows_gap = (l.setting_rect(0).bottom + l.setting_rect(1).top) / 2.0;
    assert_eq!(l.panel_hit(center(l.setting_rect(0)).0, rows_gap, &[true; 5]), None, "gap between rows");
    let shape = [true, false, true];
    let at = |i: usize| {
        let r = l.setting_rect(i);
        (r.left + 3.0, (r.top + r.bottom) / 2.0)
    };
    assert_eq!(l.panel_hit(at(1).0, at(1).1, &shape), None, "headings are not targets");
    assert_eq!(l.panel_hit(at(2).0, at(2).1, &shape), Some(PanelHit::Row(2)));
    let card = l.card();
    assert!(l.config_button_rect().bottom < card.bottom && l.note_rect().bottom < l.config_button_rect().top);
    assert!(l.setting_rect(4).bottom < l.note_rect().top);
}

#[test]
fn panel_fits_in_the_window_and_width_adapts_to_the_screen() {
    assert_eq!(Layout::panel_width(1920, 480, 1.0, 1.0), 340);
    assert_eq!(Layout::panel_width(1920, 480, 1.5, 1.5), 510);
    assert_eq!(Layout::panel_width(1920, 480, 1.0, 1.3), 442, "grows with the text size");
    assert_eq!(Layout::panel_width(800, 480, 1.0, 1.0), 800 - 16 - 480, "limited by the screen");
    assert_eq!(Layout::panel_width(480, 480, 1.0, 1.0), 0);
    let l = layout(480, 1000, 1.0, 9, 40).with_panel(340, false);
    let card = l.card();
    assert!(l.config_button_rect().bottom <= card.bottom);
}

#[test]
fn popup_stays_inside_the_window_and_hits() {
    let l = layout(480, 640, 1.0, 9, 40).with_panel(340, true);
    let at = (400.0, 20.0);
    let r = l.popup_rect(at);
    assert_eq!((r.left, r.top), (400.0, 20.0));
    assert_eq!(l.popup_entry(at, 410.0, 30.0), Some(0), "PASTE");
    assert_eq!(l.popup_entry(at, 410.0, 60.0), Some(1), "SETTINGS");
    assert!(l.popup_entry(at, 300.0, 30.0).is_none() && l.popup_entry(at, 410.0, 90.0).is_none());
    let edge = l.popup_rect((815.0, 635.0));
    assert!(edge.right <= 820.0 && edge.bottom <= 640.0 && edge.left >= 0.0);
    assert_eq!(l.popup_entry((815.0, 635.0), edge.left + 1.0, edge.top + 1.0), Some(0));
}

#[test]
fn progress_bar_fraction() {
    assert_eq!(progress(4.0, 10), 0.5, "fifth of ten");
    assert_eq!(progress(9.0, 10), 1.0, "last");
    assert_eq!(progress(0.0, 10), 0.1);
    assert_eq!(progress(0.0, 0), 0.0, "empty list");
    assert_eq!(progress(-1.0, 5), 0.0);
    assert_eq!(progress(12.0, 5), 1.0, "overshoot is clamped");
    let mid = progress(4.5, 10);
    assert!(mid > 0.5 && mid < 0.6, "follows the eased scroll");
}

#[test]
fn automatic_rows_keep_rows_near_design_height() {
    let auto = |h: i32, scale: f32| Layout::new(480, h, scale, &LayoutConfig::default());
    let l = auto(1012, 1.0);
    assert!((l.step - 64.0).abs() < 5.0 && (l.row_h - 54.0).abs() < 4.0, "{}", l.step);
    assert!((auto(2000, 1.5).step - 96.0).abs() < 8.0, "scales with DPI");
    assert!(auto(100, 1.0).rows >= 3.0 && auto(8000, 1.0).rows <= 30.0);
    let explicit = Layout::new(480, 1012, 1.0, &LayoutConfig { visible_rows: 9, ..Default::default() });
    assert_eq!(explicit.rows, 9.0);
}

#[test]
fn the_default_shape_keeps_the_original_row_metrics() {
    let l = Layout::new(480, 1012, 1.0, &LayoutConfig::default());
    let fixed = Layout::new(480, 1012, 1.0, &LayoutConfig { visible_rows: 9, ..Default::default() });
    assert_eq!(l.row_h, l.step * ROW_FILL);
    assert_eq!(fixed.row_h, fixed.step * ROW_FILL);
    assert_eq!(l.k, l.row_h / BASE_ROW_H);
}

#[test]
fn row_height_and_gap_set_the_automatic_row_count_and_the_row_to_step_ratio() {
    let height = 1012;
    let default = Layout::new(480, height, 1.0, &LayoutConfig::default());
    let span = default.list_bottom - default.list_top;
    for (row_height, gap) in [(72.0, 0.0), (36.0, 30.0), (90.0, 10.0), (54.0, 20.0)] {
        let l = Layout::new(480, height, 1.0, &shaped(row_height, gap));
        let expected = (span / (row_height + gap)).round().clamp(3.0, 30.0);
        assert_eq!(l.rows, expected, "{row_height}/{gap}");
        assert!((l.step * l.rows - span).abs() < 0.01, "the rows fill the list area");
        let ratio = (l.row_h / l.step) / (default.row_h / default.step);
        let wanted = (row_height / (row_height + gap)) / (54.0 / 64.0);
        assert!((ratio - wanted.min(1.0 / ROW_FILL)).abs() < 1e-4, "{row_height}/{gap}: {ratio} vs {wanted}");
        assert!((l.k - l.row_h / BASE_ROW_H).abs() < 1e-6, "row internals follow the row height");
    }
    let tall = Layout::new(480, height, 1.0, &shaped(90.0, 10.0));
    let short = Layout::new(480, height, 1.0, &shaped(36.0, 10.0));
    assert!(tall.row_h > default.row_h && short.row_h < default.row_h);
}

#[test]
fn hit_testing_uses_the_custom_row_height_and_gap() {
    let mut cfg = shaped(72.0, 30.0);
    cfg.visible_rows = 6;
    let l = Layout::new(480, 1012, 1.0, &cfg);
    let x = l.menu_x() + l.w / 2.0;
    for row in 0..4 {
        let top = l.row_top(row as f32, 0.0, 6);
        assert_eq!(l.row_at(x, top + 1.0, 0.0, 6), Some(row), "top of row {row}");
        assert_eq!(l.row_at(x, top + l.row_h - 1.0, 0.0, 6), Some(row), "bottom of row {row}");
        assert_eq!(l.row_at(x, top + l.row_h + 2.0, 0.0, 6), None, "the gap below row {row}");
    }
    assert!(l.step - l.row_h > 10.0, "a 30 px gap is visible: {} vs {}", l.step, l.row_h);
    let zero = Layout::new(480, 1012, 1.0, &LayoutConfig { visible_rows: 6, ..shaped(72.0, 0.0) });
    assert!(zero.step - zero.row_h < 0.01 * zero.step, "no gap leaves (nearly) touching rows");
}

#[test]
fn button_at_finds_power_buttons() {
    for (w, h, scale) in [(480, 640, 1.0), (700, 1300, 1.75)] {
        let l = layout(w, h, scale, 9, 40);
        let first = l.button_rect(0);
        let last = l.button_rect(ACTIONS.len() - 1);
        let mid = l.button_top + l.button_h / 2.0;
        assert_eq!(l.button_at(first.left + 1.0, mid), Some(0));
        assert_eq!(l.button_at(last.right - 1.0, mid), Some(ACTIONS.len() - 1));
        assert_eq!(l.button_at(first.right + l.button_gap / 2.0, mid), None, "gap");
        assert_eq!(l.button_at(first.left + 1.0, l.button_top - 5.0), None, "above the bar");
        assert!((last.right - (w as f32 - l.margin)).abs() < 0.01, "buttons span the width");
    }
}

#[test]
fn rows_stay_inside_the_margins_for_both_arc_directions() {
    for arc in [60, -60, 0] {
        let l = layout(480, 640, 1.0, 9, arc);
        for d in [0.0, 1.0, 2.5, 8.0] {
            let (panel_left, icon_left) = l.row_x(d);
            assert!(panel_left >= l.margin - 0.01 && icon_left + l.row_h <= l.w - l.margin + 0.01, "arc {arc} d {d}");
        }
        let (a, _) = l.row_x(0.0);
        let (b, _) = l.row_x(5.0);
        assert_eq!(a < b, arc < 0, "selected row is the most displaced one");
    }
}

#[test]
fn window_size_follows_work_area_percent_and_minimum() {
    let cfg = LayoutConfig { width_percent: 25, ..Default::default() };
    assert_eq!(Layout::window_size(1920, 1040, 1.0, &cfg), (480, 1024));
    assert_eq!(Layout::window_size(1280, 720, 1.0, &cfg), (420, 704), "never narrower than 420");
    assert_eq!(Layout::window_size(2560, 1400, 1.5, &cfg), (640, 1400 - 24));
    assert_eq!(Layout::window_size(2560, 1400, 2.0, &cfg), (840, 1400 - 32), "minimum scales with DPI");
    let wide = LayoutConfig { width_percent: 50, ..Default::default() };
    assert_eq!(Layout::window_size(700, 800, 1.0, &wide), (420, 784));
    assert_eq!(Layout::window_size(400, 800, 1.0, &wide).0, 384, "limited by the screen");
}

#[test]
fn open_animation_covers_every_row() {
    assert_eq!(layout(480, 640, 1.0, 9, 40).open_ms(), 12.0 * ROW_DELAY_MS + ROW_IN_MS);
    assert!(layout(480, 900, 1.0, 20, 40).open_ms() > layout(480, 640, 1.0, 9, 40).open_ms());
}

fn overlaps(a: D2D_RECT_F, b: D2D_RECT_F) -> bool {
    a.left < b.right - 0.01 && b.left < a.right - 0.01 && a.top < b.bottom - 0.01 && b.top < a.bottom - 0.01
}

#[test]
fn the_layout_holds_together_from_50_to_250_percent_on_small_and_large_screens() {
    // (work area, DPI scale): a laptop panel, a full HD screen, a scaled 4K screen.
    for ((work_w, work_h), dpi) in [((1280, 680), 1.0f32), ((1920, 1040), 1.0), ((3840, 2080), 1.5)] {
        for gui in [50, 100, 250] {
            let ctx = format!("work {work_w}x{work_h} dpi {dpi} gui {gui}%");
            let cfg = LayoutConfig { gui_scale: gui, ..Default::default() };
            let (menu_w, height) = Layout::window_size(work_w, work_h, dpi, &cfg);
            let margin = Layout::outer_margin(dpi);
            assert!(menu_w > 0 && height > 0 && height <= work_h && menu_w <= work_w - 2 * margin, "{ctx}: window {menu_w}x{height}");
            let base = Layout::new(menu_w, height, dpi, &cfg);
            assert!(base.ts.is_finite() && base.ts > 0.0 && base.ts <= Layout::text_scale(dpi, &cfg) + 1e-6, "{ctx}: ts {}", base.ts);
            assert!(base.rows >= 3.0 && base.step > 0.0 && base.row_h > 0.0 && base.row_h <= base.step, "{ctx}: rows {} step {}", base.rows, base.step);
            assert!(base.list_top < base.list_bottom && base.list_bottom <= base.footer_top, "{ctx}: list {}..{}", base.list_top, base.list_bottom);
            assert!(base.button_top + base.button_h <= base.h + 0.01, "{ctx}: power bar leaves the window");
            if gui == 50 {
                assert!(base.ts < dpi, "{ctx}: smaller than the DPI scale");
            }
            if gui == 250 && work_h == 680 {
                assert!(base.ts < 2.5 * dpi, "{ctx}: capped so the settings card fits, ts {}", base.ts);
            }

            // Every visible row is inside the list area and the window.
            let count = base.rows as usize;
            for i in 0..count {
                let top = base.row_top(i as f32, 0.0, count);
                let (left, icon) = base.row_x(i as f32);
                assert!(top >= base.list_top - 0.01 && top + base.row_h <= base.list_bottom + 0.01, "{ctx}: row {i} top {top}");
                assert!(left >= -0.01 && icon + base.row_h <= base.w + 0.01, "{ctx}: row {i} x {left}..{}", icon + base.row_h);
            }

            // The power buttons are side by side inside the bar.
            let buttons: Vec<D2D_RECT_F> = (0..ACTIONS.len()).map(|i| base.button_rect(i)).collect();
            for (i, b) in buttons.iter().enumerate() {
                assert!(within(*b, rect(0.0, 0.0, base.w, base.h)) && b.right > b.left, "{ctx}: button {i}");
                assert!(buttons[i + 1..].iter().all(|o| !overlaps(*b, *o)), "{ctx}: button {i} overlaps another");
            }

            // The settings column fits the work area and the card holds its tabs, rows and buttons.
            let panel_w = Layout::panel_width(work_w, menu_w, dpi, base.ts);
            assert!(panel_w > 0 && menu_w + panel_w <= work_w - 2 * margin, "{ctx}: panel {panel_w} next to menu {menu_w}");
            for on_left in [false, true] {
                let l = base.with_panel(panel_w, on_left);
                assert_eq!(l.width, menu_w + panel_w);
                let window = rect(0.0, 0.0, l.width as f32, l.h);
                let card = l.card();
                assert!(within(card, window), "{ctx}: card {card:?}");
                let tabs: Vec<D2D_RECT_F> = (0..TABS.len()).map(|i| l.tab_rect(i)).collect();
                let rows: Vec<D2D_RECT_F> = (0..SLOTS).map(|i| l.setting_rect(i)).collect();
                let buttons = [l.config_button_rect(), l.style_button_rect()];
                let all: Vec<D2D_RECT_F> = tabs.iter().chain(&rows).chain(&buttons).copied().collect();
                for (i, r) in all.iter().enumerate() {
                    assert!(within(*r, card) && r.right > r.left && r.bottom > r.top, "{ctx}: card element {i} {r:?} outside {card:?}");
                    assert!(all[i + 1..].iter().all(|o| !overlaps(*r, *o)), "{ctx}: card element {i} overlaps another");
                }
                assert!(l.note_rect().bottom <= buttons[0].top + 0.01 && l.note_rect().top >= rows[SLOTS - 1].bottom - 0.01, "{ctx}: note line");
                assert!(within(l.close_rect(), card), "{ctx}: close");
            }
        }
    }
}
