//! Reviewing what the agent's edits changed in the selected draft: the
//! toolbar's Changes button, a redline in place of the preview under a bar
//! of controls, and single keys that step through, revert and keep changes.
//!
//! Each draft keeps its history of agent edits ([`History`]). Review
//! compares the draft as it is with the text before one of those edits,
//! the latest by default. Reviewing never rewrites the history: keeping a
//! change only records its key, and reverting edits the draft.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use chrono::Utc;
use draftmux_storage::{Edit, History, Store, Target};
use gpui::{
    AnimationExt, AnyElement, App, Context, Entity, Focusable, FontWeight, KeyBinding, MouseButton,
    Pixels, Point, SharedString, StyleRefinement, Task, Transformation, Window, actions, anchored,
    deferred, div, linear_color_stop, linear_gradient, percentage, point, prelude::*, px, svg,
};
use markup::Document;

use super::preview::{Preview, PreviewRedline};
use super::{Chrome, shadow};
use crate::assets::CHEVRON_ICON;
use crate::compare::{self, Comparison, Hunk, Side};
use crate::drafts::relative_time;
use crate::markup_view;
use crate::menu::{MenuItem, menu_panel};
use crate::scrollbar::{ScrollbarState, Source, scrollbar};
use crate::theme::Palette;

actions!(
    review,
    [
        ToggleReview,
        NextChange,
        PreviousChange,
        RevertChange,
        KeepChange,
        EndReview
    ]
);

const KEY_CONTEXT: &str = "Review";
/// The bar of review controls over the redline.
pub(super) const BAR_HEIGHT: Pixels = px(40.);
/// The name over each column.
pub(super) const COLUMN_HEAD_HEIGHT: Pixels = px(28.);
/// How long the pointer rests on a change before it is said where it came
/// from.
const ORIGIN_DELAY: Duration = Duration::from_millis(500);
/// How long the Changes button takes to fade in.
const BUTTON_FADE: Duration = Duration::from_millis(200);

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(editor::secondary!("shift-e"), ToggleReview, None),
        KeyBinding::new("]", NextChange, Some(KEY_CONTEXT)),
        KeyBinding::new("[", PreviousChange, Some(KEY_CONTEXT)),
        KeyBinding::new("r", RevertChange, Some(KEY_CONTEXT)),
        KeyBinding::new("k", KeepChange, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", EndReview, Some(KEY_CONTEXT)),
    ]);
}

/// The agent turn a draft's next edit belongs to: what the draft was when
/// the user asked for it, and what they asked.
pub(super) struct Turn {
    before: String,
    prompt: String,
    /// The turn's first write was recorded; later ones belong to the same
    /// edit.
    recorded: bool,
}

impl Turn {
    pub(super) fn new(before: String, prompt: &str) -> Self {
        Self {
            before,
            prompt: crate::chat::message(prompt)
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default()
                .trim()
                .to_owned(),
            recorded: false,
        }
    }
}

/// The selected draft's edit history and the part of it being compared.
#[derive(Default)]
pub(super) struct Changes {
    history: History,
    kept: HashSet<String>,
    /// The edit the comparison starts before.
    since: usize,
    compared: Option<Rc<Compared>>,
    /// Changes in the comparison that aren't kept.
    unreviewed: usize,
}

/// The draft compared with the text before an edit.
pub(super) struct Compared {
    base: String,
    text: String,
    comparison: Comparison,
}

impl Changes {
    fn load(store: &Store, id: &str) -> Self {
        let history = store.history(id).unwrap_or_else(|err| {
            eprintln!("draftmux: reading the edit history: {err:#}");
            History::default()
        });
        Self {
            kept: history.kept.iter().cloned().collect(),
            since: history.edits.len().saturating_sub(1),
            history,
            ..Self::default()
        }
    }

    /// `text` compared with the text before edit [`Self::since`], reusing
    /// the last comparison while neither changed.
    fn compare(&mut self, text: &str) -> Option<Rc<Compared>> {
        let base = &self.history.edits.get(self.since)?.before;
        if let Some(compared) = &self.compared
            && compared.base == *base
            && compared.text == text
        {
            return Some(compared.clone());
        }
        let compared = Rc::new(Compared {
            comparison: compare::compare(base, text),
            base: base.clone(),
            text: text.to_owned(),
        });
        self.compared = Some(compared.clone());
        Some(compared)
    }

