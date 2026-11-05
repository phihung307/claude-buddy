//! Turns any supported media file into plain premultiplied BGRA frames: format sniffing, decoding
//! and resampling. Everything here is meant for a worker thread; playback only ever sees the result.

use crate::i18n::t;
use crate::webm;
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, ImageFormat, ImageReader};
use std::fs;
use std::io::Cursor;
use std::path::Path;
use vp9dec::{Decoder, Frame, PlaneData};

/// Refuse anything bigger: a desktop buddy has no business loading such files.
pub const MAX_MEDIA_BYTES: u64 = 50 * 1024 * 1024;
/// Upper bounds so a hostile or huge file can't take the whole machine's memory.
const MAX_SIDE: u32 = 4096;
const MAX_FRAMES: usize = 1800;
/// Raw frames are held while a file is being prepared. Longer clips are cut off at this much.
const MAX_RAW_BYTES: usize = 768 * 1024 * 1024;
/// Frames closer together than this are dropped: more would not be visible on the buddy's timer.
const MIN_FRAME_MS: u32 = 33;
const DEFAULT_FRAME_MS: u32 = 100;
const SVG_MAX_SIDE: f32 = 640.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Video,
    Animated,
    Static,
    Unknown,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Video => "video",
            Kind::Animated => "animated",
            Kind::Static => "static",
            Kind::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "video" => Kind::Video,
            "animated" => Kind::Animated,
            "static" => Kind::Static,
            _ => return None,
        })
    }
}

fn gif_kind(b: &[u8]) -> Kind {
    if b.len() < 13 {
        return Kind::Unknown;
    }
    let mut p = 13;
    if b[10] & 0x80 != 0 {
        p += 3 * (1usize << ((b[10] & 7) + 1));
    }
    fn skip_sub_blocks(b: &[u8], p: &mut usize) {
        while *p < b.len() && b[*p] != 0 {
            *p += b[*p] as usize + 1;
        }
        *p += 1;
    }
    let mut frames = 0;
    while p < b.len() {
        match b[p] {
            0x3B => break,
            0x21 => {
                p += 2;
                skip_sub_blocks(b, &mut p);
            }
            0x2C => {
                frames += 1;
                if frames > 1 {
                    return Kind::Animated;
                }
                if p + 10 > b.len() {
                    break;
                }
                let local = b[p + 9];
                p += 10;
                if local & 0x80 != 0 {
                    p += 3 * (1usize << ((local & 7) + 1));
                }
                p += 1;
                skip_sub_blocks(b, &mut p);
            }
            _ => break,
        }
    }
    if frames == 1 {
        Kind::Static
    } else {
        Kind::Unknown
    }
}

fn png_kind(b: &[u8]) -> Kind {
    let mut p = 8;
    while p + 8 <= b.len() {
        let len = u32::from_be_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]) as usize;
        match &b[p + 4..p + 8] {
            b"acTL" => return Kind::Animated, // APNG: animation control precedes the image data
            b"IDAT" => return Kind::Static,
            _ => {}
        }
        p = p.saturating_add(12).saturating_add(len);
    }
    Kind::Unknown
}

fn webp_kind(b: &[u8]) -> Kind {
    if b.len() < 16 {
        return Kind::Unknown;
    }
    match &b[12..16] {
        b"VP8 " | b"VP8L" => Kind::Static,
        b"VP8X" if b.len() >= 21 => {
            if b[20] & 0x02 != 0 {
                Kind::Animated
            } else {
                Kind::Static
            }
        }
        _ => Kind::Unknown,
    }
}

/// SVG is text: allow a BOM, an XML prolog, comments and a doctype before the root element.
fn is_svg(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&b[..b.len().min(2048)]);
    let mut s = head.trim_start_matches('\u{feff}').trim_start();
    loop {
        let next = if s.starts_with("<?") {
            s.find("?>").map(|i| i + 2)
        } else if s.starts_with("<!--") {
            s.find("-->").map(|i| i + 3)
        } else if s.len() >= 9 && s.as_bytes()[..9].eq_ignore_ascii_case(b"<!doctype") {
            s.find('>').map(|i| i + 1)
        } else {
            None
        };
        match next {
            Some(i) => s = s[i..].trim_start(),
            None => break,
        }
    }
    let lower = s.as_bytes();
    lower.len() >= 5 && lower[..4].eq_ignore_ascii_case(b"<svg") && (lower[4].is_ascii_whitespace() || lower[4] == b'>')
}

