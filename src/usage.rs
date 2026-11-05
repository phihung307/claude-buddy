//! Token usage per local day, counted from the transcripts Claude Code keeps on this machine.
//! Only the `usage` numbers are read. A scan picks up where the last one stopped, so after the first
//! run it costs almost nothing, and days already counted survive Claude Code cleaning up old files.

use crate::server::Shared;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
#[cfg(windows)]
use windows_sys::Win32::Foundation::SYSTEMTIME;
#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};
#[cfg(windows)]
use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;

const MAGIC: &[u8; 4] = b"CBU1";
const MAX_DIR_DEPTH: u32 = 8;
const KEEP_DAYS: i64 = 400;
/// Messages older than this can't affect the periods shown, so they needn't be remembered one by one.
const KEEP_SEEN_DAYS: i64 = 62;

/// Input, output, cache write, cache read.
pub type Counts = [u64; 4];
/// Local day as yyyymmdd.
pub type Days = BTreeMap<u32, Counts>;

/// What "used" means here. Cache reads are shown apart: the same context is re-read on every turn, so
/// they run about 35 times larger than everything else on real data and would drown it out.
pub fn total(c: &Counts) -> u64 {
    c[0] + c[1] + c[2]
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

fn serial(day: u32) -> i64 {
    days_from_civil((day / 10_000) as i64, (day / 100 % 100) as i64, (day % 100) as i64)
}

fn from_serial(s: i64) -> u32 {
    let (y, m, d) = civil_from_days(s);
    (y * 10_000 + m * 100 + d) as u32
}

/// Monday of the week holding `day`.
pub fn week_start(day: u32) -> u32 {
    let s = serial(day);
    // Serial 0 (1970-01-01) was a Thursday, 3 days after a Monday.
    from_serial(s - (s + 3).rem_euclid(7))
}

pub fn month_start(day: u32) -> u32 {
    day / 100 * 100 + 1
}

#[cfg(windows)]
pub fn today() -> u32 {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32
}

#[cfg(unix)]
fn local_yyyymmdd(at: libc::time_t) -> Option<u32> {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&at, &mut tm) }.is_null() {
        return None;
    }
    Some((tm.tm_year + 1900) as u32 * 10_000 + (tm.tm_mon + 1) as u32 * 100 + tm.tm_mday as u32)
}

#[cfg(unix)]
pub fn today() -> u32 {
    local_yyyymmdd(unsafe { libc::time(std::ptr::null_mut()) }).unwrap_or(0)
}

fn sum_from(days: &Days, from: u32, to: u32) -> Counts {
    let mut out = [0; 4];
    for c in days.range(from..=to).map(|(_, c)| c) {
        for i in 0..4 {
            out[i] += c[i];
        }
    }
    out
}

/// Today, this week (from Monday), this month.
pub fn periods(days: &Days, today: u32) -> [Counts; 3] {
    [sum_from(days, today, today), sum_from(days, week_start(today), today), sum_from(days, month_start(today), today)]
}

/// The last `n` days up to today, oldest first, with empty days included.
pub fn recent_days(days: &Days, today: u32, n: i64) -> Vec<(u32, u64)> {
    let t = serial(today);
    (0..n).rev().map(|back| from_serial(t - back)).map(|d| (d, days.get(&d).map_or(0, total))).collect()
}

/// Short form with the language's decimal mark: 980K, 7,5M or 7.5M, 1,2B or 1.2B.
pub fn short(n: u64) -> String {
    let scaled = |v: f64, unit: &str| {
        let s = if v < 100.0 { format!("{:.1}", (v * 10.0).floor() / 10.0) } else { format!("{v:.0}") };
        format!("{}{unit}", s.trim_end_matches(".0").replace('.', if crate::i18n::vi() { "," } else { "." }))
    };
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => scaled(n as f64 / 1e3, "K"),
        1_000_000..=999_999_999 => scaled(n as f64 / 1e6, "M"),
        _ => scaled(n as f64 / 1e9, "B"),
    }
}

/// One line for the buddy's panel.
pub fn summary_line(days: &Days, today: u32) -> String {
    let [d, w, m] = periods(days, today);
    crate::tf!("Hôm nay {} · Tuần {} · Tháng {}", "Today {} · Week {} · Month {}", short(total(&d)), short(total(&w)), short(total(&m)))
}

