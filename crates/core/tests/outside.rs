//! Changes to a draft's file made outside the editor, by a program or the
//! agent, and the question of which version to keep.

mod support;

use chrono::Duration;
use draftmux_core::history::Recorded;
use draftmux_core::outside::Choice;
use draftmux_storage::Status;
use support::Script;

fn ms(ms: i64) -> Duration {
    Duration::milliseconds(ms)
}

/// When draft `id` was last active.
fn updated_at(s: &Script, id: &str) -> chrono::DateTime<chrono::Utc> {
    s.store.load(id).unwrap().meta.updated_at
}

#[test]
fn an_outside_edit_with_nothing_pending_reloads_without_saving() {
    let mut s = Script::new();
    let id = s.draft("Mine\n");
    s.select(&id);
    let before = updated_at(&s, &id);

    s.write_outside(&id, "Theirs\n");
    s.check_outside();

    assert_eq!(s.editor.text, "Theirs\n");
    assert_eq!(s.session.text(), "Theirs\n");
    assert!(s.questions.is_empty());
    assert_eq!(s.session.drafts().deadline(), None);
    assert!(updated_at(&s, &id) > before);
    s.advance(ms(1000));
    s.leave_window().unwrap();
    assert_eq!(s.text(&id), "Theirs\n");
    assert_eq!(s.history_bytes(&id), None);

    // The file is seen once.
    s.check_outside();
    assert!(s.questions.is_empty());
    assert_eq!(s.editor.text, "Theirs\n");
}

#[test]
fn an_outside_edit_during_a_pending_save_asks_and_holds_the_save() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);

    s.type_text("Mine\n");
    s.advance(ms(300));
    s.write_outside(&id, "Theirs\n");
    s.check_outside();

    assert_eq!(s.questions.len(), 1);
    assert_eq!(s.editor.text, "Mine\n");
    s.advance(ms(300));
    assert_eq!(s.session.save_error(), None);
    assert_eq!(s.text(&id), "Theirs\n");
    assert!(s.session.drafts().deadline().is_some());
    // The save isn't retried while the question is open.
    assert_eq!(s.timer, None);
}

#[test]
fn an_earlier_save_error_is_not_shown_while_the_question_is_open() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.break_meta(&id);
    s.type_text("Mine\n");
    s.advance(ms(600));
    assert!(s.session.save_error().is_some());

    s.write_outside(&id, "Theirs\n");
    s.check_outside();
    assert_eq!(s.questions.len(), 1);
    assert_eq!(s.session.save_error(), None);
    s.advance(ms(1200));
    assert_eq!(s.session.save_error(), None);
    assert_eq!(s.text(&id), "Theirs\n");

    // Keeping the edits tries the save, which reports why it still fails.
    s.answer(Choice::KeepMine);
    assert!(s.session.save_error().is_some());
    assert_eq!(s.text(&id), "Mine\n");
    s.fix_meta(&id);
    s.advance(ms(600));
    assert_eq!(s.session.save_error(), None);
    assert_eq!(s.session.drafts().deadline(), None);
}

#[test]
fn using_the_new_version_replaces_the_text_and_drops_the_pending_edit() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");
    s.check_outside();

    s.answer(Choice::UseNew);

    assert_eq!(s.editor.text, "Theirs\n");
    assert_eq!(s.session.text(), "Theirs\n");
    assert_eq!(s.session.drafts().deadline(), None);
    s.advance(ms(1000));
    s.leave_window().unwrap();
    assert_eq!(s.text(&id), "Theirs\n");
    s.check_outside();
    assert!(s.questions.is_empty());
}

#[test]
fn keeping_my_edits_saves_them_over_the_outside_version() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");
    s.check_outside();

    s.answer(Choice::KeepMine);

    assert_eq!(s.text(&id), "Mine\n");
    assert_eq!(s.editor.text, "Mine\n");
    assert_eq!(s.session.drafts().deadline(), None);
    assert_eq!(s.session.save_error(), None);
    s.check_outside();
    assert!(s.questions.is_empty());
}

#[test]
fn keeping_my_edits_while_saves_fail_reports_the_error_and_keeps_them_pending() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");
    s.check_outside();
    s.break_meta(&id);

    s.answer(Choice::KeepMine);

    assert!(s.session.save_error().is_some());
    assert!(s.session.drafts().deadline().is_some());
    // The text is written before the metadata fails, and taken as the
    // editor's own write.
    assert_eq!(s.text(&id), "Mine\n");
    s.fix_meta(&id);
    s.advance(ms(600));
    assert_eq!(s.session.save_error(), None);
    assert_eq!(s.session.drafts().deadline(), None);
}

