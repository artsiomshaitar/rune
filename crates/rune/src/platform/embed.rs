//! Embedded `CAMetalLayer` host for UIKit, SwiftUI and AppKit views that own their own window
//! and event loop (e.g. a `UIViewRepresentable` driven by `CADisplayLink` on iOS).

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::remote::{self, Feed};
use crate::render::{Gpu, Renderer};
use crate::runtime::{Env, Runtime, Waker};
use crate::system_material::{MaterialDraw, NativeHost};
use crate::time::Instant;
use crate::tooling::{AppSpec, runtime_for};

static SHARED_GPU: OnceLock<Result<Arc<Gpu>, String>> = OnceLock::new();

fn shared_gpu() -> Result<Arc<Gpu>, String> {
    SHARED_GPU
        .get_or_init(|| Gpu::try_new(Gpu::instance(), None).map(Arc::new))
        .clone()
}

/// Warms the shared Metal device and compiles the default `Bgra8Unorm` uber pipeline on a
/// background thread so the first embedded `LayerHost` opens without shader-compile latency.
pub fn prewarm() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new().name("rune-embed-prewarm".into()).spawn(|| {
            if let Ok(gpu) = shared_gpu() {
                let _ = Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
            }
        });
    });
}

/// Outcome of one [`LayerHost::frame`] pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbedFrame {
    /// Whether a drawable was acquired and presented.
    pub drew: bool,
    /// True while springs, reveals, pending input or live bindings need another display-link tick.
    pub animating: bool,
    /// Seconds until the next scheduled timer wake (`None` when none is pending).
    pub wake_after_secs: Option<f64>,
    /// CPU time spent stepping layout/motion and encoding GPU work, in microseconds.
    pub cpu_us: u64,
    /// The frame drew nothing over its clear colour: no element and no compositor. On a
    /// transparent host ([`LayerHost::set_transparent`]) that is a wholly clear layer.
    pub blank: bool,
}

/// Drives one Rune [`Runtime`] and [`Renderer`] directly onto a `CAMetalLayer`.
///
/// Pause display-link calls while the host app is backgrounded on iOS: Metal forbids drawable
/// acquisition and command-buffer submission after `didEnterBackground`.
pub struct LayerHost<S: 'static> {
    gpu: Arc<Gpu>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    compositor: Option<Box<dyn Compositor<S>>>,
    rt: Runtime<S>,
    clock: FrameClock,
    suspended: bool,
    remote: Feed,
    start: Instant,
}

