//! English and Vietnamese. Each piece of text keeps both versions side by side where it is used, so
//! nothing can be added in one language and forgotten in the other.

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;

const LANG_VIETNAMESE: u16 = 0x2A;

static VIETNAMESE: AtomicBool = AtomicBool::new(true);

pub fn vi() -> bool {
    VIETNAMESE.load(Ordering::Relaxed)
}

/// `choice`: "vi", "en", or anything else to follow Windows.
pub fn apply(choice: &str) {
    VIETNAMESE.store(resolve(choice, system_language()), Ordering::Relaxed);
}

#[cfg(windows)]
fn system_language() -> u16 {
    unsafe { GetUserDefaultUILanguage() }
}

#[cfg(not(windows))]
fn system_language() -> u16 {
    if std::env::var("LANG").is_ok_and(|l| l.starts_with("vi")) { LANG_VIETNAMESE } else { 0 }
}

fn resolve(choice: &str, windows_language: u16) -> bool {
    match choice {
        "vi" => true,
        "en" => false,
        _ => windows_language & 0x3FF == LANG_VIETNAMESE,
    }
}

pub fn t(vi_text: &'static str, en_text: &'static str) -> &'static str {
    if vi() {
        vi_text
    } else {
        en_text
    }
}

/// `format!` in the current language.
#[macro_export]
macro_rules! tf {
    ($vi:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if $crate::i18n::vi() { format!($vi $(, $a)*) } else { format!($en $(, $a)*) }
    };
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A yyyymmdd day: 4/11/2026, or Nov 4, 2026.
pub fn date(day: u32) -> String {
    let (y, m, d) = (day / 10_000, day / 100 % 100, day % 100);
    if vi() {
        format!("{d}/{m}/{y}")
    } else {
        format!("{} {d}, {y}", MONTHS[(m.clamp(1, 12) - 1) as usize])
    }
}

/// The same without the year, for chart labels.
pub fn short_date(day: u32) -> String {
    let (m, d) = (day / 100 % 100, day % 100);
    if vi() {
        format!("{d}/{m}")
    } else {
        format!("{m}/{d}")
    }
}

/// Groups thousands the way the language writes them: 1.234.567 or 1,234,567.
pub fn grouped(n: u64) -> String {
    group_with(n, if vi() { '.' } else { ',' })
}

fn group_with(n: u64, sep: char) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_windows_only_when_asked_to() {
        assert!(resolve("vi", 0x0409));
        assert!(!resolve("en", 0x042A));
        assert!(resolve("auto", 0x042A), "Vietnamese Windows");
        assert!(!resolve("auto", 0x0409), "English Windows");
        assert!(!resolve("", 0x0809), "other languages get English");
    }

    #[test]
    fn numbers_group_by_language() {
        assert_eq!(group_with(0, '.'), "0");
        assert_eq!(group_with(999, '.'), "999");
        assert_eq!(group_with(1_000, '.'), "1.000");
        assert_eq!(group_with(1_234_567, ','), "1,234,567");
    }
}
