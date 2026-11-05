//! The picker's connection to Windows: its window, text boxes and standard dialogs.

use crate::i18n::t;
use crate::library;
use crate::picker;
use crate::theme::{self, pal, Backdrop};
use crate::window::{self, Canvas};
use std::path::{Path, PathBuf};
use windows_sys::core::w;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub type Win = HWND;

const WM_JOB_DONE: u32 = WM_APP + 20;
const WM_REFRESH: u32 = WM_APP + 21;
const WM_MOUSELEAVE: u32 = 0x02A3;
const EN_CHANGE: u32 = 0x0300;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ID_NAME: usize = 201;
const ID_PROMPT: usize = 202;
const CF_UNICODETEXT: u32 = 13;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_WANTRETURN: u32 = 0x1000;
const TIMER_PREVIEW: usize = 1;
const MIN_WINDOW_H: f32 = 760.0;

/// What stays with the window and goes away with it.
pub struct Surface {
    edit_font: HFONT,
    edit_brush: HBRUSH,
}

pub struct Made {
    pub hwnd: Win,
    pub surface: Surface,
    pub canvas: Canvas,
    pub backdrop: Backdrop,
    pub scale: f32,
    pub edit_name: Edit,
    pub edit_prompt: Edit,
}

/// A real text box, so typing methods and shortcuts behave normally.
pub struct Edit(HWND);

impl Edit {
    pub fn text(&self) -> String {
        unsafe {
            let n = GetWindowTextLengthW(self.0);
            let mut b = vec![0u16; n as usize + 1];
            let got = GetWindowTextW(self.0, b.as_mut_ptr(), b.len() as i32);
            String::from_utf16_lossy(&b[..got.max(0) as usize])
        }
    }

    pub fn set_text(&self, s: &str) {
        unsafe { SetWindowTextW(self.0, wide(s).as_ptr()) };
    }

    pub fn focus(&self) {
        unsafe { SetFocus(self.0) };
    }

    pub fn is_focused(&self) -> bool {
        unsafe { GetFocus() == self.0 }
    }