#[test]
fn an_answer_after_switching_drafts_is_ignored() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);
    s.type_text("A, mine\n");
    s.write_outside(&a, "A, theirs\n");
    s.check_outside();
    // The file goes back, so the edit can be saved and A left.
    s.write_outside(&a, "A\n");
    s.select(&b);
    s.write_outside(&a, "A, theirs\n");

    s.answer(Choice::UseNew);

    assert_eq!(s.editor.text, "B\n");
    assert_eq!(s.text(&a), "A, theirs\n");
    assert_eq!(s.text(&b), "B\n");
}

#[test]
fn an_answer_after_switching_away_and_back_is_ignored() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);
    s.type_text("A, mine\n");
    s.write_outside(&a, "A, theirs\n");
    s.check_outside();
    let question = s.questions.remove(0);
    s.write_outside(&a, "A\n");
    s.select(&b);
    s.select(&a);
    s.type_text("A, mine again\n");
    s.write_outside(&a, "A, theirs\n");

    s.answer_with(question, Choice::UseNew);

    assert_eq!(s.editor.text, "A, mine again\n");
    assert!(s.session.drafts().deadline().is_some());
    // Answering closed the question, so the next check asks again.
    s.check_outside();
    assert_eq!(s.questions.len(), 1);
    s.answer(Choice::UseNew);
    assert_eq!(s.editor.text, "A, theirs\n");
}

#[test]
fn an_answer_after_the_file_changed_again_is_ignored_and_asked_again() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");
    s.check_outside();
    s.write_outside(&id, "Theirs, again\n");

    s.answer(Choice::UseNew);
    assert_eq!(s.editor.text, "Mine\n");
    assert!(s.session.drafts().deadline().is_some());

    s.check_outside();
    assert_eq!(s.questions.len(), 1);
    s.answer(Choice::UseNew);
    assert_eq!(s.editor.text, "Theirs, again\n");
    assert_eq!(s.session.drafts().deadline(), None);
}

#[test]
fn an_answer_after_the_pending_edit_was_saved_is_ignored() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");
    s.check_outside();
    // Leaving the window saves the edit once the file is back as it was.
    s.write_outside(&id, "One\n");
    s.leave_window().unwrap();
    assert_eq!(s.text(&id), "Mine\n");
    s.write_outside(&id, "Theirs\n");

    s.answer(Choice::UseNew);

    assert_eq!(s.editor.text, "Mine\n");
    assert_eq!(s.text(&id), "Theirs\n");
    // Nothing is pending now, so the next check takes the file's text.
    s.check_outside();
    assert!(s.questions.is_empty());
    assert_eq!(s.editor.text, "Theirs\n");
}

#[test]
fn only_one_question_is_open_at_a_time() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");
    s.check_outside();
    let asked = updated_at(&s, &id);

    s.write_outside(&id, "Theirs, again\n");
    s.check_outside();

    assert_eq!(s.questions.len(), 1);
    assert_eq!(s.editor.text, "Mine\n");
    // While the question is open, the file isn't looked at.
    assert_eq!(updated_at(&s, &id), asked);
}

/// Types "Mine", has the file changed to "Theirs" outside, and dismisses
/// the question that follows once the edit was due.
fn dismissed() -> (Script, String) {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");
    s.check_outside();
    s.advance(ms(600));
    s.answer(Choice::Dismissed);
    (s, id)
}

#[test]
fn a_dismissed_question_is_not_asked_again_while_nothing_changes() {
    let (mut s, id) = dismissed();
    assert_eq!(s.editor.text, "Mine\n");
    assert!(s.session.drafts().deadline().is_some());
    let asked = updated_at(&s, &id);

    for _ in 0..5 {
        s.advance(ms(1000));
        s.check_outside();
    }

    assert!(s.questions.is_empty());
    assert_eq!(s.text(&id), "Theirs\n");
    assert_eq!(s.editor.text, "Mine\n");
    assert!(s.session.drafts().deadline().is_some());
    assert_eq!(s.session.save_error(), None);
    assert_eq!(updated_at(&s, &id), asked);
    // Leaving still refuses to replace the file.
    assert!(s.leave_window().is_err());
    assert_eq!(s.text(&id), "Theirs\n");
}

#[test]
fn a_dismissed_question_is_asked_again_when_the_file_changes_again() {
    let (mut s, id) = dismissed();
    s.check_outside();

    s.write_outside(&id, "Theirs, again\n");
    s.check_outside();

    assert_eq!(s.questions.len(), 1);
    s.answer(Choice::KeepMine);
    assert_eq!(s.text(&id), "Mine\n");
    assert_eq!(s.session.drafts().deadline(), None);
}

#[test]
fn a_dismissed_question_is_asked_again_once_a_later_edit_is_due() {
    let (mut s, id) = dismissed();

    s.type_text("Mine, more\n");
    s.check_outside();
    assert!(s.questions.is_empty());
    s.advance(ms(600));
    assert_eq!(s.text(&id), "Theirs\n");
    assert_eq!(s.timer, None);
    s.check_outside();

    assert_eq!(s.questions.len(), 1);
    s.answer(Choice::UseNew);
    assert_eq!(s.editor.text, "Theirs\n");
    assert_eq!(s.text(&id), "Theirs\n");
}

