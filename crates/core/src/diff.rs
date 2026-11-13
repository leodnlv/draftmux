//! Compares two versions of a draft: a line-then-word diff grouped into
//! hunks, each with where it sits in both versions.

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffOp, capture_diff_deadline};

/// How long each diff may run before it settles for a coarser answer.
const DIFF_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Equal,
    Delete,
    Insert,
}

/// A run of text the two versions share, or that only one of them has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub kind: Kind,
    pub text: String,
    /// The hunk a deletion or insertion belongs to.
    pub hunk: Option<usize>,
}

/// One change: neighbouring deletions and insertions, with where they sit
/// in each version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Byte range in the old text.
    pub old: Range<usize>,
    /// Byte range in the new text.
    pub new: Range<usize>,
    pub removed: String,
    pub added: String,
}

impl Hunk {
    /// Whether it only removes text.
    pub fn is_removal(&self) -> bool {
        self.added.trim().is_empty()
    }

    /// Whether it only adds text.
    pub fn is_addition(&self) -> bool {
        self.removed.trim().is_empty() && !self.added.trim().is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comparison {
    pub ops: Vec<Op>,
    pub hunks: Vec<Hunk>,
}

/// Compares `old` with `new`: lines first, then words within replaced
/// lines, grouped into hunks with their offsets in both texts.
pub fn compare(old: &str, new: &str) -> Comparison {
    let mut ops = diff_ops(old, new);
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut at_old, mut at_new) = (0, 0);
    for k in 0..ops.len() {
        let len = ops[k].text.len();
        if ops[k].kind == Kind::Equal {
            at_old += len;
            at_new += len;
            continue;
        }
        let joins = k > 0 && ops[k - 1].kind != Kind::Equal;
        if !joins {
            hunks.push(Hunk {
                old: at_old..at_old,
                new: at_new..at_new,
                removed: String::new(),
                added: String::new(),
            });
        }
        let index = hunks.len() - 1;
        let hunk = &mut hunks[index];
        ops[k].hunk = Some(index);
        if ops[k].kind == Kind::Delete {
            hunk.removed.push_str(&ops[k].text);
            at_old += len;
            hunk.old.end = at_old;
        } else {
            hunk.added.push_str(&ops[k].text);
            at_new += len;
            hunk.new.end = at_new;
        }
    }
    Comparison { ops, hunks }
}

/// The edit script from `old` to `new`, with each run of changes collapsed
/// into one deletion followed by one insertion. Unchanged stretches of at
/// most two words inside a line are folded into the surrounding change, so a
/// rewritten clause reads as one replacement instead of confetti.
fn diff_ops(old: &str, new: &str) -> Vec<Op> {
    let deadline = Instant::now() + DIFF_TIMEOUT;
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let lines = seq_ops(&old_lines, &new_lines, deadline);
    let mut ops: Vec<(Kind, String)> = Vec::new();
    let mut k = 0;
    while k < lines.len() {
        if lines[k].0 == Kind::Equal {
            ops.push((Kind::Equal, lines[k].1.to_owned()));
            k += 1;
            continue;
        }
        let (mut removed, mut added) = (String::new(), String::new());
        while k < lines.len() && lines[k].0 != Kind::Equal {
            match lines[k].0 {
                Kind::Delete => removed.push_str(lines[k].1),
                _ => added.push_str(lines[k].1),
            }
            k += 1;
        }
        if !removed.is_empty() && !added.is_empty() && related(&removed, &added, deadline) {
            let words = seq_ops(&words(&removed), &words(&added), deadline);
            ops.extend(
                words
                    .into_iter()
                    .map(|(kind, text)| (kind, text.to_owned())),
            );
        } else if !removed.is_empty() && !added.is_empty() {
            // Unrelated lines stay a separate removal and insertion; the
            // empty unchanged run between them keeps them in separate hunks.
            ops.push((Kind::Delete, removed));
            ops.push((Kind::Equal, String::new()));
            ops.push((Kind::Insert, added));
        } else if !removed.is_empty() {
            ops.push((Kind::Delete, removed));
        } else {
            ops.push((Kind::Insert, added));
        }
    }
    let ops = normalize(ops);
    // Pure insertions around a kept word stay separate: "up to 5 attempts,
    // with".
    let removes_before = |k: usize| {
        ops[k - 1].0 == Kind::Delete
            || (ops[k - 1].0 == Kind::Insert && k >= 2 && ops[k - 2].0 == Kind::Delete)
    };
    let mut folded = Vec::with_capacity(ops.len());
    for (k, op) in ops.iter().enumerate() {
        let inner = k > 0
            && k + 1 < ops.len()
            && op.0 == Kind::Equal
            && removes_before(k)
            && ops[k + 1].0 == Kind::Delete;
        if inner && !op.1.contains('\n') && op.1.split_whitespace().count() <= 2 {
            folded.push((Kind::Delete, op.1.clone()));
            folded.push((Kind::Insert, op.1.clone()));
        } else {
            folded.push(op.clone());
        }
    }
    normalize(folded)
        .into_iter()
        .map(|(kind, text)| Op {
            kind,
            text,
            hunk: None,
        })
        .collect()
}

/// The edit script between token lists, deletions before insertions.
fn seq_ops<'a>(old: &[&'a str], new: &[&'a str], deadline: Instant) -> Vec<(Kind, &'a str)> {
    let mut out = Vec::new();
    for op in capture_diff_deadline(
        Algorithm::Myers,
        old,
        0..old.len(),
        new,
        0..new.len(),
        Some(deadline),
    ) {
        let (old_range, new_range) = (op.old_range(), op.new_range());
        match op {
            DiffOp::Equal { .. } => {
                out.extend(old[old_range].iter().map(|text| (Kind::Equal, *text)));
            }
            DiffOp::Delete { .. } | DiffOp::Insert { .. } | DiffOp::Replace { .. } => {
                out.extend(old[old_range].iter().map(|text| (Kind::Delete, *text)));
                out.extend(new[new_range].iter().map(|text| (Kind::Insert, *text)));
            }
        }
    }
    out
}