/// Year, month, day, hour, minute, second.
fn parse_utc(ts: &str) -> Option<[u16; 6]> {
    // 2026-11-04T21:32:05.123Z
    let b = ts.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| ts.get(r)?.parse::<u16>().ok();
    let (month, day) = (num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some([num(0..4)?, month, day, num(11..13)?, num(14..16)?, num(17..19)?])
}

/// The local calendar day of a UTC timestamp, using the time zone rules in force on that date.
#[cfg(windows)]
fn local_day(ts: &str) -> Option<u32> {
    let [year, month, day, hour, minute, second] = parse_utc(ts)?;
    let utc = SYSTEMTIME { wYear: year, wMonth: month, wDayOfWeek: 0, wDay: day, wHour: hour, wMinute: minute, wSecond: second, wMilliseconds: 0 };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
    (ok != 0).then(|| local.wYear as u32 * 10_000 + local.wMonth as u32 * 100 + local.wDay as u32)
}

#[cfg(unix)]
fn local_day(ts: &str) -> Option<u32> {
    let [year, month, day, hour, minute, second] = parse_utc(ts)?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year as i32 - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day as i32;
    tm.tm_hour = hour as i32;
    tm.tm_min = minute as i32;
    tm.tm_sec = second as i32;
    local_yyyymmdd(unsafe { libc::timegm(&mut tm) })
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3))
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Seen {
    day: u32,
    counted: [u32; 4],
}

#[derive(Default, PartialEq, Debug)]
pub struct Ledger {
    pub days: Days,
    offsets: HashMap<String, u64>,
    /// Claude Code logs one message over several lines, sometimes in several files, with the numbers
    /// growing as it streams. Remembering what was counted per message keeps every token counted once.
    seen: HashMap<u64, Seen>,
}

impl Ledger {
    fn add_line(&mut self, line: &[u8]) {
        // Quotes inside message text are escaped, so this can't match something a message merely says,
        // and it skips the parse for the huge majority of lines.
        if memchr::memmem::find(line, b"\"usage\":{").is_none() || memchr::memmem::find(line, b"\"assistant\"").is_none() {
            return;
        }
        let Ok(j) = serde_json::from_slice::<Value>(line) else { return };
        if j.get("type").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        let Some(m) = j.get("message") else { return };
        let Some(u) = m.get("usage") else { return };
        let Some(day) = j.get("timestamp").and_then(Value::as_str).and_then(local_day) else { return };
        let id = m.get("id").and_then(Value::as_str).or_else(|| j.get("requestId").and_then(Value::as_str)).or_else(|| j.get("uuid").and_then(Value::as_str));
        let Some(id) = id else { return };
        let field = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0).min(u32::MAX as u64) as u32;
        let now = [field("input_tokens"), field("output_tokens"), field("cache_creation_input_tokens"), field("cache_read_input_tokens")];
        let entry = self.seen.entry(fnv(id)).or_insert(Seen { day, counted: [0; 4] });
        let slot = self.days.entry(entry.day).or_insert([0; 4]);
        for i in 0..4 {
            if now[i] > entry.counted[i] {
                slot[i] += (now[i] - entry.counted[i]) as u64;
                entry.counted[i] = now[i];
            }
        }
    }

    /// Reads whatever was appended since last time. A half-written last line waits for the next scan.
    fn scan_file(&mut self, path: &Path, len: u64) {
        let key = path.display().to_string();
        let mut from = self.offsets.get(&key).copied().unwrap_or(0);
        if len < from {
            from = 0; // replaced by a shorter file; counted messages are recognised again by id
        }
        if len == from {
            return;
        }
        let Ok(mut f) = fs::File::open(path) else { return };
        if f.seek(SeekFrom::Start(from)).is_err() {
            return;
        }
        let mut r = BufReader::with_capacity(256 * 1024, f);
        let mut line = Vec::new();
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line) {
                Ok(n) if n > 0 && line.last() == Some(&b'\n') => {
                    self.add_line(&line);
                    from += n as u64;
                }
                _ => break,
            }
        }
        self.offsets.insert(key, from);
    }

    fn prune(&mut self, today: u32) {
        let t = serial(today);
        self.days.retain(|d, _| t - serial(*d) <= KEEP_DAYS);
        self.seen.retain(|_, s| t - serial(s.day) <= KEEP_SEEN_DAYS);
    }

    fn serialize(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.seen.len() * 28 + self.offsets.len() * 120 + self.days.len() * 36 + 16);
        o.extend_from_slice(MAGIC);
        o.extend_from_slice(&(self.days.len() as u32).to_le_bytes());
        for (d, c) in &self.days {
            o.extend_from_slice(&d.to_le_bytes());
            c.iter().for_each(|v| o.extend_from_slice(&v.to_le_bytes()));
        }
        o.extend_from_slice(&(self.offsets.len() as u32).to_le_bytes());
        for (p, off) in &self.offsets {
            o.extend_from_slice(&(p.len() as u32).to_le_bytes());
            o.extend_from_slice(p.as_bytes());
            o.extend_from_slice(&off.to_le_bytes());
        }
        o.extend_from_slice(&(self.seen.len() as u32).to_le_bytes());
        for (h, s) in &self.seen {
            o.extend_from_slice(&h.to_le_bytes());
            o.extend_from_slice(&s.day.to_le_bytes());
            s.counted.iter().for_each(|v| o.extend_from_slice(&v.to_le_bytes()));
        }
        o
    }

    /// None for anything malformed: a damaged file means counting again, never trusting bad numbers.
    fn deserialize(d: &[u8]) -> Option<Ledger> {
        let mut p = 0usize;
        fn take<'a>(d: &'a [u8], p: &mut usize, n: usize) -> Option<&'a [u8]> {
            let end = p.checked_add(n)?;
            let s = d.get(*p..end)?;
            *p = end;
            Some(s)
        }
        fn u32_at(d: &[u8], p: &mut usize) -> Option<u32> {
            take(d, p, 4)?.try_into().ok().map(u32::from_le_bytes)
        }
        fn u64_at(d: &[u8], p: &mut usize) -> Option<u64> {
            take(d, p, 8)?.try_into().ok().map(u64::from_le_bytes)
        }
        if take(d, &mut p, 4)? != MAGIC {
            return None;
        }
        let mut l = Ledger::default();
        for _ in 0..u32_at(d, &mut p)? {
            let day = u32_at(d, &mut p)?;
            let c = [u64_at(d, &mut p)?, u64_at(d, &mut p)?, u64_at(d, &mut p)?, u64_at(d, &mut p)?];
            l.days.insert(day, c);
        }
        for _ in 0..u32_at(d, &mut p)? {
            let n = u32_at(d, &mut p)? as usize;
            let key = String::from_utf8(take(d, &mut p, n)?.to_vec()).ok()?;
            let off = u64_at(d, &mut p)?;
            l.offsets.insert(key, off);
        }
        for _ in 0..u32_at(d, &mut p)? {
            let h = u64_at(d, &mut p)?;
            let day = u32_at(d, &mut p)?;
            let counted = [u32_at(d, &mut p)?, u32_at(d, &mut p)?, u32_at(d, &mut p)?, u32_at(d, &mut p)?];
            l.seen.insert(h, Seen { day, counted });
        }
        (p == d.len()).then_some(l)
    }
}

