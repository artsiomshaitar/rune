//! The tvOS shell: a Rune app as a whole Apple TV app, with no Swift.
//!
//! [`TvApp::run`] starts UIKit with Rune's own application and scene delegates. The scene gets a
//! window whose view controller holds a `CAMetalLayer` filling its view, drives a
//! [`LayerHost`] onto it from a `CADisplayLink`, and pauses the link while nothing animates,
//! as an idle desktop window draws nothing. Siri Remote presses and touch-surface swipes reach
//! the app as keys ([`crate::remote`]).
//!
//! The app's `Info.plist` names the scene delegate, as UIKit requires of a scene-based app:
//!
//! ```xml
//! <key>UIApplicationSceneManifest</key>
//! <dict>
//!   <key>UIApplicationSupportsMultipleScenes</key><false/>
//!   <key>UISceneConfigurations</key>
//!   <dict>
//!     <key>UIWindowSceneSessionRoleApplication</key>
//!     <array><dict>
//!       <key>UISceneConfigurationName</key><string>Default</string>
//!       <key>UISceneDelegateClassName</key><string>RuneTVSceneDelegate</string>
//!     </dict></array>
//!   </dict>
//! </dict>
//! ```
//!
//! Menu goes to the app as Escape. A Menu press the system should take instead, which on the
//! app's first screen sends it to the Home screen as Apple's guidelines ask, is the app's to
//! say: [`TvApp::menu_exits`].
//!
//! A native layer can sit under the app's, filling the screen: [`set_underlay`] with an
//! `AVPlayerLayer` plays video beneath controls the app draws in Rune. While one is there the
//! app's layer is transparent wherever it draws nothing, and hidden while it draws nothing at all.
//! Then an element marked `El::system_material` gets a native `UIVisualEffectView` between the
//! two, at its frame, corner radius, opacity and clip, so the system blurs the video under it.
use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::{CString, c_char, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation_video::{NSObject, NSObjectProtocol, NSString};
use objc2_quartz_core::{CADisplayLink, CALayer, CAMetalLayer, CATransaction};
#[allow(deprecated)]
use objc2_ui_kit::{UIApplicationDelegate, UISceneDelegate, UIWindowSceneDelegate, UIApplicationMain, UIPress, UIPressType, UIResponder, UITouch, UIView, UIViewController};
use objc2_video::rc::Retained;
use objc2_video::runtime::{AnyClass, AnyObject};
use objc2_video::{ClassType, DefinedClass, Message, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};

use crate::embed::{EmbedFrame, LayerHost};
use crate::remote::{Button, Input};
use crate::runtime::Waker;
use crate::system_material::uikit::Materials;
use crate::tooling::AppSpec;

/// Puts `layer` (a `CALayer *`) under the app's own, filling the screen, in place of the one
/// there before; null takes it away. A raw pointer, as [`LayerHost::from_core_animation_layer`]
/// takes one, so no Objective-C binding's version is part of this API. While a layer is there the app's layer is transparent
/// ([`LayerHost::set_transparent`]): the app leaves its root unfilled where the layer should
/// show, and when it draws nothing at all its layer is hidden, so the screen shows the underlay
/// alone. Call it on the main thread, as `init`, `poll` and `on_key` run, any time after
/// [`TvApp::run`] starts.
///
/// # Safety
/// `layer` is null or points to a live Objective-C object.
///
/// # Panics
/// Off the main thread, or when `layer` isn't a `CALayer`.
pub unsafe fn set_underlay(layer: *mut std::ffi::c_void) {
    assert!(MainThreadMarker::new().is_some(), "rune::tvos::set_underlay runs on the main thread");
    // SAFETY: the caller's promise: null, or a live object.
    let layer = unsafe { layer.cast::<AnyObject>().as_ref() };
    let layer = layer.map(|l| l.downcast_ref::<CALayer>().expect("rune::tvos::set_underlay takes a CALayer").retain());
    with_controller(|c| c.set_underlay(layer));
}

/// A Rune app run as the whole tvOS app. Build it from the same [`AppSpec`] a desktop app runs.
pub struct TvApp<S: 'static> {
    spec: AppSpec<S>,
    menu_exits: Option<fn(&S) -> bool>,
    render_scale: Option<f64>,
    /// `TvApp::compositor`, as the step that attaches it to the host once there is one.
    attach: Option<Box<dyn FnOnce(LayerHost<S>) -> LayerHost<S>>>,
}

