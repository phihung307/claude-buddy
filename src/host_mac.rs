//! The picker's connection to macOS: its window, text boxes and standard dialogs.

use crate::i18n::t;
use crate::library;
use crate::picker;
use crate::theme::{self, pal, Backdrop};
use crate::window::Canvas;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSBackingStoreType, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSDragOperation, NSDraggingInfo, NSEvent, NSFocusRingType, NSFont,
    NSMenu, NSMenuItem, NSModalResponseOK, NSOpenPanel, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeString, NSSavePanel, NSTextField, NSTextView, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSViewLayerContentsRedrawPolicy, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_core_foundation::{CFData, CFRetained, CGFloat};
use objc2_core_graphics::{CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};
use objc2_foundation::{NSArray, NSNotification, NSPoint, NSRect, NSSize, NSString, NSTimer, NSURL};
use objc2_quartz_core::{CALayer, CATransaction};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

pub type Win = *mut core::ffi::c_void;

const MIN_H: f64 = 620.0;
const NAME_LIMIT: usize = 60;
const KEY_ESCAPE: u16 = 53;
/// Points of a trackpad's scrolling that count as one click of a wheel.
const POINTS_PER_NOTCH: f64 = 40.0;

static REFRESH: AtomicBool = AtomicBool::new(false);

thread_local! {
    static POPUP_CHOICE: Cell<Option<usize>> = const { Cell::new(None) };
    /// The animated part of the window, kept in a layer of its own so a frame doesn't copy the whole page.
    static STAGE: RefCell<Option<Retained<CALayer>>> = const { RefCell::new(None) };
    static OPEN_VIEW: RefCell<Option<Retained<MainView>>> = const { RefCell::new(None) };
}

/// A copy of part of a BGRA buffer, because the buffer keeps being written while the image is on screen.
pub fn image_of(pixels: &[u8], x: usize, y: usize, w: usize, h: usize, stride_px: usize) -> Option<CFRetained<CGImage>> {
    if w == 0 || h == 0 {
        return None;
    }
    let mut part = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        let s = (row * stride_px + x) * 4;
        part.extend_from_slice(pixels.get(s..s + w * 4)?);
    }
    let data = CFData::from_bytes(&part);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;
    let info = CGBitmapInfo(CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0);
    unsafe { CGImage::new(w, h, 8, 32, w * 4, Some(&space), info, Some(&provider), std::ptr::null(), false, CGColorRenderingIntent::RenderingIntentDefault) }
}

