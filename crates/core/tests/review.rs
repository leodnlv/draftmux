//! Reviewing the agent's edits: keeping, reverting and restoring changes,
//! and which comparison they belong to.

mod support;

use draftmux_core::review::Origin;
use draftmux_core::session::Effect;
use support::Script;

const BEFORE: &str = include_str!("../fixtures/review/word-replaced-twice.before.md");
const AFTER: &str = include_str!("../fixtures/review/word-replaced-twice.after.md");
/// Three changes.
const THREE_BEFORE: &str = "Ship on Monday.\n\nTest on Monday.\n\nPlan on Monday.\n";
const THREE_AFTER: &str = "Ship on Friday.\n\nTest on Friday.\n\nPlan on Friday.\n";

/// A selected draft that read `before` until an agent turn asked for `prompt`
/// rewrote it to `after`. Returns its id.
fn edited(s: &mut Script, before: &str, prompt: &str, after: &str) -> String {
    let id = s.draft(before);
    s.select(&id);
    s.send(prompt);
    s.agent_writes(&id, after).unwrap();
    id
}

/// A second agent turn on the selected draft `id`.
fn edit_again(s: &mut Script, id: &str, prompt: &str, after: &str) {
    s.send(prompt);
    s.agent_writes(id, after).unwrap();
}

#[test]
fn identical_changes_are_kept_one_at_a_time_across_revisions() {
    let mut s = Script::new();
    let id = edited(&mut s, BEFORE, "Use a", AFTER);
    assert_eq!(s.review().hunks().len(), 2);

    s.select_change(Some(1));
    s.keep_change().unwrap();
    assert_eq!(s.review().kept(), [false, true]);

    // Typed between the two changes, moving the kept one in the draft.
    let typed = AFTER.replace("Unrelated text", "Some unrelated text");
    s.type_text(&typed);
    let swaps: Vec<bool> = s
        .review()
        .hunks()
        .iter()
        .zip(s.review().kept())
        .filter(|(hunk, _)| hunk.added == "a")
        .map(|(_, kept)| kept)
        .collect();
    assert_eq!(swaps, [false, true]);

    // A second turn compares from its own edit, which the swaps predate.
    let again = format!("{typed}A closing line.\n");
    edit_again(&mut s, &id, "Add a closing line", &again);
    assert_eq!(s.log().range_name(), "Last edit");
    assert_eq!(s.review().hunks().len(), 1);
    assert_eq!(s.review().kept(), [false]);
    s.select_change(Some(0));
    s.keep_change().unwrap();

    // Both edits compare from the first one's baseline, where the swap was
    // kept; the closing line was kept against another baseline.
    s.compare_since(0);
    assert_eq!(s.log().range_name(), "All 2 edits");
    let kept: Vec<(String, bool)> = s
        .review()
        .hunks()
        .iter()
        .map(|hunk| hunk.added.trim().to_owned())
        .zip(s.review().kept())
        .collect();
    let expected = [
        ("a", false),
        ("Some unrelated", false),
        ("a", true),
        ("A closing line.", false),
    ]
    .map(|(added, kept)| (added.to_owned(), kept));
    assert_eq!(kept, expected);

    s.compare_since(1);
    assert_eq!(s.review().kept(), [true]);

    // Keeps are saved with the history.
    s.restart_with(&id);
    s.compare_since(0);
    assert_eq!(s.review().kept(), [false, false, true, false]);
}

#[test]
fn keeping_moves_to_the_next_unkept_change() {
    let mut s = Script::new();
    edited(&mut s, THREE_BEFORE, "Make it Friday", THREE_AFTER);
    assert_eq!(s.review().hunks().len(), 3);

    s.select_change(Some(1));
    s.keep_change().unwrap();
    assert_eq!(s.review().selected(), Some(2));
    s.keep_change().unwrap();
    assert_eq!(s.review().selected(), Some(0));
    s.keep_change().unwrap();
    assert_eq!(s.review().selected(), Some(0));
    assert_eq!(s.review().unreviewed(), 0);

    // Un-keeping stays on the change.
    s.keep_change().unwrap();
    assert_eq!(s.review().selected(), Some(0));
    assert_eq!(s.review().kept(), [false, true, true]);
    assert_eq!(s.review().unreviewed(), 1);
}

#[test]
fn stepping_wraps_around_the_changes() {
    let mut s = Script::new();
    edited(&mut s, THREE_BEFORE, "Make it Friday", THREE_AFTER);

    assert_eq!(s.review().step(true), Some(0));
    assert_eq!(s.review().step(false), Some(2));
    s.select_change(Some(2));
    assert_eq!(s.review().step(true), Some(0));
    s.select_change(Some(0));
    assert_eq!(s.review().step(false), Some(2));
}

#[test]
fn finishing_keeps_every_change() {
    let mut s = Script::new();
    let id = edited(&mut s, THREE_BEFORE, "Make it Friday", THREE_AFTER);

    s.session.finish().unwrap();

    assert_eq!(s.review().unreviewed(), 0);
    assert_eq!(s.review().kept(), [true, true, true]);
    assert_eq!(s.history(&id).kept.len(), 3);
}

#[test]
fn a_failed_keep_save_is_reported() {
    let mut s = Script::new();
    let id = edited(&mut s, BEFORE, "Use a", AFTER);
    s.break_history_save(&id);

    s.select_change(Some(0));
    assert!(s.keep_change().is_err());
    assert!(s.session.finish().is_err());

    // The keep stands for this run of the app.
    assert_eq!(s.review().kept(), [true, true]);
}

