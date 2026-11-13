//! A scripted app for workflow tests: a store in a temporary directory, the
//! core state the app keeps, a fake clock and a fake editor.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use draftmux_core::history::{EditLog, Recorded, Turns};
use draftmux_core::text::normalize_line_endings;
use draftmux_storage::{History, NewDraft, Store, Target};
use tempfile::TempDir;

pub struct Script {
    root: TempDir,
    pub store: Store,
    pub turns: Turns,
    /// The selected draft and its loaded log.
    pub selected: Option<String>,
    pub log: EditLog,
    pub editor: FakeEditor,
    t0: DateTime<Utc>,
    elapsed: Duration,
}

/// The editor's text, normalized the way the real buffer normalizes it.
#[derive(Default)]
pub struct FakeEditor {
    pub text: String,
}

impl FakeEditor {
    fn load(&mut self, text: &str) {
        self.text = normalize_line_endings(text);
    }
}

impl Script {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path());
        Self {
            root,
            store,
            turns: Turns::default(),
            selected: None,
            log: EditLog::default(),
            editor: FakeEditor::default(),
            t0: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            elapsed: Duration::zero(),
        }
    }

    /// The fake clock.
    pub fn at(&self) -> DateTime<Utc> {
        self.t0 + self.elapsed
    }

    pub fn advance(&mut self, by: Duration) {
        self.elapsed += by;
    }

    /// Creates a draft holding `text` and returns its id.
    pub fn draft(&mut self, text: &str) -> String {
        self.store
            .create(NewDraft {
                target: Target::Github,
                title: String::new(),
                source: None,
                cwd: "/home/u/code/app".into(),
                pi_agent_dir: "/tmp/.pi/agent".into(),
                note: None,
                settings: Default::default(),
                text: text.to_owned(),
            })
            .unwrap()
            .id
    }

    /// Selects draft `id`: loads its text into the editor and its log.
    pub fn select(&mut self, id: &str) {
        self.editor.load(&self.store.read_text(id).unwrap());
        self.log = EditLog::load(&self.store, id);
        self.selected = Some(id.to_owned());
    }

    pub fn deselect(&mut self) {
        self.editor = FakeEditor::default();
        self.log = EditLog::default();
        self.selected = None;
    }

    /// The user sends `message` to the selected draft's agent, which starts
    /// from the editor's text.
    pub fn send(&mut self, message: &str) {
        let id = self.selected.clone().expect("a selected draft");
        self.turns.started(&id, self.editor.text.clone(), message);
    }

    /// The agent writes `text` to draft `id`'s file, and the app hasn't
    /// seen it yet.
    pub fn agent_saves(&self, id: &str, text: &str) {
        std::fs::write(self.draft_path(id), text).unwrap();
    }

    /// The app sees the agent's write to draft `id`: it reads the file and
    /// records the turn, then shows the text if the draft is selected.
    pub fn observe_write(&mut self, id: &str) -> anyhow::Result<Recorded> {
        let text = std::fs::read_to_string(self.draft_path(id)).unwrap();
        let current = self.selected.as_deref() == Some(id);
        let at = self.at();
        let recorded =
            self.turns
                .agent_wrote(&self.store, id, &text, at, current.then_some(&mut self.log));
        if current {
            self.editor.load(&text);
        }
        recorded
    }

    /// The agent writes `text` to draft `id`, and the app sees it.
    pub fn agent_writes(&mut self, id: &str, text: &str) -> anyhow::Result<Recorded> {
        self.agent_saves(id, text);
        self.observe_write(id)
    }

    /// Archives draft `id`, selecting nothing if it was selected.
    pub fn archive(&mut self, id: &str) {
        self.store.archive(id).unwrap();
        if self.selected.as_deref() == Some(id) {
            self.deselect();
        }
    }

    /// Makes saving draft `id`'s history fail by putting a directory where
    /// the file goes. Reading it fails too, so break it after it's loaded.
    pub fn break_history_save(&self, id: &str) {
        let path = self.history_path(id);
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("blocker"), "").unwrap();
    }

    pub fn fix_history_save(&self, id: &str) {
        std::fs::remove_dir_all(self.history_path(id)).unwrap();
    }

    /// Makes draft `id`'s history unreadable.
    pub fn corrupt_history(&self, id: &str) {
        std::fs::write(self.history_path(id), "{not json").unwrap();
    }

    /// Drops everything the app holds in memory and reopens the same store.
    pub fn crash(&mut self) {
        self.store = Store::new(self.root.path());
        self.turns = Turns::default();
        self.deselect();
    }

    pub fn text(&self, id: &str) -> String {
        self.store.read_text(id).unwrap()
    }

    pub fn history(&self, id: &str) -> History {
        self.store.history(id).unwrap()
    }

    /// The history file's bytes, or `None` while there is none.
    pub fn history_bytes(&self, id: &str) -> Option<Vec<u8>> {
        std::fs::read(self.history_path(id)).ok()
    }

    fn draft_path(&self, id: &str) -> PathBuf {
        self.root.path().join(id).join("draft.md")
    }

    fn history_path(&self, id: &str) -> PathBuf {
        self.root.path().join(id).join("history.json")
    }
}
