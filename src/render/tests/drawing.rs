use super::*;

/// Alpha channel of the pixel at (x, y) of the last drawn frame.
fn alpha_at(renderer: &Renderer, x: i32, y: i32) -> u8 {
    let device = renderer.device.as_ref().unwrap();
    let mut info = BITMAP::default();
    unsafe {
        GetObjectW(device.surface.bitmap.into(), size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _));
        let pixels = std::slice::from_raw_parts(info.bmBits as *const u8, (info.bmWidthBytes * info.bmHeight) as usize);
        pixels[(y * info.bmWidthBytes + x * 4 + 3) as usize]
    }
}

#[test]
fn first_row_glow_stays_clear_of_the_header_backdrop() {
    for (h, scale) in [(1012, 1.0), (1500, 1.5)] {
        let l = layout(480, h, scale, 0, 40);
        let glow_top = l.list_top - 6.0 * l.k - 2.0 * l.k;
        assert!(glow_top >= 60.0 * scale + 4.0 * scale, "glow {glow_top} vs backdrop {}", 60.0 * scale);
    }
}

#[test]
fn header_backdrop_is_drawn_with_and_without_the_panel() {
    let cfg = LayoutConfig::default();
    let theme = crate::style::Style::from_theme(&crate::config::Theme::preset("cyberpunk").unwrap());
    for (panel_w, on_left) in [(0, false), (340, false), (340, true)] {
        let l = Layout::new(480, 1000, 1.0, &cfg).with_panel(panel_w, on_left);
        let mut renderer = Renderer::new(&l).unwrap();
        for open_ms in [0.0, 90.0, 1000.0] {
            renderer.draw(&Menu::default(), &PowerBar::default(), &theme, &l, &Overlay::default(), open_ms, 3).unwrap();
            assert_eq!(alpha_at(&renderer, l.menu_x() as i32 + 12, 30), 153, "open_ms {open_ms}");
        }
        let x = l.menu_x() as i32 + 240;
        assert_eq!(alpha_at(&renderer, x, 50), 153, "0.6 of 255 under the header, panel {panel_w} left {on_left}");
        // Never fully transparent: such pixels would let the mouse wheel through to the window behind.
        assert_eq!(alpha_at(&renderer, x, 62), 1, "near-invisible between header and list");
        assert_eq!(alpha_at(&renderer, x, 900), 1);
    }
}

#[test]
fn colors_panel_draws_headings_swatches_and_scrollbar() {
    use crate::config::Config;
    use crate::settings::Panel;
    let cfg = LayoutConfig::default();
    let config = Config::default();
    let style = crate::style::Style::from_theme(&config.colors);
    let l = Layout::new(480, 1000, 1.0, &cfg).with_panel(340, false);
    let mut renderer = Renderer::new(&l).unwrap();
    let mut panel = Panel::default();
    panel.move_selection(3);
    let view = panel.view(&config, &style, false);
    assert!(view.scroll.is_some() && view.selected.is_some());
    let slots: Vec<bool> = panel.row_slots().iter().map(Option::is_some).collect();
    let hover = l.hover_at(l.arrow_rect(3, 1).left + 2.0, l.arrow_rect(3, 1).top + 2.0, &slots, None);
    assert_eq!(hover, Some(Hover::Panel(PanelHit::Arrow(3, 1))));
    let overlay = Overlay { panel: Some(&view), popup: None, hover, popout: None };
    renderer.draw(&Menu::default(), &PowerBar::default(), &style, &l, &overlay, 1000.0, 3).unwrap();
    // The first color row (slot 3, after THEME, SAVE AS THEME and the HEADER heading) shows its swatch in the theme's color.
    let swatch = l.setting_rect(3);
    let (x, y) = ((l.arrow_rect(3, -1).left - 14.0) as i32, ((swatch.top + swatch.bottom) / 2.0) as i32);
    let backdrop = style.header_backdrop;
    assert_eq!(pixel(&renderer, x, y) & 0x00FF_FFFF, backdrop.rgb, "swatch is filled with the key's color");
}