#[test]
fn reverting_a_change_undoes_it_in_the_editor_and_un_keeps_it() {
    let mut s = Script::new();
    let id = edited(&mut s, BEFORE, "Use a", AFTER);
    s.select_change(Some(0));
    s.keep_change().unwrap();
    assert_eq!(s.history(&id).kept.len(), 1);

    s.select_change(Some(0));
    s.revert_change().unwrap();

    assert_eq!(s.editor.text, AFTER.replacen("a cache", "the cache", 1));
    assert_eq!(s.session.text(), s.editor.text);
    assert!(s.history(&id).kept.is_empty());
    assert_eq!(s.review().hunks().len(), 1);
    assert_eq!(s.review().kept(), [false]);
}

#[test]
fn restoring_goes_back_to_the_compared_text() {
    let mut s = Script::new();
    let id = edited(&mut s, BEFORE, "Use a", AFTER);
    edit_again(&mut s, &id, "Add a line", &format!("{AFTER}\nMore.\n"));
    s.compare_since(0);

    let outcome = s.session.restore_before().unwrap();
    s.apply(outcome);

    assert_eq!(s.editor.text, BEFORE);
    assert!(s.review().hunks().is_empty());
    assert_eq!(s.review().unreviewed(), 0);
}

#[test]
fn reverting_is_refused_while_the_agent_runs() {
    let mut s = Script::new();
    let id = edited(&mut s, BEFORE, "Use a", AFTER);
    s.select_change(Some(0));
    s.keep_change().unwrap();
    s.select_change(Some(0));
    s.running(&id, true);

    let err = s.session.revert(0).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Wait for the agent to finish before reverting."
    );
    assert!(s.session.restore_before().is_err());

    assert_eq!(s.editor.text, AFTER);
    assert_eq!(s.review().kept(), [true, false]);
}

#[test]
fn a_revert_from_an_older_revision_is_not_applied() {
    let mut s = Script::new();
    let id = edited(&mut s, BEFORE, "Use a", AFTER);
    let other = s.draft("Other\n");

    let typed_over = s.session.revert(0).unwrap();
    let typed = AFTER.replace("Unrelated", "Other");
    s.type_text(&typed);
    s.apply(typed_over);
    assert_eq!(s.editor.text, typed);

    let left = s.session.restore_before().unwrap();
    s.select(&other);
    s.select(&id);
    s.apply(left);
    assert_eq!(s.editor.text, typed);

    let current = s.session.revert(0).unwrap();
    assert!(matches!(
        &current.effects[..],
        [Effect::ReplaceRange { .. }]
    ));
    s.apply(current);
    assert_eq!(s.editor.text, typed.replacen("a cache", "the cache", 1));
}

#[test]
fn the_unreviewed_count_follows_typing() {
    let mut s = Script::new();
    edited(&mut s, BEFORE, "Use a", AFTER);
    assert_eq!(s.review().unreviewed(), 2);

    s.type_text(&AFTER.replacen("a cache", "the cache", 1));
    assert_eq!(s.review().unreviewed(), 1);
    s.type_text(AFTER);
    assert_eq!(s.review().unreviewed(), 2);

    s.select_change(Some(0));
    s.keep_change().unwrap();
    assert_eq!(s.review().unreviewed(), 1);
    // A change of the user's own is reviewed too.
    s.type_text(&AFTER.replace("Unrelated", "Other"));
    assert_eq!(s.review().unreviewed(), 2);
}

#[test]
fn a_new_edit_clears_the_selected_change() {
    let mut s = Script::new();
    let id = edited(&mut s, BEFORE, "Use a", AFTER);
    s.select_change(Some(1));

    edit_again(&mut s, &id, "Add a line", &format!("{AFTER}\nMore.\n"));

    assert_eq!(s.review().selected(), None);
    assert_eq!(s.log().since(), 1);
}

#[test]
fn changes_are_traced_to_the_newest_edit_that_shows_them() {
    let mut s = Script::new();
    let id = edited(&mut s, BEFORE, "Use a", AFTER);
    let again = format!("{AFTER}A closing line.\n");
    edit_again(&mut s, &id, "Add a closing line", &again);
    // Typed since the newest edit, which is compared with the draft as it
    // is now.
    s.type_text(&again.replace("Unrelated text", "Unrelated words"));
    s.compare_since(0);

    let origins: Vec<(String, Option<Origin>)> = (0..s.review().hunks().len())
        .map(|ix| {
            let added = s.review().hunks()[ix].added.trim().to_owned();
            (added, s.review().origin(ix, &s.editor.text))
        })
        .collect();
    let expected = [
        ("a", Origin::Edit(0)),
        ("words", Origin::Edit(1)),
        ("a", Origin::Edit(0)),
        ("A closing line.", Origin::Edit(1)),
    ]
    .map(|(added, origin)| (added.to_owned(), Some(origin)));
    assert_eq!(origins, expected);
    assert_eq!(s.review().origin(4, &s.editor.text), None);
}

#[test]
fn a_change_of_only_whitespace_is_the_users() {
    let mut s = Script::new();
    edited(&mut s, BEFORE, "Use a", AFTER);
    s.type_text(&AFTER.replace("text in", "text  in"));

    let hunks = s.review().hunks();
    let space = hunks.iter().position(|hunk| hunk.added == "  ").unwrap();
    assert_eq!(s.review().origin(space, &s.editor.text), Some(Origin::User));
}
