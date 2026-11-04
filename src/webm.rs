//! Just enough Matroska to get VP9 packets out of a WebM file. Transparent WebM stores alpha as a
//! second VP9 stream in each block's BlockAdditions, which ordinary system decoders ignore.

use crate::i18n::t;

const ID_SEGMENT: u32 = 0x1853_8067;
const ID_INFO: u32 = 0x1549_A966;
const ID_TIMESCALE: u32 = 0x2A_D7B1;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC: u32 = 0x86;
const ID_VIDEO: u32 = 0xE0;
const ID_ALPHA_MODE: u32 = 0x53C0;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_CLUSTER_TIME: u32 = 0xE7;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
const ID_BLOCK_ADDITIONS: u32 = 0x75A1;
const ID_BLOCK_MORE: u32 = 0xA6;
const ID_BLOCK_ADD_ID: u32 = 0xEE;
const ID_BLOCK_ADDITIONAL: u32 = 0xA5;
const TRACK_TYPE_VIDEO: u64 = 1;
/// Nesting in a valid file is far shallower; this only stops hostile files from exhausting the stack.
const MAX_DEPTH: u32 = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub time_ms: u64,
    pub color: Vec<u8>,
    pub alpha: Option<Vec<u8>>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Webm {
    pub codec: String,
    pub has_alpha_track: bool,
    pub packets: Vec<Packet>,
}

#[derive(Default)]
struct Parser {
    timescale_ns: u64,
    track: Option<u64>,
    cluster_ms: u64,
    out: Webm,
    // Current track entry while walking Tracks
    entry_number: u64,
    entry_type: u64,
    entry_codec: String,
    entry_alpha: bool,
    // Current block group
    pending: Option<Packet>,
    add_id: u64,
    add_data: Option<Vec<u8>>,
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0, |a, x| (a << 8) | *x as u64)
}

/// (value, length). The element id keeps its marker bits, sizes drop them.
fn vint(d: &[u8], p: usize, keep_marker: bool) -> Option<(u64, usize)> {
    let first = *d.get(p)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || p + len > d.len() {
        return None;
    }
    let mut v = if keep_marker { first as u64 } else { first as u64 & (0xFF >> len) };
    for b in &d[p + 1..p + len] {
        v = (v << 8) | *b as u64;
    }
    Some((v, len))
}

impl Parser {
    fn walk(&mut self, d: &[u8], depth: u32) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(t("WebM lồng quá sâu", "WebM is nested too deeply").into());
        }
        let mut p = 0;
        while p < d.len() {
            // Trailing garbage after the last complete element is not worth rejecting the file for.
            let Some((id, n)) = vint(d, p, true) else { break };
            p += n;
            let Some((size, n)) = vint(d, p, false) else { break };
            let unknown = size == (1u64 << (7 * n)) - 1;
            p += n;
            let end = if unknown { d.len() } else { p.saturating_add(size as usize).min(d.len()) };
            self.element(id as u32, &d[p..end], depth)?;
            p = end;
        }
        Ok(())
    }

    fn element(&mut self, id: u32, v: &[u8], depth: u32) -> Result<(), String> {
        match id {
            ID_SEGMENT | ID_INFO | ID_TRACKS | ID_VIDEO | ID_BLOCK_ADDITIONS => self.walk(v, depth + 1)?,
            ID_CLUSTER => {
                self.cluster_ms = 0;
                self.walk(v, depth + 1)?;
            }
            ID_TIMESCALE => self.timescale_ns = uint(v),
            ID_TRACK_ENTRY => {
                (self.entry_number, self.entry_type, self.entry_alpha) = (0, 0, false);
                self.entry_codec.clear();
                self.walk(v, depth + 1)?;
                if self.track.is_none() && self.entry_type == TRACK_TYPE_VIDEO {
                    self.track = Some(self.entry_number);
                    self.out.codec = self.entry_codec.clone();
                    self.out.has_alpha_track = self.entry_alpha;
                }
            }
            ID_TRACK_NUMBER => self.entry_number = uint(v),
            ID_TRACK_TYPE => self.entry_type = uint(v),
            ID_CODEC => self.entry_codec = String::from_utf8_lossy(v).into_owned(),
            ID_ALPHA_MODE => self.entry_alpha = uint(v) != 0,
            ID_CLUSTER_TIME => self.cluster_ms = uint(v).saturating_mul(self.timescale_ns.max(1)) / 1_000_000,
            ID_SIMPLE_BLOCK => {
                if let Some(pkt) = self.block(v)? {
                    self.out.packets.push(pkt);
                }
            }
            ID_BLOCK_GROUP => {
                self.pending = None;
                self.walk(v, depth + 1)?;
                if let Some(pkt) = self.pending.take() {
                    self.out.packets.push(pkt);
                }
            }
            ID_BLOCK => self.pending = self.block(v)?,
            ID_BLOCK_MORE => {
                (self.add_id, self.add_data) = (1, None); // BlockAddID defaults to 1
                self.walk(v, depth + 1)?;
                if self.add_id == 1 {
                    if let (Some(pkt), Some(data)) = (&mut self.pending, self.add_data.take()) {
                        pkt.alpha = Some(data);
                    }
                }
            }
            ID_BLOCK_ADD_ID => self.add_id = uint(v),
            ID_BLOCK_ADDITIONAL => self.add_data = Some(v.to_vec()),
            _ => {}
        }
        Ok(())
    }

    fn block(&mut self, v: &[u8]) -> Result<Option<Packet>, String> {
        let Some((track, n)) = vint(v, 0, false) else { return Ok(None) };
        if Some(track) != self.track || v.len() < n + 3 {
            return Ok(None);
        }
        let rel = i16::from_be_bytes([v[n], v[n + 1]]) as i64;
        if v[n + 2] & 0x06 != 0 {
            return Err(t("WebM dùng lacing, không hỗ trợ cho video", "WebM uses lacing, which isn't supported for video").into());
        }
        let rel_ms = rel * self.timescale_ns.max(1) as i64 / 1_000_000;
        let time_ms = (self.cluster_ms as i64 + rel_ms).max(0) as u64;
        Ok(Some(Packet { time_ms, color: v[n + 3..].to_vec(), alpha: None }))
    }
}

