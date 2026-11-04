//! The character collection: folders in the same format as the original app (character.json plus one
//! media file per state), so existing character folders can be imported as they are.

use crate::i18n::t;
use crate::media::{self, Kind, MAX_MEDIA_BYTES};
use crate::motion::Motion;
use crate::render::CHARACTER_NAMES;
use crate::server::now_ms;
use crate::store::State;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const STATES: [State; 5] = [State::Idle, State::Working, State::Waiting, State::Done, State::Error];
pub const STATE_KEYS: [&str; 5] = ["idle", "working", "waiting", "done", "error"];
pub fn state_text() -> [&'static str; 5] {
    if crate::i18n::vi() {
        ["Rảnh", "Đang chạy", "Cần bạn", "Đã xong", "Lỗi"]
    } else {
        ["Idle", "Working", "Needs you", "Done", "Error"]
    }
}
const MEDIA_EXTS: [&str; 9] = [".gif", ".png", ".webp", ".apng", ".svg", ".jpg", ".jpeg", ".webm", ".mp4"];
const SOUND_EXTS: [&str; 2] = [".wav", ".mp3"];
/// Motion for still images that don't set one.
const STATIC_MOTION: [Motion; 5] = [Motion::Bob, Motion::Type, Motion::Nudge, Motion::Hop, Motion::Nudge];
const VECTOR_IDS: [&str; 3] = ["mochi", "meo-muop", "gau-bo"];
const MAX_IMPORT_DEPTH: u32 = 4;

