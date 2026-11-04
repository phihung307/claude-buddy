//! MP4 through Windows' own Media Foundation decoders, so no H.264 code ships with the app.
//! MP4 has no transparency, so frames come out fully opaque.

use crate::i18n::t;
use crate::media::{frame_limit, Frames};
use std::path::Path;
use windows::core::{GUID, HSTRING};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// MF_SOURCE_READER_FIRST_VIDEO_STREAM, as the DWORD the reader methods take.
const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
/// Longest side kept while decoding: the buddy never shows a frame anywhere near this big, and big
/// clips would otherwise hold gigabytes of raw frames.
const MAX_KEPT_SIDE: u32 = 720;

fn mf_err(e: windows::core::Error) -> String {
    crate::tf!("Windows không giải mã được video này: {}", "Windows could not decode this video: {}", e.message())
}

/// Nearest-neighbour shrink of a raw frame; the real resampling happens later on the smaller result.
fn shrink(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32, stride: i32) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        let sy = (y as u64 * sh as u64 / dh as u64) as i64;
        // A negative stride means the picture is stored bottom-up.
        let row = if stride < 0 { (sh as i64 - 1 - sy) * (-stride) as i64 } else { sy * stride as i64 } as usize;
        for x in 0..dw {
            let sx = (x as u64 * sw as u64 / dw as u64) as usize;
            let (si, di) = (row + sx * 4, ((y * dw + x) * 4) as usize);
            if si + 3 < src.len() {
                out[di..di + 3].copy_from_slice(&src[si..si + 3]);
            }
            out[di + 3] = 255;
        }
    }
    out
}

pub fn decode_mp4(path: &Path, first_only: bool, min_frame_ms: u32) -> Result<Frames, String> {
    unsafe {
        // Already initialised on this thread is fine; only the reader calls below matter.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(mf_err)?;
        let result = read_all(path, first_only, min_frame_ms);
        let _ = MFShutdown();
        result
    }
}

unsafe fn read_all(path: &Path, first_only: bool, min_frame_ms: u32) -> Result<Frames, String> {
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 1).map_err(mf_err)?;
    let attrs = attrs.ok_or(t("không tạo được thuộc tính Media Foundation", "could not create Media Foundation attributes"))?;
    // Lets the reader insert the converter from the decoder's YUV to RGB32.
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(mf_err)?;
    let url = HSTRING::from(path.as_os_str());
    let reader = MFCreateSourceReaderFromURL(&url, &attrs).map_err(mf_err)?;

    let wanted = MFCreateMediaType().map_err(mf_err)?;
    wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(mf_err)?;
    wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32 as *const GUID).map_err(mf_err)?;
    reader.SetCurrentMediaType(FIRST_VIDEO, None, &wanted).map_err(mf_err)?;

    let current = reader.GetCurrentMediaType(FIRST_VIDEO).map_err(mf_err)?;
    let packed = current.GetUINT64(&MF_MT_FRAME_SIZE).map_err(mf_err)?;
    let (sw, sh) = ((packed >> 32) as u32, packed as u32);
    if sw == 0 || sh == 0 {
        return Err(t("video không có kích thước hợp lệ", "the video has no valid size").into());
    }
    // RGB32 from Media Foundation is bottom-up unless the type says otherwise.
    let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(-((sw * 4) as i32));
    let k = (MAX_KEPT_SIDE as f32 / sw.max(sh) as f32).min(1.0);
    let (dw, dh) = (((sw as f32 * k) as u32).max(1), ((sh as f32 * k) as u32).max(1));

    let max_frames = frame_limit(dw, dh);
    let mut out: Vec<(Vec<u8>, u32)> = Vec::new();
    let mut last_ms: Option<i64> = None;
    loop {
        let mut flags = 0u32;
        let mut stamp = 0i64;
        let mut sample: Option<IMFSample> = None;
        reader
            .ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut stamp), Some(&mut sample))
            .map_err(mf_err)?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(sample) = sample else { continue };
        let now_ms = stamp / 10_000;
        // Too soon after the last kept frame: skip the (expensive) copy altogether.
        if let Some(last) = last_ms {
            if now_ms - last < min_frame_ms as i64 {
                continue;
            }
            if let Some(prev) = out.last_mut() {
                prev.1 = (now_ms - last).max(1) as u32;
            }
        }
        last_ms = Some(now_ms);
        let buffer = sample.ConvertToContiguousBuffer().map_err(mf_err)?;
        let mut data = std::ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut data, None, Some(&mut len)).map_err(mf_err)?;
        let frame = shrink(std::slice::from_raw_parts(data, len as usize), sw, sh, dw, dh, stride);
        let _ = buffer.Unlock();
        out.push((frame, 33));
        if first_only || out.len() >= max_frames {
            break;
        }
    }
    if out.is_empty() {
        return Err(t("video không có khung hình nào", "the video has no frames").into());
    }
    if out.len() > 1 {
        let n = out.len();
        out[n - 1].1 = out[n - 2].1;
    }
    Ok(Frames { w: dw, h: dh, frames: out })
}
