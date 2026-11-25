//! Which providers can update and how an update run ended. Ports the client-facing rules of
//! `ProviderUpdateLaunchNotification.logic.ts`: update candidates, one-click eligibility, run
//! outcomes and their toast.
//!
//! The file's sidebar pill view (`getProviderUpdateSidebarPillView`) reads only the primary
//! environment, the desktop app's bundled server, which this client does not have; it is not
//! ported.

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::SharedString;
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::{ServerProviderCompatibilityStatus, ServerProviderUpdateStatus, ServerProviderVersionAdvisoryStatus};

use crate::i18n::t;
use crate::provider_instances::{ServerProvider, provider_display_name};

/// The provider's display name, else its driver id (`PROVIDER_DISPLAY_NAMES[driver] ?? driver`).
pub fn provider_name(driver: &str) -> &str {
    provider_display_name(driver).unwrap_or(driver)
}

fn latest_version_unusable(provider: &ServerProvider) -> bool {
    provider.compatibility_advisory.as_ref().is_some_and(|c| {
        matches!(c.latest_version_status, Some(ServerProviderCompatibilityStatus::Broken | ServerProviderCompatibilityStatus::Unsupported))
    })
}

fn is_behind_latest(provider: &ServerProvider) -> bool {
    provider
        .version_advisory
        .as_ref()
        .is_some_and(|a| a.status == ServerProviderVersionAdvisoryStatus::BehindLatest && a.latest_version.is_some())
}

/// `isProviderUpdateCandidate`: enabled, behind a latest release that works.
pub fn is_provider_update_candidate(provider: &ServerProvider) -> bool {
    provider.enabled && !latest_version_unusable(provider) && is_behind_latest(provider)
}

/// `isProviderUpdateActive`: an update is queued or running.
pub fn is_provider_update_active(provider: &ServerProvider) -> bool {
    provider
        .update_state
        .as_ref()
        .is_some_and(|u| matches!(u.status, ServerProviderUpdateStatus::Queued | ServerProviderUpdateStatus::Running))
}

/// `isProviderSettingsUpdateCandidate`: enabled, behind latest, and updatable with a command.
pub fn is_provider_settings_update_candidate(provider: &ServerProvider) -> bool {
    provider.enabled
        && !latest_version_unusable(provider)
        && provider.version_advisory.as_ref().is_some_and(|a| {
            a.status == ServerProviderVersionAdvisoryStatus::BehindLatest && a.can_update == Some(true) && a.update_command.is_some()
        })
}

/// `chooseRepresentativeProvider`: the driver's default instance, else the latest checked.
fn representative<'a>(current: Option<&'a Arc<ServerProvider>>, candidate: &'a Arc<ServerProvider>) -> &'a Arc<ServerProvider> {
    let Some(current) = current else { return candidate };
    if candidate.instance_id == candidate.driver {
        return candidate;
    }
    if current.instance_id == current.driver {
        return current;
    }
    if candidate.checked_at >= current.checked_at { candidate } else { current }
}

/// `collectProviderUpdateCandidates`: one candidate per driver, in first-seen driver order.
pub fn collect_provider_update_candidates(providers: &[Arc<ServerProvider>]) -> Vec<Arc<ServerProvider>> {
    let mut by_driver: Vec<Arc<ServerProvider>> = Vec::new();
    for provider in providers.iter().filter(|p| is_provider_update_candidate(p)) {
        match by_driver.iter().position(|p| p.driver == provider.driver) {
            Some(ix) => by_driver[ix] = representative(Some(&by_driver[ix]), provider).clone(),
            None => by_driver.push(provider.clone()),
        }
    }
    by_driver
}

/// `hasOneClickUpdateProviderCandidate`: every outdated instance of the driver updates with
/// the same command.
fn has_one_click_update(candidate: &ServerProvider, providers: &[Arc<ServerProvider>]) -> bool {
    let Some(advisory) = &candidate.version_advisory else { return false };
    if advisory.can_update != Some(true) || advisory.update_command.is_none() {
        return false;
    }
    let mut commands = HashSet::new();
    for provider in providers.iter().filter(|p| p.driver == candidate.driver && is_provider_update_candidate(p)) {
        match provider.version_advisory.as_ref() {
            Some(a) if a.can_update == Some(true) => match &a.update_command {
                Some(command) => _ = commands.insert(command.as_str()),
                None => return false,
            },
            _ => return false,
        }
    }
    commands.len() == 1
}

/// `canOneClickUpdateProviderCandidate`.
pub fn can_one_click_update(candidate: &ServerProvider, providers: &[Arc<ServerProvider>]) -> bool {
    !is_provider_update_active(candidate) && has_one_click_update(candidate, providers)
}