thread_local! {
    /// `TvApp::render_scale`, read where the view is laid out.
    static RENDER_SCALE: Cell<Option<f64>> = const { Cell::new(None) };
}

impl<S: 'static> TvApp<S> {
    pub fn new(spec: AppSpec<S>) -> Self {
        Self { spec, menu_exits: None, render_scale: None, attach: None }
    }

    /// Whether a Menu press goes to the system instead of the app, asked as the press begins.
    /// True on the app's first screen sends the app to the Home screen. Without it every Menu
    /// press reaches the app as Escape.
    pub fn menu_exits(mut self, f: fn(&S) -> bool) -> Self {
        self.menu_exits = Some(f);
        self
    }

    /// Draws at no more than `scale` device pixels per point, and lets the screen upscale the
    /// rest. On an Apple TV 4K, 1.5 draws 2880x1620: just over half the pixels of 4K, which buys
    /// a frame budget the full resolution can miss, and text stays far crisper than at 1080p. A
    /// screen with a lower scale (a 1080p television) draws at its own. The `RUNE_TVOS_SCALE`
    /// environment variable overrides it, uncapped.
    pub fn render_scale(mut self, scale: f64) -> Self {
        self.render_scale = Some(scale).filter(|s| s.is_finite() && *s > 0.0);
        self
    }

    /// Draws each frame through `compositor`, as `App::compositor` does on the desktop: an app's
    /// own GPU passes at its `El::effect` slots (glass, lenses, glows from rune-fx).
    pub fn compositor(mut self, compositor: impl crate::compose::Compositor<S>) -> Self {
        self.attach = Some(Box::new(move |host: LayerHost<S>| host.with_compositor(compositor)));
        self
    }

    /// Starts UIKit and never returns. Call it from `main` on the main thread.
    pub fn run(self) -> ! {
        let Self { spec, menu_exits, render_scale, attach } = self;
        RENDER_SCALE.with(|cell| cell.set(render_scale));
        FACTORY.with(|f| {
            *f.borrow_mut() = Some(Box::new(move |layer, viewport, scale, wake| {
                let state = (spec.init)();
                // SAFETY: `layer` is the controller's CAMetalLayer, which lives as long as the
                // app, and this runs on the main thread.
                let host = unsafe { LayerHost::from_core_animation_layer(layer, &spec, state, viewport, scale, true, wake)? };
                let host = match attach {
                    Some(attach) => attach(host),
                    None => host,
                };
                Ok(Box::new(Hosted { host, menu_exits }) as Box<dyn Host>)
            }));
        });
        let _ = (AppDelegate::class(), SceneDelegate::class(), Controller::class());
        let args: Vec<CString> = std::env::args().filter_map(|a| CString::new(a).ok()).collect();
        let mut argv: Vec<*mut c_char> = args.iter().map(|a| a.as_ptr().cast_mut()).collect();
        argv.push(std::ptr::null_mut());
        let delegate = NSString::from_str("RuneTVAppDelegate");
        let argc = (argv.len() - 1) as i32;
        // SAFETY: argv is null-terminated and outlives UIApplicationMain, which never returns.
        #[allow(deprecated)]
        unsafe { UIApplicationMain(argc, NonNull::new_unchecked(argv.as_mut_ptr()), None, Some(&delegate)) };
        std::process::exit(0)
    }
}

trait Host {
    /// One frame, with `materials` placed for its material elements when `native`.
    fn frame(&mut self, materials: Option<&mut Materials>, native: bool) -> EmbedFrame;
    fn resize(&mut self, viewport: (f32, f32), scale: f32);
    fn remote(&mut self, input: Input);
    fn menu_exits(&self) -> bool;
    fn set_transparent(&mut self, on: bool) -> bool;
}

