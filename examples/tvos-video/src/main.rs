//! Video under Rune on Apple TV: an `AVPlayerLayer` plays an HLS stream beneath the app's layer
//! (`rune::tvos::set_underlay`), and Rune draws a control bar over it, on a frosted band
//! (`El::system_material`) whose top fades up into the picture, with two pills on Liquid Glass.
//! The system blurs the video under them. Select pauses and plays, Menu hides and shows the
//! controls, which fade; with them hidden Rune draws nothing, its layer is hidden and nothing
//! redraws.
//!
//! The first second is an opaque Rune screen, so the switch to a see-through layer happens at
//! run time, from `poll`. There's no scriptable remote in the simulator, so launch flags stand in
//! for presses: `-press-at <s>` presses Select and `-hide-at <s>` presses Menu, at that many
//! seconds in. `-slow <k>` runs the app's motion at `k` times normal speed, and `-dim <a>` shows
//! the controls at that opacity, to catch a fade in a screenshot.
//!
//! `SHOT=/tmp/v.png scripts/tvos-app tvos-video` builds, launches and screenshots it. On a Mac,
//! `cargo run -p tvos-video` opens the bar over nothing.

use std::time::Duration;

use rune::{AppSpec, Color, Cx, El, Key, KeyEvent, SystemMaterial, TextStyle, Tx, col, el, oklch, row, text};

const STREAM: &str = "https://devstreaming-cdn.apple.com/videos/streaming/examples/bipbop_16x9/bipbop_16x9_variant.m3u8";
const START_AT: f64 = 1.0;

#[derive(Default)]
struct State {
    started: bool,
    paused: bool,
    controls: bool,
    seconds: f64,
    /// When the clock on the bar next reads the player.
    tick_at: f64,
    press_at: Option<f64>,
    hide_at: Option<f64>,
    slow: Option<f64>,
    dim: Option<f64>,
}

fn flag(args: &[String], name: &str) -> Option<f64> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)?.parse().ok())
}

fn init() -> State {
    let args: Vec<String> = std::env::args().collect();
    State {
        controls: true, press_at: flag(&args, "-press-at"), hide_at: flag(&args, "-hide-at"),
        slow: flag(&args, "-slow"), dim: flag(&args, "-dim"), ..State::default()
    }
}

fn toggle_pause(s: &mut State) {
    s.paused = !s.paused;
    video::set_paused(s.paused);
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn bar(s: &State) -> El {
    let accent = if s.paused { oklch(0.78, 0.15, 75.0) } else { oklch(0.75, 0.14, 150.0) };
    row().w_full().h(140.0).px(48.0).gap(40.0).center_y().radius(32.0)
        .fill(Color::WHITE.alpha(0.12))
        .child(el().key("state").size(28.0, 28.0).radius(14.0).fill(accent))
        .child(col().gap(6.0)
            .child(text("Bip Bop").font(TextStyle::new(38.0, 700.0)).color(Color::WHITE))
            .child(text(if s.paused { "Paused" } else { "Playing" }).font(TextStyle::new(29.0, 500.0)).color(accent)))
        .child(el().grow(1.0).h(8.0).radius(4.0).fill(Color::WHITE.alpha(0.25)))
        .child(text(clock(s.seconds)).font(TextStyle::new(31.0, 600.0)).color(Color::WHITE))
}

fn pill(label: &str) -> El {
    el().h(64.0).px(32.0).center().radius(32.0).system_material(SystemMaterial::Glass)
        .child(text(label).font(TextStyle::new(29.0, 600.0)).color(Color::WHITE))
}

/// The controls: a frosted band, its top fading up into the picture, under the pills and the bar.
fn controls(s: &State) -> El {
    el().key("controls").w_full().h(460.0).opacity(s.dim.unwrap_or(1.0) as f32).clip_fade_linear(0.0, 220.0, 0.0, 0.0)
        .child(col().w_full().h_full().justify_end().gap(32.0).px(96.0).pb(80.0)
            .system_material(SystemMaterial::Popover)
            .child(row().gap(24.0).child(pill("Subtitles")).child(pill("Audio")))
            .child(bar(s)))
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    if !s.started {
        return col().w_full().h_full().center().fill(oklch(0.16, 0.02, 265.0))
            .child(text("Starting the stream").font(TextStyle::new(57.0, 700.0)).color(Color::WHITE));
    }
    let mut root = col().w_full().h_full().justify_end().time_scale(s.slow.unwrap_or(1.0) as f32);
    if s.controls { root = root.child(controls(s)) }
    root
}

fn on_key(s: &mut State, ev: &KeyEvent, _: &mut Tx) -> bool {
    match &ev.key {
        Key::Text(t) if t == " " => toggle_pause(s),
        Key::Escape => s.controls = !s.controls,
        _ => return false,
    }
    true
}

fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if !s.started && now >= START_AT {
        s.started = true;
        video::start(STREAM);
        changed = true;
    }
    if s.press_at.is_some_and(|t| now >= t) {
        s.press_at = None;
        toggle_pause(s);
        changed = true;
    }
    if s.hide_at.is_some_and(|t| now >= t) {
        s.hide_at = None;
        s.controls = false;
        changed = true;
    }
    // The clock reads the player once a second while it shows, and not at all while the bar
    // is hidden. Its wake is a deadline kept in the state: `poll` runs every frame, and a wake
    // a second after each one would keep the app drawing at the rate it last animated.
    if s.started && s.controls && now >= s.tick_at {
        s.tick_at = now + 1.0;
        if let Some(seconds) = video::seconds().filter(|t| t.floor() != s.seconds.floor()) {
            // A clock ticks rather than animates: a tick that eased in would draw a third of
            // every second.
            tx.instant();
            s.seconds = seconds;
            changed = true;
        }
    }
    let tick = (s.started && s.controls).then_some(s.tick_at);
    for due in [(!s.started).then_some(START_AT), s.press_at, s.hide_at, tick].into_iter().flatten() {
        tx.wake_after(Duration::from_secs_f64((due - now).max(0.0)));
    }
    changed
}

