//! The Environments page. Ports `ConnectionsSettings view="environments"`,
//! `SavedEnvironmentRows.tsx` (`SavedBackendListRow`), `EnvironmentRow.tsx`,
//! `EnvironmentRoutesList.tsx` and `ConnectionDiagnosisDetails.tsx`. The T3
//! Connect sign-in opens from the settings nav footer and the empty state.
//!
//! Rows read [`ConnectionsState`]; every change goes through
//! [`ConnectionRuntime`] and comes back as new state. Without a runtime
//! (scenes) the page renders its state and commands do nothing.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use gpui_kit::{
    TestSupportExt as _,
    AnyElement, App, ClipboardItem, Context, Entity, IntoElement, Render, SharedString, Subscription, Task, WeakEntity,
    Window, div, prelude::*, px,
};
use t3code_client_runtime::connection::routes::{is_learned, route_id};
use t3code_client_runtime::environment::EnvironmentId;

use super::add_environment_dialog::{AddEnvironmentDialog, AddEnvironmentEvent};
use super::settings_layout::{SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::clerk::{T3ConnectSignIn, T3ConnectSignInEvent};
use crate::components::cloud::cloud_environment_connect_rows;
use crate::components::connection_status_dot::ConnectionStatusDot;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::empty::Empty;
use crate::components::ui::menu::{MenuItem, MenuPopup, dropdown_menu};
use crate::components::ui::switch::{Switch, SwitchSize};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::connection::presentation::{
    Diagnosis, HopResult, Tone, diagnose, diagnosis_text, present_phase, route_address,
    route_label, row_status, status_text, transport_label,
};
use crate::connection::{ConnectionRuntime, ConnectionsState, EnvironmentState};
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// A dialog the page has open over itself.
enum PageDialog {
    Add(Entity<AddEnvironmentDialog>),
    SignIn(Entity<T3ConnectSignIn>),
    RemoveEnvironment { id: EnvironmentId, label: SharedString },
    RemoveRoute { id: EnvironmentId, route_id: String, label: SharedString, address: Option<SharedString> },
}

/// A route being dragged to a new place in its environment's list.
#[derive(Clone)]
struct RouteDrag {
    environment_id: EnvironmentId,
    route_id: String,
    label: SharedString,
}

/// What a dragged route looks like under the pointer.
struct RouteDragPreview(SharedString);

impl Render for RouteDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        div()
            .px(theme.sp(2.))
            .py(theme.sp(1.))
            .rounded(theme.radii.md)
            .bg(theme.colors.background())
            .shadow(theme.shadows.md5.clone())
            .ui_text(theme, TextSize::Xs, weight::MEDIUM)
            .child(self.0.clone())
    }
}

/// The Environments page.
pub struct ConnectionsSettings {
    state: Entity<ConnectionsState>,
    routes_open: HashSet<EnvironmentId>,
    diagnosis_open: HashSet<EnvironmentId>,
    removing: HashSet<EnvironmentId>,
    dialog: Option<PageDialog>,
    /// Ticks once a second while a row shows a retry countdown.
    ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionsSettings {
    /// The page over the running app's state.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = ConnectionRuntime::state(cx);
        Self::with_state(state, window, cx)
    }

    /// The page over given state: the app's, or a scene's fixture.
    pub fn with_state(state: Entity<ConnectionsState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        if let Some(runtime) = cx.try_global::<ConnectionRuntime>() {
            runtime.refresh_relay(cx);
        }
        Self {
            state,
            routes_open: HashSet::new(),
            diagnosis_open: HashSet::new(),
            removing: HashSet::new(),
            dialog: None,
            ticker: None,
            _subscriptions: vec![observe],
        }
    }

    pub fn state(&self) -> &Entity<ConnectionsState> {
        &self.state
    }

    /// Opens one row's routes and diagnosis details (scenes and checks).
    pub fn toggle_for_scene(&mut self, id: &str, routes: bool, details: bool, cx: &mut Context<Self>) {
        let id = EnvironmentId::from(id);
        if routes {
            self.routes_open.insert(id.clone());
        }
        if details {
            self.diagnosis_open.insert(id);
        }
        cx.notify();
    }

