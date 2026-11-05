//! MP4 through macOS's own decoders, so no H.264 code ships with the app.
//! MP4 has no transparency, so frames come out fully opaque.

use crate::i18n::t;
use crate::media::{frame_limit, Frames};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVMediaTypeVideo, AVURLAsset};
#[allow(deprecated)]
use objc2_av_foundation::AVAsset;
use objc2_core_media::CMTime;
use objc2_core_video::{kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::path::Path;

/// Longest side kept while decoding: the buddy never shows a frame anywhere near this big, and big
/// clips would otherwise hold gigabytes of raw frames.
const MAX_KEPT_SIDE: u32 = 720;

fn failed() -> String {
    t("macOS không giải mã được video này", "macOS could not decode this video").into()
}

/// Nearest-neighbour shrink of a raw frame; the real resampling happens later on the smaller result.
fn shrink(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32, stride: usize) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh as usize {
        let sy = y * sh as usize / dh as usize;
        for x in 0..dw as usize {
            let sx = x * sw as usize / dw as usize;
            let s = sy * stride + sx * 4;
            let d = (y * dw as usize + x) * 4;
            if let Some(p) = src.get(s..s + 4) {
                out[d..d + 3].copy_from_slice(&p[..3]);
                out[d + 3] = 255;
            }
        }
    }
    out
}

fn ms_of(time: CMTime) -> i64 {
    if time.timescale > 0 {
        time.value * 1000 / time.timescale as i64
    } else {
        0
    }
}

#[allow(deprecated)]
pub fn decode_mp4(path: &Path, first_only: bool, min_frame_ms: u32) -> Result<Frames, String> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.display().to_string()));
        let asset: Retained<AVURLAsset> = AVURLAsset::URLAssetWithURL_options(&url, None);
        let asset_ref: &AVAsset = &asset;
        let kind = AVMediaTypeVideo.ok_or_else(failed)?;
        let tracks = asset_ref.tracksWithMediaType(kind);
        let track = tracks.firstObject().ok_or_else(failed)?;
        let reader = AVAssetReader::assetReaderWithAsset_error(asset_ref).map_err(|_| failed())?;
        let key: &NSString = &*(kCVPixelBufferPixelFormatTypeKey as *const objc2_core_foundation::CFString as *const NSString);
        let value = NSNumber::numberWithUnsignedInt(kCVPixelFormatType_32BGRA);
        let settings = NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[&*value as &AnyObject]);
        let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&track, Some(&settings));
        if !reader.canAddOutput(&output) {
            return Err(failed());
        }
        reader.addOutput(&output);
        if !reader.startReading() {
            return Err(failed());
        }

        let mut out: Vec<(Vec<u8>, u32)> = Vec::new();
        let (mut dw, mut dh) = (0, 0);
        let mut max_frames = 1;
        let mut last_ms: Option<i64> = None;
        while let Some(sample) = output.copyNextSampleBuffer() {
            let now_ms = ms_of(sample.presentation_time_stamp());
            // Too soon after the last kept frame: skip the (expensive) copy altogether.
            if let Some(last) = last_ms {
                if now_ms - last < min_frame_ms as i64 {
                    continue;
                }
                if let Some(prev) = out.last_mut() {
                    prev.1 = (now_ms - last).max(1) as u32;
                }
            }
            let Some(image) = sample.image_buffer() else { continue };
            last_ms = Some(now_ms);
            if CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) != 0 {
                continue;
            }
            let (sw, sh, stride) = (CVPixelBufferGetWidth(&image) as u32, CVPixelBufferGetHeight(&image) as u32, CVPixelBufferGetBytesPerRow(&image));
            let base = CVPixelBufferGetBaseAddress(&image) as *const u8;
            if dw == 0 && sw > 0 && sh > 0 {
                let k = (MAX_KEPT_SIDE as f32 / sw.max(sh) as f32).min(1.0);
                (dw, dh) = (((sw as f32 * k) as u32).max(1), ((sh as f32 * k) as u32).max(1));
                max_frames = frame_limit(dw, dh);
            }
            let frame = (!base.is_null() && dw > 0).then(|| shrink(std::slice::from_raw_parts(base, stride * sh as usize), sw, sh, dw, dh, stride));
            CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);
            if let Some(frame) = frame {
                out.push((frame, 33));
            }
            if first_only || out.len() >= max_frames {
                reader.cancelReading();
                break;
            }
        }
        if out.is_empty() {
            let message = if reader.status() == AVAssetReaderStatus::Failed { failed() } else { t("video không có khung hình nào", "the video has no frames").into() };
            return Err(message);
        }
        if out.len() > 1 {
            let n = out.len();
            out[n - 1].1 = out[n - 2].1;
        }
        Ok(Frames { w: dw, h: dh, frames: out })
    })
}
