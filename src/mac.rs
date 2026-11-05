//! The macOS side: the character in a small floating window, and a menu bar item.
//! Frames are the same premultiplied BGRA as on Windows, handed to the window as an image.

use crate::anim::{self, CharAnim, Player};
use crate::config::{user_root, Config};
use crate::i18n::t;
use crate::library::{self, state_index, Character};
use crate::server::{self, now_ms, Shared};
use crate::store::{self, State};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSControlStateValueOn, NSEvent, NSFloatingWindowLevel, NSMenu, NSMenuItem, NSScreen, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFData, CGFloat};
use objc2_core_graphics::{CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSTimer};
use objc2_quartz_core::CATransaction;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

const SIZE_FACTORS: [f32; 3] = [0.75, 1.0, 1.25];
/// How often a still picture looks for a new state. Hooks arrive on another thread and don't wake this one.
const STILL_TICK_MS: u32 = 250;
/// Frames faster than this only cost power.
const FASTEST_TICK_MS: u32 = 33;
const HOUSEKEEPING_MS: u64 = 5_000;

struct Loaded {
    token: u64,
    result: Result<Arc<CharAnim>, String>,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

struct Drag {
    start_mouse: NSPoint,
    start_origin: NSPoint,
    moved: bool,
}

struct App {
    shared: Arc<Shared>,
    config: Config,
    library: Vec<Character>,
    player: Option<Player>,
    state: State,
    scale: f32,
    load_token: u64,
    window: Retained<NSWindow>,
    view: Retained<BuddyView>,
    status: Retained<NSStatusItem>,
    timer: Option<Retained<NSTimer>>,
    tick_ms: u32,
    next_housekeeping: u64,
    drag: Option<Drag>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut b| b.as_mut().map(f)))
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClaudeBuddyView"]
    struct BuddyView;

    impl BuddyView {
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            with_app(App::tick);
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            with_app(|a| a.drag = Some(Drag { start_mouse: NSEvent::mouseLocation(), start_origin: a.window.frame().origin, moved: false }));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            with_app(App::drag_to_mouse);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            with_app(App::end_drag);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            // Outside with_app: the menu runs its own loop, and choosing from it needs the app.
            let Some(menu) = with_app(|a| a.menu()) else { return };
            NSMenu::popUpContextMenu_withEvent_forView(&menu, event, self);
        }

        #[unsafe(method(chooseCharacter:))]
        fn choose_character(&self, item: &NSMenuItem) {
            with_app(|a| a.choose(item.tag() as usize));
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _item: &NSMenuItem) {
            std::process::exit(0);
        }
    }
);

impl App {
    fn mtm(&self) -> MainThreadMarker {
        self.view.mtm()
    }

    /// Picture density: the display's, times the chosen size.
    fn char_scale(&self) -> f32 {
        self.scale * SIZE_FACTORS[self.config.size.min(2) as usize]
    }

    fn load_character(&mut self) {
        self.load_token += 1;
        let Some(c) = self.library.iter().find(|c| c.id == self.config.character).or(self.library.first()).cloned() else { return };
        let (token, dpi) = (self.load_token, self.char_scale());
        let _ = std::thread::Builder::new().name("load-character".into()).spawn(move || {
            let max_h = (anim::BASE_HEIGHT * c.scale).min(anim::MAX_STAGE_HEIGHT);
            let result = anim::load(&c, anim::MAX_WIDTH, max_h, dpi);
            *LOADED.lock().unwrap() = Some(Loaded { token, result });
        });
    }

    fn on_loaded(&mut self) {
        let Some(l) = LOADED.lock().unwrap().take() else { return };
        if l.token != self.load_token {
            return; // a newer choice won
        }
        match l.result {
            Ok(a) => {
                self.player = Some(Player::new(a, state_index(self.state), now_ms()));
                self.fit_window();
                self.show_frame();
                self.window.orderFrontRegardless();
                self.retime();
            }
            Err(e) => eprintln!("{e}"),
        }
    }

    /// The window is exactly the picture, standing on the same spot when its size changes.
    fn fit_window(&self) {
        let Some(p) = &self.player else { return };
        let size = NSSize::new((p.anim.w as f32 / self.scale) as CGFloat, (p.anim.h as f32 / self.scale) as CGFloat);
        let old = self.window.frame();
        // The very first time there is no old size, and the saved spot is already right.
        let shift = if old.size.width > 1.0 { (old.size.width - size.width) / 2.0 } else { 0.0 };
        let origin = NSPoint::new(old.origin.x + shift, old.origin.y);
        self.window.setFrame_display(NSRect::new(origin, size), true);
    }

    fn show_frame(&self) {
        let Some(p) = &self.player else { return };
        let Some(image) = frame_image(&p.pixels, p.anim.w as usize, p.anim.h as usize) else { return };
        let Some(layer) = self.view.layer() else { return };
        let image: *const CGImage = &*image;
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        // The layer takes a CGImage as its contents, through a property typed as any object.
        unsafe { layer.setContents(Some(&*(image as *const AnyObject))) };
        CATransaction::commit();
    }

    fn tick(&mut self) {
        self.on_loaded();
        let now = now_ms();
        if now >= self.next_housekeeping {
            self.next_housekeeping = now + HOUSEKEEPING_MS;
            self.housekeeping(now);
        }
        let state = store::overall(&self.shared.store.lock().unwrap().list());
        if state != self.state {
            self.state = state;
            if let Some(p) = &mut self.player {
                p.set_state(state_index(state), now);
            }
            self.show_frame();
            self.retime();
        }
        if self.player.as_mut().is_some_and(|p| p.advance(now).is_some()) {
            self.show_frame();
        }
    }

