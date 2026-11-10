//! Native material views under a tvOS app's Metal layer, moved in the drawable's transaction.
//!
//! The view controller's view holds the underlay (`rune::tvos::set_underlay`), then a host view
//! for these, then the Metal layer, which a `zPosition` keeps on top. Each material element gets
//! a `UIVisualEffectView` at its frame and corner radius, which blurs what is under the app: a
//! video playing beneath its controls. With `presentsWithTransaction` on the Metal layer while
//! any are on screen, the views move in the transaction the drawable presents in.
//!
//! UIKit draws an effect view wrong at an alpha under 1, or under a masked superview, but
//! forwards a mask set on the effect view itself to the views inside it. So the element's
//! opacity and clip both go there: one gradient down the view, which also carries the opacity,
//! and one across it as that gradient's own mask.

use super::{assign, surplus, MaterialDraw, Mode, NativeHost, Stops, SystemMaterial, MAX_NATIVE_MATERIALS};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_video::rc::Retained;
use objc2_video::runtime::{AnyClass, AnyObject, Bool};
use objc2_video::{msg_send, Encoding, Message, RefEncode};
use rune_motion::NodeId;

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } }
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    static kCACornerCurveContinuous: &'static AnyObject;
    static kCACornerCurveCircular: &'static AnyObject;
}

/// `CGColorRef`'s pointee, for its type encoding.
#[repr(C)]
struct CGColor([u8; 0]);

// SAFETY: CGColorRef's encoding, as Core Graphics declares it.
unsafe impl RefEncode for CGColor {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

/// What a view's mask shows: across, and down with the opacity folded in.
type Mask = (Option<Stops>, Stops);

struct Slot {
    view: Retained<AnyObject>,
    material: SystemMaterial,
    /// `UIGlassEffect`, which takes its corners from `cornerConfiguration`.
    glass: bool,
    /// The element it showed last, which keeps it while it stays mounted.
    owner: Option<NodeId>,
    /// Frame, corner radius and curve last set.
    shape: Option<([f64; 4], f64, bool)>,
    /// The mask last set; `None` once hidden.
    shown: Option<Option<Mask>>,
}

/// One tvOS app's material views.
pub(crate) struct Materials {
    mode: Mode,
    host: Retained<AnyObject>,
    metal: Retained<AnyObject>,
    /// Whether the drawable being drawn presents in the views' transaction.
    synced: bool,
    slots: Vec<Slot>,
}

impl Materials {
    /// A host for material views in `view` (a `UIView`), under `metal` (its `CAMetalLayer`).
    pub(crate) fn new(view: &AnyObject, metal: &AnyObject) -> Option<Self> {
        // SAFETY: UIKit calls on the main thread, in the controller's viewDidLoad.
        unsafe {
            let bounds: CGRect = msg_send![view, bounds];
            let host: Retained<AnyObject> = msg_send![msg_send![AnyClass::get(c"UIView")?, alloc], initWithFrame: bounds];
            // Width and height sizable; never focused or touched; dark, as the host's `Env` is.
            let _: () = msg_send![&*host, setAutoresizingMask: 18usize];
            let _: () = msg_send![&*host, setUserInteractionEnabled: false];
            let _: () = msg_send![&*host, setOverrideUserInterfaceStyle: 2isize];
            let _: () = msg_send![view, addSubview: &*host];
            let _: () = msg_send![metal, setZPosition: 1.0f64];
            Some(Materials { mode: Mode::from_env(), host, metal: metal.retain(), synced: false, slots: Vec::new() })
        }
    }

    /// Whether elements may leave their interior to native views at all (`RUNE_SYSTEM_MATERIAL`).
    pub(crate) fn enabled(&self) -> bool {
        self.mode != Mode::Off
    }

    fn idle(&self) -> bool {
        self.slots.iter().all(|s| s.shown.is_none())
    }

