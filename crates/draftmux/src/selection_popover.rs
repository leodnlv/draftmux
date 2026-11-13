//! The selection actions popover: actions for the editor selection, then the
//! options pi returns, in a blurred popup window above the selection. A selection made in the preview selects its
//! markdown in the editor, and the popover then sits above the preview's
//! selection instead.
//!
//! Comment swaps the items for a field: what's written there goes to the
//! chat's agent with the selections quoted, as if sent from the chat, and
//! the popover closes. With several selections (see
//! [`Editor::extra_selections`]) the popover shows their count and only
//! Comment: the selection actions are prompts written for one selection, so
//! they don't apply to several.
//!
//! The popover is a separate panel window. On Wayland, where windows can't
//! be placed in screen coordinates, it's a popup anchored to the main window
//! at its edge facing the selection. gpui can't anchor popups on macOS or
//! X11, so there it's a `WindowKind::PopUp` panel placed in screen
//! coordinates. It holds no state of its own: [`SelectionPopover`] owns the
//! state, and the panel is resized, reopened or closed to match it after
//! each frame of the main window. gpui can't move a window, but a resize
//! keeps a `PopUp`'s bottom edge and an anchored popup's anchored edge, so
//! the popover grows in place while that edge stays put.
//!
//! The main window keeps keyboard focus, so the editor keeps its selection
//! highlight; the popover's keys are bound in key contexts around the editor
//! (see [`SelectionPopover::wrap`]). A click makes the panel key only until
//! the mouse button is released. The comment field is the exception: the
//! panel stays key while it's shown, and the main window keeps the editor
//! focused behind it.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use draftmux_storage::{Draft, Store};
use editor::{Editor, EditorEvent, EditorStyle};
use gpui::{
    Action, Animation, AnimationExt, AnyElement, AnyWindowHandle, App, Bounds, Context, Div,
    Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, Pixels, SharedString, Size, Stateful,
    Subscription, Task, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, actions,
    canvas, div, ease_out_quint, font, point,
    popup::{
        PopupAnchor, PopupConstraintAdjustment, PopupGravity, PopupNotSupportedError, PopupOptions,
    },
    prelude::*,
    px,
};
use shell_env::ShellEnv;

use crate::app_settings::AppSettings;
use crate::assets::{COMMENT_ICON, SEND_ICON};
use crate::resources;
use crate::selectable::TextSelection;
use crate::selection::{self, Action as SelectionAction, Request, SelectionConfig};
use crate::theme::{self, Palette};

actions!(selection_popover, [Dismiss, Confirm, Next, Previous, Back]);

/// Applies the option with this index.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = selection_popover, no_json)]
pub struct Pick(pub usize);

/// Key contexts around the editor while the popover is shown.
const POPOVER_CONTEXT: &str = "SelectionPopover";
const RESULTS_CONTEXT: &str = "SelectionResults";
/// Around the comment field, in the panel.
const COMMENT_CONTEXT: &str = "SelectionComment";

/// How long the selection must stay put before the popover appears, so
/// shift-arrow selections don't flash it on every step.
const SHOW_DELAY: Duration = Duration::from_millis(200);
/// Between the selection and the popover.
const GAP: Pixels = px(6.);
/// Between the popover and the edges of the editor pane.
const MARGIN: Pixels = px(8.);
const PADDING: Pixels = px(5.);
/// Highlighted items follow the panel's rounded corners: concentric with
/// them, inset by the padding.
const ITEM_RADIUS: Pixels = px(crate::platform::POPUP_RADIUS - 5.);
const RESULTS_WIDTH: Pixels = px(380.);
const COMMENT_WIDTH: Pixels = px(340.);
/// How long a newly highlighted item takes to fade in.
const HIGHLIGHT_FADE: Duration = Duration::from_millis(120);
/// How long the running, results and error states take to fade in.
const MODE_FADE: Duration = Duration::from_millis(160);

pub fn init(cx: &mut App) {
    let mut bindings = Vec::new();
    // The editor, or the preview when the selection was made there.
    for focused in ["Editor", crate::selectable::KEY_CONTEXT] {
        let popover = Some(format!("{POPOVER_CONTEXT} > {focused}"));
        let results = Some(format!("{RESULTS_CONTEXT} > {focused}"));
        bindings.extend([
            KeyBinding::new("escape", Dismiss, popover.as_deref()),
            KeyBinding::new("up", Previous, popover.as_deref()),
            KeyBinding::new("down", Next, popover.as_deref()),
            KeyBinding::new("enter", Confirm, popover.as_deref()),
            KeyBinding::new("up", Previous, results.as_deref()),
            KeyBinding::new("down", Next, results.as_deref()),
            KeyBinding::new("enter", Confirm, results.as_deref()),
        ]);
        for (index, key) in ["1", "2", "3", "4", "5", "6", "7", "8", "9"]
            .into_iter()
            .enumerate()
        {
            bindings.push(KeyBinding::new(key, Pick(index), results.as_deref()));
        }
    }
    // Enter sends the comment; shift-enter still inserts a newline.
    let comment = Some(format!("{COMMENT_CONTEXT} > Editor"));
    bindings.extend([
        KeyBinding::new("enter", Confirm, comment.as_deref()),
        KeyBinding::new("escape", Back, comment.as_deref()),
    ]);
    // Bound after the editor's keys, so these win at the same depth.
    cx.bind_keys(bindings);
}

pub enum SelectionPopoverEvent {
    /// The text the user wrote in the comment field about the editor's
    /// selections: send it to the chat's agent with them quoted.
    Comment(String),
}

impl EventEmitter<SelectionPopoverEvent> for SelectionPopover {}

