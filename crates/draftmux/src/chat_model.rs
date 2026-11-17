//! The chat's model control: shows the model and thinking setting the
//! agent resolved for the draft's session, switches the model with a
//! [`ModelPicker`] and the thinking setting with a menu: pi's thinking
//! level, or Claude Code's effort.
//!
//! What it shows always comes from the agent: the owner passes the state of
//! each new agent process ([`Event::Ready`](draftmux_agent::Event)), and a
//! switch shows the state the agent reports after it. pi records a switch in
//! the session, and a Claude draft's descriptor keeps it, so the draft keeps
//! the model and thinking setting across restarts. For Claude, the model a
//! run reported using is shown beside the one chosen.

use std::sync::Arc;
use std::time::{Duration, Instant};

use draftmux_agent::{Agent, Error, ErrorKind, Model, State};
use draftmux_pi_profiles::ModelRef;
use futures::future::BoxFuture;
use gpui::{
    Anchor, Context, Entity, FocusHandle, Focusable, MouseButton, SharedString, Subscription,
    Transformation, Window, anchored, deferred, div, percentage, point, prelude::*, px, svg,
};

use crate::assets::CHEVRON_ICON;
use crate::menu::{MenuItem, menu_panel};
use crate::model_picker::{ModelPicker, PickerEvent};
use crate::profiles::ModelList;
use crate::theme;

/// The kind of agent the label is for.
pub enum Backend {
    /// A pi agent on the profile with this name.
    Pi {
        profile_name: SharedString,
    },
    Claude,
}

pub struct ChatModel {
    agent: Arc<dyn Agent>,
    backend: Backend,
    state: Option<State>,
    picker: Option<(Entity<ModelPicker>, Subscription)>,
    /// The composer, focused again when the picker closes.
    composer: FocusHandle,
    /// A model or thinking switch is under way.
    switching: bool,
    /// The thinking settings the model takes, read again with each state;
    /// none until read or when reading failed.
    levels: Option<Vec<String>>,
    /// The thinking menu is open, or opens once `levels` is read.
    menu: bool,
    /// When a press outside closed the menu. A press on its button closes
    /// the menu before the button sees it, and mustn't open it again.
    menu_closed: Option<Instant>,
    error: Option<SharedString>,
}

impl ChatModel {
    pub fn new(agent: Arc<dyn Agent>, backend: Backend, composer: FocusHandle) -> Self {
        Self {
            agent,
            backend,
            state: None,
            picker: None,
            composer,
            switching: false,
            levels: None,
            menu: false,
            menu_closed: None,
            error: None,
        }
    }

    /// Shows a state the agent reported, such as the one in `Event::Ready`.
    pub fn set_state(&mut self, state: State, cx: &mut Context<Self>) {
        self.state = Some(state);
        self.load_levels(cx);
        cx.notify();
    }

    fn show(&mut self, state: Result<State, String>, cx: &mut Context<Self>) {
        match state {
            Ok(state) => {
                self.state = Some(state);
                self.error = None;
                self.load_levels(cx);
            }
            Err(err) => self.error = Some(err.into()),
        }
        cx.notify();
    }

