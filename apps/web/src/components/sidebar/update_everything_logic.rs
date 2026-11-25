//! What "update everything" updates and how its run reads. Ports `updateEverything.logic.ts`
//! for a client that hosts no server: there is no local machine and no app update in a run,
//! and update-track switches (`collectChannelSwitchTargets`) belong to About.

use std::sync::Arc;

use gpui_kit::SharedString;
use t3code_client_runtime::connection::server_update::ServerUpdateState;
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::{ServerConfig, ServerSelfUpdateCapability};

use crate::components::server_update_action::ServerUpdateTarget;
use t3code_client_runtime::connection::server_update::ServerUpdateOutcome;
use crate::i18n::t;
use crate::version_skew::{ReleaseChannel, managed_updates_denial, release_channel, resolve_version_mismatch};

/// One saved environment as the run sees it (`UpdateEverythingMachine`).
#[derive(Clone, Debug)]
pub struct UpdateEverythingMachine {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub config: Option<Arc<ServerConfig>>,
    pub connected: bool,
    pub can_maintain: bool,
    pub update_state: ServerUpdateState,
}

/// A machine whose server restarts in the run (`RestartingMachine`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartingMachine {
    pub environment_id: EnvironmentId,
    pub continues_threads: bool,
}

/// `collectRestartingMachines`: connected machines whose server the run updates.
pub fn collect_restarting_machines(machines: &[UpdateEverythingMachine], servers: &[ServerUpdateTarget]) -> Vec<RestartingMachine> {
    machines
        .iter()
        .filter(|m| m.connected && servers.iter().any(|s| s.environment_id == m.environment_id))
        .filter_map(|m| {
            let config = m.config.as_ref()?;
            Some(RestartingMachine {
                environment_id: m.environment_id.clone(),
                continues_threads: config.settings.continue_threads_after_server_update == Some(true),
            })
        })
        .collect()
}

/// A machine the run leaves out, and why (`SkippedMachine`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedMachine {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub reason: SharedString,
}

fn channel_name(channel: ReleaseChannel) -> &'static str {
    match channel {
        ReleaseChannel::Stable => "stable",
        ReleaseChannel::Nightly => "nightly",
        ReleaseChannel::Preview => "preview",
    }
}

fn unavailable_reason(machine: &UpdateEverythingMachine) -> Option<SharedString> {
    if machine.config.is_none() || !machine.connected {
        return Some(t!("not connected"));
    }
    if !machine.can_maintain {
        return Some(t!("no permission"));
    }
    machine.update_state.is_running().then(|| t!("already updating"))
}

/// `managedMachineUpdateDenial`: the server refuses these with the same message. `target` is
/// `None` when the host picks its own version, as desktop apps do.
fn managed_machine_update_denial(config: &ServerConfig, target: Option<&str>) -> Option<SharedString> {
    managed_updates_denial(Some(config)).or_else(|| {
        let policy = config.managed_policy.as_ref()?;
        let no_nightly = policy.limits.as_ref()?.updates.as_ref()?.allow_nightly == Some(false);
        (no_nightly && target.is_some_and(|v| release_channel(v) != ReleaseChannel::Stable)).then(|| {
            t!(
                "{organization} manages T3 Code updates. Only stable releases can be installed.",
                organization = policy.organization.as_str()
            )
        })
    })
}

/// `collectServerUpdateTargets`: every server behind `target_version` the run can update, and
/// why it skips the others. Each server's updater checks it can install the release before it
/// restarts, so no registry precheck runs here.
pub fn collect_server_update_targets(
    machines: &[UpdateEverythingMachine],
    target_version: &SharedString,
) -> (Vec<ServerUpdateTarget>, Vec<SkippedMachine>) {
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for machine in machines {
        let config = machine.config.as_deref();
        if config.is_some_and(|c| resolve_version_mismatch(Some(&c.environment.server_version), target_version).is_none()) {
            continue;
        }
        let mut skip = |reason: SharedString| {
            skipped.push(SkippedMachine { environment_id: machine.environment_id.clone(), label: machine.label.clone(), reason })
        };
        let Some(config) = config.filter(|_| unavailable_reason(machine).is_none()) else {
            skip(unavailable_reason(machine).unwrap_or_else(|| t!("not connected")));
            continue;
        };
        let self_update = config.environment.capabilities.server_self_update.clone();
        let desktop_managed = self_update == Some(ServerSelfUpdateCapability::DesktopManaged);
        if let Some(denial) = managed_machine_update_denial(config, (!desktop_managed).then_some(target_version.as_ref())) {
            skip(denial);
            continue;
        }
        let desktop_app_update = config.environment.capabilities.desktop_app_update == Some(true);
        if self_update.is_none() {
            skip(t!("needs a manual update"));
            continue;
        }
        if desktop_managed && !desktop_app_update {
            skip(t!("needs a newer desktop app"));
            continue;
        }
        let server_channel = release_channel(&config.environment.server_version);
        // A desktop app installs its own track's newest release. Nightly builds run ahead of
        // Stable, so a Nightly host behind a Stable target has something newer; a Stable host may not.
        if server_channel != release_channel(target_version) && (!desktop_managed || server_channel == ReleaseChannel::Stable) {
            skip(t!("on the {channel} channel", channel = channel_name(server_channel)));
            continue;
        }
        targets.push(ServerUpdateTarget::for_config(
            machine.environment_id.clone(),
            machine.label.clone(),
            Some(config),
            target_version.clone(),
        ));
    }
    (targets, skipped)
}

