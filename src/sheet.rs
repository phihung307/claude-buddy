//! Turns sprite sheets into animated characters. A sheet holds one state's frames drawn side by side. Image
//! generators don't keep to a grid, so each figure is found by the empty space around it, and frames are then
//! lined up on the body so the character stays in place instead of jumping from frame to frame.

use crate::i18n::t;
use crate::library::STATE_KEYS;
use image::RgbaImage;
use std::fs;
use std::io::Read;
use std::path::Path;

/// How long each frame shows.
const FRAME_MS: u16 = 125;
const MAX_SHEET_BYTES: u64 = 40 * 1024 * 1024;
const MAX_SHEET_SIDE: u32 = 4096;
const MAX_FRAME_SIDE: u32 = 640;
const MIN_FRAMES: usize = 4;
const MAX_FRAMES: usize = 36;
const MARGIN: u32 = 8;
const SOLID: u8 = 128;
const SHARPEN_SIGMA: f32 = 0.8;
const SHARPEN_THRESHOLD: i32 = 6;
const MAX_ZIP_ENTRIES: usize = 256;

struct Frame {
    image: RgbaImage,
    /// Where the turning axis is, across the image.
    axis: f32,
    /// The lowest solid row: what stands on the ground.
    bottom: u32,
}

fn runs(flags: &[bool], min_len: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, on) in flags.iter().chain(std::iter::once(&false)).enumerate() {
        match (*on, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= min_len {
                    out.push((s, i));
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// Each figure's rectangle, in reading order.
fn find_figures(sheet: &RgbaImage) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = sheet.dimensions();
    let solid = |x: u32, y: u32| sheet.get_pixel(x, y)[3] >= SOLID;
    let rows: Vec<bool> = (0..h).map(|y| (0..w).any(|x| solid(x, y))).collect();
    let mut bands = Vec::new();
    for (y0, y1) in runs(&rows, (h as usize / 25).max(1)) {
        let cols: Vec<bool> = (0..w).map(|x| (y0 as u32..y1 as u32).any(|y| solid(x, y))).collect();
        bands.push((y0 as u32, y1 as u32, runs(&cols, (w as usize / 100).max(1))));
    }
    let mut widths: Vec<usize> = bands.iter().flat_map(|b| b.2.iter().map(|c| c.1 - c.0)).collect();
    widths.sort_unstable();
    let typical = widths.get(widths.len() / 2).copied().unwrap_or(0);
    let mut out = Vec::new();
    for (y0, y1, cols) in bands {
        // A burst or a slash can sit apart from the body it belongs to: a sliver joins its neighbor.
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for c in cols {
            match merged.last_mut() {
                Some(last) if (c.1 - c.0) * 10 < typical * 3 || (last.1 - last.0) * 10 < typical * 3 => last.1 = c.1,
                _ => merged.push(c),
            }
        }
        out.extend(merged.into_iter().map(|(x0, x1)| (x0 as u32, y0, x1 as u32, y1)));
    }
    out
}

fn middle_of(image: &RgbaImage, y0: u32, y1: u32) -> Option<f32> {
    let solid = |x: &u32| (y0..y1).any(|y| image.get_pixel(*x, y)[3] >= SOLID);
    let left = (0..image.width()).find(|x| solid(x))?;
    let right = (0..image.width()).rev().find(|x| solid(x))?;
    Some((left + right) as f32 / 2.0)
}

/// The head and the feet both sit on the axis from any angle, each with its own drawing error. Taking the point
/// between them, with the difference evened out around the loop, is what keeps a turn on the spot; the feet
/// alone slide about, since they change shape with every view.
fn cut(sheet: &RgbaImage) -> Result<Vec<Frame>, String> {
    let figures = find_figures(sheet);
    if !(MIN_FRAMES..=MAX_FRAMES).contains(&figures.len()) {
        return Err(crate::tf!("tìm thấy {} hình, cần từ {MIN_FRAMES} đến {MAX_FRAMES}", "found {} figures, needs {MIN_FRAMES} to {MAX_FRAMES}", figures.len()));
    }
    let mut measured = Vec::with_capacity(figures.len());
    for (x0, y0, x1, y1) in figures {
        let image = image::imageops::crop_imm(sheet, x0, y0, x1 - x0, y1 - y0).to_image();
        let rows: Vec<u32> = (0..image.height()).filter(|y| (0..image.width()).any(|x| image.get_pixel(x, *y)[3] >= SOLID)).collect();
        let (top, bottom) = (*rows.first().unwrap_or(&0), *rows.last().unwrap_or(&0));
        let tall = (bottom - top) as f32;
        let at = |f: f32| top + (tall * f) as u32;
        // Where the head is widest, which leaves out the ears.
        let width = |y: u32| (0..image.width()).filter(|x| image.get_pixel(*x, y)[3] >= SOLID).count();
        let widest = (at(0.1)..at(0.55).max(at(0.1) + 1)).max_by_key(|y| width(*y)).unwrap_or(top);
        let center = image.width() as f32 / 2.0;
        let head = middle_of(&image, widest.saturating_sub(3), (widest + 3).min(image.height())).unwrap_or(center);
        let feet = middle_of(&image, at(0.92), bottom + 1).unwrap_or(center);
        measured.push((image, head, feet - head, bottom));
    }
    let n = measured.len();
    let window = 2.min((n - 1) / 2);
    let lean: Vec<f32> = (0..n).map(|i| (0..=2 * window).map(|k| measured[(i + k + n - window) % n].2).sum::<f32>() / (2 * window + 1) as f32).collect();
    Ok(measured.into_iter().zip(lean).map(|((image, head, _, bottom), lean)| Frame { image, axis: head + lean / 2.0, bottom }).collect())
}

fn load_sheet(path: &Path) -> Result<RgbaImage, String> {
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_SHEET_BYTES {
        return Err(t("ảnh quá lớn", "the image is too large").into());
    }
    let image = image::load_from_memory(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?.into_rgba8();
    let (w, h) = image.dimensions();
    if w > MAX_SHEET_SIDE || h > MAX_SHEET_SIDE {
        return Err(crate::tf!("ảnh quá lớn ({w}x{h})", "the image is too large ({w}x{h})"));
    }
    let clear = image.pixels().filter(|p| p[3] == 0).count();
    if clear * 100 < (w * h) as usize * 15 {
        return Err(t("ảnh không có nền trong suốt", "the image has no transparent background").into());
    }
    Ok(image)
}

fn write_animation(frames: &[RgbaImage], path: &Path) -> Result<(), String> {
    let (w, h) = frames[0].dimensions();
    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_animated(frames.len() as u32, 0).map_err(|e| e.to_string())?;
    encoder.set_frame_delay(FRAME_MS, 1000).map_err(|e| e.to_string())?;
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    for frame in frames {
        writer.write_image_data(frame.as_raw()).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())
}

/// Cuts idle.png, working.png and the others in `dir` into animations of the same name in `out`. All states share
/// one canvas and one ground line, so the character keeps its size and place when its state changes.
pub fn cut_all(dir: &Path, out: &Path) -> Result<(), String> {
    let mut states = Vec::new();
    for key in STATE_KEYS {
        let file = dir.join(format!("{key}.png"));
        let frames = load_sheet(&file).and_then(|s| cut(&s)).map_err(|e| format!("{key}: {e}"))?;
        states.push(frames);
    }
    let all = || states.iter().flatten();
    let left = all().map(|f| f.axis).fold(0.0, f32::max);
    let right = all().map(|f| f.image.width() as f32 - f.axis).fold(0.0, f32::max);
    let half = left.max(right).ceil() as u32 + MARGIN;
    let tall = all().map(|f| f.bottom + 1).max().unwrap_or(1) + 2 * MARGIN;
    let (width, height) = (half * 2, tall);
    let scale = (MAX_FRAME_SIDE as f32 / width.max(height) as f32).min(1.0);
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    for (frames, key) in states.iter().zip(STATE_KEYS) {
        let mut drawn = Vec::with_capacity(frames.len());
        for f in frames {
            let mut canvas = RgbaImage::new(width, height);
            let x = half as i64 - f.axis.round() as i64;
            let y = (height - MARGIN) as i64 - (f.bottom + 1) as i64;
            image::imageops::replace(&mut canvas, &f.image, x, y);
            let sized = if scale < 1.0 {
                image::imageops::resize(&canvas, (width as f32 * scale) as u32, (height as f32 * scale) as u32, image::imageops::FilterType::Lanczos3)
            } else {
                canvas
            };
            // Generated images come out soft at this size. This firms up the outlines without touching flat color.
            drawn.push(image::imageops::unsharpen(&sized, SHARPEN_SIGMA, SHARPEN_THRESHOLD));
        }
        write_animation(&drawn, &out.join(format!("{key}.png"))).map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}

/// Takes the sheets out of a ZIP, wherever in it they are, and a name if there is one. Only those exact files are
/// ever written, under their own fixed names, so nothing in the ZIP chooses where anything goes.
pub fn extract_zip(zip_path: &Path, dir: &Path) -> Result<Option<String>, String> {
    let file = fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| t("File này không phải file zip hợp lệ.", "This isn't a valid ZIP file.").to_string())?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(t("File zip có quá nhiều file.", "The ZIP has too many files.").into());
    }
    let mut name = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let base = entry.name().map_err(|e| e.to_string())?.rsplit(['/', '\\']).next().unwrap_or("").to_lowercase();
        let wanted = STATE_KEYS.iter().find(|k| base == format!("{k}.png"));
        if wanted.is_none() && base != "name.txt" {
            continue;
        }
        // The size the ZIP claims isn't trusted: reading stops just past the limit whatever it says.
        let mut bytes = Vec::new();
        entry.by_ref().take(MAX_SHEET_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SHEET_BYTES {
            return Err(crate::tf!("{base} quá lớn.", "{base} is too large."));
        }
        match wanted {
            Some(key) => fs::write(dir.join(format!("{key}.png")), &bytes).map_err(|e| e.to_string())?,
            None => name = String::from_utf8(bytes).ok().map(|n| n.trim().chars().take(40).collect::<String>()).filter(|n| n.chars().any(char::is_alphanumeric)),
        }
    }
    let missing: Vec<&str> = STATE_KEYS.iter().copied().filter(|k| !dir.join(format!("{k}.png")).is_file()).collect();
    if !missing.is_empty() {
        return Err(crate::tf!("File zip thiếu ảnh: {}.", "The ZIP is missing images: {}.", missing.join(", ")));
    }
    Ok(name)
}

/// `claude-buddy-rs --cut-sheets <folder> <output folder>`: a process of its own, so the memory all those frames
/// take goes back to Windows the moment it's done.
pub fn cut_main(args: &[String]) -> i32 {
    let [dir, out] = args else { return 2 };
    match cut_all(Path::new(dir), Path::new(out)) {
        Ok(()) => 0,
        Err(e) => {
            eprint!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{AnimationDecoder, Rgba};
    use std::path::PathBuf;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Temp {
            let d = std::env::temp_dir().join(format!("buddy-sheet-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Temp(d)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const SOLID_RED: Rgba<u8> = Rgba([200, 30, 30, 255]);

    fn fill(s: &mut RgbaImage, x0: u32, y0: u32, w: u32, h: u32) {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                s.put_pixel(x, y, SOLID_RED);
            }
        }
    }

    /// A little figure: a wide head over narrow feet, with a tail off to one side at hip height.
    fn figure(s: &mut RgbaImage, x: u32, y: u32) {
        fill(s, x + 20, y, 40, 40);
        fill(s, x + 34, y + 40, 12, 40);
        fill(s, x, y + 50, 34, 10);
    }

    /// Four rows of four, drifting further left along each row, the way generated sheets really come out.
    fn drifting_sheet() -> RgbaImage {
        let mut s = RgbaImage::new(800, 800);
        for row in 0..4 {
            for col in 0..4 {
                figure(&mut s, 30 + col * 170 - col * 8, 40 + row * 190);
            }
        }
        s
    }

    fn write_sheets(dir: &Path, s: &RgbaImage) {
        for key in STATE_KEYS {
            s.save(dir.join(format!("{key}.png"))).unwrap();
        }
    }

    #[test]
    fn figures_are_found_where_they_really_are_not_on_a_grid() {
        let found = find_figures(&drifting_sheet());
        assert_eq!(found.len(), 16);
        assert_eq!(found[1].0, 30 + 170 - 8, "the drift is followed");
    }

    #[test]
    fn a_burst_beside_a_figure_stays_with_that_figure() {
        let mut s = drifting_sheet();
        fill(&mut s, 30 + 90, 60, 12, 12);
        assert_eq!(find_figures(&s).len(), 16);
    }

    #[test]
    fn frames_line_up_on_the_character_despite_the_drift() {
        let frames = cut(&drifting_sheet()).unwrap();
        for f in &frames {
            // Same drawing every time, so the axis must land on the same spot of it.
            assert_eq!(f.axis, frames[0].axis);
        }
        assert_eq!(frames[0].axis, 40.0 - 0.5, "the head's center, leaving out the tail");
    }

    #[test]
    fn every_state_comes_out_animated_on_one_canvas_and_timed() {
        let t = Temp::new("all");
        write_sheets(&t.0, &drifting_sheet());
        let out = t.0.join("out");
        cut_all(&t.0, &out).unwrap();
        let mut size = None;
        for key in STATE_KEYS {
            let bytes = fs::read(out.join(format!("{key}.png"))).unwrap();
            assert_eq!(crate::media::sniff(&bytes).0, crate::media::Kind::Animated, "{key}");
            let frames = image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes)).unwrap().apng().unwrap().into_frames().collect_frames().unwrap();
            assert_eq!(frames.len(), 16);
            let (n, d) = frames[0].delay().numer_denom_ms();
            assert_eq!(n / d, FRAME_MS as u32);
            let dims = frames[0].buffer().dimensions();
            assert_eq!(*size.get_or_insert(dims), dims, "{key}");
        }
    }

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            w.start_file(*name, options).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn sheets_come_out_of_a_zip_wherever_they_are() {
        let t = Temp::new("zip");
        let z = t.0.join("c.zip");
        let files: Vec<(String, &[u8])> = STATE_KEYS.iter().map(|k| (format!("character/{k}.PNG"), &b"sheet"[..])).collect();
        let mut list: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        list.push(("character/name.txt", "  Mèo Vũ Trụ\n".as_bytes()));
        list.push(("__MACOSX/notes.exe", b"ignored"));
        make_zip(&z, &list);
        let out = t.0.join("out");
        fs::create_dir_all(&out).unwrap();
        assert_eq!(extract_zip(&z, &out).unwrap().as_deref(), Some("Mèo Vũ Trụ"));
        assert_eq!(fs::read_dir(&out).unwrap().count(), 5, "only the sheets are written");
    }

    #[test]
    fn a_zip_cant_write_outside_or_pass_without_every_state() {
        let t = Temp::new("zipbad");
        let out = t.0.join("deep").join("out");
        fs::create_dir_all(&out).unwrap();
        let z = t.0.join("c.zip");
        make_zip(&z, &[("../../evil.png", b"x"), ("..\\..\\idle.png", b"x")]);
        assert!(extract_zip(&z, &out).is_err(), "missing states");
        assert!(!t.0.join("evil.png").exists() && !t.0.join("deep").join("idle.png").exists());
        assert!(out.join("idle.png").is_file(), "a tricky path lands on the plain name, inside");
        fs::write(&z, b"not a zip").unwrap();
        assert!(extract_zip(&z, &out).is_err());
    }

    #[test]
    fn sharpening_keeps_the_size_and_transparency() {
        let t = Temp::new("sharp");
        write_sheets(&t.0, &drifting_sheet());
        let out = t.0.join("out");
        cut_all(&t.0, &out).unwrap();
        let image = image::open(out.join("idle.png")).unwrap().into_rgba8();
        assert_eq!(image.get_pixel(0, 0)[3], 0, "the background stays clear");
    }

    #[test]
    fn unusable_sheets_are_refused_with_a_reason() {
        let t = Temp::new("bad");
        let opaque = RgbaImage::from_pixel(400, 400, Rgba([255, 255, 255, 255]));
        opaque.save(t.0.join("idle.png")).unwrap();
        assert!(cut_all(&t.0, &t.0.join("out")).unwrap_err().starts_with("idle: "));

        let empty = RgbaImage::new(400, 400);
        assert!(cut(&empty).is_err());
        let mut one = RgbaImage::new(400, 400);
        figure(&mut one, 100, 100);
        assert!(cut(&one).is_err(), "a single picture isn't an animation");

        fs::remove_file(t.0.join("idle.png")).unwrap();
        assert!(cut_all(&t.0, &t.0.join("out")).is_err(), "missing file");
        fs::write(t.0.join("idle.png"), b"not an image").unwrap();
        assert!(cut_all(&t.0, &t.0.join("out")).is_err());
    }
}
