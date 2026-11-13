//! The editing session: which draft is selected, the text the editor holds,
//! when its pending edit is saved, and how a failed save is reported.
//!
//! The app reports what happens (a selection, an edit, its autosave timer
//! firing, a prompt) and carries out the returned [`Outcome`]: the effects in
//! order, then its autosave timer rearmed from [`Outcome::deadline`].

use std::time::Instant;

use anyhow::Result;
use draftmux_storage::{Conflict, Store};

use crate::drafts::Drafts;
use crate::history::Turns;

/// Identifies one stay on a selection. A new one starts on every select and
/// deselect, so a load or timer meant for an earlier one is told apart even
/// when the same draft is selected again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Epoch(u64);

/// What the app does after a call.
#[derive(Debug, Default)]
pub struct Outcome {
    pub effects: Vec<Effect>,
    /// When the pending edit is due to be saved. While it is set, the app
    /// arms its autosave timer for it and then calls [`Session::wake`].
    pub deadline: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Show `text` in the editor, then report what it holds with
    /// [`Session::loaded`].
    LoadText { epoch: Epoch, text: String },
    /// Send `text` to the agent. `saved` is the draft text the agent
    /// starts from, or why it couldn't be saved first.
    Prompt {
        text: String,
        saved: Result<String, String>,
    },
}

pub struct Session {
    drafts: Drafts,
    /// Each draft's latest agent turn, until its first write is recorded as
    /// an edit.
    turns: Turns,
    /// Why the selected draft's last save failed; cleared by the next save.
    save_error: Option<String>,
    epoch: Epoch,
    /// The text the editor holds, as last reported.
    text: String,
}

impl Session {
    pub fn new(store: Store) -> Self {
        Self {
            drafts: Drafts::new(store),
            turns: Turns::default(),
            save_error: None,
            epoch: Epoch::default(),
            text: String::new(),
        }
    }

    /// The drafts list and the selected draft.
    pub fn drafts(&self) -> &Drafts {
        &self.drafts
    }

    /// The drafts list, for changes that leave the selection and its
    /// pending edit alone. Selecting, saving and archiving go through the
    /// session.
    pub fn drafts_mut(&mut self) -> &mut Drafts {
        &mut self.drafts
    }

    pub fn turns_mut(&mut self) -> &mut Turns {
        &mut self.turns
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The text the editor holds, as last reported by
    /// [`edited`](Self::edited) or [`loaded`](Self::loaded).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Why the selected draft's last save failed. A save refused because
    /// the file changed outside the editor isn't a failure: the user is
    /// asked which version to keep.
    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    /// Selects `id` and asks for its text to be loaded. Saves the previous
    /// draft first and stays on it if that fails.
    pub fn select(&mut self, id: &str) -> Result<Outcome> {
        let text = self.drafts.select(id)?;
        self.save_error = None;
        self.epoch.0 += 1;
        Ok(self.load(text))
    }

    /// Empties the editor after the selected draft left the list.
    pub fn deselect(&mut self) -> Outcome {
        self.epoch.0 += 1;
        self.load(String::new())
    }

    fn load(&mut self, text: String) -> Outcome {
        self.text.clone_from(&text);
        Outcome {
            effects: vec![Effect::LoadText {
                epoch: self.epoch,
                text,
            }],
            deadline: None,
        }
    }

    /// Records the text the editor holds after loading the selected
    /// draft's text, which may differ from the file in line endings, so
    /// loading isn't saved as an edit. Ignored for an earlier selection.
    pub fn loaded(&mut self, epoch: Epoch, text: &str) {
        if epoch != self.epoch {
            return;
        }
        self.drafts.loaded(text);
        text.clone_into(&mut self.text);
    }

    /// Reports the editor text after a user edit, and restarts the
    /// debounce.
    pub fn edited(&mut self, text: String, now: Instant) -> Outcome {
        self.drafts.edited(&text, now);
        self.text = text;
        self.due()
    }

    /// The autosave timer armed during `epoch` fired: saves the pending
    /// edit if it is due. A failed save keeps it pending and is retried
    /// after another delay.
    pub fn wake(&mut self, epoch: Epoch, now: Instant) -> Outcome {
        if epoch != self.epoch {
            return Outcome::default();
        }
        match self.drafts.poll(now) {
            Ok(()) => self.save_error = None,
            Err(err) => {
                self.save_failed(&err);
                self.drafts.edited(&self.text, now);
            }
        }
        self.due()
    }

    /// Saves the pending edit now, as the window is left or closed, the app
    /// quits, or before a prompt. The edit stays pending on failure, so a
    /// close or quit waiting on this should be refused.
    pub fn flush(&mut self) -> Result<()> {
        let result = self.drafts.flush();
        match &result {
            Ok(()) => self.save_error = None,
            Err(err) => self.save_failed(err),
        }
        result
    }

    fn save_failed(&mut self, err: &anyhow::Error) {
        if err.is::<Conflict>() {
            return;
        }
        eprintln!("draftmux: saving draft: {err:#}");
        self.save_error = Some(format!("Couldn't save: {err:#}"));
    }

    /// Sends `prompt`, whose user-written part is `message`, to the selected
    /// draft's agent. The agent reads the draft from disk, so it is saved
    /// first, and the turn starts from the saved text only if that worked.
    pub fn send(&mut self, prompt: String, message: &str) -> Outcome {
        let saved = self
            .flush()
            .map(|()| self.text.clone())
            .map_err(|err| format!("{err:#}"));
        if let (Ok(before), Some(id)) = (&saved, self.drafts.current_id()) {
            self.turns.started(id, before.clone(), message);
        }
        Outcome {
            effects: vec![Effect::Prompt {
                text: prompt,
                saved,
            }],
            deadline: self.drafts.deadline(),
        }
    }

    /// Archives `id`; see [`Drafts::archive`]. The selected draft is saved
    /// first and stays selected if that fails.
    pub fn archive(&mut self, id: &str) -> Result<Option<String>> {
        self.drafts.archive(id)
    }

    fn due(&self) -> Outcome {
        Outcome {
            effects: Vec::new(),
            deadline: self.drafts.deadline(),
        }
    }
}