/// The popover for one editor's selection.
pub struct SelectionPopover {
    editor: Entity<Editor>,
    main: AnyWindowHandle,
    draft: Option<Draft>,
    open: Option<Open>,
    panel: Option<Panel>,
    /// The popover content's size, measured by the panel after layout.
    content_size: Option<Size<Pixels>>,
    /// The editor pane in window coordinates, measured by [`Self::wrap`].
    pane: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The preview's selection, which the editor's mirrors while it has one.
    preview: Entity<TextSelection>,
    /// The preview pane in window coordinates, measured by
    /// [`Self::wrap_preview`].
    preview_pane: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Remembers whether the user has added a selection, after which the
    /// hint on how to is hidden.
    store: Store,
    show_add_hint: bool,
    pending_show: Option<Task<()>>,
    /// Follows the comment field's text while it's shown.
    comment_observer: Option<Subscription>,
    reconcile_scheduled: bool,
    /// The panel is being opened; it is set once it has.
    opening: bool,
    main_bounds: Option<Bounds<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

/// The popover is shown for `range` of `text`.
struct Open {
    range: Range<usize>,
    text: String,
    mode: Mode,
    /// The one highlighted item; the mouse and the arrow keys both move it.
    highlighted: Option<usize>,
}

enum Mode {
    Actions(Vec<draftmux_settings::SelectionAction>),
    /// This many ranges are selected; the only item is Comment.
    Several(usize),
    /// A message to pi about the selections, being written.
    Comment(Entity<Editor>),
    /// Dropping it cancels the call and kills pi.
    Running {
        /// What is being done, shown beside the spinner.
        label: SharedString,
        /// Complete replacements that are not yet safe to apply.
        previews: Vec<String>,
        _cancel: CancelOnDrop,
        _task: Task<()>,
    },
    Results(Vec<String>),
    Error(SharedString),
}

impl Mode {
    /// The index of the Comment item among the items.
    fn comment_item(&self) -> Option<usize> {
        match self {
            Self::Actions(actions) => Some(actions.len() + 1),
            Self::Several(_) => Some(0),
            _ => None,
        }
    }

    fn is_request(&self, request: &Arc<AtomicBool>) -> bool {
        matches!(self, Self::Running { _cancel, .. } if Arc::ptr_eq(&_cancel.0, request))
    }