fn set_layer_image(layer: &CALayer, image: Option<&CGImage>) {
    let contents = image.map(|i| unsafe { &*(i as *const CGImage as *const AnyObject) });
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    // A layer takes a CGImage as its contents, through a property typed as any object.
    unsafe { layer.setContents(contents) };
    CATransaction::commit();
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClaudeBuddyMainView"]
    struct MainView;

    unsafe impl NSObjectProtocol for MainView {}

    unsafe impl NSWindowDelegate for MainView {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            picker::may_close(self.handle())
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _note: &NSNotification) {
            if let Some(timer) = TIMER.with(|t| t.borrow_mut().take()) {
                timer.invalidate();
            }
            let _ = picker::closed();
            if let Some(window) = self.window() {
                window.setDelegate(None);
            }
            OPEN_VIEW.with(|v| v.borrow_mut().take());
            STAGE.with(|s| s.borrow_mut().take());
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn became_key(&self, _note: &NSNotification) {
            crate::window::sync_buddy_with_picker();
        }

        #[unsafe(method(windowDidResignKey:))]
        fn resigned_key(&self, _note: &NSNotification) {
            crate::window::sync_buddy_with_picker();
        }
    }

    impl MainView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            let Some(layer) = self.layer() else { return };
            picker::paint_into(|c| {
                if let Some(image) = c.image() {
                    set_layer_image(&layer, Some(&image));
                }
            });
            // The page just drawn already holds the latest frame.
            STAGE.with(|s| {
                if let Some(stage) = &*s.borrow() {
                    set_layer_image(stage, None);
                }
            });
            place_field_colors(self);
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            let k = self.scale();
            picker::resized((size.width * k).round() as u32, (size.height * k).round() as u32);
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_changed(&self) {
            let Some(window) = self.window() else { return };
            let k = window.backingScaleFactor();
            if (k as f32 - picker::current_scale()).abs() > 0.001 {
                picker::scale_changed(k as f32);
                let size = self.frame().size;
                picker::resized((size.width * k).round() as u32, (size.height * k).round() as u32);
            }
        }

        #[unsafe(method(pickerTick:))]
        fn tick(&self, _timer: &NSTimer) {
            // Worker threads only leave their results; this is where they are picked up.
            picker::jobs_done();
            if REFRESH.swap(false, Ordering::AcqRel) {
                picker::refresh_requested();
            }
            let minimized = self.window().is_some_and(|w| w.isMiniaturized());
            if let Some((x, y, w, h)) = picker::timer_tick(minimized) {
                let k = self.scale();
                let page_h = self.bounds().size.height;
                picker::with_canvas(|c| {
                    let Some(image) = c.image_of_part(x, y, w, h) else { return };
                    STAGE.with(|s| {
                        let Some(stage) = &*s.borrow() else { return };
                        CATransaction::begin();
                        CATransaction::setDisableActions(true);
                        // Layers are measured from the bottom left, the canvas from the top left.
                        stage.setFrame(NSRect::new(NSPoint::new(x as f64 / k, page_h - (y + h) as f64 / k), NSSize::new(w as f64 / k, h as f64 / k)));
                        CATransaction::commit();
                        set_layer_image(stage, Some(&image));
                    });
                });
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let (x, y) = self.logical(event);
            picker::mouse_moved(x, y);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            picker::mouse_left();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            // Clicking away from a text box takes the focus back, so Esc and typing go to the right place.
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self));
            }
            let (x, y) = self.logical(event);
            picker::left_down(x, y);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let (x, y) = self.logical(event);
            picker::mouse_moved(x, y);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if picker::end_drag() {
                return;
            }
            let (x, y) = self.logical(event);
            picker::left_up(self.handle(), x, y);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let d = event.scrollingDeltaY();
            picker::wheel(if event.hasPreciseScrollingDeltas() { d / POINTS_PER_NOTCH } else { d } as f32);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if event.keyCode() == KEY_ESCAPE {
                self.escape_or_close();
            } else {
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            self.escape_or_close();
        }

        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _note: &NSNotification) {
            picker::prompt_changed();
            picker::name_changed();
        }

        /// In the description box, Return starts a new line, and Esc there still means what it means elsewhere.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, control: &NSTextField, text: &NSTextView, command: Sel) -> bool {
            if command == sel!(insertNewline:) && !control.usesSingleLineMode() {
                let _: () = unsafe { msg_send![text, insertNewlineIgnoringFieldEditor: None::<&AnyObject>] };
                true
            } else if command == sel!(cancelOperation:) {
                self.escape_or_close();
                true
            } else {
                false
            }
        }

        #[unsafe(method(popupChose:))]
        fn popup_chose(&self, item: &NSMenuItem) {
            POPUP_CHOICE.with(|c| c.set(Some(item.tag() as usize)));
        }

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let board = info.draggingPasteboard();
            let files = files_on(&board);
            let at = self.convertPoint_fromView(info.draggingLocation(), None);
            picker::files_dropped(files, at.x as f32, (self.bounds().size.height - at.y) as f32);
            true
        }
    }
);

thread_local! {
    static TIMER: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
}

impl MainView {
    fn handle(&self) -> Win {
        self as *const MainView as Win
    }

    fn escape_or_close(&self) {
        if !picker::escape() {
            if let Some(window) = self.window() {
                window.performClose(None);
            }
        }
    }

    fn scale(&self) -> f64 {
        self.window().map_or(1.0, |w| w.backingScaleFactor())
    }

    /// Points from the top left, the way the page is laid out.
    fn logical(&self, event: &NSEvent) -> (f32, f32) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        (p.x as f32, (self.bounds().size.height - p.y) as f32)
    }
}

fn files_on(board: &NSPasteboard) -> Vec<PathBuf> {
    let Some(items) = board.pasteboardItems() else { return Vec::new() };
    let kind = unsafe { NSPasteboardTypeFileURL };
    items
        .iter()
        .filter_map(|item| item.stringForType(kind))
        .filter_map(|u| NSURL::URLWithString(&u))
        .filter_map(|u| u.path())
        .map(|p| PathBuf::from(p.to_string()))
        .collect()
}

/// A text box follows the theme along with the page around it.
fn place_field_colors(view: &MainView) {
    let ink = pal().ink;
    let color = NSColor::colorWithSRGBRed_green_blue_alpha((ink >> 16 & 0xff) as CGFloat / 255.0, (ink >> 8 & 0xff) as CGFloat / 255.0, (ink & 0xff) as CGFloat / 255.0, 1.0);
    for sub in view.subviews().iter() {
        if let Ok(field) = sub.downcast::<NSTextField>() {
            field.setTextColor(Some(&color));
        }
    }
}