/// Just the per-day numbers, the first section of the saved state.
fn read_days(d: &[u8]) -> Option<Days> {
    let body = d.strip_prefix(MAGIC.as_slice())?;
    let count = u32::from_le_bytes(body.get(..4)?.try_into().ok()?) as usize;
    let rows = body.get(4..count.checked_mul(36)?.checked_add(4)?)?;
    let mut days = Days::new();
    for row in rows.chunks_exact(36) {
        let day = u32::from_le_bytes(row[..4].try_into().ok()?);
        let mut c = [0u64; 4];
        for (i, v) in row[4..].chunks_exact(8).enumerate() {
            c[i] = u64::from_le_bytes(v.try_into().ok()?);
        }
        days.insert(day, c);
    }
    Some(days)
}

fn collect(dir: &Path, depth: u32, out: &mut Vec<(PathBuf, u64)>) {
    if depth > MAX_DIR_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            collect(&e.path(), depth + 1, out);
        } else if t.is_file() && e.file_name().to_string_lossy().ends_with(".jsonl") {
            if let Ok(m) = e.metadata() {
                out.push((e.path(), m.len()));
            }
        }
    }
}

fn scan_into(ledger: &mut Ledger, projects: &Path, today: u32) -> bool {
    let mut files = Vec::new();
    collect(projects, 0, &mut files);
    let changed = files.iter().any(|(p, len)| ledger.offsets.get(&p.display().to_string()) != Some(len));
    if changed {
        for (p, len) in &files {
            ledger.scan_file(p, *len);
        }
        ledger.prune(today);
    }
    changed
}

fn projects_dir() -> PathBuf {
    let settings = crate::hooks_config::settings_path();
    settings.parent().map_or_else(|| PathBuf::from("projects"), |d| d.join("projects"))
}

