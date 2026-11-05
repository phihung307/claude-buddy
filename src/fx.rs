//! Skill effects, drawn by code so they work with any character and need no extra art: the flash and
//! the kunai mark of the teleport, and the smoke a clone appears and vanishes in. Each draws one moment
//! of its effect, `p` going from 0 to 1 over its life, on top of what the buffer already holds.

use crate::render::{col, oval, paint};
use std::f32::consts::TAU;
use tiny_skia::{BlendMode, FillRule, FilterQuality, GradientStop, LineCap, Paint, PathBuilder, PixmapMut, PixmapPaint, PixmapRef, Point, RadialGradient, SpreadMode, Stroke, Transform};

const KUNAI: u32 = 0xffcc33;
const LIGHT: u32 = 0xfffbe0;
const STEEL: u32 = 0x3b4150;
const SMOKE: u32 = 0xf4f4f6;
const SMOKE_SHADE: u32 = 0xb8bcc6;

fn ease_out(p: f32) -> f32 {
    1.0 - (1.0 - p.clamp(0.0, 1.0)).powi(3)
}

fn glow(pm: &mut PixmapMut, cx: f32, cy: f32, r: f32, core: u32, edge: u32, alpha: f32) {
    let c = Point::from_xy(cx, cy);
    let stops = vec![GradientStop::new(0.0, col(core, alpha)), GradientStop::new(0.35, col(edge, alpha * 0.7)), GradientStop::new(1.0, col(edge, 0.0))];
    let (Some(shader), Some(path)) = (RadialGradient::new(c, 0.0, c, r.max(0.5), stops, SpreadMode::Pad, Transform::identity()), oval(cx, cy, r.max(0.5), r.max(0.5))) else { return };
    let mut p = Paint::default();
    p.shader = shader;
    p.anti_alias = true;
    pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
}

fn line(pm: &mut PixmapMut, from: (f32, f32), to: (f32, f32), width: f32, color: u32, alpha: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(from.0, from.1);
    pb.line_to(to.0, to.1);
    let Some(path) = pb.finish() else { return };
    let s = Stroke { width: width.max(0.5), line_cap: LineCap::Round, ..Stroke::default() };
    pm.stroke_path(&path, &paint(col(color, alpha)), &s, Transform::identity(), None);
}

/// The Flying Thunder God's flash, filling the buffer: a burst of yellow light, rays and a ring racing out.
pub fn paint_flash(buf: &mut [u8], w: u32, h: u32, p: f32) {
    let Some(mut pm) = PixmapMut::from_bytes(buf, w, h) else { return };
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let big = cx.min(cy);
    let (out, fade) = (ease_out(p), (1.0 - p).clamp(0.0, 1.0));
    glow(&mut pm, cx, cy, big * (0.3 + 0.7 * out), LIGHT, KUNAI, fade);
    for i in 0..8 {
        let a = i as f32 * TAU / 8.0 + 0.4;
        let (r0, r1) = (big * (0.1 + 0.6 * out), big * (0.3 + 0.7 * out));
        line(&mut pm, (cx + a.cos() * r0, cy + a.sin() * r0), (cx + a.cos() * r1, cy + a.sin() * r1), big * 0.035 * fade, LIGHT, fade);
    }
    let r = big * (0.15 + 0.8 * out);
    if let Some(path) = oval(cx, cy, r, r) {
        let s = Stroke { width: (big * 0.05 * fade).max(0.5), ..Stroke::default() };
        pm.stroke_path(&path, &paint(col(KUNAI, fade)), &s, Transform::identity(), None);
    }
}

/// The mark the teleport lands on: a kunai stuck in the ground in a pulse of light.
pub fn paint_mark(buf: &mut [u8], w: u32, h: u32, p: f32) {
    let Some(mut pm) = PixmapMut::from_bytes(buf, w, h) else { return };
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let big = cx.min(cy);
    let grow = ease_out(p * 2.5);
    glow(&mut pm, cx, cy, big * 0.55 * grow, LIGHT, KUNAI, 0.85);
    // Point down: tip, the blade's widest, its top, then the grip and the ring.
    let l = big * 0.7 * grow;
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy + l * 0.45);
    pb.line_to(cx + l * 0.13, cy + l * 0.02);
    pb.line_to(cx, cy - l * 0.08);
    pb.line_to(cx - l * 0.13, cy + l * 0.02);
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &paint(col(STEEL, 1.0)), FillRule::Winding, Transform::identity(), None);
    }
    line(&mut pm, (cx, cy - l * 0.08), (cx, cy - l * 0.36), l * 0.06, STEEL, 1.0);
    if let Some(path) = oval(cx, cy - l * 0.43, l * 0.07, l * 0.07) {
        let s = Stroke { width: (l * 0.03).max(0.5), ..Stroke::default() };
        pm.stroke_path(&path, &paint(col(STEEL, 1.0)), &s, Transform::identity(), None);
    }
}