struct Hosted<S: 'static> {
    host: LayerHost<S>,
    menu_exits: Option<fn(&S) -> bool>,
}

impl<S: 'static> Host for Hosted<S> {
    fn frame(&mut self, materials: Option<&mut Materials>, native: bool) -> EmbedFrame {
        self.host.set_native_materials(native && materials.is_some());
        self.host.frame_with(materials.map(|m| m as &mut dyn crate::system_material::NativeHost))
    }
    fn resize(&mut self, viewport: (f32, f32), scale: f32) { self.host.resize(viewport, scale) }
    fn remote(&mut self, input: Input) { self.host.remote(input) }
    fn menu_exits(&self) -> bool { self.menu_exits.is_some_and(|f| f(&self.host.runtime().state)) }
    fn set_transparent(&mut self, on: bool) -> bool { self.host.set_transparent(on) }
}

type Factory = Box<dyn FnOnce(*mut c_void, (f32, f32), f32, Waker) -> Result<Box<dyn Host>, String>>;

thread_local! {
    static FACTORY: RefCell<Option<Factory>> = const { RefCell::new(None) };
    static HOST: RefCell<Option<Box<dyn Host>>> = const { RefCell::new(None) };
    static CONTROLLER: RefCell<Option<Retained<Controller>>> = const { RefCell::new(None) };
}

/// The scene's view controller, once the scene has connected.
fn with_controller(f: impl FnOnce(&Controller)) {
    CONTROLLER.with(|c| if let Some(c) = &*c.borrow() { f(c) });
}

/// The open host, once the view has laid out with a size.
fn with_host<R>(f: impl FnOnce(&mut dyn Host) -> R) -> Option<R> {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let host: &mut dyn Host = h.as_mut()?.as_mut();
        Some(f(host))
    })
}

/// Starts the display link again, from any thread: new data, a timer or input wants a frame.
fn wake() {
    crate::platform::main_queue::on_main(|| with_controller(Controller::resume));
}

fn button(press: &UIPress) -> Option<Button> {
    Some(match press.r#type() {
        UIPressType::UpArrow => Button::Up,
        UIPressType::DownArrow => Button::Down,
        UIPressType::LeftArrow => Button::Left,
        UIPressType::RightArrow => Button::Right,
        UIPressType::Select => Button::Select,
        UIPressType::Menu => Button::Menu,
        UIPressType::PlayPause => Button::PlayPause,
        UIPressType::PageUp => Button::PageUp,
        UIPressType::PageDown => Button::PageDown,
        _ => return None,
    })
}