fn spec() -> AppSpec<State> {
    AppSpec { on_key: Some(on_key), poll: Some(poll), ..AppSpec::new("Video under Rune", (1920.0, 1080.0), init, view) }
}

fn main() {
    #[cfg(target_os = "tvos")]
    rune::tvos::TvApp::new(spec()).run();
    #[cfg(not(target_os = "tvos"))]
    rune::run(spec());
}

#[cfg(target_os = "tvos")]
mod video {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSString, NSURL};

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}

    #[repr(C)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }

    // SAFETY: CMTime's layout, as CoreMedia declares it.
    unsafe impl objc2::Encode for CMTime {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct("?", &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING]);
    }

    thread_local! {
        static PLAYER: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    }

    pub fn start(url: &str) {
        let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else { return };
        // SAFETY: AVFoundation's documented messages, sent on the main thread, where Rune's
        // `poll` runs.
        unsafe {
            let player: Retained<AnyObject> = msg_send![class!(AVPlayer), playerWithURL: &*url];
            let layer: Retained<AnyObject> = msg_send![class!(AVPlayerLayer), playerLayerWithPlayer: &*player];
            rune::tvos::set_underlay(Retained::as_ptr(&layer).cast_mut().cast());
            let _: () = msg_send![&*player, play];
            PLAYER.with(|p| *p.borrow_mut() = Some(player));
        }
    }

    pub fn set_paused(paused: bool) {
        PLAYER.with(|p| if let Some(player) = &*p.borrow() {
            // SAFETY: as in `start`.
            unsafe { if paused { let _: () = msg_send![&**player, pause]; } else { let _: () = msg_send![&**player, play]; } }
        });
    }

    pub fn seconds() -> Option<f64> {
        PLAYER.with(|p| {
            let player = p.borrow().clone()?;
            // SAFETY: as in `start`.
            let t: CMTime = unsafe { msg_send![&*player, currentTime] };
            (t.flags & 1 != 0 && t.timescale > 0).then(|| t.value as f64 / f64::from(t.timescale))
        })
    }
}

#[cfg(not(target_os = "tvos"))]
mod video {
    pub fn start(_: &str) {}
    pub fn set_paused(_: bool) {}
    pub fn seconds() -> Option<f64> { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Mods, Runtime};

    fn runtime(state: State) -> Runtime<State> {
        let mut rt = Runtime::new(state, view, (1920.0, 1080.0), Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(on_key);
        rt.poll = Some(poll);
        rt
    }

    #[test]
    fn the_stream_starts_a_second_in_and_scripted_presses_land_on_time() {
        let mut rt = runtime(State { controls: true, press_at: Some(3.0), hide_at: Some(5.0), ..State::default() });
        rt.frame(0.5);
        assert!(!rt.state.started);
        rt.frame(1.0);
        assert!(rt.state.started && !rt.state.paused);
        rt.frame(1.0);
        assert!(!rt.state.paused);
        rt.frame(1.0);
        assert!(rt.state.paused, "pressed at 3s");
        rt.frame(2.0);
        assert!(!rt.state.controls, "hidden at 5s");
    }

    #[test]
    fn select_pauses_and_menu_hides_the_bar() {
        let mut rt = runtime(State { started: true, controls: true, ..State::default() });
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.paused);
        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.controls);
    }

    #[test]
    fn launch_flags_read_seconds() {
        let args: Vec<String> = ["app", "-press-at", "8", "-hide-at", "x"].map(String::from).into();
        assert_eq!(flag(&args, "-press-at"), Some(8.0));
        assert_eq!(flag(&args, "-hide-at"), None);
        assert_eq!(clock(75.4), "1:15");
    }
}