/// Runs of whitespace and of everything else.
fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (at, c) in text.char_indices() {
        let is_space = c.is_whitespace();
        if space.is_some_and(|space| space != is_space) {
            out.push(&text[start..at]);
            start = at;
        }
        space = Some(is_space);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Whether replaced text shares at least half its words with its
/// replacement, so a word diff reads as an edit rather than as a line swap.
fn related(old: &str, new: &str, deadline: Instant) -> bool {
    // Words match whatever their case or the punctuation around them.
    let normal = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .map(|word| {
                word.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .collect()
    };
    let (old, new) = (normal(old), normal(new));
    let old: Vec<&str> = old.iter().map(String::as_str).collect();
    let new: Vec<&str> = new.iter().map(String::as_str).collect();
    if old.is_empty() && new.is_empty() {
        return false;
    }
    let shared = seq_ops(&old, &new, deadline)
        .iter()
        .filter(|(kind, _)| *kind == Kind::Equal)
        .count();
    4 * shared >= old.len() + new.len()
}

/// Collapses each run of changes into one deletion followed by one
/// insertion, and joins neighbouring unchanged runs.
fn normalize(ops: Vec<(Kind, String)>) -> Vec<(Kind, String)> {
    let mut out: Vec<(Kind, String)> = Vec::with_capacity(ops.len());
    let mut ops = ops.into_iter().peekable();
    while let Some((kind, text)) = ops.next() {
        if kind == Kind::Equal {
            match out.last_mut() {
                Some((Kind::Equal, last)) => last.push_str(&text),
                _ => out.push((kind, text)),
            }
            continue;
        }
        let (mut removed, mut added) = (String::new(), String::new());
        let mut take = |kind: Kind, text: String| match kind {
            Kind::Delete => removed.push_str(&text),
            _ => added.push_str(&text),
        };
        take(kind, text);
        while let Some((kind, text)) = ops.next_if(|(kind, _)| *kind != Kind::Equal) {
            take(kind, text);
        }
        if !removed.is_empty() {
            out.push((Kind::Delete, removed));
        }
        if !added.is_empty() {
            out.push((Kind::Insert, added));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each hunk as `[removed|added]`, in the new text's order.
    fn hunks(old: &str, new: &str) -> Vec<String> {
        compare(old, new)
            .hunks
            .iter()
            .map(|hunk| format!("[{}|{}]", hunk.removed, hunk.added))
            .collect()
    }

    /// One side with marks written as `{+…+}` and `[-…-]`, and a point as

    #[test]
    fn identical_texts_have_no_hunks() {
        assert!(compare("one\ntwo\n", "one\ntwo\n").hunks.is_empty());
        assert!(compare("", "").hunks.is_empty());
    }

    #[test]
    fn diffs_words_within_a_changed_line() {
        assert_eq!(
            hunks(
                "The quick brown fox jumps over the dog.\n",
                "The quick brown fox leaps over the dog.\n"
            ),
            ["[jumps|leaps]"]
        );
    }

    #[test]
    fn folds_short_unchanged_runs_between_changes() {
        // "fox" sits between two replacements, so the clause reads as one.
        assert_eq!(
            hunks(
                "The quick brown fox jumps over the lazy dog today.\n",
                "The quick red fox leaps over the lazy dog today.\n"
            ),
            ["[brown fox jumps|red fox leaps]"]
        );
    }

    #[test]
    fn keeps_long_unchanged_runs_between_changes() {
        assert_eq!(
            hunks(
                "Alpha one two three four omega.\n",
                "Beta one two three four sigma.\n"
            ),
            ["[Alpha|Beta]", "[omega.|sigma.]"]
        );
    }

    #[test]
    fn keeps_insertions_around_a_kept_word_apart() {
        assert_eq!(
            hunks("a b c d e f\n", "a X b Y c d e f\n"),
            ["[|X ]", "[|Y ]"]
        );
    }

    #[test]
    fn unrelated_replacements_stay_a_removal_and_an_insertion() {
        let comparison = compare(
            "Intro\nThe cat sat on the mat.\nEnd\n",
            "Intro\nDeploys now retry on failure.\nEnd\n",
        );
        let pairs: Vec<_> = comparison
            .hunks
            .iter()
            .map(|hunk| (hunk.removed.as_str(), hunk.added.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("The cat sat on the mat.\n", ""),
                ("", "Deploys now retry on failure.\n")
            ]
        );
    }

    #[test]
    fn hunks_know_their_offsets_in_both_texts() {
        let (old, new) = ("one\ntwo\nthree\n", "one\n2\nthree\nfour\n");
        let comparison = compare(old, new);
        for hunk in &comparison.hunks {
            assert_eq!(&old[hunk.old.clone()], hunk.removed);
            assert_eq!(&new[hunk.new.clone()], hunk.added);
        }
        assert_eq!(comparison.hunks.len(), 3);
        assert!(comparison.hunks[0].is_removal());
        assert!(comparison.hunks[1].is_addition());
        assert_eq!(comparison.hunks[2].added, "four\n");
    }

    #[test]
    fn every_change_belongs_to_its_hunk() {
        let comparison = compare("a b c\nkeep\nx\n", "a B c\nkeep\n");
        for op in &comparison.ops {
            assert_eq!(op.hunk.is_some(), op.kind != Kind::Equal);
        }
        assert_eq!(
            comparison.ops.iter().filter_map(|op| op.hunk).max(),
            Some(1)
        );
    }

    #[test]
    fn pure_additions_and_removals() {
        assert_eq!(hunks("one\nthree\n", "one\ntwo\nthree\n"), ["[|two\n]"]);
        assert_eq!(hunks("one\ntwo\nthree\n", "one\nthree\n"), ["[two\n|]"]);
        assert_eq!(hunks("", "Hello\n"), ["[|Hello\n]"]);
    }
}
