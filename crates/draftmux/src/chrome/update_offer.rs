//! The toolbar's update button and the card it opens with what changed in
//! the offered release and a way to take it.

use std::time::Duration;

use gpui::{
    Anchor, Animation, AnimationExt as _, App, Context, ElementId, FocusHandle, FontWeight,
    IntoElement, KeyBinding, MouseButton, ScrollHandle, SharedString, Window, anchored, deferred,
    div, linear_color_stop, linear_gradient, prelude::*, px,
};
use markup::Document;
use semver::Version;

use super::{Chrome, shadow};
use crate::markup_view::{self, MarkupStyle};
use crate::theme::{self, Palette, hex};
use crate::update::{self, CURRENT_VERSION, Release, Updater};

gpui::actions!(update_card, [CloseUpdateCard]);

/// How long the update button takes to appear, and each of the two rings it
/// sends out after that.
const BUTTON_ENTER: f32 = 0.22;
const BUTTON_RING: f32 = 1.4;
const BUTTON_RINGS: f32 = 2.;
/// The widest the ring spreads, in pixels.
const RING_SPREAD: f32 = 8.;

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CloseUpdateCard,
        Some("UpdateCard"),
    )]);
}

/// The open card, with the offered release's notes parsed once. It holds
/// the focus so Escape closes it, and hands it back when it closes.
pub(super) struct UpdateCard {
    version: Version,
    notes: Option<Document>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    previous: Option<FocusHandle>,
}

impl Chrome {
    /// Opens the card of the offered release, if there is one, taking the
    /// focus so Escape closes it.
    pub(super) fn open_update_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((release, _)) = offer(cx) else {
            return;
        };
        let focus = cx.focus_handle();
        let previous = window.focused(cx);
        focus.focus(window, cx);
        self.update_card = Some(UpdateCard {
            notes: release.notes.as_deref().map(markup::github::parse_file),
            version: release.version,
            scroll: ScrollHandle::new(),
            focus,
            previous,
        });
        cx.notify();
    }

    fn close_update_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.update_card.take() else {
            return;
        };
        if card.focus.is_focused(window) {
            match &card.previous {
                Some(previous) => previous.focus(window, cx),
                None => window.blur(cx),
            }
        }
        cx.notify();
    }

    /// Opens the card of an installed update or of one this copy can't
    /// install itself.
    pub(super) fn update_button(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let (release, ready) = offer(cx)?;
        let label = if ready {
            "Restart to Update"
        } else {
            "Update Available"
        };
        let tooltip: SharedString = format!("What’s New in Draftmux {}", release.version).into();
        // Keyed by the offer, so each new offer is announced once.
        let pulse = ElementId::Name(format!("update-{}-{ready}", release.version).into());
        let ring = palette.accent;
        let card = self
            .update_card
            .as_ref()
            .filter(|card| card.version == release.version)
            .map(|card| {
                div().absolute().top_full().right_0().mt_1().child(deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .snap_to_window_with_margin(px(8.))
                        .child(update_card(card, &release, ready, palette, cx)),
                ))
            });
        Some(
            div().relative().flex_none().children(card).child(
                div()
                    .id("update")
                    .h(px(22.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .bg(palette.accent_soft)
                    .text_color(palette.accent)
                    .hover(|style| style.opacity(0.85))
                    .active(|style| style.opacity(0.7))
                    .tooltip(crate::tooltip::text(tooltip))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // Only opens: the mouse-down already closed an open card.
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_update_card(window, cx);
                    }))
                    .child(label)
                    .with_animation(
                        pulse,
                        Animation::new(Duration::from_secs_f32(
                            BUTTON_ENTER + BUTTON_RING * BUTTON_RINGS,
                        )),
                        move |button, delta| {
                            let elapsed = delta * (BUTTON_ENTER + BUTTON_RING * BUTTON_RINGS);
                            let enter = ease_out((elapsed / BUTTON_ENTER).min(1.));
                            let rings = (elapsed - BUTTON_ENTER).max(0.) / BUTTON_RING;
                            let button = button.opacity(enter);
                            if rings >= BUTTON_RINGS {
                                return button;
                            }
                            let spread = ease_out(rings.fract());
                            button.shadow(vec![shadow(
                                ring.opacity(0.45 * (1. - spread)),
                                0.,
                                0.,
                                RING_SPREAD * spread,
                            )])
                        },
                    ),
            ),
        )
    }
}

