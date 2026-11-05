//! Everything the buddy window shows, drawn straight into the window's BGRA buffer.
//! Colors are given to tiny-skia with red and blue swapped, so its RGBA output is already the BGRA
//! Windows wants and no per-frame conversion is needed.

use crate::i18n::t;
use crate::store::{detail_of, history_of, name_of, status_of, Session, State};
use crate::motion::{Motion, Xform};
use crate::theme::{self, pal};
use std::f32::consts::{PI, TAU};
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, GradientStop, LineCap, LinearGradient, Paint, Path, PathBuilder, PixmapMut, PixmapPaint, PixmapRef, Point, Rect, SpreadMode, Stroke, Transform,
};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{RECT, SIZE};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW, GetTextExtentPoint32W, SelectObject,
    SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY, DIB_RGB_COLORS, DT_END_ELLIPSIS,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, GdiFlush, GetTextFaceW, HDC, HFONT, TRANSPARENT,
};

/// Window size in logical pixels; everything is multiplied by the display scale.
pub const WIN_W: f32 = 300.0;
pub const WIN_H: f32 = 620.0;
pub const CHAR_SIZE: f32 = 160.0;
const CHAR_BOTTOM_GAP: f32 = 4.0;
/// Room around a media character for what motion can do: hop rises 18 and stretches 3% of the height,
/// nudge leans 5° at the top.
pub const MOTION_MARGIN_TOP: f32 = 30.0;
pub const MOTION_MARGIN_SIDE: f32 = 30.0;
const MAX_BUBBLE_LINES: usize = 3;
const MAX_PANEL_ROWS: usize = 6;
const ROW_H: f32 = 44.0;
const USAGE_LINE_H: f32 = 18.0;

pub const CHARACTER_NAMES: [&str; 3] = ["Mochi", "Mèo Mướp", "Gấu Bơ"];

pub(crate) fn rgb(hex: u32) -> (u8, u8, u8) {
    ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Note the swap: see the module comment.
pub(crate) fn col(hex: u32, alpha: f32) -> Color {
    let (r, g, b) = rgb(hex);
    Color::from_rgba8(b, g, r, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

pub(crate) fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

/// The characters' own dark colour; bubble and panel follow the Windows theme instead.
const INK: u32 = 0x2b2118;

pub(crate) fn state_color(s: State) -> u32 {
    theme::on_theme(match s {
        State::Working => 0xe0a526,
        State::Waiting => 0xd9483b,
        State::Done => 0x3f9d5b,
        State::Idle => 0x8a8a8a,
        State::Error => 0xc42b1c,
    })
}

/// Segoe Fluent Icons / Segoe MDL2 Assets, so a state is never told by colour alone.
pub(crate) fn state_glyph(s: State) -> &'static str {
    match s {
        State::Working => "\u{E916}",
        State::Waiting => "\u{E7BA}",
        State::Done => "\u{E73E}",
        State::Error => "\u{EA39}",
        State::Idle => "\u{E73F}",
    }
}

// ---- shapes ----

pub(crate) fn oval(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    PathBuilder::from_oval(Rect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0)?)
}

pub(crate) fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let mut b = PathBuilder::new();
    b.move_to(x + r, y);
    b.line_to(x + w - r, y);
    b.quad_to(x + w, y, x + w, y + r);
    b.line_to(x + w, y + h - r);
    b.quad_to(x + w, y + h, x + w - r, y + h);
    b.line_to(x + r, y + h);
    b.quad_to(x, y + h, x, y + h - r);
    b.line_to(x, y + r);
    b.quad_to(x, y, x + r, y);
    b.close();
    b.finish()
}