/// The objects in an `NSSet` (of presses or touches).
fn set_items<T: objc2_video::Message>(set: &AnyObject) -> Vec<Retained<T>> {
    // SAFETY: `allObjects` on an NSSet returns an NSArray of its members, all of type T here.
    unsafe {
        let all: Retained<AnyObject> = msg_send![set, allObjects];
        let count: usize = msg_send![&*all, count];
        (0..count).map(|i| msg_send![&*all, objectAtIndex: i]).collect()
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneTVAppDelegate"]
    struct AppDelegate;

    impl AppDelegate {
        #[unsafe(method(application:didFinishLaunchingWithOptions:))]
        fn did_finish_launching(&self, _app: &AnyObject, _options: Option<&AnyObject>) -> bool {
            true
        }
    }

    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl UIApplicationDelegate for AppDelegate {}
);

#[derive(Default)]
struct SceneIvars {
    window: RefCell<Option<Retained<AnyObject>>>,
}

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneTVSceneDelegate"]
    #[ivars = SceneIvars]
    struct SceneDelegate;

    impl SceneDelegate {
        #[unsafe(method_id(init))]
        fn init(this: objc2_video::rc::Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(SceneIvars::default());
            // SAFETY: NSObject's init on a freshly allocated object.
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(&self, scene: &AnyObject, _session: &AnyObject, _options: &AnyObject) {
            let mtm = MainThreadMarker::from(self);
            let controller = Controller::new(mtm);
            let Some(cls) = AnyClass::get(c"UIWindow") else { return };
            // SAFETY: plain UIWindow calls on the main thread, with the scene UIKit handed us.
            unsafe {
                let window: Retained<AnyObject> = msg_send![msg_send![cls, alloc], initWithWindowScene: scene];
                let _: () = msg_send![&*window, setRootViewController: &*controller];
                let _: () = msg_send![&*window, makeKeyAndVisible];
                *self.ivars().window.borrow_mut() = Some(window);
            }
            CONTROLLER.with(|c| *c.borrow_mut() = Some(controller));
        }

        #[unsafe(method_id(window))]
        fn window(&self) -> Option<Retained<AnyObject>> {
            self.ivars().window.borrow().clone()
        }

        #[unsafe(method(setWindow:))]
        fn set_window(&self, window: Option<&AnyObject>) {
            *self.ivars().window.borrow_mut() = window.map(|w| w.retain());
        }

        // Metal may not draw in the background: the link stops there and starts on return.
        #[unsafe(method(sceneDidEnterBackground:))]
        fn did_enter_background(&self, _scene: &AnyObject) {
            with_controller(|c| c.set_background(true));
        }

        #[unsafe(method(sceneWillEnterForeground:))]
        fn will_enter_foreground(&self, _scene: &AnyObject) {
            with_controller(|c| c.set_background(false));
        }
    }

    unsafe impl NSObjectProtocol for SceneDelegate {}
    unsafe impl UISceneDelegate for SceneDelegate {}
    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);

struct ControllerIvars {
    layer: OnceCell<Retained<CAMetalLayer>>,
    link: OnceCell<Retained<CADisplayLink>>,
    background: Cell<bool>,
    /// The Menu press that began went to the system, so its end goes there too.
    menu_to_system: Cell<bool>,
    /// `set_underlay`'s layer, under `layer`.
    underlay: RefCell<Option<Retained<CALayer>>>,
    /// Whether the host draws transparent, which follows `underlay` from the next frame.
    see_through: Cell<bool>,
    /// Native views for material elements, between `underlay` and `layer`.
    materials: RefCell<Option<Materials>>,
    /// The latest timer wake `draw` asked for; an earlier one that fires finds a newer number
    /// here and does nothing, or every frame's wake would go on waking the app.
    wake: Cell<u64>,
}

define_class!(
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneTVViewController"]
    #[ivars = ControllerIvars]
    struct Controller;

    impl Controller {
        #[unsafe(method(viewDidLoad))]
        fn view_did_load(&self) {
            // SAFETY: UIViewController's own viewDidLoad.
            let _: () = unsafe { msg_send![super(self), viewDidLoad] };
            let Some(view) = self.view() else { return };
            let layer = CAMetalLayer::new();
            layer.setOpaque(true);
            // SAFETY: a UIView's layer takes sublayers on the main thread.
            unsafe {
                let root: Retained<AnyObject> = msg_send![&*view, layer];
                let _: () = msg_send![&*root, addSublayer: &*layer];
            }
            *self.ivars().materials.borrow_mut() = Materials::new(&view, &layer);
            if self.ivars().layer.set(layer).is_err() { return }
            // SAFETY: the target answers `runeTick:`; the link retains it for the app's life.
            let link = unsafe { CADisplayLink::displayLinkWithTarget_selector(self, sel!(runeTick:)) };
            let Some(run_loops) = AnyClass::get(c"NSRunLoop") else { return };
            // SAFETY: NSRunLoop's main loop and its common modes, on the main thread.
            unsafe {
                let run_loop: Retained<AnyObject> = msg_send![run_loops, mainRunLoop];
                let _: () = msg_send![&*link, addToRunLoop: &*run_loop, forMode: NSRunLoopCommonModes];
            }
            if let Err(link) = self.ivars().link.set(link) { link.invalidate() }
        }

        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout(&self) {
            // SAFETY: UIViewController's own viewDidLayoutSubviews.
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            self.layout();
        }

        #[unsafe(method(viewDidAppear:))]
        fn view_did_appear(&self, animated: bool) {
            // SAFETY: UIViewController's own viewDidAppear:, then first responder for presses.
            unsafe {
                let _: () = msg_send![super(self), viewDidAppear: animated];
                let _: bool = msg_send![self, becomeFirstResponder];
            }
        }

        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(runeTick:))]
        fn tick(&self, _link: &AnyObject) {
            self.draw();
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            let mut system = false;
            for press in set_items::<UIPress>(presses) {
                match button(&press) {
                    Some(Button::Menu) if with_host(|h| h.menu_exits()).unwrap_or(false) => {
                        self.ivars().menu_to_system.set(true);
                        system = true;
                    }
                    None => system = true,
                    Some(b) => self.input(Input::Press(b)),
                }
            }
            // SAFETY: UIResponder's own handler, with the presses UIKit handed us.
            if system { let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] }; }
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            let mut system = false;
            for press in set_items::<UIPress>(presses) {
                match button(&press) {
                    Some(Button::Menu) if self.ivars().menu_to_system.replace(false) => system = true,
                    None => system = true,
                    Some(b) => self.input(Input::Release(b)),
                }
            }
            // SAFETY: as pressesBegan.
            if system { let _: () = unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] }; }
        }

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            self.ivars().menu_to_system.set(false);
            self.input(Input::Cancel);
            // SAFETY: as pressesBegan.
            let _: () = unsafe { msg_send![super(self), pressesCancelled: presses, withEvent: event] };
        }

        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &AnyObject, _event: Option<&AnyObject>) {
            if let Some(p) = self.touch_point(touches) { self.input(Input::TouchBegan { x: p.0, y: p.1 }) }
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &AnyObject, _event: Option<&AnyObject>) {
            if let Some(p) = self.touch_point(touches) { self.input(Input::TouchMoved { x: p.0, y: p.1 }) }
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, _touches: &AnyObject, _event: Option<&AnyObject>) {
            self.input(Input::TouchEnded);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, _touches: &AnyObject, _event: Option<&AnyObject>) {
            self.input(Input::TouchEnded);
        }
    }

    unsafe impl NSObjectProtocol for Controller {}
);

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSRunLoopCommonModes: &'static NSString;
}