    /// Reads the thinking settings of the model the state shows. A menu
    /// waiting for them opens once read.
    fn load_levels(&mut self, cx: &mut Context<Self>) {
        let levels = self.agent.thinking_levels();
        cx.spawn(async move |this, cx| {
            let levels = levels.await;
            this.update(cx, |this, cx| {
                match levels {
                    Ok(levels) => this.levels = Some(levels),
                    Err(err) => {
                        this.levels = None;
                        if this.menu {
                            this.menu = false;
                            this.error = Some(format!("Couldn’t list the levels: {err}").into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn current(&self) -> Option<ModelRef> {
        let model = self.state.as_ref()?.model.as_ref()?;
        Some(ModelRef::new(&model.provider, &model.id))
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() || self.switching || !self.agent.capabilities().model_switching {
            return;
        }
        let current = self.current();
        let profile_name: SharedString = match &self.backend {
            Backend::Pi { profile_name } => profile_name.clone(),
            Backend::Claude => "Claude Code".into(),
        };
        let picker = cx.new(|cx| {
            let mut picker = ModelPicker::new(profile_name, ModelList::Loading, cx);
            picker.set_current(current, cx);
            picker
        });
        let subscription =
            cx.subscribe_in(&picker, window, |this, _, event, window, cx| match event {
                PickerEvent::Picked(model) => {
                    this.close(window, cx);
                    this.switch(model.clone(), cx);
                }
                PickerEvent::Dismissed => this.close(window, cx),
                PickerEvent::Retry => this.load_models(cx),
            });
        window.focus(&picker.focus_handle(cx), cx);
        self.picker = Some((picker, subscription));
        self.load_models(cx);
        cx.notify();
    }

    /// The draft's own agent lists the models: that is the list a switch
    /// accepts, including providers from pi's project config.
    fn load_models(&mut self, cx: &mut Context<Self>) {
        let Some((picker, _)) = &self.picker else {
            return;
        };
        let picker = picker.downgrade();
        picker
            .update(cx, |picker, cx| picker.set_models(ModelList::Loading, cx))
            .ok();
        let models = self.agent.available_models();
        cx.spawn(async move |_, cx| {
            let list = match models.await {
                Ok(models) => ModelList::Loaded(
                    models
                        .iter()
                        .map(|model| ModelRef::new(&model.provider, &model.id))
                        .collect(),
                ),
                Err(err) => ModelList::Failed(err.to_string().into()),
            };
            picker
                .update(cx, |picker, cx| picker.set_models(list, cx))
                .ok();
        })
        .detach();
    }

    /// A click outside closes the picker after the click has moved focus,
    /// so the composer gets it back only if the picker still has it.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((picker, _)) = self.picker.take()
            && picker.focus_handle(cx).is_focused(window)
        {
            window.focus(&self.composer, cx);
        }
        cx.notify();
    }

    fn switch(&mut self, model: ModelRef, cx: &mut Context<Self>) {
        if self.current().as_ref() == Some(&model) {
            return;
        }
        let switch = self.agent.switch_model(Model {
            provider: model.provider,
            id: model.id,
        });
        self.apply(switch, "Couldn’t switch model", cx);
    }

    fn open_menu(&mut self, cx: &mut Context<Self>) {
        let just_closed = self
            .menu_closed
            .take()
            .is_some_and(|closed| closed.elapsed() < Duration::from_millis(100));
        if just_closed || self.switching || self.picker.is_some() {
            return;
        }
        self.menu = !self.menu;
        if self.menu && self.levels.is_none() {
            self.load_levels(cx);
        }
        cx.notify();
    }

    fn set_thinking(&mut self, level: Option<String>, cx: &mut Context<Self>) {
        self.menu = false;
        if self
            .state
            .as_ref()
            .is_some_and(|state| state.thinking == level)
        {
            cx.notify();
            return;
        }
        let set = self.agent.set_thinking(level);
        let what = match self.backend {
            Backend::Pi { .. } => "Couldn’t change the thinking level",
            Backend::Claude => "Couldn’t change the effort",
        };
        self.apply(set, what, cx);
    }

    /// Shows the state a switch resolves to, or the error after `what`.
    fn apply(
        &mut self,
        switch: BoxFuture<'static, Result<State, Error>>,
        what: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.switching = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let state = switch.await;
            this.update(cx, |this, cx| {
                this.switching = false;
                match state {
                    // The agent stopped after switching; the next
                    // process's `Ready` state shows the change.
                    Err(err) if err.kind == ErrorKind::Stopped => cx.notify(),
                    Ok(mut state) => {
                        // A thinking switch keeps the model, so the model
                        // the last run used still applies.
                        if let Some(old) =
                            this.state.as_ref().filter(|old| old.model == state.model)
                        {
                            state.running = state.running.or_else(|| old.running.clone());
                        }
                        this.show(Ok(state), cx)
                    }
                    Err(err) => this.show(Err(format!("{what}: {err}")), cx),
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The model, and for Claude the model a run used when it has another
    /// name; the model is what gets cut short when the row is narrow.
    fn label(&self) -> (SharedString, Option<SharedString>) {
        let prefix = match self.backend {
            Backend::Pi { .. } => "π",
            Backend::Claude => "Claude",
        };
        let Some(state) = &self.state else {
            return (format!("{prefix} …").into(), None);
        };
        match (&self.backend, &state.model) {
            (Backend::Pi { .. }, None) => ("π no model".into(), None),
            // The tooltip names the provider.
            (Backend::Pi { .. }, Some(model)) => (format!("π {}", model.id).into(), None),
            (Backend::Claude, None) => match &state.running {
                Some(running) => (format!("Claude {running}").into(), None),
                None => ("Claude default model".into(), None),
            },
            (Backend::Claude, Some(model)) => (
                format!("Claude {}", model.id).into(),
                state
                    .running
                    .as_ref()
                    .filter(|running| **running != model.id)
                    .map(|running| running.clone().into()),
            ),
        }
    }

    fn tooltip(&self) -> SharedString {
        let Some(state) = &self.state else {
            return "Click to switch the model".into();
        };
        match (&self.backend, &state.model) {
            (Backend::Pi { .. }, Some(model)) => format!(
                "{}/{}. Click to switch the model.",
                model.provider, model.id
            )
            .into(),
            (Backend::Pi { .. }, None) => "Click to switch the model".into(),
            (Backend::Claude, model) => {
                let chosen = match model {
                    Some(model) => format!("Set to {}", model.id),
                    None => "Continues on the session's own model".into(),
                };
                let ran = match &state.running {
                    Some(running) => format!("; its last run used {running}"),
                    None => String::new(),
                };
                format!("{chosen}{ran}. Click to switch the model.").into()
            }
        }
    }
}

impl ChatModel {
    /// The thinking setting as the footer shows it, after the model; none
    /// before the agent reported a state with a model.
    fn thinking(&self) -> Option<SharedString> {
        let state = self.state.as_ref()?;
        match self.backend {
            Backend::Pi { .. } => {
                state.model.as_ref()?;
                state.thinking.clone().map(Into::into)
            }
            Backend::Claude => Some(state.thinking.as_deref().unwrap_or(AUTO).to_owned().into()),
        }
    }

    /// Whether the model takes a thinking setting but the one shown: pi
    /// lists only `off` for a model that doesn't think, and Claude no
    /// efforts for a model without them. Unknown counts as changeable, so
    /// a click reads the levels.
    fn changeable(&self) -> bool {
        self.levels
            .as_ref()
            .is_none_or(|levels| match self.backend {
                Backend::Pi { .. } => levels.len() > 1,
                Backend::Claude => !levels.is_empty(),
            })
    }

    fn thinking_tooltip(&self, thinking: &str) -> SharedString {
        let auto = self
            .state
            .as_ref()
            .is_some_and(|state| state.thinking.is_none());
        match (&self.backend, self.changeable()) {
            (Backend::Pi { .. }, false) => "The model doesn’t think.".into(),
            (Backend::Claude, false) => "The model has no effort setting.".into(),
            (Backend::Pi { .. }, true) => {
                format!("Thinking level {thinking}. Click to change it for this draft.").into()
            }
            (Backend::Claude, true) if auto => {
                "Claude Code picks the effort for the model. Click to change it for this draft."
                    .into()
            }
            (Backend::Claude, true) => {
                format!("Effort {thinking}. Click to change it for this draft.").into()
            }
        }
    }

    fn menu_items(&self) -> Vec<MenuItem<Option<String>>> {
        let current = self.state.as_ref().and_then(|state| state.thinking.clone());
        let heading = match self.backend {
            Backend::Pi { .. } => "Thinking",
            Backend::Claude => "Effort",
        };
        let item =
            |label: SharedString, level: Option<String>, detail: Option<&'static str>| MenuItem {
                label,
                checked: current == level,
                enabled: true,
                action: Some(level),
                detail: detail.map(Into::into),
                heading: false,
            };
        let mut items = vec![MenuItem {
            label: heading.into(),
            checked: false,
            enabled: false,
            action: None,
            detail: None,
            heading: true,
        }];
        if self.agent.capabilities().default_thinking {
            items.push(item(AUTO.into(), None, Some("Model default")));
        }
        items.extend(
            self.levels
                .iter()
                .flatten()
                .map(|level| item(level.clone().into(), Some(level.clone()), None)),
        );
        items
    }
}

/// What the footer shows when the backend picks the thinking setting.
const AUTO: &str = "auto";

impl Render for ChatModel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *theme::palette(cx);
        let (model, running) = self.label();
        let thinking = self.thinking();
        let menu_open = self.menu && self.levels.is_some();
        div()
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .text_size(px(11.))
            .text_color(palette.muted)
            .child(
                div()
                    .id("chat-model")
                    .min_w_0()
                    .flex()
                    .cursor_pointer()
                    .tooltip(crate::tooltip::text(self.tooltip()))
                    .when(self.switching, |label| label.opacity(0.5))
                    .when(!self.switching, |label| {
                        label.active(|style| style.opacity(0.8))
                    })
                    .child(div().min_w_0().truncate().child(model))
                    .when_some(running, |label, running| {
                        // Shrinks with the model, so the footer's hints
                        // beside the label never cover it.
                        label.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(format!(" · {running}"))),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.menu = false;
                            this.open(window, cx)
                        }),
                    ),
            )
            .when_some(thinking, |row, thinking| {
                let changeable = self.changeable();
                let tooltip = self.thinking_tooltip(&thinking);
                row.child(div().flex_none().ml(px(-4.)).mr(px(-6.)).child("·"))
                    .child(
                        div()
                            .id("chat-thinking")
                            .relative()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .h(px(18.))
                            .px(px(4.))
                            .rounded(px(5.))
                            .when(!menu_open, |button| {
                                button.tooltip(crate::tooltip::text(tooltip))
                            })
                            .child(thinking)
                            .when(self.switching, |button| button.opacity(0.5))
                            .when(!changeable, |button| button.text_color(palette.faint))
                            .when(changeable, |button| {
                                button
                                    .child(
                                        svg()
                                            .path(CHEVRON_ICON)
                                            .size(px(8.))
                                            .text_color(palette.muted)
                                            .with_transformation(Transformation::rotate(
                                                percentage(0.75),
                                            )),
                                    )
                                    .cursor_pointer()
                                    .hover(|style| style.bg(palette.hover))
                                    .when(menu_open, |button| button.bg(palette.hover))
                                    .active(|style| style.opacity(0.8))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.open_menu(cx)
                                        }),
                                    )
                            })
                            .when(menu_open, |button| {
                                let pick = cx.entity().downgrade();
                                let dismiss = cx.entity().downgrade();
                                button.child(
                                    div()
                                        .absolute()
                                        .bottom_full()
                                        .left_0()
                                        .mb_1()
                                        .child(deferred(
                                            anchored()
                                                .anchor(Anchor::BottomLeft)
                                                .snap_to_window_with_margin(px(8.))
                                                .child(menu_panel(
                                                    &palette,
                                                    &self.menu_items(),
                                                    move |level, _, cx| {
                                                        pick.update(cx, |this, cx| {
                                                            this.set_thinking(level, cx)
                                                        })
                                                        .ok();
                                                    },
                                                    move |_, cx| {
                                                        dismiss
                                                            .update(cx, |this, cx| {
                                                                this.menu = false;
                                                                this.menu_closed =
                                                                    Some(Instant::now());
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                    },
                                                )),
                                        )),
                                )
                            }),
                    )
            })
            .when_some(self.error.clone(), |row, error| {
                row.child(div().truncate().text_color(palette.danger).child(error))
            })
            .when_some(self.picker.as_ref(), |row, (picker, _)| {
                row.child(deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .offset(point(px(-8.), px(-6.)))
                        .child(picker.clone()),
                ))
            })
    }
}
