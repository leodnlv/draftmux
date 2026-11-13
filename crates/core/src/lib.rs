//! What happens to the user's text, without a UI: the decisions the app
//! makes about drafts, independent of how they are shown.
//!
//! Storage calls are synchronous against [`draftmux_storage::Store`], and
//! time is always an argument.

pub mod decorations;
pub mod diff;
pub mod drafts;
pub mod history;
pub mod outside;
pub mod review;
pub mod session;
pub mod text;