    fn set_previews(&mut self, request: &Arc<AtomicBool>, options: Vec<String>) -> bool {
        if !self.is_request(request) {
            return false;
        }
        let Self::Running { previews, .. } = self else {
            return false;
        };
        if *previews == options {
            return false;
        }
        *previews = options;
        true
    }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

struct Panel {
    handle: WindowHandle<PopoverPanel>,
    bounds: Bounds<Pixels>,
    /// The edge a resize keeps in place.
    pinned: Edge,
    /// Dismisses the popover when the panel, key after a click on it, loses
    /// focus to another app.
    _activation: Subscription,
}

/// A horizontal edge of the panel.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Edge {
    Top,
    Bottom,
}

impl Edge {
    /// The left corner of `bounds` on this edge.
    fn corner(self, bounds: Bounds<Pixels>) -> gpui::Point<Pixels> {
        match self {
            Edge::Top => bounds.origin,
            Edge::Bottom => bounds.bottom_left(),
        }
    }
}

impl SelectionPopover {
    pub fn new(
        editor: Entity<Editor>,
        preview: Entity<TextSelection>,
        store: Store,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor_focus = editor.read(cx).focus_handle(cx);
        let preview_focus = preview.read(cx).focus_handle(cx);
        // Focus moving out of both panes, to the chat's composer for one,
        // dismisses the popover; the selection itself stays.
        let focus_out = |handle: &FocusHandle, window: &mut Window, cx: &mut Context<Self>| {
            let panes = [editor_focus.clone(), preview_focus.clone()];
            cx.on_focus_out(handle, window, move |this, _, window, cx| {
                if !panes.iter().any(|pane| pane.contains_focused(window, cx)) {
                    this.dismiss(cx);
                }
            })
        };
        let subscriptions = vec![
            focus_out(&editor_focus, window, cx),
            focus_out(&preview_focus, window, cx),
            cx.subscribe(&editor, |this, _, event, cx| this.editor_event(event, cx)),
            cx.observe(&editor, |this, _, cx| this.schedule_reconcile(cx)),
            cx.observe(&AppSettings::global(cx), |this, settings, cx| {
                let actions = settings.read(cx).settings.selection_actions();
                if let Some(Open {
                    mode: Mode::Actions(current),
                    highlighted,
                    ..
                }) = &mut this.open
                    && *current != actions
                {
                    *current = actions;
                    *highlighted = None;
                    this.changed(cx);
                }
            }),
            // gpui can't move the panel along with the window. Activation
            // changes report bounds too, unchanged.
            cx.observe_window_bounds(window, move |this, window, cx| {
                let bounds = window.bounds();
                if this.main_bounds.replace(bounds) != Some(bounds) {
                    this.dismiss(cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.dismiss_unless_panel_active(cx);
                }
            }),
        ];
        Self {
            editor,
            main: window.window_handle(),
            draft: None,
            open: None,
            panel: None,
            content_size: None,
            pane: Rc::new(Cell::new(None)),
            preview,
            preview_pane: Rc::new(Cell::new(None)),
            show_add_hint: !store.selection_added(),
            store,
            pending_show: None,
            comment_observer: None,
            reconcile_scheduled: false,
            opening: false,
            main_bounds: Some(window.bounds()),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_draft(&mut self, draft: Option<Draft>, cx: &mut Context<Self>) {
        self.draft = draft;
        self.dismiss(cx);
    }

    /// Wraps the editor pane: adds the popover's key contexts and actions,
    /// and measures the pane for placing the popover.
    pub fn wrap(this: &Entity<Self>, editor: impl IntoElement, cx: &App) -> AnyElement {
        let pane = this.read(cx).pane.clone();
        Self::wrap_pane(this, editor, pane, false, cx)
    }

    /// Wraps the preview pane like [`Self::wrap`] wraps the editor's. The
    /// preview scrolls without the editor knowing, so while the popover is
    /// over the preview it is placed again after every frame.
    pub fn wrap_preview(this: &Entity<Self>, preview: impl IntoElement, cx: &App) -> AnyElement {
        let pane = this.read(cx).preview_pane.clone();
        Self::wrap_pane(this, preview, pane, true, cx)
    }

    fn wrap_pane(
        this: &Entity<Self>,
        child: impl IntoElement,
        pane: Rc<Cell<Option<Bounds<Pixels>>>>,
        preview: bool,
        cx: &App,
    ) -> AnyElement {
        let popover = this.read(cx);
        let context = match &popover.open {
            Some(Open {
                mode: Mode::Results(_),
                ..
            }) => Some(format!("{POPOVER_CONTEXT} {RESULTS_CONTEXT}")),
            Some(_) => Some(POPOVER_CONTEXT.to_string()),
            None => None,
        };
        let entity = this.clone();
        div()
            .size_full()
            .relative()
            .map(|div| match &context {
                Some(context) => div.key_context(context.as_str()),
                None => div,
            })
            .on_action({
                let this = this.clone();
                move |_: &Dismiss, _, cx| this.update(cx, |this, cx| this.dismiss(cx))
            })
            .on_action({
                let this = this.clone();
                move |_: &Next, _, cx| this.update(cx, |this, cx| this.move_highlight(1, cx))
            })
            .on_action({
                let this = this.clone();
                move |_: &Previous, _, cx| this.update(cx, |this, cx| this.move_highlight(-1, cx))
            })
            .on_action({
                let this = this.clone();
                move |_: &Confirm, _, cx| this.update(cx, |this, cx| this.confirm(cx))
            })
            .on_action({
                let this = this.clone();
                move |pick: &Pick, _, cx| this.update(cx, |this, cx| this.apply(pick.0, cx))
            })
            .child(child)
            // Before the panel opens, a hidden copy of its content in the
            // pane measures it, so the panel opens at its final size.
            .when(
                popover.open.is_some()
                    && popover.panel.is_none()
                    && popover.content_size.is_none()
                    && popover.in_preview(cx) == preview,
                |pane| {
                    // Style the measured element itself: an auto-sized wrapper
                    // with only an absolute child has no intrinsic width.
                    let mut copy = measured_content(this.clone(), popover.content(this, cx))
                        .text_size(px(13.))
                        .font_family(editor::platform::ui_font());
                    copy.style().visibility = Some(gpui::Visibility::Hidden);
                    pane.child(copy)
                },
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        pane.set(Some(bounds));
                        let popover = entity.read(cx);
                        let follow = preview && popover.open.is_some() && popover.in_preview(cx);
                        if popover.reconcile_scheduled || follow {
                            let this = entity.clone();
                            window.defer(cx, move |window, cx| {
                                this.update(cx, |this, cx| this.reconcile(window, cx))
                            });
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element()
    }

    /// Whether the selection was made in the preview, so the popover goes
    /// there.
    fn in_preview(&self, cx: &App) -> bool {
        self.preview.read(cx).range().is_some()
    }

    /// Whether a draft is focused that selection calls can fork.
    fn available(&self, cx: &App) -> bool {
        !self.editor.read(cx).is_read_only()
            && self
                .draft
                .as_ref()
                .is_some_and(|draft| draft.meta.pi_session_file.is_some())
    }

    fn editor_event(&mut self, event: &EditorEvent, cx: &mut Context<Self>) {
        match event {
            EditorEvent::Edited => self.dismiss(cx),
            EditorEvent::SelectionChanged(range) => {
                if self.open.as_ref().is_some_and(|open| open.range != *range) {
                    self.dismiss(cx);
                }
                self.pending_show = None;
                if !range.is_empty() && self.open.is_none() {
                    self.schedule_show(range.clone(), cx);
                }
            }
            // Adding or dropping a selection changes what the popover offers.
            EditorEvent::ExtraSelectionsChanged => {
                if self.show_add_hint && !self.editor.read(cx).extra_selections().is_empty() {
                    self.show_add_hint = false;
                    if let Err(err) = self.store.mark_selection_added() {
                        eprintln!("draftmux: remembering the added selection: {err:#}");
                    }
                }
                self.dismiss(cx);
                let range = self.editor.read(cx).selected_range();
                if !range.is_empty() {
                    self.schedule_show(range, cx);
                }
            }
        }
    }

    /// Shows the actions once the selection has settled, the mouse button
    /// is up and the editor's context menu is closed.
    fn schedule_show(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.pending_show = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SHOW_DELAY).await;
                let Ok(selecting) = this.update(cx, |this, cx| {
                    let editor = this.editor.read(cx);
                    editor.is_selecting()
                        || editor.is_context_menu_open()
                        || this.preview.read(cx).is_dragging()
                }) else {
                    return;
                };
                if !selecting {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                this.pending_show = None;
                let editor = this.editor.read(cx);
                if this.open.is_some() || editor.selected_range() != range || !this.available(cx) {
                    return;
                }
                let mode = this.items_mode(cx);
                this.open = Some(Open {
                    range,
                    text: editor.text().to_string(),
                    mode,
                    highlighted: None,
                });
                this.changed(cx);
            })
            .ok();
        }));
    }

    /// The items for the editor's selections: the actions for one, Comment
    /// for several.
    fn items_mode(&self, cx: &App) -> Mode {
        match self.editor.read(cx).selected_ranges().len() {
            1 => Mode::Actions(
                AppSettings::global(cx)
                    .read(cx)
                    .settings
                    .selection_actions(),
            ),
            count => Mode::Several(count),
        }
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.pending_show = None;
        self.comment_observer = None;
        if self.open.take().is_some() {
            // Closed now rather than after the main window's next draw: a
            // window covered by another app may not draw until it's shown
            // again, which would leave the empty panel on screen meanwhile.
            self.close_panel(cx);
            self.changed(cx);
        }
    }

    fn close_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.take() {
            // Deferred, since the panel's own events close it, and a window
            // can't be updated while it handles one.
            cx.defer(move |cx| {
                panel
                    .handle
                    .update(cx, |_, window, _| window.remove_window())
                    .ok();
            });
        }
    }