pub fn state_index(s: State) -> usize {
    STATES.iter().position(|x| *x == s).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateMedia {
    pub file: String,
    pub kind: Kind,
    pub motion: Motion,
    /// No media of its own: shows idle, and a badge keeps the state recognisable.
    pub fallback: bool,
    /// Motion chosen by hand in character.json.
    pub manual: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Drawn by the app. The index selects the look.
    Vector(usize),
    Folder(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub scale: f32,
    pub source: Source,
    pub states: [StateMedia; 5],
    /// Custom sounds for done and waiting.
    pub sounds: [Option<PathBuf>; 2],
}

impl Character {
    pub fn is_builtin(&self) -> bool {
        matches!(self.source, Source::Vector(_))
    }

    pub fn dir(&self) -> Option<&Path> {
        match &self.source {
            Source::Folder(d) => Some(d),
            Source::Vector(_) => None,
        }
    }
}

/// ASCII slug; drops Vietnamese diacritics ("Mèo Đen" → "meo-den").
pub fn slugify(s: &str) -> String {
    let base = |c: char| -> char {
        match c {
            'à' | 'á' | 'ả' | 'ã' | 'ạ' | 'ă' | 'ằ' | 'ắ' | 'ẳ' | 'ẵ' | 'ặ' | 'â' | 'ầ' | 'ấ' | 'ẩ' | 'ẫ' | 'ậ' | 'ä' | 'å' => 'a',
            'è' | 'é' | 'ẻ' | 'ẽ' | 'ẹ' | 'ê' | 'ề' | 'ế' | 'ể' | 'ễ' | 'ệ' | 'ë' => 'e',
            'ì' | 'í' | 'ỉ' | 'ĩ' | 'ị' | 'ï' => 'i',
            'ò' | 'ó' | 'ỏ' | 'õ' | 'ọ' | 'ô' | 'ồ' | 'ố' | 'ổ' | 'ỗ' | 'ộ' | 'ơ' | 'ờ' | 'ớ' | 'ở' | 'ỡ' | 'ợ' | 'ö' => 'o',
            'ù' | 'ú' | 'ủ' | 'ũ' | 'ụ' | 'ư' | 'ừ' | 'ứ' | 'ử' | 'ữ' | 'ự' | 'ü' => 'u',
            'ỳ' | 'ý' | 'ỷ' | 'ỹ' | 'ỵ' | 'ÿ' => 'y',
            'đ' => 'd',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        }
    };
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars().flat_map(char::to_lowercase).map(base) {
        // Combining marks left by decomposed input (U+0300 to U+036F) vanish instead of splitting words.
        if ('\u{300}'..='\u{36f}').contains(&c) {
            continue;
        }
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

fn ext_of(name: &str) -> String {
    Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

fn kind_from_ext(name: &str) -> Kind {
    match ext_of(name).as_str() {
        ".webm" | ".mp4" => Kind::Video,
        ".gif" | ".webp" | ".apng" => Kind::Animated,
        _ => Kind::Static,
    }
}

/// A plain media file name inside the folder; a manifest must never point elsewhere on disk.
fn is_safe_file(dir: &Path, name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':']) {
        return false;
    }
    if !MEDIA_EXTS.contains(&ext_of(name).as_str()) {
        return false;
    }
    match fs::symlink_metadata(dir.join(name)) {
        Ok(m) => m.is_file(),
        Err(_) => false,
    }
}

fn find_file(dir: &Path, base: &str, exts: &[&str]) -> Option<String> {
    exts.iter().map(|e| format!("{base}{e}")).find(|n| dir.join(n).is_file())
}

fn resolve_motion(index: usize, kind: Kind, explicit: Option<Motion>) -> Motion {
    explicit.unwrap_or(if kind == Kind::Static { STATIC_MOTION[index] } else { Motion::None })
}

fn clean_str(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// Validates a character.json (current schema or the old flat one with a top-level "motion" map)
/// against the files in `dir`. Unusable without idle media.
pub fn normalize(raw: &Value, dir: &Path) -> Result<Character, String> {
    if !raw.is_object() {
        return Err(t("character.json không phải object", "character.json is not an object").into());
    }
    let empty = Map::new();
    let raw_states = raw.get("states").and_then(Value::as_object).unwrap_or(&empty);
    let legacy_motion = raw.get("motion").and_then(Value::as_object).unwrap_or(&empty);
    let mut own: [Option<(String, Kind)>; 5] = Default::default();
    let mut explicit: [Option<Motion>; 5] = [None; 5];
    for (i, key) in STATE_KEYS.iter().enumerate() {
        let v = raw_states.get(*key);
        let (file_field, kind_field, motion_field) = match v {
            Some(Value::String(s)) => (Some(s.as_str()), None, None),
            Some(Value::Object(o)) => (o.get("file").and_then(Value::as_str), o.get("kind").and_then(Value::as_str), o.get("motion").and_then(Value::as_str)),
            _ => (None, None, None),
        };
        explicit[i] = motion_field.or_else(|| legacy_motion.get(*key).and_then(Value::as_str)).and_then(Motion::parse);
        let file = match file_field.filter(|f| is_safe_file(dir, f)) {
            Some(f) => Some(f.to_string()),
            None => find_file(dir, key, &MEDIA_EXTS),
        };
        if let Some(file) = file {
            let kind = kind_field.and_then(Kind::parse).unwrap_or_else(|| kind_from_ext(&file));
            own[i] = Some((file, kind));
        }
    }
    let Some((idle_file, idle_kind)) = own[0].clone() else {
        return Err(crate::tf!("không có media cho trạng thái idle trong {}", "no media for the idle state in {}", dir.display()));
    };
    let states: [StateMedia; 5] = std::array::from_fn(|i| {
        let (file, kind) = own[i].clone().unwrap_or((idle_file.clone(), idle_kind));
        StateMedia { file, kind, motion: resolve_motion(i, kind, explicit[i]), fallback: own[i].is_none(), manual: explicit[i].is_some() }
    });
    let name = clean_str(raw.get("name"));
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let id = [clean_str(raw.get("id")), name.clone(), folder].iter().map(|s| slugify(s)).find(|s| !s.is_empty()).ok_or(t("không tạo được id", "could not create an id"))?;
    let scale = raw.get("scale").and_then(Value::as_f64).filter(|s| *s > 0.0 && *s <= 5.0).unwrap_or(1.0) as f32;
    let sounds_dir = dir.join("sounds");
    let sound = |base: &str| find_file(&sounds_dir, base, &SOUND_EXTS).map(|f| sounds_dir.join(f));
    Ok(Character {
        name: if name.is_empty() { id.clone() } else { name },
        id,
        author: clean_str(raw.get("author")),
        description: clean_str(raw.get("description")),
        scale,
        source: Source::Folder(dir.to_path_buf()),
        states,
        sounds: [sound("done"), sound("waiting")],
    })
}

/// A missing character.json is allowed: files named by state are enough.
pub fn load_dir(dir: &Path) -> Result<Character, String> {
    let file = dir.join("character.json");
    let raw = if file.exists() {
        let text = fs::read_to_string(&file).map_err(|e| e.to_string())?;
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| crate::tf!("character.json lỗi: {e}", "character.json error: {e}"))?
    } else {
        json!({})
    };
    normalize(&raw, dir)
}

fn vector_character(i: usize) -> Character {
    let media = |_| StateMedia { file: String::new(), kind: Kind::Animated, motion: Motion::None, fallback: false, manual: false };
    Character {
        id: VECTOR_IDS[i].into(),
        name: CHARACTER_NAMES[i].into(),
        author: "Claude Buddy".into(),
        description: String::new(),
        scale: 1.0,
        source: Source::Vector(i),
        states: std::array::from_fn(media),
        sounds: [None, None],
    }
}

/// Built-in characters first, then the user's. Broken folders and duplicate ids are skipped so one
/// bad folder never empties the list.
pub fn load_all(user_root: &Path) -> Vec<Character> {
    let mut out: Vec<Character> = (0..VECTOR_IDS.len()).map(vector_character).collect();
    let Ok(entries) = fs::read_dir(user_root) else { return out };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.')) // staging folders
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for d in dirs {
        if let Ok(c) = load_dir(&d) {
            if !out.iter().any(|x| x.id == c.id) {
                out.push(c);
            }
        }
    }
    out
}

/// What the add/edit dialog sends for one state.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Slot {
    #[default]
    Empty,
    Keep {
        motion: Option<Motion>,
    },
    New {
        path: PathBuf,
        motion: Option<Motion>,
        remove_white: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub name: String,
    pub scale: f32,
    pub slots: [Slot; 5],
}

#[derive(Debug, PartialEq)]
pub struct FileInfo {
    pub kind: Kind,
    pub ext: &'static str,
    pub size: u64,
    pub white_bg: bool,
}

/// Sniffed from the bytes: a renamed file can't pretend to be something else.
pub fn inspect_file(path: &Path) -> Result<FileInfo, String> {
    let name = display_name(path);
    let meta = fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if !meta.is_file() {
        return Err(crate::tf!("{name} không phải là file.", "{name} is not a file."));
    }
    if meta.len() > MAX_MEDIA_BYTES {
        return Err(crate::tf!("{name} quá lớn (giới hạn 50 MB).", "{name} is too large (limit 50 MB)."));
    }
    let data = fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let (kind, ext) = media::sniff(&data);
    if kind == Kind::Unknown {
        return Err(crate::tf!("{name} không phải ảnh hay video được hỗ trợ.", "{name} is not a supported image or video."));
    }
    let white_bg = kind == Kind::Static
        && ext != ".svg"
        && media::load_rgba(path).is_ok_and(|(px, w, h)| media::has_white_border(&px, w, h));
    Ok(FileInfo { kind, ext, size: meta.len(), white_bg })
}

pub fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

struct Plan {
    index: usize,
    source: PlanSource,
    ext: String,
    kind: Kind,
    motion: Option<Motion>,
}

enum PlanSource {
    Copy(PathBuf),
    Bytes(Vec<u8>),
}

fn plan_new(index: usize, path: &Path, motion: Option<Motion>, remove_white: bool) -> Result<Plan, String> {
    let info = inspect_file(path)?;
    let still = info.kind == Kind::Static;
    let (source, ext) = if remove_white && still && info.ext != ".svg" {
        let (mut px, w, h) = media::load_rgba(path).map_err(|e| format!("{}: {e}", display_name(path)))?;
        media::remove_white_background(&mut px, w, h);
        (PlanSource::Bytes(media::encode_png(&px, w, h)?), ".png".to_string())
    } else {
        (PlanSource::Copy(path.to_path_buf()), info.ext.to_string())
    };
    // Manual motion only applies to still images; everything else moves by itself.
    Ok(Plan { index, source, ext, kind: info.kind, motion: motion.filter(|_| still) })
}

fn clean_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(t("Tên không được để trống.", "The name can't be empty.").into());
    }
    Ok(n.to_string())
}

fn clamp_scale(s: f32) -> f64 {
    let v = if s.is_finite() && s > 0.0 { s } else { 1.0 };
    (v.clamp(0.5, 1.5) as f64 * 100.0).round() / 100.0
}

/// Writes `<state><ext>` files and character.json, then checks it loads back before it becomes visible.
fn write_character(staging: &Path, plans: &[Plan], mut manifest: Value) -> Result<(), String> {
    let mut states = Map::new();
    for p in plans {
        let file = format!("{}{}", STATE_KEYS[p.index], p.ext);
        let target = staging.join(&file);
        match &p.source {
            PlanSource::Copy(src) => {
                fs::copy(src, &target).map_err(|e| format!("{}: {e}", display_name(src)))?;
            }
            PlanSource::Bytes(b) => fs::write(&target, b).map_err(|e| e.to_string())?,
        }
        let mut st = json!({ "file": file, "kind": p.kind.name() });
        if let Some(m) = p.motion {
            st["motion"] = json!(m.name());
        }
        states.insert(STATE_KEYS[p.index].to_string(), st);
    }
    manifest["states"] = Value::Object(states);
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())? + "\n";
    fs::write(staging.join("character.json"), text).map_err(|e| e.to_string())?;
    load_dir(staging).map(|_| ())
}

