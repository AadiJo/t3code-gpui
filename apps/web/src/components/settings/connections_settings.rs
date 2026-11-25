//! The Environments page. Ports `ConnectionsSettings view="environments"`,
//! `SavedEnvironmentRows.tsx` (`SavedBackendListRow`), `EnvironmentRow.tsx`,
//! `EnvironmentRoutesList.tsx` and `ConnectionDiagnosisDetails.tsx`. The T3
//! Connect sign-in opens from the settings nav footer and the empty state.
//!
//! Rows read [`ConnectionsState`]; every change goes through
//! [`ConnectionRuntime`] and comes back as new state. Without a runtime
//! (scenes) the page renders its state and commands do nothing.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gpui_kit::{
    TestSupportExt as _,
    AnyElement, App, ClipboardItem, Context, Entity, IntoElement, Render, SharedString, Subscription, Task, WeakEntity,
    Window, div, prelude::*, px,
};
use t3code_client_runtime::connection::catalog::TargetRoute;
use t3code_client_runtime::connection::routes::{is_learned, route_id};
use t3code_client_runtime::environment::EnvironmentId;

use super::add_environment_dialog::{AddEnvironmentDialog, AddEnvironmentEvent};
use super::settings_layout::{SettingsSection, page_container};
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::clerk::{T3ConnectSignIn, T3ConnectSignInEvent};
use crate::components::cloud::cloud_environment_connect_rows;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::empty::Empty;
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup};
use crate::components::ui::popover::{Popover, PopupHandle};
use crate::components::ui::switch::{Switch, SwitchSize};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::connection::presentation::{
    Diagnosis, HopResult, Tone, diagnose, diagnosis_text, present_phase, route_address,
    route_label, row_status, status_text, transport_label,
};
use crate::components::server_update_action::{
    ServerUpdateStore, ServerUpdateTarget, UpdateAppearance, confirm_copy, desktop_confirmation, outdated_server_update_action,
    server_update_action, server_update_progress, server_updates_action, update_all,
};
use crate::hooks::use_settings::ServerSettingsStore;
use crate::version_skew::{VersionMismatch, client_version, resolve_version_mismatch};
use t3code_client_runtime::connection::server_update::{ServerUpdateStage, ServerUpdateState};
use crate::connection::{ConnectionRuntime, ConnectionsState, EnvironmentState};
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// A dialog the page has open over itself.
enum PageDialog {
    Add(Entity<AddEnvironmentDialog>),
    SignIn(Entity<T3ConnectSignIn>),
    RemoveEnvironment { id: EnvironmentId, label: SharedString },
    RemoveRoute { id: EnvironmentId, route_id: String, label: SharedString, address: Option<SharedString> },
    /// Confirms relaunching desktop apps that run these servers (`requestConfirmDialog`).
    ConfirmUpdate(Vec<ServerUpdateTarget>),
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
    /// Rows whose "Permissions" is open, with what the session check found.
    permissions: HashMap<EnvironmentId, super::connections_settings_access::SessionCheck>,
    /// The Load balancing fold is open.
    load_balancing_open: bool,
    /// The Pull request sharing fold is open.
    pull_request_routing_open: bool,
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
        let updates = ServerUpdateStore::entity(cx);
        let observe_updates = cx.observe(&updates, |_, _, cx| cx.notify());
        if let Some(runtime) = cx.try_global::<ConnectionRuntime>() {
            runtime.refresh_relay(cx);
        }
        Self {
            state,
            routes_open: HashSet::new(),
            diagnosis_open: HashSet::new(),
            removing: HashSet::new(),
            permissions: HashMap::new(),
            load_balancing_open: false,
            pull_request_routing_open: false,
            dialog: None,
            ticker: None,
            _subscriptions: vec![observe, observe_updates],
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
            // The runtime lists the account's environments on every sign-in.
            T3ConnectSignInEvent::SignedIn => {
                this.dialog = None;
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

    /// `LoadBalancingSettings`: a folded card with the on/off switch and a
    /// preference per environment, shown with two or more enabled
    /// environments. The choices are client settings on this device.
    fn render_load_balancing(&self, environments: &[EnvironmentState], cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::hooks::use_settings::ClientSettingsStore;
        // `preferences`: Prefer, Normal, Less often, Manual only.
        const WEIGHTS: [u64; 4] = [100, 50, 25, 0];
        let preference = |weight: Option<u64>| match weight {
            None | Some(50) => 50,
            Some(0) => 0,
            Some(w) if w < 50 => 25,
            Some(_) => 100,
        };
        let environments: Vec<&EnvironmentState> =
            environments.iter().filter(|env| env.entry.enabled && present_phase(env) != crate::connection::presentation::PresentedPhase::Unsupported).collect();
        if environments.len() < 2 {
            return None;
        }
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let settings = ClientSettingsStore::get(cx);
        let enabled = settings.load_balancing_enabled;
        let weights = settings.load_balancing_weights.clone();
        let summary: SharedString = if enabled {
            let parts: Vec<String> = environments
                .iter()
                .filter_map(|env| {
                    let label = env.entry.label.clone();
                    match preference(weights.get(env.id().as_str()).copied()) {
                        100 => Some(t!("{environment} prefer", environment = label).to_string()),
                        25 => Some(t!("{environment} less often", environment = label).to_string()),
                        0 => Some(t!("{environment} manual only", environment = label).to_string()),
                        _ => None,
                    }
                })
                .collect();
            SharedString::from(parts.join(" · "))
        } else {
            t!("Off", context = "load balancing is turned off")
        };
        let open = self.load_balancing_open;
        let labels = vec![t!("Prefer"), t!("Normal", context = "load balancing preference"), t!("Less often"), t!("Manual only")];
        let divider = with_alpha(c.border, 0.5);
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(sp(4.))
            .px(sp(4.))
            .child(
                div()
                    .id("load-balancing-toggle")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .min_h(sp(11.))
                    .items_center()
                    .gap(sp(2.))
                    .py(sp(2.))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.load_balancing_open = !this.load_balancing_open;
                        cx.notify();
                    }))
                    .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Custom(sp(4.))).color(c.muted_foreground))
                    .child(div().flex_none().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(setting_title("load-balancing")))
                    .when(!summary.is_empty(), |d| {
                        d.child(div().min_w_0().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(summary))
                    }),
            )
            .child(
                div().ml_auto().flex().items_center().py(sp(1.5)).child(
                    Switch::new("load-balancing-switch", enabled)
                        .on_change(|checked, _, cx| ClientSettingsStore::update(cx, |s| s.load_balancing_enabled = checked)),
                ),
            );
        let body = open.then(|| {
            let rows = environments.iter().map(|env| {
                let env_id = env.id().as_str().to_owned();
                let current = WEIGHTS.iter().position(|w| *w == preference(weights.get(&env_id).copied())).unwrap_or(1);
                let select = crate::components::ui::select::Select::new(SharedString::from(format!("load-weight-{env_id}")), labels.clone(), current, move |ix, _, cx| {
                    let key = env_id.clone();
                    ClientSettingsStore::update(cx, |s| {
                        s.load_balancing_weights.insert(key, WEIGHTS[ix]);
                    })
                })
                .size(crate::components::ui::select::SelectSize::Xs);
                div()
                    .border_t_1()
                    .border_color(divider)
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .px(sp(4.))
                    .py(sp(2.5))
                    .child(super::settings_controls::environment_machine_icon(
                        super::settings_controls::environment_machine_kind(env.id(), cx),
                        sp(4.),
                        c.muted_foreground,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(env.entry.label.clone()))
                            .child(div().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(transport_label(env))),
                    )
                    .child(div().min_w(sp(32.)).flex_none().child(select))
            });
            div()
                .border_t_1()
                .border_color(divider)
                .child(
                    div().px(sp(4.)).py(sp(2.5)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!(
                        "New threads in shared projects start on the machine with the most free CPU and memory, weighted by each machine's preference."
                    )),
                )
                .children(rows.collect::<Vec<_>>())
        });
        Some(
            div()
                .id("load-balancing")
                .rounded(theme.radii.xl)
                .border_1()
                .border_color(with_alpha(c.border, 0.6))
                .bg(with_alpha(c.card(), 0.4))
                .shadow(theme.shadows.xs5.clone())
                .text_color(c.foreground())
                .child(header)
                .children(body)
                .into_any_element(),
        )
    }

    /// Opens or closes the Pull request sharing fold.
    pub fn toggle_pull_request_routing(&mut self, cx: &mut Context<Self>) {
        self.pull_request_routing_open = !self.pull_request_routing_open;
        cx.notify();
    }

    /// Opens or closes a row's "Permissions"; opening asks the environment
    /// what this session may do.
    fn toggle_permissions(&mut self, id: EnvironmentId, cx: &mut Context<Self>) {
        use super::connections_settings_access::{SessionCheck, check_session};
        if self.permissions.remove(&id).is_some() {
            cx.notify();
            return;
        }
        let connected = self.state.read(cx).environment(&id).is_some_and(|env| present_phase(env) == crate::connection::presentation::PresentedPhase::Connected);
        let check = connected.then(|| check_session(&id, cx)).flatten();
        self.permissions.insert(id.clone(), if check.is_some() { SessionCheck::Checking } else { SessionCheck::Unavailable });
        if let Some(check) = check {
            cx.spawn(async move |this, cx| {
                let result = check.await;
                this.update(cx, |this, cx| {
                    if let Some(slot) = this.permissions.get_mut(&id) {
                        *slot = match result {
                            Ok(state) => SessionCheck::Done(Box::new(state)),
                            Err(_) => SessionCheck::Failed,
                        };
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
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
        let config = ServerSettingsStore::config(cx, env.id());
        let update_state = ServerUpdateStore::shown_state(env.id(), config.as_ref().map(|c| c.environment.server_version.as_str()), cx);
        let resuming = matches!(update_state, ServerUpdateState::Running { stage: ServerUpdateStage::Resuming, .. });
        // While the host restarts into the update, the failure is expected, not diagnosed.
        let diagnosis = if resuming { None } else { diagnose(env) };
        let mismatch = environment_mismatch(env, cx);
        let update_controls = self.render_update_controls(env, config.as_deref(), mismatch.as_ref(), &update_state, cx);
        let mut detail: Vec<AnyElement> = Vec::new();
        if let (Some(diagnosis), true) = (&diagnosis, self.diagnosis_open.contains(env.id())) {
            detail.push(self.render_diagnosis(env, diagnosis, cx));
        }
        if self.routes_open.contains(env.id()) {
            detail.push(self.render_routes(env, cx));
        }
        if let Some(session) = self.permissions.get(env.id()) {
            let theme = cx.app_theme();
            detail.push(
                div()
                    .mt(theme.sp(2.))
                    .border_t_1()
                    .border_color(with_alpha(theme.colors.border, 0.5))
                    .child(super::connections_settings_access::session_permissions(session, true, cx))
                    .into_any_element(),
            );
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
            parts.push(if resuming { t!("Restarting") } else { status });
        }
        if let (true, Some(mismatch)) = (enabled, &mismatch) {
            parts.push(mismatch.server_version.clone());
        }
        let status_tooltip: SharedString = match &mismatch {
            Some(mismatch) => format!(
                "{}\n{}",
                status_text(env),
                t!(
                    "Update available: {serverVersion} → {clientVersion}",
                    serverVersion = mismatch.server_version,
                    clientVersion = mismatch.client_version
                )
            )
            .into(),
            None => status_text(env),
        };
        let error_tone = enabled && tone == Tone::Error && !resuming && diagnosis.is_none();
        subtitle.push(
            div()
                .id(SharedString::from(format!("env-status-{key}")))
                .min_w_0()
                .truncate()
                .when(error_tone, |d| d.text_color(c.destructive()))
                .tooltip(tooltip(status_tooltip))
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
        subtitle.push(div().flex_none().child("·").into_any_element());
        let permissions_open = self.permissions.contains_key(&id);
        let permissions_id = id.clone();
        subtitle.push(
            div()
                .id(SharedString::from(format!("env-permissions-toggle-{key}")))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(2.))
                .cursor_pointer()
                .text_color(c.muted_foreground)
                .hover(|s| s.text_color(c.foreground()))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_permissions(permissions_id.clone(), cx)))
                .child(t!("Permissions"))
                .child(chevron(permissions_open, c.muted_foreground))
                .test_support()
                .into_any_element(),
        );
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
        let progress = div().max_w(theme.rem(28.)).children(server_update_progress(format!("env-update-progress-{key}"), &update_state, cx));
        let below = below.map(IntoElement::into_any_element).into_iter().chain(Some(progress.into_any_element()));
        let switch_id = id.clone();
        let menu_entity = cx.entity().downgrade();
        let menu_env = env.clone();
        let trace_id = env.status.as_ref().and_then(|s| s.last_failure.as_ref()).and_then(|f| f.trace_id.clone());
        let controls = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(sp(1.))
            .children(update_controls)
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
            .child(Popover::new(
                SharedString::from(format!("env-menu-{key}")),
                Button::new(SharedString::from(format!("env-more-{key}")))
                    .variant(ButtonVariant::GhostMuted)
                    .size(ButtonSize::IconXs)
                    .icon(IconName::Ellipsis)
                    .disabled(removing),
                move |handle, window, cx| {
                    // The "Icon" submenu (`EnvironmentIconMenu`) opens beside the menu on hover or click.
                    let icon_open = window.use_keyed_state(SharedString::from(format!("env-icon-open-{}", menu_env.id().as_str())), cx, |_, _| false);
                    let showing = *icon_open.read(cx);
                    let kind = super::settings_controls::environment_machine_kind(menu_env.id(), cx);
                    let opener = icon_open.clone();
                    let icon_item = MenuItem::new(t!("Icon"))
                        .icon(super::settings_controls::machine_icon_source(kind))
                        .kind(MenuItemKind::Submenu)
                        .on_hover(move |hovered, _, cx| {
                            if *hovered {
                                opener.update(cx, |open, cx| {
                                    *open = true;
                                    cx.notify();
                                });
                            }
                        });
                    let clicked = icon_open.clone();
                    let icon_item = icon_item.on_select(move |_, cx| clicked.update(cx, |open, _| *open = true));
                    let (dismiss_handle, keep) = (handle.clone(), icon_open.clone());
                    let menu = environment_menu(menu_entity.clone(), &menu_env, routes_open, trace_id.clone(), icon_item, icon_open.clone())
                        .on_dismiss(move |_, cx| {
                            if !*keep.read(cx) {
                                dismiss_handle.close(cx);
                            }
                        });
                    let theme = cx.app_theme();
                    let submenu = showing.then(|| {
                        // Base UI's sub popup: inline-end, aligned to its trigger row 5px up.
                        div().mt(px(1.) + theme.sp(1.) - px(5.)).child(environment_icon_menu(menu_env.id().clone(), handle.clone(), icon_open.clone(), cx))
                    });
                    div().flex().items_start().child(menu).children(submenu).into_any_element()
                },
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
                    // `EnvironmentRow`: the machine icon, dimmed with the identity when switched off.
                    .child(super::settings_controls::environment_machine_icon(
                        super::settings_controls::environment_machine_kind(&id, cx),
                        sp(4.),
                        if enabled { c.muted_foreground } else { with_alpha(c.muted_foreground, 0.6) },
                    ))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(identity).children(below))
                    .child(controls),
            )
            .when(!detail.is_empty(), |d| d.child(div().pl(sp(7.)).children(detail)))
            .into_any_element()
    }

    /// The row's Update control, before its switch: the bare-socket Update for a host on an older
    /// protocol, else the icon for a connected host behind this client while no update runs.
    fn render_update_controls(
        &self,
        env: &EnvironmentState,
        config: Option<&t3code_contracts::ServerConfig>,
        mismatch: Option<&VersionMismatch>,
        state: &ServerUpdateState,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key = env.id().as_str();
        let server_label = t!(
            "{label} server",
            label = env.entry.label.as_str(),
            context = "names a T3 server inside sentences such as {serverLabel} updated"
        );
        let retry = matches!(state, ServerUpdateState::Failed { .. });
        let label = if retry { t!("Retry update") } else { t!("Update", context = "verb, updates the server") };
        let blocked = env.status.as_ref().and_then(|status| status.last_failure.as_ref()).filter(|failure| {
            failure.server_update_required
                && failure.kind
                    == t3code_client_runtime::connection::model::ConnectionErrorKind::Blocked(
                        t3code_client_runtime::connection::model::BlockedReason::Unsupported,
                    )
        });
        if env.entry.enabled && blocked.is_some() && !state.is_running() {
            let from = env.server_version.as_ref().map(|v| v.to_string());
            return Some(outdated_server_update_action(
                format!("env-outdated-update-{key}"),
                env.id().clone(),
                server_label,
                from,
                client_version(cx),
                label,
                cx,
            ));
        }
        let mismatch = mismatch?;
        if !env.entry.enabled || !env.is_connected() || state.is_running() {
            return None;
        }
        let target = ServerUpdateTarget::for_config(env.id().clone(), server_label, config, mismatch.client_version.clone());
        let this = cx.entity().downgrade();
        let start = target.clone();
        Some(server_update_action(
            format!("env-update-{key}"),
            &target,
            Some(label),
            UpdateAppearance::Icon,
            move |_, cx| {
                this.update(cx, |this, cx| this.start_updates(vec![start.clone()], cx)).ok();
            },
            cx,
        ))
    }

    /// Starts updates, asking first when one relaunches a desktop app.
    fn start_updates(&mut self, targets: Vec<ServerUpdateTarget>, cx: &mut Context<Self>) {
        if targets.iter().any(ServerUpdateTarget::needs_confirmation) {
            self.dialog = Some(PageDialog::ConfirmUpdate(targets));
            cx.notify();
            return;
        }
        update_all(targets, cx);
    }

    /// Saved servers the header's "Update all" can update (`savedServerUpdateTargets`).
    fn update_targets(&self, environments: &[EnvironmentState], cx: &mut App) -> Vec<ServerUpdateTarget> {
        environments
            .iter()
            .filter(|env| env.entry.enabled && env.is_connected())
            .filter_map(|env| {
                let config = ServerSettingsStore::config(cx, env.id());
                let mismatch = environment_mismatch(env, cx)?;
                let state = ServerUpdateStore::state(env.id(), cx);
                let target = ServerUpdateTarget::for_config(
                    env.id().clone(),
                    SharedString::from(env.entry.label.clone()),
                    config.as_deref(),
                    mismatch.client_version,
                );
                (!state.is_running() && target.self_update.is_some() && target.updates_remotely()).then_some(target)
            })
            .collect()
    }

    fn render_diagnosis(&self, env: &EnvironmentState, diagnosis: &Diagnosis, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let key = env.id().as_str().to_owned();
        let retry_id = env.id().clone();
        let report = diagnosis_text(&env.entry.label, diagnosis);
        let relay_only = env.entry.routes.len() == 1 && env.entry.routes[0].target.route == TargetRoute::Relay;
        let add_route_target = (env.id().clone(), SharedString::from(env.entry.label.clone()));
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
                    }))
                    .children(diagnosis.more_info_url.clone().map(|url| {
                        div()
                            .id(SharedString::from(format!("env-more-info-{key}")))
                            .cursor_pointer()
                            .text_color(c.muted_foreground)
                            .underline()
                            .hover(|s| s.text_color(c.foreground()))
                            .on_click(move |_, _, cx| cx.open_url(&url))
                            .child(t!("More info"))
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
                    )
                    .when(relay_only, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("env-add-direct-route-{key}")))
                                .label(t!("Add direct route"))
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Xs)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_add_dialog(Some(add_route_target.clone()), window, cx)
                                })),
                        )
                    }),
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
            PageDialog::ConfirmUpdate(targets) => {
                let targets = targets.clone();
                let (title, description) = confirm_copy(&desktop_confirmation(&targets));
                let surface = DialogSurface::new(title);
                let surface = match description {
                    Some(description) => surface.description(description),
                    None => surface,
                };
                surface
                    .footer(
                        Button::new("confirm-cancel")
                            .label(t!("Cancel"))
                            .variant(ButtonVariant::Outline)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .footer(Button::new("confirm-update").label(t!("Confirm")).on_click(cx.listener(move |this, _, _, cx| {
                        this.close_dialog(cx);
                        update_all(targets.clone(), cx);
                    })))
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

/// `ENVIRONMENT_MACHINE_KIND_LABELS`.
fn machine_kind_label(kind: t3code_contracts::EnvironmentMachineKind) -> SharedString {
    use t3code_contracts::EnvironmentMachineKind as Kind;
    match kind {
        Kind::Server => t!("Server"),
        Kind::Cloud => t!("Cloud VM"),
        Kind::Linux => t!("Linux/WSL"),
        Kind::Desktop => t!("Desktop"),
        Kind::Laptop => t!("Laptop"),
        Kind::MacMini => t!("Mini PC"),
        Kind::MacStudio => t!("Workstation"),
        _ => t!("Server"),
    }
}

/// `EnvironmentIconMenu`'s popup: every machine kind, the detected one
/// marked; picking the detected kind clears the override.
fn environment_icon_menu(id: EnvironmentId, handle: PopupHandle, open: Entity<bool>, cx: &App) -> MenuPopup {
    use t3code_contracts::EnvironmentMachineKind as Kind;
    let config = crate::hooks::use_settings::ServerSettingsStore::config(cx, &id);
    let reported = config.as_ref().and_then(|c| c.environment.platform.machine);
    let detected = reported.unwrap_or(Kind::Server);
    let resolved = super::settings_controls::environment_machine_kind(&id, cx);
    let mut menu = MenuPopup::new();
    let locked = config.is_none();
    if locked {
        menu = menu.item(MenuItem::new(t!("Connect to this environment to change its icon.")).disabled(true)).separator();
    }
    for kind in [Kind::Server, Kind::Cloud, Kind::Linux, Kind::Desktop, Kind::Laptop, Kind::MacMini, Kind::MacStudio] {
        let (id, handle, open) = (id.clone(), handle.clone(), open.clone());
        let mut item = MenuItem::new(machine_kind_label(kind))
            .icon(super::settings_controls::machine_icon_source(kind))
            .icon_size(IconSize::Custom(cx.app_theme().sp(3.5)))
            .kind(MenuItemKind::Radio(kind == resolved))
            .disabled(locked);
        if kind == detected {
            item = item.detail(if reported.is_some() {
                t!("detected", context = "machine kind detected automatically")
            } else {
                t!("default", context = "machine kind used when none is detected")
            });
        }
        menu = menu.item(item.on_select(move |_, cx| {
            open.update(cx, |open, _| *open = false);
            handle.close(cx);
            let patch = t3code_contracts::ClientServerSettingsPatch { environment_icon: Some((kind != detected).then_some(kind)), ..Default::default() };
            let save = crate::hooks::use_settings::ServerSettingsStore::update(id.clone(), patch, cx);
            cx.spawn(async move |cx| {
                if let Err(error) = save.await {
                    cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, t!("Setting not saved")).description(error.0.to_string()), cx));
                }
            })
            .detach();
        }));
    }
    menu
}

