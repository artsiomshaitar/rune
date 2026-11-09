use super::*;
use crate::{el, Key, Mods};

const R: [f32; 4] = [100.0, 100.0, 100.0, 100.0];

fn rect(x: f32, y: f32) -> [f32; 4] {
    [x, y, 100.0, 100.0]
}

#[test]
fn score_takes_each_direction_and_only_its_half_plane() {
    use FocusDirection::*;
    for (dir, to) in [(Right, rect(240.0, 100.0)), (Left, rect(-40.0, 100.0)), (Down, rect(100.0, 240.0)), (Up, rect(100.0, -40.0))] {
        let s = score(R, to, dir, true).unwrap_or_else(|| panic!("{dir:?} reaches {to:?}"));
        assert_eq!((s.apart, s.distance, s.offset), (false, 40.0, 0.0), "{dir:?}");
        for other in [Right, Left, Down, Up].into_iter().filter(|&d| d != dir) {
            assert!(score(R, to, other, true).is_none(), "{to:?} isn't {other:?} of the source");
        }
    }
}

#[test]
fn the_half_plane_starts_at_the_centre_line() {
    // Overlapping the far half (a neighbour under a focused card drawn scaled up) still counts.
    assert_eq!(score(R, rect(160.0, 100.0), FocusDirection::Right, true).map(|s| s.distance), Some(0.0));
    // A neighbour level with the source, starting before its centre, is no move down.
    assert!(score(R, rect(240.0, 140.0), FocusDirection::Down, true).is_none());
    assert!(score(R, rect(240.0, 140.0), FocusDirection::Right, true).is_some());
    // Wholly inside the source, or behind it, never.
    assert!(score(R, [160.0, 120.0, 20.0, 20.0], FocusDirection::Right, true).is_none());
    assert!(score(R, rect(0.0, 100.0), FocusDirection::Right, true).is_none());
    // Unless the half-plane is waived, as entering a section does.
    assert!(score(R, rect(0.0, 100.0), FocusDirection::Right, false).is_some());
}

#[test]
fn cross_axis_overlap_beats_distance_then_centres_then_order() {
    let far_below = score(R, rect(100.0, 600.0), FocusDirection::Down, true).unwrap();
    let near_diagonal = score(R, rect(220.0, 210.0), FocusDirection::Down, true).unwrap();
    assert!(near_diagonal.apart && !far_below.apart);
    assert!(far_below < near_diagonal, "overlap wins however far");
    let nearer = score(R, rect(190.0, 240.0), FocusDirection::Down, true).unwrap();
    let farther = score(R, rect(100.0, 260.0), FocusDirection::Down, true).unwrap();
    assert!(nearer < farther, "then the nearer, even off-centre");
    let centred = score(R, rect(110.0, 240.0), FocusDirection::Down, true).unwrap();
    let off = score(R, rect(160.0, 240.2), FocusDirection::Down, true).unwrap();
    assert_eq!(centred.distance, off.distance, "distances compare to the half point");
    assert!(centred < off, "then the better centred");
    let apart = score(R, rect(230.0, 230.0), FocusDirection::Down, true).unwrap();
    assert_eq!(apart.distance, 42.5, "apart: the distance between closest edges, 30 by 30");
}

