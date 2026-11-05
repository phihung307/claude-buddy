//! Text on macOS: the system's own fonts and fallback, with no font bundled. Drawn white on black into
//! a scratch bitmap that becomes the coverage mask, the way it is done on Windows.

use crate::render::{blend_mask, Font};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName, NSGraphicsContext, NSLineBreakMode, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSStringDrawingOptions, NSAttributedStringNSExtendedStringDrawing, NSAttributedStringNSStringDrawing,
};
use objc2_core_foundation::CGFloat;
use objc2_core_graphics::{CGBitmapContextCreate, CGBitmapInfo, CGColorSpace, CGImageAlphaInfo, CGImageByteOrderInfo};
use objc2_foundation::{NSAttributedString, NSAttributedStringKey, NSDictionary, NSPoint, NSRect, NSSize, NSString};

pub struct TextDrawer {
    fonts: [Retained<NSFont>; 7],
}

/// The Windows icon font's code points, as symbols every Mac has.
fn icon_text(s: &str) -> String {
    s.chars()
        .map(|c| match c as u32 {
            0xE77B => "☺",
            0xE9D2 => "▤",
            0xE710 => "+",
            0xE945 => "✦",
            0xE70F => "✎",
            0xE838 => "⌂",
            0xE74D => "✕",
            0xE72D => "↗",
            0xE711 => "✕",
            0xE73E => "✓",
            0xE713 => "⚙",
            0xE703 => "⚡",
            0xE767 => "♪",
            0xEA8F => "◉",
            0xF2B7 => "A",
            0xE7E8 => "⏻",
            0xE771 => "◐",
            0xED1A => "◌",
            0xE740 => "⤢",
            0xE707 => "✥",
            0xE916 => "◔",
            0xE7BA => "!",
            0xEA39 => "✕",
            0xE73F => "…",
            _ => "",
        })
        .collect::<String>()
}

impl TextDrawer {
    /// For the buddy's window: compact sizes.
    pub fn new(scale: f32) -> TextDrawer {
        Self::make(scale, [12.5, 12.5, 11.0, 19.0, 11.0, 28.0, 13.0])
    }

    /// For the main window.
    pub fn new_ui(scale: f32) -> TextDrawer {
        Self::make(scale, [14.0, 14.0, 12.0, 20.0, 12.0, 28.0, 16.0])
    }

    fn make(scale: f32, px: [f32; 7]) -> TextDrawer {
        let size = |i: usize| (px[i] * scale).round() as CGFloat;
        let (regular, semibold) = unsafe { (NSFontWeightRegular, NSFontWeightSemibold) };
        let fonts = [
            NSFont::systemFontOfSize_weight(size(0), regular),
            NSFont::systemFontOfSize_weight(size(1), semibold),
            NSFont::systemFontOfSize_weight(size(2), regular),
            NSFont::systemFontOfSize_weight(size(3), semibold),
            NSFont::monospacedSystemFontOfSize_weight(size(4), regular),
            NSFont::systemFontOfSize_weight(size(5), semibold),
            NSFont::systemFontOfSize_weight(size(6), regular),
        ];
        TextDrawer { fonts }
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

    fn attributed(&self, s: &str, f: Font) -> Retained<NSAttributedString> {
        let shown = if matches!(f, Font::Icon) { icon_text(s) } else { s.replace(['\r', '\n'], " ") };
        let style = NSMutableParagraphStyle::new();
        style.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let white = NSColor::whiteColor();
        let font = &self.fonts[Self::index(f)];
        let values: [&AnyObject; 3] = [font, &white, &style];
        let keys: [&NSAttributedStringKey; 3] = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName] };
        let attrs = NSDictionary::from_slices(&keys, &values);
        unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(&shown), &attrs) }
    }

    pub fn width(&self, s: &str, f: Font) -> i32 {
        self.attributed(s, f).size().width.ceil() as i32
    }

    /// Single line, ending in "…" when it doesn't fit in `bw`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, buf: &mut [u8], w: u32, h: u32, x: i32, y: i32, bw: i32, bh: i32, s: &str, f: Font, color: u32) {
        if bw <= 0 || bh <= 0 || s.is_empty() {
            return;
        }
        let text = self.attributed(s, f);
        let mut mask = vec![0u8; (bw * bh * 4) as usize];
        mask.chunks_exact_mut(4).for_each(|p| p[3] = 255);
        let Some(space) = CGColorSpace::new_device_rgb() else { return };
        let info = CGBitmapInfo(CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0);
        let context = unsafe { CGBitmapContextCreate(mask.as_mut_ptr() as *mut _, bw as usize, bh as usize, 8, bw as usize * 4, Some(&space), info.0) };
        let Some(context) = context else { return };
        let graphics = NSGraphicsContext::graphicsContextWithCGContext_flipped(&context, false);
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&graphics));
        let height = text.size().height;
        let rect = NSRect::new(NSPoint::new(0.0, ((bh as CGFloat - height) / 2.0).floor()), NSSize::new(bw as CGFloat, height));
        let options = NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::TruncatesLastVisibleLine;
        text.drawWithRect_options_context(rect, options, None);
        graphics.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        drop(graphics);
        drop(context);
        blend_mask(buf, w, h, x, y, bw, bh, &mask, color, false);
    }
}