/// Kind plus the extension matching the real format (empty when unknown). Trusts the bytes, not the name.
pub fn sniff(b: &[u8]) -> (Kind, &'static str) {
    let unknown = (Kind::Unknown, "");
    if b.len() < 4 {
        return unknown;
    }
    if b[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return (Kind::Video, ".webm");
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return (Kind::Video, ".mp4");
    }
    if b.len() >= 6 && (&b[..6] == b"GIF87a" || &b[..6] == b"GIF89a") {
        return (gif_kind(b), ".gif");
    }
    if b.len() >= 8 && b[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return (png_kind(b), ".png");
    }
    if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return (webp_kind(b), ".webp");
    }
    if b[..3] == [0xFF, 0xD8, 0xFF] {
        return (Kind::Static, ".jpg");
    }
    if is_svg(b) {
        // resvg draws SVG as a still image; SMIL or CSS animation in the file is not played.
        return (Kind::Static, ".svg");
    }
    unknown
}

/// One decoded frame: premultiplied BGRA.
pub struct Frames {
    pub w: u32,
    pub h: u32,
    pub frames: Vec<(Vec<u8>, u32)>,
}

fn premultiply_rgba_to_bgra(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        let (r, g, b) = (p[0] as u32, p[1] as u32, p[2] as u32);
        p[0] = ((b * a + 127) / 255) as u8;
        p[1] = ((g * a + 127) / 255) as u8;
        p[2] = ((r * a + 127) / 255) as u8;
    }
}

fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(crate::tf!("kích thước {w}x{h} không hợp lệ hoặc quá lớn", "size {w}x{h} is invalid or too large"));
    }
    Ok(())
}

/// Drops frames that come too quickly, handing their time to the frame before them.
fn add_frame(out: &mut Vec<(Vec<u8>, u32)>, pixels: Vec<u8>, delay_ms: u32) {
    let delay = if delay_ms == 0 { DEFAULT_FRAME_MS } else { delay_ms };
    if let Some(last) = out.last_mut() {
        if last.1 < MIN_FRAME_MS {
            last.1 += delay;
            return;
        }
    }
    out.push((pixels, delay));
}

/// How many frames of this size may be kept in memory during preparation.
pub fn frame_limit(w: u32, h: u32) -> usize {
    (MAX_RAW_BYTES / (w as usize * h as usize * 4).max(1)).clamp(1, MAX_FRAMES)
}

fn from_image_frames(decoder: impl AnimationDecoder<'static>, first_only: bool) -> Result<Frames, String> {
    let mut out: Vec<(Vec<u8>, u32)> = Vec::new();
    let (mut w, mut h) = (0, 0);
    for f in decoder.into_frames() {
        let f = f.map_err(|e| e.to_string())?;
        let (n, d) = f.delay().numer_denom_ms();
        let delay = if d == 0 { 0 } else { n / d };
        let mut img = f.into_buffer();
        (w, h) = img.dimensions();
        check_size(w, h)?;
        premultiply_rgba_to_bgra(&mut img);
        add_frame(&mut out, img.into_raw(), delay);
        if first_only || out.len() >= frame_limit(w, h) {
            break;
        }
    }
    if out.is_empty() {
        return Err(t("file không có khung hình nào", "the file has no frames").into());
    }
    Ok(Frames { w, h, frames: out })
}

fn decode_still(data: &[u8], format: ImageFormat) -> Result<Frames, String> {
    let img = ImageReader::with_format(Cursor::new(data), format).decode().map_err(|e| e.to_string())?;
    let mut rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    check_size(w, h)?;
    premultiply_rgba_to_bgra(&mut rgba);
    Ok(Frames { w, h, frames: vec![(rgba.into_raw(), 0)] })
}

fn decode_svg(data: &[u8]) -> Result<Frames, String> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;
    let size = tree.size();
    let k = (SVG_MAX_SIDE / size.width().max(size.height())).max(0.01);
    let (w, h) = (((size.width() * k).ceil() as u32).max(1), ((size.height() * k).ceil() as u32).max(1));
    check_size(w, h)?;
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h).ok_or(t("không tạo được ảnh SVG", "could not create the SVG image"))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pm.as_mut());
    // Pixmap is already premultiplied RGBA: only the channel order differs.
    let mut px = pm.take();
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    Ok(Frames { w, h, frames: vec![(px, 0)] })
}

fn plane8(p: &PlaneData) -> Result<&[u8], String> {
    match p {
        PlaneData::U8(v) => Ok(v),
        PlaneData::U16(_) => Err(t("video 10/12-bit chưa được hỗ trợ", "10/12-bit video isn't supported yet").into()),
    }
}

