//! Which servers run behind this client, and how each can be updated. Ports `versionSkew.ts`
//! and the comparison in `packages/shared/src/semver.ts`.
//!
//! A server is out of date only when it is behind: a newer server is fine, and a nightly and a
//! stable build of the same release are the same version unless both are nightlies. Updating
//! a server always targets this client's version. A dismissed notice stays dismissed for that
//! environment, client version and server version, persisted under the web's own key.

use std::cmp::Ordering;

use gpui_kit::{App, SharedString};
use serde::{Deserialize, Serialize};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::{ExecutionEnvironmentCapabilitiesServerInstallation as Installation, ServerConfig, ServerSelfUpdateCapability};

use crate::client_persistence_storage::LocalStorage;
use crate::connection::ConnectionRuntime;
use crate::i18n::t;

const DISMISSALS_KEY: &str = "t3code:version-mismatch-dismissals:v1";

/// A server behind this client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionMismatch {
    pub client_version: SharedString,
    pub server_version: SharedString,
}

impl VersionMismatch {
    /// The hint the web shows beside a mismatch.
    pub fn hint() -> SharedString {
        t!("Version mismatch. Try syncing the client and server to the same T3 Code version.")
    }

    /// The dismissal key (`buildVersionMismatchDismissalKey`).
    pub fn dismissal_key(&self, environment_id: &EnvironmentId) -> String {
        format!("{}:{}:{}", environment_id.as_str(), self.client_version, self.server_version)
    }
}

/// `major.minor.patch` and the prerelease identifiers.
#[derive(Debug, PartialEq, Eq)]
struct Semver<'a> {
    core: [u64; 3],
    prerelease: Vec<&'a str>,
}

/// Parses like `parseSemver`: build metadata dropped, missing minor or patch read as 0, a
/// leading `v` allowed.
fn parse_semver(value: &str) -> Option<Semver<'_>> {
    let value = value.trim().split('+').next().unwrap_or_default();
    let (main, prerelease) = value.split_once('-').unwrap_or((value, ""));
    let main = main.strip_prefix('v').unwrap_or(main);
    let segments: Vec<&str> = main.split('.').map(str::trim).filter(|s| !s.is_empty()).collect();
    if segments.is_empty() || segments.len() > 3 {
        return None;
    }
    let mut core = [0u64; 3];
    for (slot, segment) in core.iter_mut().zip(&segments) {
        if !segment.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = segment.parse().ok()?;
    }
    let prerelease = prerelease.split('.').map(str::trim).filter(|s| !s.is_empty()).collect();
    Some(Semver { core, prerelease })
}

fn compare_identifier(left: &str, right: &str) -> Ordering {
    let numeric = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    match (numeric(left), numeric(right)) {
        (true, true) => {
            let (l, r) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
            l.len().cmp(&r.len()).then_with(|| l.cmp(r))
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => left.cmp(right),
    }
}

/// `compareSemverVersions`: semver precedence, or plain string order when either side does not parse.
fn compare_semver(left: &str, right: &str) -> Ordering {
    let (Some(l), Some(r)) = (parse_semver(left), parse_semver(right)) else { return left.cmp(right) };
    l.core.cmp(&r.core).then_with(|| match (l.prerelease.is_empty(), r.prerelease.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for index in 0..l.prerelease.len().max(r.prerelease.len()) {
                match (l.prerelease.get(index), r.prerelease.get(index)) {
                    (None, _) => return Ordering::Less,
                    (_, None) => return Ordering::Greater,
                    (Some(a), Some(b)) => match compare_identifier(a, b) {
                        Ordering::Equal => {}
                        other => return other,
                    },
                }
            }
            Ordering::Equal
        }
    })
}

fn version_core(version: &str) -> &str {
    version.split(['-', '+']).next().unwrap_or(version)
}

