//! Folds content open and shut by animating its height, measured as it lays
//! out, so it works for content of any height. Content that changes height
//! while open eases to its new height too.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ElementId, Entity, IntoElement, Pixels, Window, div, ease_out_quint,
    prelude::*, px,
};

/// How long folding open or shut takes.
pub const DURATION: Duration = Duration::from_millis(150);

struct State {
    open: bool,
    /// The content's height when last laid out.
    natural: Option<Pixels>,
    /// The height the current animation started from, and when.
    from: Pixels,
    start: Option<Instant>,
}

impl State {
    fn target(&self) -> Pixels {
        if self.open {
            self.natural.unwrap_or(self.from)
        } else {
            px(0.)
        }
    }

    /// The animation's eased progress, while it runs.
    fn progress(&self, now: Instant) -> Option<f32> {
        let elapsed = now.saturating_duration_since(self.start?);
        let linear = elapsed.as_secs_f32() / DURATION.as_secs_f32();
        (linear < 1.).then(|| ease_out_quint()(linear))
    }

    fn height(&self, now: Instant) -> Pixels {
        match self.progress(now) {
            Some(delta) => self.from + (self.target() - self.from) * delta,
            None => self.target(),
        }
    }

    /// Starts easing from the height shown now to the current target.
    fn restart(&mut self, from: Pixels, now: Instant) {
        self.from = from;
        self.start = (!crate::platform::reduce_motion()).then_some(now);
    }
}

/// `content` shown while `open`, sliding open and shut. With `enter`, content
/// shown open the first time unfolds from nothing, rather than appearing at
/// full height. `content` is built only while some of it shows.
pub fn fold(
    id: impl Into<ElementId>,
    open: bool,
    enter: bool,
    content: impl FnOnce() -> AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let now = Instant::now();
    let state: Entity<State> = window.use_keyed_state(id, cx, |_, _| State {
        open: open && !enter,
        natural: None,
        from: px(0.),
        start: None,
    });
    state.update(cx, |state, _| {
        if state.open != open {
            let from = state.height(now);
            state.open = open;
            state.restart(from, now);
        }
    });
    let shown = state.read(cx);
    let progress = shown.progress(now);
    if !open && progress.is_none() {
        return div().into_any_element();
    }
    let clip = progress.map(|_| {
        let height = shown.height(now);
        let full = shown.from.max(shown.target());
        // Fades with the fold; a resize of open content stays opaque.
        let opacity = if shown.from > px(0.) && shown.target() > px(0.) {
            1.
        } else if full > px(0.) {
            height / full
        } else {
            0.
        };
        (height, opacity)
    });
    if clip.is_some() {
        window.request_animation_frame();
    }
    div()
        .flex()
        .flex_col()
        .when_some(clip, |wrapper, (height, opacity)| {
            wrapper.h(height).overflow_hidden().opacity(opacity)
        })
        .on_children_prepainted(move |bounds, window, cx| {
            let Some(height) = bounds.first().map(|bounds| bounds.size.height) else {
                return;
            };
            state.update(cx, |state, _| {
                if state.natural == Some(height) {
                    return;
                }
                let now = Instant::now();
                let shown = state.height(now);
                let resized = state.natural.is_some() && state.open;
                state.natural = Some(height);
                if resized {
                    state.restart(shown, now);
                }
                window.request_animation_frame();
            });
        })
        .child(div().flex_none().child(content()))
        .into_any_element()
}