    fn is_kept(&self, hunk: &Hunk) -> bool {
        self.kept.contains(&hunk.key())
    }

    fn keep(&mut self, hunk: &Hunk, kept: bool) {
        let key = hunk.key();
        if kept {
            if self.kept.insert(key.clone()) {
                self.history.kept.push(key);
            }
        } else if self.kept.remove(&key) {
            self.history.kept.retain(|other| *other != key);
        }
    }

    /// The range menu's name for the comparison, counted from the newest
    /// edit back.
    fn range_name(&self) -> String {
        let count = self.history.edits.len();
        if self.since + 1 >= count {
            "Last edit".to_owned()
        } else if self.since == 0 {
            format!("All {count} edits")
        } else {
            format!("Last {} edits", count - self.since)
        }
    }
}

/// Review, while it is open.
#[derive(Default)]
pub(super) struct Review {
    selected: Option<usize>,
    menu_open: bool,
    /// The change under the pointer.
    hovered: Option<usize>,
    /// Where the hovered change came from, once the pointer has rested on
    /// it, and where the pointer was.
    origin: Option<(SharedString, Point<Pixels>)>,
    show_origin: Option<Task<()>>,
    /// A change to scroll into view in both columns, with a serial for each
    /// request.
    reveal: Option<(usize, u64)>,
    /// Both columns' documents and what they were made from.
    documents: Option<Documents>,
}

/// The two columns: the text before the compared edits, and now.
#[derive(Clone)]
pub(super) struct Documents {
    before: Rc<Document>,
    after: Rc<Document>,
    compared: Rc<Compared>,
    target: Target,
}

/// How both columns draw the changes.
struct Marks {
    selected: Option<usize>,
    hovered: Option<usize>,
    reveal: Option<(usize, u64)>,
    kept: Vec<bool>,
}

impl Chrome {
    /// Loads the selected draft's history and leaves review.
    pub(super) fn load_changes(&mut self, cx: &mut Context<Self>) {
        self.changes = match self.drafts.current_id() {
            Some(id) => Changes::load(&self.store, id),
            None => Changes::default(),
        };
        self.review = None;
        self.sync_review(cx);
    }

    /// Records the edit an agent turn made with its first write to draft
    /// `id`, which now reads `text`.
    pub(super) fn record_edit(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(turn) = self.turns.get_mut(id) else {
            return;
        };
        // An empty draft's first draft isn't an edit to compare with.
        if turn.recorded || turn.before == text || turn.before.trim().is_empty() {
            return;
        }
        turn.recorded = true;
        let edit = Edit {
            before: turn.before.clone(),
            at: Utc::now(),
            prompt: turn.prompt.clone(),
        };
        let current = self.drafts.current_id() == Some(id);
        let mut history = if current {
            self.changes.history.clone()
        } else {
            match self.store.history(id) {
                Ok(history) => history,
                Err(err) => {
                    eprintln!("draftmux: reading the edit history: {err:#}");
                    return;
                }
            }
        };
        history.edits.push(edit);
        if let Err(err) = self.store.save_history(id, &history) {
            eprintln!("draftmux: saving the edit history: {err:#}");
        }
        if current {
            self.changes.since = history.edits.len() - 1;
            self.changes.history = history;
            if let Some(review) = &mut self.review {
                review.selected = None;
                review.menu_open = false;
            }
            self.sync_review(cx);
        }
    }

    fn save_kept(&self) {
        let Some(id) = self.drafts.current_id() else {
            return;
        };
        if let Err(err) = self.store.save_history(id, &self.changes.history) {
            eprintln!("draftmux: saving the reviewed changes: {err:#}");
        }
    }

