use super::*;

fn steps(effects: Effects<Effect>) -> Vec<Effect> {
    effects.into_iter().collect()
}

fn ev(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

/// The repeat timer a step asked for.
fn wake(effects: &[Effect]) -> (Attempt, f64) {
    effects.iter().find_map(|e| match *e { Effect::WakeAt { attempt, due } => Some((attempt, due)), _ => None }).expect("a wake")
}

#[test]
fn buttons_send_their_keys() {
    assert_eq!(Button::Select.key(), Some(Key::Text(" ".into())));
    assert_eq!(Button::Menu.key(), Some(Key::Escape));
    assert_eq!(Button::Left.key(), Some(Key::Left));
    assert_eq!(Button::PageDown.key(), Some(Key::PageDown));
    assert_eq!(Button::PlayPause.key(), None);
}

#[test]
fn a_held_arrow_repeats_after_the_delay_then_at_the_interval() {
    let mut r = Remote::new();
    let pressed = steps(r.step(Input::Press(Button::Right), 1.0));
    assert_eq!(pressed[0], Effect::Key(ev(Key::Right)));
    let (attempt, due) = wake(&pressed);
    assert_eq!(due, 1.0 + REPEAT_DELAY);

    let repeated = steps(r.step(Input::Due { attempt }, due));
    assert_eq!(repeated[0], Effect::Repeat(ev(Key::Right)));
    let (next, next_due) = wake(&repeated);
    assert_ne!(next, attempt, "each repeat is a new timer");
    assert_eq!(next_due, due + REPEAT_INTERVAL);

    assert_eq!(steps(r.step(Input::Release(Button::Right), due + 0.05)), [Effect::KeyUp(ev(Key::Right)), Effect::CancelWake]);
    assert!(steps(r.step(Input::Due { attempt: next }, next_due)).is_empty(), "released: the timer is stale");
}

#[test]
fn an_early_wake_asks_for_the_same_timer_again() {
    let mut r = Remote::new();
    let (attempt, due) = wake(&steps(r.step(Input::Press(Button::Up), 0.0)));
    assert_eq!(steps(r.step(Input::Due { attempt }, due / 2.0)), [Effect::WakeAt { attempt, due }]);
}

#[test]
fn a_late_wake_sends_one_repeat_not_a_burst() {
    let mut r = Remote::new();
    let (attempt, _) = wake(&steps(r.step(Input::Press(Button::Down), 0.0)));
    let late = steps(r.step(Input::Due { attempt }, 5.0));
    assert_eq!(late.iter().filter(|e| matches!(e, Effect::Repeat(_))).count(), 1);
    assert_eq!(wake(&late).1, 5.0 + REPEAT_INTERVAL);
}

#[test]
fn a_new_press_makes_the_old_timer_stale() {
    let mut r = Remote::new();
    let (first, due) = wake(&steps(r.step(Input::Press(Button::Left), 0.0)));
    let _ = r.step(Input::Press(Button::Right), 0.1);
    assert!(steps(r.step(Input::Due { attempt: first }, due)).is_empty());
}

#[test]
fn select_and_menu_never_repeat() {
    let mut r = Remote::new();
    assert_eq!(steps(r.step(Input::Press(Button::Select), 0.0)), [Effect::Key(ev(Key::Text(" ".into())))]);
    assert_eq!(steps(r.step(Input::Press(Button::Menu), 0.0)), [Effect::Key(ev(Key::Escape))]);
    assert!(steps(r.step(Input::Press(Button::PlayPause), 0.0)).is_empty());
}

#[test]
fn releasing_another_button_keeps_the_held_arrow_and_cancel_stops_it() {
    let mut r = Remote::new();
    let (attempt, due) = wake(&steps(r.step(Input::Press(Button::Left), 0.0)));
    let _ = r.step(Input::Press(Button::Select), 0.1);
    assert_eq!(steps(r.step(Input::Release(Button::Select), 0.2)), [Effect::KeyUp(ev(Key::Text(" ".into())))], "the timer stays");
    assert!(!steps(r.step(Input::Due { attempt }, due)).is_empty(), "still held");

    let (attempt, due) = wake(&steps(r.step(Input::Press(Button::Left), 1.0)));
    assert_eq!(steps(r.step(Input::Cancel, 1.1)), [Effect::CancelWake]);
    assert!(steps(r.step(Input::Cancel, 1.2)).is_empty(), "nothing left to cancel");
    assert!(steps(r.step(Input::Due { attempt }, due)).is_empty());
}

#[test]
fn a_swipe_sends_an_arrow_per_step_along_its_main_axis() {
    let mut r = Remote::new();
    assert!(steps(r.step(Input::TouchBegan { x: 960.0, y: 540.0 }, 0.0)).is_empty());
    assert!(steps(r.step(Input::TouchMoved { x: 960.0 + SWIPE_STEP - 1.0, y: 560.0 }, 0.0)).is_empty());
    assert_eq!(steps(r.step(Input::TouchMoved { x: 960.0 + SWIPE_STEP + 10.0, y: 580.0 }, 0.0)),
        [Effect::Key(ev(Key::Right)), Effect::KeyUp(ev(Key::Right))]);
    // The next step is measured from where the last one landed, not from the touch's start.
    assert!(steps(r.step(Input::TouchMoved { x: 960.0 + 2.0 * SWIPE_STEP - 20.0, y: 580.0 }, 0.0)).is_empty());
    assert_eq!(steps(r.step(Input::TouchMoved { x: 960.0 + 2.0 * SWIPE_STEP, y: 580.0 }, 0.0)).len(), 2);
}

#[test]
fn a_fast_swipe_sends_every_step_it_covered() {
    let mut r = Remote::new();
    let _ = r.step(Input::TouchBegan { x: 0.0, y: 1000.0 }, 0.0);
    let out = steps(r.step(Input::TouchMoved { x: 10.0, y: 1000.0 - 3.0 * SWIPE_STEP }, 0.0));
    assert_eq!(out.iter().filter(|e| **e == Effect::Key(ev(Key::Up))).count(), 3);
}

#[test]
fn a_lifted_finger_starts_over() {
    let mut r = Remote::new();
    let _ = r.step(Input::TouchBegan { x: 0.0, y: 0.0 }, 0.0);
    let _ = r.step(Input::TouchMoved { x: SWIPE_STEP - 10.0, y: 0.0 }, 0.0);
    let _ = r.step(Input::TouchEnded, 0.0);
    let _ = r.step(Input::TouchBegan { x: 0.0, y: 0.0 }, 0.0);
    assert!(steps(r.step(Input::TouchMoved { x: 20.0, y: 0.0 }, 0.0)).is_empty(), "no travel carried over");
}

#[test]
fn a_move_without_a_start_starts_tracking() {
    let mut r = Remote::new();
    assert!(steps(r.step(Input::TouchMoved { x: 500.0, y: 500.0 }, 0.0)).is_empty());
    assert_eq!(steps(r.step(Input::TouchMoved { x: 500.0 - SWIPE_STEP, y: 500.0 }, 0.0)).first(), Some(&Effect::Key(ev(Key::Left))));
}

mod feed {
    use super::super::{Button, Feed, Input, REPEAT_DELAY, REPEAT_INTERVAL};
    use crate::{Cx, El, Env, Key, KeyEvent, Runtime, Tx, col};

    #[derive(Default)]
    struct Heard {
        keys: Vec<(Key, bool)>,
    }

    fn view(_: &Heard, _: &mut Cx<Heard>) -> El {
        col().w_full().h_full()
    }

    fn on_key(s: &mut Heard, ev: &KeyEvent, tx: &mut Tx) -> bool {
        s.keys.push((ev.key.clone(), tx.key_repeat()));
        true
    }

    fn runtime() -> Runtime<Heard> {
        let mut rt = Runtime::new(Heard::default(), view, (1920.0, 1080.0), Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(on_key);
        rt.frame(0.0);
        rt
    }

    #[test]
    fn presses_reach_the_app_as_keys_and_a_held_arrow_as_repeats() {
        let mut rt = runtime();
        let mut feed = Feed::default();
        feed.input(&mut rt, Input::Press(Button::Right), 0.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.keys, [(Key::Right, false)]);
        assert_eq!(feed.wake_at(), Some(REPEAT_DELAY));

        feed.poll(&mut rt, REPEAT_DELAY / 2.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.keys.len(), 1, "not due yet");

        feed.poll(&mut rt, REPEAT_DELAY);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.keys.last(), Some(&(Key::Right, true)), "the repeat is an OS-style repeat");
        assert_eq!(feed.wake_at(), Some(REPEAT_DELAY + REPEAT_INTERVAL));

        feed.input(&mut rt, Input::Release(Button::Right), REPEAT_DELAY + 0.05);
        assert_eq!(feed.wake_at(), None);
        feed.poll(&mut rt, 10.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.keys.len(), 2, "released: no more repeats");
    }

    #[test]
    fn select_and_menu_reach_the_app_as_space_and_escape() {
        let mut rt = runtime();
        let mut feed = Feed::default();
        feed.input(&mut rt, Input::Press(Button::Select), 0.0);
        feed.input(&mut rt, Input::Press(Button::Menu), 0.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.keys, [(Key::Text(" ".into()), false), (Key::Escape, false)]);
        assert_eq!(feed.wake_at(), None);
    }
}