/// BT.601 limited range, the VP9 default when a file names no matrix; what the browser shows for these.
fn yuv_to_bgra(f: &Frame, alpha: Option<&Frame>) -> Result<Vec<u8>, String> {
    let (w, h) = (f.width as usize, f.height as usize);
    let (y, u, v) = (plane8(&f.y)?, plane8(&f.u)?, plane8(&f.v)?);
    let (sx, sy) = (f.subsampling_x as usize, f.subsampling_y as usize);
    let cw = (w + sx) >> sx;
    if y.len() < w * h || u.len() < cw * ((h + sy) >> sy) || v.len() < u.len() {
        return Err(t("khung hình video bị thiếu dữ liệu", "a video frame is missing data").into());
    }
    let a = match alpha {
        Some(af) if af.width == f.width && af.height == f.height => Some(plane8(&af.y)?),
        Some(_) => return Err(t("luồng alpha khác kích thước luồng màu", "the alpha stream and color stream differ in size").into()),
        None => None,
    };
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let c_row = (row >> sy) * cw;
        for col in 0..w {
            let i = (row * w + col) * 4;
            let al = a.map_or(255, |p| p[row * w + col]) as i32;
            if al == 0 {
                continue;
            }
            let c = (col >> sx) + c_row;
            let yy = 298 * (y[row * w + col] as i32 - 16);
            let (uu, vv) = (u[c] as i32 - 128, v[c] as i32 - 128);
            let r = ((yy + 409 * vv + 128) >> 8).clamp(0, 255);
            let g = ((yy - 100 * uu - 208 * vv + 128) >> 8).clamp(0, 255);
            let b = ((yy + 516 * uu + 128) >> 8).clamp(0, 255);
            out[i] = ((b * al + 127) / 255) as u8;
            out[i + 1] = ((g * al + 127) / 255) as u8;
            out[i + 2] = ((r * al + 127) / 255) as u8;
            out[i + 3] = al as u8;
        }
    }
    Ok(out)
}

/// The picture this packet shows, if any: superframes can hold hidden frames.
fn shown(dec: &mut Decoder, packet: &[u8]) -> Result<Option<Frame>, String> {
    let frames = dec.decode_frame(packet).map_err(|e| crate::tf!("giải mã VP9 lỗi: {e:?}", "VP9 decoding failed: {e:?}"))?;
    Ok(frames.into_iter().rev().find_map(|f| f.frame))
}

fn decode_webm(data: &[u8], first_only: bool) -> Result<Frames, String> {
    let w = webm::parse(data)?;
    if w.codec != "V_VP9" {
        return Err(crate::tf!("chỉ hỗ trợ WebM VP9, file này dùng {}", "only WebM VP9 is supported, this file uses {}", w.codec));
    }
    let (mut color, mut alpha) = (Decoder::new(), Decoder::new());
    let mut last_alpha: Option<Frame> = None;
    let mut out: Vec<(Vec<u8>, u32)> = Vec::new();
    let (mut fw, mut fh) = (0, 0);
    let times: Vec<u64> = w.packets.iter().map(|p| p.time_ms).collect();
    for (i, pkt) in w.packets.iter().enumerate() {
        let picture = shown(&mut color, &pkt.color)?;
        if let Some(a) = &pkt.alpha {
            // Both streams keep their reference state, so the alpha decoder must see every packet too.
            if let Some(f) = shown(&mut alpha, a)? {
                last_alpha = Some(f);
            }
        }
        let Some(frame) = picture else { continue };
        check_size(frame.width, frame.height)?;
        (fw, fh) = (frame.width, frame.height);
        let delay = times.get(i + 1).map_or(0, |next| next.saturating_sub(pkt.time_ms)) as u32;
        let px = yuv_to_bgra(&frame, if w.has_alpha_track { last_alpha.as_ref() } else { None })?;
        add_frame(&mut out, px, delay);
        if first_only || out.len() >= frame_limit(fw, fh) {
            break;
        }
    }
    if out.is_empty() {
        return Err(t("video không có khung hình nào", "the video has no frames").into());
    }
    // The last frame has no successor to measure against: give it its neighbour's duration.
    if out.len() > 1 {
        let n = out.len();
        if times.len() > 1 && times.last().copied().unwrap_or(0) > 0 {
            out[n - 1].1 = out[n - 2].1;
        }
    }
    Ok(Frames { w: fw, h: fh, frames: out })
}

