//! Spatial focus (`Cx::spatial_focus`, on by default under a Siri Remote): an arrow moves focus
//! to the nearest focusable element that way, chosen from where the elements are drawn, as the
//! tvOS focus engine does. It runs after everything else that can take an arrow (a focused
//! control's own keys, a roving `El::focus_group`, a focused field, the app's `on_key`).
//!
//! The rule, for a move from the focused element's rect `F` to a candidate's rect `C`, with the
//! main axis along the move and the cross axis across it:
//!
//! 1. Half-plane: `C`'s near edge is at or past `F`'s centre line, and its far edge past `F`'s
//!    far edge. Candidates behind, level with or wholly inside `F` never qualify. Measuring
//!    from the centre, not the far edge, keeps a neighbour a target while the focused card is
//!    drawn scaled up over the gap.
//! 2. Candidates that overlap `F` on the cross axis beat those that don't.
//! 3. Then the nearer wins: the gap along the main axis (0 when they touch or overlap) for
//!    overlapping candidates, and the straight distance between the closest edges for the
//!    rest. Distances are compared to the half point.
//! 4. Then the one whose cross-axis centre is closest to `F`'s, then the first in document
//!    order.
//!
//! A `El::focus_section` that doesn't hold the focused element takes part as one candidate with
//! its own rect (or its members' bounds, if it has no size). If it wins, focus lands on the
//! element last focused in it, else on its member nearest `F` by rules 2 to 4.
use super::*;
use crate::el::{FocusCandidate, FocusDirection, FocusMove, FocusTarget};

/// The sections whose last focus is kept before the ones no longer in the view are dropped.
const REMEMBERED_MAX: usize = 64;

/// What spatial focus keeps between presses.
#[derive(Debug, Default)]
pub(crate) struct SpatialFocus {
    /// Section path to the path of the element last focused inside it.
    remembered: HashMap<String, String>,
    /// A Siri Remote has sent input: spatial focus is on whatever the view says.
    remote: bool,
}

/// How good a candidate is for a move; lower is better, field by field.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub(crate) struct Score {
    /// No overlap on the cross axis.
    apart: bool,
    distance: f32,
    /// Between the cross-axis centres.
    offset: f32,
}

/// Rule 1 to 4's score for moving from `from` to `to` (`[x, y, w, h]`), or `None` when `to`
/// isn't in the half-plane that way and `half_plane` asks for it.
pub(crate) fn score(from: [f32; 4], to: [f32; 4], dir: FocusDirection, half_plane: bool) -> Option<Score> {
    let span = |r: [f32; 4], horizontal: bool| if horizontal { (r[0], r[0] + r[2]) } else { (r[1], r[1] + r[3]) };
    let horizontal = matches!(dir, FocusDirection::Left | FocusDirection::Right);
    // Main-axis spans measured forward, so "past" is always "greater".
    let forward = |(a, b): (f32, f32)| match dir {
        FocusDirection::Right | FocusDirection::Down => (a, b),
        FocusDirection::Left | FocusDirection::Up => (-b, -a),
    };
    let ((f0, f1), (t0, t1)) = (forward(span(from, horizontal)), forward(span(to, horizontal)));
    let ((c0, c1), (d0, d1)) = (span(from, !horizontal), span(to, !horizontal));
    if half_plane && !(t0 >= (f0 + f1) * 0.5 - 0.5 && t1 > f1 + 0.5) { return None; }
    let gap = (t0 - f1).max(0.0);
    let overlap = c1.min(d1) - c0.max(d0);
    let apart = overlap <= 0.0;
    let distance = if apart { gap.hypot(-overlap) } else { gap };
    Some(Score { apart, distance: (distance * 2.0).round() * 0.5, offset: ((c0 + c1) - (d0 + d1)).abs() * 0.5 })
}

/// A candidate for a move: one element, or a section entered as a whole.
#[derive(Clone, Copy, PartialEq)]
enum Unit {
    One(usize),
    Section(usize),
}