    /// Opens "Add environment", or "Add route" when `route_target` is set.
    pub fn open_add_dialog(&mut self, route_target: Option<(EnvironmentId, SharedString)>, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| AddEnvironmentDialog::new(route_target, window, cx));
        let subscription = cx.subscribe(&dialog, |this, _, event: &AddEnvironmentEvent, cx| match event {
            AddEnvironmentEvent::Close => {
                this.dialog = None;
                cx.notify();
            }
        });
        self._subscriptions.push(subscription);
        self.dialog = Some(PageDialog::Add(dialog));
        cx.notify();
    }

    /// Opens the T3 Connect email-code sign-in.
    pub fn open_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| T3ConnectSignIn::new(window, cx));
        let subscription = cx.subscribe(&dialog, |this, _, event: &T3ConnectSignInEvent, cx| match event {
            T3ConnectSignInEvent::Close => {
                this.dialog = None;
                cx.notify();
            }
            T3ConnectSignInEvent::SignedIn => {
                this.dialog = None;
                if let Some(runtime) = cx.try_global::<ConnectionRuntime>() {
                    runtime.refresh_relay(cx);
                }
                cx.notify();
            }
        });
        self._subscriptions.push(subscription);
        self.dialog = Some(PageDialog::SignIn(dialog));
        cx.notify();
    }

    /// The dialog that is open, for scripted checks.
    pub fn add_dialog(&self) -> Option<&Entity<AddEnvironmentDialog>> {
        match &self.dialog {
            Some(PageDialog::Add(dialog)) => Some(dialog),
            _ => None,
        }
    }

    pub fn sign_in_dialog(&self) -> Option<&Entity<T3ConnectSignIn>> {
        match &self.dialog {
            Some(PageDialog::SignIn(dialog)) => Some(dialog),
            _ => None,
        }
    }

    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        cx.notify();
    }

    fn set_enabled(&mut self, id: EnvironmentId, enabled: bool, cx: &mut Context<Self>) {
        let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
        let task = runtime.set_enabled(id, enabled, cx);
        cx.spawn(async move |_, cx| {
            if let Err(error) = task.await {
                cx.update(|cx| {
                    let title = if enabled { t!("Could not switch backend on") } else { t!("Could not switch backend off") };
                    Toaster::push(ToastData::new(ToastKind::Error, title).description(error.0), cx);
                });
            }
        })
        .detach();
    }

    fn remove_environment(&mut self, id: EnvironmentId, cx: &mut Context<Self>) {
        self.dialog = None;
        let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
        self.removing.insert(id.clone());
        let task = runtime.remove_environment(id.clone(), cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.removing.remove(&id);
                this.routes_open.remove(&id);
                if let Err(error) = result {
                    Toaster::push(ToastData::new(ToastKind::Error, t!("Could not remove backend")).description(error.0), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn remove_route(&mut self, id: EnvironmentId, route: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
        let task = runtime.remove_route(id, route, cx);
        cx.spawn(async move |_, cx| {
            if let Err(error) = task.await {
                cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, "Remove route").description(error.0), cx));
            }
        })
        .detach();
        cx.notify();
    }

    /// Moves `dragged` to `target`'s place in its environment's route list.
    fn reorder(&mut self, dragged: &RouteDrag, target: &str, cx: &mut Context<Self>) {
        if dragged.route_id == target {
            return;
        }
        let Some(env) = self.state.read(cx).environment(&dragged.environment_id) else { return };
        let mut order: Vec<String> = env.entry.routes.iter().map(|r| route_id(&r.target).to_owned()).collect();
        let (Some(from), Some(to)) = (order.iter().position(|id| *id == dragged.route_id), order.iter().position(|id| id == target))
        else {
            return;
        };
        let moved = order.remove(from);
        order.insert(to, moved);
        let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
        let task = runtime.reorder_routes(dragged.environment_id.clone(), order, cx);
        cx.spawn(async move |_, cx| {
            if let Err(error) = task.await {
                cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, "Reorder routes").description(error.0), cx));
            }
        })
        .detach();
    }

    fn retry(&self, id: &EnvironmentId, cx: &App) {
        if let Some(runtime) = cx.try_global::<ConnectionRuntime>() {
            runtime.retry(id);
        }
    }

    /// Keeps a one-second ticker alive while any row counts down to a retry.
    fn sync_ticker(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now();
        let counting = self.state.read(cx).environments.iter().any(|env| {
            env.status.as_ref().and_then(|s| s.retry_at).is_some_and(|at| at > now)
        });
        if counting && self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                }
            }));
        } else if !counting {
            self.ticker = None;
        }
    }

    fn render_row(&self, env: &EnvironmentState, cx: &mut Context<Self>) -> AnyElement {
        let diagnosis = diagnose(env);
        let mut detail: Vec<AnyElement> = Vec::new();
        if let (Some(diagnosis), true) = (&diagnosis, self.diagnosis_open.contains(env.id())) {
            detail.push(self.render_diagnosis(env, diagnosis, cx));
        }
        if self.routes_open.contains(env.id()) {
            detail.push(self.render_routes(env, cx));
        }
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let id = env.id().clone();
        let key = id.as_str().to_owned();
        let enabled = env.entry.enabled;
        let phase = present_phase(env);
        let (status, tone) = row_status(env);
        let routes_open = self.routes_open.contains(&id);
        let diagnosis_open = self.diagnosis_open.contains(&id);
        let removing = self.removing.contains(&id);
        let route_count = env.entry.routes.len();
        let mut subtitle: Vec<AnyElement> = Vec::new();
        let mut parts = vec![transport_label(env)];
        if diagnosis.is_none() {
            parts.push(status);
        }
        let error_tone = enabled && tone == Tone::Error && diagnosis.is_none();
        subtitle.push(
            div()
                .id(SharedString::from(format!("env-status-{key}")))
                .min_w_0()
                .truncate()
                .when(error_tone, |d| d.text_color(c.destructive()))
                .tooltip(tooltip(status_text(env)))
                .child(SharedString::from(parts.join(" · ")))
                .into_any_element(),
        );
        subtitle.push(div().flex_none().child("·").into_any_element());
        let routes_label =
            if route_count == 1 { t!("Routes") } else { SharedString::from(format!("{route_count} routes")) };
        let toggle_id = id.clone();
        subtitle.push(
            div()
                .id(SharedString::from(format!("env-routes-toggle-{key}")))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(2.))
                .cursor_pointer()
                .text_color(c.muted_foreground)
                .hover(|s| s.text_color(c.foreground()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.routes_open.remove(&toggle_id) {
                        this.routes_open.insert(toggle_id.clone());
                    }
                    cx.notify();
                }))
                .child(routes_label)
                .child(chevron(routes_open, c.muted_foreground))
                .test_support()
                .into_any_element(),
        );
        let dot = ConnectionStatusDot::for_phase(phase, Some(status_text(env)), SharedString::from(format!("env-dot-{key}")));
        let identity = div()
            .flex()
            .flex_col()
            .min_w_0()
            .when(!enabled, |d| d.opacity(0.6))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(1.5))
                    .child(dot)
                    .child(
                        div()
                            .truncate()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(c.foreground())
                            .child(env.entry.label.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(1.))
                    .min_w_0()
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(c.muted_foreground)
                    .children(subtitle),
            );
        let identity = identity.into_any_element();
        let below = diagnosis.as_ref().map(|diagnosis| {
            let status = env.status.as_ref();
            let retry_at = status.and_then(|s| s.retry_at);
            let attempt = status.map_or(0, |s| s.attempt);
            let toggle_id = id.clone();
            let mut meta: Vec<AnyElement> = Vec::new();
            if let Some(at) = retry_at {
                let seconds = at.duration_since(SystemTime::now()).map(|d| d.as_secs_f64().ceil() as u64).unwrap_or(0);
                meta.push(
                    div()
                        .child(if seconds > 0 { t!("retrying in {seconds}s", seconds = seconds) } else { t!("retrying") })
                        .into_any_element(),
                );
            }
            if retry_at.is_some() && attempt > 0 {
                meta.push(div().child("·").into_any_element());
            }
            if attempt > 0 {
                meta.push(div().child(crate::i18n::t_rich!("attempt {attempt}", attempt = attempt).plain()).into_any_element());
            }
            if retry_at.is_some() || attempt > 0 {
                meta.push(div().child("·").into_any_element());
            }
            meta.push(
                div()
                    .id(SharedString::from(format!("env-details-{key}")))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .cursor_pointer()
                    .hover(|s| s.text_color(c.foreground()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.diagnosis_open.remove(&toggle_id) {
                            this.diagnosis_open.insert(toggle_id.clone());
                        }
                        cx.notify();
                    }))
                    .child(t!("Details"))
                    .child(chevron(diagnosis_open, c.muted_foreground))
                    .into_any_element(),
            );
            div()
                .flex()
                .flex_col()
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .child(div().text_color(c.destructive()).child(diagnosis.headline.clone()))
                .child(div().flex().flex_wrap().items_center().gap(sp(1.)).text_color(c.muted_foreground).children(meta))
        });
        let switch_id = id.clone();
        let menu_entity = cx.entity().downgrade();
        let menu_env = env.clone();
        let trace_id = env.status.as_ref().and_then(|s| s.last_failure.as_ref()).and_then(|f| f.trace_id.clone());
        let controls = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(sp(1.))
            .child(
                div()
                    .id(SharedString::from(format!("env-switch-wrap-{key}")))
                    .tooltip(tooltip(if enabled { t!("Switch off") } else { t!("Switch on") }))
                    .child(
                        Switch::new(SharedString::from(format!("env-switch-{key}")), enabled)
                            .size(SwitchSize::Sm)
                            .disabled(removing)
                            .on_change({
                                let this = cx.entity().downgrade();
                                move |checked, _, cx| {
                                    this.update(cx, |this, cx| this.set_enabled(switch_id.clone(), checked, cx)).ok();
                                }
                            }),
                    ),
            )
            .child(dropdown_menu(
                SharedString::from(format!("env-menu-{key}")),
                Button::new(SharedString::from(format!("env-more-{key}")))
                    .variant(ButtonVariant::GhostMuted)
                    .size(ButtonSize::IconXs)
                    .icon(IconName::Ellipsis)
                    .disabled(removing),
                move |_, _| environment_menu(menu_entity.clone(), &menu_env, routes_open, trace_id.clone()),
            ));
        div()
            .id(SharedString::from(format!("env-row-{key}")))
            .flex()
            .flex_col()
            .px(sp(4.))
            .py(sp(2.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .child(
                        Icon::new(IconName::Server)
                            .size(IconSize::Md)
                            .color(if enabled { c.muted_foreground } else { with_alpha(c.muted_foreground, 0.6) }),
                    )
                    .child(div().flex().flex_col().flex_1().min_w_0().child(identity).children(below))
                    .child(controls),
            )
            .when(!detail.is_empty(), |d| d.child(div().pl(sp(7.)).children(detail)))
            .into_any_element()
    }

    fn render_diagnosis(&self, env: &EnvironmentState, diagnosis: &Diagnosis, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let key = env.id().as_str().to_owned();
        let retry_id = env.id().clone();
        let report = diagnosis_text(&env.entry.label, diagnosis);
        div()
            .mt(sp(2.))
            .pt(sp(2.))
            .border_t_1()
            .border_color(with_alpha(c.border, 0.5))
            .flex()
            .flex_col()
            .gap(sp(3.))
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .child(div().flex().flex_col().gap(px(2.)).children(diagnosis.hops.iter().map(|(label, result)| {
                let (mark, note) = match result {
                    HopResult::Ok => ("✓", SharedString::default()),
                    HopResult::Failed => ("✕", t!("Failed", context = "connection hop status")),
                    HopResult::Unchecked => ("–", t!("Not checked")),
                };
                div()
                    .flex()
                    .gap(sp(2.))
                    .text_color(if *result == HopResult::Failed { c.destructive() } else { c.muted_foreground })
                    .child(div().w(sp(3.)).flex().justify_center().child(mark))
                    .child(div().when(*result == HopResult::Ok, |d| d.text_color(c.foreground())).child(label.clone()))
                    .child(note)
            })))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(weight::MEDIUM).text_color(c.foreground()).child(t!("What to try")))
                    .children(diagnosis.steps.iter().enumerate().map(|(ix, step)| {
                        div()
                            .flex()
                            .gap(sp(1.))
                            .text_color(c.muted_foreground)
                            .child(format!("{}.", ix + 1))
                            .child(div().flex_1().child(step.clone()))
                    })),
            )
            .child(div().flex().flex_col().text_color(c.muted_foreground).children(diagnosis.facts.iter().map(|(label, value)| {
                div()
                    .flex()
                    .gap(sp(3.))
                    .font(crate::appearance_fonts::mono_font(weight::NORMAL))
                    .child(div().flex_none().child(label.clone()))
                    .child(div().flex_1().min_w_0().text_color(c.foreground()).child(value.clone()))
            })))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(sp(2.))
                    .child(
                        Button::new(SharedString::from(format!("env-retry-{key}")))
                            .label(t!("Retry now"))
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Xs)
                            .on_click(cx.listener(move |this, _, _, cx| this.retry(&retry_id, cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("env-copy-diagnostics-{key}")))
                            .label(t!("Copy diagnostics"))
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Xs)
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(report.clone()))),
                    ),
            )
            .into_any_element()
    }

    fn render_routes(&self, env: &EnvironmentState, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let routes = env.entry.routes.clone();
        let active = env.status.as_ref().filter(|_| env.is_connected()).and_then(|s| s.active_route_id.clone());
        let refused: Vec<String> = env.status.as_ref().map(|s| s.refused_route_ids.to_vec()).unwrap_or_default();
        let key = env.id().as_str().to_owned();
        let drop_bg = with_alpha(c.border, 0.4);
        let add_target = (env.id().clone(), SharedString::from(env.entry.label.clone()));
        let rows = routes.iter().enumerate().map(|(ix, route)| {
            let id = route_id(&route.target).to_owned();
            let label = route_label(route);
            let address = route_address(route);
            let in_use = active.as_deref() == Some(id.as_str());
            let is_refused = refused.contains(&id);
            let removable = routes.len() > 1 && !is_learned(route);
            let drag = RouteDrag { environment_id: env.id().clone(), route_id: id.clone(), label: label.clone() };
            let drop_target = id.clone();
            let remove = (env.id().clone(), id.clone(), label.clone(), address.clone());
            div()
                .id(SharedString::from(format!("route-{key}-{ix}")))
                .flex()
                .items_center()
                .gap(sp(2.))
                .py(sp(2.))
                .drag_over::<RouteDrag>(move |style, _, _, _| style.bg(drop_bg))
                .on_drop(cx.listener(move |this, dragged: &RouteDrag, _, cx| this.reorder(dragged, &drop_target, cx)))
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
                                .gap(sp(1.5))
                                .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                                .text_color(c.foreground())
                                .child(label.clone())
                                .when(in_use, |d| {
                                    d.child(
                                        div()
                                            .px(sp(1.))
                                            .rounded(theme.radii.sm)
                                            .bg(with_alpha(theme.fixed.success, 0.12))
                                            .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                                            .text_color(theme.fixed.success_foreground)
                                            .child(t!("In use")),
                                    )
                                })
                                .when(is_refused, |d| {
                                    d.child(
                                        div()
                                            .px(sp(1.))
                                            .rounded(theme.radii.sm)
                                            .bg(with_alpha(c.destructive(), 0.08))
                                            .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                                            .text_color(c.destructive_foreground())
                                            .child(t!("Sign-in refused")),
                                    )
                                }),
                        )
                        .children(address.clone().map(|address| {
                            let text = if is_learned(route) {
                                SharedString::from(format!("{address} · {}", t!("found automatically")))
                            } else {
                                address
                            };
                            div()
                                .truncate()
                                .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                                .text_color(c.muted_foreground)
                                .child(text)
                        }))
                        .when(is_refused && address.is_some(), |d| {
                            d.child(
                                div()
                                    .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                                    .text_color(c.muted_foreground)
                                    .child(t!("The host refused this device's sign-in. Pair this address again with Add route.")),
                            )
                        }),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("route-grip-{key}-{ix}")))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(sp(6.))
                        .rounded(theme.radii.sm)
                        .cursor_grab()
                        .text_color(with_alpha(c.muted_foreground, 0.7))
                        .hover(|s| s.text_color(c.foreground()))
                        .on_drag(drag, |drag, _, _, cx| cx.new(|_| RouteDragPreview(drag.label.clone())))
                        .child(Icon::new(IconName::GripVertical).size(IconSize::Sm)),
                )
                .when(removable, |d| {
                    d.child(
                        Button::new(SharedString::from(format!("route-remove-{key}-{ix}")))
                            .variant(ButtonVariant::GhostMuted)
                            .size(ButtonSize::IconXs)
                            .icon(IconName::X)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let (id, route_id, label, address) = remove.clone();
                                this.dialog = Some(PageDialog::RemoveRoute { id, route_id, label, address });
                                cx.notify();
                            })),
                    )
                })
        });
        div()
            .mt(sp(2.))
            .py(sp(2.))
            .border_t_1()
            .border_color(with_alpha(c.border, 0.5))
            .child(div().flex().flex_col().children(rows))
            .child(
                div().flex().ml(-sp(1.5)).pt(sp(1.)).child(
                    Button::new(SharedString::from(format!("route-add-{key}")))
                        .label(t!("Add route"))
                        .icon(IconName::Plus)
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::Xs)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_add_dialog(Some(add_target.clone()), window, cx)
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.as_ref()?;
        let on_close = {
            let this = cx.entity().downgrade();
            move |_: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| this.close_dialog(cx)).ok();
            }
        };
        let content: AnyElement = match dialog {
            PageDialog::Add(dialog) => dialog.clone().into_any_element(),
            PageDialog::SignIn(dialog) => dialog.clone().into_any_element(),
            PageDialog::RemoveEnvironment { id, label, .. } => {
                let id = id.clone();
                let message = t!(
                    "Remove {environment} from this device?\nThis forgets its pairing, credentials, and cached threads here. Switch it off instead to keep it saved.",
                    environment = label
                );
                let (title, description) = message.split_once('\n').unwrap_or((message.as_ref(), ""));
                DialogSurface::new(SharedString::from(title.to_owned()))
                    .description(SharedString::from(description.to_owned()))
                    .footer(
                        Button::new("confirm-cancel")
                            .label(t!("Cancel"))
                            .variant(ButtonVariant::Outline)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .footer(
                        Button::new("confirm-remove-environment")
                            .label(t!("Remove"))
                            .variant(ButtonVariant::Destructive)
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_environment(id.clone(), cx))),
                    )
                    .into_any_element()
            }
            PageDialog::RemoveRoute { id, route_id, label, address } => {
                let (id, route_id) = (id.clone(), route_id.clone());
                let text = match address {
                    Some(address) => t!("Remove {route} route?\n{address}", route = label, address = address),
                    None => t!("Remove {route} route?", route = label),
                };
                let mut lines = text.split('\n');
                let title = SharedString::from(lines.next().unwrap_or_default().to_owned());
                let rest = lines.collect::<Vec<_>>().join("\n");
                let mut surface = DialogSurface::new(title);
                if !rest.is_empty() {
                    surface = surface.description(rest);
                }
                surface
                    .footer(
                        Button::new("confirm-cancel")
                            .label(t!("Cancel"))
                            .variant(ButtonVariant::Outline)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .footer(
                        Button::new("confirm-remove-route")
                            .label(t!("Remove route"))
                            .variant(ButtonVariant::Destructive)
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_route(id.clone(), route_id.clone(), cx))),
                    )
                    .into_any_element()
            }
        };
        Some(Modal::dialog("environments-dialog", content, on_close).into_any_element())
    }
}