pub(crate) fn line(x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Path> {
    let mut b = PathBuilder::new();
    b.move_to(x0, y0);
    b.line_to(x1, y1);
    b.finish()
}

fn curve(x0: f32, y0: f32, cx: f32, cy: f32, x1: f32, y1: f32) -> Option<Path> {
    let mut b = PathBuilder::new();
    b.move_to(x0, y0);
    b.quad_to(cx, cy, x1, y1);
    b.finish()
}

/// Soft squircle body: flatter at the bottom than an ellipse, so it sits on the ground.
fn body_path() -> Option<Path> {
    let mut b = PathBuilder::new();
    b.move_to(100.0, 56.0);
    b.cubic_to(142.0, 56.0, 164.0, 86.0, 164.0, 124.0);
    b.cubic_to(164.0, 160.0, 142.0, 178.0, 100.0, 178.0);
    b.cubic_to(58.0, 178.0, 36.0, 160.0, 36.0, 124.0);
    b.cubic_to(36.0, 86.0, 58.0, 56.0, 100.0, 56.0);
    b.close();
    b.finish()
}

pub(crate) struct Pen<'a, 'b> {
    pub pm: &'a mut PixmapMut<'b>,
    pub tf: Transform,
}

impl Pen<'_, '_> {
    pub fn fill(&mut self, path: Option<Path>, hex: u32, alpha: f32) {
        if let Some(p) = path {
            self.pm.fill_path(&p, &paint(col(hex, alpha)), FillRule::Winding, self.tf, None);
        }
    }

    pub fn stroke(&mut self, path: Option<Path>, hex: u32, width: f32) {
        if let Some(p) = path {
            let s = Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
            self.pm.stroke_path(&p, &paint(col(hex, 1.0)), &s, self.tf, None);
        }
    }
}

// ---- character ----

struct Pose {
    dx: f32,
    dy: f32,
    sx: f32,
    sy: f32,
    tilt: f32,
}

/// How long one loop of a state's animation lasts. Every movement below repeats a whole number of times
/// within it, so a prepared loop plays forever with no visible seam.
pub fn cycle_of(state: State) -> f32 {
    match state {
        State::Idle | State::Error => 4.0,
        State::Working => 3.6,
        State::Waiting => 2.2,
        State::Done => 1.8,
    }
}

/// The closest speed (radians per second) that fits a whole number of times into the cycle.
fn fit(speed: f32, cycle: f32) -> f32 {
    TAU * (speed * cycle / TAU).round().max(1.0) / cycle
}

fn pose_for(state: State, t: f32) -> Pose {
    let c = cycle_of(state);
    match state {
        State::Idle => {
            let b = (t * fit(1.6, c)).sin();
            Pose { dx: 0.0, dy: 0.0, sx: 1.0 - b * 0.012, sy: 1.0 + b * 0.016, tilt: 0.0 }
        }
        State::Working => {
            let phase = (t * fit(5.2, c)).sin().abs();
            let squash = (1.0 - phase).powi(3) * 0.06;
            Pose { dx: 0.0, dy: -phase * 9.0, sx: 1.0 + squash, sy: 1.0 - squash, tilt: (t * fit(2.6, c)).sin() * 0.02 }
        }
        State::Waiting => {
            // A short shake every couple of seconds catches the eye without being tiring to watch.
            let cycle = t % 2.2;
            let dx = if cycle < 0.5 { (cycle * 40.0).sin() * 4.0 * (1.0 - cycle * 2.0) } else { 0.0 };
            let b = (t * fit(2.4, c)).sin();
            Pose { dx, dy: 0.0, sx: 1.0 - b * 0.01, sy: 1.0 + b * 0.012, tilt: 0.0 }
        }
        State::Done => {
            let cycle = t % 1.8;
            let hop = if cycle < 0.5 { (cycle / 0.5 * PI).sin() } else { 0.0 };
            let b = (t * fit(1.8, c)).sin();
            Pose { dx: 0.0, dy: -hop * 16.0, sx: 1.0 - hop * 0.03 + b * 0.006, sy: 1.0 + hop * 0.04 - b * 0.006, tilt: 0.0 }
        }
        State::Error => {
            let b = (t * fit(1.1, c)).sin();
            Pose { dx: 0.0, dy: 0.0, sx: 1.04, sy: 0.93 + b * 0.006, tilt: (t * fit(0.9, c)).sin() * 0.025 }
        }
    }
}

struct Look {
    body: u32,
    shade: u32,
    light: u32,
}

const LOOKS: [Look; 3] = [
    Look { body: 0xe08a6a, shade: 0xbf6a4c, light: 0xf5b79c },
    Look { body: 0xf0b45e, shade: 0xc98a38, light: 0xfbd79a },
    Look { body: 0xb98a63, shade: 0x8f6543, light: 0xd9b48f },
];

fn decor_behind(pen: &mut Pen, kind: usize, look: &Look) {
    match kind {
        1 => {
            // Cat ears
            for (x0, x1, x2) in [(50.0, 58.0, 88.0), (150.0, 142.0, 112.0)] {
                let mut b = PathBuilder::new();
                b.move_to(x0, 96.0);
                b.line_to(x1, 44.0);
                b.line_to(x2, 66.0);
                b.close();
                pen.fill(b.finish(), look.body, 1.0);
            }
            for (x0, x1, x2) in [(60.0, 62.0, 80.0), (140.0, 138.0, 120.0)] {
                let mut b = PathBuilder::new();
                b.move_to(x0, 82.0);
                b.line_to(x1, 56.0);
                b.line_to(x2, 68.0);
                b.close();
                pen.fill(b.finish(), 0xf5a3a3, 0.9);
            }
        }
        2 => {
            // Bear ears
            for x in [58.0, 142.0] {
                pen.fill(oval(x, 68.0, 20.0, 20.0), look.body, 1.0);
                pen.fill(oval(x, 68.0, 11.0, 11.0), look.shade, 1.0);
            }
        }
        _ => {}
    }
}

fn decor_front(pen: &mut Pen, kind: usize, look: &Look, state: State, t: f32) {
    match kind {
        0 => {
            // Sprout that sways a little
            let sway = (t * fit(1.3, cycle_of(state))).sin() * 3.0;
            pen.stroke(curve(100.0, 58.0, 100.0, 46.0, 100.0 + sway, 38.0), 0x5f9e5a, 4.0);
            let mut left = PathBuilder::new();
            left.move_to(100.0 + sway, 38.0);
            left.quad_to(84.0 + sway, 22.0, 76.0 + sway, 36.0);
            left.quad_to(88.0 + sway, 44.0, 100.0 + sway, 38.0);
            left.close();
            pen.fill(left.finish(), 0x79b86f, 1.0);
            let mut right = PathBuilder::new();
            right.move_to(100.0 + sway, 38.0);
            right.quad_to(116.0 + sway, 20.0, 126.0 + sway, 32.0);
            right.quad_to(114.0 + sway, 44.0, 100.0 + sway, 38.0);
            right.close();
            pen.fill(right.finish(), 0x8dc982, 1.0);
        }
        1 => {
            // Stripes and whiskers
            for (x, h) in [(86.0, 12.0), (100.0, 16.0), (114.0, 12.0)] {
                pen.stroke(line(x, 62.0, x, 62.0 + h), look.shade, 4.0);
            }
            for (x0, x1) in [(52.0, 32.0), (148.0, 168.0)] {
                pen.stroke(line(x0, 136.0, x1, 130.0), INK, 1.8);
                pen.stroke(line(x0, 144.0, x1, 148.0), INK, 1.8);
            }
        }
        _ => {
            // Bear muzzle
            pen.fill(oval(100.0, 138.0, 22.0, 16.0), look.light, 1.0);
        }
    }
}

fn draw_eyes(pen: &mut Pen, state: State, t: f32) {
    const LX: f32 = 78.0;
    const RX: f32 = 122.0;
    const Y: f32 = 114.0;
    match state {
        State::Error => {
            for x in [LX, RX] {
                pen.stroke(line(x - 7.0, Y - 7.0, x + 7.0, Y + 7.0), INK, 4.0);
                pen.stroke(line(x + 7.0, Y - 7.0, x - 7.0, Y + 7.0), INK, 4.0);
            }
        }
        State::Done => {
            for x in [LX, RX] {
                pen.stroke(curve(x - 8.0, Y + 3.0, x, Y - 10.0, x + 8.0, Y + 3.0), INK, 4.0);
            }
        }
        State::Waiting => {
            for x in [LX, RX] {
                pen.fill(oval(x, Y, 9.5, 11.0), INK, 1.0);
                pen.fill(oval(x + 3.0, Y - 4.0, 3.2, 3.2), 0xffffff, 1.0);
            }
        }
        State::Working => {
            let look = (t * fit(1.9, cycle_of(state))).sin() * 4.0;
            for x in [LX, RX] {
                pen.fill(oval(x + look, Y, 6.5, 8.5), INK, 1.0);
                pen.fill(oval(x + look + 2.0, Y - 3.0, 2.2, 2.2), 0xffffff, 1.0);
            }
        }
        State::Idle => {
            // Blink for about 0.14 s every 4 s
            let c = t % 4.0;
            let open = if c < 0.14 { (c / 0.07 - 1.0).abs() } else { 1.0 };
            for x in [LX, RX] {
                if open < 0.25 {
                    pen.stroke(line(x - 6.5, Y, x + 6.5, Y), INK, 3.5);
                } else {
                    pen.fill(oval(x, Y, 6.5, 8.5 * open), INK, 1.0);
                    pen.fill(oval(x + 2.0, Y - 3.0 * open, 2.2, 2.2 * open), 0xffffff, 1.0);
                }
            }
        }
    }
}

fn draw_mouth(pen: &mut Pen, state: State, t: f32) {
    match state {
        State::Done => {
            let mut b = PathBuilder::new();
            b.move_to(88.0, 133.0);
            b.quad_to(100.0, 156.0, 112.0, 133.0);
            b.close();
            pen.fill(b.finish(), 0x8c2f2a, 1.0);
        }
        State::Error => pen.stroke(curve(90.0, 146.0, 100.0, 132.0, 110.0, 146.0), INK, 3.5),
        State::Waiting => pen.fill(oval(100.0, 141.0, 6.0, 7.5), 0x8c2f2a, 1.0),
        State::Working => {
            let w = 5.0 + (t * fit(3.1, cycle_of(state))).sin().abs() * 2.5;
            pen.stroke(line(100.0 - w, 139.0, 100.0 + w, 139.0), INK, 3.5);
        }
        State::Idle => pen.stroke(curve(92.0, 136.0, 100.0, 146.0, 108.0, 136.0), INK, 3.5),
    }
}

fn draw_badge(pen: &mut Pen, state: State, t: f32) {
    const X: f32 = 164.0;
    const Y: f32 = 42.0;
    match state {
        State::Working => {
            for i in 0..3 {
                let phase = (t * fit(3.2, cycle_of(state)) - i as f32 * 0.7).sin() * 0.5 + 0.5;
                let x = 146.0 + i as f32 * 16.0;
                pen.fill(oval(x, 42.0 - phase * 5.0, 5.5, 5.5), state_color(state), 0.35 + phase * 0.65);
            }
        }
        State::Waiting => {
            let r = 17.0 + (t * fit(4.0, cycle_of(state))).sin().abs() * 2.5;
            pen.fill(oval(X, Y, r, r), state_color(state), 1.0);
            pen.stroke(line(X, Y - 9.0, X, Y + 2.0), 0xffffff, 5.0);
            pen.fill(oval(X, Y + 9.0, 2.8, 2.8), 0xffffff, 1.0);
        }
        State::Done => {
            let r = 17.0 + (t * fit(2.0, cycle_of(state))).sin() * 1.0;
            pen.fill(oval(X, Y, r, r), state_color(state), 1.0);
            pen.stroke(line(X - 8.0, Y + 1.0, X - 2.0, Y + 7.0), 0xffffff, 4.5);
            pen.stroke(line(X - 2.0, Y + 7.0, X + 9.0, Y - 6.0), 0xffffff, 4.5);
            for i in 0..3 {
                let a = t * fit(1.1, cycle_of(state)) + i as f32 * TAU / 3.0;
                let alpha = (a * 2.0).sin().abs();
                pen.fill(oval(30.0 + i as f32 * 14.0, 60.0 + a.sin() * 12.0 + i as f32 * 8.0, 3.2, 3.2), 0xe0a526, alpha);
            }
        }
        State::Error => {
            pen.fill(oval(X, Y, 17.0, 17.0), state_color(state), 1.0);
            pen.stroke(line(X - 6.5, Y - 6.5, X + 6.5, Y + 6.5), 0xffffff, 4.5);
            pen.stroke(line(X + 6.5, Y - 6.5, X - 6.5, Y + 6.5), 0xffffff, 4.5);
        }
        State::Idle => {}
    }
}

/// Draws into the 200x200 virtual square mapped by `base`.
pub fn draw_character(pm: &mut PixmapMut, base: Transform, kind: usize, state: State, t: f32) {
    let look = &LOOKS[kind.min(LOOKS.len() - 1)];
    let pose = pose_for(state, t);
    let mut pen = Pen { pm, tf: base };

    // The ground shadow stays put and tightens as the body rises.
    let shadow = 1.0 + pose.dy / 60.0;
    pen.fill(oval(100.0 + pose.dx, 184.0, 56.0 * shadow, 7.0 * shadow), 0x000000, 0.16);

    // Scale around the feet so squash and stretch keep it on the ground.
    pen.tf = base
        .pre_translate(100.0 + pose.dx, 178.0 + pose.dy)
        .pre_concat(Transform::from_row(pose.tilt.cos(), pose.tilt.sin(), -pose.tilt.sin(), pose.tilt.cos(), 0.0, 0.0))
        .pre_scale(pose.sx, pose.sy)
        .pre_translate(-100.0, -178.0);

    decor_behind(&mut pen, kind, look);
    pen.fill(oval(66.0, 176.0, 16.0, 7.0), look.shade, 1.0);
    pen.fill(oval(134.0, 176.0, 16.0, 7.0), look.shade, 1.0);
    pen.fill(body_path(), look.body, 1.0);
    pen.stroke(curve(54.0, 92.0, 66.0, 70.0, 88.0, 64.0), look.light, 6.0);
    pen.fill(oval(58.0, 132.0, 10.0, 7.0), 0xf58f8f, 0.55);
    pen.fill(oval(142.0, 132.0, 10.0, 7.0), 0xf58f8f, 0.55);
    decor_front(&mut pen, kind, look, state, t);
    draw_eyes(&mut pen, state, t);
    draw_mouth(&mut pen, state, t);

    pen.tf = base;
    draw_badge(&mut pen, state, t);
}

/// Pixel rectangle of an area standing on the window's bottom, horizontally centered.
pub fn bottom_rect(w: u32, h: u32, scale: f32, rw: u32, rh: u32) -> (u32, u32, u32, u32) {
    let (rw, rh) = (rw.min(w), rh.min(h));
    let bottom = (CHAR_BOTTOM_GAP * scale) as u32;
    ((w - rw) / 2, h.saturating_sub(rh + bottom), rw, rh)
}

/// Pixel rectangle of the character area, the only part of the window animation touches.
pub fn char_rect(w: u32, h: u32, scale: f32) -> (u32, u32, u32, u32) {
    let size = (CHAR_SIZE * scale).ceil() as u32;
    bottom_rect(w, h, scale, size, size)
}

/// Logical size of the area a media character needs, motion included.
pub fn media_region(stage_w: u32, stage_h: u32, scale: f32) -> (f32, f32) {
    let w = (stage_w as f32 / scale + MOTION_MARGIN_SIDE * 2.0).min(WIN_W);
    (w, stage_h as f32 / scale + MOTION_MARGIN_TOP)
}

/// Moves a pixel rectangle and cuts it to the window.
fn moved_rect((x, y, rw, rh): (u32, u32, u32, u32), (dx, dy): (i32, i32), w: u32, h: u32) -> (u32, u32, u32, u32) {
    let x0 = (x as i32 + dx).clamp(0, w as i32);
    let y0 = (y as i32 + dy).clamp(0, h as i32);
    let x1 = (x as i32 + rw as i32 + dx).clamp(0, w as i32);
    let y1 = (y as i32 + rh as i32 + dy).clamp(0, h as i32);
    (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
}

fn clear_rect(buf: &mut [u8], w: u32, (x, y, cw, ch): (u32, u32, u32, u32)) {
    for row in y..y + ch {
        let start = ((row * w + x) * 4) as usize;
        buf[start..start + (cw * 4) as usize].fill(0);
    }
}

/// Places `pixels` with its feet at (feet_x, feet_y), moved by `xf`. Drawn with filtering only when it
/// actually moves or leans; plain copies are what keep idle video this cheap.
#[allow(clippy::too_many_arguments)]
pub fn draw_frame(pm: &mut PixmapMut, pixels: &[u8], fw: u32, fh: u32, feet_x: f32, feet_y: f32, scale: f32, xf: Xform) {
    let Some(src) = PixmapRef::from_bytes(pixels, fw, fh) else { return };
    let (hw, fh_f) = (fw as f32 / 2.0, fh as f32);
    let moved = xf.dy != 0.0 || xf.rot != 0.0 || xf.sx != 1.0 || xf.sy != 1.0;
    let quality = if moved { FilterQuality::Bilinear } else { FilterQuality::Nearest };
    let tf = Transform::from_translate(feet_x, feet_y + xf.dy * scale)
        .pre_concat(Transform::from_rotate(xf.rot))
        .pre_scale(xf.sx, xf.sy)
        .pre_translate(-hw, -fh_f);
    let p = PixmapPaint { opacity: 1.0, blend_mode: BlendMode::SourceOver, quality };
    pm.draw_pixmap(0, 0, src, &p, tf, None);
}

/// Copies a rectangle of a frame into the window at (left, top), replacing what was there. Far cheaper
/// than drawing it, and all a frame that doesn't lean or bounce needs. Returns where it landed.
#[allow(clippy::too_many_arguments)]
fn blit(buf: &mut [u8], w: u32, h: u32, pixels: &[u8], fw: u32, left: i32, top: i32, (rx, ry, rw, rh): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let x0 = (left + rx as i32).clamp(0, w as i32);
    let x1 = (left + rx as i32 + rw as i32).clamp(0, w as i32);
    let y0 = (top + ry as i32).clamp(0, h as i32);
    let y1 = (top + ry as i32 + rh as i32).clamp(0, h as i32);
    if x1 <= x0 || y1 <= y0 {
        return (0, 0, 0, 0);
    }
    let count = (x1 - x0) as usize * 4;
    for y in y0..y1 {
        let src = ((y - top) as usize * fw as usize + (x0 - left) as usize) * 4;
        let dst = (y as usize * w as usize + x0 as usize) * 4;
        buf[dst..dst + count].copy_from_slice(&pixels[src..src + count]);
    }
    (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
}

fn frame_origin(w: u32, h: u32, scale: f32, fw: u32, fh: u32, off: (i32, i32)) -> (i32, i32) {
    let bottom = h as i32 - (CHAR_BOTTOM_GAP * scale) as i32;
    ((w as i32 - fw as i32) / 2 + off.0, bottom - fh as i32 + off.1)
}

/// Updates only the part of a frame that changed since the last one.
#[allow(clippy::too_many_arguments)]
pub fn paint_change(buf: &mut [u8], w: u32, h: u32, scale: f32, pixels: &[u8], fw: u32, fh: u32, rect: (u32, u32, u32, u32), off: (i32, i32)) -> (u32, u32, u32, u32) {
    let (left, top) = frame_origin(w, h, scale, fw, fh, off);
    blit(buf, w, h, pixels, fw, left, top, rect)
}

/// Redraws a media character into the window; `buf` is the whole window. Returns the dirty rectangle.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn paint_media(buf: &mut [u8], w: u32, h: u32, scale: f32, pixels: &[u8], fw: u32, fh: u32, xf: Xform, badge: Option<State>, t: f32, off: (i32, i32)) -> (u32, u32, u32, u32) {
    paint_stage(buf, w, h, scale, pixels, fw, fh, xf, badge, t, off, true)
}

/// `room`: keep the margins motion needs. Characters that never move on their own stage don't.
#[allow(clippy::too_many_arguments)]
pub fn paint_stage(buf: &mut [u8], w: u32, h: u32, scale: f32, pixels: &[u8], fw: u32, fh: u32, xf: Xform, badge: Option<State>, t: f32, off: (i32, i32), room: bool) -> (u32, u32, u32, u32) {
    let (rw, rh) = if room { media_region(fw, fh, scale) } else { (fw as f32 / scale, fh as f32 / scale) };
    // Reaches the window's bottom edge: leaning dips a corner below the feet, into the gap under them.
    let bottom = h.saturating_sub((CHAR_BOTTOM_GAP * scale) as u32);
    let (rw_px, rh_px) = (((rw * scale).ceil() as u32).min(w), ((rh * scale).ceil() as u32).min(bottom));
    let region = moved_rect(((w - rw_px) / 2, bottom - rh_px, rw_px, rh_px + (h - bottom)), off, w, h);
    clear_rect(buf, w, region);
    if xf == Motion::None.at(0.0) && badge.is_none() {
        let (left, top) = frame_origin(w, h, scale, fw, fh, off);
        blit(buf, w, h, pixels, fw, left, top, (0, 0, fw, fh));
        return region;
    }
    let feet_x = w as f32 / 2.0 + off.0 as f32;
    let feet_y = bottom as f32 + off.1 as f32;
    let Some(mut pm) = PixmapMut::from_bytes(buf, w, h) else { return region };
    draw_frame(&mut pm, pixels, fw, fh, feet_x, feet_y, scale, xf);
    if let Some(state) = badge {
        draw_badge_at(&mut pm, feet_x + fw as f32 / 2.0 - 18.0 * scale, feet_y - fh as f32 + 18.0 * scale, scale, state, t);
    }
    region
}

/// The state badge alone, for a state that shows idle's picture.
pub fn draw_badge_at(pm: &mut PixmapMut, x: f32, y: f32, scale: f32, state: State, t: f32) {
    // The badge is drawn around (164, 42) of the character's 200x200 square.
    let k = scale * 0.8;
    let mut pen = Pen { pm, tf: Transform::from_translate(x - 164.0 * k, y - 42.0 * k).pre_scale(k, k) };
    draw_badge(&mut pen, state, t);
}

/// Redraws only the character area. `buf` is the whole window; returns the dirty rectangle.
#[allow(clippy::too_many_arguments)]
pub fn paint_character(buf: &mut [u8], w: u32, h: u32, scale: f32, kind: usize, state: State, t: f32, off: (i32, i32)) -> (u32, u32, u32, u32) {
    let (x, y, cw, ch) = char_rect(w, h, scale);
    let dirty = moved_rect((x, y, cw, ch), off, w, h);
    clear_rect(buf, w, dirty);
    let Some(mut pm) = PixmapMut::from_bytes(buf, w, h) else { return dirty };
    let k = CHAR_SIZE * scale / 200.0;
    let base = Transform::from_translate((x as i32 + off.0) as f32, (y as i32 + off.1) as f32).pre_scale(k, k);
    draw_character(&mut pm, base, kind, state, t);
    dirty
}

// ---- text ----

#[derive(Clone, Copy)]
pub enum Font {
    Normal,
    Bold,
    Small,
    Title,
    Mono,
    Display,
    Icon,
}

/// Text comes from GDI so Vietnamese and symbols get Windows' own fonts and fallback, with no font
/// bundled. GDI can't write alpha, so it draws white on black and that becomes the coverage mask.
#[cfg(windows)]
pub struct TextDrawer {
    dc: HDC,
    fonts: [HFONT; 7],
}

/// Whether a font is installed: asking for a missing one silently gives a different font.
#[cfg(windows)]
fn has_font(dc: HDC, name: &str) -> bool {
    let wanted: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let f = CreateFontW(-12, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 0, 0, wanted.as_ptr());
        let old = SelectObject(dc, f);
        let mut got = [0u16; 64];
        let n = GetTextFaceW(dc, got.len() as i32, got.as_mut_ptr());
        SelectObject(dc, old);
        DeleteObject(f);
        n > 0 && String::from_utf16_lossy(&got[..n as usize - 1]).eq_ignore_ascii_case(name)
    }
}

/// Windows 11's UI font, or Windows 10's.
#[cfg(windows)]
fn ui_face(dc: HDC) -> Vec<u16> {
    let name = if has_font(dc, "Segoe UI Variable Text") { "Segoe UI Variable Text" } else { "Segoe UI" };
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows 11's icon font, or Windows 10's. They share code points.
#[cfg(windows)]
fn icon_face(dc: HDC) -> Vec<u16> {
    let name = if has_font(dc, "Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[cfg(target_os = "macos")]
pub use crate::text_mac::TextDrawer;

/// Linux has no window to draw it in yet.
#[cfg(not(any(windows, target_os = "macos")))]
pub struct TextDrawer;

#[cfg(not(any(windows, target_os = "macos")))]
impl TextDrawer {
    pub fn new(_scale: f32) -> TextDrawer {
        TextDrawer
    }

    pub fn new_ui(_scale: f32) -> TextDrawer {
        TextDrawer
    }

    pub fn width(&self, s: &str, _f: Font) -> i32 {
        s.chars().count() as i32 * 7
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, _buf: &mut [u8], _w: u32, _h: u32, _x: i32, _y: i32, _bw: i32, _bh: i32, _s: &str, _f: Font, _color: u32) {}
}

#[cfg(windows)]
impl TextDrawer {
    /// For the buddy's window: compact sizes.
    pub fn new(scale: f32) -> TextDrawer {
        Self::make(scale, [12.5, 12.5, 11.0, 19.0, 11.0, 28.0, 13.0])
    }

    /// For the main window: the Windows type ramp.
    pub fn new_ui(scale: f32) -> TextDrawer {
        Self::make(scale, [14.0, 14.0, 12.0, 20.0, 12.0, 28.0, 16.0])
    }

    fn make(scale: f32, px: [f32; 7]) -> TextDrawer {
        let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        let face = ui_face(dc);
        let icons = icon_face(dc);
        let mono: Vec<u16> = "Consolas\0".encode_utf16().collect();
        let font = |size: f32, weight: i32, name: &[u16]| unsafe {
            CreateFontW(-(size * scale).round() as i32, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, CLEARTYPE_QUALITY as u32, 0, name.as_ptr())
        };
        unsafe { SetBkMode(dc, TRANSPARENT as i32) };
        let fonts = [
            font(px[0], 400, &face),
            font(px[1], 600, &face),
            font(px[2], 400, &face),
            font(px[3], 600, &face),
            font(px[4], 400, &mono),
            font(px[5], 600, &face),
            font(px[6], 400, &icons),
        ];
        TextDrawer { dc, fonts }
    }

    fn index(f: Font) -> usize {
        match f {
            Font::Normal => 0,
            Font::Bold => 1,
            Font::Small => 2,
            Font::Title => 3,
            Font::Mono => 4,
            Font::Display => 5,
            Font::Icon => 6,
        }
    }

    fn select(&self, f: Font) {
        unsafe { SelectObject(self.dc, self.fonts[Self::index(f)]) };
    }

    pub fn width(&self, s: &str, f: Font) -> i32 {
        self.select(f);
        let w = wide(s);
        let mut size = SIZE { cx: 0, cy: 0 };
        unsafe { GetTextExtentPoint32W(self.dc, w.as_ptr(), w.len() as i32, &mut size) };
        size.cx
    }

    /// Single line, ending in "…" when it doesn't fit in `bw`.
    ///
    /// GDI can't write alpha, so the text is drawn on a scratch bitmap and blended in by hand. It is
    /// drawn the way round it will finally appear (dark on white, or light on black), because Windows
    /// tunes the stem weights for that, and using one mask for both is what makes text thin and blurry.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, buf: &mut [u8], w: u32, h: u32, x: i32, y: i32, bw: i32, bh: i32, s: &str, f: Font, color: u32) {
        if bw <= 0 || bh <= 0 || s.is_empty() {
            return;
        }
        let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bw,
            biHeight: -bh,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        unsafe {
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(self.dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                return;
            }
            let old = SelectObject(self.dc, bmp);
            self.select(f);
            let (r, g, b) = rgb(color);
            // Rec. 601 weights: what counts here is only which way round the text reads.
            let dark_text = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 < 128;
            let len = (bw * bh * 4) as usize;
            if dark_text {
                std::ptr::write_bytes(bits as *mut u8, 0xFF, len);
            }
            SetTextColor(self.dc, if dark_text { 0 } else { 0x00ff_ffff });
            let mut text = wide(s);
            let mut rc = RECT { left: 0, top: 0, right: bw, bottom: bh };
            DrawTextW(self.dc, text.as_mut_ptr(), text.len() as i32, &mut rc, DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS | DT_VCENTER);
            GdiFlush();
            let mask = std::slice::from_raw_parts(bits as *const u8, len);
            blend_mask(buf, w, h, x, y, bw, bh, mask, color, dark_text);
            SelectObject(self.dc, old);
            DeleteObject(bmp);
        }
    }
}

#[cfg(windows)]
impl Drop for TextDrawer {
    fn drop(&mut self) {
        unsafe {
            for f in self.fonts {
                DeleteObject(f);
            }
            DeleteDC(self.dc);
        }
    }
}

/// Below this the surface under the text is too see-through for colour fringes to be right.
const SUBPIXEL_MIN_ALPHA: u32 = 170;

#[allow(clippy::too_many_arguments)]
pub(crate) fn blend_mask(buf: &mut [u8], w: u32, h: u32, x: i32, y: i32, bw: i32, bh: i32, mask: &[u8], color: u32, dark_text: bool) {
    let (r, g, b) = rgb(color);
    let src = [b as u32, g as u32, r as u32];
    for my in 0..bh {
        let py = y + my;
        if py < 0 || py >= h as i32 {
            continue;
        }
        for mx in 0..bw {
            let px = x + mx;
            if px < 0 || px >= w as i32 {
                continue;
            }
            let m = ((my * bw + mx) * 4) as usize;
            let cov = |c: usize| if dark_text { 255 - mask[m + c] as u32 } else { mask[m + c] as u32 };
            let (c0, c1, c2) = (cov(0), cov(1), cov(2));
            if c0 == 0 && c1 == 0 && c2 == 0 {
                continue;
            }
            let i = ((py as u32 * w + px as u32) * 4) as usize;
            let a = buf[i + 3] as u32;
            // ClearType per channel on a solid surface; over a see-through one, the middle channel alone.
            let cs = if a >= SUBPIXEL_MIN_ALPHA { [c0, c1, c2] } else { [c1, c1, c1] };
            let out_a = c1 + a * (255 - c1) / 255;
            for k in 0..3 {
                let v = (src[k] * cs[k] + buf[i + k] as u32 * (255 - cs[k])) / 255;
                // Premultiplied colour can't exceed alpha, or Windows shows it wrong.
                buf[i + k] = v.min(out_a) as u8;
            }
            buf[i + 3] = out_a as u8;
        }
    }
}

// ---- bubble and panel ----

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Character,
    Bubble,
    Row(String),
}

/// Clickable area in window pixels.
pub struct HitRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub hit: Hit,
}

pub struct View<'a> {
    pub sessions: &'a [Session],
    pub panel_open: bool,
    pub hover: Option<&'a str>,
    pub note: Option<&'a str>,
    pub now: u64,
    /// Logical size the character takes at the bottom of the window.
    pub char_region: (f32, f32),
    /// Where the character sits from its usual spot, logical pixels. Up is negative.
    pub char_off: (f32, f32),
    /// Token usage summary shown under the panel's title.
    pub usage: Option<&'a str>,
    pub bubble: Bubble,
}

