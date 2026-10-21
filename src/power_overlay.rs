//! Full-screen power overlay: its timeline, the glitch parameters per frame, pixel helpers for the
//! glitch and the confirm dialog's geometry. No Win32 calls here, so all of it is unit-tested.

use std::ops::Range;

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

use crate::power::Action;

/// Glitch before the confirm dialog settles.
pub const GLITCH_IN_MS: u64 = 500;
/// Glitch before an action that needs no confirmation runs.
pub const GLITCH_RUN_MS: u64 = 300;
/// Reverse glitch after ABORT.
pub const GLITCH_OUT_MS: u64 = 200;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Glitching in; progress 0..1.
    Glitching(f32),
    /// Dialog is up and waiting for a decision. Nothing animates.
    Confirm,
    /// Reverse glitch after ABORT; progress 0..1.
    Cancelling(f32),
    /// The action should run now.
    Executing,
    /// The reverse glitch is over: restore the menu window.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Decision {
    Execute,
    /// Tick at which ABORT was chosen.
    Cancel(u64),
}

/// Which dialog button has keyboard focus.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Focus {
    Execute,
    /// The safe default: a stray Enter cancels.
    #[default]
    Abort,
}

/// Keys that move the dialog's focus. EXECUTE is the left button, ABORT the right one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Nav {
    Left,
    Right,
    /// Tab and Shift+Tab: with two buttons both just switch.
    Switch,
}

impl Focus {
    pub fn moved(self, nav: Nav) -> Focus {
        match (nav, self) {
            (Nav::Left, _) => Focus::Execute,
            (Nav::Right, _) => Focus::Abort,
            (Nav::Switch, Focus::Execute) => Focus::Abort,
            (Nav::Switch, Focus::Abort) => Focus::Execute,
        }
    }
}

/// Whether a WM_KEYDOWN `lparam` is an auto-repeat (bit 30: the key was already down).
pub fn is_auto_repeat(lparam: isize) -> bool {
    lparam & (1 << 30) != 0
}

/// One run of the overlay for one power action.
#[derive(Debug)]
pub struct Sequence {
    action: Action,
    confirm: bool,
    start: u64,
    decision: Option<Decision>,
    focus: Focus,
}

impl Sequence {
    pub fn new(action: Action, confirm: bool, now: u64) -> Self {
        Self { action, confirm, start: now, decision: None, focus: Focus::default() }
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn navigate(&mut self, nav: Nav) {
        self.focus = self.focus.moved(nav);
    }

    pub fn action(&self) -> Action {
        self.action
    }

    pub fn confirm(&self) -> bool {
        self.confirm
    }

    pub fn phase(&self, now: u64) -> Phase {
        match self.decision {
            Some(Decision::Execute) => Phase::Executing,
            Some(Decision::Cancel(at)) => {
                let elapsed = now.saturating_sub(at);
                if elapsed >= GLITCH_OUT_MS { Phase::Done } else { Phase::Cancelling(elapsed as f32 / GLITCH_OUT_MS as f32) }
            }
            None => {
                let elapsed = now.saturating_sub(self.start);
                let total = if self.confirm { GLITCH_IN_MS } else { GLITCH_RUN_MS };
                if elapsed < total {
                    Phase::Glitching(elapsed as f32 / total as f32)
                } else if self.confirm {
                    Phase::Confirm
                } else {
                    Phase::Executing
                }
            }
        }
    }

    /// Whether a frame timer is needed: only the glitches move.
    pub fn animating(&self, now: u64) -> bool {
        matches!(self.phase(now), Phase::Glitching(_) | Phase::Cancelling(_))
    }

    /// EXECUTE; only while the dialog is waiting.
    pub fn execute(&mut self, now: u64) -> bool {
        let waiting = self.phase(now) == Phase::Confirm;
        if waiting {
            self.decision = Some(Decision::Execute);
        }
        waiting
    }

    /// ABORT; only while the dialog is waiting.
    pub fn cancel(&mut self, now: u64) -> bool {
        let waiting = self.phase(now) == Phase::Confirm;
        if waiting {
            self.decision = Some(Decision::Cancel(now));
        }
        waiting
    }
}

/// How the frame looks, each 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Strength of slices, color split, blocks, scanlines and flicker.
    pub glitch: f32,
    /// How much of the final dim has been reached.
    pub dim: f32,
    /// Dialog opacity.
    pub dialog: f32,
    /// Chance per frame that the menu is drawn at all: it stutters out when the dialog arrives.
    pub menu: f32,
}