fn chevron(open: bool, color: gpui_kit::Hsla) -> impl IntoElement {
    Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Xs).color(color)
}

/// `environmentMcpUrl`: `/mcp` on the first route with an HTTP address.
/// The web prefers the relay's address and the connected route; this app
/// reads the saved routes in order.
fn environment_mcp_url(env: &EnvironmentState) -> Option<String> {
    env.entry.routes.iter().find_map(|route| {
        let mut url = route.bearer_profile()?.http_base_url.clone();
        url.set_path("/mcp");
        url.set_query(None);
        url.set_fragment(None);
        Some(url.to_string())
    })
}

/// The row's "More actions" menu.
fn environment_menu(
    this: WeakEntity<ConnectionsSettings>,
    env: &EnvironmentState,
    routes_open: bool,
    trace_id: Option<String>,
) -> MenuPopup {
    let id = env.id().clone();
    let label = SharedString::from(env.entry.label.clone());
    let mcp_url = environment_mcp_url(env);
    let mut menu = MenuPopup::new()
        .item(MenuItem::new(if routes_open { t!("Hide routes") } else { t!("Routes") }).icon(IconName::Route).on_select({
            let (this, id) = (this.clone(), id.clone());
            move |_, cx| {
                this.update(cx, |this, cx| {
                    if !this.routes_open.remove(&id) {
                        this.routes_open.insert(id.clone());
                    }
                    cx.notify();
                })
                .ok();
            }
        }));
    if let Some(url) = mcp_url {
        menu = menu.item(MenuItem::new(t!("Copy MCP URL")).on_select(move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
        }));
    }
    if let Some(trace_id) = trace_id {
        menu = menu.item(MenuItem::new(t!("Copy trace ID")).on_select(move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(trace_id.clone()));
        }));
    }
    menu.separator().item(MenuItem::new(t!("Remove from this device…")).destructive().on_select(move |_, cx| {
        this.update(cx, |this, cx| {
            this.dialog = Some(PageDialog::RemoveEnvironment { id: id.clone(), label: label.clone() });
            cx.notify();
        })
        .ok();
    }))
}