pub fn decode_bytes(data: &[u8], path: &Path, first_only: bool) -> Result<Frames, String> {
    let (kind, ext) = sniff(data);
    if kind == Kind::Unknown {
        return Err(t("không phải ảnh hay video được hỗ trợ", "not a supported image or video").into());
    }
    match ext {
        ".webm" => decode_webm(data, first_only),
        #[cfg(windows)]
        ".mp4" => crate::mf::decode_mp4(path, first_only, MIN_FRAME_MS),
        #[cfg(target_os = "macos")]
        ".mp4" => crate::mp4_mac::decode_mp4(path, first_only, MIN_FRAME_MS),
        #[cfg(not(any(windows, target_os = "macos")))]
        ".mp4" => Err(t("hệ điều hành này chưa đọc được MP4", "MP4 can't be read on this system yet").into()),
        ".gif" => {
            let d = GifDecoder::new(Cursor::new(data.to_vec())).map_err(|e| e.to_string())?;
            from_image_frames(d, first_only)
        }
        ".png" if kind == Kind::Animated => {
            let d = PngDecoder::new(Cursor::new(data.to_vec())).map_err(|e| e.to_string())?;
            from_image_frames(d.apng().map_err(|e| e.to_string())?, first_only)
        }
        ".png" => decode_still(data, ImageFormat::Png),
        ".webp" if kind == Kind::Animated => {
            let d = WebPDecoder::new(Cursor::new(data.to_vec())).map_err(|e| e.to_string())?;
            from_image_frames(d, first_only)
        }
        ".webp" => decode_still(data, ImageFormat::WebP),
        ".jpg" => decode_still(data, ImageFormat::Jpeg),
        ".svg" => decode_svg(data),
        _ => Err(t("không phải ảnh hay video được hỗ trợ", "not a supported image or video").into()),
    }
}

pub fn decode_file(path: &Path, first_only: bool) -> Result<Frames, String> {
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_MEDIA_BYTES {
        return Err(t("file quá lớn (giới hạn 50 MB)", "the file is too large (limit 50 MB)").into());
    }
    decode_bytes(&fs::read(path).map_err(|e| e.to_string())?, path, first_only)
}