/// The status without its ✓ or ✗: the state's icon beside it already says that.
fn status_words(s: &Session) -> String {
    status_of(s).trim_end_matches(['✓', '✗']).trim_end().to_string()
}

/// How much the bubble says. It covers whatever is behind it, so it only stays up when it must.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bubble {
    Hidden,
    /// Only sessions that are stuck until you act.
    Attention,
    All,
}

fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then) / 1000;
    match s {
        0..=59 => t("vừa xong", "just now").into(),
        60..=3599 => crate::tf!("{} phút", "{} min", s / 60),
        _ => crate::tf!("{} giờ", "{} h", s / 3600),
    }
}

struct Ui<'a> {
    w: u32,
    h: u32,
    scale: f32,
    text: &'a TextDrawer,
}

impl Ui<'_> {
    /// `tail`: where a little pointer sits along the card, and whether it points up.
    #[allow(clippy::too_many_arguments)]
    fn card(&self, buf: &mut [u8], x: f32, y: f32, w: f32, h: f32, border: u32, tail: Option<(f32, bool)>) {
        let p = pal();
        let Some(mut pm) = PixmapMut::from_bytes(buf, self.w, self.h) else { return };
        let tf = Transform::from_scale(self.scale, self.scale);
        let mut pen = Pen { pm: &mut pm, tf };
        pen.fill(round_rect(x, y + 3.0, w, h, 8.0), 0x000000, 0.10);
        pen.fill(round_rect(x, y + 1.0, w, h, 8.0), 0x000000, 0.08);
        let alpha = if theme::translucent() { theme::FLOATING_ALPHA } else { 1.0 };
        pen.fill(round_rect(x, y, w, h, 8.0), p.surface, alpha);
        pen.stroke(round_rect(x, y, w, h, 8.0), border, 1.0);
        if let Some((cx, up)) = tail {
            // Starts exactly at the edge: overlapping the see-through card would show as a lighter patch.
            let (edge, tip) = if up { (y, y - 6.0) } else { (y + h, y + h + 6.0) };
            let mut b = PathBuilder::new();
            b.move_to(cx - 7.0, edge);
            b.line_to(cx, tip);
            b.line_to(cx + 7.0, edge);
            b.close();
            pen.fill(b.finish(), p.surface, alpha);
            pen.stroke(line(cx - 7.0, edge, cx, tip), border, 1.0);
            pen.stroke(line(cx, tip, cx + 7.0, edge), border, 1.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn label(&self, buf: &mut [u8], x: f32, y: f32, w: f32, h: f32, s: &str, f: Font, color: u32) {
        let k = self.scale;
        self.text.draw(buf, self.w, self.h, (x * k) as i32, (y * k) as i32, (w * k) as i32, (h * k) as i32, s, f, color);
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32, hit: Hit) -> HitRect {
        let k = self.scale;
        HitRect { x: (x * k) as i32, y: (y * k) as i32, w: (w * k) as i32, h: (h * k) as i32, hit }
    }
}

/// Draws everything except the character; returns the clickable areas, topmost first.
pub fn paint_ui(buf: &mut [u8], w: u32, h: u32, scale: f32, text: &TextDrawer, view: &View) -> Vec<HitRect> {
    buf.fill(0);
    let p = pal();
    let ui = Ui { w, h, scale, text };
    let mut hits = Vec::new();
    let (char_w, char_h) = view.char_region;
    let (off_x, off_y) = view.char_off;
    let char_bottom = WIN_H - CHAR_BOTTOM_GAP + off_y;
    let char_top = char_bottom - char_h;
    // Raised past the middle of its travel, there is more room under the character than over it, so
    // the bubble and panel go there instead.
    let travel = (WIN_H - CHAR_BOTTOM_GAP - char_h).max(0.0);
    let below = -off_y > travel + off_y;
    let mut top = char_top - 2.0;
    let mut down = char_bottom + CHAR_BOTTOM_GAP + 2.0;

    // Bubble: one line per active session, plus the top one's detail.
    let active: Vec<&Session> = view
        .sessions
        .iter()
        .filter(|s| match view.bubble {
            Bubble::Hidden => false,
            Bubble::Attention => matches!(s.state, State::Waiting | State::Error),
            Bubble::All => s.state != State::Idle,
        })
        .collect();
    if let Some(first) = active.first() {
        let shown = active.len().min(MAX_BUBBLE_LINES);
        let more = active.len() - shown;
        let detail = detail_of(first);
        let height = 12.0 + shown as f32 * 20.0 + if more > 0 { 16.0 } else { 0.0 } + if detail.is_some() { 17.0 } else { 0.0 };
        // Only as wide as its words: everything behind the rest of it stays visible and clickable.
        const MAX_W: f32 = 272.0;
        const LINE_LEFT: f32 = 12.0 + 20.0;
        let measure = |s: &str, f: Font| text.width(s, f) as f32 / scale + 2.0;
        let mut content: f32 = 0.0;
        for s in active.iter().take(shown) {
            content = content.max(LINE_LEFT + measure(&format!("{}:", name_of(s)), Font::Bold) + 4.0 + measure(&status_words(s), Font::Normal));
        }
        if let Some(d) = &detail {
            content = content.max(12.0 + measure(d, Font::Small));
        }
        let bw = (content + 12.0).clamp(150.0, MAX_W);
        // Follows the character sideways as far as the window allows; the tail keeps pointing at it.
        let room = ((WIN_W - 16.0 - bw) / 2.0).max(0.0);
        let bx = (WIN_W - bw) / 2.0 + off_x.clamp(-room, room);
        let tail_x = (WIN_W / 2.0 + off_x).clamp(bx + 16.0, bx + bw - 16.0);
        let by = if below { down + 7.0 } else { top - 7.0 - height };
        ui.card(buf, bx, by, bw, height, p.stroke, Some((tail_x, below)));
        let mut y = by + 6.0;
        for s in active.iter().take(shown) {
            // Name and status shrink separately, so a long session name never hides the status.
            let name = format!("{}:", name_of(s));
            let status = status_words(s);
            let inner = bw - LINE_LEFT - 12.0;
            let name_w = measure(&name, Font::Bold).min(inner * 0.55);
            ui.label(buf, bx + 10.0, y, 18.0, 20.0, state_glyph(s.state), Font::Icon, state_color(s.state));
            ui.label(buf, bx + LINE_LEFT, y, name_w, 20.0, &name, Font::Bold, p.ink);
            ui.label(buf, bx + LINE_LEFT + name_w + 4.0, y, inner - name_w - 4.0, 20.0, &status, Font::Normal, p.ink);
            y += 20.0;
        }
        if more > 0 {
            ui.label(buf, bx + LINE_LEFT, y, bw - LINE_LEFT - 12.0, 16.0, &crate::tf!("+{more} phiên khác", "+{more} more"), Font::Small, p.muted);
            y += 16.0;
        }
        if let Some(d) = &detail {
            ui.label(buf, bx + 12.0, y, bw - 24.0, 17.0, d, Font::Small, p.muted);
        }
        hits.push(ui.rect(bx, if below { by - 7.0 } else { by }, bw, height + 7.0, Hit::Bubble));
        if below {
            down = by + height;
        } else {
            top = by;
        }
    }

    if view.panel_open {
        // As many rows as fit above whatever the character and bubble take up.
        let usage_h = if view.usage.is_some() { USAGE_LINE_H } else { 0.0 };
        let space = if below { WIN_H - down } else { top };
        let fits = ((space - 6.0 - 38.0 - 18.0 - usage_h) / ROW_H).floor().max(1.0) as usize;
        let shown = view.sessions.len().min(MAX_PANEL_ROWS).min(fits);
        let extra = view.sessions.len() - shown;
        let rows = shown.max(1) as f32 * ROW_H;
        let footer = if extra > 0 || view.note.is_some() { 18.0 } else { 0.0 };
        let height = 30.0 + usage_h + rows + footer + 6.0;
        let (pw, px) = (284.0, (WIN_W - 284.0) / 2.0);
        let py = if below { down + 6.0 } else { (top - 6.0 - height).max(1.0) };
        ui.card(buf, px, py, pw, height, p.stroke, None);
        ui.label(buf, px + 12.0, py + 6.0, 180.0, 20.0, t("Phiên Claude", "Claude sessions"), Font::Bold, p.ink);
        if !view.sessions.is_empty() {
            ui.label(buf, px + pw - 44.0, py + 6.0, 32.0, 20.0, &view.sessions.len().to_string(), Font::Small, p.muted);
        }
        let mut y = py + 30.0;
        if let Some(u) = view.usage {
            ui.label(buf, px + 12.0, y - 4.0, pw - 24.0, USAGE_LINE_H, u, Font::Small, p.muted);
            y += usage_h;
        }
        if view.sessions.is_empty() {
            ui.label(buf, px + 12.0, y, pw - 24.0, ROW_H, t("Chưa có phiên nào", "No sessions yet"), Font::Normal, p.muted);
        }
        for s in view.sessions.iter().take(shown) {
            if view.hover == Some(s.id.as_str()) {
                if let Some(mut pm) = PixmapMut::from_bytes(buf, w, h) {
                    let mut pen = Pen { pm: &mut pm, tf: Transform::from_scale(scale, scale) };
                    pen.fill(round_rect(px + 4.0, y + 1.0, pw - 8.0, ROW_H - 2.0, 4.0), p.hover, 1.0);
                }
            }
            ui.label(buf, px + 8.0, y + 5.0, 18.0, 19.0, state_glyph(s.state), Font::Icon, state_color(s.state));
            let inner = pw - 28.0 - 12.0;
            let age = ago(view.now, s.updated_at);
            let age_w = text.width(&age, Font::Small) as f32 / scale + 2.0;
            ui.label(buf, px + 28.0, y + 5.0, inner - age_w - 4.0, 19.0, &name_of(s), Font::Bold, p.ink);
            ui.label(buf, px + pw - 10.0 - age_w, y + 5.0, age_w, 19.0, &age, Font::Small, p.muted);
            let status = status_words(s);
            let sub = history_of(s).or_else(|| if s.state != State::Idle { detail_of(s) } else { None });
            let mut second = if s.source == "cli" { status } else { format!("{status} · {}", s.source) };
            if let Some(sub) = sub {
                second = format!("{second} · {sub}");
            }
            ui.label(buf, px + 28.0, y + 23.0, inner, 17.0, &second, Font::Small, p.muted);
            hits.push(ui.rect(px, y, pw, ROW_H, Hit::Row(s.id.clone())));
            y += ROW_H;
        }
        if view.sessions.is_empty() {
            y += ROW_H;
        }
        if let Some(n) = view.note {
            ui.label(buf, px + 12.0, y, pw - 24.0, 18.0, n, Font::Small, state_color(State::Waiting));
        } else if extra > 0 {
            ui.label(buf, px + 12.0, y, pw - 24.0, 18.0, &crate::tf!("+{extra} phiên khác", "+{extra} more"), Font::Small, p.muted);
        }
    }
    hits.push(ui.rect((WIN_W - char_w) / 2.0 + off_x, char_top, char_w, char_h, Hit::Character));
    hits
}

/// The logo: a mochi with a sprout, in a 200x200 square. Below about 24 pixels only the silhouette
/// and eyes can be told apart, so `small` drops the rest and draws those heavier.
pub fn draw_logo(pm: &mut PixmapMut, tf: Transform, small: bool) {
    let mut pen = Pen { pm, tf };
    // Sprout first, so the body overlaps its foot.
    pen.stroke(curve(100.0, 62.0, 100.0, 44.0, 106.0, 32.0), 0x4f8f4c, if small { 11.0 } else { 7.0 });
    let leaf = |x0: f32, c0: f32, tip: f32, c1: f32| {
        let mut b = PathBuilder::new();
        b.move_to(106.0, 34.0);
        b.quad_to(x0, c0, tip, 22.0);
        b.quad_to(x0 + 2.0, c1, 106.0, 34.0);
        b.close();
        b.finish()
    };
    pen.fill(leaf(84.0, 8.0, 62.0, 40.0), 0x6fb564, 1.0);
    pen.fill(leaf(130.0, 2.0, 156.0, 36.0), 0x8ccf7c, 1.0);
    if let Some(body) = body_path() {
        let stops = vec![GradientStop::new(0.0, col(0xf4a582, 1.0)), GradientStop::new(1.0, col(0xd2603f, 1.0))];
        let mut p = Paint::default();
        p.anti_alias = true;
        if let Some(shader) = LinearGradient::new(Point::from_xy(100.0, 56.0), Point::from_xy(100.0, 178.0), stops, SpreadMode::Pad, Transform::identity()) {
            p.shader = shader;
            pen.pm.fill_path(&body, &p, FillRule::Winding, pen.tf, None);
        }
    }
    if small {
        for x in [74.0, 126.0] {
            pen.fill(oval(x, 116.0, 13.0, 17.0), INK, 1.0);
        }
        return;
    }
    pen.stroke(curve(56.0, 96.0, 66.0, 74.0, 88.0, 68.0), 0xfad3bf, 7.0);
    pen.fill(oval(58.0, 134.0, 11.0, 8.0), 0xf58f8f, 0.6);
    pen.fill(oval(142.0, 134.0, 11.0, 8.0), 0xf58f8f, 0.6);
    for x in [78.0, 122.0] {
        pen.fill(oval(x, 114.0, 8.0, 10.5), INK, 1.0);
        pen.fill(oval(x + 2.5, 110.0, 2.6, 2.6), 0xffffff, 1.0);
    }
    pen.stroke(curve(90.0, 136.0, 100.0, 149.0, 110.0, 136.0), INK, 5.0);
}

/// The logo as icon pixels: straight (non-premultiplied) BGRA, as icons expect. A status colour adds a
/// dot in the corner, which lets the tray show what is going on without anything on the desktop.
pub fn paint_icon(size: u32, status: Option<u32>) -> Vec<u8> {
    let mut buf = vec![0u8; (size * size * 4) as usize];
    if let Some(mut pm) = PixmapMut::from_bytes(&mut buf, size, size) {
        // The drawing runs from about y 8 to y 180: frame that tightly, without cutting it.
        let k = size as f32 / 176.0;
        let tf = Transform::from_scale(k, k).pre_translate(-12.0, -6.0);
        draw_logo(&mut pm, tf, size < 24);
        if let Some(c) = status {
            let s = size as f32;
            let r = s * 0.2;
            let mut pen = Pen { pm: &mut pm, tf: Transform::identity() };
            pen.fill(oval(s - r - 0.5, s - r - 0.5, r + (s * 0.05).max(1.0), r + (s * 0.05).max(1.0)), 0xffffff, 1.0);
            pen.fill(oval(s - r - 0.5, s - r - 0.5, r, r), c, 1.0);
        }
    }
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255) / a).min(255) as u8;
            }
        }
    }
    buf
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::store::{Event, Store};

    fn sample() -> Vec<Session> {
        let mut st = Store::default();
        let add = |st: &mut Store, id: &str, cwd: &str, event: &str, f: &dyn Fn(&mut Event)| {
            let mut e = Event { session_id: id.into(), event: event.into(), cwd: Some(cwd.into()), ..Default::default() };
            f(&mut e);
            st.apply(&e, 1_000).unwrap();
        };
        add(&mut st, "a", "C:\\work\\shop-api", "PreToolUse", &|e| {
            e.activity = Some("chạy cargo test".into());
            e.task = Some("Sửa lỗi đăng nhập khi token hết hạn".into());
        });
        add(&mut st, "b", "C:\\work\\frontend", "Notification", &|e| e.message = Some("Claude cần quyền chạy lệnh Bash".into()));
        add(&mut st, "c", "C:\\work\\thư-viện-rất-dài-tên-để-kiểm-tra-cắt-chữ", "Stop", &|e| e.summary = Some("Đã thêm unit test".into()));
        st.list()
    }

    #[test]
    fn character_stays_inside_its_dirty_rect_in_every_state_and_look() {
        // Anything drawn outside the rect would never be cleared and would smear across frames.
        let (w, h, scale) = (300u32, 620u32, 1.0);
        let (x, y, cw, ch) = char_rect(w, h, scale);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for kind in 0..LOOKS.len() {
            for state in [State::Idle, State::Working, State::Waiting, State::Done, State::Error] {
                for i in 0..200 {
                    paint_character(&mut buf, w, h, scale, kind, state, i as f32 * 0.05, (0, 0));
                }
            }
        }
        for py in 0..h {
            for px in 0..w {
                let inside = px >= x && px < x + cw && py >= y && py < y + ch;
                let a = buf[((py * w + px) * 4 + 3) as usize];
                assert!(inside || a == 0, "pixel drawn outside the dirty rect at {px},{py}");
            }
        }
    }

    #[test]
    fn premultiplied_invariant_holds_after_text() {
        // UpdateLayeredWindow misrenders a color channel larger than alpha.
        let (w, h) = (300u32, 620u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let text = TextDrawer::new(1.0);
        let sessions = sample();
        paint_ui(&mut buf, w, h, 1.0, &text, &View { sessions: &sessions, panel_open: true, hover: Some("b"), note: Some("Không tìm thấy cửa sổ"), now: 90_000, char_region: (CHAR_SIZE, CHAR_SIZE), char_off: (0.0, 0.0), usage: Some("Hôm nay 7,4M · Tuần 49M · Tháng 180M"), bubble: Bubble::All });
        paint_character(&mut buf, w, h, 1.0, 0, State::Waiting, 1.0, (0, 0));
        assert!(buf.chunks_exact(4).any(|p| p[3] > 0));
        for p in buf.chunks_exact(4) {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "not premultiplied: {p:?}");
        }
    }

    #[test]
    fn ui_leaves_the_character_area_untouched_and_hits_cover_rows() {
        let (w, h) = (300u32, 620u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let text = TextDrawer::new(1.0);
        let sessions = sample();
        let hits = paint_ui(&mut buf, w, h, 1.0, &text, &View { sessions: &sessions, panel_open: true, hover: None, note: None, now: 90_000, char_region: (CHAR_SIZE, CHAR_SIZE), char_off: (0.0, 0.0), usage: Some("Hôm nay 7,4M · Tuần 49M · Tháng 180M"), bubble: Bubble::All });
        assert_eq!(hits.iter().filter(|r| matches!(r.hit, Hit::Row(_))).count(), 3);
        assert!(hits.iter().any(|r| r.hit == Hit::Bubble));
        assert_eq!(hits.last().unwrap().hit, Hit::Character);
        let (x, y, cw, ch) = char_rect(w, h, 1.0);
        for py in y..y + ch {
            for px in x..x + cw {
                assert_eq!(buf[((py * w + px) * 4 + 3) as usize], 0, "UI drawn over the character at {px},{py}");
            }
        }
        // Panel fits on screen even with many rows: nothing may reach above the window.
        for r in &hits {
            assert!(r.y >= 0);
        }
    }

    #[test]
    fn media_with_motion_stays_inside_its_dirty_rect() {
        use crate::motion::ALL;
        let (w, h, scale) = (300u32, 620u32, 1.0_f32);
        let (fw, fh) = (200u32, 190u32);
        let pixels = vec![255u8; (fw * fh * 4) as usize];
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let mut region = (0, 0, 0, 0);
        for m in ALL {
            for i in 0..120 {
                let t = i as f32 * 0.035;
                region = paint_media(&mut buf, w, h, scale, &pixels, fw, fh, m.at(t), Some(State::Waiting), t, (0, 0));
            }
        }
        let (x, y, rw, rh) = region;
        for py in 0..h {
            for px in 0..w {
                let inside = px >= x && px < x + rw && py >= y && py < y + rh;
                assert!(inside || buf[((py * w + px) * 4 + 3) as usize] == 0, "smear outside the dirty rect at {px},{py}");
            }
        }
    }

    #[test]
    fn media_of_any_shape_stays_inside_its_dirty_rect() {
        use crate::motion::ALL;
        let (w, h) = (300u32, 620u32);
        for (fw, fh) in [(264u32, 300u32), (264, 40), (30, 300), (200, 190)] {
            let pixels = vec![255u8; (fw * fh * 4) as usize];
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let mut region = (0, 0, 0, 0);
            for m in ALL {
                for i in 0..120 {
                    region = paint_media(&mut buf, w, h, 1.0, &pixels, fw, fh, m.at(i as f32 * 0.035), None, 0.0, (0, 0));
                }
            }
            let (x, y, rw, rh) = region;
            assert_eq!(y + rh, h, "reaches the window's bottom edge");
            for py in 0..h {
                for px in 0..w {
                    let inside = px >= x && px < x + rw && py >= y && py < y + rh;
                    assert!(inside || buf[((py * w + px) * 4 + 3) as usize] == 0, "{fw}x{fh}: smear at {px},{py}");
                }
            }
        }
    }

    #[test]
    fn a_character_moved_anywhere_stays_clean_and_clear_of_the_bubble_and_panel() {
        let (w, h) = (300u32, 620u32);
        let text = TextDrawer::new(1.0);
        let sessions = sample();
        let (stage_w, stage_h) = (200u32, 190u32);
        let pixels = vec![255u8; (stage_w * stage_h * 4) as usize];
        let region = media_region(stage_w, stage_h, 1.0);
        let travel = WIN_H - 4.0 - region.1;
        for (dx, dy) in [(0.0, 0.0), (-50.0, 0.0), (50.0, 0.0), (0.0, -travel / 2.0 + 1.0), (0.0, -travel / 2.0 - 1.0), (-50.0, -travel), (50.0, -travel)] {
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let hits = paint_ui(&mut buf, w, h, 1.0, &text, &View { sessions: &sessions, panel_open: true, hover: None, note: None, now: 90_000, char_region: region, char_off: (dx, dy), usage: Some("x"), bubble: Bubble::All });
            let off = (dx as i32, dy as i32);
            let (x, y, rw, rh) = paint_media(&mut vec![0u8; (w * h * 4) as usize], w, h, 1.0, &pixels, stage_w, stage_h, crate::motion::Motion::None.at(0.0), None, 0.0, off);
            // Nothing else may be drawn where the character gets cleared on every frame.
            for py in y..y + rh {
                for px in x..x + rw {
                    assert_eq!(buf[((py * w + px) * 4 + 3) as usize], 0, "at offset {dx},{dy}: UI under the character at {px},{py}");
                }
            }
            assert!(hits.iter().all(|r| r.y >= 0 && r.y + r.h <= h as i32 + 1), "at offset {dx},{dy}: off the window");
            assert!(hits.iter().filter(|r| matches!(r.hit, Hit::Row(_))).count() >= 1, "at offset {dx},{dy}: no rows fit");
            // Repeated frames with the character cut off by the window edge must not panic or smear.
            let mut b2 = vec![0u8; (w * h * 4) as usize];
            for i in 0..20 {
                let region2 = paint_media(&mut b2, w, h, 1.0, &pixels, stage_w, stage_h, crate::motion::Motion::Nudge.at(1.3 + i as f32 * 0.02), None, 0.0, off);
                assert_eq!(region2, (x, y, rw, rh));
            }
        }
    }

    #[test]
    fn panel_never_runs_off_the_window_with_a_tall_character_and_many_sessions() {
        let (w, h) = (300u32, 620u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let text = TextDrawer::new(1.0);
        let mut st = Store::default();
        for i in 0..20 {
            let e = Event { session_id: format!("s{i}"), event: "Stop".into(), cwd: Some(format!("C:\\p{i}")), ..Default::default() };
            st.apply(&e, 1_000).unwrap();
        }
        let sessions = st.list();
        let hits = paint_ui(&mut buf, w, h, 1.0, &text, &View { sessions: &sessions, panel_open: true, hover: None, note: Some("x"), now: 5_000, char_region: (264.0, 322.0), char_off: (0.0, 0.0), usage: Some("x"), bubble: Bubble::All });
        let rows: Vec<&HitRect> = hits.iter().filter(|r| matches!(r.hit, Hit::Row(_))).collect();
        assert!(!rows.is_empty());
        assert!(hits.iter().all(|r| r.y >= 0), "something above the window");
        let bubble = hits.iter().find(|r| r.hit == Hit::Bubble).unwrap();
        assert!(rows.iter().all(|r| r.y + r.h <= bubble.y), "rows overlap the bubble");
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn text_is_solid_and_keeps_its_colour_on_any_surface() {
        let (w, h) = (120u32, 40u32);
        let text = TextDrawer::new_ui(2.0);
        let darkest = |bg: u8, a: u8, color: u32| {
            let mut buf = vec![0u8; (w * h * 4) as usize];
            buf.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[bg, bg, bg, a]));
            text.draw(&mut buf, w, h, 2, 2, 116, 36, "Hill", Font::Bold, color);
            buf
        };
        // Dark on an opaque light card: stems must reach close to full black, not stay grey.
        let dark = darkest(250, 255, 0x000000);
        let min = dark.chunks_exact(4).map(|p| p[1]).min().unwrap();
        assert!(min < 25, "stems only reach {min}");
        // Light on an opaque dark card
        let light = darkest(30, 255, 0xffffff);
        let max = light.chunks_exact(4).map(|p| p[1]).max().unwrap();
        assert!(max > 230, "stems only reach {max}");
        // On nothing at all (bare Mica): the text brings its own alpha, and stays premultiplied.
        let bare = darkest(0, 0, 0x1b1b1b);
        assert!(bare.chunks_exact(4).map(|p| p[3]).max().unwrap() > 230);
        assert!(bare.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    }

    #[test]
    fn small_updates_land_where_the_full_paint_put_them() {
        let (w, h) = (100u32, 100u32);
        let (fw, fh) = (10u32, 10u32);
        let mut a = vec![0u8; (fw * fh * 4) as usize];
        a[((3 * fw + 4) * 4) as usize..((3 * fw + 5) * 4) as usize].copy_from_slice(&[9, 9, 9, 255]);
        for off in [(0, 0), (-7, -20), (45, 0), (-60, 0)] {
            let mut full = vec![0u8; (w * h * 4) as usize];
            paint_stage(&mut full, w, h, 1.0, &a, fw, fh, Motion::None.at(0.0), None, 0.0, off, false);
            let mut partial = vec![0u8; (w * h * 4) as usize];
            paint_change(&mut partial, w, h, 1.0, &a, fw, fh, (0, 0, fw, fh), off);
            assert_eq!(full, partial, "offset {off:?}");
        }
    }

    #[test]
    fn the_bubble_says_only_what_its_mode_allows() {
        let (w, h) = (300u32, 620u32);
        let text = TextDrawer::new(1.0);
        let sessions = sample(); // one working, one waiting, one done
        let bubble_of = |mode| {
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let v = View { sessions: &sessions, panel_open: false, hover: None, note: None, now: 90_000, char_region: (CHAR_SIZE, CHAR_SIZE), char_off: (0.0, 0.0), usage: None, bubble: mode };
            paint_ui(&mut buf, w, h, 1.0, &text, &v).into_iter().find(|r| r.hit == Hit::Bubble).map(|r| r.h)
        };
        assert_eq!(bubble_of(Bubble::Hidden), None);
        let (some, all) = (bubble_of(Bubble::Attention).unwrap(), bubble_of(Bubble::All).unwrap());
        assert!(some < all, "attention shows fewer lines: {some} vs {all}");
    }

    #[test]
    fn every_size_of_character_stays_clear_of_the_bubble() {
        let (w, h) = (300u32, 620u32);
        let text = TextDrawer::new(1.0);
        let sessions = sample();
        for f in [0.75f32, 1.0, 1.25] {
            let (fw, fh) = ((150.0 * f) as u32, (190.0 * f) as u32);
            let pixels = vec![255u8; (fw * fh * 4) as usize];
            let m = media_region(fw, fh, f);
            let region = (m.0 * f, m.1 * f);
            let mut buf = vec![0u8; (w * h * 4) as usize];
            paint_ui(&mut buf, w, h, 1.0, &text, &View { sessions: &sessions, panel_open: true, hover: None, note: None, now: 1, char_region: region, char_off: (0.0, 0.0), usage: None, bubble: Bubble::All });
            let (x, y, rw, rh) = paint_media(&mut vec![0u8; (w * h * 4) as usize], w, h, f, &pixels, fw, fh, crate::motion::Motion::None.at(0.0), None, 0.0, (0, 0));
            for py in y..y + rh {
                for px in x..x + rw {
                    assert_eq!(buf[((py * w + px) * 4 + 3) as usize], 0, "size {f}: UI under the character at {px},{py}");
                }
            }
        }
    }

    #[test]
    fn the_logo_fits_its_square_at_every_size() {
        for size in [16u32, 24, 32, 48, 256] {
            let px = paint_icon(size, None);
            let s = size as usize;
            let opaque = px.chunks_exact(4).filter(|p| p[3] > 128).count();
            assert!(opaque * 100 / (s * s) > 30, "{size}: logo fills too little");
            for i in 0..s {
                for (x, y) in [(i, 0), (i, s - 1), (0, i), (s - 1, i)] {
                    assert!(px[(y * s + x) * 4 + 3] < 200, "{size}: cut off at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn a_status_dot_changes_the_icon() {
        assert_ne!(paint_icon(32, None), paint_icon(32, Some(state_color(State::Waiting))));
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(30_000, 0), "vừa xong");
        assert_eq!(ago(180_000, 0), "3 phút");
        assert_eq!(ago(7_200_000, 0), "2 giờ");
    }

    /// Regenerates assets/app.ico: cargo test export_app_icon -- --ignored
    #[test]
    #[ignore]
    fn export_app_icon() {
        let sizes = [16u32, 20, 24, 32, 40, 48, 64, 256];
        let images: Vec<Vec<u8>> = sizes
            .iter()
            .map(|s| {
                let mut px = paint_icon(*s, None);
                px.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
                crate::media::encode_png(&px, *s as usize, *s as usize).unwrap()
            })
            .collect();
        let mut ico = vec![0, 0, 1, 0];
        ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * sizes.len() as u32;
        for (s, img) in sizes.iter().zip(&images) {
            let side = if *s == 256 { 0 } else { *s as u8 };
            ico.extend_from_slice(&[side, side, 0, 0, 1, 0, 32, 0]);
            ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
            ico.extend_from_slice(&offset.to_le_bytes());
            offset += img.len() as u32;
        }
        images.iter().for_each(|i| ico.extend_from_slice(i));
        std::fs::create_dir_all("assets").unwrap();
        std::fs::write("assets/app.ico", ico).unwrap();
    }

    /// Writes PNG previews for eyeballing: cargo test preview -- --ignored, output in $PREVIEW_DIR.
    #[test]
    #[ignore]
    fn preview() {
        let dir = std::env::var("PREVIEW_DIR").expect("PREVIEW_DIR");
        // Sheet of every look and state on a mid-gray background.
        let states = [State::Idle, State::Working, State::Waiting, State::Done, State::Error];
        let (cell, sw, sh) = (200u32, 5 * 200u32, 3 * 200u32);
        let mut sheet = vec![0u8; (sw * sh * 4) as usize];
        for (ky, kind) in (0..3).enumerate() {
            for (sx, state) in states.iter().enumerate() {
                let mut pm = PixmapMut::from_bytes(&mut sheet, sw, sh).unwrap();
                let base = Transform::from_translate((sx as u32 * cell) as f32, (ky as u32 * cell) as f32);
                draw_character(&mut pm, base, kind, *state, 0.9);
            }
        }
        write_png(&format!("{dir}/characters.png"), &sheet, sw, sh);

        // The full window, at 100%, with bubble and open panel.
        let (w, h) = (300u32, 620u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let text = TextDrawer::new(1.0);
        let sessions = sample();
        paint_ui(&mut buf, w, h, 1.0, &text, &View { sessions: &sessions, panel_open: true, hover: Some("a"), note: None, now: 200_000, char_region: (CHAR_SIZE, CHAR_SIZE), char_off: (0.0, 0.0), usage: Some("Hôm nay 7,4M · Tuần 49M · Tháng 180M"), bubble: Bubble::All });
        paint_character(&mut buf, w, h, 1.0, 0, State::Waiting, 0.3, (0, 0));
        write_png(&format!("{dir}/window.png"), &buf, w, h);
    }

    /// Premultiplied BGRA composited over gray, as an uncompressed PNG.
    pub fn write_png(path: &str, bgra: &[u8], w: u32, h: u32) {
        let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
        for row in bgra.chunks_exact((w * 4) as usize) {
            raw.push(0);
            for p in row.chunks_exact(4) {
                // Swatch of the desktop the window would float over.
                let bg = 205u32 * (255 - p[3] as u32) / 255;
                raw.extend_from_slice(&[(p[2] as u32 + bg).min(255) as u8, (p[1] as u32 + bg).min(255) as u8, (p[0] as u32 + bg).min(255) as u8]);
            }
        }
        let crc = |d: &[u8]| {
            let mut c = !0u32;
            for b in d {
                c ^= *b as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        };
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |tag: &[u8], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut d = tag.to_vec();
            d.extend_from_slice(data);
            out.extend_from_slice(&d);
            out.extend_from_slice(&crc(&d).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
        for (i, b) in blocks.iter().enumerate() {
            z.push((i + 1 == blocks.len()) as u8);
            z.extend_from_slice(&(b.len() as u16).to_le_bytes());
            z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
            z.extend_from_slice(b);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for x in &raw {
            a = (a + *x as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        chunk(b"IDAT", &z);
        chunk(b"IEND", &[]);
        std::fs::write(path, out).unwrap();
    }
}

#[cfg(test)]
pub use tests::write_png as write_test_png;
