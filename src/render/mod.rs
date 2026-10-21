use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};
use windows::core::{HSTRING, Result, w};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::apps::{ICON, Icon, tint};
use crate::config::{ActionsAnchor, GUI_SCALE_MAX, GUI_SCALE_MIN, LayoutConfig, ShortList, Side};
use crate::lang::{self, t};
use crate::lineedit;
use crate::fonts as fontlist;
use crate::fontmenu::{DropGeom, DropdownView, Spot, VISIBLE};
use crate::togglemenu::TogglesView;
use crate::style::{Corner, DEFAULT_FAMILY, Fonts, Paint, Role, Show, Style, TextAlign, Weight};
use crate::menu::{Item, Menu, Tag};
use crate::power::{ACTIONS, Action, PowerBar};
use crate::power_overlay::{self as ov, Dialog, Look, Rng};
use crate::settings::{Button, PanelView, SLOTS, Slot, TABS};
use windows_numerics::{Matrix3x2, Vector2};

mod header;
mod layout;
mod overlay;
mod painter;
mod panel;
mod rows;
#[cfg(test)]
mod tests;

pub use layout::{DropHit, Hover, Layout, ListHit, POPUP_ENTRIES, PanelHit, PopoutParams};
use header::*;
use layout::*;
use overlay::*;
use painter::*;


const GLITCH_MS: f32 = 180.0;
const ROW_DELAY_MS: f32 = 28.0;
const ROW_IN_MS: f32 = 140.0;

const fn rgba(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a,
    }
}

/// Blend of two paints (`t` = 0 is `from`) at element opacity `a`.
fn mix(from: Paint, to: Paint, t: f32, a: f32) -> D2D1_COLOR_F {
    let (f, g) = (rgba(from.rgb, 1.0), rgba(to.rgb, 1.0));
    let alpha = a * (from.alpha + (to.alpha - from.alpha) * t);
    D2D1_COLOR_F { r: f.r + (g.r - f.r) * t, g: f.g + (g.g - f.g) * t, b: f.b + (g.b - f.b) * t, a: alpha }
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

fn contains(r: D2D_RECT_F, x: f32, y: f32) -> bool {
    (r.left..=r.right).contains(&x) && (r.top..=r.bottom).contains(&y)
}

fn grow(r: D2D_RECT_F, by: f32) -> D2D_RECT_F {
    rect(r.left - by, r.top - by, r.right + by, r.bottom + by)
}

/// Background alpha of the menu window: a fully transparent pixel of a layered window is
/// click-through (wheel and clicks go to the window behind), so the empty gaps get 1/255 instead,
/// which is invisible but keeps the whole window rectangle hit-testable.
const HIT_ALPHA: f32 = 1.0 / 255.0;

/// Opacity of the full-screen dim behind the power dialog.
const DIM_ALPHA: f32 = 0.8;
/// Lowest opacity of faded text with the high contrast setting.
const HIGH_CONTRAST_ALPHA: f32 = 0.9;

/// A faded text opacity, raised to at least 0.9 with high contrast.
fn lifted(high_contrast: bool, alpha: f32) -> f32 {
    if high_contrast { alpha.max(HIGH_CONTRAST_ALPHA) } else { alpha }
}

/// Text opacity of a list row whose own opacity is `a` (fade towards the list edges and the
/// open animation). With high contrast the text stays near-opaque until the row is almost
/// gone, so it doesn't grey out along the arc; a row at 0 stays invisible.
fn row_text_lifted(high_contrast: bool, a: f32) -> f32 {
    if high_contrast { a.max(HIGH_CONTRAST_ALPHA * (a / 0.3).min(1.0)) } else { a }
}

/// Open-animation time that is past every row's slide-in.
const SETTLED_MS: f32 = 1.0e6;

/// One frame of the full-screen power overlay.
pub struct PowerFrame {
    pub look: Look,
    pub action: Action,
    /// Screen (window) size in pixels.
    pub screen: (i32, i32),
    /// Where the menu window's top-left is on that screen.
    pub origin: (f32, f32),
    pub hover: Option<Hover>,
    /// The dialog button with keyboard focus.
    pub focus: ov::Focus,
    pub frame_no: u32,
}

/// What is drawn on top of the menu: the settings panel and the right-click popup (window
/// coordinates), and what the mouse is hovering.
#[derive(Default)]
pub struct Overlay<'a> {
    pub panel: Option<&'a PanelView>,
    pub popup: Option<(f32, f32)>,
    pub hover: Option<Hover>,
    pub popout: Option<PopoutParams>,
}

/// GDI DIB the software render target draws into; handed to `UpdateLayeredWindow`.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

