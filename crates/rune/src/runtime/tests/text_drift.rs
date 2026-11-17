use super::*;
use crate::render::{KIND_GLYPH, KIND_RECT, KIND_TRIANGLE};
use crate::{Color, Live, Shape, TextStyle};

/// A plate the app drifts by sub-pixel amounts every frame (`live_offset`, not pixel-snapped),
/// at a fractional place, carrying a triangle and a label, as a focused button leans with a
/// swipe on the remote.
struct Plate {
    lean: Live<[f32; 2]>,
}

fn plate(s: &Plate, _: &mut Cx<Plate>) -> El {
    let triangle = el().size(11.0, 12.0).shape(Shape::new().fill(vec![[0.0, 0.0], [11.0, 6.0], [0.0, 12.0]], Color::BLACK));
    let label = crate::text("Resume").font(TextStyle::new(15.0, 600.0)).color(Color::BLACK);
    el().size(240.0, 120.0).child(
        crate::row().absolute(20.3, 30.6).padding(12.0).gap(8.0).radius(10.0).fill(Color::WHITE)
            .child(triangle).child(label)
            .live_offset(&s.lean).pixel_snap(false),
    )
}

fn plate_at(rt: &Runtime<Plate>) -> [f32; 2] {
    let r = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).expect("the plate's fill").rect;
    [r[0], r[1]]
}

fn triangle_at(rt: &Runtime<Plate>) -> [f32; 2] {
    let r = rt.instances.iter().find(|i| i.params[1] == KIND_TRIANGLE).expect("the triangle").rect;
    [r[0], r[1]]
}

/// The coverage-weighted centre of the label's ink (device px), from its atlas masks.
fn ink_at(rt: &Runtime<Plate>) -> [f32; 2] {
    let atlas = &rt.text.atlas;
    let size = crate::text::ATLAS_SIZE as usize;
    let (mut sum, mut at) = (0.0f64, [0.0f64; 2]);
    for i in rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH) {
        let data = atlas.page_data(i.params[2] as usize).expect("an atlas page");
        let (u, v) = (i.uv[0] as usize, i.uv[1] as usize);
        for y in 0..i.rect[3] as usize {
            for x in 0..i.rect[2] as usize {
                let c = data[(v + y) * size + u + x] as f64;
                sum += c;
                at[0] += c * (i.rect[0] as f64 + x as f64 + 0.5);
                at[1] += c * (i.rect[1] as f64 + y as f64 + 0.5);
            }
        }
    }
    assert!(sum > 0.0, "draws glyphs");
    [(at[0] / sum) as f32, (at[1] / sum) as f32]
}