fn state_file() -> PathBuf {
    crate::config::dir().join("usage.bin")
}

/// `claude-buddy-rs --scan-usage`: counts anything new and saves it. A process of its own because the
/// first run walks every transcript on the machine, and Windows' heap would keep that memory in the
/// app for good; a process that exits gives all of it back.
pub fn scan_main() {
    // Anything else that wants the CPU goes first.
    #[cfg(windows)]
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST) };
    #[cfg(unix)]
    unsafe { libc::nice(19) };
    let file = state_file();
    let mut ledger = fs::read(&file).ok().and_then(|d| Ledger::deserialize(&d)).unwrap_or_default();
    if scan_into(&mut ledger, &projects_dir(), today()) {
        if let Some(d) = file.parent() {
            let _ = fs::create_dir_all(d);
        }
        let tmp = file.with_extension("tmp");
        if fs::write(&tmp, ledger.serialize()).is_ok() {
            let _ = fs::rename(&tmp, &file);
        }
    }
}

/// Runs a scan unless one is already running, then hands the app the fresh numbers.
pub fn request_scan(shared: Arc<Shared>) {
    if shared.scanning.swap(true, Ordering::AcqRel) {
        return;
    }
    let worker = shared.clone();
    let spawned = std::thread::Builder::new().stack_size(256 * 1024).name("usage-scan".into()).spawn(move || {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("--scan-usage").status();
        }
        if let Some(days) = fs::read(state_file()).ok().and_then(|d| read_days(&d)) {
            *worker.usage.lock().unwrap() = days;
        }
        worker.scanned.store(true, Ordering::Release);
        worker.scanning.store(false, Ordering::Release);
        #[cfg(any(windows, target_os = "macos"))]
        {
            crate::window::wake(worker.hwnd.load(Ordering::Relaxed));
            crate::picker::notify_changed();
        }
    });
    if spawned.is_err() {
        shared.scanning.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod real_files {
    use super::*;

    /// Where the time goes on real data: BUDDY_PROJECTS=<folder> cargo test --release real_usage -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_usage_scan_profile() {
        let root = PathBuf::from(std::env::var("BUDDY_PROJECTS").expect("BUDDY_PROJECTS"));
        let mut files = Vec::new();
        collect(&root, 0, &mut files);
        let (mut bytes, mut lines, mut hit, mut hit_bytes, mut longest) = (0u64, 0u64, 0u64, 0u64, 0usize);
        let t = std::time::Instant::now();
        let mut buf = Vec::new();
        for (p, _) in &files {
            let mut r = BufReader::with_capacity(256 * 1024, fs::File::open(p).unwrap());
            loop {
                buf.clear();
                let n = r.read_until(b'\n', &mut buf).unwrap();
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                lines += 1;
                longest = longest.max(n);
                if memchr::memmem::find(&buf, b"\"usage\":{").is_some() && memchr::memmem::find(&buf, b"\"assistant\"").is_some() {
                    hit += 1;
                    hit_bytes += n as u64;
                }
            }
        }
        println!("read+filter {:.1}s: {} files, {:.0} MB, {lines} lines, longest line {:.1} MB; candidates {hit} ({:.0} MB)", t.elapsed().as_secs_f64(), files.len(), bytes as f64 / 1e6, longest as f64 / 1e6, hit_bytes as f64 / 1e6);
        let t = std::time::Instant::now();
        let mut l = Ledger::default();
        scan_into(&mut l, &root, today());
        println!("full scan {:.1}s, {} messages, {} days", t.elapsed().as_secs_f64(), l.seen.len(), l.days.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Temp {
            let d = std::env::temp_dir().join(format!("buddy-usage-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Temp(d)
        }
        fn append(&self, rel: &str, text: &str) {
            use std::io::Write;
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::OpenOptions::new().create(true).append(true).open(p).unwrap().write_all(text.as_bytes()).unwrap();
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Noon UTC stays on the same calendar day in nearly every zone near Vietnam's and is what tests
    /// compare against by asking the same conversion, so they hold in any zone.
    const TS: &str = "2026-11-04T12:00:00.000Z";

    fn msg(id: &str, ts: &str, input: u64, output: u64, write: u64, read: u64) -> String {
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"{ts}\",\"requestId\":\"r\",\"message\":{{\"id\":\"{id}\",\"content\":[],\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output},\"cache_creation_input_tokens\":{write},\"cache_read_input_tokens\":{read}}}}}}}\n"
        )
    }

    fn day() -> u32 {
        local_day(TS).unwrap()
    }

    fn scan(l: &mut Ledger, t: &Temp) -> bool {
        scan_into(l, &t.0, day())
    }

    #[test]
    fn calendar_math_is_right_across_boundaries() {
        assert_eq!(week_start(20261104), 20261102, "a Wednesday goes back to Monday");
        assert_eq!(week_start(20261102), 20261102, "a Monday is its own start");
        assert_eq!(week_start(20261108), 20261102, "a Sunday belongs to the week before it");
        assert_eq!(week_start(20260101), 20251229, "weeks cross years");
        assert_eq!(week_start(20240302), 20240226, "leap February");
        assert_eq!(month_start(20261104), 20261101);
        for s in [-1000, 0, 365, 20_000, 30_000] {
            assert_eq!(serial(from_serial(s)), s);
        }
    }

    #[test]
    fn periods_add_up_by_calendar() {
        let mut d = Days::new();
        d.insert(20261104, [1, 1, 1, 100]); // today, Wednesday
        d.insert(20261102, [10, 0, 0, 0]); // Monday: this week
        d.insert(20261101, [100, 0, 0, 0]); // Sunday: last week, this month
        d.insert(20261031, [1000, 0, 0, 0]); // last month
        d.insert(20261105, [9999, 0, 0, 0]); // future (clock changed): never counted
        let [t, w, m] = periods(&d, 20261104);
        assert_eq!(t, [1, 1, 1, 100]);
        assert_eq!(total(&w), 13);
        assert_eq!(total(&m), 113);
        assert_eq!(total(&t), 3, "cache reads stay out of the total");
    }

    #[test]
    fn recent_days_cover_gaps_and_month_ends() {
        let mut d = Days::new();
        d.insert(20261101, [5, 0, 0, 99]);
        d.insert(20261031, [1, 1, 1, 0]);
        d.insert(20261020, [7, 0, 0, 0]); // too old for this window
        assert_eq!(recent_days(&d, 20261102, 4), vec![(20261030, 0), (20261031, 3), (20261101, 5), (20261102, 0)]);
    }

    #[test]
    fn numbers_read_short() {
        assert_eq!(short(0), "0");
        assert_eq!(short(999), "999");
        assert_eq!(short(1_000), "1K");
        assert_eq!(short(980_400), "980K");
        assert_eq!(short(7_485_185), "7,4M");
        assert_eq!(short(1_200_000_000), "1,2B");
        assert_eq!(short(30_000_000), "30M");
    }

    #[test]
    fn timestamps_parse_strictly() {
        assert!(local_day(TS).is_some());
        for bad in ["", "yesterday", "2026-13-04T12:00:00Z", "2026-11-04 12:00:00", "2026-11-40T12:00:00Z", "é026-11-04T12:00:00Z"] {
            assert_eq!(local_day(bad), None, "{bad}");
        }
    }

    #[test]
    fn counts_each_message_once_at_its_highest_numbers() {
        let t = Temp::new("dedupe");
        t.append("p/a.jsonl", &msg("m1", TS, 5, 1, 100, 1000));
        t.append("p/a.jsonl", &msg("m1", TS, 5, 40, 100, 1000)); // same message, more output as it streamed
        t.append("p/a.jsonl", &msg("m1", TS, 5, 40, 100, 1000));
        t.append("p/a.jsonl", &msg("m2", TS, 1, 2, 3, 4));
        let mut l = Ledger::default();
        scan(&mut l, &t);
        assert_eq!(l.days[&day()], [6, 42, 103, 1004]);
    }

    #[test]
    fn the_same_message_in_another_file_is_not_counted_again() {
        let t = Temp::new("crossfile");
        t.append("p/session.jsonl", &msg("m1", TS, 5, 5, 5, 5));
        t.append("p/resumed.jsonl", &msg("m1", TS, 5, 5, 5, 5));
        t.append("p/session/subagents/agent.jsonl", &msg("sub", TS, 1, 1, 1, 1));
        let mut l = Ledger::default();
        scan(&mut l, &t);
        assert_eq!(l.days[&day()], [6, 6, 6, 6], "subagents count, copies don't");
    }

    #[test]
    fn only_real_assistant_usage_counts() {
        let t = Temp::new("filter");
        // A user pasting usage JSON: inside a string, so its quotes are escaped.
        t.append("a.jsonl", "{\"type\":\"user\",\"timestamp\":\"2026-11-04T12:00:00Z\",\"message\":{\"content\":\"\\\"usage\\\":{\\\"input_tokens\\\":999} assistant\"}}\n");
        // Unescaped but on the wrong kind of line
        t.append("a.jsonl", "{\"type\":\"progress\",\"note\":\"assistant\",\"timestamp\":\"2026-11-04T12:00:00Z\",\"message\":{\"id\":\"x\",\"usage\":{\"input_tokens\":999}}}\n");
        t.append("a.jsonl", "not json \"usage\":{ \"assistant\"\n");
        t.append("a.jsonl", "{\"type\":\"assistant\",\"message\":{\"id\":\"no-time\",\"usage\":{\"input_tokens\":999}}}\n");
        t.append("a.jsonl", &msg("ok", TS, 1, 0, 0, 0));
        let mut l = Ledger::default();
        scan(&mut l, &t);
        assert_eq!(l.days[&day()], [1, 0, 0, 0]);
    }

    #[test]
    fn scans_are_incremental_and_wait_for_whole_lines() {
        let t = Temp::new("incremental");
        t.append("a.jsonl", &msg("m1", TS, 1, 0, 0, 0));
        let mut l = Ledger::default();
        assert!(scan(&mut l, &t));
        assert!(!scan(&mut l, &t), "nothing new");

        let second = msg("m2", TS, 2, 0, 0, 0);
        let (head, tail) = second.split_at(30);
        t.append("a.jsonl", head);
        assert!(scan(&mut l, &t));
        assert_eq!(l.days[&day()][0], 1, "half a line is not read yet");
        t.append("a.jsonl", tail);
        scan(&mut l, &t);
        assert_eq!(l.days[&day()][0], 3);
        // Growth across scans is counted as the difference only.
        t.append("a.jsonl", &msg("m2", TS, 2, 10, 0, 0));
        scan(&mut l, &t);
        assert_eq!(l.days[&day()], [3, 10, 0, 0]);
    }

    #[test]
    fn a_rewritten_file_does_not_count_twice() {
        let t = Temp::new("rewrite");
        t.append("a.jsonl", &msg("m1", TS, 7, 0, 0, 0));
        t.append("a.jsonl", &msg("m2", TS, 7, 0, 0, 0));
        let mut l = Ledger::default();
        scan(&mut l, &t);
        fs::write(t.0.join("a.jsonl"), msg("m1", TS, 7, 0, 0, 0)).unwrap();
        scan(&mut l, &t);
        assert_eq!(l.days[&day()][0], 14);
    }

    #[test]
    fn deleted_transcripts_keep_their_days() {
        let t = Temp::new("deleted");
        t.append("a.jsonl", &msg("m1", TS, 7, 0, 0, 0));
        let mut l = Ledger::default();
        scan(&mut l, &t);
        fs::remove_file(t.0.join("a.jsonl")).unwrap();
        t.append("b.jsonl", &msg("m2", TS, 1, 0, 0, 0));
        scan(&mut l, &t);
        assert_eq!(l.days[&day()][0], 8);
    }

    #[test]
    fn old_data_is_pruned_but_recent_stays() {
        let mut l = Ledger::default();
        l.days.insert(20240101, [1; 4]);
        l.days.insert(20261001, [1; 4]);
        l.seen.insert(1, Seen { day: 20260801, counted: [0; 4] });
        l.seen.insert(2, Seen { day: 20261020, counted: [0; 4] });
        l.prune(20261104);
        assert_eq!(l.days.keys().copied().collect::<Vec<_>>(), [20261001]);
        assert_eq!(l.seen.keys().copied().collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn state_round_trips_and_rejects_damage() {
        let t = Temp::new("state");
        t.append("đường dẫn/a.jsonl", &msg("m1", TS, 1, 2, 3, 4));
        let mut l = Ledger::default();
        scan(&mut l, &t);
        let bytes = l.serialize();
        assert_eq!(Ledger::deserialize(&bytes).unwrap(), l);
        assert_eq!(read_days(&bytes).unwrap(), l.days);
        assert!(read_days(b"nope").is_none());
        assert!(read_days(&bytes[..6]).is_none());
        for cut in 0..bytes.len() {
            assert!(Ledger::deserialize(&bytes[..cut]).is_none(), "truncated at {cut}");
        }
        assert!(Ledger::deserialize(b"nope").is_none());
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Ledger::deserialize(&extra).is_none());
        // Absurd counts must fail cleanly rather than try to allocate or spin.
        let mut huge = MAGIC.to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(Ledger::deserialize(&huge).is_none());
    }
}
