mod support;

use chrono::Duration;
use draftmux_core::outside::Choice;
use support::Script;

#[test]
fn flushing_a_dismissed_conflict_reports_why_it_cannot_close() {
    let mut s = Script::new();
    let id = s.draft("Original");
    s.select(&id);
    s.type_text("Mine");
    s.write_outside(&id, "Theirs");
    s.check_outside();
    s.answer(Choice::Dismissed);
    assert!(s.session.flush().is_err());
    assert!(s.session.save_error().is_some());
    assert_eq!(s.text(&id), "Theirs");
    assert_eq!(s.editor.text, "Mine");
    s.check_outside();
    assert!(s.questions.is_empty());
}

#[test]
fn selecting_another_draft_releases_the_old_question() {
    let mut s = Script::new();
    let a = s.draft("A");
    let b = s.draft("B");
    s.select(&a);
    s.type_text("Mine");
    s.write_outside(&a, "Theirs");
    s.check_outside();
    s.write_outside(&a, "A");
    s.select(&b);
    s.type_text("Edited B");
    s.advance(Duration::milliseconds(600));
    assert_eq!(s.text(&b), "Edited B");
    s.type_text("Mine B");
    s.write_outside(&b, "Theirs B");
    s.check_outside();
    assert_eq!(s.questions.len(), 2);
    s.answer(Choice::Dismissed);
    s.check_outside();
    assert_eq!(
        s.questions.len(),
        1,
        "old answer must not clear the new question"
    );
}

#[test]
fn deselecting_clears_the_previous_save_error() {
    let mut s = Script::new();
    let id = s.draft("A");
    s.select(&id);
    s.break_meta(&id);
    s.type_text("B");
    assert!(s.session.flush().is_err());
    s.deselect();
    assert_eq!(s.session.save_error(), None);
}

#[test]
fn accepted_first_draft_retries_metadata_without_resending() {
    let mut s = Script::new();
    let id = s.handed_off();
    s.select(&id);
    s.first_draft_ready(&id);
    s.break_meta(&id);
    s.accepted(&id);
    s.first_draft_ready(&id);
    assert_eq!(s.prompts.len(), 1);
    assert!(!s.session.writing());
    s.fix_meta(&id);
    s.first_draft_ready(&id);
    assert_eq!(s.prompts.len(), 1);
    assert!(!s.store.load(&id).unwrap().meta.first_draft_pending);
    s.restart_with(&id);
    s.first_draft_ready(&id);
    assert_eq!(s.prompts.len(), 1);
}

#[test]
fn review_actions_refuse_a_comparison_from_before_typing() {
    let mut s = Script::new();
    let id = s.draft("a cat");
    s.select(&id);
    s.send("Change animal");
    s.agent_writes(&id, "a dog").unwrap();
    // Report typing without the harness recomputing the comparison.
    s.editor.text = "🦀 a dog".into();
    s.session.edited(s.editor.text.clone(), s.now());
    assert!(s.session.revert(0).is_err());
    assert!(s.session.restore_before().is_err());
    s.session.sync_review();
    assert!(s.session.revert(0).is_ok());
    let outcome = s.session.restore_before().unwrap();
    s.apply(outcome);
    assert_eq!(s.editor.text, "a cat");
}

#[test]
fn deselecting_releases_the_old_question() {
    let mut s = Script::new();
    let id = s.draft("A");
    s.select(&id);
    s.type_text("Mine");
    s.write_outside(&id, "Theirs");
    s.check_outside();
    s.write_outside(&id, "A");
    s.archive(&id);
    let other = s.draft("B");
    s.select(&other);
    s.write_outside(&other, "New B");
    s.check_outside();
    assert_eq!(s.editor.text, "New B");
}