#[test]
fn a_half_opaque_panel_color_gives_a_half_alpha_pixel() {
    let alpha_at = |alpha: f32| {
        let mut f = Fixture::new(crate::style::Shape::default()).panel(0);
        f.style.panel_fill.alpha = alpha;
        let card = f.l.card();
        pixel(&f.render(), (card.left + 4.0) as i32, (card.top + 100.0) as i32) >> 24
    };
    let (full, half) = (alpha_at(1.0), alpha_at(0.5));
    assert!(full > 200, "{full}");
    assert!((half as i32 * 2 - full as i32).abs() <= 3, "{half} vs {full}");
}

#[test]
fn the_ui_font_has_every_character_the_polish_strings_use() {
    let mut needed: Vec<char> = crate::lang::polish_text().chars().filter(|c| c.is_alphabetic() && !c.is_ascii()).collect();
    needed.sort();
    needed.dedup();
    assert!(needed.contains(&'Ł') && needed.contains(&'Ż'), "{needed:?}");
    unsafe {
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
        let mut collection = None;
        dwrite.GetSystemFontCollection(&mut collection, false).unwrap();
        let collection = collection.unwrap();
        let (mut index, mut exists) = (0, windows::core::BOOL::default());
        collection.FindFamilyName(w!("Bahnschrift"), &mut index, &mut exists).unwrap();
        assert!(exists.as_bool(), "Bahnschrift is installed");
        let font = collection
            .GetFontFamily(index)
            .unwrap()
            .GetFirstMatchingFont(DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_FONT_STRETCH_SEMI_CONDENSED, DWRITE_FONT_STYLE_NORMAL)
            .unwrap();
        for c in needed {
            assert!(font.HasCharacter(c as u32).unwrap().as_bool(), "Bahnschrift lacks {c}");
        }
    }
}

#[test]
fn polish_text_draws_on_every_tab() {
    use crate::config::Config;
    use crate::settings::Panel;
    crate::lang::use_builtin("pl");
    let config = Config::default();
    let style = crate::style::Style::from_theme(&config.colors);
    let l = Layout::new(480, 1000, 1.0, &LayoutConfig::default()).with_panel(340, false);
    let mut renderer = Renderer::new(&l).unwrap();
    for tab in 0..TABS.len() {
        let mut panel = Panel::default();
        panel.select_tab(tab);
        let view = panel.view(&config, &style, true);
        let overlay = Overlay { panel: Some(&view), popup: Some((100.0, 20.0)), hover: None, popout: None };
        renderer.draw(&Menu::default(), &PowerBar::default(), &style, &l, &overlay, 1000.0, 3).unwrap();
    }
    crate::lang::use_builtin("en");
}

#[test]
fn wizard_draws_every_step_in_both_languages_and_its_buttons_are_hit_targets() {
    use crate::config::Config;
    use crate::settings::{Panel, WIZARD_STEPS};
    let config = Config::default();
    let style = crate::style::Style::from_theme(&config.colors);
    let l = Layout::new(480, 1000, 1.0, &LayoutConfig::default()).with_panel(340, false).with_wizard(true);
    let mut renderer = Renderer::new(&l).unwrap();
    for code in ["pl", "en"] {
        crate::lang::use_builtin(code);
        let mut panel = Panel::default();
        panel.start_wizard(true);
        for step in 0..WIZARD_STEPS {
            let view = panel.view(&config, &style, false);
            assert!(view.wizard.is_some() && view.button.is_none(), "{code} step {step}");
            let overlay = Overlay { panel: Some(&view), popup: None, hover: Some(Hover::Panel(PanelHit::Next)), popout: None };
            renderer.draw(&Menu::default(), &PowerBar::default(), &style, &l, &overlay, 1000.0, 3).unwrap();
            panel.wizard_next();
        }
    }
    crate::lang::use_builtin("en");
    let at = |i: usize| {
        let r = l.wizard_button(i);
        ((r.left + r.right) / 2.0 + l.menu_x(), (r.top + r.bottom) / 2.0)
    };
    for (i, hit) in [PanelHit::Back, PanelHit::Skip, PanelHit::Next].into_iter().enumerate() {
        assert_eq!(l.panel_hit(at(i).0, at(i).1, &[true]), Some(hit));
    }
    let tab = center(l.tab_rect(0));
    assert_eq!(l.panel_hit(tab.0, tab.1, &[true]), None, "no tabs in the wizard");
    let row = center(l.setting_rect(0));
    assert_eq!(l.panel_hit(row.0 - 40.0, row.1, &[true]), Some(PanelHit::Row(0)));
}