type Route = fn(&FocusMove<'_>) -> FocusTarget;

#[derive(Default)]
struct Grid {
    /// No `Cx::spatial_focus`.
    off: bool,
    sections: bool,
    pressed: Vec<String>,
    held: Vec<FocusDirection>,
    on_key: usize,
    /// The root's `on_focus_move`.
    route: Option<Route>,
    /// Each row's `on_focus_move`.
    row_route: Option<Route>,
    /// What the root's hook saw: direction, from, the default's key and from's section.
    moves: Vec<(FocusDirection, String, Option<String>, Option<String>)>,
    row_asked: usize,
}

fn card(key: String, x: f32, y: f32) -> El {
    let pressed = key.clone();
    el().key(key).absolute(x, y).size(200.0, 150.0).focusable(true).role("button")
        .on_press(move |s: &mut Grid| s.pressed.push(pressed.clone()))
}

/// A nav bar of two pills over two rows of four cards, each a section when `sections` is on.
/// Pills: home x 20..120, search 140..240, y 10..50. Cards: x 20 + 220c, 200 wide; row 0 at
/// y 100..250, row 1 at 300..450.
fn grid(s: &Grid, cx: &mut Cx<Grid>) -> El {
    if !s.off { cx.spatial_focus(); }
    let section = |e: El| if s.sections { e.focus_section() } else { e };
    let nav = section(el().key("nav").absolute(0.0, 0.0).size(1000.0, 60.0))
        .child(card("home".into(), 20.0, 10.0).size(100.0, 40.0))
        .child(card("search".into(), 140.0, 10.0).size(100.0, 40.0));
    let row_route = s.row_route;
    let row = |r: usize| {
        let mut row = section(el().key(format!("row{r}")).absolute(0.0, 100.0 + r as f32 * 200.0).size(1000.0, 150.0))
            .children((0..4).map(move |c| card(format!("c{r}{c}"), 20.0 + c as f32 * 220.0, 0.0)));
        if let Some(route) = row_route {
            row = row.on_focus_move(move |s: &mut Grid, mv: &FocusMove<'_>, _| { s.row_asked += 1; route(mv) });
        }
        row
    };
    let mut root = el().key("root").size(1000.0, 600.0).child(nav).child(row(0)).child(row(1))
        .on_focus_held(|s: &mut Grid, dir, _| s.held.push(dir));
    if let Some(route) = s.route {
        root = root.on_focus_move(move |s: &mut Grid, mv: &FocusMove<'_>, _| {
            let default = mv.default.and_then(|d| mv.candidates[d].key.clone());
            s.moves.push((mv.direction, mv.from.key.clone().unwrap(), default, mv.from.section.clone()));
            route(mv)
        });
    }
    root
}

fn runtime(state: Grid) -> Runtime<Grid> {
    let mut rt = Runtime::new(state, grid, (1000.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn press(rt: &mut Runtime<Grid>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

fn focus(rt: &mut Runtime<Grid>, key: &str) {
    let key = key.to_owned();
    rt.update(move |_, tx| tx.focus(key, true));
    rt.frame(0.0);
}

fn focused(rt: &Runtime<Grid>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn walk(rt: &mut Runtime<Grid>, steps: &[(Key, &str)]) {
    for (i, (key, want)) in steps.iter().enumerate() {
        press(rt, key.clone());
        assert_eq!(focused(rt).as_deref(), Some(*want), "step {i}: {key:?}");
    }
}

#[test]
fn arrows_walk_the_grid_by_geometry_in_all_four_directions() {
    let mut rt = runtime(Grid::default());
    walk(&mut rt, &[
        (Key::Down, "home"), // Nothing focused: the first element.
        (Key::Right, "search"),
        (Key::Down, "c00"), // The only card under search (140..240) is c00 (20..220).
        (Key::Right, "c01"),
        (Key::Right, "c02"),
        (Key::Down, "c12"),
        (Key::Left, "c11"),
        (Key::Up, "c01"),
        (Key::Up, "search"), // Neither pill is over c01: the nearer edge wins.
        (Key::Left, "home"),
    ]);
    assert!(rt.state.held.is_empty());
    assert!(rt.elements().iter().any(|e| e.focused && e.focus_visible), "an arrow shows the focus ring");
}

#[test]
fn an_arrow_after_a_blur_returns_to_the_element_last_focused() {
    let mut rt = runtime(Grid::default());
    focus(&mut rt, "c12");
    rt.update(|_, tx| tx.blur());
    rt.frame(0.0);
    assert_eq!(focused(&rt), None);
    press(&mut rt, Key::Left);
    assert_eq!(focused(&rt).as_deref(), Some("c12"));
}

#[test]
fn a_press_with_nowhere_to_go_is_held() {
    let mut rt = runtime(Grid::default());
    focus(&mut rt, "home");
    press(&mut rt, Key::Left);
    press(&mut rt, Key::Up);
    focus(&mut rt, "c13");
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt).as_deref(), Some("c13"));
    use FocusDirection::*;
    assert_eq!(rt.state.held, [Left, Up, Right, Down]);
}

#[test]
fn ties_go_to_the_first_in_document_order() {
    fn view(swap: &bool, cx: &mut Cx<bool>) -> El {
        cx.spatial_focus();
        let tile = |key: &str, x: f32| el().key(key).absolute(x, 100.0).size(90.0, 50.0).focusable(true);
        let (a, b) = (tile("left", 0.0), tile("right", 110.0));
        let (first, second) = if *swap { (b, a) } else { (a, b) };
        el().size(300.0, 200.0).child(el().key("top").absolute(0.0, 0.0).size(200.0, 50.0).focusable(true))
            .child(first).child(second)
    }
    for (swap, want) in [(false, "left"), (true, "right")] {
        let mut rt = Runtime::new(swap, view, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.focus("top", true)); rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Down, mods: Mods::default() }); rt.frame(0.0);
        let focused = rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
        assert_eq!(focused.as_deref(), Some(want), "equal distance and centring, swap {swap}");
    }
}

#[test]
fn sections_land_on_their_last_focus_else_the_nearest() {
    let mut rt = runtime(Grid { sections: true, ..Default::default() });
    walk(&mut rt, &[
        (Key::Down, "home"),
        (Key::Down, "c00"), // Row 0 entered fresh: the card nearest home.
        (Key::Right, "c01"),
        (Key::Right, "c02"),
        (Key::Down, "c12"), // Row 1 entered fresh: the card under c02.
        (Key::Left, "c11"),
        (Key::Left, "c10"),
        (Key::Up, "c02"), // Back where row 0 was left, not the c00 above.
        (Key::Up, "home"), // The nav's last focus, though search is nearer.
        (Key::Right, "search"),
        (Key::Down, "c02"),
        (Key::Down, "c10"),
    ]);
}

#[test]
fn a_section_is_entered_as_one_target_the_size_of_its_frame() {
    let mut rt = runtime(Grid { sections: true, ..Default::default() });
    focus(&mut rt, "c03");
    // Neither pill is near c03 (680..880), but the nav bar spans the width: Up enters it.
    press(&mut rt, Key::Up);
    assert_eq!(focused(&rt).as_deref(), Some("search"));
}

#[test]
fn a_move_hook_replaces_the_default() {
    let mut rt = runtime(Grid {
        route: Some(|mv| if mv.direction == FocusDirection::Down { FocusTarget::To("c13".into()) } else { FocusTarget::Default }),
        ..Default::default()
    });
    focus(&mut rt, "c01");
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt).as_deref(), Some("c13"));
    assert_eq!(rt.state.moves, [(FocusDirection::Down, "c01".into(), Some("c11".into()), None)]);
    press(&mut rt, Key::Left);
    assert_eq!(focused(&rt).as_deref(), Some("c12"), "Default keeps the geometric move");
}