    /// Also catches turns that never send Stop, like interrupted ones.
    fn housekeeping(&mut self, now: u64) {
        let mut st = self.shared.store.lock().unwrap();
        for (id, pid) in st.running_pids() {
            let gone = unsafe { libc::kill(pid as i32, 0) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if gone {
                st.fail(&id, t("Tiến trình Claude đã dừng", "The Claude process stopped"), now);
            }
        }
        st.tick(now);
        // Announcements have nowhere to go yet.
        self.shared.pending.lock().unwrap().clear();
    }

    fn retime(&mut self) {
        let want = match &self.player {
            Some(p) if !p.clip().is_still() => p.clip().min_delay().max(FASTEST_TICK_MS),
            _ => STILL_TICK_MS,
        };
        if want != self.tick_ms {
            self.start_timer(want);
        }
    }

    fn start_timer(&mut self, ms: u32) {
        if let Some(old) = self.timer.take() {
            old.invalidate();
        }
        self.tick_ms = ms;
        let target: &AnyObject = &self.view;
        self.timer = Some(unsafe { NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(ms as f64 / 1000.0, target, sel!(tick:), None, true) });
    }

    fn drag_to_mouse(&mut self) {
        let Some(d) = &mut self.drag else { return };
        let mouse = NSEvent::mouseLocation();
        d.moved = true;
        self.window.setFrameOrigin(NSPoint::new(d.start_origin.x + mouse.x - d.start_mouse.x, d.start_origin.y + mouse.y - d.start_mouse.y));
    }

    fn end_drag(&mut self) {
        if self.drag.take().is_some_and(|d| d.moved) {
            let o = self.window.frame().origin;
            self.config.position = Some((o.x as i32, o.y as i32));
            self.config.save();
        }
    }

    fn choose(&mut self, index: usize) {
        let Some(c) = self.library.get(index) else { return };
        self.config.character = c.id.clone();
        self.config.save();
        self.load_character();
        let menu = self.menu();
        self.status.setMenu(Some(&menu));
    }

    fn menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        let target: &AnyObject = &self.view;
        for (i, c) in self.library.iter().enumerate() {
            let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(&c.name), Some(sel!(chooseCharacter:)), &NSString::new()) };
            unsafe { item.setTarget(Some(target)) };
            item.setTag(i as isize);
            if c.id == self.config.character {
                item.setState(NSControlStateValueOn);
            }
            menu.addItem(&item);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let quit = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(t("Thoát", "Quit")), Some(sel!(quit:)), &NSString::new()) };
        unsafe { quit.setTarget(Some(target)) };
        menu.addItem(&quit);
        menu
    }
}

/// Copies the frame, because the player keeps writing into its pixels while the window shows this.
fn frame_image(pixels: &[u8], w: usize, h: usize) -> Option<objc2_core_foundation::CFRetained<CGImage>> {
    let data = CFData::from_bytes(pixels);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;
    let info = CGBitmapInfo(CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0);
    unsafe { CGImage::new(w, h, 8, 32, w * 4, Some(&space), info, Some(&provider), std::ptr::null(), false, CGColorRenderingIntent::RenderingIntentDefault) }
}

/// Where the saved spot still is on a screen, or else the bottom right corner.
fn start_origin(config: &Config, mtm: MainThreadMarker) -> NSPoint {
    if let Some((x, y)) = config.position {
        let p = NSPoint::new(x as CGFloat, y as CGFloat);
        let on_screen = NSScreen::screens(mtm).iter().any(|s| {
            let f = s.frame();
            p.x >= f.origin.x && p.y >= f.origin.y && p.x < f.origin.x + f.size.width && p.y < f.origin.y + f.size.height
        });
        if on_screen {
            return p;
        }
    }
    let area = NSScreen::mainScreen(mtm).map_or(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0)), |s| s.visibleFrame());
    NSPoint::new(area.origin.x + area.size.width - 240.0, area.origin.y + 16.0)
}

pub fn run() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let config = Config::load();
    let shared = Arc::new(Shared::default());
    if let Err(e) = server::start(shared.clone(), config.port) {
        eprintln!("{}", crate::tf!("Không mở được cổng {}: {e}", "Could not open port {}: {e}", config.port));
        std::process::exit(1);
    }
    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon: it lives in the menu bar.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let frame = NSRect::new(start_origin(&config, mtm), NSSize::new(1.0, 1.0));
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), frame, NSWindowStyleMask::Borderless, NSBackingStoreType::Buffered, false) };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setHasShadow(false);
    window.setLevel(NSFloatingWindowLevel);
    window.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::Stationary | NSWindowCollectionBehavior::IgnoresCycle);

    let view: Retained<BuddyView> = unsafe { msg_send![BuddyView::alloc(mtm), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0))] };
    view.setWantsLayer(true);
    window.setContentView(Some(&view));

    let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status.button(mtm) {
        button.setTitle(&NSString::from_str("🐾"));
    }

    let scale = NSScreen::mainScreen(mtm).map_or(2.0, |s| s.backingScaleFactor()) as f32;
    let mut state = App {
        shared,
        library: library::load_all(&user_root()),
        config,
        player: None,
        state: State::Idle,
        scale,
        load_token: 0,
        window,
        view,
        status,
        timer: None,
        tick_ms: 0,
        next_housekeeping: 0,
        drag: None,
    };
    let menu = state.menu();
    state.status.setMenu(Some(&menu));
    state.start_timer(STILL_TICK_MS);
    state.load_character();
    APP.with(|a| *a.borrow_mut() = Some(state));
    app.run();
}
