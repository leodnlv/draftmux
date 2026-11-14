//! Hover labels for icon-only buttons, shown after gpui's hover delay.

use std::time::{Duration, Instant};

use gpui::{AnyView, App, Context, SharedString, Window, div, ease_out_quint, prelude::*, px};

use crate::chrome::shadow;
use crate::theme::{self, hex};

/// How long a tooltip takes to fade in.
const FADE_IN: Duration = Duration::from_millis(120);

pub struct Tooltip {
    text: SharedString,
    shown: Instant,
}

/// A tooltip builder for `.tooltip(...)` showing `text`.
pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        cx.new(|_| Tooltip {
            text: text.clone(),
            shown: Instant::now(),
        })
        .into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fade = self.shown.elapsed().as_secs_f32() / FADE_IN.as_secs_f32();
        if fade < 1. {
            window.request_animation_frame();
        }
        // gpui places the tooltip at the pointer; the margin clears the cursor.
        div()
            .pl(px(8.))
            .pt(px(16.))
            .when(fade < 1., |tooltip| tooltip.opacity(ease_out_quint()(fade)))
            .child(card(self.text.clone(), cx))
    }
}

/// Longer text wraps in a card [`WRAP_WIDTH`] wide: gpui lays a tooltip out
/// at its narrowest, and moves one that would cross the window's right edge
/// to end at the pointer, so a long single line lands far from it.
const WRAP_CHARS: usize = 56;
const WRAP_WIDTH: f32 = 360.;

/// A tooltip's card showing `text`, for hover labels gpui doesn't place.
pub fn card(text: SharedString, cx: &App) -> gpui::Div {
    let palette = theme::palette(cx);
    let wrap = text.chars().count() > WRAP_CHARS;
    div()
        .px(px(7.))
        .py(px(3.))
        .rounded(px(5.))
        .bg(palette.menu)
        .text_color(palette.text)
        .text_size(px(12.))
        .line_height(px(15.))
        .when(wrap, |card| card.w(px(WRAP_WIDTH)))
        .when(!wrap, |card| card.whitespace_nowrap())
        .shadow(vec![
            shadow(hex(0x00000033), 0., 0., 0.5),
            shadow(hex(0x00000026), 2., 8., 0.),
        ])
        .child(text)
}