/// Visible area over all given frames: (x, y, w, h), or None when everything is transparent.
pub fn alpha_box(frames: &[(Vec<u8>, u32)], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (w as usize, h as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (px, _) in frames {
        for y in 0..h {
            let row = &px[y * w * 4..(y + 1) * w * 4];
            // The middle of a row was already covered by an earlier frame most of the time, so only
            // the parts that could still widen the box are looked at.
            for x in (0..x0.min(w)).chain(x1..w) {
                if row[x * 4 + 3] > 8 {
                    (x0, x1) = (x0.min(x), x1.max(x + 1));
                    (y0, y1) = (y0.min(y), y1.max(y + 1));
                }
            }
            if y < y0 || y + 1 > y1 {
                if let Some(x) = (0..w).find(|x| row[x * 4 + 3] > 8) {
                    (x0, x1) = (x0.min(x), x1.max(x + 1));
                    (y0, y1) = (y0.min(y), y1.max(y + 1));
                }
            }
        }
    }
    (x1 > x0 && y1 > y0).then(|| (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// For each output position: which input samples contribute and how much. Area average when
/// shrinking (keeps thin lines from flickering), linear when enlarging.
fn weights(dst_len: usize, src_start: f32, src_scale: f32, src_len: usize) -> Vec<(usize, Vec<f32>)> {
    let support = src_scale.max(1.0);
    (0..dst_len)
        .map(|i| {
            let center = src_start + (i as f32 + 0.5) * src_scale - 0.5;
            let lo = ((center - support).floor().max(0.0) as usize).min(src_len.saturating_sub(1));
            let hi = ((center + support).ceil().max(0.0) as usize).min(src_len.saturating_sub(1));
            let mut w: Vec<f32> = (lo..=hi).map(|s| (1.0 - ((s as f32 - center).abs() / support)).max(0.0)).collect();
            let sum: f32 = w.iter().sum();
            if sum > 0.0 {
                w.iter_mut().for_each(|x| *x /= sum);
            } else {
                w[0] = 1.0;
            }
            (lo, w)
        })
        .collect()
}

/// Maps the output canvas onto the source: output pixel (0,0) is source (src_x, src_y), and one output
/// pixel covers `scale` source pixels. Output outside the source stays transparent.
pub struct Resampler {
    dw: usize,
    dh: usize,
    sw: usize,
    h: Vec<(usize, Vec<f32>)>,
    v: Vec<(usize, Vec<f32>)>,
    /// Output columns/rows that overlap the source at all
    x_range: (usize, usize),
    y_range: (usize, usize),
    row_buf: Vec<f32>,
}

impl Resampler {
    pub fn new(sw: u32, sh: u32, src_x: f32, src_y: f32, scale: f32, dw: u32, dh: u32) -> Resampler {
        let range = |dst: u32, start: f32, len: u32| {
            let a = ((-start / scale).ceil().max(0.0) as usize).min(dst as usize);
            let b = (((len as f32 - start) / scale).floor().max(0.0) as usize).min(dst as usize);
            (a, b.max(a))
        };
        Resampler {
            dw: dw as usize,
            dh: dh as usize,
            sw: sw as usize,
            h: weights(dw as usize, src_x, scale, sw as usize),
            v: weights(dh as usize, src_y, scale, sh as usize),
            x_range: range(dw, src_x, sw),
            y_range: range(dh, src_y, sh),
            row_buf: Vec::new(),
        }
    }

    pub fn run(&mut self, src: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; self.dw * self.dh * 4];
        let (x0, x1) = self.x_range;
        let width = x1 - x0;
        if width == 0 {
            return out;
        }
        let needed_rows = src.len() / (self.sw * 4);
        // Horizontal pass, lazily per source row, so the cost follows the area actually needed.
        let mut cache: Vec<Option<Vec<f32>>> = vec![None; needed_rows];
        let mut accum = std::mem::take(&mut self.row_buf);
        accum.clear();
        accum.resize(width * 4, 0.0);
        for y in self.y_range.0..self.y_range.1 {
            accum.iter_mut().for_each(|a| *a = 0.0);
            let (first, vw) = &self.v[y];
            for (k, weight) in vw.iter().enumerate() {
                let sy = first + k;
                if *weight <= 0.0 {
                    continue;
                }
                let row = cache[sy].get_or_insert_with(|| {
                    let line = &src[sy * self.sw * 4..(sy + 1) * self.sw * 4];
                    let mut r = vec![0f32; width * 4];
                    for (i, x) in (x0..x1).enumerate() {
                        let (fx, hw) = &self.h[x];
                        let px = &mut r[i * 4..i * 4 + 4];
                        for (j, w) in hw.iter().enumerate() {
                            let s = &line[(fx + j) * 4..(fx + j) * 4 + 4];
                            px[0] += s[0] as f32 * w;
                            px[1] += s[1] as f32 * w;
                            px[2] += s[2] as f32 * w;
                            px[3] += s[3] as f32 * w;
                        }
                    }
                    r
                });
                for (a, r) in accum.iter_mut().zip(row.iter()) {
                    *a += r * weight;
                }
            }
            let line = &mut out[y * self.dw * 4 + x0 * 4..y * self.dw * 4 + x1 * 4];
            for (d, a) in line.iter_mut().zip(accum.iter()) {
                *d = (a + 0.5) as u8;
            }
        }
        self.row_buf = accum;
        out
    }
}

/// Removes a white background from a straight-alpha RGBA still: flood fill from the border through
/// near-white pixels, plus enclosed white islands big enough not to be a highlight or teeth.
/// Returns the number of pixels made fully transparent.
pub fn remove_white_background(px: &mut [u8], w: usize, h: usize) -> usize {
    const HARD: i32 = 8;
    const SOFT: i32 = 26;
    const MIN_CORE: usize = 300;
    let dist = |px: &[u8], p: usize| {
        let i = p * 4;
        (255 - px[i] as i32).max(255 - px[i + 1] as i32).max(255 - px[i + 2] as i32)
    };
    let mut bg = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let border = (0..w).flat_map(|x| [x, (h - 1) * w + x]).chain((0..h).flat_map(|y| [y * w, y * w + w - 1]));
    for p in border {
        if !bg[p] && dist(px, p) < SOFT {
            bg[p] = true;
            stack.push(p);
        }
    }
    let neighbours = |p: usize| {
        let (x, y) = (p % w, p / w);
        [
            (x > 0).then(|| p - 1),
            (x + 1 < w).then(|| p + 1),
            (y > 0).then(|| p - w),
            (y + 1 < h).then(|| p + w),
        ]
    };
    while let Some(p) = stack.pop() {
        for q in neighbours(p).into_iter().flatten() {
            if !bg[q] && dist(px, q) < SOFT {
                bg[q] = true;
                stack.push(q);
            }
        }
    }
    let mut seen = vec![false; w * h];
    for start in 0..w * h {
        if bg[start] || seen[start] || dist(px, start) >= SOFT {
            continue;
        }
        seen[start] = true;
        let mut island = vec![start];
        let (mut k, mut core) = (0, 0);
        while k < island.len() {
            let p = island[k];
            k += 1;
            if dist(px, p) <= HARD {
                core += 1;
            }
            for q in neighbours(p).into_iter().flatten() {
                if !seen[q] && !bg[q] && dist(px, q) < SOFT {
                    seen[q] = true;
                    island.push(q);
                }
            }
        }
        if core >= MIN_CORE {
            for p in island {
                bg[p] = true;
            }
        }
    }
    let mut cleared = 0;
    for p in 0..w * h {
        if !bg[p] {
            continue;
        }
        let d = dist(px, p);
        let a = if d <= HARD { 0.0 } else { (d - HARD) as f32 / (SOFT - HARD) as f32 };
        if a == 0.0 {
            cleared += 1;
        }
        let i = p * 4;
        // Un-blend from white so semi-transparent edge pixels don't keep a light halo.
        for c in 0..3 {
            px[i + c] = if a > 0.0 { ((px[i + c] as f32 - 255.0 * (1.0 - a)) / a).round().clamp(0.0, 255.0) as u8 } else { 0 };
        }
        px[i + 3] = (a * 255.0).round() as u8;
    }
    cleared
}

/// True when most of the image border is opaque near-white, i.e. it has no real transparency.
pub fn has_white_border(px: &[u8], w: usize, h: usize) -> bool {
    if w < 2 || h < 2 {
        return false;
    }
    let is_white = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        px[i + 3] > 250 && (255 - px[i] as i32).max(255 - px[i + 1] as i32).max(255 - px[i + 2] as i32) < 26
    };
    let top = (0..w).flat_map(|x| [(x, 0), (x, h - 1)]);
    let sides = (1..h - 1).flat_map(|y| [(0, y), (w - 1, y)]);
    let (mut white, mut total) = (0usize, 0usize);
    for (x, y) in top.chain(sides) {
        total += 1;
        white += is_white(x, y) as usize;
    }
    white * 10 > total * 9
}

/// Straight-alpha RGBA of a still image, for the white background tools.
pub fn load_rgba(path: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&data).map_err(|e| e.to_string())?.into_rgba8();
    let (w, h) = img.dimensions();
    check_size(w, h)?;
    Ok((img.into_raw(), w as usize, h as usize))
}

pub fn encode_png(rgba: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::write_buffer_with_format(&mut Cursor::new(&mut out), rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8, ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_by_content_not_name() {
        assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3, 0]).1, ".webm");
        assert_eq!(sniff(b"\0\0\0\x18ftypisom\0\0").1, ".mp4");
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]).1, ".jpg");
        assert_eq!(sniff(b"<?xml version=\"1.0\"?><!-- c --><svg width=\"1\"/>"), (Kind::Static, ".svg"));
        assert_eq!(sniff(b"\xef\xbb\xbf<!DOCTYPE svg><svg></svg>").1, ".svg");
        assert_eq!(sniff(b"<svgish></svgish>").0, Kind::Unknown);
        assert_eq!(sniff(b"hello world, not media").0, Kind::Unknown);
        assert_eq!(sniff(b"").0, Kind::Unknown);
    }

    fn png_chunks(types: &[&[u8; 4]]) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        for t in types {
            b.extend_from_slice(&0u32.to_be_bytes());
            b.extend_from_slice(*t);
            b.extend_from_slice(&[0; 4]);
        }
        b
    }

    #[test]
    fn tells_apng_from_png() {
        assert_eq!(sniff(&png_chunks(&[b"IHDR", b"IDAT"])), (Kind::Static, ".png"));
        assert_eq!(sniff(&png_chunks(&[b"IHDR", b"acTL", b"IDAT"])), (Kind::Animated, ".png"));
        assert_eq!(sniff(&png_chunks(&[b"IHDR"])).0, Kind::Unknown);
    }

    #[test]
    fn tells_animated_webp() {
        let mut v = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\x02\0\0\0".to_vec();
        assert_eq!(sniff(&v), (Kind::Animated, ".webp"));
        v[20] = 0;
        assert_eq!(sniff(&v).0, Kind::Static);
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0").0, Kind::Static);
    }

    #[test]
    fn counts_gif_frames() {
        let header = |extra: &[u8]| [b"GIF89a".as_slice(), &[1, 0, 1, 0, 0, 0, 0], extra].concat();
        let frame = [0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 1, 0, 0];
        assert_eq!(sniff(&header(&frame)), (Kind::Static, ".gif"));
        assert_eq!(sniff(&header(&[frame, frame].concat())).0, Kind::Animated);
        assert_eq!(sniff(&header(&[])).0, Kind::Unknown);
    }

    #[test]
    fn hostile_sniff_input_never_panics() {
        for n in 0..64 {
            let junk: Vec<u8> = (0..n).map(|i| (i * 37) as u8).collect();
            let _ = sniff(&junk);
            let mut gif = b"GIF89a".to_vec();
            gif.extend_from_slice(&junk);
            let _ = sniff(&gif);
            let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            png.extend_from_slice(&junk);
            let _ = sniff(&png);
            let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
            webp.extend_from_slice(&junk);
            let _ = sniff(&webp);
        }
        let _ = sniff(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0xFF, 0xFF, 0xFF, 0xFF, b'x', b'x', b'x', b'x']);
    }

    #[test]
    fn premultiplies_and_swaps() {
        let mut p = [200, 100, 50, 128];
        premultiply_rgba_to_bgra(&mut p);
        assert_eq!(p, [25, 50, 100, 128]);
    }

    #[test]
    fn fast_frames_merge_into_the_previous() {
        let mut out = Vec::new();
        add_frame(&mut out, vec![1], 16);
        add_frame(&mut out, vec![2], 16);
        add_frame(&mut out, vec![3], 16);
        add_frame(&mut out, vec![4], 16);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (vec![1], 32 + 16));
        let mut out = Vec::new();
        add_frame(&mut out, vec![1], 0);
        assert_eq!(out[0].1, DEFAULT_FRAME_MS);
    }

    #[test]
    fn alpha_box_covers_every_frame() {
        let (w, h) = (10usize, 10usize);
        let frame = |x: usize, y: usize| {
            let mut f = vec![0u8; w * h * 4];
            f[(y * w + x) * 4 + 3] = 255;
            (f, 0)
        };
        assert_eq!(alpha_box(&[frame(2, 3), frame(7, 8)], 10, 10), Some((2, 3, 6, 6)));
        assert_eq!(alpha_box(&[frame(7, 8), frame(2, 3)], 10, 10), Some((2, 3, 6, 6)));
        assert_eq!(alpha_box(&[frame(5, 5)], 10, 10), Some((5, 5, 1, 1)));
        assert_eq!(alpha_box(&[(vec![0u8; 400], 0)], 10, 10), None);
    }

    #[test]
    fn resampling_keeps_flat_color_and_premultiplied_form() {
        let src: Vec<u8> = [50u8, 100, 150, 200].repeat(64 * 64);
        for (dw, scale) in [(16, 4.0), (128, 0.5), (64, 1.0), (47, 1.361)] {
            let mut r = Resampler::new(64, 64, 0.0, 0.0, scale, dw, dw);
            let out = r.run(&src);
            let centre = ((dw / 2 * dw + dw / 2) * 4) as usize;
            assert_eq!(&out[centre..centre + 4], &[50, 100, 150, 200], "scale {scale}");
        }
    }

    #[test]
    fn resampling_places_the_crop_and_leaves_the_rest_transparent() {
        let mut src = vec![0u8; 4 * 4 * 4];
        src[(4 + 1) * 4..(4 + 1) * 4 + 4].copy_from_slice(&[9, 9, 9, 255]); // pixel (1,1)
        // Canvas starting at source (1,1): that pixel lands on (0,0).
        let out = Resampler::new(4, 4, 1.0, 1.0, 1.0, 3, 3).run(&src);
        assert_eq!(&out[..4], &[9, 9, 9, 255]);
        // Canvas starting left of the source: the first column has nothing to show.
        let out = Resampler::new(4, 4, -2.0, 0.0, 1.0, 4, 4).run(&src);
        assert_eq!(&out[(4 + 3) * 4..(4 + 3) * 4 + 4], &[9, 9, 9, 255]);
        assert_eq!(out[3], 0);
    }

    fn white_image_with_box(w: usize, h: usize) -> Vec<u8> {
        let mut px = vec![255u8; w * h * 4];
        for y in h / 4..h * 3 / 4 {
            for x in w / 4..w * 3 / 4 {
                px[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[10, 20, 30]);
            }
        }
        px
    }

    #[test]
    fn white_background_goes_and_the_subject_stays() {
        let (w, h) = (40, 40);
        let mut px = white_image_with_box(w, h);
        assert!(has_white_border(&px, w, h));
        assert!(remove_white_background(&mut px, w, h) > 0);
        assert_eq!(px[3], 0, "corner is transparent");
        let c = (h / 2 * w + w / 2) * 4;
        assert_eq!(&px[c..c + 4], &[10, 20, 30, 255], "subject untouched");
        assert!(!has_white_border(&px, w, h));
    }

    #[test]
    fn small_white_details_survive_but_big_enclosed_holes_do_not() {
        let (w, h) = (60, 60);
        let mut px = vec![0u8; w * h * 4];
        px.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[10, 20, 30, 255]));
        let mut paint = |x0: usize, y0: usize, size: usize| {
            for y in y0..y0 + size {
                for x in x0..x0 + size {
                    px[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[255, 255, 255]);
                }
            }
        };
        paint(5, 5, 4); // a highlight: 16 pixels
        paint(20, 20, 25); // a hole between arm and body: 625 pixels
        remove_white_background(&mut px, w, h);
        assert_eq!(px[((6 * w + 6) * 4) + 3], 255, "highlight kept");
        assert_eq!(px[((30 * w + 30) * 4) + 3], 0, "enclosed hole removed");
    }

    #[test]
    fn png_round_trip_keeps_alpha() {
        let rgba = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let png = encode_png(&rgba, 2, 1).unwrap();
        assert_eq!(sniff(&png), (Kind::Static, ".png"));
        let back = image::load_from_memory(&png).unwrap().into_rgba8();
        assert_eq!(back.into_raw(), rgba);
    }
}

