//! The in-process Markdown Format action and wrap menu.

pub use markup::format::{WRAP_WIDTH, Wrap};
use std::path::PathBuf;
use std::time::Duration;

use editor::{Editor, EditorEvent};
use gpui::{
    Anchor, Animation, AnimationExt, App, Context, ElementId, Entity, FontWeight, KeyBinding,
    SharedString, Subscription, Task, Transformation, Window, actions, anchored, deferred, div,
    ease_out_quint, percentage, prelude::*, px, svg,
};

use crate::assets::CHEVRON_ICON;
use crate::menu::{MenuItem, menu_panel};
use crate::theme;

actions!(format, [Format]);

const MESSAGE_FADE: Duration = Duration::from_millis(200);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(editor::secondary!("shift-f"), Format, None)]);
}

/// Runs Format for one editor and renders its button and last message.
pub struct Formatter {
    editor: Entity<Editor>,
    /// The focused draft's directory, used to reject stale results.
    draft_dir: Option<PathBuf>,
    agent_running: bool,
    /// The draft's target reads text differently from CommonMark, so Format
    /// is hidden.
    hidden: bool,
    /// Changes even when the user switches away and back before completion.
    generation: u64,
    running: Option<Task<()>>,
    message: Option<SharedString>,
    menu_open: bool,
    _editor_events: Subscription,
}

impl Formatter {
    pub fn new(editor: Entity<Editor>, cx: &mut Context<Self>) -> Self {
        let _editor_events = cx.subscribe(&editor, |this, _, event, cx| {
            if *event == EditorEvent::Edited {
                this.generation += 1;
                if this.message.take().is_some() {
                    cx.notify();
                }
            }
        });
        Self {
            editor,
            draft_dir: None,
            agent_running: false,
            hidden: false,
            generation: 0,
            running: None,
            message: None,
            menu_open: false,
            _editor_events,
        }
    }

    pub fn set_draft_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.generation += 1;
        self.draft_dir = Some(dir);
        self.menu_open = false;
        self.message = None;
        cx.notify();
    }

    /// Format is unavailable while the agent runs, and a format that
    /// finishes after the agent started is discarded.
    pub fn set_agent_running(&mut self, running: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        self.agent_running = running;
        self.menu_open = false;
        cx.notify();
    }

    /// Format uses CommonMark, so Format is hidden for targets that read
    /// text differently and a format that finishes after the switch is
    /// discarded. It would escape and rewrite plain text, and in a Slack
    /// message, where every line break shows, it merges a line after a `>`
    /// quote into the quote, adds and collapses blank lines and renumbers
    /// lists.
    pub fn set_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        self.hidden = hidden;
        self.menu_open = false;
        self.message = None;
        cx.notify();
    }

    fn available(&self) -> bool {
        self.draft_dir.is_some() && !self.agent_running && !self.hidden && self.running.is_none()
    }

    pub fn format(&mut self, wrap: Wrap, cx: &mut Context<Self>) {
        self.format_with(move |input| async move { run_formatter(&input, wrap) }, cx);
    }

    fn format_with<F>(&mut self, work: impl FnOnce(String) -> F, cx: &mut Context<Self>)
    where
        F: Future<Output = Result<markup::format::Report, String>> + Send + 'static,
    {
        if !self.available() {
            return;
        }
        let Some(dir) = self.draft_dir.clone() else {
            return;
        };
        let generation = self.generation;
        let input = self.editor.read(cx).text().to_string();
        self.message = None;
        cx.notify();

        self.menu_open = false;
        let run = cx.background_executor().spawn(work(input.clone()));
        self.running = Some(cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| {
                this.running = None;
                cx.notify();
                // The text, the draft or the editor's owner changed while
                // Format ran; its output is about something else.
                if this.agent_running
                    || this.hidden
                    || this.draft_dir.as_ref() != Some(&dir)
                    || !result_is_current(
                        generation,
                        this.generation,
                        &input,
                        this.editor.read(cx).text(),
                    )
                {
                    return;
                }
                match result {
                    Err(error) => this.message = Some(error.into()),
                    Ok(report) => {
                        if report.text != input {
                            this.editor.update(cx, |editor, cx| {
                                crate::changes::replace_all(editor, &report.text, cx)
                            });
                        } else if !report.declined {
                            this.message = Some("Already formatted".into());
                        }
                        if report.declined {
                            this.message =
                                Some("Some Markdown was left unchanged for safety".into());
                        }
                    }
                }
            })
            .ok();
        }));
    }
}