/// The candidates "Update all" runs on one machine.
pub fn one_click_update_candidates(providers: &[Arc<ServerProvider>]) -> Vec<Arc<ServerProvider>> {
    collect_provider_update_candidates(providers).into_iter().filter(|c| can_one_click_update(c, providers)).collect()
}

/// `getProviderRestartNotice`: only OpenCode reports an installed CLI apart from a running
/// external server.
pub fn provider_restart_notice(provider: &ServerProvider) -> Option<&str> {
    let state = provider.update_state.as_ref()?;
    (state.status == ServerProviderUpdateStatus::Succeeded
        && provider.installed_version.is_some()
        && provider.version.is_some()
        && provider.installed_version != provider.version)
        .then_some(())
        .and(state.message.as_deref())
}

/// One provider update a run started (`ProviderUpdateRun`).
#[derive(Clone, Debug)]
pub struct ProviderUpdateRun {
    pub environment_id: EnvironmentId,
    pub machine_label: SharedString,
    pub driver: String,
    pub instance_id: String,
    /// The providers the server answered with, or why the call failed.
    pub result: Result<Vec<ServerProvider>, SharedString>,
}

/// How one provider update ended (`getProviderUpdateRunOutcome`).
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderUpdateOutcome {
    Updated { message: Option<SharedString> },
    Failed { line: SharedString },
}

impl ProviderUpdateOutcome {
    pub fn is_updated(&self) -> bool {
        matches!(self, Self::Updated { .. })
    }
}

/// `getProviderUpdateRunOutcome`.
pub fn provider_update_run_outcome(run: &ProviderUpdateRun) -> ProviderUpdateOutcome {
    let label = format!("{} · {}", run.machine_label, provider_name(&run.driver));
    let providers = match &run.result {
        Ok(providers) => providers,
        Err(error) => return ProviderUpdateOutcome::Failed { line: format!("{label}: {error}").into() },
    };
    let provider = providers.iter().find(|p| p.instance_id == run.instance_id);
    let state = provider.and_then(|p| p.update_state.as_ref());
    if state.is_some_and(|s| s.status == ServerProviderUpdateStatus::Succeeded) {
        let notice = provider.and_then(provider_restart_notice);
        return ProviderUpdateOutcome::Updated { message: notice.map(|n| format!("{label}: {n}").into()) };
    }
    let reason = state.and_then(|s| s.message.clone()).map(SharedString::from).unwrap_or_else(|| t!("Provider update did not finish."));
    ProviderUpdateOutcome::Failed { line: format!("{label}: {reason}").into() }
}

/// A toast's kind, title and description (`getProviderUpdateRunToastView`).
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderUpdateToastView {
    pub success: bool,
    pub title: SharedString,
    pub description: SharedString,
}

fn updated_description(count: usize) -> SharedString {
    if count == 1 { t!("New sessions will use the updated provider.") } else { t!("New sessions will use the updated providers.") }
}

/// `getProviderUpdateRunToastView`.
pub fn provider_update_run_toast_view(runs: &[ProviderUpdateRun]) -> Option<ProviderUpdateToastView> {
    let settled: Vec<ProviderUpdateOutcome> = runs.iter().map(provider_update_run_outcome).collect();
    if settled.is_empty() {
        return None;
    }
    let failures: Vec<&str> = settled
        .iter()
        .filter_map(|o| match o {
            ProviderUpdateOutcome::Failed { line } => Some(line.as_ref()),
            ProviderUpdateOutcome::Updated { .. } => None,
        })
        .collect();
    let mut successes: Vec<&str> = Vec::new();
    for outcome in &settled {
        if let ProviderUpdateOutcome::Updated { message: Some(message) } = outcome
            && !successes.contains(&message.as_ref())
        {
            successes.push(message);
        }
    }
    if failures.is_empty() {
        let title = if settled.len() == 1 {
            t!("Provider updated")
        } else {
            t!("{count, plural, one {{count} providers updated} other {{count} providers updated}}", count = settled.len())
        };
        let description = if successes.is_empty() { updated_description(settled.len()) } else { successes.join("\n").into() };
        return Some(ProviderUpdateToastView { success: true, title, description });
    }
    let title = if failures.len() < settled.len() {
        t!(
            "{total, plural, one {{failed} of {total} provider updates failed} other {{failed} of {total} provider updates failed}}",
            failed = failures.len(),
            total = settled.len()
        )
    } else if settled.len() == 1 {
        t!("Provider update failed")
    } else {
        t!("Provider updates failed")
    };
    let description = failures.iter().chain(successes.iter()).copied().collect::<Vec<_>>().join("\n").into();
    Some(ProviderUpdateToastView { success: false, title, description })
}