/// Decoder checks against real files; they need the unpacked original app, so they only run on request:
/// BUDDY_SAMPLES=<characters folder> cargo test --release real_ -- --ignored --nocapture
#[cfg(test)]
mod real_files {
    use super::*;
    use std::path::PathBuf;

    fn samples() -> PathBuf {
        PathBuf::from(std::env::var("BUDDY_SAMPLES").expect("BUDDY_SAMPLES"))
    }

    /// Same crop as the thumbnail the browser made: first idle frame, union box of sampled frames,
    /// centered on 128x128.
    fn thumb_from(frames: &Frames) -> Vec<u8> {
        const S: f32 = 128.0;
        let n = frames.frames.len();
        let step = (n / 24).max(1);
        let sampled: Vec<(Vec<u8>, u32)> = frames.frames.iter().step_by(step).cloned().collect();
        let (x, y, w, h) = alpha_box(&sampled, frames.w, frames.h).unwrap();
        let k = (S - 8.0) / w.max(h) as f32;
        let (ox, oy) = ((S - w as f32 * k) / 2.0, (S - h as f32 * k) / 2.0);
        let mut r = Resampler::new(frames.w, frames.h, x as f32 - ox / k, y as f32 - oy / k, 1.0 / k, 128, 128);
        r.run(&frames.frames[0].0)
    }