impl Render for ConnectionsSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_ticker(cx);
        let environments: Vec<EnvironmentState> = self.state.read(cx).environments.clone();
        let rows: Vec<AnyElement> = environments.iter().map(|env| self.render_row(env, cx)).collect();
        let cloud_rows = cloud_environment_connect_rows(&self.state, cx);
        let dialog = self.render_dialog(cx);
        let theme = cx.app_theme();
        let c = &theme.colors;
        let header_action = Button::new("add-environment")
            .label(t!("Add environment"))
            .icon(IconName::Plus)
            .variant(ButtonVariant::GhostMuted)
            .size(ButtonSize::Xs)
            .on_click(cx.listener(|this, _, window, cx| this.open_add_dialog(None, window, cx)));
        let mut section = SettingsSection::new(t!("Environments")).header_action(header_action).children(rows).children(cloud_rows.rows);
        if environments.is_empty() && !cloud_rows.any {
            let description = t!("Click “Add environment” to pair another environment.");
            section = section.child(
                div()
                    .min_h(theme.rem(13.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Empty::new(t!("No saved remote environments")).icon(IconName::ChevronsLeftRightEllipsis).description(description)),
            );
        }
        page_container(cx)
            .text_color(c.foreground())
            .child(section)
            .children(dialog)
    }
}