fn ramp(p: f32, from: f32, to: f32) -> f32 {
    ((p - from) / (to - from)).clamp(0.0, 1.0)
}

impl Phase {
    pub fn look(self, confirm: bool) -> Look {
        match self {
            Phase::Glitching(p) if confirm => Look {
                glitch: (1.0 - p).sqrt(),
                dim: ramp(p, 0.0, 0.7),
                dialog: ramp(p, 0.6, 1.0),
                menu: 1.0 - ramp(p, 0.45, 0.8),
            },
            Phase::Glitching(p) => Look { glitch: 0.5 + 0.5 * p, dim: p, dialog: 0.0, menu: 1.0 - 0.5 * ramp(p, 0.6, 1.0) },
            Phase::Confirm => Look { glitch: 0.0, dim: 1.0, dialog: 1.0, menu: 0.0 },
            Phase::Cancelling(p) => Look {
                glitch: 0.8 * (1.0 - p),
                dim: 1.0 - p,
                dialog: 1.0 - ramp(p, 0.0, 0.4),
                menu: ramp(p, 0.2, 0.6),
            },
            Phase::Executing => Look { glitch: 1.0, dim: 1.0, dialog: if confirm { 1.0 } else { 0.0 }, menu: 0.0 },
            Phase::Done => Look { glitch: 0.0, dim: 0.0, dialog: 0.0, menu: 1.0 },
        }
    }
}

/// Small xorshift generator seeded from the frame number, so a frame looks the same however often
/// it is redrawn (hover redraws must not re-roll the glitch).
pub struct Rng(u32);

impl Rng {
    pub fn new(frame_no: u32, salt: u32) -> Self {
        Self(frame_no.wrapping_add(salt.wrapping_mul(0x9E37_79B9)).wrapping_mul(2_654_435_761) | 1)
    }

    /// Uniform in 0..1.
    pub fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 % 10_000) as f32 / 10_000.0
    }
}

/// A band of rows pushed sideways.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slice {
    pub y: usize,
    pub h: usize,
    pub dx: i32,
}

/// Bands within `height` rows for one frame; `strength` is `Look::glitch`.
pub fn slices(frame_no: u32, strength: f32, height: usize, scale: f32) -> Vec<Slice> {
    let mut rng = Rng::new(frame_no, 1);
    (0..(strength * 14.0).round() as usize)
        .map(|_| {
            let y = (rng.unit() * height as f32) as usize;
            let h = ((2.0 + rng.unit() * 28.0 * scale) as usize).min(height.saturating_sub(y));
            let dx = ((rng.unit() - 0.5) * 2.0 * 90.0 * scale * strength) as i32;
            Slice { y, h, dx }
        })
        .collect()
}

/// A flickering colored block; `color` indexes the three glitch colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub rect: D2D_RECT_F,
    pub color: usize,
}

pub fn blocks(frame_no: u32, strength: f32, width: f32, height: f32, scale: f32) -> Vec<Block> {
    let mut rng = Rng::new(frame_no, 2);
    (0..(strength * 9.0).round() as usize)
        .map(|_| {
            let (w, h) = ((30.0 + rng.unit() * 220.0) * scale, (4.0 + rng.unit() * 26.0) * scale);
            let (x, y) = (rng.unit() * (width - w).max(0.0), rng.unit() * (height - h).max(0.0));
            let color = ((rng.unit() * 3.0) as usize).min(2);
            Block { rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, color }
        })
        .collect()
}

/// Rotates the pixels of `rows` within columns `x0..x1` by `dx` (to the right when positive).
pub fn shift_rows(px: &mut [u32], stride: usize, rows: Range<usize>, x0: usize, x1: usize, dx: i32) {
    let width = x1.saturating_sub(x0);
    if width == 0 {
        return;
    }
    let by = dx.rem_euclid(width as i32) as usize;
    for y in rows {
        px[y * stride + x0..y * stride + x1].rotate_right(by);
    }
}

