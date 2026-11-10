//! The Siri Remote as keys, for the tvOS shell ([`crate::tvos`]) and any host that embeds Rune on
//! Apple TV through [`LayerHost::remote`](crate::embed::LayerHost::remote).
//!
//! A press of a button sends its key ([`Button::key`]): the arrows (the touch surface's edges
//! clicked, or a keyboard's arrows in the simulator) as arrows, Select as Space, which presses
//! the focused control, and Menu as Escape. UIKit sends a held arrow once, so a held arrow or
//! page button repeats here: after [`REPEAT_DELAY`], then every [`REPEAT_INTERVAL`], as an OS
//! key repeat (`Tx::key_repeat` is true for it). A swipe on the touch surface sends one arrow
//! for every [`SWIPE_STEP`] points it travels along its main axis.
//!
//! [`Remote::step`] is the whole machine: input and the time in, keys out. A held button's
//! repeat is a timer: the step that starts it returns [`Effect::WakeAt`] with its attempt, and the
//! host feeds [`Input::Due`] back with that attempt when the time comes. A wake for a press that
//! has since been released, or replaced by another, carries an attempt the state no longer holds
//! and sends nothing.
use crate::machine::{Attempt, Effects, Seq};
use crate::{Key, KeyEvent, Mods};

/// How long an arrow is held before it repeats, in seconds.
pub const REPEAT_DELAY: f64 = 0.45;
/// Seconds between repeats of a held arrow.
pub const REPEAT_INTERVAL: f64 = 0.12;
/// How far a touch travels on the touch surface, in the view's points, for each arrow it sends.
/// tvOS reports an indirect touch in the coordinates of the view it lands on, so a 1920-point
/// wide screen takes about six steps from edge to edge.
pub const SWIPE_STEP: f32 = 320.0;

/// A Siri Remote button, as UIKit's `UIPress.PressType` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    Select,
    Menu,
    PlayPause,
    PageUp,
    PageDown,
}

impl Button {
    /// The key this button sends: arrows as arrows, Select as Space, Menu as Escape, the page
    /// buttons as Page Up and Page Down, and Play/Pause as [`Key::PlayPause`].
    pub fn key(self) -> Option<Key> {
        Some(match self {
            Self::Up => Key::Up,
            Self::Down => Key::Down,
            Self::Left => Key::Left,
            Self::Right => Key::Right,
            Self::Select => Key::Text(" ".into()),
            Self::Menu => Key::Escape,
            Self::PageUp => Key::PageUp,
            Self::PageDown => Key::PageDown,
            Self::PlayPause => Key::PlayPause,
        })
    }

    fn repeats(self) -> bool {
        matches!(self, Self::Up | Self::Down | Self::Left | Self::Right | Self::PageUp | Self::PageDown)
    }
}

/// What the remote did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Press(Button),
    Release(Button),
    /// The system took the presses back (`pressesCancelled`): a held arrow stops repeating.
    Cancel,
    /// A finger landed on the touch surface, at this point in the view.
    TouchBegan { x: f32, y: f32 },
    TouchMoved { x: f32, y: f32 },
    TouchEnded,
    /// The time an [`Effect::WakeAt`] asked for came, with its attempt.
    Due { attempt: Attempt },
}