    /// A window lost focus: hide the popover if the focus left the app.
    /// Clicking the panel moves focus from the main window to the panel and
    /// back without the app losing it; checking either window's focus while
    /// that settles would catch both inactive and hide the popover under
    /// the click.
    fn dismiss_unless_panel_active(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            this.update(cx, |this, cx| {
                if !crate::platform::app_active(cx) {
                    this.dismiss(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The state changed: redraw and place the panel to match.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.content_size = None;
        self.redraw(cx);
        self.schedule_reconcile(cx);
    }

    fn set_mode(&mut self, mode: Mode, highlighted: Option<usize>, cx: &mut Context<Self>) {
        self.comment_observer = None;
        if let Some(open) = &mut self.open {
            open.mode = mode;
            open.highlighted = highlighted;
            self.changed(cx);
        }
    }

    fn item_count(&self) -> usize {
        match self.open.as_ref().map(|open| &open.mode) {
            Some(Mode::Actions(actions)) => actions.len() + 2,
            Some(Mode::Several(_)) => 1,
            Some(Mode::Results(options)) => options.len(),
            _ => 0,
        }
    }

    fn set_highlight(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.open
            && open.highlighted != Some(index)
        {
            open.highlighted = Some(index);
            self.redraw(cx);
        }
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.item_count() as isize;
        if let Some(open) = &mut self.open
            && count > 0
        {
            let current = open
                .highlighted
                .map_or(if delta > 0 { -1 } else { 0 }, |i| i as isize);
            open.highlighted = Some((current + delta).rem_euclid(count) as usize);
            self.redraw(cx);
        }
    }

    /// Repaints the panel without changing its size.
    fn redraw(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(panel) = &self.panel {
            panel
                .handle
                .update(cx, |_, window, _| window.refresh())
                .ok();
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(open) = &self.open else { return };
        match &open.mode {
            Mode::Results(_) => {
                if let Some(index) = open.highlighted {
                    self.apply(index, cx);
                }
            }
            Mode::Actions(_) => {
                if let Some(index) = open.highlighted {
                    self.pick_action(index, cx);
                }
            }
            Mode::Several(_) if open.highlighted.is_some() => self.start_comment(cx),
            Mode::Comment(_) => self.send_comment(cx),
            _ => {}
        }
    }

    fn pick_action(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(Open {
            mode: Mode::Actions(actions),
            ..
        }) = &self.open
        else {
            return;
        };
        if index == actions.len() + 1 {
            self.start_comment(cx);
        } else if index == 0 {
            self.run(
                SelectionAction::Alternatives,
                "Generating alternatives…".into(),
                cx,
            );
        } else if let Some(action) = actions.get(index - 1) {
            let label = format!("{}…", action.name).into();
            self.run(
                SelectionAction::Instruction(action.prompt.clone()),
                label,
                cx,
            );
        }
    }

    /// Swaps the items for the comment field and gives it the keyboard.
    fn start_comment(&mut self, cx: &mut Context<Self>) {
        let field = theme::editor(comment_style, cx);
        // Typing grows the field, and the panel with it.
        let observer = cx.subscribe(&field, |this, _, event, cx| {
            if matches!(event, EditorEvent::Edited) {
                this.changed(cx);
            }
        });
        self.set_mode(Mode::Comment(field), None, cx);
        self.comment_observer = Some(observer);
        self.focus_comment(cx);
    }

    fn commenting(&self) -> bool {
        matches!(
            self.open,
            Some(Open {
                mode: Mode::Comment(_),
                ..
            })
        )
    }

    /// Makes the panel key with the comment field focused, if it's shown.
    /// The main window keeps the editor focused, so the selection stays
    /// painted.
    fn focus_comment(&self, cx: &mut Context<Self>) {
        let (
            Some(panel),
            Some(Open {
                mode: Mode::Comment(field),
                ..
            }),
        ) = (&self.panel, &self.open)
        else {
            return;
        };
        let (handle, focus) = (panel.handle, field.focus_handle(cx));
        cx.defer(move |cx| {
            handle
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    window.focus(&focus, cx);
                })
                .ok();
        });
    }

    /// Leaves the comment field for the items it replaced, with Comment
    /// highlighted.
    fn back(&mut self, cx: &mut Context<Self>) {
        if !self.commenting() {
            return;
        }
        let mode = self.items_mode(cx);
        let highlighted = mode.comment_item();
        self.set_mode(mode, highlighted, cx);
        self.focus_main(cx);
    }

    /// Hands the comment to the chat and closes the popover.
    fn send_comment(&mut self, cx: &mut Context<Self>) {
        let Some(Open {
            mode: Mode::Comment(field),
            ..
        }) = &self.open
        else {
            return;
        };
        let text = field.read(cx).text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.dismiss(cx);
        self.focus_main(cx);
        cx.emit(SelectionPopoverEvent::Comment(text));
    }

    fn focus_main(&self, cx: &mut Context<Self>) {
        let main = self.main;
        cx.defer(move |cx| {
            main.update(cx, |_, window, _| window.activate_window())
                .ok();
        });
    }

    fn run(&mut self, action: SelectionAction, label: SharedString, cx: &mut Context<Self>) {
        let Some(open) = &self.open else { return };
        let Some(meta) = self.draft.as_ref().map(|draft| &draft.meta) else {
            return;
        };
        let kept = meta.settings.clone();
        let Some(request) = Request::new(meta, open.text.clone(), open.range.clone(), action)
        else {
            return;
        };
        let pi = match ShellEnv::global(cx) {
            None => Err("Reading the login shell environment…".to_string()),
            Some(env) => env
                .pi()
                .map(|pi| env.command(pi))
                .map_err(|err| err.to_string()),
        };
        let extension = resources::pi_extension().map_err(|err| format!("{err:#}"));
        let (pi, extension) = match (pi, extension) {
            (Ok(pi), Ok(extension)) => (pi, extension),
            (Err(err), _) | (_, Err(err)) => {
                return self.set_mode(Mode::Error(err.into()), None, cx);
            }
        };

        let cancel = Arc::new(AtomicBool::new(false));
        // The count is an app preference, read on every call; the rest is
        // what the draft kept at creation.
        let alternatives = AppSettings::global(cx).read(cx).settings.alternatives();
        let config = SelectionConfig::for_draft(&kept, alternatives);
        let (send, updates) = async_channel::unbounded();
        let call = cx.background_executor().spawn({
            let cancel = cancel.clone();
            async move {
                selection::run(
                    pi,
                    &extension,
                    &request,
                    &config,
                    &std::env::temp_dir(),
                    &cancel,
                    |options| {
                        let _ = send.try_send(options);
                    },
                )
            }
        });
        let request = cancel.clone();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(options) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if let Some(Open { mode, .. }) = &mut this.open
                            && mode.set_previews(&request, options)
                        {
                            this.changed(cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = call.await;
            this.update(cx, |this, cx| {
                if !this
                    .open
                    .as_ref()
                    .is_some_and(|open| open.mode.is_request(&request))
                {
                    return;
                }
                match result {
                    Ok(options) => this.set_mode(Mode::Results(options), Some(0), cx),
                    Err(err) => this.set_mode(Mode::Error(err.into()), None, cx),
                }
            })
            .ok();
        });
        self.set_mode(
            Mode::Running {
                label,
                previews: Vec::new(),
                _cancel: CancelOnDrop(cancel),
                _task: task,
            },
            None,
            cx,
        );
    }

    /// Replaces the selection with option `index` as one undo step, if the
    /// text is still what the options were made for.
    fn apply(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else { return };
        let Mode::Results(options) = &open.mode else {
            self.open = Some(open);
            return;
        };
        let Some(option) = options.get(index) else {
            self.open = Some(open);
            return;
        };
        self.changed(cx);
        self.focus_main(cx);
        self.editor.update(cx, |editor, cx| {
            if editor.text() == open.text && editor.selected_range() == open.range {
                editor.replace_range(open.range.clone(), option, cx);
                let inserted = open.range.start..editor.selected_range().end;
                editor.flash(vec![inserted], cx);
            }
        });
    }

    fn schedule_reconcile(&mut self, cx: &mut Context<Self>) {
        if self.reconcile_scheduled
            || (self.open.is_none() && self.panel.is_none() && !self.opening)
        {
            return;
        }
        self.reconcile_scheduled = true;
        // The editor pane's wrapper reconciles after the main window's next
        // draw, when the editor's layout and the selection's bounds are
        // current (see `wrap`).
        let main = self.main;
        cx.defer(move |cx| {
            main.update(cx, |_, window, _| window.refresh()).ok();
        });
    }

    /// Opens, resizes, reopens or closes the panel to show the current state
    /// above the selection.
    fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reconcile_scheduled = false;
        // Wait for the content's size rather than show a guess and correct
        // it, which reopens the panel wherever it moves.
        if self.opening || (self.open.is_some() && self.content_size.is_none()) {
            return;
        }
        let target = self.placement(window, cx);
        match (&mut self.panel, target) {
            (None, None) => {}
            (Some(panel), Some((bounds, _))) if panel.bounds == bounds => {}
            (Some(panel), Some((bounds, _)))
                if panel.pinned.corner(panel.bounds) == panel.pinned.corner(bounds) =>
            {
                panel.bounds = bounds;
                panel
                    .handle
                    .update(cx, |_, window, _| window.resize(bounds.size))
                    .ok();
            }
            (_, target) => {
                self.close_panel(cx);
                if let Some((bounds, side)) = target {
                    self.open_panel(bounds, side, window, cx);
                }
            }
        }
    }

    /// Where the panel goes, in screen coordinates: above the selection's
    /// first visible row, or below its last when there's no room above, with
    /// the edge that faces the selection. `None` when the popover is hidden
    /// or the selection is scrolled away.
    fn placement(&self, window: &Window, cx: &App) -> Option<(Bounds<Pixels>, Edge)> {
        self.open.as_ref()?;
        let (pane, rows) = if self.in_preview(cx) {
            let rows = self.preview.read(cx).selection_rects().to_vec();
            (self.preview_pane.get()?, rows)
        } else {
            (self.pane.get()?, self.editor.read(cx).selection_bounds())
        };
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|row| row.bottom() > pane.top() && row.top() < pane.bottom())
            .collect();
        let (first, last) = (rows.first()?, rows.last()?);
        let size = self.content_size?;

        let above = first.top() - GAP - size.height;
        let (y, side) = if above >= pane.top() + MARGIN {
            (above, Edge::Bottom)
        } else {
            (last.bottom() + GAP, Edge::Top)
        };
        let max_x = (pane.right() - size.width - MARGIN).max(pane.left() + MARGIN);
        let x = (first.left() - px(6.)).clamp(pane.left() + MARGIN, max_x);
        let origin = window.bounds().origin + point(x, y);
        Some((Bounds::new(origin.map(|v| v.round()), size), side))
    }

    /// Opens the panel at `bounds`, pinned at the `side` edge where the
    /// platform can anchor it there.
    fn open_panel(
        &mut self,
        bounds: Bounds<Pixels>,
        side: Edge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let display_id = window.display(cx).map(|display| display.id());
        let options = move |kind| WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id,
            window_background: crate::platform::popup_background(),
            ..Default::default()
        };
        // Growing away from the selection from the corner on the `side`
        // edge, so a resize keeps that edge in place.
        let anchored = WindowKind::AnchoredPopup(PopupOptions {
            parent: window.window_handle(),
            anchor_rect: Bounds::new(
                side.corner(bounds) - window.bounds().origin,
                gpui::size(px(1.), px(1.)),
            ),
            anchor: PopupAnchor::TopLeft,
            gravity: match side {
                Edge::Top => PopupGravity::BottomRight,
                Edge::Bottom => PopupGravity::TopRight,
            },
            constraint_adjustment: PopupConstraintAdjustment::SLIDE_X
                | PopupConstraintAdjustment::SLIDE_Y,
            offset: point(px(0.), px(0.)),
            grab: false,
        });
        self.opening = true;
        let popover = cx.entity();
        // The panel renders this entity as it opens, so it opens after the
        // current update.
        cx.defer(move |cx| {
            let mut activation = None;
            let mut open = |kind, cx: &mut App| {
                cx.open_window(options(kind), |window, cx| {
                    activation = Some(popover.update(cx, |_, cx| {
                        cx.observe_window_activation(window, |this, window, cx| {
                            if !window.is_window_active() {
                                this.dismiss_unless_panel_active(cx);
                            }
                        })
                    }));
                    cx.new(|_| PopoverPanel {
                        popover: popover.clone(),
                    })
                })
            };
            let opened = match open(anchored, cx) {
                // A `PopUp` resize keeps its bottom edge.
                Err(err) if err.is::<PopupNotSupportedError>() => {
                    open(WindowKind::PopUp, cx).map(|handle| (handle, Edge::Bottom))
                }
                opened => opened.map(|handle| (handle, side)),
            };
            popover.update(cx, |this, cx| {
                this.opening = false;
                match opened {
                    Ok((handle, pinned)) => {
                        this.panel = Some(Panel {
                            handle,
                            bounds,
                            pinned,
                            _activation: activation.expect("set when the window opens"),
                        });
                        // Dismissed while it opened.
                        if this.open.is_none() {
                            return this.close_panel(cx);
                        }
                        // Reopened elsewhere while the comment field shows.
                        this.focus_comment(cx);
                        // The state may have changed while it opened.
                        this.schedule_reconcile(cx);
                    }
                    Err(err) => {
                        eprintln!("draftmux: couldn't open the selection popover: {err:#}");
                        this.open = None;
                    }
                }
            });
        });
    }

