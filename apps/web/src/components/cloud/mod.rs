//! Environments on the T3 Connect account that are not saved here yet.
//! Ports `CloudEnvironmentConnectList.tsx` (`CloudEnvironmentConnectRows`):
//! a status dot, the label, the relay's view of it, and "Add".

use gpui_kit::{AnyElement, Context, Entity, IntoElement, SharedString, div, prelude::*};
use t3code_client_runtime::connection::catalog::TargetRoute;
use t3code_client_runtime::relay::CloudAccountState;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::connection_status_dot::ConnectionStatusDot;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::settings::connections_settings::ConnectionsSettings;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::{ConnectionRuntime, ConnectionsState, RelayAvailability};
use crate::i18n::t;

/// The rows, and whether there were any (the page shows its empty state otherwise).
pub struct CloudRows {
    pub rows: Vec<AnyElement>,
    pub any: bool,
}

/// One row per account environment that has no T3 Connect route here.
pub fn cloud_environment_connect_rows(state: &Entity<ConnectionsState>, cx: &mut Context<ConnectionsSettings>) -> CloudRows {
    let snapshot = state.read(cx);
    let Some(cloud) = &snapshot.cloud else { return CloudRows { rows: Vec::new(), any: false } };
    if !matches!(cloud.account, CloudAccountState::SignedIn(_)) {
        return CloudRows { rows: Vec::new(), any: false };
    }
    let theme = cx.app_theme();
    let c = &theme.colors;
    let mut rows = Vec::new();
    if let Some(error) = cloud.discovery.error.clone().filter(|_| cloud.discovery.environments.is_empty()) {
        rows.push(
            div()
                .px(theme.sp(4.))
                .py(theme.sp(3.))
                .child(
                    div()
                        .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                        .text_color(c.destructive())
                        .child(t!("Could not load T3 Connect environments")),
                )
                .child(div().mt(theme.sp(1.)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(error))
                .child(
                    div().mt(theme.sp(3.)).child(
                        Button::new("relay-retry")
                            .label(t!("Try again"))
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .on_click(|_, _, cx| {
                                if let Some(runtime) = cx.try_global::<ConnectionRuntime>() {
                                    runtime.refresh_relay(cx);
                                }
                            }),
                    ),
                )
                .into_any_element(),
        );
        return CloudRows { rows, any: true };
    }
    for relay in &cloud.discovery.environments {
        let record = &relay.record;
        let saved = snapshot.environment(&record.environment_id);
        let has_relay = saved.is_some_and(|env| env.entry.routes.iter().any(|r| r.target.route == TargetRoute::Relay));
        if has_relay {
            continue;
        }
        let saved_without_relay = saved.is_some();
        let not_added = if saved_without_relay { t!("Saved without T3 Connect") } else { t!("Not added") };
        let (relay_text, dot) = match relay.availability {
            RelayAvailability::Online => (t!("Relay online"), c_success(cx)),
            RelayAvailability::Offline => (t!("Relay offline"), None),
            RelayAvailability::Checking => (t!("Checking relay status…"), Some(c.warning)),
            RelayAvailability::Error => (t!("Relay status unavailable"), Some(c.destructive())),
        };
        let status = SharedString::from(format!("T3 Connect · {not_added} · {relay_text}"));
        let key = record.environment_id.as_str().to_owned();
        let record_for_add = record.clone();
        rows.push(
            div()
                .id(SharedString::from(format!("relay-row-{key}")))
                .flex()
                .items_center()
                .gap(theme.sp(3.))
                .px(theme.sp(4.))
                .py(theme.sp(2.5))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.sp(2.))
                                .child(ConnectionStatusDot::new(dot, Some(relay_text.clone()), SharedString::from(format!("relay-dot-{key}"))))
                                .child(Icon::new(IconName::Server).size(IconSize::Md).color(c.muted_foreground))
                                .child(div().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(SharedString::from(record.label.clone()))),
                        )
                        .child(
                            div()
                                .mt(theme.sp(1.))
                                .truncate()
                                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                .text_color(c.muted_foreground)
                                .child(status),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("relay-add-{key}")))
                        .label(if saved_without_relay { t!("Add route") } else { t!("Add", context = "verb, adds an environment") })
                        .size(ButtonSize::Sm)
                        .on_click(move |_, _, cx| {
                            let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
                            let label = record_for_add.label.clone();
                            let task = runtime.add_relay_environment(record_for_add.clone(), cx);
                            cx.spawn(async move |cx| {
                                let result = task.await;
                                cx.update(|cx| match result {
                                    Ok(()) => Toaster::push(
                                        if saved_without_relay {
                                            ToastData::new(ToastKind::Success, t!("T3 Connect route added")).description(t!(
                                                "{environment} falls back to T3 Connect when its other routes are unreachable.",
                                                environment = label
                                            ))
                                        } else {
                                            ToastData::new(ToastKind::Success, t!("Environment added")).description(t!(
                                                "Connecting to {environment} through T3 Connect.",
                                                environment = label
                                            ))
                                        },
                                        cx,
                                    ),
                                    Err(error) => Toaster::push(
                                        ToastData::new(ToastKind::Error, t!("Could not connect environment")).description(error.0),
                                        cx,
                                    ),
                                });
                            })
                            .detach();
                        }),
                )
                .into_any_element(),
        );
    }
    let any = !rows.is_empty();
    CloudRows { rows, any }
}

fn c_success(cx: &gpui_kit::App) -> Option<gpui_kit::Hsla> {
    Some(cx.app_theme().fixed.success)
}