fn fresh_staging(user_root: &Path, name: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(user_root).map_err(|e| e.to_string())?;
    let staging = user_root.join(name);
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging).map_err(|e| e.to_string())?;
    Ok(staging)
}

/// Creates `<user_root>/<id>/` from the files picked in the add dialog. Returns the new id.
pub fn create(draft: &Draft, user_root: &Path, is_taken: &dyn Fn(&str) -> bool) -> Result<String, String> {
    let name = clean_name(&draft.name)?;
    let id = slugify(&name);
    if id.is_empty() {
        return Err(t("Tên cần có ít nhất một chữ cái hoặc chữ số.", "The name needs at least one letter or digit.").into());
    }
    if !matches!(draft.slots[0], Slot::New { .. }) {
        return Err(t("Cần ít nhất file cho trạng thái Rảnh (idle).", "The Idle state needs a file.").into());
    }
    if is_taken(&id) || user_root.join(&id).exists() {
        return Err(crate::tf!("Đã có nhân vật với id \"{id}\". Hãy đổi tên khác.", "A character with id \"{id}\" already exists. Choose another name."));
    }
    let mut plans = Vec::new();
    for (i, slot) in draft.slots.iter().enumerate() {
        if let Slot::New { path, motion, remove_white } = slot {
            plans.push(plan_new(i, path, *motion, *remove_white)?);
        }
    }
    let staging = fresh_staging(user_root, &format!(".creating-{id}"))?;
    let manifest = json!({ "schema": 1, "id": id, "name": name, "author": "", "scale": clamp_scale(draft.scale) });
    let result = write_character(&staging, &plans, manifest).and_then(|()| fs::rename(&staging, user_root.join(&id)).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(|()| id)
}

/// Applies an edit in place, swapped in only once complete. The id never changes.
pub fn update(c: &Character, draft: &Draft, user_root: &Path) -> Result<(), String> {
    let name = clean_name(&draft.name)?;
    let Some(dir) = c.dir() else { return Err(t("Nhân vật có sẵn của app không sửa được.", "Built-in characters can't be edited.").into()) };
    let target = user_root.join(&c.id);
    if !same_path(dir, &target) && dir.parent().map(|p| same_path(p, user_root)) != Some(true) {
        return Err(t("Nhân vật này không nằm trong thư mục của bạn.", "This character isn't in your folder.").into());
    }
    let mut plans = Vec::new();
    for (i, slot) in draft.slots.iter().enumerate() {
        match slot {
            Slot::Empty => {}
            Slot::Keep { motion } => {
                let cur = &c.states[i];
                if cur.fallback {
                    continue; // nothing of its own to keep
                }
                plans.push(Plan { index: i, source: PlanSource::Copy(dir.join(&cur.file)), ext: ext_of(&cur.file), kind: cur.kind, motion: *motion });
            }
            Slot::New { path, motion, remove_white } => plans.push(plan_new(i, path, *motion, *remove_white)?),
        }
    }
    if !plans.iter().any(|p| p.index == 0) {
        return Err(t("Cần ít nhất file cho trạng thái Rảnh (idle).", "The Idle state needs a file.").into());
    }
    // Keep fields the dialog does not edit (author, description…); drop the legacy motion map.
    let mut raw: Value = fs::read_to_string(dir.join("character.json")).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or_else(|| json!({}));
    if !raw.is_object() {
        raw = json!({});
    }
    raw.as_object_mut().map(|o| o.shift_remove("motion"));
    raw["schema"] = json!(1);
    raw["id"] = json!(c.id);
    raw["name"] = json!(name);
    raw["scale"] = json!(clamp_scale(draft.scale));

    let staging = fresh_staging(user_root, &format!(".editing-{}", c.id))?;
    let result = (|| {
        write_character(&staging, &plans, raw)?;
        let sounds = dir.join("sounds");
        if sounds.is_dir() {
            copy_tree(&sounds, &staging.join("sounds"), 1)?;
        }
        let trash = user_root.join(format!(".trash-{}-{}", c.id, now_ms()));
        fs::rename(dir, &trash).map_err(|e| e.to_string())?;
        if let Err(e) = fs::rename(&staging, &target) {
            let _ = fs::rename(&trash, dir); // put the original back
            return Err(e.to_string());
        }
        let _ = fs::remove_dir_all(&trash);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Why a character cannot be removed; None when it can.
pub fn remove_blocker(c: &Character, current: &str) -> Option<String> {
    if c.is_builtin() {
        return Some(t("Nhân vật có sẵn của app không xoá được.", "Built-in characters can't be deleted.").into());
    }
    if c.id == current {
        return Some(t("Đang dùng nhân vật này. Hãy chọn nhân vật khác trước khi xoá.", "This character is in use. Choose another one before deleting it.").into());
    }
    None
}

/// Deletes a user character folder; refuses anything that is not directly inside `user_root`.
pub fn remove_dir(c: &Character, user_root: &Path) -> Result<(), String> {
    let Some(dir) = c.dir() else { return Err(t("Nhân vật có sẵn của app không xoá được.", "Built-in characters can't be deleted.").into()) };
    let root = user_root.canonicalize().map_err(|e| e.to_string())?;
    let full = dir.canonicalize().map_err(|e| e.to_string())?;
    if full.parent() != Some(root.as_path()) {
        return Err(crate::tf!("{} không phải nhân vật của bạn.", "{} is not one of your characters.", dir.display()));
    }
    fs::remove_dir_all(&full).map_err(|e| e.to_string())
}

/// Symlinks are skipped so nothing outside the folder comes along.
fn copy_tree(src: &Path, dst: &Path, depth: u32) -> Result<(), String> {
    if depth > MAX_IMPORT_DEPTH {
        return Ok(());
    }
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for e in fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let Ok(t) = e.file_type() else { continue };
        let target = dst.join(e.file_name());
        if t.is_dir() {
            copy_tree(&e.path(), &target, depth + 1)?;
        } else if t.is_file() && e.file_name() != "thumb.png" {
            fs::copy(e.path(), &target).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

fn unique_id(user_root: &Path, base: &str, is_taken: &dyn Fn(&str) -> bool) -> String {
    let free = |id: &str| !is_taken(id) && !user_root.join(id).exists();
    if free(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base}-{i}")).find(|id| free(id)).unwrap_or_else(|| base.to_string())
}

/// Copies an existing character folder in under a free id. Returns the new id.
pub fn import_dir(src: &Path, user_root: &Path, is_taken: &dyn Fn(&str) -> bool) -> Result<String, String> {
    let c = load_dir(src)?;
    let id = unique_id(user_root, &c.id, is_taken);
    let staging = fresh_staging(user_root, &format!(".importing-{id}"))?;
    let result = (|| {
        copy_tree(src, &staging, 0)?;
        // The manifest is rewritten rather than left as is: an old flat one or none at all becomes current.
        let mut raw: Value = fs::read_to_string(staging.join("character.json")).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or_else(|| json!({}));
        if !raw.is_object() {
            raw = json!({});
        }
        raw["id"] = json!(id);
        if raw.get("name").and_then(Value::as_str).is_none_or(|n| n.trim().is_empty()) {
            raw["name"] = json!(c.name);
        }
        let text = serde_json::to_string_pretty(&raw).map_err(|e| e.to_string())? + "\n";
        fs::write(staging.join("character.json"), text).map_err(|e| e.to_string())?;
        load_dir(&staging)?;
        fs::rename(&staging, user_root.join(&id)).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(|()| id)
}

/// Guesses a state from a file name when several files are dropped together.
pub fn guess_state(file_name: &str) -> Option<usize> {
    let n = file_name.to_lowercase();
    const GUESS: [(&[&str], usize); 5] = [
        (&["error", "fail", "sad", "cry"], 4),
        (&["idle", "stand", "rest"], 0),
        (&["work", "run"], 1),
        (&["wait", "ask"], 2),
        (&["done", "finish"], 3),
    ];
    GUESS.iter().find(|(words, _)| words.iter().any(|w| n.contains(w))).map(|(_, s)| *s)
}

/// The manifest the dialog is about to write, shown so nothing about the save is a surprise.
pub fn manifest_preview(draft: &Draft, id: &str, exts: &[Option<String>; 5], kinds: &[Option<Kind>; 5]) -> String {
    let mut states = Map::new();
    for i in 0..5 {
        if let (Some(ext), Some(kind)) = (&exts[i], kinds[i]) {
            let mut st = json!({ "file": format!("{}{ext}", STATE_KEYS[i]), "kind": kind.name() });
            let motion = match &draft.slots[i] {
                Slot::Keep { motion } | Slot::New { motion, .. } => *motion,
                Slot::Empty => None,
            };
            if let Some(m) = motion {
                st["motion"] = json!(m.name());
            }
            states.insert(STATE_KEYS[i].to_string(), st);
        }
    }
    let mut m = json!({ "schema": 1, "id": id, "name": draft.name.trim(), "scale": clamp_scale(draft.scale) });
    m["states"] = Value::Object(states);
    serde_json::to_string_pretty(&m).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Temp {
            let d = std::env::temp_dir().join(format!("buddy-lib-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Temp(d)
        }
        fn write(&self, rel: &str, data: &[u8]) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, data).unwrap();
            p
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn png_bytes() -> Vec<u8> {
        media::encode_png(&[9, 8, 7, 255], 1, 1).unwrap()
    }

    fn white_png() -> Vec<u8> {
        let mut px = vec![255u8; 16 * 16 * 4];
        px[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 3].copy_from_slice(&[1, 2, 3]);
        media::encode_png(&px, 16, 16).unwrap()
    }

    fn no_ids(_: &str) -> bool {
        false
    }

    fn draft_with(idle: &Path) -> Draft {
        let mut d = Draft { name: "Mèo Đen".into(), scale: 1.0, slots: Default::default() };
        d.slots[0] = Slot::New { path: idle.to_path_buf(), motion: None, remove_white: false };
        d
    }

    #[test]
    fn slugs_drop_vietnamese_marks() {
        assert_eq!(slugify("Mèo Đen"), "meo-den");
        assert_eq!(slugify("  Gấu   Bơ!! "), "gau-bo");
        assert_eq!(slugify("Me\u{300}o"), "meo");
        assert_eq!(slugify("Nguyễn Thị Ánh Tuyết 2"), "nguyen-thi-anh-tuyet-2");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn reads_the_current_schema_with_fallbacks_and_motion() {
        let t = Temp::new("schema");
        t.write("idle.png", &png_bytes());
        t.write("done.webm", b"x");
        let c = normalize(
            &json!({"id":"Bé Na","name":"Bé Na","scale":0.6,"unlock":{"sessions":5},
                "states":{"idle":{"file":"idle.png","kind":"static"},"done":{"file":"done.webm","kind":"video"}, "error":{"motion":"look"}}}),
            &t.0,
        )
        .unwrap();
        assert_eq!(c.id, "be-na");
        assert_eq!(c.scale, 0.6);
        assert_eq!(c.states[0].motion, Motion::Bob);
        assert!(!c.states[0].fallback);
        assert_eq!(c.states[3].kind, Kind::Video);
        assert_eq!(c.states[3].motion, Motion::None);
        assert!(c.states[1].fallback);
        assert_eq!(c.states[1].file, "idle.png");
        assert_eq!(c.states[1].motion, Motion::Type, "a state showing idle moves like its own default");
        assert_eq!(c.states[4].motion, Motion::Look);
        assert!(c.states[4].manual);
    }

    #[test]
    fn reads_the_old_flat_schema_and_bare_folders() {
        let t = Temp::new("flat");
        t.write("idle.gif", b"x");
        t.write("working.png", &png_bytes());
        let c = normalize(&json!({"name":"Cũ","states":{"idle":"idle.gif"},"motion":{"idle":"hop"}}), &t.0).unwrap();
        assert_eq!(c.states[0].motion, Motion::Hop);
        assert_eq!(c.states[1].file, "working.png");
        let bare = load_dir(&t.0).unwrap();
        assert_eq!(bare.states[0].kind, Kind::Animated);
    }

    #[test]
    fn manifests_cannot_point_outside_the_folder() {
        let t = Temp::new("escape");
        t.write("secret.png", &png_bytes());
        let inner = t.0.join("char");
        fs::create_dir_all(&inner).unwrap();
        for evil in ["../secret.png", "..\\secret.png", "C:\\Windows\\win.ini", "sub/idle.png"] {
            let r = normalize(&json!({"states":{"idle":{"file":evil}}}), &inner);
            assert!(r.is_err(), "{evil} should not be accepted");
        }
        fs::write(inner.join("notes.txt"), "x").unwrap();
        assert!(normalize(&json!({"states":{"idle":{"file":"notes.txt"}}}), &inner).is_err());
    }

    #[test]
    fn broken_values_fall_back_safely() {
        let t = Temp::new("values");
        t.write("idle.png", &png_bytes());
        for scale in [json!(0), json!(-1), json!(99), json!("big")] {
            assert_eq!(normalize(&json!({"scale":scale}), &t.0).unwrap().scale, 1.0);
        }
        assert!(normalize(&json!([1]), &t.0).is_err());
    }

    #[test]
    fn list_keeps_builtins_first_and_skips_broken_and_staging_folders() {
        let t = Temp::new("list");
        t.write("zeta/idle.png", &png_bytes());
        t.write("broken/character.json", b"{ nope");
        t.write(".creating-x/idle.png", &png_bytes());
        t.write("mochi/idle.png", &png_bytes()); // id collides with a built-in
        let l = load_all(&t.0);
        let ids: Vec<&str> = l.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["mochi", "meo-muop", "gau-bo", "zeta"]);
        assert!(l[0].is_builtin());
        assert_eq!(load_all(&t.0.join("missing")).len(), 3);
    }

    #[test]
    fn create_writes_a_loadable_character_renamed_by_state() {
        let t = Temp::new("create");
        let src = t.write("src/whatever-name.png", &png_bytes());
        let root = t.0.join("chars");
        let mut d = draft_with(&src);
        d.scale = 9.0;
        d.slots[3] = Slot::New { path: src.clone(), motion: Some(Motion::Hop), remove_white: false };
        let id = create(&d, &root, &no_ids).unwrap();
        assert_eq!(id, "meo-den");
        let c = load_dir(&root.join("meo-den")).unwrap();
        assert_eq!(c.name, "Mèo Đen");
        assert_eq!(c.scale, 1.5, "scale is clamped");
        assert_eq!(c.states[0].file, "idle.png");
        assert_eq!(c.states[3].file, "done.png");
        assert_eq!(c.states[3].motion, Motion::Hop);
        assert!(c.states[1].fallback);
        assert!(!root.join(".creating-meo-den").exists());
    }

    #[test]
    fn create_refuses_bad_input_and_leaves_nothing_behind() {
        let t = Temp::new("refuse");
        let good = t.write("a.png", &png_bytes());
        let junk = t.write("fake.png", b"this is not an image at all");
        let root = t.0.join("chars");

        let mut d = draft_with(&good);
        d.name = "   ".into();
        assert!(create(&d, &root, &no_ids).unwrap_err().contains("Tên"));
        d.name = "!!!".into();
        assert!(create(&d, &root, &no_ids).is_err());

        let mut d = draft_with(&good);
        d.slots[0] = Slot::Empty;
        assert!(create(&d, &root, &no_ids).unwrap_err().contains("idle"));

        let d = draft_with(&junk);
        assert!(create(&d, &root, &no_ids).unwrap_err().contains("fake.png"));

        let d = draft_with(&good);
        assert!(create(&d, &root, &|id| id == "meo-den").unwrap_err().contains("meo-den"));
        create(&d, &root, &no_ids).unwrap();
        assert!(create(&d, &root, &no_ids).is_err(), "folder already exists");
        let left: Vec<_> = fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, ["meo-den"]);
    }

    #[test]
    fn extension_comes_from_the_real_format() {
        let t = Temp::new("ext");
        let src = t.write("actually-png.jpg", &png_bytes());
        let root = t.0.join("chars");
        create(&draft_with(&src), &root, &no_ids).unwrap();
        assert!(root.join("meo-den/idle.png").exists());
    }

    #[test]
    fn motion_is_ignored_for_files_that_move_by_themselves() {
        let t = Temp::new("gifmotion");
        let frame: &[u8] = &[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 1, 0, 0];
        let gif = t.write("a.gif", &[b"GIF89a".as_slice(), &[1, 0, 1, 0, 0, 0, 0], frame, frame].concat());
        let p = plan_new(0, &gif, Some(Motion::Hop), false).unwrap();
        assert_eq!(p.kind, Kind::Animated);
        assert_eq!(p.motion, None);
    }

    #[test]
    fn white_background_removal_produces_a_transparent_png() {
        let t = Temp::new("white");
        let src = t.write("photo.png", &white_png());
        assert!(inspect_file(&src).unwrap().white_bg);
        let root = t.0.join("chars");
        let mut d = draft_with(&src);
        d.slots[0] = Slot::New { path: src, motion: None, remove_white: true };
        create(&d, &root, &no_ids).unwrap();
        let (px, _, _) = media::load_rgba(&root.join("meo-den/idle.png")).unwrap();
        assert_eq!(px[3], 0, "corner became transparent");
    }

    #[test]
    fn update_keeps_replaces_and_drops_files_and_unrelated_fields() {
        let t = Temp::new("update");
        let root = t.0.join("chars");
        let dir = root.join("na");
        t.write("chars/na/idle.png", &png_bytes());
        t.write("chars/na/done.png", &png_bytes());
        t.write("chars/na/waiting.png", &png_bytes());
        t.write("chars/na/sounds/done.wav", b"RIFF");
        fs::write(dir.join("character.json"), r#"{"id":"na","name":"Na","author":"Tôi","description":"Mô tả","motion":{"idle":"hop"},"unlock":{"sessions":3}}"#).unwrap();
        let c = load_dir(&dir).unwrap();
        let replacement = t.write("new.png", &white_png());
        let mut d = Draft { name: "Na Mới".into(), scale: 0.5, slots: Default::default() };
        d.slots[0] = Slot::Keep { motion: Some(Motion::Look) };
        d.slots[2] = Slot::New { path: replacement, motion: None, remove_white: false };
        // done is left Empty: dropped
        update(&c, &d, &root).unwrap();
        let after = load_dir(&dir).unwrap();
        assert_eq!(after.id, "na", "id never changes");
        assert_eq!(after.name, "Na Mới");
        assert_eq!(after.author, "Tôi");
        assert_eq!(after.description, "Mô tả");
        assert_eq!(after.states[0].motion, Motion::Look);
        assert!(after.states[3].fallback);
        assert!(!dir.join("done.png").exists());
        assert_eq!(fs::read(dir.join("waiting.png")).unwrap(), white_png());
        assert!(dir.join("sounds/done.wav").exists());
        assert!(after.sounds[0].is_some());
        let leftovers: Vec<_> = fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers, ["na"]);
    }

    #[test]
    fn a_failed_update_leaves_the_original_untouched() {
        let t = Temp::new("failupdate");
        let root = t.0.join("chars");
        let dir = root.join("na");
        t.write("chars/na/idle.png", &png_bytes());
        let c = load_dir(&dir).unwrap();
        let junk = t.write("junk.png", b"nope");
        let mut d = Draft { name: "X".into(), scale: 1.0, slots: Default::default() };
        d.slots[0] = Slot::New { path: junk, motion: None, remove_white: false };
        assert!(update(&c, &d, &root).is_err());
        assert_eq!(load_dir(&dir).unwrap().id, "na");
        assert_eq!(fs::read(dir.join("idle.png")).unwrap(), png_bytes());
        // Dropping idle isn't allowed either
        let d = Draft { name: "X".into(), scale: 1.0, slots: Default::default() };
        assert!(update(&c, &d, &root).is_err());
    }

    #[test]
    fn builtins_cannot_be_edited_or_removed() {
        let t = Temp::new("builtin");
        let c = vector_character(0);
        assert!(remove_blocker(&c, "other").is_some());
        assert!(update(&c, &Draft::default(), &t.0).is_err());
        assert!(remove_dir(&c, &t.0).is_err());
    }

    #[test]
    fn removal_rules() {
        let t = Temp::new("remove");
        let root = t.0.join("chars");
        t.write("chars/na/idle.png", &png_bytes());
        let c = load_dir(&root.join("na")).unwrap();
        assert!(remove_blocker(&c, "na").unwrap().contains("Đang dùng"));
        assert_eq!(remove_blocker(&c, "other"), None);
        remove_dir(&c, &root).unwrap();
        assert!(!root.join("na").exists());
    }

    #[test]
    fn removal_refuses_folders_outside_the_user_root() {
        let t = Temp::new("removeout");
        let root = t.0.join("chars");
        fs::create_dir_all(&root).unwrap();
        t.write("elsewhere/idle.png", &png_bytes());
        let c = load_dir(&t.0.join("elsewhere")).unwrap();
        assert!(remove_dir(&c, &root).is_err());
        assert!(t.0.join("elsewhere/idle.png").exists());
    }

    #[test]
    fn import_copies_under_a_free_id_and_keeps_the_source() {
        let t = Temp::new("import");
        t.write("src/idle.png", &png_bytes());
        t.write("src/thumb.png", &png_bytes());
        t.write("src/sounds/done.wav", b"RIFF");
        t.write("src/character.json", br#"{"schema":1,"id":"na","name":"Na","unlock":{"sessions":9},"states":{"idle":{"file":"idle.png"}}}"#);
        let root = t.0.join("chars");
        assert_eq!(import_dir(&t.0.join("src"), &root, &no_ids).unwrap(), "na");
        assert_eq!(import_dir(&t.0.join("src"), &root, &no_ids).unwrap(), "na-2");
        assert_eq!(import_dir(&t.0.join("src"), &root, &|id| id == "na-3").unwrap(), "na-4");
        let c = load_dir(&root.join("na-2")).unwrap();
        assert_eq!(c.id, "na-2");
        assert!(c.sounds[0].is_some());
        assert!(!root.join("na/thumb.png").exists(), "thumbnails are made again here");
        assert!(t.0.join("src/idle.png").exists());
    }

    #[test]
    fn import_handles_a_folder_without_a_manifest_and_refuses_unusable_ones() {
        let t = Temp::new("importbare");
        t.write("Bé Na/idle.png", &png_bytes());
        t.write("empty/readme.txt", b"x");
        let root = t.0.join("chars");
        assert_eq!(import_dir(&t.0.join("Bé Na"), &root, &no_ids).unwrap(), "be-na");
        assert_eq!(load_dir(&root.join("be-na")).unwrap().name, "be-na");
        assert!(import_dir(&t.0.join("empty"), &root, &no_ids).is_err());
        assert!(!root.join(".importing-empty").exists());
    }

    #[test]
    fn inspect_explains_rejections() {
        let t = Temp::new("inspect");
        let p = t.write("a.txt", b"hello");
        assert!(inspect_file(&p).unwrap_err().contains("a.txt"));
        assert!(inspect_file(&t.0).is_err());
        assert!(inspect_file(&t.0.join("missing.png")).is_err());
        let big = t.0.join("big.png");
        let f = fs::File::create(&big).unwrap();
        f.set_len(MAX_MEDIA_BYTES + 1).unwrap();
        drop(f);
        assert!(inspect_file(&big).unwrap_err().contains("50 MB"));
    }

    #[test]
    fn guesses_states_from_names() {
        assert_eq!(guess_state("Cat_Idle.webm"), Some(0));
        assert_eq!(guess_state("working-2.gif"), Some(1));
        assert_eq!(guess_state("WAITING.png"), Some(2));
        assert_eq!(guess_state("finish.png"), Some(3));
        assert_eq!(guess_state("sad_cat.png"), Some(4));
        assert_eq!(guess_state("cat.png"), None);
    }

    #[test]
    fn manifest_preview_matches_what_gets_written() {
        let mut d = Draft { name: " Na ".into(), scale: 1.2, slots: Default::default() };
        d.slots[0] = Slot::New { path: "x.png".into(), motion: Some(Motion::Hop), remove_white: true };
        let mut exts: [Option<String>; 5] = Default::default();
        let mut kinds: [Option<Kind>; 5] = Default::default();
        exts[0] = Some(".png".into());
        kinds[0] = Some(Kind::Static);
        let j: Value = serde_json::from_str(&manifest_preview(&d, "na", &exts, &kinds)).unwrap();
        assert_eq!(j["name"], "Na");
        assert!(j.get("unlock").is_none());
        assert_eq!(j["states"]["idle"]["file"], "idle.png");
        assert_eq!(j["states"]["idle"]["motion"], "hop");
        assert!(j["states"].get("done").is_none());
    }
}