/// Chromatic aberration on premultiplied BGRA pixels: red is taken `dx` pixels to the left, blue
/// `dx` pixels to the right, green stays. Uniform areas are unchanged.
pub fn rgb_split(px: &mut [u32], stride: usize, rows: Range<usize>, x0: usize, x1: usize, dx: usize, scratch: &mut Vec<u32>) {
    if x1 <= x0 {
        return;
    }
    let last = x1 - x0 - 1;
    for y in rows {
        let row = &mut px[y * stride + x0..y * stride + x1];
        scratch.clear();
        scratch.extend_from_slice(row);
        for (i, out) in row.iter_mut().enumerate() {
            let (red, green, blue) = (scratch[i.saturating_sub(dx)], scratch[i], scratch[(i + dx).min(last)]);
            let alpha = (red >> 24).max(green >> 24).max(blue >> 24);
            *out = alpha << 24 | (red & 0x00FF_0000) | (green & 0x0000_FF00) | (blue & 0x0000_00FF);
        }
    }
}

/// What a click or the pointer is on in the confirm dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DialogHit {
    Execute,
    Abort,
    /// On the card but not a button: ignored.
    Card,
    /// Outside the card: cancels.
    Outside,
}

/// Screen-space rectangles of the confirm dialog, centred on a `width` x `height` screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dialog {
    pub card: D2D_RECT_F,
    pub caption: D2D_RECT_F,
    pub title: D2D_RECT_F,
    pub prompt: D2D_RECT_F,
    pub execute: D2D_RECT_F,
    pub abort: D2D_RECT_F,
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

fn contains(r: D2D_RECT_F, x: f32, y: f32) -> bool {
    (r.left..=r.right).contains(&x) && (r.top..=r.bottom).contains(&y)
}

const CARD_W: f32 = 560.0;
const CARD_H: f32 = 280.0;
const CARD_PAD: f32 = 32.0;
const BUTTON_H: f32 = 46.0;
const BUTTON_GAP: f32 = 16.0;

