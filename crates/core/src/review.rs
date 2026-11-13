//! Reviewing what the agent's edits changed in the selected draft: the
//! draft as it is compared with the text before one of its edits, the latest
//! by default, which changes in that comparison the user kept, and which one
//! is selected.
//!
//! Reviewing never rewrites the history: keeping a change only records its
//! key, and reverting edits the draft.

use std::rc::Rc;

use crate::decorations::{Decoration, decorations};
use crate::diff::{self, Comparison, Hunk};
use crate::history::{ChangeKey, EditLog};

/// The draft compared with the text before an edit.
pub struct Compared {
    pub base: String,
    pub text: String,
    pub comparison: Comparison,
    /// Where each change shows in `base` and `text`.
    pub decorations: Vec<Decoration>,
}

/// Which edit made a change: an agent edit, by its index in the history, or
/// the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Edit(usize),
    User,
}

/// The selected draft's edit history, its comparison with the draft, and
/// the change selected in it.
#[derive(Default)]
pub struct ReviewState {
    log: EditLog,
    compared: Option<Rc<Compared>>,
    /// Changes in the comparison that aren't kept.
    unreviewed: usize,
    selected: Option<usize>,
}

impl ReviewState {
    pub fn new(log: EditLog) -> Self {
        Self {
            log,
            ..Self::default()
        }
    }

    pub fn log(&self) -> &EditLog {
        &self.log
    }

    pub(crate) fn log_mut(&mut self) -> &mut EditLog {
        &mut self.log
    }

    /// The text before edit [`EditLog::since`], which the draft is compared
    /// with.
    pub fn base(&self) -> Option<&str> {
        let edit = self.log.edits().get(self.log.since())?;
        Some(&edit.before)
    }

    /// Compares `text` with the [`base`](Self::base), reusing the last
    /// comparison while neither changed, and counts the changes not kept.
    /// The selected change stays selected, or the last one if it is gone.
    pub fn compare(&mut self, text: &str) -> Option<Rc<Compared>> {
        let base = &self.log.edits().get(self.log.since())?.before;
        let compared = match &self.compared {
            Some(compared) if compared.base == *base && compared.text == text => compared.clone(),
            _ => {
                let comparison = diff::compare(base, text);
                let compared = Rc::new(Compared {
                    decorations: decorations(&comparison),
                    comparison,
                    base: base.clone(),
                    text: text.to_owned(),
                });
                self.compared = Some(compared.clone());
                compared
            }
        };
        self.count();
        let count = compared.comparison.hunks.len();
        self.selected = self
            .selected
            .map(|selected| selected.min(count.saturating_sub(1)))
            .filter(|_| count > 0);
        Some(compared)
    }

    pub fn comparison(&self) -> Option<&Comparison> {
        self.compared.as_ref().map(|compared| &compared.comparison)
    }

    /// The changes in the last comparison.
    pub fn hunks(&self) -> &[Hunk] {
        self.comparison()
            .map_or(&[], |comparison| comparison.hunks.as_slice())
    }

    /// How many changes in the last comparison aren't kept.
    pub fn unreviewed(&self) -> usize {
        self.unreviewed
    }