/// What the host does for a step.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Send this key (`Runtime::key`).
    Key(KeyEvent),
    /// Send this key as an auto-repeat (`Runtime::repeat_key`).
    Repeat(KeyEvent),
    /// This key came up (`Runtime::key_up`).
    KeyUp(KeyEvent),
    /// Feed `Input::Due { attempt }` back at `due`, in the seconds `step` is given.
    WakeAt { attempt: Attempt, due: f64 },
    /// The repeat timer is gone (the arrow came up): the host needn't wake for it.
    CancelWake,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum Hold {
    #[default]
    Idle,
    Held { button: Button, attempt: Attempt, due: f64 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Swipe {
    #[default]
    Idle,
    /// A finger is down; `x` and `y` are where the next step is measured from.
    Tracking { x: f32, y: f32 },
}

/// The remote's held button and the touch in progress.
#[derive(Clone, Debug, Default)]
pub struct Remote {
    seq: Seq,
    hold: Hold,
    swipe: Swipe,
}

fn event(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

impl Remote {
    pub fn new() -> Self {
        Self::default()
    }

    /// One input at `now` (seconds, any monotonic origin), and the keys it sends.
    pub fn step(&mut self, input: Input, now: f64) -> Effects<Effect> {
        match input {
            Input::Press(button) => {
                let Some(key) = button.key() else { return Effects::none() };
                let mut out = Effects::one(Effect::Key(event(key)));
                if button.repeats() {
                    let (attempt, due) = (self.seq.next(), now + REPEAT_DELAY);
                    self.hold = Hold::Held { button, attempt, due };
                    out.push(Effect::WakeAt { attempt, due });
                }
                out
            }
            Input::Release(button) => {
                let mut out = button.key().map_or_else(Effects::none, |key| Effects::one(Effect::KeyUp(event(key))));
                if matches!(self.hold, Hold::Held { button: held, .. } if held == button) {
                    self.hold = Hold::Idle;
                    out.push(Effect::CancelWake);
                }
                out
            }
            Input::Cancel => match std::mem::take(&mut self.hold) {
                Hold::Held { .. } => Effects::one(Effect::CancelWake),
                Hold::Idle => Effects::none(),
            },
            Input::Due { attempt: a } => match self.hold {
                Hold::Held { button, attempt, due } if attempt == a && now >= due => {
                    let Some(key) = button.key() else { return Effects::none() };
                    // A late frame sends one repeat, not a burst to catch up.
                    let (attempt, due) = (self.seq.next(), now + REPEAT_INTERVAL);
                    self.hold = Hold::Held { button, attempt, due };
                    Effects::two(Effect::Repeat(event(key)), Effect::WakeAt { attempt, due })
                }
                // Woken early: the same timer, asked for again.
                Hold::Held { attempt, due, .. } if attempt == a => Effects::one(Effect::WakeAt { attempt, due }),
                _ => Effects::none(),
            },
            Input::TouchBegan { x, y } => {
                self.swipe = Swipe::Tracking { x, y };
                Effects::none()
            }
            Input::TouchMoved { x, y } => {
                let Swipe::Tracking { x: x0, y: y0 } = self.swipe else {
                    self.swipe = Swipe::Tracking { x, y };
                    return Effects::none();
                };
                let (dx, dy) = (x - x0, y - y0);
                let horizontal = dx.abs() >= dy.abs();
                let travel = if horizontal { dx } else { dy };
                let steps = (travel.abs() / SWIPE_STEP).floor() as u32;
                if steps == 0 {
                    return Effects::none();
                }
                let key = match (horizontal, travel > 0.0) {
                    (true, true) => Key::Right,
                    (true, false) => Key::Left,
                    (false, true) => Key::Down,
                    (false, false) => Key::Up,
                };
                // The cross axis starts over too, so a long diagonal drift never turns.
                let used = steps as f32 * SWIPE_STEP * travel.signum();
                self.swipe = if horizontal { Swipe::Tracking { x: x0 + used, y } } else { Swipe::Tracking { x, y: y0 + used } };
                let mut out = Effects::none();
                for _ in 0..steps {
                    out.push(Effect::Key(event(key.clone())));
                    out.push(Effect::KeyUp(event(key.clone())));
                }
                out
            }
            Input::TouchEnded => {
                self.swipe = Swipe::Idle;
                Effects::none()
            }
        }
    }
}

/// A [`Remote`] feeding a [`Runtime`](crate::Runtime): its keys go in as the OS's would, and
/// the repeat it waits for is kept until [`Feed::poll`] finds it due. Times are seconds from
/// any one origin.
#[cfg_attr(not(any(test, target_vendor = "apple")), allow(dead_code))]
#[derive(Debug, Default)]
pub(crate) struct Feed {
    remote: Remote,
    wake: Option<(Attempt, f64)>,
}

#[cfg_attr(not(any(test, target_vendor = "apple")), allow(dead_code))]
impl Feed {
    pub(crate) fn input<S: 'static>(&mut self, rt: &mut crate::Runtime<S>, input: Input, now: f64) {
        rt.remote_drives_focus();
        for effect in self.remote.step(input, now) {
            match effect {
                Effect::Key(ev) => rt.key(ev),
                Effect::Repeat(ev) => rt.repeat_key(ev),
                Effect::KeyUp(ev) => {
                    // A release with no `on_key_up` handler needs no frame, so nothing to queue.
                    let _ = rt.key_up(ev);
                }
                Effect::WakeAt { attempt, due } => self.wake = Some((attempt, due)),
                Effect::CancelWake => self.wake = None,
            }
        }
    }

    /// Feeds the waiting repeat back when it is due.
    pub(crate) fn poll<S: 'static>(&mut self, rt: &mut crate::Runtime<S>, now: f64) {
        if let Some((attempt, due)) = self.wake
            && now >= due
        {
            self.wake = None;
            self.input(rt, Input::Due { attempt }, now);
        }
    }

    /// When [`Feed::poll`] has a repeat to send next.
    pub(crate) fn wake_at(&self) -> Option<f64> {
        self.wake.map(|(_, due)| due)
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