/// Builds the app's host on `layer`, once: the factory `TvApp::run` left is taken.
fn open_host(layer: &CAMetalLayer, viewport: (f32, f32), scale: f32) {
    let Some(factory) = FACTORY.with(|f| f.borrow_mut().take()) else { return };
    let ptr = std::ptr::from_ref(layer).cast_mut().cast::<c_void>();
    match factory(ptr, viewport, scale, Arc::new(wake)) {
        Ok(host) => HOST.with(|h| *h.borrow_mut() = Some(host)),
        Err(e) => eprintln!("rune: the tvOS host failed to open: {e}"),
    }
}

impl Controller {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ControllerIvars {
            layer: OnceCell::new(),
            link: OnceCell::new(),
            background: Cell::new(false),
            menu_to_system: Cell::new(false),
            underlay: RefCell::new(None),
            see_through: Cell::new(false),
            materials: RefCell::new(None),
            wake: Cell::new(0),
        });
        // SAFETY: UIViewController's designated init with no nib.
        unsafe { msg_send![super(this), initWithNibName: None::<&AnyObject>, bundle: None::<&AnyObject>] }
    }

    fn view_size(&self) -> Option<((f32, f32), f32, CGRect)> {
        let view: Retained<UIView> = self.view()?;
        let bounds = view.bounds();
        // The screen's scale, not the view's: a plain view backed by no drawing of its own reports
        // 1, so on an Apple TV 4K the app drew at 1080p and the TV upscaled it.
        let scale = screen_scale(&view).unwrap_or_else(|| view.contentScaleFactor()) as f32;
        Some(((bounds.size.width as f32, bounds.size.height as f32), scale.max(1.0), bounds))
    }

    /// Sizes the layer to the view, and opens the host on the first layout with a size.
    fn layout(&self) {
        let (Some(layer), Some((viewport, scale, bounds))) = (self.ivars().layer.get(), self.view_size()) else { return };
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        layer.setFrame(CGRect { origin: CGPoint::ZERO, size: bounds.size });
        layer.setContentsScale(scale as f64);
        if let Some(underlay) = &*self.ivars().underlay.borrow() { underlay.setFrame(bounds) }
        CATransaction::commit();
        if with_host(|h| h.resize(viewport, scale)).is_none() {
            open_host(layer, viewport, scale);
        }
        self.resume();
    }

    fn input(&self, input: Input) {
        with_host(|h| h.remote(input));
        self.resume();
    }

    fn touch_point(&self, touches: &AnyObject) -> Option<(f32, f32)> {
        let touch = set_items::<UITouch>(touches).into_iter().next()?;
        let view = self.view()?;
        let p = touch.locationInView(Some(&view));
        Some((p.x as f32, p.y as f32))
    }

    fn set_underlay(&self, layer: Option<Retained<CALayer>>) {
        let Some(view) = self.view() else { return };
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if let Some(old) = self.ivars().underlay.take() { old.removeFromSuperlayer() }
        if let Some(layer) = &layer {
            layer.setFrame(view.bounds());
            view.layer().insertSublayer_atIndex(layer, 0);
        }
        CATransaction::commit();
        *self.ivars().underlay.borrow_mut() = layer;
        self.resume();
    }

    /// Whether the host's transparency is behind `underlay`'s, and a frame is still to show it.
    fn see_through_stale(&self) -> bool {
        self.ivars().underlay.borrow().is_some() != self.ivars().see_through.get()
    }

    fn resume(&self) {
        if self.ivars().background.get() { return }
        if let Some(link) = self.ivars().link.get() { link.setPaused(false) }
    }

    fn set_background(&self, background: bool) {
        self.ivars().background.set(background);
        if let Some(link) = self.ivars().link.get() { link.setPaused(background) }
    }

    fn draw(&self) {
        let ivars = self.ivars();
        if self.see_through_stale() {
            let on = !ivars.see_through.get();
            if with_host(|h| h.set_transparent(on)).is_some() { ivars.see_through.set(on) }
        }
        let mut materials = ivars.materials.borrow_mut();
        let native = ivars.see_through.get() && materials.as_ref().is_some_and(Materials::enabled);
        let Some(frame) = with_host(|h| h.frame(materials.as_mut(), native)) else { return };
        drop(materials);
        if let Some(layer) = ivars.layer.get().filter(|_| frame.drew) {
            let hidden = ivars.see_through.get() && frame.blank;
            if layer.isHidden() != hidden { layer.setHidden(hidden) }
        }
        // An underlay set from this frame's `poll` or `on_key` is seen through from the next.
        if frame.animating || self.see_through_stale() { return }
        if let Some(link) = ivars.link.get() { link.setPaused(true) }
        let wake = ivars.wake.get() + 1;
        ivars.wake.set(wake);
        if let Some(after) = frame.wake_after_secs {
            crate::platform::main_queue::on_main_after(after, move || with_controller(|c| if c.ivars().wake.get() == wake { c.resume() }));
        }
    }
}

/// The scale of the screen showing `view`: 2 on an Apple TV 4K at 4K, 1 at 1080p. `None` before
/// the view is in a window.
fn screen_scale(view: &UIView) -> Option<f64> {
    // `RUNE_TVOS_SCALE=1.5` draws at that scale instead, over `TvApp::render_scale`: for
    // weighing sharpness against frame time on a given Apple TV.
    if let Some(scale) = std::env::var("RUNE_TVOS_SCALE").ok().and_then(|v| v.parse::<f64>().ok()).filter(|s| *s > 0.0) {
        return Some(scale);
    }
    let cap = RENDER_SCALE.with(Cell::get);
    // SAFETY: `UIView.window`, `UIWindow.screen` and `UIScreen.scale` exist on every tvOS this
    // supports, and this runs on the main thread, in layout.
    unsafe {
        let window: Option<Retained<AnyObject>> = msg_send![view, window];
        let screen: Option<Retained<AnyObject>> = msg_send![&*window?, screen];
        let scale: f64 = msg_send![&*screen?, scale];
        Some(cap.map_or(scale, |cap| cap.min(scale)))
    }
}