/// What a machine's row is doing in a run (`UpdateRunMachineStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateRunMachineStatus {
    Providers,
    Waiting,
    Downloading,
    Restarting,
    Done,
    Failed,
}

impl UpdateRunMachineStatus {
    /// `RUN_STATUS_LABELS`.
    pub fn label(self) -> SharedString {
        match self {
            Self::Providers => t!("updating providers"),
            Self::Waiting => t!("waiting"),
            Self::Downloading => t!("downloading"),
            Self::Restarting => t!("restarting"),
            Self::Done => t!("done"),
            Self::Failed => t!("failed"),
        }
    }

    fn is_active(self) -> bool {
        !matches!(self, Self::Done | Self::Failed)
    }

    /// Working now: active and not waiting for its turn.
    pub fn is_working(self) -> bool {
        self.is_active() && self != Self::Waiting
    }
}

/// A machine's end in a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateRunResult {
    Done,
    Failed,
}

/// One machine in a run (`UpdateRunMachine`).
#[derive(Clone, Debug)]
pub struct UpdateRunMachine {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub updates_providers: bool,
    /// The server update still owed; cleared once it succeeded.
    pub server: Option<ServerUpdateTarget>,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    pub result: Option<UpdateRunResult>,
    pub failure: Option<SharedString>,
    pub provider_failure: Option<SharedString>,
}

/// The run's phase (`UpdateEverythingRun.phase`); this client never counts down to or installs
/// an app update of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateRunPhase {
    Providers,
    Servers,
}

/// A running or settled update of everything (`UpdateEverythingRun`).
#[derive(Clone, Debug)]
pub struct UpdateEverythingRun {
    pub phase: UpdateRunPhase,
    pub machines: Vec<UpdateRunMachine>,
}

/// `resolveUpdateRunMachineStatus`.
pub fn resolve_update_run_machine_status(machine: &UpdateRunMachine, phase: UpdateRunPhase, server_state: &ServerUpdateState) -> UpdateRunMachineStatus {
    use t3code_client_runtime::connection::server_update::ServerUpdateStage;
    match machine.result {
        Some(UpdateRunResult::Done) => return UpdateRunMachineStatus::Done,
        Some(UpdateRunResult::Failed) => return UpdateRunMachineStatus::Failed,
        None => {}
    }
    if phase == UpdateRunPhase::Providers && machine.updates_providers {
        return UpdateRunMachineStatus::Providers;
    }
    match server_state {
        ServerUpdateState::Running { stage: ServerUpdateStage::Resuming, .. } if machine.server.is_some() => UpdateRunMachineStatus::Restarting,
        ServerUpdateState::Running { .. } if machine.server.is_some() => UpdateRunMachineStatus::Downloading,
        _ => UpdateRunMachineStatus::Waiting,
    }
}

/// `isServerUpdateHandedOff`: the host accepted the update and is restarting into it.
pub fn is_server_update_handed_off(state: &ServerUpdateState) -> bool {
    use t3code_client_runtime::connection::server_update::ServerUpdateStage;
    matches!(state, ServerUpdateState::Running { stage: ServerUpdateStage::Resuming, .. })
}

/// `closeFinishedUpdateRun`: a run whose every machine is done is over.
pub fn close_finished_update_run(run: UpdateEverythingRun) -> Option<UpdateEverythingRun> {
    let finished = run.phase == UpdateRunPhase::Servers && run.machines.iter().all(|m| m.result == Some(UpdateRunResult::Done));
    (!finished).then_some(run)
}

const UPDATE_SLOW_AFTER_MS: u64 = 3 * 60 * 1000;