/// A puff of smoke of radius about `r` around (cx, cy): billows burst out, then thin away.
pub fn paint_smoke(buf: &mut [u8], w: u32, h: u32, cx: f32, cy: f32, r: f32, p: f32) {
    let Some(mut pm) = PixmapMut::from_bytes(buf, w, h) else { return };
    let (out, fade) = (ease_out(p), (1.0 - p).clamp(0.0, 1.0).powf(0.8));
    // Fixed angles and sizes, so the same puff never flickers between frames.
    const PUFFS: [(f32, f32); 9] = [(0.0, 1.0), (0.7, 0.8), (1.4, 0.95), (2.1, 0.75), (2.8, 0.9), (3.5, 0.8), (4.2, 1.0), (4.9, 0.7), (5.6, 0.85)];
    for (shade, color, dy) in [(true, SMOKE_SHADE, r * 0.08), (false, SMOKE, 0.0)] {
        for (a, size) in PUFFS {
            let d = r * (0.15 + 0.55 * out);
            let pr = r * size * (0.32 + 0.12 * out) * if shade { 1.05 } else { 1.0 };
            if let Some(path) = oval(cx + a.cos() * d, cy + a.sin() * d * 0.8 + dy, pr, pr) {
                pm.fill_path(&path, &paint(col(color, fade * 0.9)), FillRule::Winding, Transform::identity(), None);
            }
        }
        if let Some(path) = oval(cx, cy + dy, r * 0.45 * (1.0 - 0.3 * out), r * 0.45 * (1.0 - 0.3 * out)) {
            pm.fill_path(&path, &paint(col(color, fade * 0.9)), FillRule::Winding, Transform::identity(), None);
        }
    }
}

/// One frame of a character, `k` times its prepared size, with its feet at `feet`.
#[allow(clippy::too_many_arguments)]
pub fn paint_frame(buf: &mut [u8], w: u32, h: u32, pixels: &[u8], fw: u32, fh: u32, k: f32, feet: (f32, f32), opacity: f32) {
    let (Some(mut pm), Some(src)) = (PixmapMut::from_bytes(buf, w, h), PixmapRef::from_bytes(pixels, fw, fh)) else { return };
    let quality = if (k - 1.0).abs() < 0.001 { FilterQuality::Nearest } else { FilterQuality::Bilinear };
    let tf = Transform::from_translate(feet.0.round(), feet.1.round()).pre_scale(k, k).pre_translate(-(fw as f32) / 2.0, -(fh as f32));
    pm.draw_pixmap(0, 0, src, &PixmapPaint { opacity: opacity.clamp(0.0, 1.0), blend_mode: BlendMode::SourceOver, quality }, tf, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(buf: &[u8]) -> usize {
        buf.chunks_exact(4).filter(|p| p[3] > 0).count()
    }

    #[test]
    fn effects_fade_out_to_nothing() {
        for f in [paint_flash as fn(&mut [u8], u32, u32, f32)] {
            let mut buf = vec![0u8; 64 * 64 * 4];
            f(&mut buf, 64, 64, 0.3);
            assert!(lit(&buf) > 100);
            let mut buf = vec![0u8; 64 * 64 * 4];
            f(&mut buf, 64, 64, 1.0);
            assert_eq!(lit(&buf), 0);
        }
        let mut buf = vec![0u8; 64 * 64 * 4];
        paint_smoke(&mut buf, 64, 64, 32.0, 32.0, 20.0, 1.0);
        assert_eq!(lit(&buf), 0);
        paint_smoke(&mut buf, 64, 64, 32.0, 32.0, 20.0, 0.2);
        assert!(lit(&buf) > 100);
    }

    #[test]
    fn the_mark_shows_from_early_on() {
        let mut buf = vec![0u8; 64 * 64 * 4];
        paint_mark(&mut buf, 64, 64, 0.2);
        assert!(lit(&buf) > 50);
    }

    #[test]
    fn a_frame_stands_on_its_feet() {
        let pixels = vec![255u8; 10 * 20 * 4];
        let mut buf = vec![0u8; 40 * 40 * 4];
        paint_frame(&mut buf, 40, 40, &pixels, 10, 20, 1.0, (20.0, 35.0), 1.0);
        let at = |x: u32, y: u32| buf[((y * 40 + x) * 4 + 3) as usize];
        assert_eq!(at(20, 34), 255);
        assert_eq!(at(20, 15), 255);
        assert_eq!(at(20, 36), 0);
        assert_eq!(at(20, 14), 0);
        assert_eq!(at(14, 25), 0);
    }
}
