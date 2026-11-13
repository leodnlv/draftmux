//! Where a comparison's changes show on each side, as ranges of that
//! side's text. A renderer finds the rendered text for each range through
//! its target's source map; keeping and reverting use the hunks themselves,
//! so a decoration never authorizes an edit.

use std::ops::Range;

use crate::diff::{Comparison, Hunk};

/// Which version of a comparison is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The old text, showing what was removed.
    Before,
    /// The new text, showing what was added.
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationKind {
    /// Text only the new version has.
    Inserted,
    /// Text only the old version has.
    Deleted,
    /// Where a side shows none of a change's text: the change's place in
    /// it, an empty range.
    Point,
}

/// Part of one side's text that a change marks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoration {
    pub side: Side,
    /// The change, by index in [`Comparison::hunks`].
    pub hunk: usize,
    /// Byte range in the side's text; empty for a [`DecorationKind::Point`].
    pub range: Range<usize>,
    pub kind: DecorationKind,
}

/// Every change's decorations, the old side's first, each side in text
/// order. A side gets its own text of a change, and a point where that text
/// is empty or only whitespace, so every change shows on both sides.
pub fn decorations(comparison: &Comparison) -> Vec<Decoration> {
    let mut out = Vec::new();
    for side in [Side::Before, Side::After] {
        for (hunk, change) in comparison.hunks.iter().enumerate() {
            side_decorations(side, hunk, change, &mut out);
        }
    }
    out
}

fn side_decorations(side: Side, hunk: usize, change: &Hunk, out: &mut Vec<Decoration>) {
    let decoration = |range, kind| Decoration {
        side,
        hunk,
        range,
        kind,
    };
    let (range, kind, text) = match side {
        Side::Before => (change.old.clone(), DecorationKind::Deleted, &change.removed),
        Side::After => (change.new.clone(), DecorationKind::Inserted, &change.added),
    };
    let shown = (!range.is_empty()).then(|| decoration(range.clone(), kind));
    let point = |at: usize| {
        text.trim()
            .is_empty()
            .then(|| decoration(at..at, DecorationKind::Point))
    };
    // The added text goes after the removed text, so the old side's point
    // follows its text and the new side's precedes it.
    match side {
        Side::Before => out.extend(shown.into_iter().chain(point(range.end))),
        Side::After => out.extend(point(range.start).into_iter().chain(shown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::compare;

    fn on(comparison: &Comparison, side: Side) -> Vec<(usize, Range<usize>, DecorationKind)> {
        decorations(comparison)
            .into_iter()
            .filter(|decoration| decoration.side == side)
            .map(|decoration| (decoration.hunk, decoration.range, decoration.kind))
            .collect()
    }

    #[test]
    fn a_replacement_marks_its_text_on_both_sides() {
        let comparison = compare("The fox jumps.\n", "The fox leaps.\n");
        let at = "The fox ".len();
        assert_eq!(
            on(&comparison, Side::Before),
            [(0, at..at + 6, DecorationKind::Deleted)]
        );
        assert_eq!(
            on(&comparison, Side::After),
            [(0, at..at + 6, DecorationKind::Inserted)]
        );
    }

    #[test]
    fn an_addition_is_a_point_where_it_goes_in_the_old_text() {
        let (old, new) = ("one\nthree\n", "one\ntwo\nthree\n");
        let comparison = compare(old, new);
        assert_eq!(
            on(&comparison, Side::Before),
            [(0, 4..4, DecorationKind::Point)]
        );
        assert_eq!(
            on(&comparison, Side::After),
            [(0, 4..8, DecorationKind::Inserted)]
        );
    }

    #[test]
    fn a_whitespace_change_also_gets_a_point() {
        let comparison = compare("a\n", "a\n\n");
        assert_eq!(
            on(&comparison, Side::Before),
            [(0, 2..2, DecorationKind::Point)]
        );
        assert_eq!(
            on(&comparison, Side::After),
            [
                (0, 2..2, DecorationKind::Point),
                (0, 2..3, DecorationKind::Inserted)
            ]
        );
    }
}