/// `formatUpdateElapsed`: "<1m", "4m", "1h 5m".
pub fn format_update_elapsed(ms: u64) -> SharedString {
    let minutes = ms / 60_000;
    if minutes < 1 {
        return t!("<1m");
    }
    if minutes < 60 {
        return t!("{minutes}m", minutes = minutes);
    }
    t!("{hours}h {minutes}m", hours = minutes / 60, minutes = minutes % 60)
}

/// The capsule's tone (`UpdateCapsuleView.tone`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapsuleTone {
    Default,
    Slow,
    Failed,
}

/// The capsule's action (`UpdateCapsuleView.action`); "Later" belongs to an app restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapsuleAction {
    Retry,
    Dismiss,
}

impl CapsuleAction {
    pub fn label(self) -> SharedString {
        match self {
            Self::Retry => t!("Retry"),
            Self::Dismiss => t!("Dismiss"),
        }
    }
}

/// What the progress capsule shows (`UpdateCapsuleView`).
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateCapsuleView {
    pub tone: CapsuleTone,
    pub label: SharedString,
    /// The slowest machine's status and elapsed time, once it passed three minutes.
    pub slow: Option<(SharedString, SharedString)>,
    pub action: Option<CapsuleAction>,
}

/// `describeUpdateCapsule` for `machines` (each with its status) at `now` (ms since the epoch).
pub fn describe_update_capsule(machines: &[(&UpdateRunMachine, UpdateRunMachineStatus)], now: u64) -> UpdateCapsuleView {
    let total = machines.len();
    let done = machines.iter().filter(|(_, s)| *s == UpdateRunMachineStatus::Done).count();
    let counted = || t!("{done} of {total} updated", done = done, total = total);
    let slowest = machines.iter().filter(|(_, s)| s.is_working()).min_by_key(|(m, _)| m.started_at);
    if let Some((machine, status)) = slowest
        && now.saturating_sub(machine.started_at) >= UPDATE_SLOW_AFTER_MS
    {
        return UpdateCapsuleView {
            tone: CapsuleTone::Slow,
            label: machine.label.clone(),
            slow: Some((status.label(), format_update_elapsed(now - machine.started_at))),
            action: None,
        };
    }
    if machines.iter().any(|(_, s)| s.is_active()) {
        return UpdateCapsuleView { tone: CapsuleTone::Default, label: counted(), slow: None, action: None };
    }
    let failed: Vec<&UpdateRunMachine> = machines.iter().filter(|(_, s)| *s == UpdateRunMachineStatus::Failed).map(|(m, _)| *m).collect();
    if let [first, ..] = failed.as_slice() {
        let label = if failed.len() == 1 {
            t!("{machine} failed", machine = first.label.as_ref())
        } else {
            t!("{count, plural, one {{count} failed} other {{count} failed}}", count = failed.len())
        };
        let retry = failed.iter().any(|m| m.server.is_some());
        return UpdateCapsuleView {
            tone: CapsuleTone::Failed,
            label,
            slow: None,
            action: Some(if retry { CapsuleAction::Retry } else { CapsuleAction::Dismiss }),
        };
    }
    UpdateCapsuleView { tone: CapsuleTone::Default, label: counted(), slow: None, action: None }
}

/// How a run ended, for its toast (`UpdateEverythingResult`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateEverythingResult {
    pub updated_servers: usize,
    pub updated_providers: usize,
    pub failures: Vec<SharedString>,
}

/// `collectUpdateEverythingResult`.
pub fn collect_update_everything_result(
    servers: &[(SharedString, ServerUpdateOutcome)],
    providers: &[crate::components::provider_update_launch_notification::ProviderUpdateOutcome],
) -> UpdateEverythingResult {
    use crate::components::provider_update_launch_notification::ProviderUpdateOutcome;
    let mut failures: Vec<SharedString> = servers
        .iter()
        .filter_map(|(label, outcome)| match outcome {
            ServerUpdateOutcome::Failed(failure) => {
                Some(format!("{label}: {}", crate::components::server_update_action::failure_text(failure)).into())
            }
            _ => None,
        })
        .collect();
    failures.extend(providers.iter().filter_map(|o| match o {
        ProviderUpdateOutcome::Failed { line } => Some(line.clone()),
        ProviderUpdateOutcome::Updated { .. } => None,
    }));
    UpdateEverythingResult {
        updated_servers: servers.iter().filter(|(_, o)| matches!(o, ServerUpdateOutcome::Updated(_))).count(),
        updated_providers: providers.iter().filter(|o| o.is_updated()).count(),
        failures,
    }
}