#[test]
fn high_contrast_raises_only_faded_text() {
    assert_eq!(lifted(false, 0.55), 0.55);
    assert_eq!(lifted(true, 0.55), 0.9);
    assert_eq!(lifted(true, 1.0), 1.0, "already strong text is untouched");
    assert_eq!(row_text_lifted(false, 0.5), 0.5);
    assert_eq!(row_text_lifted(true, 0.5), 0.9, "faded by the arc");
    assert_eq!(row_text_lifted(true, 1.0), 1.0);
    assert_eq!(row_text_lifted(true, 0.0), 0.0, "an invisible row stays invisible");
    assert!(row_text_lifted(true, 0.1) < 0.9 && row_text_lifted(true, 0.1) >= 0.1, "ramps in near the end of the fade");
}

#[test]
fn high_contrast_and_big_text_draw_everywhere() {
    use crate::config::Config;
    use crate::settings::Panel;
    let config = Config::default();
    let style = crate::style::Style::from_theme(&config.colors);
    let cfg = LayoutConfig { gui_scale: 130, high_contrast: true, ..Default::default() };
    let l = Layout::new(480, 1000, 1.0, &cfg).with_panel(Layout::panel_width(1920, 480, 1.0, 1.3), false);
    let mut renderer = Renderer::new(&l).unwrap();
    let view = Panel::default().view(&config, &style, false);
    let hover = Some(Hover::Gear);
    let overlay = Overlay { panel: Some(&view), popup: Some((100.0, 20.0)), hover, popout: None };
    let power = PowerBar { focused: Some(1) };
    renderer.draw(&Menu::default(), &power, &style, &l, &overlay, 1000.0, 3).unwrap();
    let mut frame = power_frame(ov::Phase::Confirm, 1);
    frame.focus = ov::Focus::Execute;
    renderer.draw_power(&Menu::default(), &power, &style, &l, &frame).unwrap();
}

#[test]
fn direct_pixel_edits_between_draw_passes_survive() {
    let l = Layout::default();
    let mut renderer = Renderer::new(&l).unwrap();
    let style = crate::style::Style::from_theme(&crate::config::Theme::preset("cyberpunk").unwrap());
    renderer.prepare(&l, (800, 600), &style).unwrap();
    let device = renderer.device.as_ref().unwrap();
    unsafe {
        device.target.BeginDraw();
        device.target.Clear(Some(&rgba(0, 0.0)));
        device.target.EndDraw(None, None).unwrap();
        *device.bits.add(5 * 800 + 5) = 0xFF12_3456;
        device.target.BeginDraw();
        device.brush.SetColor(&rgba(0xFF0000, 1.0));
        device.target.FillRectangle(&rect(100.0, 100.0, 110.0, 110.0), &device.brush);
        device.target.EndDraw(None, None).unwrap();
        assert_eq!(*device.bits.add(5 * 800 + 5), 0xFF12_3456);
        assert_eq!(*device.bits.add(105 * 800 + 105), 0xFFFF_0000);
    }
}

