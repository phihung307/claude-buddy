//! Text on macOS: the system's own fonts and fallback, with no font bundled. Drawn white on black into
//! a scratch bitmap that becomes the coverage mask, the way it is done on Windows.

use crate::render::{blend_mask, Font};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName, NSGraphicsContext, NSImage, NSLineBreakMode, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSStringDrawingOptions, NSAttributedStringNSExtendedStringDrawing, NSAttributedStringNSStringDrawing,
};
use objc2_core_foundation::CGFloat;
use objc2_core_graphics::{CGBitmapContextCreate, CGBitmapInfo, CGColorSpace, CGImageAlphaInfo, CGImageByteOrderInfo};
use objc2_foundation::{NSAttributedString, NSAttributedStringKey, NSDictionary, NSPoint, NSRect, NSSize, NSString};

pub struct TextDrawer {
    fonts: [Retained<NSFont>; 7],
}

/// The Windows icon font's code points, as the system's symbols on a Mac.
fn symbol_name(s: &str) -> Option<&'static str> {
    Some(match s.chars().next()? as u32 {
        0xE77B => "person",
        0xE9D2 => "chart.xyaxis.line",
        0xE710 => "plus",
        0xE945 => "bolt",
        0xE70F => "pencil",
        0xE838 => "folder",
        0xE74D => "trash",
        0xE72D => "square.and.arrow.up",
        0xE711 => "xmark",
        0xE73E => "checkmark",
        0xE713 => "gearshape",
        0xE703 => "powerplug",
        0xE767 => "speaker.wave.2",
        0xEA8F => "bell",
        0xF2B7 => "globe",
        0xE7E8 => "power",
        0xE771 => "circle.lefthalf.filled",
        0xED1A => "eye.slash",
        0xE740 => "arrow.up.left.and.arrow.down.right",
        0xE707 => "arrow.up.and.down.and.arrow.left.and.right",
        0xE916 => "clock",
        0xE7BA => "exclamationmark",
        0xEA39 => "xmark",
        0xE73F => "ellipsis",
        _ => return None,
    })
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
        let shown = s.replace(['\r', '\n'], " ");
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
        let symbol = if matches!(f, Font::Icon) { symbol_name(s).and_then(|n| NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(n), None)) } else { None };
        if matches!(f, Font::Icon) && symbol.is_none() {
            return;
        }
        let text = self.attributed(s, f);
        let mut mask = vec![0u8; (bw * bh * 4) as usize];
        // Symbols draw black, so theirs goes on white and is read the other way round.
        let (paper, dark_mask) = if symbol.is_some() { (255, true) } else { (0, false) };
        mask.chunks_exact_mut(4).for_each(|p| {
            p[..3].fill(paper);
            p[3] = 255;
        });
        let Some(space) = CGColorSpace::new_device_rgb() else { return };
        let info = CGBitmapInfo(CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0);
        let context = unsafe { CGBitmapContextCreate(mask.as_mut_ptr() as *mut _, bw as usize, bh as usize, 8, bw as usize * 4, Some(&space), info.0) };
        let Some(context) = context else { return };
        let graphics = NSGraphicsContext::graphicsContextWithCGContext_flipped(&context, false);
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&graphics));
        if let Some(image) = &symbol {
            let side = self.fonts[Self::index(f)].pointSize().min(bw as CGFloat).min(bh as CGFloat);
            let natural = image.size();
            let k = side / natural.width.max(natural.height).max(1.0);
            let size = NSSize::new(natural.width * k, natural.height * k);
            // Left aligned and centred in height, the way a glyph of the icon font lands.
            let at = NSPoint::new(0.0, ((bh as CGFloat - size.height) / 2.0).floor());
            image.drawInRect(NSRect::new(at, size));
        } else {
            let height = text.size().height;
            let rect = NSRect::new(NSPoint::new(0.0, ((bh as CGFloat - height) / 2.0).floor()), NSSize::new(bw as CGFloat, height));
            let options = NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::TruncatesLastVisibleLine;
            text.drawWithRect_options_context(rect, options, None);
        }
        graphics.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        drop(graphics);
        drop(context);
        blend_mask(buf, w, h, x, y, bw, bh, &mask, color, dark_mask);
    }
}