    /// Brings the comparison, the Changes count and the redline up to date
    /// with the draft and the review.
    pub(super) fn sync_review(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).text().to_owned();
        let compared = self.changes.compare(&text);
        self.changes.unreviewed = compared.as_ref().map_or(0, |compared| {
            compared
                .comparison
                .hunks
                .iter()
                .filter(|hunk| !self.changes.is_kept(hunk))
                .count()
        });
        let target = self.drafts.current().map(|draft| draft.meta.target);
        let sides = match (&mut self.review, compared, target) {
            (Some(review), Some(compared), Some(target)) => {
                let count = compared.comparison.hunks.len();
                review.selected = review
                    .selected
                    .map(|selected| selected.min(count.saturating_sub(1)))
                    .filter(|_| count > 0);
                let documents = match &review.documents {
                    Some(documents)
                        if Rc::ptr_eq(&documents.compared, &compared)
                            && documents.target == target =>
                    {
                        documents.clone()
                    }
                    _ => {
                        let side = |side| {
                            let text = compare::marked(&compared.comparison, side);
                            Rc::new(markup_view::preview(target, &text))
                        };
                        let documents = Documents {
                            before: side(Side::Before),
                            after: side(Side::After),
                            compared: compared.clone(),
                            target,
                        };
                        review.documents = Some(documents.clone());
                        documents
                    }
                };
                let kept: Vec<bool> = compared
                    .comparison
                    .hunks
                    .iter()
                    .map(|hunk| self.changes.is_kept(hunk))
                    .collect();
                let marks = Marks {
                    selected: review.selected,
                    hovered: review.hovered,
                    reveal: review.reveal,
                    kept,
                };
                Some((documents, marks))
            }
            _ => None,
        };
        let (before, after) = match sides {
            Some((documents, marks)) => (
                Some(self.preview_redline(documents.before, &marks, cx)),
                Some(self.preview_redline(documents.after, &marks, cx)),
            ),
            None => (None, None),
        };
        for (preview, redline) in [(&self.before_preview, before), (&self.preview, after)] {
            preview.update(cx, |preview, cx| {
                if redline.is_some() {
                    preview.target = target;
                }
                if preview.redline.is_some() || redline.is_some() {
                    preview.redline = redline;
                    cx.notify();
                }
            });
        }
        cx.notify();
    }

    fn preview_redline(
        &self,
        document: Rc<Document>,
        marks: &Marks,
        cx: &mut Context<Self>,
    ) -> PreviewRedline {
        let on_click = {
            let this = cx.weak_entity();
            Rc::new(move |hunk, window: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| this.click_change(hunk, window, cx))
                    .ok();
            })
        };
        let on_hover = {
            let this = cx.weak_entity();
            Rc::new(
                move |hit: Option<(usize, Point<Pixels>)>, _: &mut Window, cx: &mut App| {
                    this.update(cx, |this, cx| this.change_hovered(hit, cx))
                        .ok();
                },
            )
        };
        PreviewRedline {
            document,
            selected: marks.selected,
            hovered: marks.hovered,
            kept: marks.kept.clone(),
            reveal: marks.reveal,
            on_click,
            on_hover,
        }
    }

    fn comparison(&self) -> Option<&Comparison> {
        self.changes
            .compared
            .as_ref()
            .map(|compared| &compared.comparison)
    }

    fn selected_hunk(&self) -> Option<&Hunk> {
        let selected = self.review.as_ref()?.selected?;
        self.comparison()?.hunks.get(selected)
    }

    pub(super) fn toggle_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_some() {
            self.close_review(window, cx);
        } else {
            self.open_review(window, cx);
        }
    }

    fn open_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.changes.history.edits.is_empty() || self.drafts.current().is_none() {
            return;
        }
        self.show_preview = true;
        self.review = Some(Review::default());
        // Review's keys are single letters, which the editor would type.
        window.focus(&self.review_focus, cx);
        self.sync_review(cx);
    }

    pub(super) fn close_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.take().is_none() {
            return;
        }
        self.editor
            .update(cx, |editor, cx| editor.set_marked(None, cx));
        if self.review_focus.contains_focused(window, cx) {
            if self.show_editor {
                window.focus(&self.editor.focus_handle(cx), cx);
            } else {
                window.focus(&self.content_focus, cx);
            }
        }
        self.sync_review(cx);
    }

    /// Selects change `selected`, or nothing, in the redline and the editor.
    fn select_change(&mut self, selected: Option<usize>, cx: &mut Context<Self>) {
        let Some(review) = &mut self.review else {
            return;
        };
        review.selected = selected;
        self.mark_selected(cx);
        self.sync_review(cx);
    }

    /// Points out the selected change's text in the editor, or where its
    /// removed text was.
    fn mark_selected(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).text();
        let range = self.selected_hunk().map(|hunk| {
            let span = &text[hunk.new.clone()];
            let start = hunk.new.start + (span.len() - span.trim_start().len());
            let end = (hunk.new.end - (span.len() - span.trim_end().len())).max(start);
            start..end
        });
        self.editor
            .update(cx, |editor, cx| editor.set_marked(range, cx));
    }

    fn click_change(&mut self, hunk: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = &self.review else {
            return;
        };
        let selected = (review.selected != Some(hunk)).then_some(hunk);
        window.focus(&self.review_focus, cx);
        self.select_change(selected, cx);
    }

    /// A press outside the changes and the review bar clears the selected
    /// change.
    pub(super) fn pressed_outside_changes(&mut self, cx: &mut Context<Self>) {
        if self
            .review
            .as_ref()
            .is_some_and(|review| review.selected.is_some())
        {
            self.select_change(None, cx);
        }
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self
            .comparison()
            .map_or(0, |comparison| comparison.hunks.len());
        let Some(review) = &self.review else {
            return;
        };
        if count == 0 {
            return;
        }
        let selected = match (review.selected, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
        };
        self.reveal_change(selected);
        self.select_change(Some(selected), cx);
    }

    /// Scrolls change `hunk` into view in both columns on the next frame.
    fn reveal_change(&mut self, hunk: usize) {
        if let Some(review) = &mut self.review {
            let serial = review.reveal.map_or(0, |(_, serial)| serial + 1);
            review.reveal = Some((hunk, serial));
        }
    }

    fn revert_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hunk) = self.selected_hunk().cloned() else {
            return;
        };
        if self.editor.read(cx).is_read_only() {
            self.show_error("Wait for the agent to finish before reverting.", cx);
            return;
        }
        // One undoable edit, as if typed.
        self.editor.update(cx, |editor, cx| {
            editor.replace_range(hunk.new.clone(), &hunk.removed, cx)
        });
        window.focus(&self.review_focus, cx);
        // The next change takes the reverted one's place, once the edit has
        // been compared.
        cx.defer_in(window, |this, _, cx| this.mark_selected(cx));
        self.show_undoable_edit(
            if hunk.is_removal() {
                "Restored the removed text"
            } else if hunk.is_addition() {
                "Removed the added text"
            } else {
                "Reverted the change"
            },
            cx,
        );
    }

    fn revert_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(base) = self
            .changes
            .history
            .edits
            .get(self.changes.since)
            .map(|edit| edit.before.clone())
        else {
            return;
        };
        if self.editor.read(cx).is_read_only() {
            self.show_error("Wait for the agent to finish before reverting.", cx);
            return;
        }
        self.editor
            .update(cx, |editor, cx| editor.replace_all(&base, cx));
        window.focus(&self.review_focus, cx);
        self.select_change(None, cx);
        self.show_undoable_edit("Reverted all changes", cx);
    }

    /// Keeps the selected change and moves on to the next one not kept, or
    /// un-keeps it.
    fn keep_change(&mut self, cx: &mut Context<Self>) {
        let Some(hunk) = self.selected_hunk().cloned() else {
            return;
        };
        let keep = !self.changes.is_kept(&hunk);
        self.changes.keep(&hunk, keep);
        self.save_kept();
        let next = keep
            .then(|| {
                let hunks = &self.comparison()?.hunks;
                let at = self.review.as_ref()?.selected?;
                (1..hunks.len())
                    .map(|step| (at + step) % hunks.len())
                    .find(|&ix| !self.changes.is_kept(&hunks[ix]))
            })
            .flatten();
        match next {
            Some(next) => {
                self.reveal_change(next);
                self.select_change(Some(next), cx);
            }
            None => self.sync_review(cx),
        }
    }

    /// Keeps every change in the comparison and closes review.
    fn finish_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hunks = self
            .comparison()
            .map(|comparison| comparison.hunks.clone())
            .unwrap_or_default();
        for hunk in &hunks {
            self.changes.keep(hunk, true);
        }
        self.save_kept();
        self.close_review(window, cx);
    }

    fn end_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.review {
            Some(review) if review.selected.is_some() => self.select_change(None, cx),
            Some(_) => self.close_review(window, cx),
            None => {}
        }
    }

    fn compare_since(&mut self, since: usize, cx: &mut Context<Self>) {
        self.changes.since = since;
        if let Some(review) = &mut self.review {
            review.menu_open = false;
        }
        self.select_change(None, cx);
    }

    fn change_hovered(&mut self, hit: Option<(usize, Point<Pixels>)>, cx: &mut Context<Self>) {
        let Some(review) = &mut self.review else {
            return;
        };
        let hovered = hit.map(|(hunk, _)| hunk);
        if review.hovered == hovered {
            return;
        }
        review.hovered = hovered;
        review.origin = None;
        review.show_origin = hit.map(|(hunk, position)| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(ORIGIN_DELAY).await;
                this.update(cx, |this, cx| {
                    let origin = this.change_origin(hunk, cx);
                    if let Some(review) = &mut this.review
                        && review.hovered == Some(hunk)
                    {
                        review.origin = origin.map(|origin| (origin.into(), position));
                        cx.notify();
                    }
                })
                .ok();
            })
        });
        // Both columns outline the change under the pointer.
        self.sync_review(cx);
    }

    /// Which agent edit made change `hunk`: the newest edit in the
    /// comparison with a change of its own in it.
    fn change_origin(&self, hunk: usize, cx: &App) -> Option<String> {
        let hunk = self.comparison()?.hunks.get(hunk)?;
        let edits = &self.changes.history.edits;
        let text = self.editor.read(cx).text();
        let made = |edit: &Hunk| {
            let (added, removed) = (edit.added.trim(), edit.removed.trim());
            (!added.is_empty() && hunk.added.contains(added))
                || (!removed.is_empty() && hunk.removed.contains(removed))
        };
        for (ix, edit) in edits.iter().enumerate().skip(self.changes.since).rev() {
            let after = edits.get(ix + 1).map_or(text, |next| next.before.as_str());
            if compare::compare(&edit.before, after).hunks.iter().any(made) {
                return Some(format!(
                    "From “{}” · {}",
                    clip(&edit.prompt, 40),
                    relative_time(edit.at, Utc::now())
                ));
            }
        }
        Some("Your edit".to_owned())
    }

    /// The toolbar's Changes button, for a draft the agent has edited, with
    /// the number of changes not yet kept.
    pub(super) fn review_button(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let edits = self.changes.history.edits.len();
        if edits == 0 {
            return None;
        }
        let count = self.changes.unreviewed;
        let open = self.review.is_some();
        let palette = *palette;
        Some(
            div()
                .id("review")
                .flex_none()
                .h(px(24.))
                .pl(px(if count > 0 { 5. } else { 9. }))
                .pr(px(9.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(6.))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .when(open, |button| button.bg(palette.selected_row))
                .hover(|style| style.bg(palette.hover))
                .active(|style| style.opacity(0.8))
                .tooltip(crate::tooltip::text(if open {
                    editor::by_platform!("Close Review (⌘⇧E)", "Close Review (Ctrl+Shift+E)")
                } else {
                    editor::by_platform!("Review Changes (⌘⇧E)", "Review Changes (Ctrl+Shift+E)")
                }))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_review(window, cx)))
                .when(count > 0, |button| {
                    button.child(
                        div()
                            .min_w(px(16.))
                            .h(px(16.))
                            .px(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.on_accent)
                            .bg(palette.accent)
                            .child(count.to_string()),
                    )
                })
                .child(if count == 1 { "Change" } else { "Changes" })
                // Fades in when the draft gets its first edit, and with
                // each edit after.
                .with_animation(
                    ("review-button", edits),
                    gpui::Animation::new(BUTTON_FADE).with_easing(gpui::ease_out_quint()),
                    |button, delta| button.opacity(delta),
                )
                .into_any_element(),
        )
    }

    /// The preview pane's contents while reviewing: the bar of controls,
    /// with the redline below it. Review's keys work while the focus is in
    /// here.
    pub(super) fn review_pane(
        &self,
        pane: gpui::Div,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        pane.track_focus(&self.review_focus)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &NextChange, _, cx| this.step(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousChange, _, cx| this.step(false, cx)))
            .on_action(
                cx.listener(|this, _: &RevertChange, window, cx| this.revert_change(window, cx)),
            )
            .on_action(cx.listener(|this, _: &KeepChange, _, cx| this.keep_change(cx)))
            .on_action(cx.listener(|this, _: &EndReview, window, cx| this.end_review(window, cx)))
            .child(self.review_bar(palette, cx))
    }

    /// A column's name over its preview: the text before the compared
    /// edits, or now.
    pub(super) fn column_head(&self, before: bool, palette: &Palette) -> impl IntoElement {
        let name = if !before {
            "Now".to_owned()
        } else if self.changes.since + 1 >= self.changes.history.edits.len() {
            let prompt = self
                .changes
                .history
                .edits
                .get(self.changes.since)
                .map_or("", |edit| edit.prompt.as_str());
            format!("Before “{}”", clip(prompt, 30))
        } else {
            format!("Before {}", self.changes.range_name().to_lowercase())
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(COLUMN_HEAD_HEIGHT)
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(16.))
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(palette.muted)
            .bg(palette.toolbar)
            .border_b_1()
            .border_color(palette.hairline)
            .child(div().flex_none().size(px(7.)).rounded_full().bg(if before {
                palette.removed
            } else {
                palette.added
            }))
            .child(div().min_w_0().truncate().child(name))
    }

    /// Review's view, in place of the editor and preview: the bar, over the
    /// text before the compared edits and now, side by side.
    pub(super) fn review_columns(&self, palette: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let column = |preview: &Entity<Preview>, bar: &ScrollbarState, before: bool| {
            let scroll = preview.read(cx).scroll.clone();
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .when(!before, |column| {
                    column.border_l_1().border_color(palette.hairline)
                })
                .child(self.column_head(before, palette))
                .child(
                    div()
                        .absolute()
                        .top(COLUMN_HEAD_HEIGHT)
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(
                            preview.clone().cached(
                                StyleRefinement::default()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full(),
                            ),
                        )
                        .child(scrollbar(Source::Handle(scroll), bar)),
                )
        };
        let columns = div()
            .absolute()
            .top(BAR_HEIGHT)
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .child(column(&self.before_preview, &self.before_scrollbar, true))
            .child(column(&self.preview, &self.scrollbars[2], false));
        self.review_pane(
            super::pane(palette.pane, palette)
                .flex_1()
                .relative()
                .child(columns),
            palette,
            cx,
        )
    }

    fn review_bar(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *palette;
        let review = self.review.as_ref();
        let count = self
            .comparison()
            .map_or(0, |comparison| comparison.hunks.len());
        let selected = review.and_then(|review| review.selected);
        let kept = self
            .selected_hunk()
            .is_some_and(|hunk| self.changes.is_kept(hunk));
        let menu_open = review.is_some_and(|review| review.menu_open);
        let range = div()
            .id("review-range")
            .flex_none()
            .h(px(24.))
            .pl(px(8.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded(px(6.))
            .font_weight(FontWeight::MEDIUM)
            .when(menu_open, |button| button.bg(palette.hover))
            .hover(|style| style.bg(palette.hover))
            .active(|style| style.opacity(0.8))
            .tooltip(crate::tooltip::text("Compare with an Earlier Version"))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(review) = &mut this.review {
                    review.menu_open = !review.menu_open;
                    cx.notify();
                }
            }))
            .child(self.changes.range_name())
            .child(
                svg()
                    .path(CHEVRON_ICON)
                    .size(px(9.))
                    .text_color(palette.muted)
                    .with_transformation(Transformation::rotate(percentage(0.25))),
            );
        let range = div().relative().child(range).when(menu_open, |range| {
            range.child(
                div().absolute().top_full().left_0().mt_1().child(deferred(
                    anchored()
                        .snap_to_window_with_margin(px(8.))
                        .child(self.range_menu(&palette, cx)),
                )),
            )
        });
        let changes = (count > 0).then(|| {
            let position: SharedString = match selected {
                Some(at) => format!("{} of {count}", at + 1).into(),
                None => format!("– of {count}").into(),
            };
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .pl(px(4.))
                        .mr(px(2.))
                        .border_l_1()
                        .border_color(palette.hairline)
                        .child(step_button(
                            "review-previous",
                            false,
                            editor::by_platform!("Previous Change ([)", "Previous Change ([)"),
                            &palette,
                            cx.listener(|this, _, _, cx| this.step(false, cx)),
                        ))
                        .child(
                            div()
                                .id("review-position")
                                .px(px(2.))
                                .text_color(palette.muted)
                                .whitespace_nowrap()
                                .when(selected.is_none(), |position| {
                                    position.tooltip(crate::tooltip::text(format!(
                                        "{count} change{}. Click one or press ] to select it.",
                                        if count == 1 { "" } else { "s" }
                                    )))
                                })
                                .child(position),
                        )
                        .child(step_button(
                            "review-next",
                            true,
                            "Next Change (])",
                            &palette,
                            cx.listener(|this, _, _, cx| this.step(true, cx)),
                        )),
                )
                .child(bar_button("review-revert", "Revert", &palette).when_else(
                    selected.is_some(),
                    |button| {
                        button
                            .tooltip(crate::tooltip::text("Revert This Change (R)"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.revert_change(window, cx)),
                            )
                    },
                    |button| button.opacity(0.45),
                ))
                .child(
                    bar_button("review-keep", if kept { "Kept" } else { "Keep" }, &palette)
                        .when(kept, |button| {
                            button
                                .bg(gpui::transparent_black())
                                .shadow(Vec::new())
                                .text_color(palette.muted)
                                .hover(|style| style.bg(palette.hover).text_color(palette.text))
                        })
                        .when_else(
                            selected.is_some(),
                            |button| {
                                button
                                    .tooltip(crate::tooltip::text(if kept {
                                        "Mark as Not Reviewed (K)"
                                    } else {
                                        "Keep This Change (K)"
                                    }))
                                    .on_click(cx.listener(|this, _, _, cx| this.keep_change(cx)))
                            },
                            |button| button.opacity(0.45),
                        ),
                )
        });
        div()
            .id("review-bar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(BAR_HEIGHT)
            .flex()
            .items_center()
            .gap(px(4.))
            .pl(px(4.))
            .pr(px(8.))
            .bg(palette.toolbar)
            .border_b_1()
            .border_color(palette.hairline)
            .text_size(px(12.))
            // Clicks on the controls never clear the selected change.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(range)
            .children(changes)
            .when(count == 0, |bar| {
                bar.child(
                    div()
                        .pl(px(10.))
                        .border_l_1()
                        .border_color(palette.hairline)
                        .text_color(palette.muted)
                        .child("No changes"),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("review-done")
                    .flex_none()
                    .h(px(24.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.on_accent)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(palette.primary_top, 0.),
                        linear_color_stop(palette.primary_bottom, 1.),
                    ))
                    .hover(|style| {
                        style.bg(linear_gradient(
                            180.,
                            linear_color_stop(palette.primary_hover_top, 0.),
                            linear_color_stop(palette.primary_hover_bottom, 1.),
                        ))
                    })
                    .shadow(vec![
                        shadow(palette.primary_ring, 0., 0., 0.5),
                        shadow(palette.primary_drop, 1., 1.5, 0.),
                    ])
                    .active(|style| style.opacity(0.8))
                    .tooltip(crate::tooltip::text("Mark All as Reviewed and Close"))
                    .on_click(cx.listener(|this, _, window, cx| this.finish_review(window, cx)))
                    .child("Done"),
            )
    }

    /// "Show Changes Since" each agent edit, newest first, and Revert All.
    fn range_menu(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let now = Utc::now();
        let edits = &self.changes.history.edits;
        let mut items = vec![MenuItem {
            label: "Show Changes Since".into(),
            checked: false,
            enabled: false,
            action: None,
            detail: None,
            heading: true,
        }];
        items.extend(edits.iter().enumerate().rev().map(|(ix, edit)| MenuItem {
            label: format!("“{}”", clip(&edit.prompt, 34)).into(),
            checked: ix == self.changes.since,
            enabled: true,
            action: Some(Some(ix)),
            detail: Some(relative_time(edit.at, now).into()),
            heading: false,
        }));
        items.push(MenuItem {
            label: SharedString::default(),
            checked: false,
            enabled: false,
            action: None,
            detail: None,
            heading: false,
        });
        items.push(MenuItem {
            label: "Revert All Changes".into(),
            checked: false,
            enabled: self
                .comparison()
                .is_some_and(|comparison| !comparison.hunks.is_empty()),
            action: Some(None),
            detail: None,
            heading: false,
        });
        let pick = cx.entity().downgrade();
        let dismiss = cx.entity().downgrade();
        menu_panel(
            palette,
            &items,
            move |action: Option<usize>, window, cx| {
                pick.update(cx, |this, cx| match action {
                    Some(since) => this.compare_since(since, cx),
                    None => {
                        if let Some(review) = &mut this.review {
                            review.menu_open = false;
                        }
                        this.revert_all(window, cx);
                    }
                })
                .ok();
            },
            move |_, cx| {
                dismiss
                    .update(cx, |this, cx| {
                        if let Some(review) = &mut this.review {
                            review.menu_open = false;
                        }
                        cx.notify();
                    })
                    .ok();
            },
        )
    }

    /// Where the change under the pointer came from, beside the pointer.
    pub(super) fn change_origin_tooltip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (text, position) = self.review.as_ref()?.origin.clone()?;
        Some(deferred(
            anchored()
                .position(position + point(px(8.), px(16.)))
                .snap_to_window_with_margin(px(8.))
                .child(crate::tooltip::card(text, cx)),
        ))
    }
}