    /// In pixels of the window; None hides it.
    pub fn place(&self, rect: Option<(i32, i32, i32, i32)>) {
        unsafe {
            match rect {
                Some((x, y, w, h)) => {
                    SetWindowPos(self.0, std::ptr::null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                }
                None => {
                    ShowWindow(self.0, SW_HIDE);
                }
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn colorref(rgb: u32) -> u32 {
    (rgb & 0xff) << 16 | (rgb & 0xff00) | (rgb >> 16 & 0xff)
}

pub fn open_folder(dir: &Path) {
    let d = wide(&dir.display().to_string());
    unsafe { ShellExecuteW(std::ptr::null_mut(), w!("open"), d.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}

/// The text box keeps keys to itself, so the app's message loop hands Esc over from there.
pub fn forward_escape(target: HWND) -> bool {
    let open = picker::window();
    if open.is_null() || unsafe { GetParent(target) } != open {
        return false;
    }
    unsafe { PostMessageW(open, WM_KEYDOWN, 0x1B, 0) };
    true
}

pub fn post_job(hwnd: isize) {
    unsafe { PostMessageW(hwnd as HWND, WM_JOB_DONE, 0, 0) };
}

pub fn post_refresh(hwnd: isize) {
    unsafe { PostMessageW(hwnd as HWND, WM_REFRESH, 0, 0) };
}

pub fn invalidate(hwnd: Win) {
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

pub fn bring_forward(h: Win) {
    unsafe {
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        SetForegroundWindow(h);
    }
}

pub fn create_window() -> Option<Made> {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = w!("ClaudeBuddyRSPicker");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class;
        wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        wc.hIcon = LoadIconW(instance, 1 as *const u16);
        RegisterClassW(&wc); // already registered on later opens: harmless

        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class,
            w!("Claude Buddy"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return None;
        }
        let backdrop = theme::apply_backdrop(hwnd);
        let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
        let (w, h) = ((1100.0 * scale) as u32, (820.0 * scale) as u32);
        let Some(canvas) = Canvas::new(w.max(1), h.max(1)) else {
            DestroyWindow(hwnd);
            return None;
        };
        let face = w!("Segoe UI");
        let edit_font = CreateFontW(-(14.0 * scale).round() as i32, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face);
        let make_edit = |id: usize, style: u32| {
            let e = CreateWindowExW(
                0, // the border is drawn with the rest of the page, in the Windows 11 style
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_TABSTOP | style,
                0,
                0,
                10,
                10,
                hwnd,
                id as HMENU,
                instance,
                std::ptr::null(),
            );
            SendMessageW(e, WM_SETFONT, edit_font as WPARAM, 1);
            // GDI leaves alpha at zero where it draws, which over Mica makes the typed text vanish. A
            // layered child is composited as fully opaque instead.
            SetWindowLongPtrW(e, GWL_EXSTYLE, GetWindowLongPtrW(e, GWL_EXSTYLE) | WS_EX_LAYERED as isize);
            SetLayeredWindowAttributes(e, 0, 255, LWA_ALPHA);
            e
        };
        let edit_name = make_edit(ID_NAME, ES_AUTOHSCROLL);
        SendMessageW(edit_name, 0x00C5 /* EM_LIMITTEXT */, 60, 0);
        let edit_prompt = make_edit(ID_PROMPT, ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | WS_VSCROLL);
        SendMessageW(edit_prompt, 0x00C5 /* EM_LIMITTEXT */, crate::codex::MAX_DESCRIPTION, 0);
        Some(Made {
            hwnd,
            surface: Surface { edit_font, edit_brush: CreateSolidBrush(colorref(pal().surface)) },
            canvas,
            backdrop,
            scale,
            edit_name: Edit(edit_name),
            edit_prompt: Edit(edit_prompt),
        })
    }
}

pub fn show_window(hwnd: Win, (w, h): (u32, u32), frame_ms: u32) {
    unsafe {
        // Centered in the space the taskbar leaves, on the screen you are working on.
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST), &mut mi);
        let a = mi.rcWork;
        let (fw, fh) = ((w as i32).min(a.right - a.left), (h as i32).min(a.bottom - a.top));
        SetWindowPos(hwnd, std::ptr::null_mut(), a.left + (a.right - a.left - fw) / 2, a.top + (a.bottom - a.top - fh) / 2, fw, fh, SWP_NOZORDER);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetTimer(hwnd, TIMER_PREVIEW, frame_ms, None);
    }
}

/// A menu at the pointer. Returns the chosen item's position.
pub fn popup_choice(owner: Win, items: &[(String, bool)]) -> Option<usize> {
    unsafe {
        let m = CreatePopupMenu();
        for (i, (label, checked)) in items.iter().enumerate() {
            AppendMenuW(m, MF_STRING | if *checked { MF_CHECKED } else { MF_UNCHECKED }, i + 1, wide(label).as_ptr());
        }
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, 0, owner, std::ptr::null());
        DestroyMenu(m);
        (cmd != 0).then(|| cmd as usize - 1)
    }
}

pub fn pick_files(owner: HWND, multi: bool) -> Vec<PathBuf> {
    let filter: Vec<u16> = t("Ảnh và video\0*.webm;*.mp4;*.png;*.apng;*.gif;*.webp;*.svg;*.jpg;*.jpeg\0Mọi file\0*.*\0\0", "Images and videos\0*.webm;*.mp4;*.png;*.apng;*.gif;*.webp;*.svg;*.jpg;*.jpeg\0All files\0*.*\0\0").encode_utf16().collect();
    let mut buffer = vec![0u16; 32 * 1024];
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buffer.as_mut_ptr();
    ofn.nMaxFile = buffer.len() as u32;
    ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR | OFN_HIDEREADONLY | if multi { OFN_ALLOWMULTISELECT } else { 0 };
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return Vec::new();
    }
    // One file: its full path. Several: the folder, then each name, zero-separated.
    let parts: Vec<String> = buffer.split(|c| *c == 0).take_while(|p| !p.is_empty()).map(String::from_utf16_lossy).collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [one] => vec![PathBuf::from(one)],
        [dir, names @ ..] => names.iter().map(|n| Path::new(dir).join(n)).collect(),
    }
}

pub fn copy_text(owner: HWND, text: &str) -> bool {
    let wide_text = wide(text);
    let bytes = wide_text.len() * 2;
    unsafe {
        if OpenClipboard(owner) == 0 {
            return false;
        }
        EmptyClipboard();
        let block = GlobalAlloc(GMEM_MOVEABLE, bytes);
        let mut ok = false;
        if !block.is_null() {
            let target = GlobalLock(block) as *mut u16;
            if !target.is_null() {
                std::ptr::copy_nonoverlapping(wide_text.as_ptr(), target, wide_text.len());
                GlobalUnlock(block);
                // The clipboard owns the memory once this succeeds.
                ok = !SetClipboardData(CF_UNICODETEXT, block as _).is_null();
            }
            if !ok {
                GlobalFree(block);
            }
        }
        CloseClipboard();
        ok
    }
}

pub fn pick_save_file(owner: HWND, suggested: &str) -> Option<PathBuf> {
    let filter: Vec<u16> = t("Nhân vật Claude Buddy\0*.buddy\0\0", "Claude Buddy character\0*.buddy\0\0").encode_utf16().collect();
    let extension = wide(library::PACK_EXTENSION);
    let mut buffer: Vec<u16> = suggested.encode_utf16().collect();
    buffer.resize(4096, 0);
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buffer.as_mut_ptr();
    ofn.nMaxFile = buffer.len() as u32;
    ofn.lpstrDefExt = extension.as_ptr();
    ofn.Flags = OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_NOCHANGEDIR | OFN_HIDEREADONLY;
    if unsafe { GetSaveFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buffer[..end])))
}