fn result_is_current(start: u64, current: u64, input: &str, text: &str) -> bool {
    start == current && input == text
}

fn run_formatter(input: &str, wrap: Wrap) -> Result<markup::format::Report, String> {
    recover_format(|| markup::format::format_report(input, wrap))
}

fn recover_format(
    work: impl FnOnce() -> markup::format::Report + std::panic::UnwindSafe,
) -> Result<markup::format::Report, String> {
    std::panic::catch_unwind(work).map_err(|_| "Couldn't format this Markdown".to_owned())
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;

impl Render for Formatter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = !self.hidden;
        let available = self.available();
        let palette = *theme::palette(cx);
        div()
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .children(self.message.clone().map(|message| {
                div()
                    .max_w(px(360.))
                    .truncate()
                    .text_color(palette.muted)
                    .child(message.clone())
                    // Keyed by the text, so a different message fades in too.
                    .with_animation(
                        ElementId::Name(message),
                        Animation::new(MESSAGE_FADE).with_easing(ease_out_quint()),
                        |message, delta| message.opacity(delta),
                    )
            }))
            .when(visible, |this| {
                this.child(self.button(available, &palette, cx))
            })
    }
}

impl Formatter {
    /// Format, with a chevron that opens the wrap choices.
    fn button(
        &mut self,
        available: bool,
        palette: &theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !available {
            self.menu_open = false;
        }
        let format = div()
            .id("format")
            .flex()
            .items_center()
            .gap_1p5()
            .pl_2()
            .pr_1p5()
            .py_0p5()
            .rounded_l_md()
            .child("Format")
            .child(
                div()
                    .text_color(palette.muted)
                    .child(editor::by_platform!("⌘⇧F", "Ctrl+Shift+F")),
            )
            .when(available, |button| {
                button
                    .hover(|style| style.bg(palette.hover))
                    .active(|style| style.opacity(0.8))
                    .on_click(cx.listener(|this, _, _, cx| this.format(Wrap::Keep, cx)))
            });
        let chevron = div()
            .id("format-menu")
            .h_full()
            .flex()
            .items_center()
            .px_1()
            .rounded_r_md()
            .when(self.menu_open, |button| button.bg(palette.hover))
            .child(
                svg()
                    .path(CHEVRON_ICON)
                    .size(px(10.))
                    .text_color(palette.muted)
                    .with_transformation(Transformation::rotate(percentage(0.25))),
            )
            .when(available, |button| {
                button
                    .hover(|style| style.bg(palette.hover))
                    .active(|style| style.opacity(0.8))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = !this.menu_open;
                        cx.notify();
                    }))
            });
        div()
            .relative()
            .flex()
            .items_stretch()
            .text_color(palette.text)
            .font_weight(FontWeight::MEDIUM)
            .when(!available, |group| group.opacity(0.4))
            .child(format)
            .child(chevron)
            .when(self.menu_open, |group| {
                group.child(
                    div().absolute().top_full().right_0().mt_1().child(deferred(
                        anchored()
                            .anchor(Anchor::TopRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(self.menu(palette, cx)),
                    )),
                )
            })
    }

    fn menu(&self, palette: &theme::Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let item = |label: String, wrap| MenuItem {
            label: label.into(),
            checked: false,
            enabled: true,
            action: Some(wrap),
            detail: None,
            heading: false,
        };
        let items = [
            item(format!("Wrap at {WRAP_WIDTH} Columns"), Wrap::Always),
            item("Unwrap Paragraphs".into(), Wrap::Never),
        ];
        let pick = cx.entity().downgrade();
        let dismiss = cx.entity().downgrade();
        menu_panel(
            palette,
            &items,
            move |wrap, _, cx| {
                pick.update(cx, |this, cx| {
                    this.menu_open = false;
                    this.format(wrap, cx);
                })
                .ok();
            },
            move |_, cx| {
                dismiss
                    .update(cx, |this, cx| {
                        this.menu_open = false;
                        cx.notify();
                    })
                    .ok();
            },
        )
    }
}