/// The row's "More actions" menu; `icon_item` opens the Icon submenu, and
/// hovering any other row closes it.
fn environment_menu(
    this: WeakEntity<ConnectionsSettings>,
    env: &EnvironmentState,
    routes_open: bool,
    trace_id: Option<String>,
    icon_item: MenuItem,
    icon_open: Entity<bool>,
) -> MenuPopup {
    let id = env.id().clone();
    let label = SharedString::from(env.entry.label.clone());
    let mcp_url = environment_mcp_url(env);
    let close_icon = move |hovered: &bool, _: &mut Window, cx: &mut App| {
        if *hovered {
            icon_open.update(cx, |open, cx| {
                *open = false;
                cx.notify();
            });
        }
    };
    let close_icon = Rc::new(close_icon);
    let hover = |item: MenuItem| {
        let close = close_icon.clone();
        item.on_hover(move |hovered, window, cx| close(hovered, window, cx))
    };
    let mut menu = MenuPopup::new()
        .item(icon_item)
        .item(hover(MenuItem::new(if routes_open { t!("Hide routes") } else { t!("Routes") }).icon(IconName::Route)).on_select({
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
        menu = menu.item(hover(MenuItem::new(t!("Copy MCP URL"))).on_select(move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
        }));
    }
    if let Some(trace_id) = trace_id {
        menu = menu.item(hover(MenuItem::new(t!("Copy trace ID"))).on_select(move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(trace_id.clone()));
        }));
    }
    menu.separator().item(hover(MenuItem::new(t!("Remove from this device…")).destructive()).on_select(move |_, cx| {
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
        let load_balancing = self.render_load_balancing(&environments, cx);
        let shared: Vec<&EnvironmentState> =
            environments.iter().filter(|env| env.entry.enabled && present_phase(env) != crate::connection::presentation::PresentedPhase::Unsupported).collect();
        let pull_request_routing = self.render_pull_request_routing(&shared, self.pull_request_routing_open, cx);
        let cloud_rows = cloud_environment_connect_rows(&self.state, cx);
        let dialog = self.render_dialog(cx);
        let targets = self.update_targets(&environments, cx);
        let this = cx.entity().downgrade();
        let update_all_button = (!targets.is_empty()).then(|| {
            server_updates_action(
                "environments-update-all",
                &targets,
                None,
                ButtonVariant::GhostMuted,
                move |targets, _, cx| {
                    this.update(cx, |this, cx| this.start_updates(targets, cx)).ok();
                },
                cx,
            )
        });
        let theme = cx.app_theme();
        let c = &theme.colors;
        let header_action = Button::new("add-environment")
            .label(t!("Add environment"))
            .icon(IconName::Plus)
            .variant(ButtonVariant::GhostMuted)
            .size(ButtonSize::Xs)
            .on_click(cx.listener(|this, _, window, cx| this.open_add_dialog(None, window, cx)));
        let header_action = div().flex().items_center().gap(theme.sp(1.)).children(update_all_button).child(header_action);
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
            .children(super::linked_environments_settings::sections(cx))
            .children(load_balancing)
            .children(pull_request_routing)
            .children(dialog)
    }
}

/// The environment's server behind this client, from its config, else the version its session
/// reported (an environment whose config has not arrived).
fn environment_mismatch(env: &EnvironmentState, cx: &App) -> Option<VersionMismatch> {
    let config = ServerSettingsStore::config(cx, env.id());
    let server = config.as_ref().map(|c| SharedString::from(c.environment.server_version.clone())).or_else(|| env.server_version.clone());
    resolve_version_mismatch(server.as_deref(), &client_version(cx))
}
