//! Follows the Windows light/dark setting and accent colour, with the Windows 11 colour ramp.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
#[cfg(windows)]
use windows_sys::core::{s, w, PCWSTR};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HWND};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMWA_CAPTION_COLOR, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
#[cfg(windows)]
use windows_sys::Win32::UI::Controls::MARGINS;
#[cfg(windows)]
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

/// Windows' default accent, for when the setting can't be read.
const DEFAULT_ACCENT: u32 = 0x0078D4;

static DARK: AtomicBool = AtomicBool::new(false);
static ACCENT: AtomicU32 = AtomicU32::new(DEFAULT_ACCENT);
/// The user wants see-through surfaces and Windows has them switched on.
static TRANSLUCENT: AtomicBool = AtomicBool::new(true);
static SYSTEM_TRANSPARENCY: AtomicBool = AtomicBool::new(true);

/// What sits behind a window's page background.
#[derive(Clone, Copy, PartialEq, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Backdrop {
    Solid,
    /// Windows 11: the page is left empty and Windows fills it with Mica.
    Mica,
    /// Windows 10: the desktop blurred, under a tint of the page colour.
    Blur,
}

pub fn set_translucent(on: bool) {
    TRANSLUCENT.store(on, Ordering::Relaxed);
}

pub fn translucent() -> bool {
    TRANSLUCENT.load(Ordering::Relaxed) && SYSTEM_TRANSPARENCY.load(Ordering::Relaxed)
}

/// How opaque cards are over a see-through page.
pub const CARD_ALPHA: f32 = 0.72;
/// The buddy's bubble and panel have no real blur behind them (Windows can't blur behind a per-pixel
/// window), so they stay opaque enough for text to read over anything.
pub const FLOATING_ALPHA: f32 = 0.94;
pub const BLUR_TINT_ALPHA: f32 = 0.72;

#[cfg(windows)]
#[repr(C)]
struct AccentPolicy {
    state: i32,
    flags: i32,
    color: u32,
    animation: i32,
}

#[cfg(windows)]
#[repr(C)]
struct CompositionData {
    attribute: i32,
    data: *mut AccentPolicy,
    size: usize,
}

/// Windows 10's blur has no public API; this long-standing one is looked up so that a Windows without
/// it just stays solid.
#[cfg(windows)]
fn set_blur(hwnd: HWND, on: bool) -> bool {
    const WCA_ACCENT_POLICY: i32 = 19;
    const ACCENT_DISABLED: i32 = 0;
    const ACCENT_ENABLE_BLURBEHIND: i32 = 3;
    unsafe {
        let Some(f) = GetProcAddress(GetModuleHandleW(w!("user32.dll")), s!("SetWindowCompositionAttribute")) else { return false };
        let f: unsafe extern "system" fn(HWND, *mut CompositionData) -> i32 = std::mem::transmute(f);
        let mut accent = AccentPolicy { state: if on { ACCENT_ENABLE_BLURBEHIND } else { ACCENT_DISABLED }, flags: 0, color: 0, animation: 0 };
        let mut data = CompositionData { attribute: WCA_ACCENT_POLICY, data: &mut accent, size: std::mem::size_of::<AccentPolicy>() };
        f(hwnd, &mut data) != 0
    }
}

#[derive(Clone, Copy)]
pub struct Pal {
    pub bg: u32,
    pub surface: u32,
    pub hover: u32,
    pub stroke: u32,
    pub ink: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub on_accent: u32,
    pub danger: u32,
    pub warning: u32,
}

pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s & 0xff) as f32, (b >> s & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

fn palette(dark: bool, accent: u32) -> Pal {
    if dark {
        // Accent colours are picked for light backgrounds: the dark theme uses a lighter tone of them.
        let a = mix(accent, 0xffffff, 0.45);
        Pal {
            bg: 0x202020,
            surface: 0x2b2b2b,
            hover: 0x383838,
            stroke: 0x3f3f3f,
            ink: 0xffffff,
            muted: 0xb4b4b4,
            accent: a,
            accent_hover: mix(a, 0xffffff, 0.15),
            on_accent: 0x000000,
            danger: 0xff8a80,
            warning: 0xfce100,
        }
    } else {
        Pal {
            bg: 0xf3f3f3,
            surface: 0xfbfbfb,
            hover: 0xeaeaea,
            stroke: 0xe0e0e0,
            ink: 0x1b1b1b,
            muted: 0x5d5d5d,
            accent,
            accent_hover: mix(accent, 0x000000, 0.12),
            on_accent: 0xffffff,
            danger: 0xc42b1c,
            warning: 0x9d5d00,
        }
    }
}

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

/// Status colours were picked for light backgrounds: the dark theme lifts them to stay visible.
pub fn on_theme(color: u32) -> u32 {
    if is_dark() {
        mix(color, 0xffffff, 0.3)
    } else {
        color
    }
}

pub fn pal() -> Pal {
    palette(DARK.load(Ordering::Relaxed), ACCENT.load(Ordering::Relaxed))
}

/// The registry stores it as 0xAABBGGRR.
#[cfg(any(windows, test))]
fn accent_from_registry(abgr: u32) -> u32 {
    (abgr & 0xff) << 16 | (abgr & 0xff00) | (abgr >> 16 & 0xff)
}

#[cfg(windows)]
fn read_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let (mut data, mut size) = (0u32, 4u32);
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key, value, RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut _, &mut size) };
    (r == ERROR_SUCCESS).then_some(data)
}

