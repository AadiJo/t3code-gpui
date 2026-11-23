//! The main pane before any environment is saved. Ports
//! `HostedEmptyState.tsx` in its T3 Connect variant: sign in, run
//! `t3 connect` on the computer, then add it from the list; plus the
//! pairing variant's "Add pairing link" for direct and Tailscale hosts.

use gpui_kit::{App, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use t3code_client_runtime::relay::CloudAccountState;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, mono_font, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::connection::{ConnectionsState, RelayAvailability};
use crate::i18n::t;

/// What the empty state asks the app to open.
#[derive(Clone, Debug)]
pub enum HostedEmptyStateEvent {
    OpenEnvironments,
    SignIn,
}

/// The empty main pane.
pub struct HostedEmptyState {
    state: Entity<ConnectionsState>,
    header_left: gpui_kit::Pixels,
    header_height: gpui_kit::Pixels,
    _observe: Subscription,
}

impl EventEmitter<HostedEmptyStateEvent> for HostedEmptyState {}

impl HostedEmptyState {
    pub fn new(state: Entity<ConnectionsState>, header_height: gpui_kit::Pixels, header_left: gpui_kit::Pixels, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self { state, header_left, header_height, _observe: observe }
    }
}

fn step(number: u8, content: impl IntoElement, cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    div()
        .flex()
        .gap(theme.sp(3.))
        .child(div().w(theme.sp(2.)).flex_none().pt(theme.sp(1.)).text_color(theme.colors.muted_foreground).child(number.to_string()))
        .child(div().flex_1().min_w_0().child(content))
}

fn command_block(command: &'static str, cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    div()
        .mt(theme.sp(2.))
        .px(theme.sp(3.))
        .py(theme.sp(2.))
        .rounded(theme.radii.md)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.derived.inline_code)
        .font(mono_font(weight::NORMAL))
        .text_size(gpui_kit::px(12.))
        .child(command)
}

impl Render for HostedEmptyState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let account = state.cloud.as_ref().map(|cloud| cloud.account.clone());
        let waiting = state
            .cloud
            .as_ref()
            .is_none_or(|cloud| cloud.discovery.environments.iter().all(|env| env.availability == RelayAvailability::Checking));
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let signed_in = matches!(account, Some(CloudAccountState::SignedIn(_)));
        let sign_in: gpui_kit::AnyElement = match &account {
            Some(CloudAccountState::SignedIn(_)) => div()
                .flex()
                .items_center()
                .gap(sp(1.5))
                .text_color(c.muted_foreground)
                .child(Icon::new(IconName::Check).size(IconSize::Md))
                .child(t!("Signed in"))
                .into_any_element(),
            _ => Button::new("empty-sign-in")
                .label(t!("Sign in"))
                .size(ButtonSize::Sm)
                .disabled(matches!(account, Some(CloudAccountState::Loading)))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(HostedEmptyStateEvent::SignIn)))
                .into_any_element(),
        };
        let run = div()
            .flex()
            .flex_col()
            .child(t!("Run on your computer:"))
            .child(command_block("t3 connect", cx))
            .when(signed_in && waiting, |d| {
                d.child(
                    div()
                        .mt(sp(4.))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(t!("Waiting for your computer…")),
                )
            })
            .when(signed_in && !waiting, |d| {
                d.child(
                    div().mt(sp(4.)).child(
                        Button::new("empty-open-environments")
                            .label(t!("Open Environments"))
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HostedEmptyStateEvent::OpenEnvironments))),
                    ),
                )
            });
        let steps = div()
            .flex()
            .flex_col()
            .gap(sp(4.))
            .w_full()
            .text_left()
            .ui_text(theme, TextSize::Sm, weight::NORMAL)
            .child(step(1, sign_in, cx))
            .child(step(2, run, cx));
        div()
            .id("hosted-empty-state")
            .flex()
            .flex_col()
            .size_full()
            .bg(c.background())
            .text_color(c.foreground())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(self.header_height)
                    .pl(self.header_left + sp(4.))
                    .border_b_1()
                    .border_color(c.border)
                    .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(theme.derived.empty_state_text).child("T3 Code")),
            )
            .child(
                div().flex_1().flex().items_center().justify_center().p(sp(6.)).child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(sp(6.))
                        .max_w(theme.rem(24.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(sp(2.))
                                .child(div().ui_text(theme, TextSize::Xl, weight::SEMIBOLD).child(t!("No computer connected")))
                                .child(
                                    div()
                                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                                        .text_color(c.muted_foreground)
                                        .child(t!("Your agents run on your computer.")),
                                ),
                        )
                        .child(steps)
                        .child(
                            Button::new("empty-add-pairing")
                                .label(t!("Add pairing link"))
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Sm)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(HostedEmptyStateEvent::OpenEnvironments))),
                        ),
                ),
            )
    }
}

impl HostedEmptyState {
    /// The label for scenes and checks.
    pub fn title() -> SharedString {
        t!("No computer connected")
    }
}