    /// Called by the panel after layout with the content's natural size.
    fn measured(&mut self, measured: Size<Pixels>, cx: &mut Context<Self>) {
        let measured = measured.map(|v| v.ceil());
        if self.content_size != Some(measured) {
            self.content_size = Some(measured);
            self.schedule_reconcile(cx);
        }
    }

    fn content(&self, this: &Entity<Self>, cx: &App) -> AnyElement {
        let Some(open) = &self.open else {
            return div().into_any_element();
        };
        let palette = *theme::palette(cx);
        let item = |index: usize| {
            div()
                .id(("item", index))
                .active(|style| style.opacity(0.8))
                .rounded(ITEM_RADIUS)
                .on_mouse_move({
                    let this = this.clone();
                    move |_, _, cx| this.update(cx, |this, cx| this.set_highlight(index, cx))
                })
        };
        // The highlight fades in when it lands on an item and drops off when
        // it leaves, so sweeping across items doesn't leave a trail.
        let highlight = |index: usize, item: Stateful<Div>| -> AnyElement {
            if open.highlighted != Some(index) {
                return item.into_any_element();
            }
            item.text_color(palette.on_accent)
                .with_animation(
                    ("item-highlight", index),
                    Animation::new(HIGHLIGHT_FADE).with_easing(ease_out_quint()),
                    move |item, delta| item.bg(palette.accent.opacity(delta)),
                )
                .into_any_element()
        };
        let comment_item = |index: usize| {
            let color = if open.highlighted == Some(index) {
                palette.on_accent
            } else {
                palette.text
            };
            let item = item(index)
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(9.))
                .py(px(5.))
                .whitespace_nowrap()
                .child(
                    gpui::svg()
                        .path(COMMENT_ICON)
                        .size(px(13.))
                        .text_color(color),
                )
                .child("Comment")
                .on_click({
                    let this = this.clone();
                    move |_, _, cx| this.update(cx, |this, cx| this.start_comment(cx))
                });
            highlight(index, item)
        };
        let divider = || {
            div()
                .flex_none()
                .w(px(1.))
                .h(px(16.))
                .mx(px(3.))
                .bg(palette.menu_line)
        };
        let content = match &open.mode {
            Mode::Actions(actions) => div()
                .flex()
                .items_center()
                .gap(px(2.))
                .flex_nowrap()
                .children(
                    std::iter::once(("Alternatives", Some("✦")))
                        .chain(actions.iter().map(|action| (action.name.as_str(), None)))
                        .enumerate()
                        .map(|(index, (label, icon))| {
                            let item =
                                item(index)
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .px(px(9.))
                                    .py(px(5.))
                                    .whitespace_nowrap()
                                    .children(icon.map(|icon| {
                                        div().text_size(px(11.)).child(icon.to_string())
                                    }))
                                    .child(div().max_w(px(140.)).truncate().child(label.to_owned()))
                                    .on_click({
                                        let this = this.clone();
                                        move |_, _, cx| {
                                            this.update(cx, |this, cx| this.pick_action(index, cx))
                                        }
                                    });
                            highlight(index, item)
                        }),
                )
                .child(divider())
                .child(comment_item(actions.len() + 1))
                // Until the user has added a selection, nothing else shows
                // that they can.
                .when(self.show_add_hint, |actions| {
                    actions.child(
                        div()
                            .flex_none()
                            .pl(px(4.))
                            .pr(px(7.))
                            .text_size(px(11.))
                            .text_color(palette.faint)
                            .whitespace_nowrap()
                            .child(editor::by_platform!("⌘-drag to add", "Ctrl-drag to add")),
                    )
                })
                .into_any_element(),
            Mode::Several(count) => div()
                .flex()
                .items_center()
                .gap(px(2.))
                .flex_nowrap()
                .whitespace_nowrap()
                .child(
                    div()
                        .px(px(9.))
                        .py(px(5.))
                        .text_color(palette.muted)
                        .child(format!("{count} selections")),
                )
                .child(divider())
                .child(comment_item(0))
                .into_any_element(),
            Mode::Comment(field) => {
                let empty = field.read(cx).text().trim().is_empty();
                let placeholder: SharedString = match self.editor.read(cx).selected_ranges().len() {
                    1 => "Tell pi what to change here…".into(),
                    n => format!("Tell pi what to do with these {n} selections…").into(),
                };
                let focus = field.focus_handle(cx);
                div()
                    .w(COMMENT_WIDTH - PADDING * 2.)
                    .flex()
                    .flex_col()
                    .key_context(COMMENT_CONTEXT)
                    .on_action({
                        let this = this.clone();
                        move |_: &Confirm, _, cx| this.update(cx, |this, cx| this.send_comment(cx))
                    })
                    .on_action({
                        let this = this.clone();
                        move |_: &Back, _, cx| this.update(cx, |this, cx| this.back(cx))
                    })
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(px(6.))
                            .p(px(4.))
                            .pl(px(9.))
                            .rounded(ITEM_RADIUS)
                            .bg(palette.menu_field)
                            .border_1()
                            .border_color(palette.hairline)
                            // The field is as wide as its text; a press beside
                            // it still focuses it.
                            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                window.focus(&focus, cx)
                            })
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w_0()
                                    .child(field.clone())
                                    .when(empty, |input| {
                                        input.child(
                                            div()
                                                .absolute()
                                                .top(px(2.))
                                                .left_0()
                                                .text_color(palette.faint)
                                                .whitespace_nowrap()
                                                .child(placeholder),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id("send-comment")
                                    .size(px(22.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .bg(if empty {
                                        palette.send_disabled
                                    } else {
                                        palette.accent
                                    })
                                    .when(!empty, |button| {
                                        button.active(|style| style.opacity(0.8)).on_click({
                                            let this = this.clone();
                                            move |_, _, cx| {
                                                this.update(cx, |this, cx| this.send_comment(cx))
                                            }
                                        })
                                    })
                                    .child(
                                        gpui::svg()
                                            .path(SEND_ICON)
                                            .size(px(12.))
                                            .text_color(palette.on_accent),
                                    ),
                            ),
                    )
                    .child(footer(
                        &["↵ send to pi", "⇧↵ newline", "esc back"],
                        &palette,
                    ))
                    .into_any_element()
            }
            Mode::Running {
                label, previews, ..
            } => div()
                .flex()
                .flex_col()
                .when(!previews.is_empty(), |view| {
                    view.w(RESULTS_WIDTH - PADDING * 2.)
                })
                .children(previews.iter().map(|option| {
                    div()
                        .px(px(8.))
                        .py(px(7.))
                        .line_height(px(19.))
                        .child(option.trim().to_string())
                }))
                .child(
                    div()
                        .px(px(10.))
                        .py(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_color(palette.muted)
                        .whitespace_nowrap()
                        .child(crate::chat::spinner(&palette))
                        .child(label.clone()),
                )
                .when(!previews.is_empty(), |view| {
                    view.child(footer(
                        &["Preview · waiting to finish", "esc cancel"],
                        &palette,
                    ))
                })
                .into_any_element(),
            Mode::Results(options) => {
                let keys = format!("1–{} or ↵ apply", options.len());
                div()
                    .w(RESULTS_WIDTH - PADDING * 2.)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(options.iter().enumerate().map(|(index, option)| {
                        let highlighted = open.highlighted == Some(index);
                        let item = item(index)
                            .flex()
                            .gap(px(8.))
                            .px(px(8.))
                            .py(px(7.))
                            .line_height(px(19.))
                            .child(
                                div()
                                    .w(px(10.))
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(if highlighted {
                                        palette.on_accent_muted
                                    } else {
                                        palette.faint
                                    })
                                    .child((index + 1).to_string()),
                            )
                            .child(div().flex_1().min_w_0().child(option.trim().to_string()))
                            .on_click({
                                let this = this.clone();
                                move |_, _, cx| this.update(cx, |this, cx| this.apply(index, cx))
                            });
                        highlight(index, item)
                    }))
                    .child(footer(&[keys.as_str(), "↑↓ move", "esc cancel"], &palette))
                    .into_any_element()
            }
            Mode::Error(message) => div()
                .max_w(RESULTS_WIDTH - PADDING * 2.)
                .px(px(10.))
                .py(px(8.))
                .text_color(palette.muted)
                .child(message.clone())
                .into_any_element(),
        };
        // The actions show with the panel; later states replace them in place
        // and fade in rather than snapping.
        let key = match &open.mode {
            Mode::Actions(_) | Mode::Several(_) => return content,
            Mode::Comment(_) => "popover-comment",
            Mode::Running { .. } => "popover-running",
            Mode::Results(_) => "popover-results",
            Mode::Error(_) => "popover-error",
        };
        div()
            .child(content)
            .with_animation(
                key,
                Animation::new(MODE_FADE).with_easing(ease_out_quint()),
                |content, delta| content.opacity(delta),
            )
            .into_any_element()
    }
}

fn footer(hints: &[&str], palette: &Palette) -> impl IntoElement {
    div()
        .flex()
        .gap(px(10.))
        .mt(px(4.))
        .px(px(8.))
        .pt(px(6.))
        .pb(px(3.))
        .border_t_1()
        .border_color(palette.menu_line)
        .text_size(px(11.))
        .text_color(palette.faint)
        .whitespace_nowrap()
        .children(hints.iter().map(|hint| hint.to_string()))
}

/// The comment field: the UI font at the popover's size, as tall as its
/// text.
fn comment_style(palette: &Palette) -> EditorStyle {
    EditorStyle {
        font: font(editor::platform::ui_font()),
        font_size: px(13.),
        line_height: px(19.),
        padding_x: px(0.),
        padding_top: px(2.),
        padding_bottom: px(1.),
        text: palette.text,
        heading: palette.text,
        fit_content: true,
        ..palette.editor_style()
    }
}

/// The root view of the popup panel; renders [`SelectionPopover`]'s state.
struct PopoverPanel {
    popover: Entity<SelectionPopover>,
}

impl Render for PopoverPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let popover = self.popover.clone();
        let content = popover.read(cx).content(&popover, cx);
        let palette = theme::palette(cx);
        let (background, text) = (crate::platform::menu_color(palette), palette.text);
        div()
            .size_full()
            .bg(background)
            .text_color(text)
            .text_size(px(13.))
            .font_family(editor::platform::ui_font())
            // A click makes the panel key; give the keyboard back to the
            // editor, unless the comment field has it.
            .on_mouse_up(gpui::MouseButton::Left, {
                let popover = popover.clone();
                move |_, _, cx| {
                    popover.update(cx, |this, cx| {
                        if !this.commenting() {
                            this.focus_main(cx);
                        }
                    })
                }
            })
            .child(measured_content(popover, content))
    }
}

