use super::*;

fn bench_fixtures(shape: crate::style::Shape) -> Vec<(&'static str, Fixture)> {
    let mut list = Fixture::new(shape);
    list.open_ms = 90.0;
    let mut power = Fixture::new(shape).hover(Hover::Power(1));
    power.power.focused = Some(0);
    vec![
        ("list (easing in)", list),
        ("pop-out", Fixture::new(shape).popout(0)),
        ("settings panel", Fixture::new(shape).panel(1).hover(Hover::Panel(PanelHit::Arrow(0, 1)))),
        ("power bar hover", power),
        ("confirm dialog", Fixture::new(shape).dialog(ov::Focus::Execute).hover(Hover::Execute)),
    ]
}

#[test]
#[ignore = "benchmark: cargo test --release bench_frame -- --ignored --nocapture"]
fn bench_frame() {
    use crate::style::{Corner, Shape};
    const FRAMES: u32 = 200;
    let shapes = [
        ("square", Shape::default()),
        ("round", Shape { row_radius: 12.0, panel_radius: 12.0, tag_radius: 6.0, icon_radius: 6.0, ..Shape::default() }),
        ("chamfer", Shape { row_radius: 12.0, panel_radius: 12.0, tag_radius: 6.0, icon_radius: 6.0, corner: Corner::Chamfer, ..Shape::default() }),
    ];
    let names: Vec<&str> = bench_fixtures(Shape::default()).iter().map(|(n, _)| *n).collect();
    println!("avg ms/frame over {FRAMES} frames, 525x1010 @1.25");
    println!("{:<20}{:>10}{:>10}{:>10}", "case", shapes[0].0, shapes[1].0, shapes[2].0);
    let mut table = vec![[0.0f64; 3]; names.len()];
    for (column, (_, shape)) in shapes.iter().enumerate() {
        for (row, (_, fixture)) in bench_fixtures(*shape).into_iter().enumerate() {
            let mut renderer = Renderer::new(&fixture.l).unwrap();
            for _ in 0..10 {
                fixture.draw(&mut renderer);
            }
            let start = std::time::Instant::now();
            for _ in 0..FRAMES {
                fixture.draw(&mut renderer);
            }
            table[row][column] = start.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        }
    }
    for (name, row) in names.iter().zip(&table) {
        println!("{name:<20}{:>10.3}{:>10.3}{:>10.3}", row[0], row[1], row[2]);
    }
}