/// `resolveVersionMismatch`: `Some` only when the server is behind `client_version`.
pub fn resolve_version_mismatch(server_version: Option<&str>, client_version: &str) -> Option<VersionMismatch> {
    let client = client_version.trim();
    let server = server_version.map(str::trim).filter(|v| !v.is_empty())?;
    if client.is_empty() {
        return None;
    }
    let (client_core, server_core) = (version_core(client), version_core(server));
    let nightly = |v: &str| parse_semver(v).is_some_and(|s| s.prerelease.first() == Some(&"nightly"));
    let both_nightly = nightly(client) && nightly(server);
    let behind = if parse_semver(client_core).is_some() && parse_semver(server_core).is_some() {
        let (server, client) = if both_nightly { (server, client) } else { (server_core, client_core) };
        compare_semver(server, client) == Ordering::Less
    } else {
        server != client
    };
    behind.then(|| VersionMismatch { client_version: client.to_owned().into(), server_version: server.to_owned().into() })
}

/// This client's version (`APP_VERSION`), as the server sees it in `clientAppVersion`.
pub fn client_version(cx: &App) -> SharedString {
    cx.try_global::<ConnectionRuntime>()
        .map(|runtime| SharedString::from(runtime.auth().presentation().app_version.clone()))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").into())
}

/// `resolveServerConfigVersionMismatch` for an environment's config.
pub fn config_version_mismatch(config: Option<&ServerConfig>, cx: &App) -> Option<VersionMismatch> {
    resolve_version_mismatch(config.map(|config| config.environment.server_version.as_str()), &client_version(cx))
}

/// How the server can update itself, if it can (`resolveServerSelfUpdateCapability`).
pub fn self_update_capability(config: Option<&ServerConfig>) -> Option<ServerSelfUpdateCapability> {
    config?.environment.capabilities.server_self_update.clone()
}

/// Whether the desktop app that runs the server can update it (`supportsDesktopAppUpdate`).
pub fn supports_desktop_app_update(config: Option<&ServerConfig>) -> bool {
    config.is_some_and(|config| config.environment.capabilities.desktop_app_update == Some(true))
}

/// Whether the client can start the update remotely: the host updates itself, and a
/// desktop-managed host only when its app can.
pub fn updates_remotely(capability: Option<&ServerSelfUpdateCapability>, desktop_app_update: bool) -> bool {
    capability.is_some_and(|c| *c != ServerSelfUpdateCapability::DesktopManaged || desktop_app_update)
}

/// The command a user runs on the host when it cannot update itself (`manualServerUpdateCommand`).
pub fn manual_server_update_command(target_version: &str, installation: Option<&Installation>) -> String {
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    match installation {
        Some(Installation::NpmGlobal(npm)) => {
            format!("npm install --global --prefix {} t3@{target_version}", quote(&npm.prefix))
        }
        // An inherited T3CODE_HOME would otherwise send the update to a home the launcher is not in.
        Some(Installation::T3Launcher(launcher)) => {
            format!("{} update --base-dir {} {target_version}", quote(&launcher.launcher), quote(&launcher.base_dir))
        }
        Some(Installation::NpxOrPnpmDlxOrBunx(runner)) => {
            use t3code_contracts::ExecutionEnvironmentCapabilitiesServerInstallationNpxOrPnpmDlxOrBunxKind as Kind;
            let runner = match runner.kind {
                Kind::PnpmDlx => "pnpm dlx",
                Kind::Bunx => "bunx",
                _ => "npx",
            };
            format!("{runner} t3@{target_version}")
        }
        _ => format!("npx t3@{target_version}"),
    }
}

/// Whether the manual command installs (npm global or launcher) rather than relaunches.
pub fn manual_command_installs(installation: Option<&Installation>) -> bool {
    matches!(installation, Some(Installation::NpmGlobal(_) | Installation::T3Launcher(_)))
}

#[derive(Default, Serialize, Deserialize)]
struct Dismissals {
    keys: Vec<String>,
}