#[test]
fn power_overlay_frames_draw_at_screen_size_and_release_on_the_next_menu_draw() {
    let l = Layout::new(480, 560, 1.0, &LayoutConfig::default());
    let style = crate::style::Style::from_theme(&crate::config::Theme::preset("cyberpunk").unwrap());
    let mut renderer = Renderer::new(&l).unwrap();
    let (menu, power) = (Menu::default(), PowerBar::default());
    for (i, phase) in [ov::Phase::Glitching(0.0), ov::Phase::Glitching(0.3), ov::Phase::Glitching(0.9), ov::Phase::Cancelling(0.5)]
        .into_iter()
        .enumerate()
    {
        renderer.draw_power(&menu, &power, &style, &l, &power_frame(phase, i as u32)).unwrap();
        assert_eq!(renderer.device.as_ref().unwrap().size, (800, 600));
    }
    // Settled: dim over the whole screen, an opaque dialog card in the middle, no menu.
    renderer.draw_power(&menu, &power, &style, &l, &power_frame(ov::Phase::Confirm, 9)).unwrap();
    let dim = (DIM_ALPHA * 255.0).round() as i32;
    assert!((((pixel(&renderer, 700, 1) >> 24) as i32) - dim).abs() <= 1, "dim over the screen");
    let card = Dialog::new(800.0, 600.0, 1.0).card;
    assert!(pixel(&renderer, card.left as i32 + 6, 300) >> 24 >= 250, "card is opaque");
    // The next ordinary menu draw swaps the big DIB for a menu-sized one.
    renderer.draw(&menu, &power, &style, &l, &Overlay::default(), 1000.0, 0).unwrap();
    assert_eq!(renderer.device.as_ref().unwrap().size, (480, 560));
}

#[test]
fn popout_draws_in_both_corner_shapes() {
    let l = layout(480, 1000, 1.0, 9, 40).with_popout(176, false);
    let mut renderer = Renderer::new(&l).unwrap();
    let labels: Vec<String> = ["RUN", "RUN AS ADMIN", "OPEN LOCATION", "COPY PATH", "PIN"].map(String::from).into();
    let width = renderer.popout_width(&labels, 1.0);
    assert!((150.0..=320.0).contains(&width));
    let menu = Menu::default();
    for corner in [crate::style::Corner::Round, crate::style::Corner::Chamfer] {
        let mut style = Style::from_theme(&crate::config::Config::default().colors);
        style.shape.corner = corner;
        let p = PopoutParams { width, ..popout(ActionsAnchor::Row, Side::Left, (0, 0)) };
        let overlay = Overlay { popout: Some(p), ..Overlay::default() };
        renderer.draw(&menu, &PowerBar::default(), &style, &l, &overlay, SETTLED_MS, 0).unwrap();
    }
}

fn two_items() -> Menu {
    let item = |name: &str, tag| Item { name: name.into(), tag, id: String::new(), icon: None, extra: None };
    let mut menu = Menu::default();
    menu.set_apps(vec![item("Alpha", Tag::App), item("Beta", Tag::App)], &[]);
    menu
}

fn draw_rows(cfg: &LayoutConfig, shape: crate::style::Shape) -> (Renderer, Layout) {
    let mut style = Style::from_theme(&crate::config::Theme::preset("cyberpunk").unwrap());
    style.shape = shape;
    let l = Layout::new(480, 1000, 1.0, &LayoutConfig { shape, ..*cfg });
    let mut renderer = Renderer::new(&l).unwrap();
    renderer.draw(&two_items(), &PowerBar::default(), &style, &l, &Overlay::default(), SETTLED_MS, 3).unwrap();
    (renderer, l)
}

#[test]
fn icons_off_removes_the_icon_box_and_the_row_uses_its_room() {
    let shape = crate::style::Shape::default();
    let gap_alpha = |icons: bool| {
        let (renderer, l) = draw_rows(&LayoutConfig { icons, ..Default::default() }, shape);
        // The second row: the selected one has a glow that reaches into the gap.
        let (_, icon_left) = l.row_x(1.0);
        let top = l.row_top(1.0, 0.0, 2);
        alpha_at(&renderer, (l.menu_x + icon_left - 3.0 * l.k) as i32, (top + l.row_h / 2.0) as i32)
    };
    assert_eq!(gap_alpha(true), 1, "icons on: empty gap between the panel and the icon box");
    assert!(gap_alpha(false) > 150, "icons off: the panel covers it");
}