#[test]
fn an_edit_typed_back_to_the_dismissed_text_is_not_asked_about() {
    let (mut s, id) = dismissed();

    s.type_text("Mine, more\n");
    s.type_text("Mine\n");
    s.advance(ms(600));
    s.check_outside();

    assert!(s.questions.is_empty());
    assert_eq!(s.text(&id), "Theirs\n");
}

#[test]
fn a_held_edit_is_saved_once_the_file_is_back_as_it_was() {
    let (mut s, id) = dismissed();

    s.write_outside(&id, "One\n");
    s.check_outside();
    assert!(s.questions.is_empty());
    s.advance(ms(1));

    assert_eq!(s.text(&id), "Mine\n");
    assert_eq!(s.session.drafts().deadline(), None);
    // A later change is a new one, and asked about if an edit is pending.
    s.type_text("Mine, more\n");
    s.write_outside(&id, "Theirs\n");
    s.check_outside();
    assert_eq!(s.questions.len(), 1);
}

#[test]
fn a_save_refused_before_the_check_saw_the_change_is_asked_about() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");
    s.write_outside(&id, "Theirs\n");

    s.advance(ms(600));
    assert_eq!(s.timer, None);
    s.check_outside();

    assert_eq!(s.questions.len(), 1);
    s.answer(Choice::KeepMine);
    assert_eq!(s.text(&id), "Mine\n");
}

#[test]
fn a_crlf_outside_edit_reloads_without_being_saved_back() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);

    s.write_outside(&id, "One\r\nTwo\r\n");
    s.check_outside();
    assert_eq!(s.editor.text, "One\nTwo\n");

    // The editor reports the replacement as an edit after it is
    // acknowledged.
    s.type_text("One\nTwo\n");
    assert_eq!(s.session.drafts().deadline(), None);
    s.advance(ms(1000));
    s.leave_window().unwrap();
    assert_eq!(s.text(&id), "One\r\nTwo\r\n");
}

#[test]
fn an_agent_write_to_the_selected_draft_reloads_it_and_records_the_turn() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.send("Make it two");
    let before = updated_at(&s, &id);

    assert_eq!(s.agent_writes(&id, "Two\n").unwrap(), Recorded::Added);

    assert_eq!(s.editor.text, "Two\n");
    assert_eq!(s.session.text(), "Two\n");
    assert_eq!(s.session.drafts().deadline(), None);
    assert!(updated_at(&s, &id) > before);
    let history = s.history(&id);
    assert_eq!(history.edits.len(), 1);
    assert_eq!(history.edits[0].before, "One\n");
    assert_eq!(s.log().edits(), history.edits);
    // The poll that follows finds nothing new.
    s.check_outside();
    assert!(s.questions.is_empty());
}

#[test]
fn an_agent_write_during_a_pending_edit_is_asked_about_on_the_next_check() {
    let mut s = Script::new();
    let id = s.draft("One\n");
    s.select(&id);
    s.type_text("Mine\n");

    s.agent_writes(&id, "Theirs\n").unwrap();
    assert_eq!(s.editor.text, "Mine\n");
    assert!(s.questions.is_empty());

    s.check_outside();
    assert_eq!(s.questions.len(), 1);
    s.answer(Choice::UseNew);
    assert_eq!(s.editor.text, "Theirs\n");
}

#[test]
fn a_late_agent_write_after_switching_updates_the_other_draft() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    let b = s.draft("B\n");
    s.select(&a);
    s.send("Change A");
    s.select(&b);
    assert_eq!(s.session.drafts().rows()[0].draft.id, b);

    assert_eq!(s.agent_writes(&a, "A, changed\n").unwrap(), Recorded::Added);

    assert_eq!(s.editor.text, "B\n");
    assert_eq!(s.session.text(), "B\n");
    assert!(s.log().edits().is_empty());
    assert_eq!(s.history(&a).edits[0].before, "A\n");
    let rows = s.session.drafts().rows();
    assert_eq!(rows[0].draft.id, a);
    assert_eq!(rows[0].title, "A, changed");
    s.check_outside();
    assert!(s.questions.is_empty());
    assert_eq!(s.editor.text, "B\n");
}

#[test]
fn a_late_agent_write_after_archiving_records_history_and_keeps_it_archived() {
    let mut s = Script::new();
    let a = s.draft("A\n");
    s.select(&a);
    s.send("Change A");
    s.archive(&a);

    assert_eq!(s.agent_writes(&a, "A, changed\n").unwrap(), Recorded::Added);

    assert_eq!(s.history(&a).edits[0].before, "A\n");
    assert_eq!(s.text(&a), "A, changed\n");
    assert_eq!(s.store.load(&a).unwrap().meta.status, Status::Archived);
    assert!(s.session.drafts().rows().is_empty());
    assert_eq!(s.selected(), None);
    assert_eq!(s.editor.text, "");
}