pub fn pick_folder(owner: HWND) -> Option<PathBuf> {
    let title = wide(t("Chọn thư mục nhân vật", "Choose a character folder"));
    let mut info: BROWSEINFOW = unsafe { std::mem::zeroed() };
    info.hwndOwner = owner;
    info.lpszTitle = title.as_ptr();
    info.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
    unsafe {
        let list = SHBrowseForFolderW(&info);
        if list.is_null() {
            return None;
        }
        let mut path = vec![0u16; 1024];
        let ok = SHGetPathFromIDListW(list, path.as_mut_ptr());
        windows_sys::Win32::System::Com::CoTaskMemFree(list as *const _);
        if ok == 0 {
            return None;
        }
        let n = path.iter().position(|c| *c == 0).unwrap_or(path.len());
        Some(PathBuf::from(String::from_utf16_lossy(&path[..n])))
    }
}

pub fn confirm(owner: HWND, title: &str, message: &str) -> bool {
    let (t, m) = (wide(title), wide(message));
    unsafe { MessageBoxW(owner, m.as_ptr(), t.as_ptr(), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) == IDYES }
}

pub fn show_error(owner: HWND, title: &str, message: &str) {
    let (t, m) = (wide(title), wide(message));
    unsafe { MessageBoxW(owner, m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONERROR) };
}

fn dropped_files(drop: HDROP) -> (Vec<PathBuf>, POINT) {
    let mut out = Vec::new();
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        DragQueryPoint(drop, &mut pt);
        let n = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
        for i in 0..n {
            let len = DragQueryFileW(drop, i, std::ptr::null_mut(), 0) as usize;
            let mut b = vec![0u16; len + 1];
            DragQueryFileW(drop, i, b.as_mut_ptr(), b.len() as u32);
            out.push(PathBuf::from(String::from_utf16_lossy(&b[..len])));
        }
        DragFinish(drop);
    }
    (out, pt)
}

