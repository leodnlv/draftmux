//! Saving the selected draft: the autosave debounce and its retries, the
//! saves before leaving it, and how failures are reported.

mod support;

use chrono::Duration;
use draftmux_core::history::Recorded;
use draftmux_storage::Status;
use support::Script;

fn ms(ms: i64) -> Duration {
    Duration::milliseconds(ms)
}

#[test]
fn typing_saves_once_the_debounce_passes() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);

    s.type_text("One two\n");
    s.advance(ms(300));
    s.type_text("One two three\n");
    s.advance(ms(500));
    assert_eq!(s.text(&id), "One\n");

    s.advance(ms(100));
    assert_eq!(s.text(&id), "One two three\n");
    assert_eq!(s.session.drafts().deadline(), None);
    assert_eq!(s.session.save_error(), None);
}

#[test]
fn a_failed_save_keeps_the_text_pending_and_retries_after_the_delay() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.break_meta(&id);

    s.type_text("Two\n");
    s.advance(ms(600));
    let error = s.session.save_error().expect("a save error").to_owned();
    assert!(error.starts_with("Couldn't save: "), "{error}");
    let retry = s.session.drafts().deadline().expect("a pending edit");
    assert_eq!(retry, s.now() + ms(600).to_std().unwrap());
    // The text is written before the metadata fails.
    assert_eq!(s.text(&id), "Two\n");

    // Each retry that fails waits another delay.
    s.advance(ms(600));
    assert_eq!(s.session.save_error(), Some(error.as_str()));
    assert!(s.session.drafts().deadline().is_some());

    s.fix_meta(&id);
    s.advance(ms(600));
    assert_eq!(s.session.save_error(), None);
    assert_eq!(s.session.drafts().deadline(), None);
    assert_eq!(s.text(&id), "Two\n");
}

#[test]
fn a_save_failure_then_switching_drafts_stays_on_the_draft() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);
    s.break_meta(&a);
    s.type_text("A, edited\n");
    s.advance(ms(600));
    let error = s.session.save_error().map(str::to_owned);
    assert!(error.is_some());

    assert!(s.try_select(&b).is_err());
    assert_eq!(s.selected().as_deref(), Some(a.as_str()));
    assert_eq!(s.editor.text, "A, edited\n");
    assert_eq!(s.session.text(), "A, edited\n");
    assert!(s.session.drafts().deadline().is_some());
    assert_eq!(s.session.save_error().map(str::to_owned), error);

    s.fix_meta(&a);
    s.try_select(&b).unwrap();
    assert_eq!(s.selected().as_deref(), Some(b.as_str()));
    assert_eq!(s.text(&a), "A, edited\n");
    assert_eq!(s.editor.text, "B\n");
    assert_eq!(s.session.save_error(), None);
}

#[test]
fn a_save_failure_then_archiving_keeps_the_draft() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    s.select(&a);
    s.break_meta(&a);
    s.type_text("A, edited\n");

    assert!(s.try_archive(&a).is_err());
    s.fix_meta(&a);
    assert_eq!(s.store.load(&a).unwrap().meta.status, Status::Active);
    assert_eq!(s.selected().as_deref(), Some(a.as_str()));
    assert!(s.session.drafts().deadline().is_some());

    s.archive(&a);
    assert_eq!(s.store.load(&a).unwrap().meta.status, Status::Archived);
    assert_eq!(s.text(&a), "A, edited\n");
    assert_eq!(s.selected(), None);
}

#[test]
fn a_conflict_is_not_reported_as_a_save_error() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);

    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");
    s.advance(ms(600));

    assert_eq!(s.session.save_error(), None);
    assert_eq!(s.text(&id), "Theirs\n");
    // The edit stays pending and the save is retried.
    assert!(s.session.drafts().deadline().is_some());
    assert!(s.leave_window().is_err());
    assert_eq!(s.session.save_error(), None);
}

#[test]
fn leaving_the_window_saves_pending_edits() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);

    s.type_text("Two\n");
    s.leave_window().unwrap();

    assert_eq!(s.text(&id), "Two\n");
    assert_eq!(s.session.drafts().deadline(), None);
    // The timer that was armed finds nothing left to save.
    s.advance(ms(600));
    assert_eq!(s.text(&id), "Two\n");
}

#[test]
fn closing_with_a_failing_save_is_refused_and_keeps_the_text() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.break_meta(&id);
    s.type_text("Two\n");

    assert!(s.session.flush().is_err());
    assert!(s.session.save_error().is_some());
    assert_eq!(s.session.text(), "Two\n");
    assert!(s.session.drafts().deadline().is_some());

    s.fix_meta(&id);
    s.session.flush().unwrap();
    assert_eq!(s.session.save_error(), None);
    assert_eq!(s.session.drafts().deadline(), None);
}

#[test]
fn sending_saves_first_and_starts_the_turn() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Two\n");

    s.send("Make it three");
    assert_eq!(s.text(&id), "Two\n");
    assert_eq!(
        s.prompts,
        [("Make it three".to_owned(), Ok("Two\n".to_owned()))]
    );
    assert_eq!(s.session.drafts().deadline(), None);

    assert_eq!(s.agent_writes(&id, "Three\n").unwrap(), Recorded::Added);
    let history = s.history(&id);
    assert_eq!(history.edits[0].before, "Two\n");
    assert_eq!(history.edits[0].prompt, "Make it three");
}

#[test]
fn sending_after_a_failed_save_starts_no_turn_and_passes_the_error() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.break_meta(&id);
    s.type_text("Two\n");

    s.send("Make it three");
    let (text, saved) = &s.prompts[0];
    assert_eq!(text, "Make it three");
    let error = saved.as_ref().unwrap_err();
    assert_eq!(
        s.session.save_error(),
        Some(format!("Couldn't save: {error}").as_str())
    );

    s.fix_meta(&id);
    assert_eq!(s.agent_writes(&id, "Three\n").unwrap(), Recorded::Ignored);
    assert_eq!(s.history_bytes(&id), None);
}

#[test]
fn a_wake_from_before_a_switch_does_not_save_the_new_draft() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);
    s.type_text("A, edited\n");
    let (stale, _) = s.timer.unwrap();

    s.select(&b);
    assert_eq!(s.text(&a), "A, edited\n");
    s.type_text("B, edited\n");
    let later = s.now() + ms(1000).to_std().unwrap();
    let outcome = s.session.wake(stale, later);
    assert_eq!(outcome.deadline, None);
    assert_eq!(s.text(&b), "B\n");
    assert!(s.session.drafts().deadline().is_some());

    s.advance(ms(600));
    assert_eq!(s.text(&b), "B, edited\n");
}

#[test]
fn loading_crlf_text_is_not_an_edit() {
    let mut s = Script::new();
    let id = s.draft("One\r\nTwo\r\n");
    s.select(&id);
    assert_eq!(s.editor.text, "One\nTwo\n");
    assert_eq!(s.session.text(), "One\nTwo\n");

    // The editor reports the load as an edit after it is acknowledged.
    s.type_text("One\nTwo\n");
    assert_eq!(s.session.drafts().deadline(), None);
    s.advance(ms(1000));
    s.leave_window().unwrap();
    assert_eq!(s.text(&id), "One\r\nTwo\r\n");
}

#[test]
fn a_load_for_an_earlier_selection_is_ignored() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);
    let stale = s.session.epoch();
    s.select(&b);

    s.session.loaded(stale, "A\n");
    assert_eq!(s.session.text(), "B\n");
    s.type_text("B\n");
    assert_eq!(s.session.drafts().deadline(), None);
}
