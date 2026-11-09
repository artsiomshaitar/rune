//! A row of cards on Apple TV: the remote's arrows move the focus, Select presses the focused
//! card, and Menu on this, the only screen, goes back to the Home screen.
//!
//! `scripts/tvos-app examples/tvos-hello` builds it, bundles it and launches it on the tvOS
//! simulator. On a Mac, `cargo run -p tvos-hello` opens the same view in a window, where the
//! keyboard's arrows, Space and Escape stand in for the remote.

use rune::tok::motion;
use rune::{AppSpec, Color, Cx, El, Key, KeyEvent, TextStyle, Tx, col, el, oklch, row, text};

const CARDS: usize = 6;

#[derive(Default)]
struct State {
    focus: usize,
    presses: [u32; CARDS],
}

fn card(s: &State, i: usize) -> El {
    let focused = s.focus == i;
    el().key(format!("card-{i}")).size(240.0, 360.0).radius(18.0)
        .fill(oklch(0.42, 0.11, 40.0 + 50.0 * i as f32))
        .scale(if focused { 1.1 } else { 1.0 })
        .shadow(if focused { 24.0 } else { 6.0 }, if focused { 48.0 } else { 12.0 }, Color::BLACK.alpha(0.5))
        .motion(motion::SNAPPY)
        .child(col().w_full().h_full().center().gap(8.0)
            .child(text(format!("{}", i + 1)).font(TextStyle::new(72.0, 700.0)).color(Color::WHITE))
            .child(text(format!("{} presses", s.presses[i])).font(TextStyle::new(29.0, 500.0)).color(Color::WHITE.alpha(0.8))))
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut cards = row().gap(48.0);
    for i in 0..CARDS {
        cards = cards.child(card(s, i));
    }
    col().w_full().h_full().center().gap(64.0).fill(oklch(0.16, 0.02, 265.0))
        .child(text("Rune on Apple TV").font(TextStyle::new(57.0, 700.0)).color(Color::WHITE))
        .child(cards)
}

fn on_key(s: &mut State, ev: &KeyEvent, _: &mut Tx) -> bool {
    match &ev.key {
        Key::Left => s.focus = s.focus.saturating_sub(1),
        Key::Right => s.focus = (s.focus + 1).min(CARDS - 1),
        Key::Text(t) if t == " " => s.presses[s.focus] += 1,
        _ => return false,
    }
    true
}

fn spec() -> AppSpec<State> {
    AppSpec { on_key: Some(on_key), ..AppSpec::new("Rune on Apple TV", (1920.0, 1080.0), State::default, view) }
}

fn main() {
    #[cfg(target_os = "tvos")]
    rune::tvos::TvApp::new(spec()).menu_exits(|_| true).run();
    #[cfg(not(target_os = "tvos"))]
    rune::run(spec());
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Mods, Runtime};
    use rune::remote::{Button, Remote, Input, Effect};

    #[test]
    fn remote_presses_move_the_focus_and_press_the_card() {
        let env = Env { system_dark: true, scale: 1.0 };
        let mut rt = Runtime::new(State::default(), view, (1920.0, 1080.0), env);
        rt.on_key = Some(on_key);
        rt.frame(0.0);
        let mut remote = Remote::new();
        for input in [Input::Press(Button::Right), Input::Release(Button::Right), Input::Press(Button::Select)] {
            for effect in remote.step(input, 0.0) {
                if let Effect::Key(ev) = effect { rt.key(ev) }
            }
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.focus, 1);
        assert_eq!(rt.state.presses[1], 1);
        rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.focus, 0);
    }
}
