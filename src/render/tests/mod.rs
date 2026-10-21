use super::*;
use super::panel::{ARROW_INSET, POPUP_INSET};

mod drawing;
mod fonts;
mod geometry;
mod shapes;
mod show;

fn layout(width: i32, height: i32, scale: f32, rows: u32, arc: i32) -> Layout {
    Layout::new(width, height, scale, &LayoutConfig { visible_rows: rows, arc_shift: arc, ..Default::default() })
}

fn pixel(renderer: &Renderer, x: i32, y: i32) -> u32 {
    let device = renderer.device.as_ref().unwrap();
    unsafe { *device.bits.add((y * device.size.0 + x) as usize) }
}

fn power_frame(phase: ov::Phase, frame_no: u32) -> PowerFrame {
    PowerFrame {
        look: phase.look(true),
        action: Action::Shutdown,
        screen: (800, 600),
        origin: (20.0, 20.0),
        hover: None,
        focus: ov::Focus::Abort,
        frame_no,
    }
}

fn center(r: D2D_RECT_F) -> (f32, f32) {
    ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0)
}

fn popout(anchor: ActionsAnchor, side: Side, offset: (i32, i32)) -> PopoutParams {
    PopoutParams { items: 5, selected: 0, width: 170.0, row: 2, scroll: 2.0, rows: 10, anchor, side, cursor: None, offset }
}

fn shaped(row_height: f32, row_gap: f32) -> LayoutConfig {
    LayoutConfig { shape: crate::style::Shape { row_height, row_gap, ..Default::default() }, ..Default::default() }
}

/// A menu with `n` app rows.
fn many_items(n: usize) -> Menu {
    let mut menu = Menu::default();
    let items = (0..n).map(|i| Item { name: format!("App {i:02}"), tag: Tag::App, id: format!("id{i}"), icon: None, extra: None });
    menu.set_apps(items.collect(), &[]);
    menu
}

/// Everything one offscreen frame needs, at 525x1010 and scale 1.25.
struct Fixture {
    style: Style,
    l: Layout,
    menu: Menu,
    power: PowerBar,
    view: Option<PanelView>,
    popup: Option<(f32, f32)>,
    hover: Option<Hover>,
    popout: Option<PopoutParams>,
    open_ms: f32,
    /// The confirm dialog (full-screen frame) with its keyboard focus; `hover` applies to it too.
    dialog: Option<ov::Focus>,
}

impl Fixture {
    fn new(shape: crate::style::Shape) -> Self {
        let mut style = Style::from_theme(&crate::config::Config::default().colors);
        style.shape = shape;
        let l = Layout::new(525, 1010, 1.25, &LayoutConfig { shape, ..Default::default() });
        Self { style, l, menu: many_items(20), power: PowerBar::default(), view: None, popup: None, hover: None, popout: None, open_ms: SETTLED_MS, dialog: None }
    }

    fn panel(mut self, tab: usize) -> Self {
        let mut panel = crate::settings::Panel::default();
        panel.select_tab(tab);
        self.view = Some(panel.view(&crate::config::Config::default(), &self.style, false));
        self.l = self.l.with_panel(Layout::panel_width(1920, 525, 1.25, self.l.ts), false);
        self
    }

    fn wizard(mut self, step: usize) -> Self {
        let mut panel = crate::settings::Panel::default();
        panel.start_wizard(true);
        for _ in 0..step {
            panel.wizard_next();
        }
        self.view = Some(panel.view(&crate::config::Config::default(), &self.style, false));
        self.l = self.l.with_panel(Layout::panel_width(1920, 525, 1.25, self.l.ts), false).with_wizard(true);
        self
    }

    fn popout(mut self, selected: usize) -> Self {
        self.menu.open_actions(None);
        self.menu.set_popout();
        let items = self.menu.popout().unwrap().labels().len();
        let p = PopoutParams {
            items,
            selected,
            width: Layout::popout_width(120.0, self.l.ts),
            row: 2,
            scroll: 0.0,
            rows: 20,
            anchor: ActionsAnchor::Row,
            side: Side::Left,
            cursor: None,
            offset: (0, 0),
        };
        self.l = self.l.with_popout(Layout::popout_column(self.l.popout_overhang(&p), 1920, 525, 1.25), false);
        self.popout = Some(p);
        self
    }

    fn popup(mut self) -> Self {
        self.popup = Some((100.0, 60.0));
        self
    }

    fn dialog(mut self, focus: ov::Focus) -> Self {
        self.dialog = Some(focus);
        self
    }

    fn hover(mut self, hover: Hover) -> Self {
        self.hover = Some(hover);
        self
    }

    fn hover_if(self, on: bool, hover: Hover) -> Self {
        if on { self.hover(hover) } else { self }
    }

    fn draw(&self, renderer: &mut Renderer) {
        if let Some(focus) = self.dialog {
            let mut f = power_frame(ov::Phase::Confirm, 1);
            (f.screen, f.focus, f.hover) = ((1000, 700), focus, self.hover);
            renderer.draw_power(&self.menu, &self.power, &self.style, &self.l, &f).unwrap();
        } else {
            let overlay = Overlay { panel: self.view.as_ref(), popup: self.popup, hover: self.hover, popout: self.popout };
            renderer.draw(&self.menu, &self.power, &self.style, &self.l, &overlay, self.open_ms, 3).unwrap();
        }
    }

    fn render(&self) -> Renderer {
        let mut renderer = Renderer::new(&self.l).unwrap();
        self.draw(&mut renderer);
        renderer
    }
}

fn within(inner: D2D_RECT_F, outer: D2D_RECT_F) -> bool {
    inner.left >= outer.left - 0.01 && inner.top >= outer.top - 0.01 && inner.right <= outer.right + 0.01 && inner.bottom <= outer.bottom + 0.01
}

/// `n` family names: the first installed ones, then made-up ones (a format for those falls back).
fn font_list(n: usize) -> Vec<String> {
    let mut list = crate::fonts::installed_now();
    list.truncate(n);
    let have = list.len();
    list.extend((have..n).map(|i| format!("Test Family {i:03}")));
    list
}

/// A LOOK panel with the dropdown of the global family (`None`) or of `role` open.
fn dropdown_panel(role: Option<Role>, families: Vec<String>, style: &Style) -> crate::settings::Panel {
    let mut panel = crate::settings::Panel::default();
    panel.fonts = families;
    panel.select_tab(1);
    let row = role.map_or(crate::settings::Row::FontFamily, crate::settings::Row::FontRole);
    let at = panel.rows().iter().position(|&r| r == row).unwrap();
    panel.select(at);
    panel.open_dropdown(&style.fonts);
    panel
}

impl Fixture {
    /// The LOOK tab with a font dropdown open over it, on a list of 300 families.
    fn dropdown(mut self, role: Option<Role>) -> Self {
        let panel = dropdown_panel(role, font_list(300), &self.style);
        self = self.panel(1);
        self.view = Some(panel.view(&crate::config::Config::default(), &self.style, false));
        self
    }

    fn drop_geom(&self) -> DropGeom {
        let dropdown = self.view.as_ref().and_then(|v| v.dropdown.as_ref()).expect("a dropdown is open");
        DropGeom { slot: dropdown.slot, role: dropdown.role }
    }
}
