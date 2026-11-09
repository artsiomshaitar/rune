//! Spatial focus, as the tvOS focus engine moves it: what an element declares for it
//! (`El::focus_section`, `El::on_focus_move`, `El::on_focus_held`) and what the hooks are handed.
//! The runtime side, which picks the target, is `runtime/input/spatial.rs`.
use super::*;

pub(crate) type FocusMoveFn = Rc<dyn Fn(crate::states::States<'_>, &FocusMove<'_>, &mut Tx) -> FocusTarget>;
pub(crate) type FocusHeldFn = Rc<dyn Fn(crate::states::States<'_>, FocusDirection, &mut Tx)>;

/// The way an arrow (or a Siri Remote press or swipe) moves focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FocusDirection {
    Up,
    Down,
    Left,
    Right,
}

impl FocusDirection {
    /// The direction an unmodified arrow key moves focus, or `None` for any other key.
    pub fn of(key: &crate::Key) -> Option<Self> {
        Some(match key {
            crate::Key::Up => Self::Up,
            crate::Key::Down => Self::Down,
            crate::Key::Left => Self::Left,
            crate::Key::Right => Self::Right,
            _ => return None,
        })
    }
}

/// A focusable element as spatial focus sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusCandidate {
    /// Its `El::key`, if it has one.
    pub key: Option<String>,
    /// Its path, which names it even without a key.
    pub path: String,
    /// Where it's drawn, `[x, y, w, h]` in window points.
    pub rect: [f32; 4],
    /// The key of the nearest `El::focus_section` around it, if it has one.
    pub section: Option<String>,
}

/// A move an `El::on_focus_move` hook may replace.
#[derive(Debug)]
pub struct FocusMove<'a> {
    pub direction: FocusDirection,
    /// The focused element.
    pub from: &'a FocusCandidate,
    /// Every other element focus can reach, in document order, wherever it lies.
    pub candidates: &'a [FocusCandidate],
    /// The index in `candidates` of the element the default rule picks, or `None` when it
    /// finds nothing that way, which holds the press.
    pub default: Option<usize>,
}

impl FocusMove<'_> {
    /// The candidates inside the section keyed `section`, in document order.
    pub fn in_section<'s>(&'s self, section: &'s str) -> impl Iterator<Item = &'s FocusCandidate> + 's {
        self.candidates.iter().filter(move |c| c.section.as_deref() == Some(section))
    }
}

/// What an `El::on_focus_move` hook decides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    /// No opinion: an `on_focus_move` further out decides, else the default rule.
    Default,
    /// Focus stays, and the press is reported as held (`El::on_focus_held`).
    Held,
    /// Focus moves to the element with this key, or the first focusable one inside it. A key
    /// that names nothing focusable falls back to the default rule.
    To(String),
}

impl El {
    /// A focus section: arrows that enter it from outside land on the element last focused in
    /// it, else on the one nearest the press, as a tvOS `focusSection()` does. From outside it
    /// is one target the size of its own frame, so a nav bar of a few pills is reachable from
    /// anywhere under it. Use it for a row of posters or a nav bar: "come back to where you
    /// left". It's not a roving [`El::focus_group`]: every element inside stays a target of
    /// its own once focus is in the section.
    pub fn focus_section(mut self) -> Self {
        self.cold_mut().focus.section = true;
        self
    }
    /// Decides the arrows' moves while focus is on this element or inside it. Asked from the
    /// focused element outward; the first hook that answers other than
    /// [`FocusTarget::Default`] decides. Put it on a section for its own rules, or on the root
    /// for the whole view's. An app's `on_key` that takes an arrow comes first and bypasses
    /// spatial focus altogether.
    pub fn on_focus_move<S: 'static>(mut self, f: impl Fn(&mut S, &FocusMove<'_>, &mut Tx) -> FocusTarget + 'static) -> Self {
        self.cold_mut().focus.on_move = Some(Rc::new(move |s, mv, tx| f(downcast::<S>(s), mv, tx)));
        self
    }
    /// Called when an arrow finds nowhere to go from focus on this element or inside it (or a
    /// hook said [`FocusTarget::Held`]): the press is held, for an app to play a bump against
    /// the edge. The nearest hook from the focused element outward gets it.
    pub fn on_focus_held<S: 'static>(mut self, f: impl Fn(&mut S, FocusDirection, &mut Tx) + 'static) -> Self {
        self.cold_mut().focus.on_held = Some(Rc::new(move |s, dir, tx| f(downcast::<S>(s), dir, tx)));
        self
    }
}