/// Reads the current Windows settings; call at start and whenever they change.
#[cfg(windows)]
pub fn reload() {
    let light = read_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme"));
    // CLAUDE_BUDDY_RS_THEME forces a theme for checking both without changing the Windows setting.
    let forced = std::env::var("CLAUDE_BUDDY_RS_THEME").ok();
    DARK.store(forced.as_deref().map_or(light == Some(0), |t| t == "dark"), Ordering::Relaxed);
    let transparency = read_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("EnableTransparency"));
    SYSTEM_TRANSPARENCY.store(transparency != Some(0), Ordering::Relaxed);
    let accent = read_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor")).map_or(DEFAULT_ACCENT, accent_from_registry);
    ACCENT.store(accent, Ordering::Relaxed);
}

/// Sets up the title bar and what is behind the page, and says what it got. Windows 10 ignores the
/// attributes it doesn't know, which is what tells the two apart.
#[cfg(windows)]
pub fn apply_backdrop(hwnd: HWND) -> Backdrop {
    let p = pal();
    let dark: i32 = DARK.load(Ordering::Relaxed) as i32;
    let forced = std::env::var("CLAUDE_BUDDY_RS_BACKDROP").ok();
    let want = translucent();
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &dark as *const i32 as *const _, 4);
        let mica_type: i32 = if want && forced.as_deref() != Some("blur") { DWMSBT_MAINWINDOW } else { DWMSBT_NONE };
        let mica_ok = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE as u32, &mica_type as *const i32 as *const _, 4) == 0;
        let got = if want && mica_ok && mica_type == DWMSBT_MAINWINDOW {
            Backdrop::Mica
        } else if want && set_blur(hwnd, true) {
            Backdrop::Blur
        } else {
            set_blur(hwnd, false);
            Backdrop::Solid
        };
        let edge = if got == Backdrop::Solid { 0 } else { -1 };
        let margins = MARGINS { cxLeftWidth: edge, cxRightWidth: edge, cyTopHeight: edge, cyBottomHeight: edge };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
        // COLORREF is 0x00BBGGRR. Default (0xFFFFFFFF) lets the backdrop show through the title bar.
        let caption = if got == Backdrop::Solid { (p.bg & 0xff) << 16 | (p.bg & 0xff00) | (p.bg >> 16 & 0xff) } else { 0xFFFF_FFFF };
        DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as u32, &caption as *const u32 as *const _, 4);
        got
    }
}

/// Light or dark from the system; pages are solid here.
#[cfg(not(windows))]
pub fn reload() {
    #[cfg(target_os = "macos")]
    let system_dark = objc2_foundation::NSUserDefaults::standardUserDefaults().stringForKey(&objc2_foundation::NSString::from_str("AppleInterfaceStyle")).is_some_and(|s| s.to_string() == "Dark");
    #[cfg(not(target_os = "macos"))]
    let system_dark = false;
    let forced = std::env::var("CLAUDE_BUDDY_RS_THEME").ok();
    DARK.store(forced.as_deref().map_or(system_dark, |t| t == "dark"), Ordering::Relaxed);
    SYSTEM_TRANSPARENCY.store(false, Ordering::Relaxed);
}

#[cfg(not(windows))]
pub fn apply_backdrop<T>(_window: T) -> Backdrop {
    Backdrop::Solid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: u32) -> f32 {
        let lin = |s: u32| {
            let v = (c >> s & 0xff) as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(16) + 0.7152 * lin(8) + 0.0722 * lin(0)
    }

    fn contrast(a: u32, b: u32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn registry_colours_swap_to_rgb() {
        assert_eq!(accent_from_registry(0xff1058bb), 0xbb5810);
    }

    #[test]
    fn text_is_readable_in_both_themes_with_any_common_accent() {
        for accent in [0x0078d4, 0xbb5810, 0x107c10, 0xe81123, 0x744da9, 0x515c6b] {
            for dark in [false, true] {
                let p = palette(dark, accent);
                for surface in [p.bg, p.surface, p.hover] {
                    assert!(contrast(p.ink, surface) >= 7.0, "ink {dark}");
                    assert!(contrast(p.muted, surface) >= 4.5, "muted {dark} {surface:06x}");
                }
                assert!(contrast(p.danger, p.surface) >= 4.5, "danger {dark}");
                assert!(contrast(p.on_accent, p.accent) >= 3.0, "accent {accent:06x} {dark}");
            }
        }
    }
}