/// The drift: the lean ramps out to 2.5 points and back on both axes, a 60th of a second a step.
/// Returns, per axis, how far the triangle and the ink stray from the plate's drawn edge (device
/// px, max minus min) and how far the plate went.
fn drift() -> ([f32; 2], [f32; 2], [f32; 2]) {
    let lean = Live::new([0.0, 0.0]);
    let mut rt = Runtime::new(Plate { lean: lean.clone() }, plate, (240.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let (mut shape, mut ink, mut travel) = ([(f32::MAX, f32::MIN); 2], [(f32::MAX, f32::MIN); 2], [(f32::MAX, f32::MIN); 2]);
    let widen = |r: &mut (f32, f32), v: f32| *r = (r.0.min(v), r.1.max(v));
    for k in 0..=120 {
        let u = 1.0 - (k as f32 / 60.0 - 1.0).abs();
        lean.set([2.5 * u, -1.7 * u]);
        rt.frame(1.0 / 60.0);
        let (p, t, i) = (plate_at(&rt), triangle_at(&rt), ink_at(&rt));
        for a in 0..2 {
            widen(&mut shape[a], t[a] - p[a]);
            widen(&mut ink[a], i[a] - p[a]);
            widen(&mut travel[a], p[a]);
        }
    }
    let spread = |r: [(f32, f32); 2]| r.map(|(lo, hi)| hi - lo);
    (spread(shape), spread(ink), spread(travel))
}

/// What a plate that doesn't pixel-snap carries drifts with its exact edges: the triangle to the
/// float, the label's ink to within a glide phase, rather than each stepping a whole pixel.
#[test]
fn content_drifts_with_a_box_that_does_not_snap() {
    let (shape, ink, travel) = drift();
    eprintln!("over {travel:?} px of travel the triangle strays {shape:?} px and the ink {ink:?} px from the plate");
    assert!(travel[0] > 4.9 && travel[1] > 3.3, "the plate moves: {travel:?}");
    assert!(shape.iter().all(|&v| v < 0.05), "the triangle keeps its place on the plate: {shape:?}");
    assert!(ink.iter().all(|&v| v < 0.3), "the ink keeps its place on the plate: {ink:?}");
}

/// Back at no lean the frame is byte for byte what a plate that never drifted draws: text, the
/// triangle and the plate's edges all on the grid as at rest.
#[test]
fn a_drift_lands_on_the_resting_frame() {
    let lean = Live::new([0.0, 0.0]);
    let mut rt = Runtime::new(Plate { lean: lean.clone() }, plate, (240.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for k in 1..=30 {
        lean.set([0.07 * k as f32, -0.05 * k as f32]);
        rt.frame(1.0 / 60.0);
    }
    lean.set([0.0, 0.0]);
    rt.frame(1.0 / 60.0);
    let mut still = Runtime::new(Plate { lean: Live::new([0.0, 0.0]) }, plate, (240.0, 120.0), Env::new(true, 2.0));
    still.frame(0.0);
    let bytes = |rt: &Runtime<Plate>| bytemuck::cast_slice::<_, u8>(&rt.instances).to_vec();
    assert!(bytes(&rt) == bytes(&still), "the plate back at rest draws as a still one");
}

/// A glazed button as the app draws one: a plate at whole points carrying a triangle and a label
/// at 28 points, inside a rounded window, leaning with `lean` and scaled `k` (springing there).
struct Button {
    label: &'static str,
    lean: Live<[f32; 2]>,
    k: f32,
}

fn button(s: &Button, _: &mut Cx<Button>) -> El {
    let triangle = el().size(22.0, 24.0).shape(Shape::new().fill(vec![[0.0, 0.0], [22.0, 12.0], [0.0, 24.0]], Color::BLACK));
    let label = crate::text(s.label).font(TextStyle::new(28.0, 600.0)).color(Color::BLACK);
    let pill = crate::row().h(72.0).px(40.0).items_center().gap(16.0).child(triangle).child(label);
    el().size(480.0, 200.0).child(
        el().absolute(20.0, 30.0).size(300.0, 72.0).radius(999.0).fill(Color::WHITE)
            .child(el().absolute(0.0, 0.0).w_full().h_full().clip_rounded(999.0).child(el().absolute(0.0, 0.0).child(pill)))
            .scale(s.k)
            .scale_motion(rune_motion::Spring::new(0.42, 0.0))
            .live_offset(&s.lean)
            .pixel_snap(false),
    )
}

/// tvOS's text (`FontSmoothing::Off`) at the Apple TV's scale.
fn button_runtime(label: &'static str, k: f32) -> (Runtime<Button>, Live<[f32; 2]>) {
    let lean = Live::new([0.0, 0.0]);
    let mut rt = Runtime::new(Button { label, lean: lean.clone(), k }, button, (480.0, 200.0), Env::new(true, 2.0));
    rt.text.set_font_smoothing(crate::text::FontSmoothing::Off);
    rt.frame(0.0);
    rt.frame(1.0);
    (rt, lean)
}

/// Each glyph's ink centre across, against the plate's left edge, in the plate's resting device
/// px (its drawn width over its resting one), in the order drawn.
fn glyphs_on_plate(rt: &Runtime<Button>) -> Vec<f32> {
    let plate = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).expect("the plate's fill").rect;
    glyph_centres(&rt.text, &rt.instances).iter().map(|c| (c - plate[0]) / (plate[2] / 600.0)).collect()
}

/// Each glyph's ink centre across (device px), in the order drawn.
fn glyph_centres(text: &crate::text::TextSystem, instances: &[crate::render::Instance]) -> Vec<f32> {
    let atlas = &text.atlas;
    let size = crate::text::ATLAS_SIZE as usize;
    instances.iter().filter(|i| i.params[1] == KIND_GLYPH).map(|i| {
        let data = atlas.page_data(i.params[2] as usize).expect("an atlas page");
        // A zoomed quad samples a mask of its own size; a resting one is its mask.
        let (w, h) = if i.uv[2] > 0.0 { (i.uv[2], i.uv[3]) } else { (i.rect[2], i.rect[3]) };
        let (u, v) = (i.uv[0] as usize, i.uv[1] as usize);
        let (mut sum, mut at) = (0.0f64, 0.0f64);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let c = data[(v + y) * size + u + x] as f64;
                sum += c;
                at += c * (x as f64 + 0.5);
            }
        }
        assert!(sum > 0.0, "a glyph with ink");
        i.rect[0] + (at / sum) as f32 * i.rect[2] / w
    }).collect()
}

/// How far each glyph strays on the plate over the frames (max minus min), and how far its place
/// against the first glyph's strays: a word that moves as one piece keeps the second near zero.
fn spreads(frames: &[Vec<f32>]) -> (Vec<f32>, Vec<f32>) {
    let n = frames[0].len();
    assert!(frames.iter().all(|f| f.len() == n), "the same glyphs every frame");
    let spread = |f: &dyn Fn(&Vec<f32>) -> f32| {
        let (lo, hi) = frames.iter().map(f).fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        hi - lo
    };
    ((0..n).map(|g| spread(&|f| f[g])).collect(), (1..n).map(|g| spread(&|f| f[g] - f[0])).collect())
}

/// Every glyph of a label on a plate at whole points drifts with it, the first one too: a first
/// glyph's pen rests on a pixel boundary, and the plate's drawn edge less the drift wobbled by a
/// 128th of a pixel, so it floored a whole pixel either way while the rest glided. At rest and
/// at the press's 0.96, whose label rests on its own boundaries.
#[test]
fn every_glyph_drifts_with_its_plate() {
    for label in ["Resume", "Watchlist"] {
        for k in [1.0, 0.96] {
            let (mut rt, lean) = button_runtime(label, k);
            let frames: Vec<Vec<f32>> = (0..=80).map(|step| {
                lean.set([step as f32 * 0.037, step as f32 * -0.023]);
                rt.frame(1.0 / 60.0);
                glyphs_on_plate(&rt)
            }).collect();
            let (own, against_first) = spreads(&frames);
            eprintln!("{label} at {k}: each glyph strays {own:?} px, against the first {against_first:?}");
            assert!(own.iter().all(|&v| v < 0.3), "{label} at {k}: each glyph keeps its place on the plate: {own:?}");
            assert!(against_first.iter().all(|&v| v < 0.05), "{label} at {k}: the word moves as one piece: {against_first:?}");
        }
    }
}

/// The press's dip and its spring back: every glyph, the first too, keeps its place on the
/// scaling plate, and the word scales as one piece.
#[test]
fn every_glyph_scales_with_its_plate() {
    for label in ["Resume", "Watchlist"] {
        let (mut rt, _lean) = button_runtime(label, 1.0);
        let mut frames = Vec::new();
        for step in 0..90 {
            if step == 0 { rt.update(|s, _| s.k = 0.96); }
            if step == 40 { rt.update(|s, _| s.k = 1.0); }
            rt.frame(1.0 / 120.0);
            frames.push(glyphs_on_plate(&rt));
        }
        let (own, against_first) = spreads(&frames);
        eprintln!("{label}: each glyph strays {own:?} px, against the first {against_first:?}");
        assert!(own.iter().all(|&v| v < 0.3), "{label}: each glyph keeps its place on the plate: {own:?}");
        assert!(against_first.iter().all(|&v| v < 0.05), "{label}: the word scales as one piece: {against_first:?}");
    }
}

/// Focus landing on a bar button: its plate pops in, keyed, from 0.96 and 40% on a spring,
/// carrying its label in a rounded window; `at` is where the label sits on the plate, which a
/// bar of centred pills puts at fractions of a point.
struct Pop {
    on: bool,
    at: f32,
    lean: Live<[f32; 2]>,
}

fn pop(s: &Pop, _: &mut Cx<Pop>) -> El {
    let mut root = el().size(480.0, 200.0);
    if s.on {
        let label = crate::text("Watchlist").font(TextStyle::new(30.0, 600.0)).color(Color::BLACK);
        let pill = el().px(28.0).py(12.0).motion(rune_motion::Spring::from_response(0.35, 0.85)).child(crate::row().child(label));
        let window = el().absolute(0.0, 0.0).w_full().h_full().clip_rounded(999.0).transition(crate::Transition::none())
            .child(el().absolute(s.at, 0.0).transition(crate::Transition::none()).child(pill.transition(crate::Transition::none())));
        let snap = rune_motion::Spring::from_response(0.105, 1.0);
        let mut transition = crate::Transition::scale(0.96).springs(crate::SpringPair::new(snap, rune_motion::Spring::NONE));
        transition.insertion.opacity = 0.4;
        root = root.child(
            el().key("plate").absolute(40.0, 30.0).size(220.0, 64.0).radius(999.0).fill(Color::WHITE).child(window)
                .transition(transition).live_offset(&s.lean).pixel_snap(false),
        );
    }
    root
}

/// While the plate pops, every glyph of its label, the first among them, sits on the scaling
/// plate where it rests, and the frame the scale lands moves none of them. Text whose scale moves
/// was laid out from no place in particular, so its pens rounded to whole pixels apart from its
/// origin's fraction and each glyph landed up to a pixel from where it had been drawn.
#[test]
fn a_popping_label_lands_where_it_rests() {
    for at in [0.0, 0.37, 0.71] {
        for lean in [[0.0, 0.0], [0.3, -0.2]] {
            let mut rt = Runtime::new(Pop { on: false, at, lean: Live::new(lean) }, pop, (480.0, 200.0), Env::new(true, 2.0));
            rt.text.set_font_smoothing(crate::text::FontSmoothing::Off);
            rt.frame(0.0);
            rt.update(|s, _| s.on = true);
            let mut frames = Vec::new();
            for _ in 0..60 {
                rt.frame(1.0 / 120.0);
                let plate = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).expect("the plate's fill").rect;
                frames.push(glyph_centres(&rt.text, &rt.instances).iter().map(|c| (c - plate[0]) / (plate[2] / 440.0)).collect::<Vec<_>>());
            }
            let rest = frames.last().expect("frames").clone();
            let worst = (0..rest.len())
                .map(|g| frames.iter().map(|f| (f[g] - rest[g]).abs()).fold(0.0f32, f32::max))
                .collect::<Vec<_>>();
            eprintln!("at {at}, lean {lean:?}: each glyph strays at most {worst:?} px from where it rests");
            assert!(worst.iter().all(|&v| v < 0.05), "at {at}, lean {lean:?}: the label pops in where it rests: {worst:?}");
        }
    }
}