#[test]
fn rounded_and_cut_corners_clear_the_corner_pixels_and_square_ones_do_not() {
    use crate::style::{Corner, Shape};
    let corner_alpha = |shape: Shape| {
        let (renderer, l) = draw_rows(&LayoutConfig::default(), shape);
        let (panel_left, icon_left) = l.row_x(0.0);
        let top = l.row_top(0.0, 0.0, 2);
        let at = |x: f32, y: f32| alpha_at(&renderer, (l.menu_x + x) as i32, y as i32);
        (at(panel_left, top), at(icon_left, top), at(panel_left + 40.0, top + l.row_h / 2.0))
    };
    let square = corner_alpha(Shape::default());
    assert!(square.0 > 50 && square.1 > 50, "square corners are filled: {square:?}");
    for corner in [Corner::Round, Corner::Chamfer] {
        let shaped = corner_alpha(Shape { row_radius: 20.0, icon_radius: 20.0, corner, ..Shape::default() });
        assert_eq!((shaped.0, shaped.1), (1, 1), "{corner:?}: the corner pixels are cut away");
        assert!(shaped.2 > 150, "{corner:?}: the middle of the row is still filled");
    }
    // Only the radius that applies moves: the icon box stays square with icon_radius 0.
    let only_rows = corner_alpha(Shape { row_radius: 20.0, ..Shape::default() });
    assert_eq!((only_rows.0, only_rows.1 > 50), (1, true));
}

#[test]
fn shapes_and_zero_borders_draw_on_every_surface() {
    use crate::config::Config;
    use crate::settings::Panel;
    use crate::style::{Corner, Shape};
    let config = Config::default();
    for corner in [Corner::Round, Corner::Chamfer] {
        let shape = Shape { row_radius: 12.0, panel_radius: 24.0, tag_radius: 6.0, icon_radius: 10.0, corner, border_width: 0.0, row_height: 70.0, row_gap: 4.0, ..Shape::default() };
        let mut style = Style::from_theme(&config.colors);
        style.shape = shape;
        let l = Layout::new(480, 1000, 1.0, &LayoutConfig { shape, ..Default::default() }).with_panel(340, false);
        let mut renderer = Renderer::new(&l).unwrap();
        let mut panel = Panel::default();
        panel.select_tab(1);
        let view = panel.view(&config, &style, false);
        let overlay = Overlay { panel: Some(&view), popup: Some((100.0, 60.0)), hover: None, popout: None };
        renderer.draw(&two_items(), &PowerBar::default(), &style, &l, &overlay, SETTLED_MS, 3).unwrap();
    }
}

#[test]
fn the_extreme_gui_scales_draw_with_the_panel_open() {
    let config = crate::config::Config::default();
    let style = Style::from_theme(&config.colors);
    for gui in [50, 250] {
        let cfg = LayoutConfig { gui_scale: gui, ..Default::default() };
        let (menu_w, height) = Layout::window_size(1920, 1040, 1.0, &cfg);
        let base = Layout::new(menu_w, height, 1.0, &cfg);
        let l = base.with_panel(Layout::panel_width(1920, menu_w, 1.0, base.ts), false);
        let mut renderer = Renderer::new(&l).unwrap();
        for tab in 0..TABS.len() {
            let mut panel = crate::settings::Panel::default();
            panel.select_tab(tab);
            let view = panel.view(&config, &style, false);
            let overlay = Overlay { panel: Some(&view), popup: Some((100.0, 20.0)), hover: Some(Hover::Gear), popout: None };
            renderer.draw(&many_items(20), &PowerBar { focused: Some(1) }, &style, &l, &overlay, SETTLED_MS, 3).unwrap();
        }
    }
}