/// `updateTargetsSentence` without the app: "Update 2 servers and 1 provider".
fn update_targets_sentence(updated: bool, servers: usize, providers: usize) -> Option<SharedString> {
    Some(match (servers > 0, providers > 0, updated) {
        (true, true, false) => t!(
            "Update {servers, plural, one {{servers} server} other {{servers} servers}} and {providers, plural, one {{providers} provider} other {{providers} providers}}",
            servers = servers,
            providers = providers
        ),
        (true, true, true) => t!(
            "Updated {servers, plural, one {{servers} server} other {{servers} servers}} and {providers, plural, one {{providers} provider} other {{providers} providers}}",
            servers = servers,
            providers = providers
        ),
        (true, false, false) => t!("Update {servers, plural, one {{servers} server} other {{servers} servers}}", servers = servers),
        (true, false, true) => t!("Updated {servers, plural, one {{servers} server} other {{servers} servers}}", servers = servers),
        (false, true, false) => {
            t!("Update {providers, plural, one {{providers} provider} other {{providers} providers}}", providers = providers)
        }
        (false, true, true) => {
            t!("Updated {providers, plural, one {{providers} provider} other {{providers} providers}}", providers = providers)
        }
        (false, false, _) => return None,
    })
}

/// A run's toast (`getUpdateEverythingToastView`): success with what updated, or an error
/// listing each failure.
pub fn update_everything_toast_view(result: &UpdateEverythingResult) -> Option<(bool, SharedString, Option<SharedString>)> {
    let updated = update_targets_sentence(true, result.updated_servers, result.updated_providers);
    if result.failures.is_empty() {
        return updated.map(|title| (true, title, None));
    }
    let description = result.failures.iter().map(SharedString::as_ref).collect::<Vec<_>>().join("\n");
    Some((false, updated.unwrap_or_else(|| t!("Update failed")), Some(description.into())))
}

/// What a dialog row updates on its machine (`UpdateEverythingRow.app`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowApp {
    Server,
    DesktopApp,
}

impl RowApp {
    /// `APP_LABELS`.
    pub fn label(self) -> SharedString {
        match self {
            Self::Server => t!("Server"),
            Self::DesktopApp => t!("Desktop app"),
        }
    }
}

/// One machine in the dialog (`UpdateEverythingRow`).
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateEverythingRow {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub app: Option<RowApp>,
    pub skipped: Option<SharedString>,
    pub providers: Vec<SharedString>,
}

/// A machine with providers to update (`UpdateEverythingPlan.providerMachines`).
#[derive(Clone, Debug)]
pub struct ProviderMachine {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub providers: Vec<SharedString>,
}

/// What the pill's tooltip and the dialog say (`describeUpdateEverything`).
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateEverythingView {
    pub summary: SharedString,
    pub version: Option<SharedString>,
    pub provider_count: usize,
    /// Servers first, then provider-only machines, then skipped ones.
    pub rows: Vec<UpdateEverythingRow>,
}

/// `describeUpdateEverything`.
pub fn describe_update_everything(servers: &[ServerUpdateTarget], provider_machines: &[ProviderMachine], skipped: &[SkippedMachine]) -> UpdateEverythingView {
    let provider_count = provider_machines.iter().map(|m| m.providers.len()).sum();
    let summary =
        update_targets_sentence(false, servers.len(), provider_count).unwrap_or_else(|| t!("Update {items}", items = ""));
    let mut rows: Vec<UpdateEverythingRow> = Vec::new();
    let mut row = |id: &EnvironmentId, label: &SharedString, change: &dyn Fn(&mut UpdateEverythingRow)| {
        let ix = rows.iter().position(|r| &r.environment_id == id).unwrap_or_else(|| {
            rows.push(UpdateEverythingRow { environment_id: id.clone(), label: label.clone(), app: None, skipped: None, providers: Vec::new() });
            rows.len() - 1
        });
        change(&mut rows[ix]);
    };
    for server in servers {
        let app = if server.self_update == Some(ServerSelfUpdateCapability::DesktopManaged) { RowApp::DesktopApp } else { RowApp::Server };
        row(&server.environment_id, &server.server_label, &|r| r.app = Some(app));
    }
    for machine in provider_machines {
        row(&machine.environment_id, &machine.label, &|r| r.providers = machine.providers.clone());
    }
    for machine in skipped {
        row(&machine.environment_id, &machine.label, &|r| r.skipped = Some(machine.reason.clone()));
    }
    let version = servers.iter().find(|s| s.self_update != Some(ServerSelfUpdateCapability::DesktopManaged)).map(|s| s.target_version.clone());
    UpdateEverythingView { summary, version, provider_count, rows }
}