    fn count(&mut self) {
        let unreviewed = self
            .hunks()
            .iter()
            .filter(|hunk| !is_kept(&self.log, hunk))
            .count();
        self.unreviewed = unreviewed;
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_hunk(&self) -> Option<&Hunk> {
        self.hunks().get(self.selected?)
    }

    pub fn select(&mut self, selected: Option<usize>) {
        self.selected = selected;
    }

    /// The change a step forward or back from the selected one, wrapping
    /// around; from nothing selected, the first or the last.
    pub fn step(&self, forward: bool) -> Option<usize> {
        let count = self.hunks().len();
        if count == 0 {
            return None;
        }
        Some(match (self.selected, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
        })
    }

    /// The first change after change `ix` that isn't kept, wrapping around.
    pub fn next_unkept(&self, ix: usize) -> Option<usize> {
        let hunks = self.hunks();
        (1..hunks.len())
            .map(|step| (ix + step) % hunks.len())
            .find(|&ix| !is_kept(&self.log, &hunks[ix]))
    }

    pub fn is_kept(&self, ix: usize) -> bool {
        self.hunks()
            .get(ix)
            .is_some_and(|hunk| is_kept(&self.log, hunk))
    }

    /// Whether each change in the last comparison is kept.
    pub fn kept(&self) -> Vec<bool> {
        self.hunks()
            .iter()
            .map(|hunk| is_kept(&self.log, hunk))
            .collect()
    }

    /// Keeps change `ix`, or un-keeps it. Returns false if there is no such
    /// change.
    pub(crate) fn keep(&mut self, ix: usize, kept: bool) -> bool {
        let Some(hunk) = self.hunks().get(ix).cloned() else {
            return false;
        };
        keep(&mut self.log, &hunk, kept);
        self.count();
        true
    }

    /// Keeps every change in the last comparison.
    pub(crate) fn keep_all(&mut self) {
        let Some(compared) = self.compared.clone() else {
            return;
        };
        for hunk in &compared.comparison.hunks {
            keep(&mut self.log, hunk, true);
        }
        self.count();
    }

    /// Compares from before edit `since` on, with no change selected.
    pub fn compare_since(&mut self, since: usize) {
        self.log.set_since(since);
        self.selected = None;
    }

    /// An agent turn added an edit, which the comparison now starts from.
    pub(crate) fn edit_added(&mut self) {
        self.selected = None;
    }

    /// Which edit made change `ix` of the comparison with `text`: the newest
    /// agent edit in the comparison with a change of its own in it, or else
    /// the user.
    pub fn origin(&self, ix: usize, text: &str) -> Option<Origin> {
        let hunk = self.hunks().get(ix)?;
        let edits = self.log.edits();
        let made = |edit: &Hunk| {
            let (added, removed) = (edit.added.trim(), edit.removed.trim());
            (!added.is_empty() && hunk.added.contains(added))
                || (!removed.is_empty() && hunk.removed.contains(removed))
        };
        for (ix, edit) in edits.iter().enumerate().skip(self.log.since()).rev() {
            let after = edits.get(ix + 1).map_or(text, |next| next.before.as_str());
            if diff::compare(&edit.before, after).hunks.iter().any(made) {
                return Some(Origin::Edit(ix));
            }
        }
        Some(Origin::User)
    }
}

/// `hunk`'s key in `log`, for the comparison from edit [`EditLog::since`].
fn key<'a>(log: &EditLog, hunk: &'a Hunk) -> ChangeKey<'a> {
    ChangeKey {
        since: log.since(),
        old_start: hunk.old.start,
        removed: &hunk.removed,
        added: &hunk.added,
    }
}

fn is_kept(log: &EditLog, hunk: &Hunk) -> bool {
    log.is_kept(&key(log, hunk))
}

fn keep(log: &mut EditLog, hunk: &Hunk, kept: bool) {
    let key = key(log, hunk);
    log.keep(&key, kept);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeping_records_the_change_once_and_unkeeping_forgets_it() {
        let mut log = EditLog::default();
        let hunk = diff::compare("a\n", "b\n").hunks[0].clone();
        keep(&mut log, &hunk, true);
        keep(&mut log, &hunk, true);
        assert!(is_kept(&log, &hunk));
        assert_eq!(log.history().kept.len(), 1);
        keep(&mut log, &hunk, false);
        assert!(!is_kept(&log, &hunk));
        assert!(log.history().kept.is_empty());
    }

    const BEFORE: &str = include_str!("../fixtures/review/word-replaced-twice.before.md");
    const AFTER: &str = include_str!("../fixtures/review/word-replaced-twice.after.md");

    #[test]
    fn keeping_a_change_leaves_identical_ones_elsewhere_unkept() {
        let mut log = EditLog::default();
        let hunks = diff::compare(BEFORE, AFTER).hunks;
        assert_eq!(hunks.len(), 2);
        assert_eq!(
            (&hunks[0].removed, &hunks[0].added),
            (&hunks[1].removed, &hunks[1].added)
        );
        keep(&mut log, &hunks[1], true);
        assert!(!is_kept(&log, &hunks[0]));
        assert!(is_kept(&log, &hunks[1]));
    }

    #[test]
    fn a_kept_change_stays_kept_through_edits_elsewhere() {
        let mut log = EditLog::default();
        keep(&mut log, &diff::compare(BEFORE, AFTER).hunks[1], true);
        // Typed between the two changes, moving the kept one in the draft.
        let edited = AFTER.replace("Unrelated text", "Some unrelated text");
        let swaps: Vec<bool> = diff::compare(BEFORE, &edited)
            .hunks
            .iter()
            .filter(|hunk| hunk.added == "a")
            .map(|hunk| is_kept(&log, hunk))
            .collect();
        assert_eq!(swaps, [false, true]);
    }

    #[test]
    fn reverting_a_kept_change_does_not_pass_the_keep_to_its_twin() {
        let mut log = EditLog::default();
        keep(&mut log, &diff::compare(BEFORE, AFTER).hunks[0], true);
        let reverted = AFTER.replacen("a cache", "the cache", 1);
        let hunks = diff::compare(BEFORE, &reverted).hunks;
        assert_eq!(hunks.len(), 1);
        assert!(!is_kept(&log, &hunks[0]));
    }

    #[test]
    fn keeps_belong_to_the_edit_they_were_compared_from() {
        let mut log = EditLog::default();
        let hunk = diff::compare(BEFORE, AFTER).hunks[0].clone();
        log.set_since(1);
        keep(&mut log, &hunk, true);
        log.set_since(0);
        assert!(!is_kept(&log, &hunk));
        log.set_since(1);
        assert!(is_kept(&log, &hunk));
    }
}