fn lparam_point(l: LPARAM, k: f32) -> (f32, f32) {
    (((l & 0xffff) as i16 as f32) / k, (((l >> 16) & 0xffff) as i16 as f32) / k)
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_ACTIVATE => {
            // While you use this window the buddy must not sit on top of its buttons.
            window::sync_buddy_with_picker();
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            picker::paint_into(|c| {
                BitBlt(hdc, 0, 0, c.w as i32, c.h as i32, c.dc, 0, 0, SRCCOPY);
            });
            EndPaint(hwnd, &ps);
            0
        }
        WM_SIZE => {
            picker::resized((lp & 0xffff) as u32, ((lp >> 16) & 0xffff) as u32);
            0
        }
        WM_GETMINMAXINFO => {
            let k = GetDpiForWindow(hwnd) as f32 / 96.0;
            let info = &mut *(lp as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT { x: (picker::MIN_WINDOW_W * k) as i32, y: (MIN_WINDOW_H * k) as i32 };
            0
        }
        WM_DPICHANGED => {
            let r = &*(lp as *const RECT);
            picker::scale_changed((wp & 0xffff) as f32 / 96.0);
            SetWindowPos(hwnd, std::ptr::null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            0
        }
        WM_TIMER => {
            if wp == TIMER_PREVIEW {
                if let Some((x, y, sw, sh)) = picker::timer_tick(IsIconic(hwnd) != 0) {
                    picker::with_canvas(|c| {
                        let dc = GetDC(hwnd);
                        BitBlt(dc, x, y, sw, sh, c.dc, x, y, SRCCOPY);
                        ReleaseDC(hwnd, dc);
                    });
                }
            }
            0
        }
        WM_CTLCOLOREDIT => {
            let hdc = wp as HDC;
            SetTextColor(hdc, colorref(pal().ink));
            SetBkColor(hdc, colorref(pal().surface));
            picker::surface(|s| s.edit_brush as LRESULT).unwrap_or(0)
        }
        WM_SETTINGCHANGE => {
            theme::reload();
            picker::surface(|s| {
                DeleteObject(s.edit_brush);
                s.edit_brush = CreateSolidBrush(colorref(pal().surface));
            });
            picker::theme_changed();
            0
        }
        WM_JOB_DONE => {
            picker::jobs_done();
            0
        }
        WM_REFRESH => {
            picker::refresh_requested();
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_point(lp, picker::current_scale());
            if picker::mouse_moved(x, y) {
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                TrackMouseEvent(&mut t);
            }
            0
        }
        WM_MOUSELEAVE => {
            picker::mouse_left();
            0
        }
        WM_LBUTTONDOWN => {
            // Clicking away from a text box takes the focus back, so Esc and typing go to the right place.
            SetFocus(hwnd);
            let (x, y) = lparam_point(lp, picker::current_scale());
            if picker::left_down(x, y) {
                SetCapture(hwnd);
            }
            0
        }
        WM_LBUTTONUP => {
            if picker::end_drag() {
                ReleaseCapture();
                return 0;
            }
            let (x, y) = lparam_point(lp, picker::current_scale());
            picker::left_up(hwnd, x, y);
            0
        }
        WM_MOUSEWHEEL => {
            picker::wheel(((wp >> 16) & 0xffff) as i16 as f32 / 120.0);
            0
        }
        WM_COMMAND => {
            let code = ((wp >> 16) & 0xffff) as u32;
            let id = wp & 0xffff;
            if code == EN_CHANGE && id == ID_PROMPT {
                picker::prompt_changed();
            }
            if code == EN_CHANGE && id == ID_NAME {
                picker::name_changed();
            }
            0
        }
        WM_DROPFILES => {
            let (files, pt) = dropped_files(wp as HDROP);
            let k = picker::current_scale();
            picker::files_dropped(files, pt.x as f32 / k, pt.y as f32 / k);
            0
        }
        WM_KEYDOWN if wp == 0x1B => {
            if !picker::escape() {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_CLOSE => {
            if picker::may_close(hwnd) {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            if let Some(s) = picker::closed() {
                KillTimer(hwnd, TIMER_PREVIEW);
                DeleteObject(s.edit_font);
                DeleteObject(s.edit_brush);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
