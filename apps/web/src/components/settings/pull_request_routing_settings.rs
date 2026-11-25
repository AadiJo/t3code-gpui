//! "Pull request sharing" on Settings → Environments. Ports `PullRequestRoutingSettings.tsx` and
//! the browser grant storage of `connection/storage.ts` (`makeBrowserPullRequestRoutingPermissions`).
//!
//! Grants live in local storage, one key per environment under the web's own names, so grants
//! imported from an existing install keep applying; [`init`] hands them to the client runtime's
//! [`PullRequestRouting`], which routes pull request calls, and drops a grant once its machine's
//! saved endpoints change.

use gpui_kit::{AnyElement, App, Context, IntoElement, SharedString, TestSupportExt as _, div, prelude::*};
use t3code_client_runtime::connection::pull_request_routing::{
    PullRequestRouting, PullRequestRoutingPermission as Permission, StoredPullRequestRoutingPermission, connection_key, permission_for,
};

use super::connections_settings::ConnectionsSettings;
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::select::{Select, SelectSize};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::client_persistence_storage::LocalStorage;
use crate::connection::presentation::transport_label;
use crate::connection::{ConnectionRuntime, EnvironmentState};
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// The stored names predate the rename and stay, so the web and this client share grants.
const KEY_PREFIX: &str = "t3code:github-routing:";

const OPTIONS: [Permission; 3] = [Permission::Off, Permission::Read, Permission::ReadWrite];

fn option_label(permission: Permission) -> SharedString {
    match permission {
        Permission::Off => t!("Off", context = "pull request routing permission"),
        Permission::Read => t!("Read PRs"),
        Permission::ReadWrite => t!("Read and act"),
    }
}

/// Starts pull request routing with the stored grants of the saved environments. Call once the
/// connection runtime runs; later calls do nothing.
pub fn init(cx: &mut App) {
    if !ConnectionRuntime::is_running(cx) || PullRequestRouting::global().is_some() {
        return;
    }
    let registry = ConnectionRuntime::global(cx).registry().clone();
    let ids: Vec<String> = registry.entries().borrow().iter().map(|e| e.environment_id.as_str().to_owned()).collect();
    let stored: Vec<StoredPullRequestRoutingPermission> = ids
        .iter()
        .filter_map(|id| {
            // A grant counts only under its own environment's key, as the web reads it.
            LocalStorage::get::<StoredPullRequestRoutingPermission>(cx, &format!("{KEY_PREFIX}{id}")).filter(|g| g.environment_id.as_str() == id)
        })
        .collect();
    PullRequestRouting::install(registry, stored);
    forget_stale(cx);
    let state = ConnectionRuntime::state(cx);
    cx.observe(&state, |_, cx| forget_stale(cx)).detach();
}

/// A changed set of saved addresses may reach a different server: its grant is dropped.
fn forget_stale(cx: &mut App) {
    let Some(routing) = PullRequestRouting::global() else { return };
    for environment_id in routing.forget_stale() {
        LocalStorage::set::<StoredPullRequestRoutingPermission>(cx, &format!("{KEY_PREFIX}{}", environment_id.as_str()), None);
    }
}

/// Saves `permission` for the saved environment `environment_id` (the row's select, and checks).
pub fn set_environment_permission(environment_id: &t3code_client_runtime::environment::EnvironmentId, permission: Permission, cx: &mut App) {
    let environment = ConnectionRuntime::state(cx).read(cx).environment(environment_id).cloned();
    if let Some(environment) = environment {
        set_permission(&environment, permission, cx);
    }
}

/// Saves `permission` for the environment's saved endpoints.
fn set_permission(environment: &EnvironmentState, permission: Permission, cx: &mut App) {
    let Some(routing) = PullRequestRouting::global() else { return };
    match routing.set(environment.id(), permission) {
        Some(grants) => {
            let key = format!("{KEY_PREFIX}{}", environment.id().as_str());
            let grant = grants.iter().find(|g| &g.environment_id == environment.id());
            LocalStorage::set(cx, &key, grant);
        }
        None => _ = Toaster::push(ToastData::new(ToastKind::Error, t!("Could not save pull request routing permission")), cx),
    }
}