/// A review bar button with a hairline ring, like a push button.
fn bar_button(
    id: &'static str,
    label: &'static str,
    palette: &Palette,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(24.))
        .px(px(7.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .font_weight(FontWeight::MEDIUM)
        .bg(palette.control)
        .shadow(vec![
            shadow(palette.line_strong, 0., 0., 0.5),
            shadow(palette.card_drop, 1., 1., 0.),
        ])
        .hover(|style| style.bg(palette.control_hover))
        .active(|style| style.opacity(0.8))
        .child(label)
}

fn step_button(
    id: &'static str,
    forward: bool,
    tooltip: &'static str,
    palette: &Palette,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let palette = *palette;
    div()
        .id(id)
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .group(id)
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(0.8))
        .tooltip(crate::tooltip::text(tooltip))
        .on_click(on_click)
        .child(
            svg()
                .path(CHEVRON_ICON)
                .size(px(10.))
                .text_color(palette.muted)
                .group_hover(id, |style| style.text_color(palette.text))
                .when(!forward, |icon| {
                    icon.with_transformation(Transformation::rotate(percentage(0.5)))
                }),
        )
}

/// `text` on one line, cut to `max` characters with an ellipsis.
fn clip(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let mut clipped: String = text.chars().take(max - 1).collect();
    clipped.push('…');
    clipped
}