    fn sync(&mut self, draws: &[MaterialDraw], scale: f32) {
        let draws = &draws[..draws.len().min(MAX_NATIVE_MATERIALS)];
        let wanted: Vec<_> = draws.iter().map(|d| (resolve(self.mode, d.material), d.id)).collect();
        let views: Vec<_> = self.slots.iter().map(|s| (s.material, s.owner)).collect();
        let mut taken = vec![false; self.slots.len()];
        for (draw, (at, &(material, id))) in draws.iter().zip(assign(&views, &wanted).into_iter().zip(&wanted)) {
            let k = match at {
                Some(k) => k,
                None => {
                    let Some(slot) = make_view(material, &self.host) else { continue };
                    self.slots.push(slot);
                    taken.push(false);
                    self.slots.len() - 1
                }
            };
            taken[k] = true;
            let slot = &mut self.slots[k];
            slot.owner = Some(id);
            let (frame, radius) = draw.points(scale);
            let shape = (frame, radius, draw.continuous);
            let mask = draw.faded_mask();
            let view = &*slot.view;
            // SAFETY: UIView and CALayer setters on the main thread.
            unsafe {
                if slot.shown.is_none() { let _: () = msg_send![view, setHidden: false]; }
                if slot.shape != Some(shape) {
                    let _: () = msg_send![view, setFrame: rect(frame[0], frame[1], frame[2], frame[3])];
                    set_corners(view, slot.glass, radius, draw.continuous);
                }
                if slot.shape.is_none_or(|s| s.0[2..] != frame[2..]) || slot.shown.as_ref() != Some(&mask) {
                    let _: () = msg_send![view, setMaskView: std::ptr::null::<AnyObject>()];
                    if let Some(mask) = &mask && let Some(m) = mask_view(mask, frame[2], frame[3]) {
                        let _: () = msg_send![view, setMaskView: &*m];
                    }
                }
            }
            slot.shape = Some(shape);
            slot.shown = Some(mask);
        }
        for (slot, _) in self.slots.iter_mut().zip(taken).filter(|(s, taken)| !taken && s.shown.is_some()) {
            // SAFETY: a UIView setter on the main thread.
            unsafe { let _: () = msg_send![&*slot.view, setHidden: true]; }
            slot.shown = None;
            slot.owner = None;
        }
        let views: Vec<_> = self.slots.iter().map(|s| (s.material, s.shown.is_none())).collect();
        for k in surplus(&views) {
            // SAFETY: removes a view this struct added to the host.
            unsafe { let _: () = msg_send![&*self.slots.remove(k).view, removeFromSuperview]; }
        }
    }
}

impl NativeHost for Materials {
    fn acquire(&mut self, draws: &[MaterialDraw]) {
        let synced = self.mode == Mode::Synced && !draws.is_empty();
        if synced != self.synced {
            // SAFETY: a CAMetalLayer property, on the main thread, before its next drawable.
            unsafe { let _: () = msg_send![&*self.metal, setPresentsWithTransaction: Bool::new(synced)]; }
            self.synced = synced;
        }
    }

