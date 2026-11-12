//! A cached rendering boundary for the preview's fixed-size viewport.

use std::cell::Cell;
use std::rc::Rc;

use draftmux_storage::Target;
use gpui::{
    AnyElement, App, Bounds, Context, DispatchPhase, Entity, IntoElement, MouseMoveEvent, Pixels,
    Point, Render, ScrollHandle, Window, canvas, div, prelude::*, px,
};
use markup::Document;

use crate::block_cache::BlockCache;
use crate::inline_text::MarkHandlers;
use crate::markup_view::{self, Redline, RedlineColors, SelectBody};
use crate::selectable::{self, TextSelection};
use crate::theme;

pub(super) struct Preview {
    pub document: Option<Document>,
    pub fields: markup::frontmatter::Fields,
    pub copied_field: Option<String>,
    pub copied_task: gpui::Task<()>,
    pub target: Option<Target>,
    pub selection: Entity<TextSelection>,
    pub scroll: ScrollHandle,
    pub cache: BlockCache,
    pub appearance: Option<(Target, bool, f32, f32)>,
    /// The agent is writing into the draft, or about to write its first
    /// draft; an empty preview shows a skeleton of the message instead of
    /// "Nothing to preview".
    pub writing: bool,
    /// While reviewing, the redline shown instead of the document.
    pub redline: Option<PreviewRedline>,
    pub _selection: gpui::Subscription,
}

/// A redline of the draft ([`crate::compare::redline`]) parsed for the
/// target, and how its changes are drawn.
pub(super) struct PreviewRedline {
    pub document: Rc<Document>,
    pub selected: Option<usize>,
    /// Whether each change is kept.
    pub kept: Vec<bool>,
    pub handlers: Rc<MarkHandlers>,
    /// The change under the pointer during the current mouse move, set by
    /// the marks' hover handlers.
    pub hit: Rc<Cell<Option<(usize, Point<Pixels>)>>>,
    /// Told after every mouse move which change, if any, is under the
    /// pointer, and where the pointer is.
    pub on_hover: Rc<dyn Fn(Option<(usize, Point<Pixels>)>, &mut Window, &mut App)>,
}

pub(super) struct FieldCopied(pub String);
impl gpui::EventEmitter<FieldCopied> for Preview {}

impl Render for Preview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        canvas(
            move |bounds, window, cx| {
                let mut body = entity.update(cx, |this, cx| this.body(bounds, window, cx));
                body.layout_as_root(bounds.size.into(), window, cx);
                body.prepaint_at(bounds.origin, window, cx);
                entity.update(cx, |this, cx| {
                    let texts = this.cache.texts();
                    this.selection
                        .update(cx, |selection, _| selection.set_texts(texts));
                });
                body
            },
            |_, mut body: AnyElement, window, cx| body.paint(window, cx),
        )
        .size_full()
    }
}