/// Turns of every draft whose agent may write next.
pub(super) type Turns = HashMap<String, Turn>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_long_prompts_to_one_line() {
        assert_eq!(clip("make it\n  shorter", 40), "make it shorter");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn turn_prompts_are_the_users_first_line() {
        let turn = Turn::new("before".into(), "\nTighten the summary\nand more");
        assert_eq!(turn.prompt, "Tighten the summary");
    }

    #[test]
    fn range_names_count_back_from_the_newest_edit() {
        let edit = || Edit {
            before: String::new(),
            at: Utc::now(),
            prompt: String::new(),
        };
        let mut changes = Changes {
            history: History {
                edits: vec![edit(), edit(), edit()],
                kept: Vec::new(),
            },
            ..Changes::default()
        };
        changes.since = 2;
        assert_eq!(changes.range_name(), "Last edit");
        changes.since = 1;
        assert_eq!(changes.range_name(), "Last 2 edits");
        changes.since = 0;
        assert_eq!(changes.range_name(), "All 3 edits");
    }

    #[test]
    fn keeping_records_the_change_once_and_unkeeping_forgets_it() {
        let mut changes = Changes::default();
        let hunk = compare::compare("a\n", "b\n").hunks[0].clone();
        changes.keep(&hunk, true);
        changes.keep(&hunk, true);
        assert!(changes.is_kept(&hunk));
        assert_eq!(changes.history.kept.len(), 1);
        changes.keep(&hunk, false);
        assert!(!changes.is_kept(&hunk));
        assert!(changes.history.kept.is_empty());
    }
}
