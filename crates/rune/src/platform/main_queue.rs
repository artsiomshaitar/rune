//! Work for the main thread from anywhere: the main dispatch queue, which the event loop drains.

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(queue: *const u8, context: *mut std::ffi::c_void, work: extern "C" fn(*mut std::ffi::c_void));
    #[cfg(target_os = "tvos")]
    fn dispatch_time(when: u64, delta: i64) -> u64;
    #[cfg(target_os = "tvos")]
    fn dispatch_after_f(when: u64, queue: *const u8, context: *mut std::ffi::c_void, work: extern "C" fn(*mut std::ffi::c_void));
}

type Job = Box<dyn FnOnce() + Send>;

extern "C" fn run(context: *mut std::ffi::c_void) {
    // SAFETY: `context` is the box `on_main` or `on_main_after` leaked, delivered exactly once.
    let job = unsafe { Box::from_raw(context.cast::<Job>()) };
    job();
}

/// Run `f` on the main queue, which the window event loop drains.
pub(crate) fn on_main(f: impl FnOnce() + Send + 'static) {
    let job: Job = Box::new(f);
    let context = Box::into_raw(Box::new(job)).cast();
    // SAFETY: the main queue outlives the process; `run` frees `context`.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, context, run) };
}

/// Run `f` on the main queue `secs` seconds from now.
#[cfg(target_os = "tvos")]
pub(crate) fn on_main_after(secs: f64, f: impl FnOnce() + Send + 'static) {
    let job: Job = Box::new(f);
    let context = Box::into_raw(Box::new(job)).cast();
    let delta = (secs.max(0.0) * 1e9) as i64;
    // SAFETY: as `on_main`; `dispatch_time(DISPATCH_TIME_NOW, delta)` is `delta` ns from now.
    unsafe { dispatch_after_f(dispatch_time(0, delta), &raw const _dispatch_main_q, context, run) };
}