impl Preview {
    fn copy_field(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(value) = self.fields.get(key) else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(value.clone()));
        self.copied_field = Some(key.to_owned());
        cx.emit(FieldCopied(key.to_owned()));
        self.copied_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                this.copied_field = None;
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn fields_card(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if self.fields.is_empty() {
            return None;
        }
        let palette = theme::get(cx).palette;
        let compact_label = self.fields.len() == 1
            && (self.fields.contains_key("title") || self.fields.contains_key("subject"));
        Some(
            div()
                .rounded(px(10.))
                .bg(palette.card)
                .border_1()
                .border_color(palette.card_ring)
                .shadow(vec![super::shadow(palette.card_drop, 1., 3., 0.)])
                .mx(px(-4.))
                .mb(px(20.))
                .children(self.fields.iter().enumerate().map(|(index, (key, value))| {
                    let group: gpui::SharedString = format!("field-{index}").into();
                    let copied = self.copied_field.as_ref() == Some(key);
                    let copy_key = key.clone();
                    let mut chars = key.chars();
                    let label = chars
                        .next()
                        .map(|first| {
                            first.to_uppercase().collect::<String>()
                                + &chars.as_str().to_lowercase()
                        })
                        .unwrap_or_default();
                    div()
                        .group(group.clone())
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .pl(px(12.))
                        .pr(px(6.))
                        .py(px(7.))
                        .text_size(px(13.))
                        .text_color(palette.text)
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(palette.hairline)
                        })
                        .child(
                            div()
                                .when(!compact_label, |label| label.w(px(64.)))
                                .flex_shrink_0()
                                .text_color(palette.muted)
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(key == "title" || key == "subject", |value| {
                                    value.font_weight(gpui::FontWeight::SEMIBOLD)
                                })
                                .child(value.clone()),
                        )
                        .child(
                            div()
                                .id(("copy-field", index))
                                .focusable()
                                .tab_index(0)
                                .aria_label(format!("Copy {key}"))
                                .cursor_pointer()
                                .size(px(28.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .opacity(if copied { 1. } else { 0. })
                                .group_hover(group, |style| style.opacity(1.))
                                .focus_visible(|style| style.opacity(1.).bg(palette.hover))
                                .hover(|style| style.bg(palette.hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.copy_field(&copy_key, cx);
                                }))
                                .child(
                                    gpui::svg()
                                        .path(if copied {
                                            crate::assets::TOAST_DONE_ICON
                                        } else {
                                            crate::assets::COPY_ICON
                                        })
                                        .size(px(14.))
                                        .text_color(if copied {
                                            palette.toast_done
                                        } else {
                                            palette.muted
                                        }),
                                ),
                        )
                })),
        )
    }

    fn body(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _timing = editor::timing::span("preview-elements");
        let frame = editor::timing::span("preview-frame");
        let theme = theme::get(cx);
        let appearance = self
            .target
            .map(|target| (target, theme.dark, window.scale_factor(), theme.zoom));
        if self.appearance != appearance {
            self.cache.clear();
            self.appearance = appearance;
        }
        let document = match &self.redline {
            Some(redline) => Some(&*redline.document),
            None => self.document.as_ref(),
        };
        if document.is_none_or(|doc| doc.blocks.is_empty()) {
            self.cache.reset();
        }
        // px_6 is 1.5 rem on each side of the scrolling viewport.
        let width = (bounds.size.width - window.rem_size() * 3.).max(px(0.));
        let (body, background) = match (document, self.target) {
            (Some(doc), Some(target)) if !doc.blocks.is_empty() => {
                let mut style = markup_view::style(target, theme.dark).zoomed(theme.zoom);
                style.redline = self.redline.as_ref().map(|redline| {
                    let palette = &theme.palette;
                    Rc::new(Redline {
                        selected: redline.selected,
                        kept: redline.kept.clone(),
                        colors: RedlineColors {
                            added: palette.diff_added,
                            added_background: palette.added_bg,
                            removed: palette.diff_removed,
                            removed_background: palette.removed_bg,
                            selected: palette.accent,
                        },
                        handlers: redline.handlers.clone(),
                    })
                });
                let style = Rc::new(style);
                // A redline's text isn't the draft's, so it isn't selected
                // for the editor.
                let select = self.redline.is_none().then(|| SelectBody {
                    state: self.selection.clone(),
                    body: 0,
                    color: theme.palette.selection,
                    registry: None,
                });
                (
                    markup_view::render_preview(
                        doc,
                        style.clone(),
                        &self.scroll,
                        select,
                        (&mut self.cache, width),
                        window,
                        cx,
                    ),
                    Some(style.background),
                )
            }
            _ if self.writing
                && let Some(target) = self.target =>
            {
                let style = markup_view::style(target, theme.dark).zoomed(theme.zoom);
                (
                    super::skeleton::render(target, &style),
                    Some(style.background),
                )
            }
            _ => (
                div()
                    // pt_5 and pb_10 take 3.75 rem of the viewport's height.
                    .h((bounds.size.height - window.rem_size() * 3.75).max(px(0.)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.))
                    .text_color(theme.palette.faint)
                    .child("Nothing to preview")
                    .into_any_element(),
                None,
            ),
        };

        let hover = self.redline.as_ref().map(|redline| {
            let (hit, on_hover) = (redline.hit.clone(), redline.on_hover.clone());
            // Painted before the body, so it hears each mouse move after the
            // marks have reported the change under the pointer.
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            let hit = hit.take();
                            on_hover(hit.map(|(hunk, _)| (hunk, event.position)), window, cx);
                        }
                    });
                },
            )
            .absolute()
        });
        selectable::area(&self.selection, cx)
            .id("preview")
            .size_full()
            .when_some(background, |this, background| this.bg(background))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px_6()
            .pt_5()
            .pb_10()
            .children(hover)
            .children(self.fields_card(cx))
            .child(body)
            .child(canvas(|_, _, _| (), move |_, _, _, _| drop(frame)).absolute())
            .into_any_element()
    }
}