impl<S: 'static> Runtime<S> {
    /// A Siri Remote drives this window: arrows move focus spatially from now on.
    pub(crate) fn remote_drives_focus(&mut self) {
        self.spatial.remote = true;
    }

    fn spatial_on(&self) -> bool {
        self.defaults.spatial_focus || self.spatial.remote
    }

    /// Where node `i` is drawn, `[x, y, w, h]`.
    fn focus_rect(&self, i: usize) -> [f32; 4] {
        self.drawn.get(i).copied().unwrap_or_else(|| {
            let t = self.nodes[i].target;
            [t[0], t[1], t[2] - t[0], t[3] - t[1]]
        })
    }

    /// Node `i` and its ancestors, nearest first.
    fn up_from(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(Some(i), |&j| self.nodes[j].parent)
    }

    fn holds(&self, outer: usize, i: usize) -> bool {
        self.up_from(i).any(|j| j == outer)
    }

    /// Records the focused element as the last focus of every section around it.
    pub(super) fn remember_section_focus(&mut self) {
        let Some(i) = self.focused_index() else { return };
        let sections: Vec<usize> = self.up_from(i).skip(1).filter(|&s| self.nodes[s].focus.section).collect();
        if sections.is_empty() { return; }
        let path = &self.nodes[i].path;
        for s in sections {
            self.spatial.remembered.insert(self.nodes[s].path.clone(), path.clone());
        }
        if self.spatial.remembered.len() > REMEMBERED_MAX {
            let nodes = &self.nodes;
            self.spatial.remembered.retain(|section, _| nodes.iter().any(|n| n.focus.section && &n.path == section));
        }
    }

    /// An unmodified arrow, with spatial focus on: focus moves, or the press is held.
    pub(super) fn spatial_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.spatial_on() || ev.mods != crate::Mods::default() { return false; }
        let Some(dir) = FocusDirection::of(&ev.key) else { return false };
        let reach: Vec<usize> = (0..self.nodes.len()).filter(|&j| self.can_focus(j)).collect();
        let Some(from) = self.focused_index().filter(|&i| self.can_focus(i)) else {
            let last = self.focus.tab_start().and_then(|p| reach.iter().copied().find(|&j| self.nodes[j].path == p));
            let Some(j) = last.or(reach.first().copied()) else { return false };
            self.spatial_focus(j, tx);
            return true;
        };
        let others: Vec<usize> = reach.into_iter().filter(|&j| j != from).collect();
        let default = self.spatial_pick(from, dir, &others);
        let target = match self.spatial_hook(from, dir, &others, default, tx) {
            FocusTarget::Default => default,
            FocusTarget::Held => None,
            FocusTarget::To(key) => self.spatial_by_key(&key, &others).or(default),
        };
        match target {
            Some(j) => self.spatial_focus(j, tx),
            None => {
                log_event!(self, "focus held {dir:?}");
                let held = self.up_from(from).find_map(|i| self.nodes[i].focus.on_held.clone());
                if let Some(f) = held { f(states!(self), dir, tx); }
            }
        }
        true
    }

    fn spatial_focus(&mut self, j: usize, tx: &mut Tx) {
        let path = self.nodes[j].path.clone();
        log_event!(self, "focus {path}");
        self.set_focus(Some(path.clone()), true);
        tx.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, path));
    }

    /// The default rule's target among `others` (see the module docs).
    fn spatial_pick(&self, from: usize, dir: FocusDirection, others: &[usize]) -> Option<usize> {
        let at = self.focus_rect(from);
        let outer = |j: usize| self.up_from(j).skip(1).filter(|&s| self.nodes[s].focus.section && !self.holds(s, from)).last();
        let mut units: Vec<Unit> = Vec::new();
        for &j in others {
            let unit = outer(j).map_or(Unit::One(j), Unit::Section);
            if !units.contains(&unit) { units.push(unit); }
        }
        let rect = |unit: Unit| match unit {
            Unit::One(j) => self.focus_rect(j),
            Unit::Section(s) => {
                let r = self.focus_rect(s);
                if r[2] > 0.0 && r[3] > 0.0 { return r; }
                let members = others.iter().filter(|&&j| self.holds(s, j)).map(|&j| self.focus_rect(j));
                let [x0, y0, x1, y1] = members.fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, r| {
                    [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[0] + r[2]), b[3].max(r[1] + r[3])]
                });
                [x0, y0, x1 - x0, y1 - y0]
            }
        };
        let best = units.into_iter()
            .filter_map(|u| score(at, rect(u), dir, true).map(|s| (s, u)))
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))?.1;
        match best {
            Unit::One(j) => Some(j),
            Unit::Section(s) => {
                let members: Vec<usize> = others.iter().copied().filter(|&j| self.holds(s, j)).collect();
                let remembered = self.spatial.remembered.get(&self.nodes[s].path)
                    .and_then(|p| members.iter().copied().find(|&j| &self.nodes[j].path == p));
                remembered.or_else(|| members.into_iter()
                    .filter_map(|j| score(at, self.focus_rect(j), dir, false).map(|s| (s, j)))
                    .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(_, j)| j))
            }
        }
    }

    /// The `on_focus_move` hooks from `from` outward; the first that answers decides.
    fn spatial_hook(&mut self, from: usize, dir: FocusDirection, others: &[usize], default: Option<usize>, tx: &mut Tx) -> FocusTarget {
        let hooks: Vec<_> = self.up_from(from).filter_map(|i| self.nodes[i].focus.on_move.clone()).collect();
        if hooks.is_empty() { return FocusTarget::Default; }
        let candidate = |j: usize| FocusCandidate {
            key: self.nodes[j].key.clone(),
            path: self.nodes[j].path.clone(),
            rect: self.focus_rect(j),
            section: self.up_from(j).skip(1).find(|&s| self.nodes[s].focus.section).and_then(|s| self.nodes[s].key.clone()),
        };
        let from_candidate = candidate(from);
        let candidates: Vec<_> = others.iter().map(|&j| candidate(j)).collect();
        let mv = FocusMove {
            direction: dir,
            from: &from_candidate,
            candidates: &candidates,
            default: default.and_then(|d| others.iter().position(|&j| j == d)),
        };
        for f in hooks {
            let target = f(states!(self), &mv, tx);
            if target != FocusTarget::Default { return target; }
        }
        FocusTarget::Default
    }

    /// The element keyed `key` among `others`, or the first of them inside it.
    fn spatial_by_key(&self, key: &str, others: &[usize]) -> Option<usize> {
        let anchor = (0..self.nodes.len()).find(|&j| self.nodes[j].key.as_deref() == Some(key))?;
        others.iter().copied().find(|&j| self.holds(anchor, j))
    }
}

#[cfg(test)]
#[path = "spatial_tests.rs"]
mod tests;