pub fn parse(data: &[u8]) -> Result<Webm, String> {
    let mut p = Parser { timescale_ns: 1_000_000, ..Default::default() };
    p.walk(data, 0)?;
    if p.track.is_none() {
        return Err(t("WebM không có track video", "WebM has no video track").into());
    }
    Ok(p.out)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.push(0x08); // 4-byte size follows the 0x08 marker's implicit 5-byte width
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    fn block(track: u8, rel: i16, flags: u8, data: &[u8]) -> Vec<u8> {
        let mut v = vec![0x80 | track];
        v.extend_from_slice(&rel.to_be_bytes());
        v.push(flags);
        v.extend_from_slice(data);
        v
    }

    fn file(cluster_body: &[u8]) -> Vec<u8> {
        let video = el(&[0xE0], &el(&[0x53, 0xC0], &[1]));
        let entry = [el(&[0xD7], &[1]), el(&[0x83], &[1]), el(&[0x86], b"V_VP9"), video].concat();
        let tracks = el(&[0x16, 0x54, 0xAE, 0x6B], &el(&[0xAE], &entry));
        let info = el(&[0x15, 0x49, 0xA9, 0x66], &el(&[0x2A, 0xD7, 0xB1], &1_000_000u32.to_be_bytes()));
        let cluster = el(&[0x1F, 0x43, 0xB6, 0x75], cluster_body);
        el(&[0x18, 0x53, 0x80, 0x67], &[info, tracks, cluster].concat())
    }

    #[test]
    fn reads_tracks_and_alpha_blocks() {
        let more = el(&[0xA6], &[el(&[0xEE], &[1]), el(&[0xA5], b"ALPHA")].concat());
        let group = el(&[0xA0], &[el(&[0xA1], &block(1, 40, 0, b"COLOR")), el(&[0x75, 0xA1], &more)].concat());
        let simple = el(&[0xA3], &block(1, 80, 0x80, b"PLAIN"));
        let other_track = el(&[0xA3], &block(2, 90, 0x80, b"AUDIO"));
        let body = [el(&[0xE7], &[0x03, 0xE8]), group, simple, other_track].concat();
        let w = parse(&file(&body)).unwrap();
        assert_eq!(w.codec, "V_VP9");
        assert!(w.has_alpha_track);
        assert_eq!(
            w.packets,
            vec![
                Packet { time_ms: 1040, color: b"COLOR".to_vec(), alpha: Some(b"ALPHA".to_vec()) },
                Packet { time_ms: 1080, color: b"PLAIN".to_vec(), alpha: None },
            ]
        );
    }

    #[test]
    fn ignores_additions_with_another_id() {
        let more = el(&[0xA6], &[el(&[0xEE], &[2]), el(&[0xA5], b"OTHER")].concat());
        let group = el(&[0xA0], &[el(&[0xA1], &block(1, 0, 0, b"C")), el(&[0x75, 0xA1], &more)].concat());
        assert_eq!(parse(&file(&group)).unwrap().packets[0].alpha, None);
    }

    #[test]
    fn rejects_lacing_and_files_without_video() {
        assert!(parse(&file(&el(&[0xA3], &block(1, 0, 0x02, b"X")))).is_err());
        assert!(parse(b"\x1a\x45\xdf\xa3 not really").is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn truncated_and_hostile_input_never_panics() {
        let good = file(&el(&[0xA3], &block(1, 0, 0x80, b"DATA")));
        for cut in 0..good.len() {
            let _ = parse(&good[..cut]);
        }
        // Absurd size claims, and nesting deeper than any real file
        let _ = parse(&[0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE]);
        let mut deep = vec![];
        for _ in 0..64 {
            deep = el(&[0x18, 0x53, 0x80, 0x67], &deep);
        }
        assert!(parse(&deep).is_err());
    }
}