    fn present(&mut self, draws: &[MaterialDraw], scale: f32, present: &mut dyn FnMut()) {
        if draws.is_empty() && self.idle() { return present(); }
        let Some(transaction) = AnyClass::get(c"CATransaction") else { return present() };
        // SAFETY: CATransaction class methods on the main thread, balanced below.
        unsafe { let _: () = msg_send![transaction, begin]; let _: () = msg_send![transaction, setDisableActions: true]; }
        self.sync(draws, scale);
        // SAFETY: closes the transaction opened above, with the drawable inside it when synced.
        if !self.synced { unsafe { let _: () = msg_send![transaction, commit]; } }
        present();
        if self.synced { unsafe { let _: () = msg_send![transaction, commit]; } }
    }
}

/// The material shown: `Glass` is the regular blur before tvOS 26 and in legacy mode.
fn resolve(mode: Mode, material: SystemMaterial) -> SystemMaterial {
    match material {
        SystemMaterial::Glass if mode == Mode::Legacy || AnyClass::get(c"UIGlassEffect").is_none() => SystemMaterial::Popover,
        other => other,
    }
}

/// A hidden effect view in `host`.
fn make_view(material: SystemMaterial, host: &AnyObject) -> Option<Slot> {
    let glass = material == SystemMaterial::Glass;
    // SAFETY: UIKit initializers and setters on the main thread; the view is retained by its slot.
    unsafe {
        let effect: Retained<AnyObject> = if glass {
            // UIGlassEffectStyleRegular.
            msg_send![AnyClass::get(c"UIGlassEffect")?, effectWithStyle: 0isize]
        } else {
            // UIBlurEffectStyleRegular, which follows the host's dark style.
            msg_send![AnyClass::get(c"UIBlurEffect")?, effectWithStyle: 4isize]
        };
        let view: Retained<AnyObject> = msg_send![msg_send![AnyClass::get(c"UIVisualEffectView")?, alloc], initWithEffect: &*effect];
        let _: () = msg_send![&*view, setUserInteractionEnabled: false];
        let _: () = msg_send![&*view, setHidden: true];
        let _: () = msg_send![host, addSubview: &*view];
        Some(Slot { view, material, glass, owner: None, shape: None, shown: None })
    }
}

/// Rounds `view`: glass through its `cornerConfiguration`, a blur by clipping its layer.
unsafe fn set_corners(view: &AnyObject, glass: bool, radius: f64, continuous: bool) {
    // SAFETY: UIKit and CALayer setters on the main thread.
    unsafe {
        if glass && let Some(fixed) = AnyClass::get(c"UICornerRadius") && let Some(config) = AnyClass::get(c"UICornerConfiguration") {
            let r: Retained<AnyObject> = msg_send![fixed, fixedRadius: radius];
            let c: Retained<AnyObject> = msg_send![config, configurationWithUniformRadius: &*r];
            let _: () = msg_send![view, setCornerConfiguration: &*c];
            return;
        }
        let layer: Retained<AnyObject> = msg_send![view, layer];
        let _: () = msg_send![&*layer, setCornerRadius: radius];
        let curve: &AnyObject = if continuous { kCACornerCurveContinuous } else { kCACornerCurveCircular };
        let _: () = msg_send![&*layer, setCornerCurve: curve];
        let _: () = msg_send![view, setClipsToBounds: true];
    }
}

/// A mask view `w` by `h` points: the gradient down, masked by the one across.
fn mask_view((across, down): &Mask, w: f64, h: f64) -> Option<Retained<AnyObject>> {
    // SAFETY: UIKit and Core Animation calls on the main thread.
    unsafe {
        let bounds = rect(0.0, 0.0, w, h);
        let view: Retained<AnyObject> = msg_send![msg_send![AnyClass::get(c"UIView")?, alloc], initWithFrame: bounds];
        let layer: Retained<AnyObject> = msg_send![&*view, layer];
        let down = gradient(down, bounds, false)?;
        if let Some(across) = across { let _: () = msg_send![&*down, setMask: &*gradient(across, bounds, true)?]; }
        let _: () = msg_send![&*layer, addSublayer: &*down];
        Some(view)
    }
}

/// A white `CAGradientLayer` over `bounds` whose alpha follows `stops`, across or down.
unsafe fn gradient(stops: &Stops, bounds: CGRect, across: bool) -> Option<Retained<AnyObject>> {
    // SAFETY: Core Animation, UIKit and Foundation calls on the main thread; the arrays retain
    // the colours and numbers they hold.
    unsafe {
        let layer: Retained<AnyObject> = msg_send![AnyClass::get(c"CAGradientLayer")?, layer];
        let colors: Retained<AnyObject> = msg_send![AnyClass::get(c"NSMutableArray")?, array];
        let locations: Retained<AnyObject> = msg_send![AnyClass::get(c"NSMutableArray")?, array];
        let ui_color = AnyClass::get(c"UIColor")?;
        let number = AnyClass::get(c"NSNumber")?;
        for &[at, alpha] in stops {
            let color: Retained<AnyObject> = msg_send![ui_color, colorWithWhite: 1.0f64, alpha: alpha];
            let cg: *mut CGColor = msg_send![&*color, CGColor];
            let _: () = msg_send![&*colors, addObject: cg.cast::<AnyObject>()];
            let n: Retained<AnyObject> = msg_send![number, numberWithDouble: at];
            let _: () = msg_send![&*locations, addObject: &*n];
        }
        let _: () = msg_send![&*layer, setFrame: bounds];
        let _: () = msg_send![&*layer, setColors: &*colors];
        let _: () = msg_send![&*layer, setLocations: &*locations];
        let (start, end) = if across { (CGPoint { x: 0.0, y: 0.5 }, CGPoint { x: 1.0, y: 0.5 }) } else { (CGPoint { x: 0.5, y: 0.0 }, CGPoint { x: 0.5, y: 1.0 }) };
        let _: () = msg_send![&*layer, setStartPoint: start];
        let _: () = msg_send![&*layer, setEndPoint: end];
        Some(layer)
    }
}