/// `content` laid out at its natural size, then measured to size the
/// panel.
fn measured_content(popover: Entity<SelectionPopover>, content: AnyElement) -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .p(PADDING)
        .child(content)
        .child(
            canvas(
                move |bounds, _, cx| popover.update(cx, |this, cx| this.measured(bounds.size, cx)),
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn previews_are_read_only_and_final_replacement_is_one_undo(cx: &mut gpui::TestAppContext) {
        let config = tempfile::tempdir().unwrap();
        cx.update(|cx| crate::app_settings::init(config.path().to_owned(), cx));
        let window = cx.add_window(|_, _| gpui::EmptyView);
        let (editor, popover) = window
            .update(cx, |_, window, cx| {
                let editor = cx.new(|cx| Editor::new("Hi there\n", cx));
                let preview = cx.new(TextSelection::new);
                let store = Store::new(config.path().join("drafts"));
                let popover =
                    cx.new(|cx| SelectionPopover::new(editor.clone(), preview, store, window, cx));
                (editor, popover)
            })
            .unwrap();
        cx.update(|cx| {
            editor.update(cx, |editor, cx| editor.select_range(3..9, cx));
            popover.update(cx, |popover, cx| {
                popover.open = Some(Open {
                    text: "Hi there\n".into(),
                    range: 3..9,
                    highlighted: None,
                    mode: Mode::Running {
                        label: "Generating alternatives…".into(),
                        previews: vec!["friend\n".into()],
                        _cancel: CancelOnDrop(Arc::new(AtomicBool::new(false))),
                        _task: Task::ready(()),
                    },
                });
                assert_eq!(popover.item_count(), 0);
                popover.move_highlight(1, cx);
                popover.confirm(cx);
                popover.apply(0, cx);
                assert_eq!(popover.editor.read(cx).text(), "Hi there\n");
                popover.set_mode(Mode::Results(vec!["friend\n".into()]), Some(0), cx);
                popover.confirm(cx);
                assert_eq!(popover.editor.read(cx).text(), "Hi friend\n");
            });
            editor.update(cx, |editor, cx| {
                assert!(editor.undo(cx));
                assert_eq!(editor.text(), "Hi there\n");
                assert!(!editor.undo(cx));
            });
        });
    }

    #[gpui::test]
    fn comment_sends_its_text_and_esc_goes_back_to_the_items(cx: &mut gpui::TestAppContext) {
        let config = tempfile::tempdir().unwrap();
        cx.update(|cx| crate::app_settings::init(config.path().to_owned(), cx));
        let window = cx.add_window(|_, _| gpui::EmptyView);
        let (editor, popover) = window
            .update(cx, |_, window, cx| {
                let editor = cx.new(|cx| Editor::new("Hi there\n", cx));
                let preview = cx.new(TextSelection::new);
                let store = Store::new(config.path().join("drafts"));
                let popover =
                    cx.new(|cx| SelectionPopover::new(editor.clone(), preview, store, window, cx));
                (editor, popover)
            })
            .unwrap();
        let sent = Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let sent = sent.clone();
            cx.subscribe(
                &popover,
                move |_, SelectionPopoverEvent::Comment(text), _| {
                    sent.borrow_mut().push(text.clone())
                },
            )
            .detach();
            editor.update(cx, |editor, cx| editor.select_range(3..8, cx));
            let field = cx.new(|cx| Editor::new("", cx));
            popover.update(cx, |popover, cx| {
                let Mode::Actions(actions) = popover.items_mode(cx) else {
                    panic!("one selection has the actions");
                };
                // After Alternatives and the actions.
                let comment = Some(actions.len() + 1);
                popover.open = Some(Open {
                    text: "Hi there\n".into(),
                    range: 3..8,
                    highlighted: None,
                    mode: Mode::Comment(field.clone()),
                });
                assert_eq!(popover.item_count(), 0);
                // Nothing written, nothing sent.
                popover.confirm(cx);
                assert!(popover.commenting());
                popover.back(cx);
                assert!(matches!(
                    popover.open,
                    Some(Open { mode: Mode::Actions(_), highlighted, .. }) if highlighted == comment
                ));
                popover.set_mode(Mode::Comment(field.clone()), None, cx);
            });
            field.update(cx, |field, cx| field.set_text("  Make it formal\n", cx));
            popover.update(cx, |popover, cx| popover.confirm(cx));
            assert!(popover.read(cx).open.is_none());
        });
        assert_eq!(*sent.borrow(), ["Make it formal"]);
    }

    #[test]
    fn previews_keep_request_alive_and_ignore_stale_updates() {
        let request = Arc::new(AtomicBool::new(false));
        let stale = Arc::new(AtomicBool::new(false));
        let mut mode = Mode::Running {
            label: "Generating alternatives…".into(),
            previews: Vec::new(),
            _cancel: CancelOnDrop(request.clone()),
            _task: Task::ready(()),
        };
        assert!(!mode.set_previews(&stale, vec!["stale".into()]));
        assert!(mode.set_previews(&request, vec!["first".into()]));
        assert!(!mode.set_previews(&request, vec!["first".into()]));
        assert!(matches!(&mode, Mode::Running { previews, .. } if previews == &["first"]));
        assert!(!request.load(Ordering::Relaxed));
        assert!(mode.set_previews(&request, Vec::new()));
        assert!(!request.load(Ordering::Relaxed));
        drop(mode);
        assert!(request.load(Ordering::Relaxed));
        let mut completed = Mode::Results(vec!["final".into()]);
        assert!(!completed.set_previews(&request, vec!["late".into()]));
    }
}
