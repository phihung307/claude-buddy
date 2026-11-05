//! Small synthesized chimes, built once and handed to the system player, so no audio files ship.

use crate::store::State;
use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::path::Path;
#[cfg(windows)]
use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_MEMORY, SND_NODEFAULT};
#[cfg(windows)]
use windows_sys::Win32::Media::Multimedia::mciSendStringW;

const RATE: u32 = 22_050;
/// (frequency Hz, start s, duration s)
type Note = (f32, f32, f32);

fn wav(notes: &[Note]) -> Vec<u8> {
    let total = notes.iter().map(|n| n.1 + n.2).fold(0.0, f32::max);
    let count = (total * RATE as f32) as usize;
    let mut mix = vec![0f32; count];
    for &(freq, at, dur) in notes {
        let start = (at * RATE as f32) as usize;
        let len = (dur * RATE as f32) as usize;
        for i in 0..len.min(count.saturating_sub(start)) {
            let t = i as f32 / RATE as f32;
            let env = (t / 0.015).min(1.0) * (1.0 - t / dur).powi(2);
            mix[start + i] += (TAU * freq * t).sin() * env * 0.22;
        }
    }
    let data_len = (count * 2) as u32;
    let mut out = Vec::with_capacity(44 + count * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in mix {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

#[cfg(windows)]
fn mci(command: &str) {
    let c: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { mciSendStringW(c.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut()) };
}

/// A character's own sound. Falls back on silence rather than an error: it is only a sound.
#[cfg(target_os = "macos")]
pub fn play_file(path: &Path) {
    let path = objc2_foundation::NSString::from_str(&path.display().to_string());
    let sound = objc2_app_kit::NSSound::initWithContentsOfFile_byReference(<objc2_app_kit::NSSound as objc2::AllocAnyThread>::alloc(), &path, true);
    if let Some(sound) = sound {
        start(sound);
    }
}

/// Kept until the next one, so it isn't released while it plays.
#[cfg(target_os = "macos")]
fn start(sound: objc2::rc::Retained<objc2_app_kit::NSSound>) {
    thread_local! {
        static PLAYING: std::cell::RefCell<Option<objc2::rc::Retained<objc2_app_kit::NSSound>>> = const { std::cell::RefCell::new(None) };
    }
    sound.play();
    PLAYING.with(|p| *p.borrow_mut() = Some(sound));
}

#[cfg(windows)]
pub fn play_file(path: &Path) {
    let is_mp3 = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp3"));
    let shown = path.display().to_string();
    if is_mp3 {
        // MCI commands are text: a quote in the path would end the file name.
        if !shown.contains('"') {
            mci("close buddysound");
            mci(&format!("open \"{shown}\" type mpegvideo alias buddysound"));
            mci("play buddysound");
        }
        return;
    }
    let w: Vec<u16> = shown.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { PlaySoundW(w.as_ptr(), std::ptr::null_mut(), SND_FILENAME | SND_ASYNC | SND_NODEFAULT) };
}

pub fn play(state: State) {
    static DONE: OnceLock<Vec<u8>> = OnceLock::new();
    static WAITING: OnceLock<Vec<u8>> = OnceLock::new();
    static ERROR: OnceLock<Vec<u8>> = OnceLock::new();
    let buf = match state {
        State::Done => DONE.get_or_init(|| wav(&[(659.0, 0.0, 0.18), (880.0, 0.12, 0.3)])),
        State::Waiting => WAITING.get_or_init(|| wav(&[(880.0, 0.0, 0.14), (880.0, 0.18, 0.2)])),
        State::Error => ERROR.get_or_init(|| wav(&[(440.0, 0.0, 0.2), (330.0, 0.16, 0.32)])),
        _ => return,
    };
    #[cfg(target_os = "macos")]
    {
        let data = objc2_foundation::NSData::with_bytes(buf);
        if let Some(sound) = objc2_app_kit::NSSound::initWithData(<objc2_app_kit::NSSound as objc2::AllocAnyThread>::alloc(), &data) {
            start(sound);
        }
    }
    // The buffers are static, so they outlive the asynchronous playback SND_MEMORY reads from.
    #[cfg(windows)]
    unsafe { PlaySoundW(buf.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_well_formed_wav() {
        let w = wav(&[(440.0, 0.0, 0.1), (660.0, 0.05, 0.1)]);
        assert_eq!(&w[..4], b"RIFF");
        let data_len = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
        assert_eq!(w.len(), 44 + data_len);
        assert!(data_len > 0);
    }
}