/// Whether the mismatch notice with `key` was dismissed (`isVersionMismatchDismissed`).
pub fn is_dismissed(key: &str, cx: &App) -> bool {
    LocalStorage::get::<Dismissals>(cx, DISMISSALS_KEY).is_some_and(|d| d.keys.iter().any(|k| k == key))
}

/// Dismisses the mismatch notice with `key` for good (`dismissVersionMismatch`).
pub fn dismiss(key: String, cx: &mut App) {
    let mut dismissals = LocalStorage::get::<Dismissals>(cx, DISMISSALS_KEY).unwrap_or_default();
    if !dismissals.keys.contains(&key) {
        dismissals.keys.push(key);
        LocalStorage::set(cx, DISMISSALS_KEY, Some(&dismissals));
    }
}

/// The CLI release a version belongs to (`cliReleaseChannelOf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseChannel {
    Stable,
    Nightly,
    Preview,
}

pub fn release_channel(version: &str) -> ReleaseChannel {
    // `^[^-+]+-(nightly|preview)\.\d{8}\.\d+$`
    let Some((core, rest)) = version.split_once('-') else { return ReleaseChannel::Stable };
    if core.is_empty() || core.contains('+') {
        return ReleaseChannel::Stable;
    }
    let mut parts = rest.splitn(3, '.');
    let (channel, date, build) = (parts.next(), parts.next(), parts.next());
    let digits = |s: Option<&str>, len: Option<usize>| {
        s.is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && len.is_none_or(|len| s.len() == len))
    };
    if !digits(date, Some(8)) || !digits(build, None) {
        return ReleaseChannel::Stable;
    }
    match channel {
        Some("nightly") => ReleaseChannel::Nightly,
        Some("preview") => ReleaseChannel::Preview,
        _ => ReleaseChannel::Stable,
    }
}

/// Whether updating between these versions crosses Stable and Nightly (`crossesReleaseTrack`);
/// hosts refuse that outside a track switch.
pub fn crosses_release_track(from_version: &str, to_version: &str) -> bool {
    let (from, to) = (release_channel(from_version), release_channel(to_version));
    from != to && from != ReleaseChannel::Preview && to != ReleaseChannel::Preview
}

/// Why the environment's managed policy refuses updates (`managedUpdatesDenial`).
pub fn managed_updates_denial(config: Option<&ServerConfig>) -> Option<SharedString> {
    let policy = config?.managed_policy.as_ref()?;
    let off = policy.limits.as_ref()?.updates.as_ref()?.mode == t3code_contracts::ManagedUpdatesMode::Off;
    off.then(|| t!("{organization} manages T3 Code updates.", organization = policy.organization.as_str()))
}

/// Failed updates whose notice was dismissed, by environment, for this run of the app
/// (`dismissedServerUpdateFailures`). A newer failure is a different state and shows again.
#[derive(Default)]
struct DismissedFailures(Vec<(EnvironmentId, t3code_client_runtime::connection::server_update::ServerUpdateState)>);

impl gpui_kit::Global for DismissedFailures {}

/// Whether the failure notice for `state` was dismissed (`isServerUpdateFailureDismissed`).
pub fn is_server_update_failure_dismissed(
    environment_id: &EnvironmentId,
    state: &t3code_client_runtime::connection::server_update::ServerUpdateState,
    cx: &App,
) -> bool {
    matches!(state, t3code_client_runtime::connection::server_update::ServerUpdateState::Failed { .. })
        && cx.try_global::<DismissedFailures>().is_some_and(|d| d.0.iter().any(|(id, s)| id == environment_id && s == state))
}

/// Dismisses the failure notice for `state` (`dismissServerUpdateFailure`); other states are ignored.
pub fn dismiss_server_update_failure(
    environment_id: &EnvironmentId,
    state: &t3code_client_runtime::connection::server_update::ServerUpdateState,
    cx: &mut App,
) {
    if matches!(state, t3code_client_runtime::connection::server_update::ServerUpdateState::Failed { .. }) {
        cx.default_global::<DismissedFailures>().0.push((environment_id.clone(), state.clone()));
    }
}