    /// Mean absolute error per channel over the picture, against the premultiplied reference.
    fn compare(character: &str) -> (f64, f64) {
        let dir = samples().join(character);
        let started = std::time::Instant::now();
        let frames = decode_file(&dir.join("idle.webm"), false).unwrap();
        let secs = started.elapsed().as_secs_f64();
        let ours = thumb_from(&frames);
        let reference = image::open(dir.join("thumb.png")).unwrap().into_rgba8();
        assert_eq!(reference.dimensions(), (128, 128));
        let mut theirs = reference.into_raw();
        premultiply_rgba_to_bgra(&mut theirs);
        let (mut color, mut alpha, mut n, mut na) = (0f64, 0f64, 0f64, 0f64);
        for (a, b) in ours.chunks_exact(4).zip(theirs.chunks_exact(4)) {
            alpha += (a[3] as f64 - b[3] as f64).abs();
            na += 1.0;
            for c in 0..3 {
                color += (a[c] as f64 - b[c] as f64).abs();
                n += 1.0;
            }
        }
        println!("{character}: {} frames {}x{}, decode {secs:.2}s, color MAE {:.2}, alpha MAE {:.2}", frames.frames.len(), frames.w, frames.h, color / n, alpha / na);
        (color / n, alpha / na)
    }

    #[test]
    #[ignore]
    fn real_webm_matches_the_browsers_rendering() {
        for c in ["bear", "black-cat", "guidang-potato", "minato", "uncle-steve"] {
            let (color, alpha) = compare(c);
            assert!(color < 6.0 && alpha < 6.0, "{c} differs from the browser: color {color:.2}, alpha {alpha:.2}");
        }
    }

    #[test]
    #[ignore]
    fn real_mp4_decodes() {
        let path = PathBuf::from(std::env::var("BUDDY_MP4").expect("BUDDY_MP4"));
        let f = decode_file(&path, false).unwrap();
        println!("mp4: {} frames {}x{}", f.frames.len(), f.w, f.h);
        assert!(!f.frames.is_empty());
        let (px, _) = &f.frames[f.frames.len() / 2];
        assert!(px.chunks_exact(4).all(|p| p[3] == 255), "mp4 has no transparency");
        assert!(px.chunks_exact(4).any(|p| p[0] != px[0] || p[1] != px[1]), "picture is not blank");
    }
}