/// What stays with the window and goes away with it.
pub struct Surface {
    _window: Retained<NSWindow>,
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
pub struct Edit {
    field: Retained<NSTextField>,
    limit: usize,
}

impl Edit {
    fn new(mtm: MainThreadMarker, view: &MainView, multi_line: bool, limit: usize) -> Edit {
        let field = NSTextField::new(mtm);
        field.setBezeled(false);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setEditable(true);
        field.setSelectable(true);
        field.setFocusRingType(NSFocusRingType::None);
        field.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        field.setHidden(true);
        field.setUsesSingleLineMode(!multi_line);
        if let Some(cell) = field.cell() {
            cell.setWraps(multi_line);
            cell.setScrollable(!multi_line);
        }
        let delegate: &AnyObject = view;
        let _: () = unsafe { msg_send![&*field, setDelegate: delegate] };
        view.addSubview(&field);
        Edit { field, limit }
    }

    pub fn text(&self) -> String {
        self.field.stringValue().to_string().chars().take(self.limit).collect()
    }

    pub fn set_text(&self, s: &str) {
        self.field.setStringValue(&NSString::from_str(s));
    }

    pub fn focus(&self) {
        if let Some(window) = self.field.window() {
            window.makeFirstResponder(Some(&self.field));
        }
    }

    pub fn is_focused(&self) -> bool {
        self.field.currentEditor().is_some()
    }

    /// In pixels of the window from its top left; None hides it.
    pub fn place(&self, rect: Option<(i32, i32, i32, i32)>) {
        let (Some((x, y, w, h)), Some(window), Some(parent)) = (rect, self.field.window(), unsafe { self.field.superview() }) else {
            self.field.setHidden(true);
            return;
        };
        let k = window.backingScaleFactor();
        let bottom = parent.bounds().size.height - (y + h) as f64 / k;
        self.field.setFrame(NSRect::new(NSPoint::new(x as f64 / k, bottom), NSSize::new(w as f64 / k, h as f64 / k)));
        self.field.setHidden(false);
    }
}

pub fn open_path(path: &str) {
    let _ = Command::new("open").arg(path).spawn();
}

pub fn open_folder(dir: &Path) {
    let _ = Command::new("open").arg(dir).spawn();
}

/// Results wait in a list that the window's regular beat looks at.
pub fn post_job(_hwnd: isize) {}

pub fn post_refresh(_hwnd: isize) {
    REFRESH.store(true, Ordering::Release);
}

pub fn invalidate(hwnd: Win) {
    OPEN_VIEW.with(|v| {
        if let Some(view) = v.borrow().as_ref().filter(|view| view.handle() == hwnd) {
            view.setNeedsDisplay(true);
        }
    });
}

pub fn bring_forward(hwnd: Win) {
    OPEN_VIEW.with(|v| {
        if let Some(view) = v.borrow().as_ref().filter(|view| view.handle() == hwnd) {
            if let Some(window) = view.window() {
                window.deminiaturize(None);
                window.makeKeyAndOrderFront(None);
            }
            NSApplication::sharedApplication(view.mtm()).activate();
        }
    });
}

pub fn is_key(hwnd: Win) -> bool {
    OPEN_VIEW.with(|v| v.borrow().as_ref().filter(|view| view.handle() == hwnd).and_then(|view| view.window()).is_some_and(|w| w.isKeyWindow()))
}

pub fn create_window() -> Option<Made> {
    let mtm = MainThreadMarker::new()?;
    let screen = objc2_app_kit::NSScreen::mainScreen(mtm)?;
    let area = screen.visibleFrame();
    let size = NSSize::new(1100f64.min(area.size.width), 820f64.min(area.size.height));
    let frame = NSRect::new(NSPoint::new(area.origin.x + (area.size.width - size.width) / 2.0, area.origin.y + (area.size.height - size.height) / 2.0), size);
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::Resizable;
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), frame, style, NSBackingStoreType::Buffered, false) };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Claude Buddy"));
    window.setContentMinSize(NSSize::new((picker::MIN_WINDOW_W as f64).min(area.size.width), MIN_H.min(area.size.height)));
    window.setAcceptsMouseMovedEvents(true);

    let view: Retained<MainView> = unsafe { msg_send![MainView::alloc(mtm), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), size)] };
    view.setWantsLayer(true);
    view.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::OnSetNeedsDisplay);
    window.setContentView(Some(&view));
    window.setDelegate(Some(ProtocolObject::from_ref(&*view)));
    view.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
    let area_options = NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::ActiveInKeyWindow | NSTrackingAreaOptions::InVisibleRect;
    let tracking = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), NSRect::new(NSPoint::new(0.0, 0.0), size), area_options, Some(&view), None) };
    view.addTrackingArea(&tracking);

    let stage = CALayer::new();
    stage.setZPosition(1.0);
    if let Some(layer) = view.layer() {
        layer.addSublayer(&stage);
    }
    STAGE.with(|s| *s.borrow_mut() = Some(stage));

    let scale = window.backingScaleFactor() as f32;
    let canvas = Canvas::new((size.width * scale as f64) as u32, (size.height * scale as f64) as u32)?;
    let backdrop = theme::apply_backdrop(view.handle());
    let edit_name = Edit::new(mtm, &view, false, NAME_LIMIT);
    let edit_prompt = Edit::new(mtm, &view, true, crate::codex::MAX_DESCRIPTION);
    let hwnd = view.handle();
    OPEN_VIEW.with(|v| *v.borrow_mut() = Some(view));
    Some(Made { hwnd, surface: Surface { _window: window }, canvas, backdrop, scale, edit_name, edit_prompt })
}

