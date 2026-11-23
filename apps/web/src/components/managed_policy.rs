//! What an administrator's policy locks on a server, and how a locked
//! control looks. Ports `ManagedPolicy.tsx`, `state/managedPolicy.ts` and the
//! summary helpers in `contracts/managedPolicy.ts`.

use std::collections::BTreeSet;

use gpui_kit::{AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, prelude::*};
use t3code_contracts::{ManagedPolicySummary, RuntimeMode};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::i18n::t;

/// `isManagedSettingPath`: the path is locked, sits inside a locked setting, or contains one.
pub fn is_managed_setting_path(policy: Option<&ManagedPolicySummary>, path: &str) -> bool {
    policy.is_some_and(|policy| {
        policy.locked_settings.iter().any(|locked| {
            locked == path
                || locked.strip_prefix(path).is_some_and(|rest| rest.starts_with('.'))
                || path.strip_prefix(locked.as_str()).is_some_and(|rest| rest.starts_with('.'))
        })
    })
}

/// The organization that locks any of `paths`, for a row's "Managed by" tooltip.
pub fn managed_by(policy: Option<&ManagedPolicySummary>, paths: &[&str]) -> Option<SharedString> {
    let policy = policy?;
    paths
        .iter()
        .any(|path| is_managed_setting_path(Some(policy), path))
        .then(|| SharedString::from(policy.organization.clone()))
}

/// "Managed by {organization}." for a locked row.
pub fn managed_by_reason(organization: &str) -> SharedString {
    t!("Managed by {organization}.", organization = organization)
}

/// `isManagedDriverAllowed`.
pub fn is_managed_driver_allowed(policy: Option<&ManagedPolicySummary>, driver: &str) -> bool {
    match policy.and_then(|p| p.limits.as_ref()?.providers.as_ref()?.allowed_drivers.as_ref()) {
        Some(allowed) => allowed.iter().any(|d| d.as_str() == driver),
        None => true,
    }
}

/// Whether a policy allows no driver at all, which leaves nothing to add.
pub fn allows_no_provider(policy: Option<&ManagedPolicySummary>) -> bool {
    policy
        .and_then(|p| p.limits.as_ref()?.providers.as_ref()?.allowed_drivers.as_ref())
        .is_some_and(Vec::is_empty)
}

/// `disallowedProviderMessage`.
pub fn disallowed_provider_message(organization: Option<&str>, label: &str) -> String {
    match organization {
        Some(org) => format!("{org}'s policy does not allow the {label} provider."),
        None => format!("Your organization policy does not allow the {label} provider."),
    }
}

/// What a policy pins on instances of one driver (`ManagedInstanceLocks`).
#[derive(Clone, Debug, PartialEq)]
pub struct ManagedInstanceLocks {
    pub config_keys: BTreeSet<String>,
    pub environment_names: BTreeSet<String>,
    pub whole_environment: bool,
    pub reason: SharedString,
}

/// `managedInstanceLocks`.
pub fn managed_instance_locks(policy: Option<&ManagedPolicySummary>, driver: &str) -> Option<ManagedInstanceLocks> {
    let policy = policy?;
    let paths = policy.limits.as_ref()?.providers.as_ref()?.locked_instance_fields.as_ref()?.get(driver)?;
    if paths.is_empty() {
        return None;
    }
    let under = |prefix: &str| paths.iter().filter_map(|p| p.strip_prefix(prefix)).map(str::to_owned).collect();
    Some(ManagedInstanceLocks {
        config_keys: under("config."),
        environment_names: under("environment."),
        whole_environment: paths.iter().any(|p| p == "environment"),
        reason: managed_by_reason(&policy.organization),
    })
}

fn runtime_mode_rank(mode: RuntimeMode) -> u8 {
    match mode {
        RuntimeMode::ApprovalRequired => 0,
        RuntimeMode::AutoAcceptEdits => 1,
        RuntimeMode::Auto => 2,
        RuntimeMode::FullAccess => 3,
    }
}

fn runtime_mode_label(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::ApprovalRequired => "Supervised",
        RuntimeMode::AutoAcceptEdits => "Auto-accept edits",
        RuntimeMode::Auto => "Auto",
        RuntimeMode::FullAccess => "Full access",
    }
}

/// `managedRuntimeModeDenial`: why `mode` is above the policy's ceiling.
pub fn managed_runtime_mode_denial(policy: Option<&ManagedPolicySummary>, mode: RuntimeMode) -> Option<String> {
    let policy = policy?;
    let ceiling = policy.limits.as_ref()?.max_runtime_mode?;
    (runtime_mode_rank(mode) > runtime_mode_rank(ceiling))
        .then(|| format!("{} limits threads to {}.", policy.organization, runtime_mode_label(ceiling)))
}

/// A capability a policy can switch off (`managedCapabilityDenial`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedCapability {
    RemoteClients,
    Connect,
    WebhookTasks,
    BrowserCookieImport,
    PreviewTunnel,
}

/// `managedCapabilityDenial`.
pub fn managed_capability_denial(policy: Option<&ManagedPolicySummary>, capability: ManagedCapability) -> Option<String> {
    let policy = policy?;
    let limits = policy.limits.as_ref()?;
    let (denied, label) = match capability {
        ManagedCapability::RemoteClients => (
            limits.clients.as_ref().is_some_and(|c| !c.iter().any(|c| c.as_str() == "remote")),
            "connecting from other devices",
        ),
        ManagedCapability::Connect => (limits.connect == Some(false), "T3 Connect"),
        ManagedCapability::WebhookTasks => (limits.webhook_tasks == Some(false), "webhook tasks"),
        ManagedCapability::BrowserCookieImport => (limits.browser_cookie_import == Some(false), "importing browser cookies"),
        ManagedCapability::PreviewTunnel => {
            (limits.preview_tunnel == Some(false), "opening this machine's local servers on other devices")
        }
    };
    denied.then(|| format!("{}'s policy does not allow {label}.", policy.organization))
}

/// `ManagedControl`: a control the policy turns off, drawn inert at half
/// opacity with the reason on hover. Without a reason it renders the control as is.
#[derive(IntoElement)]
pub struct ManagedControl {
    id: SharedString,
    reason: Option<SharedString>,
    child: AnyElement,
}

impl ManagedControl {
    pub fn new(id: impl Into<SharedString>, reason: Option<SharedString>, child: impl IntoElement) -> Self {
        Self { id: id.into(), reason, child: child.into_any_element() }
    }
}

impl RenderOnce for ManagedControl {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        match self.reason {
            None => self.child,
            Some(reason) => div()
                .id(self.id)
                .flex()
                .child(div().flex().items_center().gap_2().opacity(0.5).occlude().child(self.child))
                .tooltip(tooltip(TooltipSurface::new(reason)))
                .into_any_element(),
        }
    }
}

/// `ManagedNotice`: one line of policy text with the building mark.
pub fn managed_notice(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    div()
        .flex()
        .items_start()
        .gap(theme.sp(2.))
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .text_color(theme.colors.muted_foreground)
        .child(div().mt(theme.sp(0.5)).child(Icon::new(IconName::Building).size(IconSize::Sm).color(theme.colors.muted_foreground)))
        .child(text.into())
}