impl<S: 'static> LayerHost<S> {
    /// Attaches a new Rune runtime and Metal surface to `layer` (`CAMetalLayer *`), blending in
    /// sRGB space (`srgb_blend = true`, matching Apple's native text compositing).
    ///
    /// # Safety
    /// `layer` must be a valid `CAMetalLayer` pointer that outlives this `LayerHost`, and surface
    /// creation / presentation must run on a thread allowed to touch that layer (the UI thread).
    pub unsafe fn from_core_animation_layer(
        layer: *mut c_void,
        spec: &AppSpec<S>,
        state: S,
        viewport: (f32, f32),
        scale: f32,
        dark: bool,
        wake: Waker,
    ) -> Result<Self, String> {
        if layer.is_null() {
            return Err("null CAMetalLayer pointer".into());
        }
        let gpu = shared_gpu()?;
        let surface = unsafe {
            gpu.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
                .map_err(|e| format!("create_surface_unsafe: {e}"))?
        };
        let scale = scale.max(1.0);
        let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let srgb_blend = true;
        let (config, _) = crate::window::surface_config(&surface, &gpu.adapter, (px_w, px_h), srgb_blend)
            .ok_or_else(|| "CAMetalLayer has no supported wgpu surface configuration".to_owned())?;
        surface.configure(&gpu.device, &config);
        let target_format = config.format.remove_srgb_suffix();
        let mut renderer = Renderer::new(&gpu.device, target_format);
        renderer.set_srgb_blend(true);

        let w = px_w as f32 / scale;
        let h = px_h as f32 / scale;
        let mut rt = runtime_for(spec, state, (w, h), Env::new(dark, scale), wake);
        rt.set_srgb_blend(true);
        rt.set_os_drag(false);
        let now = Instant::now();
        rt.wall_start = Some(now);
        rt.record_events = false;
        // `draw` below clears to `clear_color`, so the root's own fill needn't paint again.
        rt.root_cleared = true;

        Ok(Self {
            gpu,
            surface,
            config,
            renderer,
            compositor: None,
            rt,
            clock: FrameClock::new(now),
            suspended: viewport.0 <= 0.0 || viewport.1 <= 0.0,
            remote: Feed::default(),
            start: now,
        })
    }

    /// Attaches a custom [`Compositor`] (e.g. for [`El::effect`](crate::El::effect) shader passes)
    /// and turns on `Runtime::track_marks` so `rt.marks` aligns with `rt.instances`.
    pub fn with_compositor(mut self, compositor: impl Compositor<S>) -> Self {
        self.rt.track_marks = true;
        self.compositor = Some(Box::new(compositor));
        self
    }

    /// Clears each frame to transparent instead of the app root's fill, and makes the layer
    /// non-opaque, so whatever lies beneath it shows wherever nothing is drawn: a native video
    /// layer under the controls drawn over it. A fill on the app root still paints, so the app
    /// leaves it off while it wants to see through. Off again, the layer is opaque and clears to
    /// the root's fill. Returns whether the layer can composite translucent pixels at all.
    pub fn set_transparent(&mut self, on: bool) -> bool {
        let translucent = crate::render::translucency::mode(&self.surface, &self.gpu.adapter);
        let mode = translucent.filter(|_| on).unwrap_or(wgpu::CompositeAlphaMode::Opaque);
        if self.config.alpha_mode != mode {
            self.config.alpha_mode = mode;
            self.surface.configure(&self.gpu.device, &self.config);
        }
        self.rt.transparent = mode != wgpu::CompositeAlphaMode::Opaque;
        self.rt.invalidate();
        !on || translucent.is_some()
    }

    pub fn runtime(&self) -> &Runtime<S> {
        &self.rt
    }

    pub fn runtime_mut(&mut self) -> &mut Runtime<S> {
        &mut self.rt
    }

    /// Updates the viewport in points and display scale, reconfiguring the Metal swapchain only
    /// when its clamped device-pixel dimensions change.
    pub fn resize(&mut self, viewport: (f32, f32), scale: f32) {
        if viewport.0 <= 0.0 || viewport.1 <= 0.0 {
            self.suspended = true;
            return;
        }
        self.suspended = false;
        let scale = scale.max(1.0);
        let max_dim = self.gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0 * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1 * scale).round() as u32).clamp(1, max_dim);
        if self.config.width != px_w || self.config.height != px_h {
            self.config.width = px_w;
            self.config.height = px_h;
            self.surface.configure(&self.gpu.device, &self.config);
        }
        self.rt.set_viewport((px_w as f32 / scale, px_h as f32 / scale), scale);
    }

    /// Siri Remote input, as keys (`rune::remote`): call it from `pressesBegan`/`pressesEnded`
    /// and the indirect touches of the hosting view, then request a frame. A held arrow repeats
    /// from [`LayerHost::frame`], whose `wake_after_secs` includes the next repeat.
    pub fn remote(&mut self, input: remote::Input) {
        let now = self.seconds();
        self.remote.input(&mut self.rt, input, now);
    }

    /// Seconds since this host opened, the clock the remote's repeats run on.
    fn seconds(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Lets elements marked `El::system_material` leave their interior to native views, which
    /// the shell places from each frame's draws ([`LayerHost::frame_with`]).
    pub(crate) fn set_native_materials(&mut self, on: bool) {
        if self.rt.native_materials != on {
            self.rt.native_materials = on;
            self.rt.invalidate();
        }
    }

    /// Steps animations and layout to `Instant::now()` and presents one Metal frame.
    pub fn frame(&mut self) -> EmbedFrame {
        self.frame_with(None)
    }

    /// [`LayerHost::frame`], presenting with `materials` moved to this frame's material elements.
    pub(crate) fn frame_with(&mut self, mut materials: Option<&mut dyn NativeHost>) -> EmbedFrame {
        if self.suspended {
            return EmbedFrame::default();
        }
        let now = self.seconds();
        self.remote.poll(&mut self.rt, now);
        let t0 = Instant::now();
        let (animating, dt) = self.clock.advance(&mut self.rt, t0, false);
        let update_ms = t0.elapsed().as_secs_f64() * 1000.0;
        crate::clipboard::flush_memory(&mut self.rt);
        if self.rt.popup_menu().is_some() {
            self.rt.cancel_popups();
        }
        self.rt.refuse_file_dialog("native file dialogs are unsupported in embedded views");

        let draws: &[MaterialDraw] = if self.rt.native_materials { &self.rt.materials } else { &[] };
        if let Some(m) = materials.as_deref_mut() { m.acquire(draws) }
        let comp_animating = self.compositor.as_ref().is_some_and(|c| c.animating());
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                return EmbedFrame { animating: true, ..EmbedFrame::default() };
            }
            _ => return EmbedFrame { animating: animating || comp_animating, ..EmbedFrame::default() },
        };

        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.renderer.format()),
            ..Default::default()
        });
        self.renderer.upload_images(&self.gpu.device, &self.gpu.queue, &mut self.rt.images);
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let size = (self.config.width, self.config.height);
        let format = self.renderer.format();
        if let Some(comp) = &mut self.compositor {
            let mut f = Frame {
                device: &self.gpu.device,
                queue: &self.gpu.queue,
                encoder: &mut encoder,
                target: &target,
                format,
                size,
                dt,
                update_ms,
                rt: &mut self.rt,
                ui: &mut self.renderer,
            };
            comp.render(&mut f);
        } else {
            self.renderer.prepare(
                &self.gpu.device,
                &self.gpu.queue,
                &mut self.rt.text.atlas,
                &self.rt.instances,
                size,
            );
            self.renderer.draw(&mut encoder, &target, self.rt.clear_color());
        }
        self.renderer.submit(&self.gpu.queue, encoder);
        let mut frame = Some(frame);
        let mut present = || if let Some(f) = frame.take() { self.gpu.queue.present(f) };
        match materials {
            Some(m) => m.present(if self.rt.native_materials { &self.rt.materials } else { &[] }, self.rt.env.scale, &mut present),
            None => present(),
        }

        let cpu_us = t0.elapsed().as_micros() as u64;
        let wake_after_secs = self.rt.wake_deadline().and_then(|d| {
            let elapsed = self.rt.wall_start?.elapsed().as_secs_f64();
            Some((d - elapsed).max(0.0))
        });
        let remote_after = self.remote.wake_at().map(|due| (due - self.seconds()).max(0.0));
        let wake_after_secs = match (wake_after_secs, remote_after) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let comp_animating = self.compositor.as_ref().is_some_and(|c| c.animating());
        let keep_ticking = animating || comp_animating || self.rt.has_pending_input() || self.rt.live_pending();
        EmbedFrame {
            drew: true,
            animating: keep_ticking,
            wake_after_secs,
            cpu_us,
            blank: self.compositor.is_none() && self.rt.instances.is_empty(),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use objc2_video::{
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject},
    };

    use super::LayerHost;
    use crate::compose::{Compositor, Frame};
    use crate::render::KIND_EFFECT;
    use crate::tooling::AppSpec;
    use crate::{Cx, El, col, effect};

    struct MarkProbe(Arc<AtomicU32>);

    impl Compositor<()> for MarkProbe {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            for (idx, inst) in f.rt.instances.iter().enumerate() {
                if inst.params[1] == KIND_EFFECT {
                    self.0.store(f.rt.marks.get(idx).copied().unwrap_or(0), Ordering::Relaxed);
                }
            }
            f.draw_ui();
        }
    }

    #[test]
    fn layer_host_runs_compositor_with_tracked_marks() {
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let seen = Arc::new(AtomicU32::new(0));
        let view = |_: &(), _: &mut Cx<()>| -> El { col().size(120.0, 80.0).child(effect(42).size(60.0, 20.0)) };
        let spec = AppSpec::new("embed-test", (120.0, 80.0), || (), view);
        let host = unsafe {
            LayerHost::from_core_animation_layer(
                Retained::as_ptr(&layer) as *mut std::ffi::c_void,
                &spec,
                (),
                (120.0, 80.0),
                2.0,
                true,
                Arc::new(|| {}),
            )
        };
        let Ok(mut host) = host else { return };
        host = host.with_compositor(MarkProbe(seen.clone()));
        let out = host.frame();
        assert!(out.drew);
        assert_eq!(seen.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn a_held_remote_arrow_wakes_the_host_for_its_repeat() {
        use crate::remote::{Button, Input, REPEAT_DELAY};
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let view = |_: &u32, _: &mut Cx<u32>| -> El { col().size(120.0, 80.0) };
        let on_key = |s: &mut u32, _: &crate::KeyEvent, _: &mut crate::Tx| { *s += 1; true };
        let spec = AppSpec { on_key: Some(on_key), ..AppSpec::new("embed-remote", (120.0, 80.0), || 0, view) };
        let host = unsafe {
            LayerHost::from_core_animation_layer(Retained::as_ptr(&layer) as *mut std::ffi::c_void, &spec, 0, (120.0, 80.0), 2.0, true, Arc::new(|| {}))
        };
        let Ok(mut host) = host else { return };
        host.remote(Input::Press(Button::Down));
        let out = host.frame();
        assert_eq!(host.runtime().state, 1, "the press reached on_key");
        let wake = out.wake_after_secs.expect("a wake for the repeat");
        assert!(wake > 0.0 && wake <= REPEAT_DELAY, "{wake}");
        host.remote(Input::Release(Button::Down));
        assert_eq!(host.frame().wake_after_secs, None, "released: nothing to wake for");
    }

    #[derive(Default)]
    struct Views {
        acquired: Vec<usize>,
        presented: Vec<usize>,
    }

    impl crate::system_material::NativeHost for Views {
        fn acquire(&mut self, draws: &[crate::system_material::MaterialDraw]) {
            self.acquired.push(draws.len());
        }
        fn present(&mut self, draws: &[crate::system_material::MaterialDraw], _: f32, present: &mut dyn FnMut()) {
            self.presented.push(draws.len());
            present();
            present();
        }
    }

    #[test]
    fn native_materials_reach_the_shell_around_the_present() {
        use crate::{Color, SystemMaterial, el};
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let view = |_: &(), _: &mut Cx<()>| -> El {
            col().w_full().h_full().child(el().size(60.0, 40.0).fill(Color::WHITE).system_material(SystemMaterial::Popover))
        };
        let spec = AppSpec::new("embed-materials", (120.0, 80.0), || (), view);
        let host = unsafe {
            LayerHost::from_core_animation_layer(Retained::as_ptr(&layer) as *mut std::ffi::c_void, &spec, (), (120.0, 80.0), 2.0, true, Arc::new(|| {}))
        };
        let Ok(mut host) = host else { return };
        let mut views = Views::default();
        let out = host.frame_with(Some(&mut views));
        assert!(out.drew && !out.blank, "without native views the element paints its fill");
        assert_eq!((views.acquired.as_slice(), views.presented.as_slice()), (&[0][..], &[0][..]));

        host.set_native_materials(true);
        let out = host.frame_with(Some(&mut views));
        assert!(out.drew && out.blank, "over a native view the fill goes");
        assert_eq!((views.acquired.as_slice(), views.presented.as_slice()), (&[0, 1][..], &[0, 1][..]));

        host.set_native_materials(false);
        host.frame_with(Some(&mut views));
        assert_eq!(views.presented.last(), Some(&0), "turned off, the shell is told to hide its views");
    }

    #[test]
    fn a_transparent_host_clears_to_nothing_and_says_when_it_drew_nothing() {
        use crate::Color;
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let opaque = |layer: &AnyObject| -> bool { unsafe { msg_send![layer, isOpaque] } };
        let view = |filled: &bool, _: &mut Cx<bool>| -> El {
            let root = col().w_full().h_full();
            if *filled { root.fill(Color::BLACK) } else { root }
        };
        let spec = AppSpec::new("embed-transparent", (120.0, 80.0), || true, view);
        let host = unsafe {
            LayerHost::from_core_animation_layer(Retained::as_ptr(&layer) as *mut std::ffi::c_void, &spec, true, (120.0, 80.0), 2.0, true, Arc::new(|| {}))
        };
        let Ok(mut host) = host else { return };
        let out = host.frame();
        assert!(out.drew && out.blank, "an opaque host clears to the root's fill and skips painting it");
        assert!(opaque(&layer));
        assert_eq!(host.runtime().clear_color()[3], 1.0);

        assert!(host.set_transparent(true));
        assert!(!opaque(&layer));
        assert_eq!(host.runtime().clear_color(), [0.0; 4]);
        assert!(!host.frame().blank, "the root's fill paints over a transparent clear");
        host.runtime_mut().update(|filled, tx| { *filled = false; tx.instant() });
        assert!(host.frame().blank, "an unfilled root draws nothing");

        assert!(host.set_transparent(false));
        assert!(opaque(&layer));
        assert_eq!(host.runtime().clear_color()[3], 1.0);
    }
}
