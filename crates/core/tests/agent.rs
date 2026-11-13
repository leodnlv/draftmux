//! What the drafts' agents do to the editor: read-only while one runs, and
//! the first draft of a handed-off draft.

mod support;

use draftmux_core::history::Recorded;
use support::Script;

/// Whether the selected draft's first draft is still to be requested.
fn pending(s: &Script) -> bool {
    let draft = s.session.drafts().current().unwrap();
    draft.meta.first_draft_pending
}

#[test]
fn running_makes_only_that_drafts_editor_read_only() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);

    s.running(&a, true);
    assert!(s.session.editor_read_only());
    assert!(s.session.writing());

    s.select(&b);
    assert!(!s.session.editor_read_only());
    assert!(!s.session.writing());

    s.select(&a);
    s.running(&a, false);
    assert!(!s.session.editor_read_only());
    assert!(!s.session.writing());
}

#[test]
fn running_reported_for_another_draft_leaves_the_selected_editor_writable() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);

    s.running(&b, true);

    assert!(!s.session.editor_read_only());
    assert!(!s.session.writing());
}

#[test]
fn selecting_a_draft_takes_its_last_running_state() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);
    s.running(&b, true);

    s.select(&b);
    assert!(s.session.editor_read_only());

    s.running(&b, false);
    s.select(&a);
    s.select(&b);
    assert!(!s.session.editor_read_only());

    s.running(&b, true);
    s.archive(&b);
    assert!(!s.session.editor_read_only());
    assert!(!s.session.writing());
}

#[test]
fn the_first_draft_of_an_empty_draft_is_prompted_and_no_longer_pending() {
    let mut s = Script::new();
    let id = s.handed_off();
    s.select(&id);
    assert!(s.session.writing());
    assert!(!s.session.editor_read_only());

    s.first_draft_ready(&id);

    assert_eq!(
        s.prompts,
        [("Write the first draft".to_owned(), Ok(String::new()))]
    );
    assert!(s.offers.is_empty());
    assert!(!pending(&s));
    assert!(!s.store.load(&id).unwrap().meta.first_draft_pending);
    assert!(!s.session.writing());
}

#[test]
fn the_first_draft_request_is_offered_when_the_user_already_typed() {
    let mut s = Script::new();
    let id = s.handed_off();
    s.select(&id);
    s.type_text("My own start\n");

    s.first_draft_ready(&id);

    assert_eq!(s.offers, ["Write the first draft"]);
    assert!(s.prompts.is_empty());
    assert!(!s.store.load(&id).unwrap().meta.first_draft_pending);
    // Offering saves nothing; the typed text waits for autosave.
    assert_eq!(s.text(&id), "");
    assert!(s.session.drafts().deadline().is_some());
}

#[test]
fn a_whitespace_only_editor_is_saved_and_prompted() {
    let mut s = Script::new();
    let id = s.handed_off();
    s.select(&id);
    s.type_text(" \n");

    s.first_draft_ready(&id);

    assert_eq!(
        s.prompts,
        [("Write the first draft".to_owned(), Ok(" \n".to_owned()))]
    );
    assert_eq!(s.text(&id), " \n");
}

#[test]
fn a_first_draft_request_whose_save_fails_is_prompted_with_the_error() {
    let mut s = Script::new();
    let id = s.handed_off();
    s.select(&id);
    s.type_text("\n");
    s.break_meta(&id);

    s.first_draft_ready(&id);

    let [(_, Err(err))] = s.prompts.as_slice() else {
        panic!("expected a failed save, got {:?}", s.prompts);
    };
    assert!(!err.is_empty());
    assert!(s.session.save_error().is_some());
}

#[test]
fn a_retried_first_draft_request_goes_through_send_and_records_no_edit() {
    let mut s = Script::new();
    let id = s.handed_off();
    s.select(&id);
    s.first_draft_ready(&id);
    // The agent refused the prompt; Retry sends it again like a message.
    let (prompt, _) = s.prompts[0].clone();
    s.send(&prompt);
    assert_eq!(s.prompts.len(), 2);
    s.running(&id, true);

    assert_eq!(
        s.agent_writes(&id, "A first draft\n").unwrap(),
        Recorded::Ignored
    );

    s.running(&id, false);
    assert_eq!(s.editor.text, "A first draft\n");
    assert_eq!(s.history_bytes(&id), None);
    assert!(s.log().edits().is_empty());
}

#[test]
fn a_first_draft_request_is_not_repeated_after_a_restart() {
    let mut s = Script::new();
    let id = s.handed_off();
    s.select(&id);
    s.first_draft_ready(&id);
    // The app quits before the agent accepts the prompt.

    s.crash();
    s.session.drafts_mut().refresh();
    s.select(&id);

    assert!(!pending(&s));
    assert!(!s.session.writing());
}

#[test]
fn a_first_draft_ready_for_an_unselected_draft_is_ignored() {
    let mut s = Script::new();
    let id = s.handed_off();
    let other = s.draft("Other\n");
    s.select(&other);

    s.first_draft_ready(&id);

    assert!(s.prompts.is_empty());
    assert!(s.offers.is_empty());
    assert!(s.store.load(&id).unwrap().meta.first_draft_pending);
}

#[test]
fn a_first_draft_request_starts_no_turn() {
    let mut s = Script::new();
    let id = s.handed_off();
    s.select(&id);
    s.first_draft_ready(&id);

    assert_eq!(
        s.agent_writes(&id, "A first draft\n").unwrap(),
        Recorded::Ignored
    );
    assert_eq!(s.editor.text, "A first draft\n");
    assert_eq!(s.history_bytes(&id), None);
}