pub fn show_window(hwnd: Win, _size: (u32, u32), frame_ms: u32) {
    OPEN_VIEW.with(|v| {
        let binding = v.borrow();
        let Some(view) = binding.as_ref().filter(|view| view.handle() == hwnd) else { return };
        if let Some(window) = view.window() {
            window.makeKeyAndOrderFront(None);
            window.makeFirstResponder(Some(&**view));
        }
        NSApplication::sharedApplication(view.mtm()).activate();
        let target: &AnyObject = view;
        let timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(frame_ms as f64 / 1000.0, target, sel!(pickerTick:), None, true) };
        TIMER.with(|t| *t.borrow_mut() = Some(timer));
        view.setNeedsDisplay(true);
    });
}

/// A menu at the pointer. Returns the chosen item's position.
pub fn popup_choice(owner: Win, items: &[(String, bool)]) -> Option<usize> {
    let mtm = MainThreadMarker::new()?;
    OPEN_VIEW.with(|v| {
        let binding = v.borrow();
        let view = binding.as_ref().filter(|view| view.handle() == owner)?;
        let menu = NSMenu::new(mtm);
        let target: &AnyObject = view;
        for (i, (label, checked)) in items.iter().enumerate() {
            let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(label), Some(sel!(popupChose:)), &NSString::new()) };
            unsafe { item.setTarget(Some(target)) };
            item.setTag(i as isize);
            item.setState(if *checked { NSControlStateValueOn } else { NSControlStateValueOff });
            menu.addItem(&item);
        }
        POPUP_CHOICE.with(|c| c.set(None));
        let at = view.window().map_or(NSPoint::new(0.0, 0.0), |w| view.convertPoint_fromView(w.mouseLocationOutsideOfEventStream(), None));
        menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(&**view));
        POPUP_CHOICE.with(|c| c.get())
    })
}

fn alert(title: &str, message: &str, buttons: &[&str]) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let a = NSAlert::new(mtm);
    a.setMessageText(&NSString::from_str(title));
    a.setInformativeText(&NSString::from_str(message));
    for b in buttons {
        a.addButtonWithTitle(&NSString::from_str(b));
    }
    NSApplication::sharedApplication(mtm).activate();
    a.runModal() == NSAlertFirstButtonReturn
}

pub fn confirm(_owner: Win, title: &str, message: &str) -> bool {
    // Same as on Windows: what can't be undone is never the default.
    !alert(title, message, &[t("Không", "No"), t("Có", "Yes")])
}

pub fn show_error(_owner: Win, title: &str, message: &str) {
    alert(title, message, &["OK"]);
}

fn paths_of(urls: &NSArray<NSURL>) -> Vec<PathBuf> {
    urls.iter().filter_map(|u| u.path()).map(|p| PathBuf::from(p.to_string())).collect()
}

pub fn pick_files(_owner: Win, multi: bool) -> Vec<PathBuf> {
    let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setAllowsMultipleSelection(multi);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    if panel.runModal() != NSModalResponseOK {
        return Vec::new();
    }
    paths_of(&panel.URLs())
}

pub fn pick_folder(_owner: Win) -> Option<PathBuf> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(false);
    panel.setCanChooseDirectories(true);
    panel.setCanCreateDirectories(true);
    panel.setMessage(Some(&NSString::from_str(t("Chọn thư mục nhân vật", "Choose a character folder"))));
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    paths_of(&panel.URLs()).into_iter().next()
}

pub fn pick_save_file(_owner: Win, suggested: &str) -> Option<PathBuf> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&NSString::from_str(&format!("{suggested}.{}", library::PACK_EXTENSION)));
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    panel.URL().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string()))
}

pub fn copy_text(_owner: Win, text: &str) -> bool {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    board.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
}