/// A line too long for its window, scrolled by the app through `live_offset` inside a clip that
/// fades its cut ends, as a focused title's marquee does. No box here opts out of pixel-snapping.
struct Scroll {
    by: Live<[f32; 2]>,
}

fn scroll(s: &Scroll, _: &mut Cx<Scroll>) -> El {
    let line = crate::text("Chainsaw Man - The Movie: Reze Arc").font(TextStyle::new(22.0, 500.0)).color(Color::WHITE);
    el().size(400.0, 100.0).child(
        el().absolute(20.0, 30.0).px(16.0).w(132.0).clip_fade(16.0, 0.0, 16.0, 0.0).child(line.live_offset(&s.by)),
    )
}

/// The scrolled line's glyphs move with its offset, frame by frame as a marquee glides it (two
/// thirds of a point a frame), with the browser's text and tvOS's: the middle glyph's ink a
/// point to the left for every point scrolled, to within the pixel the line snaps to.
#[test]
fn a_scrolled_line_draws_where_it_scrolled() {
    for smoothing in [None, Some(crate::text::FontSmoothing::Off)] {
        let by = Live::new([0.0, 0.0]);
        let mut rt = Runtime::new(Scroll { by: by.clone() }, scroll, (400.0, 100.0), Env::new(true, 2.0));
        if let Some(smoothing) = smoothing {
            rt.text.set_font_smoothing(smoothing);
        }
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let rest = glyph_centres(&rt.text, &rt.instances);
        let middle = rest.len() / 2;
        for step in 1..=30 {
            let scrolled = step as f32 * 2.0 / 3.0;
            by.set([-scrolled, 0.0]);
            rt.frame(1.0 / 60.0);
            let at = glyph_centres(&rt.text, &rt.instances)[middle];
            let expected = rest[middle] - 2.0 * scrolled;
            assert!((at - expected).abs() < 1.0, "{smoothing:?}, {scrolled} pt in: the middle glyph at {at}, not {expected}");
        }
    }
}
