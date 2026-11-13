//! Recording agent turns in each draft's edit history.

mod support;

use chrono::Duration;
use draftmux_core::history::Recorded;
use draftmux_storage::{Edit, History};
use support::Script;

#[test]
fn first_write_records_one_edit_and_later_writes_join_it() {
    let mut s = Script::new();
    let id = s.draft("Hello world\n");
    s.select(&id);
    s.send("\n  Make it friendlier  \nand shorter");
    let at = s.at();

    assert_eq!(s.agent_writes(&id, "Hi world\n").unwrap(), Recorded::Added);
    s.advance(Duration::seconds(5));
    assert_eq!(
        s.agent_writes(&id, "Hi there, world\n").unwrap(),
        Recorded::Ignored
    );

    let history = s.history(&id);
    assert_eq!(history.edits.len(), 1);
    assert_eq!(history.edits[0].before, "Hello world\n");
    assert_eq!(history.edits[0].prompt, "Make it friendlier");
    assert_eq!(history.edits[0].at, at);
    assert_eq!(s.log.edits(), history.edits);
    assert_eq!(s.log.since(), 0);
    assert_eq!(s.text(&id), "Hi there, world\n");

    // The next turn adds an edit, and the comparison starts before it.
    s.send("Again");
    assert_eq!(s.agent_writes(&id, "Hi again\n").unwrap(), Recorded::Added);
    let history = s.history(&id);
    assert_eq!(history.edits.len(), 2);
    assert_eq!(history.edits[1].before, "Hi there, world\n");
    assert_eq!(s.log.since(), 1);
}

#[test]
fn a_write_that_changes_nothing_records_nothing() {
    let mut s = Script::new();
    let id = s.draft("Same\n");
    s.select(&id);
    s.send("Leave it");

    assert_eq!(s.agent_writes(&id, "Same\n").unwrap(), Recorded::Ignored);
    assert_eq!(s.history_bytes(&id), None);

    // The turn isn't spent: a later write that changes the draft records it.
    assert_eq!(s.agent_writes(&id, "Changed\n").unwrap(), Recorded::Added);
    assert_eq!(s.history(&id).edits[0].before, "Same\n");
}

#[test]
fn a_write_without_a_turn_records_nothing() {
    let mut s = Script::new();
    let id = s.draft("Text\n");
    s.select(&id);

    assert_eq!(s.agent_writes(&id, "Other\n").unwrap(), Recorded::Ignored);
    assert_eq!(s.history_bytes(&id), None);
}

#[test]
fn the_first_draft_of_an_empty_draft_is_not_an_edit() {
    let mut s = Script::new();
    let id = s.draft(" \n");
    s.select(&id);
    s.send("Write the first draft");

    assert_eq!(
        s.agent_writes(&id, "A first draft\n").unwrap(),
        Recorded::Ignored
    );
    assert_eq!(s.history_bytes(&id), None);
    assert!(s.log.edits().is_empty());
}

#[test]
fn a_failed_history_save_records_the_turn_on_its_next_write() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.send("Change it");
    s.break_history_save(&id);

    assert!(s.agent_writes(&id, "Two\n").is_err());
    assert!(s.log.edits().is_empty());
    assert_eq!(s.text(&id), "Two\n");

    s.fix_history_save(&id);
    assert_eq!(s.agent_writes(&id, "Three\n").unwrap(), Recorded::Added);
    let history = s.history(&id);
    assert_eq!(history.edits.len(), 1);
    assert_eq!(history.edits[0].before, "One\n");
    assert_eq!(s.log.edits(), history.edits);
}

#[test]
fn a_write_to_an_unselected_draft_extends_its_saved_history() {
    let mut s = Script::new();
    let a = s.draft("A one\n");
    let b = s.draft("B one\n");
    s.select(&a);
    s.send("First");
    s.agent_writes(&a, "A two\n").unwrap();
    s.send("Second");
    s.select(&b);

    assert_eq!(s.agent_writes(&a, "A three\n").unwrap(), Recorded::Added);
    let history = s.history(&a);
    let befores: Vec<&str> = history
        .edits
        .iter()
        .map(|edit| edit.before.as_str())
        .collect();
    assert_eq!(befores, ["A one\n", "A two\n"]);
    assert!(s.log.edits().is_empty());
    assert_eq!(s.history_bytes(&b), None);
}

#[test]
fn a_write_while_the_history_is_unreadable_leaves_the_file_alone() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.corrupt_history(&id);
    s.select(&id);
    s.send("Change it");
    let corrupt = s.history_bytes(&id);

    assert!(s.agent_writes(&id, "Two\n").is_err());
    assert_eq!(s.history_bytes(&id), corrupt);

    // Once the file reads again, the log is replaced by what was saved and
    // compares from the new edit.
    let repaired = History {
        edits: vec![Edit {
            before: "Zero\n".into(),
            at: s.at(),
            prompt: "Old".into(),
        }],
        kept: vec!["k".into()],
    };
    s.store.save_history(&id, &repaired).unwrap();
    assert_eq!(s.agent_writes(&id, "Three\n").unwrap(), Recorded::Added);
    let history = s.history(&id);
    assert_eq!(history.edits.len(), 2);
    assert_eq!(history.edits[1].before, "One\n");
    assert_eq!(history.kept, ["k"]);
    assert_eq!(s.log.history(), &history);
    assert_eq!(s.log.since(), 1);
}

#[test]
fn a_write_after_archiving_still_records() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.send("Change it");
    s.archive(&id);

    assert_eq!(s.agent_writes(&id, "Two\n").unwrap(), Recorded::Added);
    let history = s.history(&id);
    assert_eq!(history.edits.len(), 1);
    assert_eq!(history.edits[0].before, "One\n");
    assert_eq!(s.text(&id), "Two\n");
}

#[test]
fn a_crash_before_history_persists_keeps_the_text_and_loses_the_turn() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.send("First");
    s.agent_writes(&id, "Two\n").unwrap();
    let saved = s.history_bytes(&id).unwrap();

    s.send("Second");
    s.agent_saves(&id, "Three\n");
    s.crash();

    assert_eq!(s.text(&id), "Three\n");
    assert_eq!(s.history_bytes(&id).unwrap(), saved);
    assert_eq!(s.history(&id).edits.len(), 1);
    s.select(&id);
    assert_eq!(s.observe_write(&id).unwrap(), Recorded::Ignored);
    assert_eq!(s.history_bytes(&id).unwrap(), saved);
}

#[test]
fn a_crlf_write_equal_to_the_normalized_baseline_records_an_edit() {
    let mut s = Script::new();
    let id = s.draft("One\r\nTwo\r\n");
    s.select(&id);
    assert_eq!(s.editor.text, "One\nTwo\n");
    s.send("Leave it");

    assert_eq!(
        s.agent_writes(&id, "One\r\nTwo\r\n").unwrap(),
        Recorded::Added
    );
    assert_eq!(s.history(&id).edits[0].before, "One\nTwo\n");
}