// Field order matters: the target must be released before its surface.
struct Device {
    target: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    /// Per item index: (tinted if it was needed, color). Cleared when the item list or the accent colour changes.
    icons: RefCell<HashMap<usize, (Option<ID2D1Bitmap>, ID2D1Bitmap)>>,
    /// The font dropdown's entries, each set in its own family: formats by size and family, built
    /// when an entry first scrolls into view.
    previews: RefCell<fontlist::Lru<(Rc<Face>, bool)>>,
    /// Accent the cached tinted bitmaps were made with.
    tint_accent: Cell<u32>,
    surface: Surface,
    /// The DIB's pixels (premultiplied BGRA, top-down, `size.0` pixels per row), for the glitch
    /// passes that run between two `EndDraw`/`BeginDraw` pairs.
    bits: *mut u32,
    size: (i32, i32),
}

pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    formats: Formats,
    /// (scale, k, fonts) the formats were built for.
    fonts_for: (f32, f32, Fonts),
    device: Option<Device>,
}

impl Renderer {
    pub fn new(layout: &Layout) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = Fonts::default();
            let formats = Formats::new(&dwrite, layout.ts, layout.k, &fonts)?;
            Ok(Self { factory, dwrite, formats, fonts_for: (layout.ts, layout.k, fonts), device: None })
        }
    }

    /// Frees the bitmap and render target while hidden; recreated on the next draw.
    pub fn release(&mut self) {
        self.device = None;
        self.formats.forget_layouts();
    }

    /// The caret position of `query` nearest to `x` px from the left of the query line, or with
    /// `nearest` false the character under `x`. `scroll` is how far the text (not the "> ") is scrolled.
    pub fn query_caret_at(&self, query: &str, x: f32, nearest: bool, scroll: f32) -> usize {
        let (shown, offsets) = lineedit::shouted(query);
        let end = offsets.len() - 1;
        let Some(layout) = query_layout(&self.dwrite, &self.formats.query, &format!("> {shown}")) else { return end };
        let x = if x < prompt_width(&layout) { x } else { x + scroll };
        let (mut trailing, mut inside, mut m) = (Default::default(), Default::default(), DWRITE_HIT_TEST_METRICS::default());
        if unsafe { layout.HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m) }.is_err() {
            return end;
        }
        let unit = if nearest && x > m.left + m.width / 2.0 { m.textPosition + m.length } else { m.textPosition };
        lineedit::char_at_unit(&offsets, unit.saturating_sub(PROMPT_UNITS))
    }

    /// Pixel width of the actions pop-out for `labels`.
    pub fn popout_width(&self, labels: &[String], ts: f32) -> f32 {
        let widest = labels.iter().map(|label| text_width(&self.dwrite, &self.formats.label, label)).fold(0.0, f32::max);
        Layout::popout_width(widest, ts)
    }

    /// Item indexes changed meaning; drop bitmaps cached by index.
    pub fn clear_icons(&self) {
        if let Some(device) = &self.device {
            device.icons.borrow_mut().clear();
        }
    }

    fn create_device(&self, width: i32, height: i32) -> Result<Device> {
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) => bitmap,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e);
                }
            };
            let previous = SelectObject(dc, bitmap.into());
            let surface = Surface { dc, bitmap, previous };

            // 96 dpi explicitly: the default is the system DPI, which would scale our pixel coordinates.
            let target = self.factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            })?;
            target.BindDC(dc, &RECT { left: 0, top: 0, right: width, bottom: height })?;
            // ClearType needs an opaque background; the window is transparent.
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = target.CreateSolidColorBrush(&rgba(0xFFFFFF, 1.0), None)?;
            Ok(Device {
                target,
                brush,
                icons: RefCell::default(),
                previews: RefCell::new(fontlist::Lru::new(PREVIEW_CAP)),
                tint_accent: Cell::new(0),
                surface,
                bits: bits as *mut u32,
                size: (width, height),
            })
        }
    }

    pub fn draw(
        &mut self,
        menu: &Menu,
        power: &PowerBar,
        style: &Style,
        l: &Layout,
        overlay: &Overlay,
        open_ms: f32,
        frame_no: u32,
    ) -> Result<()> {
        self.prepare(l, (l.width, l.height), style)?;
        let device = self.device.as_ref().unwrap();
        let painter = Painter { factory: &self.factory, dwrite: &self.dwrite, device, rt: &device.target, brush: &device.brush, formats: &self.formats, l, s: style, hover: overlay.hover };
        unsafe {
            device.target.BeginDraw();
            device.target.Clear(Some(&rgba(0, HIT_ALPHA)));
            // The menu is drawn in its own coordinates; the panel column may sit to its left.
            painter.scene(menu, power, overlay, (l.menu_x, 0.0), open_ms, frame_no);
            let result = device.target.EndDraw(None, None);
            if result.is_err() {
                self.device = None;
            }
            result
        }
    }

    /// Fonts and a device of `size`, replacing a device of another size (the full-screen overlay's
    /// big DIB is dropped here as soon as the menu draws again).
    fn prepare(&mut self, l: &Layout, size: (i32, i32), style: &Style) -> Result<()> {
        if self.fonts_for != (l.ts, l.k, style.fonts) {
            self.formats = Formats::new(&self.dwrite, l.ts, l.k, &style.fonts)?;
            self.fonts_for = (l.ts, l.k, style.fonts);
        }
        if self.device.as_ref().is_some_and(|d| d.size != size) {
            self.device = None;
        }
        if self.device.is_none() {
            self.device = Some(self.create_device(size.0, size.1)?);
        }
        let device = self.device.as_ref().unwrap();
        if device.tint_accent.replace(style.row_icon.rgb) != style.row_icon.rgb {
            device.icons.borrow_mut().clear();
        }
        Ok(())
    }

    /// A frame of the full-screen power overlay: dim, the menu glitching on it, then scanlines,
    /// colored blocks and the confirm dialog. The window is `f.screen` big, the menu at `f.origin`.
    pub fn draw_power(&mut self, menu: &Menu, power: &PowerBar, style: &Style, l: &Layout, f: &PowerFrame) -> Result<()> {
        self.prepare(l, f.screen, style)?;
        let device = self.device.as_ref().unwrap();
        let painter = Painter { factory: &self.factory, dwrite: &self.dwrite, device, rt: &device.target, brush: &device.brush, formats: &self.formats, l, s: style, hover: f.hover };
        let look = f.look;
        let show_menu = look.menu >= 1.0 || (look.menu > 0.0 && Rng::new(f.frame_no, 3).unit() < look.menu);
        let result = unsafe {
            device.target.BeginDraw();
            device.target.Clear(Some(&painter.c(style.overlay_dim, DIM_ALPHA * look.dim)));
            if show_menu {
                painter.scene(menu, power, &Overlay::default(), f.origin, SETTLED_MS, f.frame_no);
            }
            device.target.EndDraw(None, None)
        };
        let result = result.and_then(|()| {
            if show_menu && look.glitch > 0.0 {
                glitch_menu(device, f.origin, (l.width, l.height), look, f.frame_no, l.scale);
            }
            unsafe {
                device.target.BeginDraw();
                painter.effects(look, f.frame_no, f.screen);
                if look.dialog > 0.0 {
                    painter.dialog(f, &Dialog::new(f.screen.0 as f32, f.screen.1 as f32, l.ts));
                }
                device.target.EndDraw(None, None)
            }
        });
        if result.is_err() {
            self.device = None;
        }
        result
    }

    pub fn present(&self, hwnd: HWND, position: POINT) -> Result<()> {
        let Some(device) = &self.device else { return Ok(()) };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&position),
                Some(&SIZE { cx: device.size.0, cy: device.size.1 }),
                Some(device.surface.dc),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }
}

