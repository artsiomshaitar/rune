//! System backdrop materials (`El::system_material`): the platform blurs what is behind the
//! window under an element's rounded rect. On macOS a native view per material sits under the
//! transparent Metal layer and moves in the same Core Animation transaction as the drawable,
//! so the material, Rune's shadow and the content on top present together. On tvOS the same
//! holds for a `UIVisualEffectView` under an app's layer while it is transparent (an underlay
//! is set), and there the view also follows the element's clips, soft ones included. Everywhere
//! else, in opaque windows and in headless renders, the element's fill (or a neutral tint)
//! stands in.

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "tvos")]
pub(crate) mod uikit;
#[cfg(test)]
mod tests;

use crate::Color;
use rune_motion::NodeId;

/// A backdrop material the system composites behind a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SystemMaterial {
    /// Liquid Glass (`NSGlassEffectView`, regular style) on macOS 26 and later; the popover
    /// material on earlier macOS. On tvOS, `UIGlassEffect` (regular) from tvOS 26, and the
    /// regular blur before it.
    Glass,
    /// The popover vibrancy material (`NSVisualEffectView`, `.popover`, behind window). On tvOS,
    /// the regular `UIBlurEffect`.
    Popover,
}

/// Native material views per window. Elements past this many draw their fallback instead.
pub const MAX_NATIVE_MATERIALS: usize = 4;

/// One material element as built this frame, in physical pixels like the instances.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MaterialDraw {
    pub id: NodeId,
    pub material: SystemMaterial,
    /// x, y, width, height.
    pub rect: [f32; 4],
    /// Corner radius, already clamped to half the shorter side.
    pub radius: f32,
    /// Continuous corners (`El::continuous_corners`).
    pub continuous: bool,
    /// The element's effective opacity, ancestors included.
    pub opacity: f32,
    /// The clip on the element from its ancestors (x0, y0, x1, y1), and its soft edges: the fade
    /// widths (left, top, right, bottom; negative for a linear ramp) and where each ramp starts.
    pub clip: [f32; 4],
    pub fade: [f32; 4],
    pub fade_origin: [f32; 4],
}

/// Alpha along one axis of a native view, as gradient stops: the location, from 0 at the view's
/// start to 1 at its end, and the alpha there. Two stops at one location are a hard edge.
pub(crate) type Stops = Vec<[f64; 2]>;

/// How many straight pieces a smoothstep ramp becomes in a gradient; a linear one needs one.
const RAMP_PIECES: usize = 8;

impl MaterialDraw {
    /// The native view's frame (x, y, width, height from the top left) and corner radius, in points.
    #[cfg_attr(not(any(target_os = "macos", target_os = "tvos")), allow(dead_code))]
    pub(crate) fn points(&self, scale: f32) -> ([f64; 4], f64) {
        let s = f64::from(scale.max(1e-3));
        (self.rect.map(|v| f64::from(v) / s), f64::from(self.radius) / s)
    }

    /// What of the view its clip shows, across (x) and down (y), as a mask's gradient stops:
    /// `None` for an axis the clip leaves whole. Rounded clips count as their rectangle.
    #[cfg_attr(not(target_os = "tvos"), allow(dead_code))]
    pub(crate) fn mask(&self) -> (Option<Stops>, Option<Stops>) {
        let axis = |e: usize| {
            let (lo, len) = (self.rect[e], self.rect[e + 2]);
            stops(lo, len, [self.clip[e], self.clip[e + 2]], [self.fade[e], self.fade[e + 2]], [self.fade_origin[e], self.fade_origin[e + 2]])
        };
        (axis(0), axis(1))
    }

    /// The mask a native view takes where its own alpha can't fade it (UIKit's effect views):
    /// `mask` with the opacity folded into the stops down the view. `None` when it shows whole.
    #[cfg_attr(not(target_os = "tvos"), allow(dead_code))]
    pub(crate) fn faded_mask(&self) -> Option<(Option<Stops>, Stops)> {
        let (across, down) = self.mask();
        let opacity = f64::from(self.opacity.clamp(0.0, 1.0));
        (across.is_some() || down.is_some() || opacity < 1.0).then(|| {
            let down = down.unwrap_or_else(|| vec![[0.0, 1.0], [1.0, 1.0]]);
            (across, down.into_iter().map(|[at, a]| [at, a * opacity]).collect())
        })
    }
}

/// One axis of `MaterialDraw::mask`: a view from `lo` over `len`, cut hard outside `clip` and
/// ramping in over `fade` from each `origin`, as `clip.wgsl` draws a soft clip.
fn stops(lo: f32, len: f32, clip: [f32; 2], fade: [f32; 2], origin: [f32; 2]) -> Option<Stops> {
    if len <= 0.0 { return None; }
    let hi = lo + len;
    let ramp = |d: f32, w: f32| {
        if w == 0.0 { return 1.0; }
        let x = (d / w.abs()).clamp(0.0, 1.0);
        if w < 0.0 { x } else { x * x * (3.0 - 2.0 * x) }
    };
    let alpha = |p: f32| if p < clip[0] || p > clip[1] { 0.0 } else { ramp(p - origin[0], fade[0]) * ramp(origin[1] - p, fade[1]) };
    let mut at = vec![lo, hi, clip[0], clip[1]];
    for (o, w, sign) in [(origin[0], fade[0], 1.0), (origin[1], fade[1], -1.0)] {
        let pieces = if w < 0.0 { 1 } else { RAMP_PIECES };
        if w != 0.0 { at.extend((0..=pieces).map(|k| o + sign * w.abs() * k as f32 / pieces as f32)); }
    }
    at.retain(|p| (lo..=hi).contains(p));
    at.sort_by(f32::total_cmp);
    at.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    const EPS: f32 = 1e-3;
    let mut out = Stops::new();
    for p in at {
        let loc = f64::from((p - lo) / len);
        let (before, after) = (alpha(p - EPS), alpha(p + EPS));
        if p > lo { out.push([loc, f64::from(before)]); }
        if p < hi && (p == lo || (after - before).abs() > 1e-4) { out.push([loc, f64::from(after)]); }
    }
    out.iter().any(|s| s[1] < 0.999).then_some(out)
}