impl Dialog {
    pub fn new(width: f32, height: f32, scale: f32) -> Self {
        let (w, h, pad) = ((CARD_W * scale).min(width), (CARD_H * scale).min(height), CARD_PAD * scale);
        let (left, top) = ((width - w) / 2.0, (height - h) / 2.0);
        let card = rect(left, top, left + w, top + h);
        let inner = (card.left + pad, card.right - pad);
        let button_top = card.bottom - 30.0 * scale - BUTTON_H * scale;
        let half = (inner.1 - inner.0 - BUTTON_GAP * scale) / 2.0;
        let button = |x: f32| rect(x, button_top, x + half, button_top + BUTTON_H * scale);
        Self {
            card,
            caption: rect(inner.0, card.top + 24.0 * scale, inner.1, card.top + 46.0 * scale),
            title: rect(inner.0, card.top + 50.0 * scale, inner.1, card.top + 130.0 * scale),
            prompt: rect(inner.0, card.top + 138.0 * scale, inner.1, card.top + 174.0 * scale),
            execute: button(inner.0),
            abort: button(inner.0 + half + BUTTON_GAP * scale),
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> DialogHit {
        if contains(self.execute, x, y) {
            DialogHit::Execute
        } else if contains(self.abort, x, y) {
            DialogHit::Abort
        } else if contains(self.card, x, y) {
            DialogHit::Card
        } else {
            DialogHit::Outside
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centre(r: D2D_RECT_F) -> (f32, f32) {
        ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0)
    }

    #[test]
    fn confirm_sequence_waits_for_a_decision_after_the_glitch() {
        let seq = Sequence::new(Action::Shutdown, true, 1000);
        assert_eq!(seq.phase(1000), Phase::Glitching(0.0));
        assert!(matches!(seq.phase(1250), Phase::Glitching(p) if (p - 0.5).abs() < 1e-6));
        assert!(seq.animating(1499));
        assert_eq!(seq.phase(1500), Phase::Confirm);
        assert_eq!(seq.phase(900_000), Phase::Confirm, "waits indefinitely");
        assert!(!seq.animating(1500), "settled: no timer needed");
    }

    #[test]
    fn decisions_are_only_taken_while_the_dialog_waits() {
        let mut seq = Sequence::new(Action::Restart, true, 0);
        assert!(!seq.execute(100) && !seq.cancel(100), "ignored during the glitch");
        assert!(matches!(seq.phase(100), Phase::Glitching(_)));
        assert!(seq.execute(600));
        assert_eq!(seq.phase(600), Phase::Executing);
        assert!(!seq.cancel(601), "already decided");
    }

    #[test]
    fn focus_starts_on_abort_and_moves_with_the_arrows_and_tab() {
        let mut seq = Sequence::new(Action::Shutdown, true, 0);
        assert_eq!(seq.focus(), Focus::Abort);
        seq.navigate(Nav::Left);
        assert_eq!(seq.focus(), Focus::Execute);
        seq.navigate(Nav::Left);
        assert_eq!(seq.focus(), Focus::Execute, "left stops at EXECUTE");
        seq.navigate(Nav::Right);
        assert_eq!(seq.focus(), Focus::Abort);
        seq.navigate(Nav::Right);
        assert_eq!(seq.focus(), Focus::Abort);
        seq.navigate(Nav::Switch);
        assert_eq!(seq.focus(), Focus::Execute);
        seq.navigate(Nav::Switch);
        assert_eq!(seq.focus(), Focus::Abort);
    }

    #[test]
    fn a_held_key_is_recognised_as_auto_repeat() {
        assert!(!is_auto_repeat(0x001C_0001), "first press: previous state bit clear");
        assert!(is_auto_repeat(0x401C_0001), "bit 30 set: key was already down");
    }

    #[test]
    fn cancelling_plays_the_reverse_glitch_then_finishes() {
        let mut seq = Sequence::new(Action::SignOut, true, 0);
        assert!(seq.cancel(700));
        assert_eq!(seq.phase(700), Phase::Cancelling(0.0));
        assert!(matches!(seq.phase(800), Phase::Cancelling(p) if (p - 0.5).abs() < 1e-6));
        assert!(seq.animating(850));
        assert_eq!(seq.phase(700 + GLITCH_OUT_MS), Phase::Done);
        assert!(!seq.animating(2000));
        assert!(!seq.execute(900), "an aborted sequence can't execute");
    }

    #[test]
    fn unconfirmed_actions_glitch_briefly_then_run() {
        let mut seq = Sequence::new(Action::Lock, false, 50);
        assert!(matches!(seq.phase(100), Phase::Glitching(_)));
        assert_eq!(seq.phase(50 + GLITCH_RUN_MS - 1), Phase::Glitching((GLITCH_RUN_MS - 1) as f32 / GLITCH_RUN_MS as f32));
        assert_eq!(seq.phase(50 + GLITCH_RUN_MS), Phase::Executing);
        assert!(!seq.cancel(1000), "there is no dialog to abort");
    }

    #[test]
    fn looks_join_up_at_the_phase_boundaries() {
        let end_of_glitch = Phase::Glitching(1.0).look(true);
        let settled = Phase::Confirm.look(true);
        assert_eq!(end_of_glitch, settled);
        let start_of_reverse = Phase::Cancelling(0.0).look(true);
        assert_eq!((start_of_reverse.dim, start_of_reverse.dialog, start_of_reverse.menu), (1.0, 1.0, 0.0));
        let end_of_reverse = Phase::Cancelling(1.0).look(true);
        assert_eq!((end_of_reverse.glitch, end_of_reverse.dim, end_of_reverse.dialog, end_of_reverse.menu), (0.0, 0.0, 0.0, 1.0));
        assert_eq!(Phase::Done.look(true).dim, 0.0);
        let start = Phase::Glitching(0.0).look(true);
        assert_eq!((start.glitch, start.dim, start.dialog, start.menu), (1.0, 0.0, 0.0, 1.0));
        assert_eq!(Phase::Glitching(0.5).look(false).dialog, 0.0, "no dialog without confirmation");
        for p in (0..=100).map(|i| i as f32 / 100.0) {
            for look in [Phase::Glitching(p).look(true), Phase::Glitching(p).look(false), Phase::Cancelling(p).look(true)] {
                for v in [look.glitch, look.dim, look.dialog, look.menu] {
                    assert!((0.0..=1.0).contains(&v), "p {p}: {look:?}");
                }
            }
        }
    }

    #[test]
    fn glitch_frames_are_deterministic_and_scale_with_strength() {
        assert_eq!(slices(7, 1.0, 1000, 1.0), slices(7, 1.0, 1000, 1.0));
        assert_ne!(slices(7, 1.0, 1000, 1.0), slices(8, 1.0, 1000, 1.0));
        assert!(slices(7, 0.0, 1000, 1.0).is_empty());
        assert!(slices(7, 1.0, 1000, 1.0).len() > slices(7, 0.3, 1000, 1.0).len());
        for s in slices(3, 1.0, 500, 1.25) {
            assert!(s.y + s.h <= 500 && s.dx.abs() <= 113);
        }
        assert!(blocks(1, 0.0, 1920.0, 1080.0, 1.0).is_empty());
        for frame in 0..40 {
            for b in blocks(frame, 1.0, 1920.0, 1080.0, 1.25) {
                assert!(b.rect.left >= 0.0 && b.rect.right <= 1920.0 && b.rect.top >= 0.0 && b.rect.bottom <= 1080.0);
                assert!(b.color < 3);
            }
        }
    }

    #[test]
    fn shifting_rows_rotates_only_the_chosen_columns_and_rows() {
        let mut px: Vec<u32> = (0..24).collect(); // 6 wide, 4 tall
        shift_rows(&mut px, 6, 1..3, 1, 5, 1);
        assert_eq!(&px[0..6], &[0, 1, 2, 3, 4, 5], "row outside the band");
        assert_eq!(&px[6..12], &[6, 10, 7, 8, 9, 11], "columns 1..5 rotated right by one");
        assert_eq!(&px[18..24], &[18, 19, 20, 21, 22, 23]);
        shift_rows(&mut px, 6, 1..2, 1, 5, -1);
        assert_eq!(&px[6..12], &[6, 7, 8, 9, 10, 11], "negative shift undoes it");
        shift_rows(&mut px, 6, 0..4, 2, 2, 5);
    }

    #[test]
    fn rgb_split_separates_channels_around_a_bright_pixel() {
        let mut px = vec![0u32; 9];
        px[4] = 0xFFFF_FFFF;
        let mut scratch = Vec::new();
        rgb_split(&mut px, 9, 0..1, 0, 9, 2, &mut scratch);
        assert_eq!(px[4], 0xFF00_FF00, "green stays, alpha kept");
        assert_eq!(px[6], 0xFF00_0000 | 0x00FF_0000, "red appears dx to the right");
        assert_eq!(px[2], 0xFF00_0000 | 0x0000_00FF, "blue appears dx to the left");
        let mut flat = vec![0xCC10_2030u32; 7];
        rgb_split(&mut flat, 7, 0..1, 0, 7, 3, &mut scratch);
        assert!(flat.iter().all(|&p| p == 0xCC10_2030), "uniform areas don't change");
    }

    #[test]
    fn dialog_is_centred_and_its_buttons_are_hit_at_every_dpi() {
        for (w, h, scale) in [(1920.0, 1080.0, 1.0), (2560.0, 1440.0, 1.25), (3840.0, 2160.0, 1.5), (800.0, 600.0, 1.0)] {
            let d = Dialog::new(w, h, scale);
            let (cx, cy) = centre(d.card);
            assert!((cx - w / 2.0).abs() < 0.01 && (cy - h / 2.0).abs() < 0.01, "{w}x{h}");
            assert!(d.card.left >= 0.0 && d.card.right <= w && d.card.top >= 0.0 && d.card.bottom <= h);
            assert!(d.execute.right < d.abort.left, "buttons don't overlap");
            assert!(d.execute.left >= d.card.left && d.abort.right <= d.card.right && d.abort.bottom <= d.card.bottom);
            assert!(d.prompt.bottom < d.execute.top && d.title.bottom <= d.prompt.top && d.caption.bottom <= d.title.top);
            assert_eq!(d.hit(centre(d.execute).0, centre(d.execute).1), DialogHit::Execute);
            assert_eq!(d.hit(centre(d.abort).0, centre(d.abort).1), DialogHit::Abort);
            assert_eq!(d.hit(cx, d.title.top + 1.0), DialogHit::Card);
            assert_eq!(d.hit(d.card.left - 1.0, cy), DialogHit::Outside);
            assert_eq!(d.hit(0.0, 0.0), DialogHit::Outside);
        }
        let big = Dialog::new(2560.0, 1440.0, 1.25);
        let small = Dialog::new(2560.0, 1440.0, 1.0);
        assert!((big.card.right - big.card.left) > (small.card.right - small.card.left), "grows with DPI");
    }
}
