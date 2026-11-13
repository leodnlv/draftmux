//! Changes made to the selected draft's file outside the editor while the
//! user has edits that aren't saved yet, and the question of which version
//! to keep.

use crate::session::Epoch;

/// Asks the user whether to use the version on disk or keep their edits.
/// Answered with [`Session::answer`](crate::session::Session::answer), which
/// ignores the answer once it no longer applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub(crate) epoch: Epoch,
    pub(crate) id: String,
    /// The file's text when the question was asked.
    pub(crate) disk: String,
}

/// The user's answer to a [`Question`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Replace the editor's text with the file's.
    UseNew,
    /// Save the editor's text over the file's.
    KeepMine,
    /// Closed without choosing.
    Dismissed,
}