impl Painter<'_> {
    /// The menu in its own coordinates at `origin`, then the settings panel and popup in window
    /// coordinates. Called between `BeginDraw` and `EndDraw`.
    unsafe fn scene(&self, menu: &Menu, power: &PowerBar, overlay: &Overlay, origin: (f32, f32), open_ms: f32, frame_no: u32) {
        let (l, st, f) = (self.l, self.s, self.formats);
        unsafe {
            self.rt.SetTransform(&Matrix3x2::translation(origin.0, origin.1));
            self.header(menu, open_ms, frame_no);
            self.rt.PushAxisAlignedClip(&rect(0.0, l.list_top - 8.0 * l.scale, l.w, l.list_bottom), D2D1_ANTIALIAS_MODE_ALIASED);
            let status = match (menu.items.is_empty(), menu.view.is_empty()) {
                (true, _) => Some(t("status.scanning")),
                (false, true) => Some(t("status.no_matches")),
                _ => None,
            };
            if let Some(status) = status {
                let (k, y) = (l.k, l.band_y);
                self.text(&status, &f.name, rect(l.margin + 14.0 * k, y + 14.0 * k, l.w, y + 40.0 * k), self.c(st.row_status, self.hc(0.6)));
            }
            if let Some(actions) = menu.list_actions() {
                for (row, act) in actions.acts.iter().enumerate() {
                    let label = actions.plugin.get(row).map_or_else(|| act.label(), |p| p.label.clone());
                    let item = Item { name: label, tag: Tag::Action, id: String::new(), icon: None, extra: None };
                    self.row(row, &item, false, (row, actions.scroll, actions.acts.len()), open_ms);
                }
            } else {
                for (row, &index) in menu.view.iter().enumerate() {
                    self.row(index, &menu.items[index], menu.is_recent(index), (row, menu.scroll, menu.view.len()), open_ms);
                }
            }
            self.rt.PopAxisAlignedClip();
            if l.show.power_bar {
                self.power_bar(power, open_ms);
            }
            self.rt.SetTransform(&Matrix3x2::identity());
            if let Some(view) = overlay.panel {
                self.panel(view);
                if let Some(dropdown) = &view.dropdown {
                    self.dropdown(dropdown);
                }
                if let Some(toggles) = &view.toggles {
                    self.toggles(toggles);
                }
            }
            if let Some(anchor) = overlay.popup {
                self.popup(anchor);
            }
            if let (Some(p), Some(actions)) = (&overlay.popout, menu.popout()) {
                self.popout(p, &actions.labels());
            }
        }
    }
}
