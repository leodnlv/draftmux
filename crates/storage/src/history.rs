use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Contents of a draft's `history.json`: the agent's edits to the draft,
/// oldest first, and the changes the user has reviewed. Reviewing never
/// rewrites the edits; each new agent edit appends one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub edits: Vec<Edit>,
    /// Keys of the changes the user kept, which stay kept whichever edits
    /// they are compared across.
    #[serde(default)]
    pub kept: Vec<String>,
}

/// One agent edit to a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    /// The draft text before the edit.
    pub before: String,
    pub at: DateTime<Utc>,
    /// The first line of the user's message that asked for the edit.
    pub prompt: String,
}