pub(super) fn hash_pixels(renderer: &Renderer) -> u64 {
    let device = renderer.device.as_ref().unwrap();
    let pixels = unsafe { std::slice::from_raw_parts(device.bits, device.size.0 as usize * device.size.1 as usize) };
    pixels.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, p| (h ^ *p as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Surfaces drawn with square corners, recorded before the shape work (this machine's fonts).
fn square_surfaces() -> Vec<Fixture> {
    let shape = crate::style::Shape::default();
    let mut list = Fixture::new(shape);
    list.open_ms = 90.0;
    let mut power = Fixture::new(shape).hover(Hover::Power(1));
    power.power.focused = Some(0);
    let mut v = vec![list, Fixture::new(shape), Fixture::new(shape).popout(1), Fixture::new(shape).popup().hover(Hover::Popup(1)), power];
    v.extend((0..TABS.len()).map(|tab| Fixture::new(shape).panel(tab).hover(Hover::Panel(PanelHit::Arrow(0, -1)))));
    v.push(Fixture::new(shape).wizard(0).hover(Hover::Panel(PanelHit::Next)));
    v.push(Fixture::new(shape).dialog(ov::Focus::Execute).hover(Hover::Execute));
    v.push(Fixture::new(shape).dialog(ov::Focus::Abort).hover(Hover::Abort));
    v.push(Fixture::new(shape).hover(Hover::Gear));
    v.push(Fixture::new(shape).panel(0).hover(Hover::Panel(PanelHit::Close)));
    v
}

#[test]
#[ignore = "prints the hashes square_corners_draw_exactly_as_before checks"]
fn print_square_hashes() {
    let hashes: Vec<String> = square_surfaces().iter().map(|f| format!("{:#018x}", hash_pixels(&f.render()))).collect();
    println!("HASHES {}", hashes.join(", "));
}

#[test]
fn square_corners_draw_exactly_as_before() {
    // Recorded with `print_square_hashes` before the shape work; fonts make them machine-specific.
    // Re-recorded for the settings tabs (SHAPE became LOOK, with GUI SCALE and the font rows; LAYOUT lost TEXT SIZE): entries 5-9 and 14; again for TITLE and SUBTITLE moving from BEHAVIOR to LOOK: entries 6 and 8; again for the font rows (one per text role, no ROLE selector): entry 6, the LOOK tab, whose scrollbar thumb got shorter; again for the [show] rows and TAG spacing rows in LOOK (entry 6), and for TAG chips being as wide as their text (every surface with rows; the dialogs have none). Again for the ROW NAME toggle in LOOK (entry 6), , the STYLE preset row leaving it, and the header toggles moving into a pop-out (all entry 6). With the [show] defaults the surfaces were pixel-identical before the chip fix.
    const BEFORE: [u64; 15] = [
        0x02680eee2693fc2d, 0x6f3892e3a37a66aa, 0x1835f2a5c51c61af, 0x8b4192803e48104b, 0x22c4d6247e150536, 0xa4ffb95c2c2f2f4e, 0xe71a161b861ece89,
        0xe703a6e57222b8e4, 0x56db206c8346f4fa, 0xebfd6be932d4319e, 0xb827e75b4778e5b9, 0xfdf38fea6b24459c, 0x3227f61bead6fcdf, 0x1b80164cfdc83a97,
        0x2b7743240c9f5644,
    ];
    let now: Vec<u64> = square_surfaces().iter().map(|f| hash_pixels(&f.render())).collect();
    assert_eq!(now, BEFORE, "a surface with radius 0 changed its pixels");
}

fn rounded(radius: f32, corner: crate::style::Corner) -> crate::style::Shape {
    crate::style::Shape { row_radius: radius, panel_radius: radius, tag_radius: radius, icon_radius: radius, corner, ..Default::default() }
}

const BOTH_CORNERS: [crate::style::Corner; 2] = [crate::style::Corner::Round, crate::style::Corner::Chamfer];

/// Every surface the menu draws, with the hovers and focus that add boxes.
fn all_surfaces(shape: crate::style::Shape) -> Vec<Fixture> {
    let new = || Fixture::new(shape);
    let mut v = Vec::new();
    let mut mid = new();
    mid.open_ms = 90.0;
    let mut actions = new();
    actions.menu.open_actions(None);
    let mut query = new();
    query.menu.set_query("app");
    query.menu.query.select_all();
    let mut bar = new().hover(Hover::Power(1));
    bar.power.focused = Some(0);
    let mut bar_both = new().hover(Hover::Power(0));
    bar_both.power.focused = Some(0);
    v.extend([new(), mid, actions, query, new().popout(1), new().popup().hover(Hover::Popup(0)), new().popup().hover(Hover::Popup(1)), bar, bar_both]);
    v.extend([new().hover(Hover::Power(3)), new().hover(Hover::Gear), new().panel(0).hover(Hover::Panel(PanelHit::Close))]);
    for tab in 0..TABS.len() {
        for slot in [0, 2, 3] {
            v.push(new().panel(tab).hover(Hover::Panel(PanelHit::Arrow(slot, 1))));
        }
        v.push(new().panel(tab).hover(Hover::Panel(PanelHit::Row(1))));
    }
    v.extend((0..crate::settings::WIZARD_STEPS).map(|step| new().wizard(step).hover(Hover::Panel(PanelHit::Next))));
    v.extend([
        new().dropdown(None).hover(Hover::Drop(DropHit::Item(2))),
        new().dropdown(Some(Role::Tag)).hover(Hover::Drop(DropHit::Size(1))),
        new().dropdown(Some(Role::Tag)).hover(Hover::Drop(DropHit::Item(3))),
    ]);
    v.extend([
        new().dialog(ov::Focus::Execute).hover(Hover::Execute),
        new().dialog(ov::Focus::Abort).hover(Hover::Abort),
        new().dialog(ov::Focus::Execute).hover(Hover::Abort),
    ]);
    v
}

/// Plain rectangles that are meant to stay plain: the readability backdrop, the full-screen
/// glitch effects, and the strips along a card's top edge, which `top_strip` keeps between the
/// corners. Matched against the text of the source line that asked for the rectangle.
const PLAIN_OK: [&str; 7] = [
    "st.header_backdrop",
    "rgba(0, 0.35)",
    "self.fill(block.rect",
    "x + (40.0 + random() * 160.0) * s",
    "strip_left, card.top, strip_right",
    "mark_right, card.top",
    "rgba(0x404040, 1.0)",
];

/// The painter's source files, to read back the line that asked for a plain rectangle.
const SOURCES: [(&str, &str); 6] = [
    ("render/header.rs", include_str!("../header.rs")),
    ("render/overlay.rs", include_str!("../overlay.rs")),
    ("render/painter.rs", include_str!("../painter.rs")),
    ("render/panel.rs", include_str!("../panel.rs")),
    ("render/rows.rs", include_str!("../rows.rs")),
    ("render/mod.rs", include_str!("../mod.rs")),
];

fn plain_boxes(f: &Fixture) -> Vec<String> {
    PLAIN_BOXES.with(|boxes| boxes.borrow_mut().clear());
    drop(f.render());
    let found = PLAIN_BOXES.with(|boxes| boxes.borrow().iter().map(|at| (at.file().replace('\\', "/"), at.line())).collect::<Vec<_>>());
    found
        .into_iter()
        .map(|(file, line)| {
            let (_, source) = SOURCES.iter().find(|(name, _)| file.ends_with(name)).unwrap_or_else(|| panic!("a plain box from {file}:{line}"));
            format!("{line}: {}", source.lines().nth(line as usize - 1).unwrap().trim())
        })
        .collect()
}

#[test]
fn with_a_radius_every_box_goes_through_the_shaped_path() {
    for corner in BOTH_CORNERS {
        let mut seen_backdrop = false;
        for (n, f) in all_surfaces(rounded(12.0, corner)).iter().enumerate() {
            for line in plain_boxes(f) {
                let allowed = PLAIN_OK.iter().any(|ok| line.contains(ok));
                assert!(allowed, "{corner:?} surface {n} draws a square box at {line}");
                seen_backdrop |= line.contains("st.header_backdrop");
            }
        }
        assert!(seen_backdrop, "{corner:?}: the recorder sees the plain boxes that are allowed");
    }
    // The guard has teeth: without a radius the same surfaces do draw plain boxes.
    let square: usize = all_surfaces(crate::style::Shape::default()).iter().map(|f| plain_boxes(f).iter().filter(|l| !PLAIN_OK.iter().any(|ok| l.contains(ok))).count()).sum();
    assert!(square > 100, "only {square} square boxes at radius 0");
}

fn delta(a: u32, b: u32) -> i32 {
    (0..4).map(|i| (((a >> (8 * i)) & 0xFF) as i32 - ((b >> (8 * i)) & 0xFF) as i32).abs()).max().unwrap()
}

/// The pixel inside each corner of `r` that touches its bounding box.
fn corner_pixels(r: D2D_RECT_F) -> [(i32, i32); 4] {
    let (left, top, right, bottom) = (r.left.ceil() as i32, r.top.ceil() as i32, r.right.floor() as i32 - 1, r.bottom.floor() as i32 - 1);
    [(left, top), (right, top), (left, bottom), (right, bottom)]
}

fn invert(paint: &mut Paint) {
    *paint = Paint { rgb: !paint.rgb & 0xFF_FFFF, alpha: 1.0 };
}

/// Draws `make(false)` and `make(true)`, which differ only in the colour or state of one
/// element, and checks that the element shows up around `probe` and in none of the four corner
/// pixels of `target`: the corners are outside its curve or cut.
fn check_corners(label: &str, make: impl Fn(bool) -> Fixture, target: D2D_RECT_F, probe: (f32, f32)) {
    let (base, on) = (make(false).render(), make(true).render());
    for (x, y) in corner_pixels(target) {
        let d = delta(pixel(&base, x, y), pixel(&on, x, y));
        assert!(d <= 3, "{label}: the corner pixel ({x},{y}) of {target:?} is painted (delta {d})");
    }
    let (px, py) = (probe.0 as i32, probe.1 as i32);
    let centre = (-3..=3).flat_map(|dy| (-3..=3).map(move |dx| (px + dx, py + dy))).map(|(x, y)| delta(pixel(&base, x, y), pixel(&on, x, y))).max().unwrap();
    assert!(centre >= 8, "{label}: nothing is drawn around {probe:?} ({centre})");
}

/// Slot of a stepper row that is not the selected one.
fn spare_stepper(view: &PanelView) -> usize {
    let steppers = view.slots.iter().enumerate().filter(|(i, s)| matches!(s, Slot::Row(r) if r.stepper) && view.selected != Some(*i));
    steppers.map(|(i, _)| i).last().expect("a stepper row")
}

#[test]
fn corners_of_every_box_are_outside_the_curve_and_the_cut() {
    use crate::style::Shape;
    for corner in BOTH_CORNERS {
        let shape = rounded(12.0, corner);
        let new = move || Fixture::new(shape);
        let flipped = |f: &mut Fixture, on: bool, paint: fn(&mut Style) -> &mut Paint| {
            if on {
                invert(paint(&mut f.style));
            }
        };
        let l = new().l;
        let (k, ts) = (l.k, l.ts);
        let label = |what: &str| format!("{what} ({corner:?})");

        // Rows: the panel and the icon box of an unselected row, and its tag chip.
        let count = new().menu.view.len();
        let (panel_left, icon_left) = l.row_x(1.0);
        let top = l.row_top(1.0, 0.0, count);
        let panel = rect(panel_left, top, icon_left - 6.0 * k, top + l.row_h);
        let icon = rect(icon_left, top, icon_left + l.row_h, top + l.row_h);
        let row = |on: bool| {
            let mut f = new();
            flipped(&mut f, on, |s| &mut s.row_panel);
            f
        };
        check_corners(&label("row panel"), row, panel, center(panel));
        check_corners(&label("icon box"), row, icon, center(icon));
        let tag_width = (10.0 + 6.0 * t("tag.app").chars().count() as f32) * k;
        let tag = rect(panel_left + 14.0 * k, top + 33.0 * k, panel_left + 14.0 * k + tag_width, top + 47.0 * k);
        check_corners(
            &label("tag chip"),
            |on| {
                let mut f = new();
                flipped(&mut f, on, |s| &mut s.tag_app);
                f
            },
            tag,
            center(tag),
        );

        // Pop-out: the frame, and the highlighted item (concentric: its parent is the frame).
        let popout = |selected: usize| new().popout(selected);
        let p = popout(0).popout.unwrap();
        let frame = popout(0).l.popout_rect(&p);
        check_corners(
            &label("pop-out frame"),
            |on| {
                let mut f = popout(0);
                flipped(&mut f, on, |s| &mut s.popup_fill);
                f
            },
            frame,
            center(frame),
        );
        let pad = POPOUT_PAD * ts;
        let item = rect(frame.left + pad, frame.top + pad, frame.right - pad, frame.top + pad + POPOUT_ITEM_H * ts);
        let last = p.items - 1;
        check_corners(&label("pop-out selected item"), |on| popout(if on { last } else { 0 }), item, center(item));

        // Right-click popup: the hovered line sits inside the popup's rounded edge.
        let popup = |hover: Option<Hover>| {
            let mut f = new().popup();
            f.hover = hover;
            f
        };
        let r = l.popup_rect((100.0, 60.0));
        let inset = POPUP_INSET * ts;
        let line0 = grow(rect(r.left, r.top, r.right, r.top + POPUP_H * ts), -inset);
        check_corners(&label("popup hover line 0"), |on| popup(on.then_some(Hover::Popup(0))), line0, center(line0));
        let line1 = grow(rect(r.left, r.top + POPUP_H * ts, r.right, r.bottom), -inset);
        check_corners(&label("popup hover line 1"), |on| popup(on.then_some(Hover::Popup(1))), line1, center(line1));
        check_corners(
            &label("popup"),
            |on| {
                let mut f = popup(None);
                flipped(&mut f, on, |s| &mut s.popup_fill);
                f
            },
            r,
            center(r),
        );

        // Settings: the card, a row and its hovered arrow, the close button, a colour swatch.
        let settings = |tab: usize| new().panel(tab);
        let view_slot = spare_stepper(settings(1).view.as_ref().unwrap());
        let card = settings(1).l.card();
        check_corners(
            &label("settings card"),
            |on| {
                let mut f = settings(1);
                flipped(&mut f, on, |s| &mut s.panel_fill);
                f
            },
            card,
            (card.left + 6.0 * ts, (card.top + card.bottom) / 2.0),
        );
        let row_rect = settings(1).l.setting_rect(view_slot);
        check_corners(
            &label("settings row"),
            |on| {
                let mut f = settings(1);
                flipped(&mut f, on, |s| &mut s.panel_row_fill);
                f
            },
            row_rect,
            (row_rect.left + 4.0 * ts, (row_rect.top + row_rect.bottom) / 2.0),
        );
        let arrow = grow(settings(1).l.arrow_rect(view_slot, 1), -ARROW_INSET * ts);
        check_corners(
            &label("settings arrow hover"),
            |on| settings(1).hover_if(on, Hover::Panel(PanelHit::Arrow(view_slot, 1))),
            arrow,
            ((arrow.left + arrow.right) / 2.0, arrow.top + 8.0),
        );
        let close = settings(1).l.close_rect();
        check_corners(&label("close button hover"), |on| settings(1).hover_if(on, Hover::Panel(PanelHit::Close)), close, center(close));
        let colors = settings(0);
        let slot = colors.view.as_ref().unwrap().slots.iter().position(|s| matches!(s, Slot::Row(r) if r.color.is_some())).unwrap();
        let (row_rect, lo) = (colors.l.setting_rect(slot), colors.l.arrow_rect(slot, -1));
        let swatch = rect(lo.left - 22.0 * ts, row_rect.top + 12.0 * ts, lo.left - 6.0 * ts, row_rect.top + 28.0 * ts);
        check_corners(
            &label("colour swatch"),
            |on| {
                let mut f = new();
                flipped(&mut f, on, |s| &mut s.header_backdrop);
                f.panel(0)
            },
            swatch,
            center(swatch),
        );

        // Font dropdown: its frame, and a hovered entry inside it (concentric with the frame).
        let dropdown = |on_hover: Option<Hover>| new().dropdown(Some(Role::Tag)).hover_if(on_hover.is_some(), on_hover.unwrap_or(Hover::Gear));
        let drop_parts = |f: &Fixture| f.l.dropdown_parts(f.drop_geom());
        let outer = drop_parts(&dropdown(None)).outer;
        check_corners(
            &label("font dropdown"),
            |on| {
                let mut f = dropdown(None);
                flipped(&mut f, on, |s| &mut s.popup_fill);
                f
            },
            outer,
            (outer.left + 3.0 * ts, (outer.top + outer.bottom) / 2.0),
        );
        let base = dropdown(None);
        let entry = base.l.dropdown_item(&drop_parts(&base), 3);
        check_corners(&label("font dropdown entry"), |on| dropdown(on.then_some(Hover::Drop(DropHit::Item(3)))), entry, center(entry));

        // Header: the gear's hover, the power buttons.
        let gear = l.gear_rect();
        check_corners(&label("gear hover"), |on| new().hover_if(on, Hover::Gear), gear, center(gear));
        let button = l.button_rect(1);
        check_corners(
            &label("power button"),
            |on| {
                let mut f = new();
                flipped(&mut f, on, |s| &mut s.power_fill);
                f
            },
            button,
            center(button),
        );

        // Confirm dialog: the button; the card is checked by alpha, its dim backdrop has its colour.
        let dialog = Dialog::new(1000.0, 700.0, ts);
        check_corners(
            &label("dialog button"),
            |on| {
                let mut f = new().dialog(ov::Focus::Abort);
                flipped(&mut f, on, |s| &mut s.overlay_button_fill);
                f
            },
            dialog.execute,
            center(dialog.execute),
        );
        let renderer = new().dialog(ov::Focus::Abort).render();
        let dim = (DIM_ALPHA * 255.0).round() as i32;
        for (x, y) in corner_pixels(dialog.card) {
            assert!((pixel(&renderer, x, y) >> 24) as i32 <= dim + 12, "{}: dialog card corner ({x},{y})", label("card"));
        }
        let (cx, cy) = center(dialog.card);
        assert!(pixel(&renderer, cx as i32, cy as i32) >> 24 >= 245, "the dialog card is opaque in the middle");
    }

    // Control: without a radius the same corners are painted, so the check above can fail.
    let plain = |on: bool| {
        let mut f = Fixture::new(Shape::default());
        if on {
            invert(&mut f.style.row_panel);
        }
        f
    };
    let l = plain(false).l;
    let top = l.row_top(1.0, 0.0, 20);
    let corner = corner_pixels(rect(l.row_x(1.0).0, top, l.row_x(1.0).1 - 6.0 * l.k, top + l.row_h))[0];
    let (a, b) = (plain(false).render(), plain(true).render());
    assert!(delta(pixel(&a, corner.0, corner.1), pixel(&b, corner.0, corner.1)) > 8, "square rows do paint their corners");
}

/// Strips and thin lines that sit inside a rounded parent never touch a pixel outside it.
fn check_nothing_outside(label: &str, make: impl Fn(bool) -> Fixture) {
    let (base, on) = (make(false).render(), make(true).render());
    let device = base.device.as_ref().unwrap();
    let (w, h) = device.size;
    let mut checked = 0;
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (pixel(&base, x, y), pixel(&on, x, y));
            if a >> 24 <= 2 {
                assert!(b >> 24 <= 2, "{label}: pixel ({x},{y}) outside every shape is painted ({a:#010x} -> {b:#010x})");
                checked += 1;
            }
        }
    }
    assert!(checked > 1000, "{label}: nothing to check");
}

fn with_alpha(paint: &mut Paint, alpha: f32) {
    *paint = Paint { rgb: 0xFF00FF, alpha };
}

#[test]
fn scanlines_strips_and_the_scrollbar_stay_inside_their_shapes() {
    for corner in BOTH_CORNERS {
        for radius in [1.0, 6.0, 12.0, 24.0] {
            let shape = rounded(radius, corner);
            let name = |what: &str| format!("{what}, radius {radius} {corner:?}");
            check_nothing_outside(&name("row scanlines"), |on| {
                let mut f = Fixture::new(shape);
                with_alpha(&mut f.style.row_scanline, if on { 1.0 } else { 0.0 });
                f
            });
            check_nothing_outside(&name("row scanlines mid-animation"), |on| {
                let mut f = Fixture::new(shape);
                f.open_ms = 90.0;
                with_alpha(&mut f.style.row_scanline, if on { 1.0 } else { 0.0 });
                f
            });
            check_nothing_outside(&name("panel strips"), |on| {
                let mut f = Fixture::new(shape);
                let alpha = if on { 1.0 } else { 0.0 };
                with_alpha(&mut f.style.panel_strip, alpha);
                with_alpha(&mut f.style.panel_strip_mark, alpha);
                f.panel(0)
            });
            check_nothing_outside(&name("scrollbar"), |on| {
                let mut f = Fixture::new(shape);
                let alpha = if on { 1.0 } else { 0.0 };
                with_alpha(&mut f.style.panel_frame, alpha);
                with_alpha(&mut f.style.panel_arrow, alpha);
                f.panel(0)
            });
            // The dialog sits on the dim backdrop, so look for the strip's bright pixels outside the card.
            let renderer = Fixture::new(shape).dialog(ov::Focus::Abort).render();
            let card = Dialog::new(1000.0, 700.0, 1.25).card;
            let reach = shape.panel_radius.min(24.0) * 1.25;
            let mut checked = 0;
            for y in card.top as i32..(card.top + reach) as i32 + 1 {
                for x in card.left as i32..(card.left + reach) as i32 + 1 {
                    // Signed distance outside the card's corner curve or cut, at the pixel centre.
                    let (qx, qy) = (card.left + reach - (x as f32 + 0.5), card.top + reach - (y as f32 + 0.5));
                    let outside = if qx > 0.0 && qy > 0.0 {
                        if corner == crate::style::Corner::Round { qx.hypot(qy) - reach } else { (qx + qy - reach) / std::f32::consts::SQRT_2 }
                    } else {
                        -1.0
                    };
                    if outside >= 1.5 {
                        assert!(pixel(&renderer, x, y) >> 24 <= 225, "{}: pixel ({x},{y}) outside the dialog card is bright", name("dialog strip"));
                        checked += 1;
                    }
                }
            }
            assert!(checked > 0 || radius < 12.0, "{}: no pixel outside the corner to check", name("dialog strip"));
        }
    }
}

#[test]
fn kept_text_layouts_draw_exactly_like_fresh_ones() {
    // `kept` lays strings out once and resizes them for the next frame; every frame is also drawn with `DrawText`.
    let scrolls = [0.0, 0.4, 1.0, 2.5, 7.3, 0.4, 7.3, 19.0];
    for shape in [crate::style::Shape::default(), rounded(12.0, crate::style::Corner::Chamfer)] {
        let fixtures = |scroll: f32| {
            let mut list = Fixture::new(shape);
            list.menu.scroll = scroll;
            let mut query = Fixture::new(shape);
            query.menu.set_query("app");
            query.menu.scroll = scroll;
            vec![list, query, Fixture::new(shape).panel(1), Fixture::new(shape).popout(1), Fixture::new(shape).dropdown(Some(Role::Tag)), Fixture::new(shape).dialog(ov::Focus::Execute)]
        };
        let mut kept = Renderer::new(&Fixture::new(shape).l).unwrap();
        for scroll in scrolls {
            for (n, f) in fixtures(scroll).iter().enumerate() {
                f.draw(&mut kept);
                FRESH_TEXT.set(true);
                let fresh = hash_pixels(&f.render());
                FRESH_TEXT.set(false);
                assert_eq!(hash_pixels(&kept), fresh, "surface {n} at scroll {scroll}");
            }
        }
        assert!(kept.formats.name.layouts.borrow().len() > 1, "the names were not kept");
        kept.release();
        assert!(kept.formats.name.layouts.borrow().is_empty(), "release frees the laid-out strings");
    }
}

#[test]
#[ignore = "benchmark: cargo test --release bench_scroll -- --ignored --nocapture"]
fn bench_scroll() {
    use crate::style::{Corner, Shape};
    const FRAMES: u32 = 200;
    let shapes = [
        ("square", Shape::default()),
        ("round", Shape { row_radius: 12.0, panel_radius: 12.0, tag_radius: 6.0, icon_radius: 6.0, ..Shape::default() }),
        ("chamfer", Shape { row_radius: 12.0, panel_radius: 12.0, tag_radius: 6.0, icon_radius: 6.0, corner: Corner::Chamfer, ..Shape::default() }),
    ];
    println!("avg ms/frame over {FRAMES} frames of the list scrolling (every frame another position), 525x1010 @1.25");
    println!("{:<20}{:>10}{:>10}{:>10}", "case", shapes[0].0, shapes[1].0, shapes[2].0);
    for (label, fresh) in [("DrawText", true), ("kept layouts", false)] {
        let mut cells = Vec::new();
        for (_, shape) in &shapes {
            let mut fixture = Fixture::new(*shape);
            fixture.menu = many_items(40);
            let mut renderer = Renderer::new(&fixture.l).unwrap();
            FRESH_TEXT.set(fresh);
            for frame in 0..10 {
                fixture.menu.scroll = (frame as f32 * 0.07) % 30.0;
                fixture.draw(&mut renderer);
            }
            let start = std::time::Instant::now();
            for frame in 0..FRAMES {
                fixture.menu.scroll = (frame as f32 * 0.07) % 30.0;
                fixture.draw(&mut renderer);
            }
            cells.push(start.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64);
            FRESH_TEXT.set(false);
        }
        println!("{label:<20}{:>10.3}{:>10.3}{:>10.3}", cells[0], cells[1], cells[2]);
    }
}
