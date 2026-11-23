//! "Add environment" and "Add route". Ports `AddEnvironmentDialog.tsx`: a
//! backend host plus pairing code, where pasting a pairing URL (direct,
//! hosted, or a `t3code://pair` link) into the host field fills both. SSH
//! is not offered: this app has no SSH launcher.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use t3code_client_runtime::connection::onboarding::{parse_host_and_code, parse_pairing_url};
use t3code_client_runtime::environment::EnvironmentId;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::dialog_glass;
use crate::components::ui::input::TextInput;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::ConnectionRuntime;
use crate::i18n::t;
use crate::theme_color::with_alpha;

#[derive(Clone, Debug)]
pub enum AddEnvironmentEvent {
    Close,
}

/// The dialog. `route_target` is the environment a new route joins.
pub struct AddEnvironmentDialog {
    route_target: Option<(EnvironmentId, SharedString)>,
    host: Entity<InputState>,
    code: Entity<InputState>,
    error: Option<SharedString>,
    adding: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AddEnvironmentEvent> for AddEnvironmentDialog {}

impl AddEnvironmentDialog {
    pub fn new(route_target: Option<(EnvironmentId, SharedString)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = cx.new(|cx| InputState::new(window, cx).placeholder("backend.example.com"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder(t!("PAIRCODE")));
        let code_for_paste = code.clone();
        let on_host = cx.subscribe_in(&host, window, move |this, state, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                // A full pairing link fills both fields, as on the web.
                let value = state.read(cx).value();
                if let Ok(request) = parse_pairing_url(&value) {
                    let host = request.http_base_url.to_string();
                    let host = host.trim_end_matches('/').to_owned();
                    state.update(cx, |state, cx| state.set_value(host, window, cx));
                    code_for_paste.update(cx, |state, cx| state.set_value(request.credential, window, cx));
                }
                this.error = None;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.submit(cx),
            _ => {}
        });
        let on_code = cx.subscribe(&code, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(cx);
            }
        });
        host.update(cx, |state, cx| state.focus(window, cx));
        Self { route_target, host, code, error: None, adding: false, _subscriptions: vec![on_host, on_code] }
    }

    /// The host and code fields, for scripted checks.
    pub fn fields(&self) -> (&Entity<InputState>, &Entity<InputState>) {
        (&self.host, &self.code)
    }

    /// Pairs: checks the address first, then spends the code.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.adding {
            return;
        }
        let host = self.host.read(cx).value();
        let code = self.code.read(cx).value();
        let request = match parse_host_and_code(&host, &code) {
            Ok(request) => request,
            Err(error) => {
                let message = SharedString::from(error.to_string());
                Toaster::push(ToastData::new(ToastKind::Error, t!("Could not add backend")).description(message.clone()), cx);
                self.error = Some(message);
                cx.notify();
                return;
            }
        };
        let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
        let expected = self.route_target.as_ref().map(|(id, _)| id.clone());
        let route_label = self.route_target.as_ref().map(|(_, label)| label.clone());
        let task = runtime.pair(request, expected, cx);
        self.adding = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.adding = false;
                match result {
                    Ok(_) => {
                        let toast = match &route_label {
                            Some(environment) => ToastData::new(ToastKind::Success, t!("Route added"))
                                .description(t!("{environment} now has another way to connect.", environment = environment)),
                            None => ToastData::new(ToastKind::Success, t!("Backend added"))
                                .description(t!("The environment is saved and will reconnect on app startup.")),
                        };
                        Toaster::push(toast, cx);
                        cx.emit(AddEnvironmentEvent::Close);
                    }
                    Err(error) => {
                        Toaster::push(
                            ToastData::new(ToastKind::Error, t!("Could not add backend")).description(error.0.clone()),
                            cx,
                        );
                        this.error = Some(error.0);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn field_label(text: SharedString, cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    div()
        .mb(theme.sp(1.5))
        .ui_text(theme, TextSize::Xs, weight::MEDIUM)
        .text_color(theme.colors.foreground())
        .child(text)
}

impl Render for AddEnvironmentDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let (title, description) = match &self.route_target {
            Some((_, environment)) => (
                t!("Add a route to {environment}", environment = environment),
                t!("Pair this machine again over another address, such as its Tailscale name. It joins the existing routes instead of adding a second machine."),
            ),
            None => (t!("Add Environment"), t!("Pair another environment to this client.")),
        };
        let mode_card = div()
            .flex()
            .items_start()
            .gap(sp(3.))
            .min_h(sp(24.))
            .p(sp(4.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(with_alpha(c.primary(), 0.5))
            .bg(with_alpha(c.primary(), 0.05))
            .child(
                div()
                    .mt(px(2.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(sp(8.))
                    .rounded(theme.radii.md)
                    .border_1()
                    .border_color(with_alpha(c.primary(), 0.3))
                    .bg(with_alpha(c.primary(), 0.1))
                    .child(Icon::new(IconName::ChevronsLeftRightEllipsis).size(IconSize::Md).color(c.primary())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(t!("Remote link")))
                    .child(
                        div()
                            .mt(sp(1.))
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .child(t!("Enter a backend host and pairing code.")),
                    ),
            );
        let fields = div()
            .flex()
            .gap(sp(3.))
            .child(
                div()
                    .id("add-environment-host")
                    .flex_1()
                    .min_w_0()
                    .child(field_label(t!("Host"), cx))
                    .child(TextInput::new(&self.host).disabled(self.adding)),
            )
            .child(
                div()
                    .id("add-environment-code")
                    .w(theme.rem(10.))
                    .flex_none()
                    .child(field_label(t!("Pairing code"), cx))
                    .child(TextInput::new(&self.code).disabled(self.adding)),
            );
        let submit_label = if self.adding {
            t!("Adding…")
        } else if self.route_target.is_some() {
            t!("Add route")
        } else {
            t!("Add environment")
        };
        dialog_glass(theme)
            .id("add-environment-dialog")
            .w_full()
            .max_w(theme.rem(48.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.))
                    .p(sp(6.))
                    .pb(sp(3.))
                    .child(
                        div()
                            .ui_text(theme, TextSize::Xl, weight::SEMIBOLD)
                            .line_height(px(TextSize::Xl.size(theme.fonts.interface) * 1.375))
                            .child(title),
                    )
                    .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(description)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(4.))
                    .px(sp(6.))
                    .pt(sp(1.))
                    .pb(sp(6.))
                    .child(div().flex().gap(sp(3.)).child(div().flex_1().child(mode_card)).child(div().flex_1()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(3.))
                            .child(fields)
                            .child(
                                div()
                                    .mt(sp(1.))
                                    .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                                    .text_color(c.muted_foreground)
                                    .child(t!("Paste a full pairing URL here to fill both fields automatically.")),
                            ),
                    )
                    .children(self.error.clone().map(|error| {
                        div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(error)
                    }))
                    .child(
                        div().flex().w_full().child(
                            Button::new("add-environment-submit")
                                .label(submit_label)
                                .icon(IconName::Plus)
                                .variant(ButtonVariant::Outline)
                                .disabled(self.adding)
                                .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                        ),
                    ),
            )
            .child(
                div().absolute().top(sp(2.)).right(sp(2.)).child(
                    Button::new("add-environment-close")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Icon)
                        .icon(IconName::X)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(AddEnvironmentEvent::Close))),
                ),
            )
    }
}