/// A shell's native material views, which an embedded host moves around each present.
pub(crate) trait NativeHost {
    /// This frame's draws, before the drawable is acquired: the drawable presents in the views'
    /// transaction only when the layer said so as it was acquired.
    fn acquire(&mut self, draws: &[MaterialDraw]);
    /// Presents the frame (`present`) with the views matching `draws`.
    fn present(&mut self, draws: &[MaterialDraw], scale: f32, present: &mut dyn FnMut());
}

/// Which existing view each draw takes: the view that showed the same element, else a free view
/// of the same material, else none (the caller makes one). `views` holds each view's material
/// and the element it showed last; `draws` each draw's material and element.
#[cfg_attr(not(any(target_os = "macos", target_os = "tvos")), allow(dead_code))]
pub(crate) fn assign(views: &[(SystemMaterial, Option<NodeId>)], draws: &[(SystemMaterial, NodeId)]) -> Vec<Option<usize>> {
    let mut taken = vec![false; views.len()];
    let mut out = vec![None; draws.len()];
    for (slot, &(material, id)) in out.iter_mut().zip(draws) {
        if let Some(v) = (0..views.len()).find(|&v| !taken[v] && views[v] == (material, Some(id))) {
            taken[v] = true;
            *slot = Some(v);
        }
    }
    for (slot, &(material, _)) in out.iter_mut().zip(draws) {
        if slot.is_some() { continue; }
        if let Some(v) = (0..views.len()).find(|&v| !taken[v] && views[v].0 == material) {
            taken[v] = true;
            *slot = Some(v);
        }
    }
    out
}

/// The views to remove, last first, from views of these materials, `true` where spare: all but
/// the first spare of each material, so unmounting a few elements needn't keep their views.
#[cfg_attr(not(any(target_os = "macos", target_os = "tvos")), allow(dead_code))]
pub(crate) fn surplus(views: &[(SystemMaterial, bool)]) -> Vec<usize> {
    (0..views.len()).rev().filter(|&k| views[k].1 && views[..k].iter().any(|&(m, spare)| spare && m == views[k].0)).collect()
}

/// The Metal layer's `presentsWithTransaction`, which native materials, a moving window frame
/// and a live resize all need. One owner, so none turns it off under another.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PresentFlag {
    moving: bool,
    materials: bool,
    live: bool,
    written: bool,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl PresentFlag {
    /// Whether the layer presents with transactions.
    pub(crate) fn written(&self) -> bool {
        self.written
    }

    /// A window frame animation started or stopped. The value to write, when it changed.
    pub(crate) fn moving(&mut self, on: bool) -> Option<bool> {
        self.moving = on;
        self.write()
    }

    /// Native materials synced with the drawable switched on or off. The value to write, when it changed.
    pub(crate) fn materials(&mut self, on: bool) -> Option<bool> {
        self.materials = on;
        self.write()
    }

    /// The user started or stopped dragging the window's edge. The value to write, when it changed.
    pub(crate) fn live(&mut self, on: bool) -> Option<bool> {
        self.live = on;
        self.write()
    }

    fn write(&mut self) -> Option<bool> {
        let on = self.moving || self.materials || self.live;
        (on != self.written).then(|| { self.written = on; on })
    }
}

/// One corner radius for a rect: native materials take a single radius, so this uses the
/// largest corner, clamped to half the shorter side as Rune's own rounded rects are.
pub(crate) fn clamp_radius(radii: [f32; 4], width: f32, height: f32) -> f32 {
    radii.into_iter().fold(0.0f32, f32::max).min(width.min(height) * 0.5).max(0.0)
}

/// What a material element paints in Rune's own surface when no native view backs it: its
/// fill, or a neutral tint close to the material's average look when it has none.
pub(crate) fn fallback_fill(fill: Color, dark: bool) -> Color {
    if fill.a > 0.0 { return fill; }
    if dark { Color { r: 0.16, g: 0.16, b: 0.17, a: 0.94 } } else { Color { r: 0.96, g: 0.96, b: 0.97, a: 0.94 } }
}

/// How the shell hosts materials, from `RUNE_SYSTEM_MATERIAL`: unset for native views synced
/// with the drawable; `legacy` forces the pre-26 `NSVisualEffectView` path; `unsynced` moves
/// native views outside the drawable's transaction (to compare); `off` draws the fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Synced,
    Legacy,
    Unsynced,
    Off,
}

impl Mode {
    pub(crate) fn parse(value: Option<&str>) -> Mode {
        match value.map(str::trim) {
            Some("legacy") => Mode::Legacy,
            Some("unsynced") => Mode::Unsynced,
            Some("off" | "0") => Mode::Off,
            _ => Mode::Synced,
        }
    }

    #[cfg_attr(not(any(target_os = "macos", target_os = "tvos")), allow(dead_code))]
    pub(crate) fn from_env() -> Mode {
        Mode::parse(std::env::var("RUNE_SYSTEM_MATERIAL").ok().as_deref())
    }
}