/// "{machines} read and act · {machines} read PRs", or `None` when every machine is off.
fn summary(entries: &[(SharedString, Permission)]) -> Option<SharedString> {
    let groups: Vec<String> = [Permission::ReadWrite, Permission::Read]
        .into_iter()
        .filter_map(|permission| {
            let labels: Vec<&str> = entries.iter().filter(|(_, p)| *p == permission).map(|(label, _)| label.as_ref()).collect();
            (!labels.is_empty()).then(|| {
                let machines = labels.join(", ");
                match permission {
                    Permission::ReadWrite => t!("{machines} read and act", machines = machines.as_str()).to_string(),
                    _ => t!("{machines} read PRs", machines = machines.as_str()).to_string(),
                }
            })
        })
        .collect();
    (!groups.is_empty()).then(|| groups.join(" · ").into())
}

impl ConnectionsSettings {
    /// The folded "Pull request sharing" section, for two or more machines (`environments` are
    /// the page's enabled, supported ones).
    pub(super) fn render_pull_request_routing(&self, environments: &[&EnvironmentState], open: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if environments.len() < 2 {
            return None;
        }
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let grants = PullRequestRouting::global().map(|r| r.permissions().borrow().clone()).unwrap_or_else(|| std::sync::Arc::from([]));
        let entries: Vec<(SharedString, Permission)> = environments
            .iter()
            .map(|env| (SharedString::from(env.entry.label.clone()), permission_for(&env.entry, &grants)))
            .collect();
        let summary = summary(&entries).unwrap_or_else(|| t!("Off", context = "pull request routing permission"));
        let divider = with_alpha(c.border, 0.5);
        let header = div().flex().flex_wrap().items_center().gap_x(sp(4.)).px(sp(4.)).child(
            div()
                .id("pull-request-routing-toggle")
                .flex()
                .flex_1()
                .min_w_0()
                .min_h(sp(11.))
                .items_center()
                .gap(sp(2.))
                .py(sp(2.))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.toggle_pull_request_routing(cx)))
                .test_support()
                .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Custom(sp(4.))).color(c.muted_foreground))
                .child(div().flex_none().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(setting_title("pull-request-routing")))
                .child(div().min_w_0().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(summary)),
        );
        let ready = PullRequestRouting::global().is_some();
        let body = open.then(|| {
            let labels: Vec<SharedString> = OPTIONS.iter().map(|p| option_label(*p)).collect();
            let rows = environments.iter().map(|env| {
                let current = permission_for(&env.entry, &grants);
                let environment = (*env).clone();
                // A machine with no saved endpoint cannot hold a grant.
                let disabled = !ready || connection_key(&env.entry).is_none();
                let select = Select::new(
                    SharedString::from(format!("pull-request-routing-select-{}", env.id().as_str())),
                    labels.clone(),
                    OPTIONS.iter().position(|p| *p == current).unwrap_or(0),
                    move |ix, _, cx| {
                        if !disabled {
                            set_permission(&environment, OPTIONS[ix], cx);
                        }
                    },
                )
                .size(SelectSize::Xs);
                let select = div()
                    .id(SharedString::from(format!("pull-request-routing-{}", env.id().as_str())))
                    .when(disabled, |d| d.opacity(0.64))
                    .child(select)
                    .test_support();
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
                .child(div().px(sp(4.)).py(sp(2.5)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!(
                    "Machines you trust here can read PR data through each other's source control access. Enable both machines. Read and act may use broader permissions than the machine that owns them. This applies only to this device."
                )))
                .children(rows.collect::<Vec<_>>())
        });
        Some(
            div()
                .id("pull-request-routing")
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
}