/// Cubic ease-out over `t` in 0..=1.
fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// The release the toolbar offers, and whether it is installed and only
/// waits for a restart.
fn offer(cx: &Context<Chrome>) -> Option<(Release, bool)> {
    match &Updater::global(cx).read(cx).status {
        update::Status::Ready(release) => Some((release.clone(), true)),
        update::Status::Blocked { release, .. } => Some((release.clone(), false)),
        _ => None,
    }
}

fn update_card(
    card: &UpdateCard,
    release: &Release,
    ready: bool,
    palette: &Palette,
    cx: &mut Context<Chrome>,
) -> impl IntoElement + use<> {
    let detail: SharedString = if ready {
        format!("Installed. Restart Draftmux to switch from {CURRENT_VERSION}.").into()
    } else {
        match &Updater::global(cx).read(cx).status {
            update::Status::Blocked { reason, .. } => reason.clone().into(),
            _ => SharedString::default(),
        }
    };
    let notes = match &card.notes {
        Some(doc) => markup_view::render(doc, &notes_style(cx), "update-notes", None),
        None => div()
            .text_color(palette.muted)
            .child("The release notes couldn’t be loaded.")
            .into_any_element(),
    };
    let (action, page) = if ready {
        ("Restart to Update", None)
    } else {
        ("Open Release Page", Some(release.page.clone()))
    };
    div()
        .id("update-card")
        .track_focus(&card.focus)
        .key_context("UpdateCard")
        .occlude()
        .w(px(380.))
        .flex()
        .flex_col()
        .rounded(px(10.))
        .bg(palette.menu)
        .text_size(px(13.))
        .line_height(px(18.))
        .text_color(palette.text)
        .shadow(vec![
            shadow(hex(0x00000033), 0., 0., 0.5),
            shadow(hex(0x00000029), 6., 18., 0.),
        ])
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_update_card(window, cx)))
        .on_action(
            cx.listener(|this, _: &CloseUpdateCard, window, cx| this.close_update_card(window, cx)),
        )
        .child(
            div()
                .px(px(16.))
                .pt(px(14.))
                .pb(px(12.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .border_b_1()
                .border_color(palette.hairline)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("What’s New in Draftmux {}", release.version)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(palette.muted)
                        .child(detail),
                ),
        )
        .child(
            div()
                .id("update-notes-scroll")
                .max_h(px(320.))
                .overflow_y_scroll()
                .track_scroll(&card.scroll)
                .px(px(16.))
                .py(px(12.))
                .child(notes),
        )
        .child(
            div()
                .px(px(16.))
                .py(px(10.))
                .flex()
                .justify_end()
                .gap(px(8.))
                .border_t_1()
                .border_color(palette.hairline)
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(
                    div()
                        .id("update-later")
                        .h(px(24.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(palette.control)
                        .shadow(vec![
                            shadow(palette.line_strong, 0., 0., 0.5),
                            shadow(palette.card_drop, 1., 1., 0.),
                        ])
                        .hover(|style| style.bg(palette.control_hover))
                        .active(|style| style.opacity(0.8))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_update_card(window, cx)),
                        )
                        .child("Later"),
                )
                .child(
                    div()
                        .id("update-take")
                        .h(px(24.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
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
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_update_card(window, cx);
                            match &page {
                                Some(page) => cx.open_url(page),
                                None => this.restart_to_update(cx),
                            }
                        }))
                        .child(action),
                ),
        )
}

/// GitHub's Markdown at the card's text size. The version headings are no
/// larger than the text, so they stay below the card's title.
fn notes_style(cx: &Context<Chrome>) -> MarkupStyle {
    let mut style = MarkupStyle::github(theme::get(cx).dark);
    style.text_size = px(13.);
    style.heading_scale[2] = 1.;
    style.block_gap = px(8.);
    style.heading_gap = px(12.);
    style
}