#[test]
fn a_move_hook_sees_sections_and_targets_one_by_key() {
    let mut rt = runtime(Grid {
        sections: true,
        route: Some(|mv| match mv.direction {
            // A "screen slot" rule: the card at the same index in the next row, wherever it is.
            FocusDirection::Down => {
                let Some(row) = mv.from.section.as_deref() else { return FocusTarget::Default };
                let slot = mv.in_section(row).filter(|c| c.rect[0] < mv.from.rect[0]).count();
                let next = if row == "row0" { "row1" } else { return FocusTarget::Held };
                mv.in_section(next).nth(slot).and_then(|c| c.key.clone()).map_or(FocusTarget::Default, FocusTarget::To)
            }
            FocusDirection::Up => FocusTarget::To("row0".into()),
            FocusDirection::Right => FocusTarget::To("nothing".into()),
            FocusDirection::Left => FocusTarget::Default,
        }),
        ..Default::default()
    });
    focus(&mut rt, "c02");
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt).as_deref(), Some("c12"));
    assert_eq!(rt.state.moves[0].3.as_deref(), Some("row0"), "the hook sees the focus's section");
    press(&mut rt, Key::Up);
    assert_eq!(focused(&rt).as_deref(), Some("c00"), "a section's key focuses its first element");
    press(&mut rt, Key::Right);
    assert_eq!(focused(&rt).as_deref(), Some("c01"), "a key that names nothing falls back to the default");
}

#[test]
fn a_move_hook_can_hold_the_press() {
    let mut rt = runtime(Grid { route: Some(|_| FocusTarget::Held), ..Default::default() });
    focus(&mut rt, "c01");
    press(&mut rt, Key::Right);
    assert_eq!(focused(&rt).as_deref(), Some("c01"));
    assert_eq!(rt.state.held, [FocusDirection::Right]);
}

#[test]
fn move_hooks_are_asked_from_the_focus_outward() {
    let mut rt = runtime(Grid {
        sections: true,
        row_route: Some(|mv| if mv.direction == FocusDirection::Up { FocusTarget::To("c13".into()) } else { FocusTarget::Default }),
        route: Some(|mv| if mv.direction == FocusDirection::Up { FocusTarget::Held } else { FocusTarget::Default }),
        ..Default::default()
    });
    focus(&mut rt, "c11");
    press(&mut rt, Key::Up);
    assert_eq!(focused(&rt).as_deref(), Some("c13"), "the row's hook answers first");
    assert!(rt.state.moves.is_empty(), "and the root's is never asked");
    press(&mut rt, Key::Left);
    assert_eq!(focused(&rt).as_deref(), Some("c12"), "both say Default: the geometric move");
    assert_eq!((rt.state.row_asked, rt.state.moves.len()), (2, 1));
}

#[test]
fn an_app_on_key_that_takes_an_arrow_bypasses_spatial_focus() {
    let mut rt = runtime(Grid { route: Some(|_| FocusTarget::Held), ..Default::default() });
    rt.on_key = Some(|s, ev, _| {
        let arrow = FocusDirection::of(&ev.key).is_some();
        s.on_key += usize::from(arrow);
        arrow
    });
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt), None, "with nothing focused the app's on_key comes first too");
    focus(&mut rt, "c01");
    press(&mut rt, Key::Right);
    assert_eq!(focused(&rt).as_deref(), Some("c01"));
    assert_eq!(rt.state.on_key, 2);
    assert!(rt.state.moves.is_empty() && rt.state.held.is_empty(), "the module never ran");
}

#[test]
fn select_presses_the_focused_element() {
    let mut rt = runtime(Grid::default());
    walk(&mut rt, &[(Key::Down, "home"), (Key::Down, "c00"), (Key::Right, "c01")]);
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.state.pressed, ["c01"]);
}

#[test]
fn a_siri_remote_turns_spatial_focus_on_and_its_select_presses() {
    use crate::remote::{Button, Input};
    let mut rt = runtime(Grid { off: true, ..Default::default() });
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt), None, "off by default on a keyboard");
    let mut feed = crate::remote::Feed::default();
    for (i, button) in [Button::Down, Button::Down, Button::Right, Button::Select].into_iter().enumerate() {
        feed.input(&mut rt, Input::Press(button), i as f64);
        feed.input(&mut rt, Input::Release(button), i as f64 + 0.1);
        rt.frame(0.0);
    }
    assert_eq!(focused(&rt).as_deref(), Some("c01"));
    assert_eq!(rt.state.pressed, ["c01"]);
}

#[test]
fn tab_order_is_the_same_with_spatial_focus_on_or_off() {
    let tabs = |off: bool| {
        let mut rt = runtime(Grid { off, sections: true, ..Default::default() });
        (0..12).map(|i| {
            rt.key(KeyEvent { key: Key::Tab, mods: Mods { shift: i >= 8, ..Default::default() } });
            rt.frame(0.0);
            focused(&rt).unwrap()
        }).collect::<Vec<_>>()
    };
    let on = tabs(false);
    assert_eq!(on, tabs(true));
    assert_eq!(on[..3], ["home", "search", "c00"]);
}

#[test]
fn arrows_move_nothing_with_spatial_focus_off() {
    let mut rt = runtime(Grid { off: true, ..Default::default() });
    focus(&mut rt, "c01");
    for key in [Key::Right, Key::Down, Key::Left, Key::Up] { press(&mut rt, key); }
    assert_eq!(focused(&rt).as_deref(), Some("c01"));
    assert!(rt.state.held.is_empty());
}
