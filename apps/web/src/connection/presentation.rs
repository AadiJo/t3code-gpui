//! Status text, route labels and failure diagnosis for the environment
//! screens. Ports `client-runtime/src/connection/presentation.ts`,
//! `diagnosis.ts` and the route label helpers of `routes.ts`, with the
//! English source strings unchanged so the web app's translations apply.

use gpui_kit::SharedString;
use t3code_client_runtime::connection::catalog::ConnectionRoute;
use t3code_client_runtime::connection::model::{
    BlockedReason, ConnectionErrorKind, ConnectionHop, ConnectionVia, FailureCause, NetworkStatus, TransientReason,
};
use t3code_client_runtime::connection::routes::{RouteKind, route_kind};
use t3code_client_runtime::connection::{ConnectionError, ConnectionPhase, ConnectionStatus};

use super::EnvironmentState;
use crate::i18n::t;

const T3_CONNECT_NAME: &str = "T3 Connect";

/// `EnvironmentConnectionPhase`: the supervisor phase as the UI words it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentedPhase {
    Off,
    Available,
    Offline,
    Connecting,
    Reconnecting,
    /// Reconnecting because the host has not approved this device yet.
    AwaitingApproval,
    Connected,
    Error,
    Unsupported,
}

/// `presentConnectionState`, plus `Off` for a switched-off environment.
pub fn present_phase(environment: &EnvironmentState) -> PresentedPhase {
    let Some(status) = &environment.status else {
        return if environment.entry.enabled { PresentedPhase::Connecting } else { PresentedPhase::Off };
    };
    let approval = status.last_failure.as_ref().is_some_and(ConnectionError::is_approval_pending);
    match status.phase {
        ConnectionPhase::Connecting | ConnectionPhase::Backoff if approval => PresentedPhase::AwaitingApproval,
        ConnectionPhase::Available => PresentedPhase::Available,
        ConnectionPhase::Offline => PresentedPhase::Offline,
        ConnectionPhase::Connecting if status.attempt <= 1 && status.last_failure.is_none() => PresentedPhase::Connecting,
        ConnectionPhase::Connecting | ConnectionPhase::Backoff => PresentedPhase::Reconnecting,
        ConnectionPhase::Connected => PresentedPhase::Connected,
        ConnectionPhase::Blocked => match &status.last_failure {
            Some(failure) if failure.is_unsupported() => PresentedPhase::Unsupported,
            _ => PresentedPhase::Error,
        },
    }
}

/// How a status reads: muted or as an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Error,
}

fn error_text(status: Option<&ConnectionStatus>) -> Option<&str> {
    status.and_then(|status| status.last_failure.as_ref()).map(|failure| failure.detail.as_str())
}

/// `savedBackendStatus`: the short status in an environment row's subtitle.
pub fn row_status(environment: &EnvironmentState) -> (SharedString, Tone) {
    let error = error_text(environment.status.as_ref());
    match present_phase(environment) {
        PresentedPhase::Off => (t!("Off", context = "environment switched off"), Tone::Muted),
        PresentedPhase::Connected => (t!("Connected"), Tone::Muted),
        PresentedPhase::Connecting => (t!("Connecting"), Tone::Muted),
        PresentedPhase::AwaitingApproval => {
            (error.map(SharedString::from).unwrap_or_else(|| t!("Waiting for approval")), Tone::Muted)
        }
        PresentedPhase::Reconnecting => match error {
            Some(error) => (t!("Reconnecting: {error}", error = error), Tone::Error),
            None => (t!("Reconnecting"), Tone::Error),
        },
        PresentedPhase::Unsupported => (t!("Client not supported"), Tone::Muted),
        PresentedPhase::Error => match error {
            Some(error) => (t!("Connection failed: {error}", error = error), Tone::Error),
            None => (t!("Connection failed"), Tone::Error),
        },
        PresentedPhase::Offline => (t!("Offline"), Tone::Muted),
        PresentedPhase::Available => (t!("Not connected"), Tone::Muted),
    }
}

/// `connectionStatusText`: the longer status for tooltips.
pub fn status_text(environment: &EnvironmentState) -> SharedString {
    let error = error_text(environment.status.as_ref());
    match present_phase(environment) {
        PresentedPhase::Off => t!("Switched off"),
        PresentedPhase::Available => t!("Available", context = "connection status"),
        PresentedPhase::Offline => t!("Offline", context = "connection status"),
        PresentedPhase::Connecting => t!("Connecting..."),
        PresentedPhase::AwaitingApproval => {
            error.map(SharedString::from).unwrap_or_else(|| t!("Waiting for approval on the host"))
        }
        PresentedPhase::Reconnecting => match error {
            Some(error) => t!("Failed to connect. Reconnecting... Reason: {error}", error = error),
            None => t!("Reconnecting..."),
        },
        PresentedPhase::Connected => t!("Connected", context = "connection status"),
        PresentedPhase::Unsupported => t!("Client not supported"),
        PresentedPhase::Error => match error {
            Some(error) => t!("Connection failed. Reason: {error}", error = error),
            None => t!("Connection failed"),
        },
    }
}

/// `connectionRouteLabelText`.
pub fn route_label(route: &ConnectionRoute) -> SharedString {
    match route_kind(route) {
        RouteKind::Relay => T3_CONNECT_NAME.into(),
        RouteKind::Loopback => t!("This device"),
        RouteKind::Lan => "LAN".into(),
        RouteKind::Tailnet => "Tailscale".into(),
        RouteKind::Vpn => "VPN".into(),
        RouteKind::Ssh => "SSH".into(),
        RouteKind::Public => route
            .bearer_profile()
            .and_then(|profile| profile.http_base_url.host_str().map(|host| SharedString::from(host.to_owned())))
            .unwrap_or_else(|| t!("Remote link")),
    }
}

/// `connectionRouteAddress`: the route's base URL, or none for relay routes.
pub fn route_address(route: &ConnectionRoute) -> Option<SharedString> {
    route.bearer_profile().map(|profile| profile.http_base_url.to_string().into())
}

/// `environmentTransportLabel`: how the row says the environment is reached.
pub fn transport_label(environment: &EnvironmentState) -> SharedString {
    let routes = &environment.entry.routes;
    let active = environment.status.as_ref().and_then(|status| status.active_route_id.as_deref());
    if routes.len() > 1 {
        let active = active.and_then(|id| {
            routes.iter().find(|route| t3code_client_runtime::connection::routes::route_id(&route.target) == id)
        });
        return match active {
            Some(route) => t!("via {route}", route = route_label(route)),
            None => route_label(&routes[0]),
        };
    }
    match routes.first() {
        Some(route) if route_kind(route) == RouteKind::Relay => T3_CONNECT_NAME.into(),
        Some(route) => route_address(route).unwrap_or_else(|| t!("Remote link")),
        None => t!("Remote link"),
    }
}

/// Round trip as the row shows it, e.g. `23 ms`.
pub fn latency_text(environment: &EnvironmentState) -> Option<SharedString> {
    environment.latency.map(|latency| format!("{} ms", latency.as_millis().max(1)).into())
}

/// How far a connection got along its path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HopResult {
    Ok,
    Failed,
    Unchecked,
}

/// What failed on the way to an environment, and what to try.
#[derive(Clone, Debug)]
pub struct Diagnosis {
    pub headline: SharedString,
    pub hops: Vec<(SharedString, HopResult)>,
    pub steps: Vec<SharedString>,
    pub facts: Vec<(SharedString, SharedString)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cause {
    Offline,
    Outdated,
    SessionPending,
    Failure(FailureCause),
}

struct Located {
    hop: ConnectionHop,
    cause: Option<Cause>,
    guessed: bool,
}

fn is_unreachable(failure: &ConnectionError) -> bool {
    matches!(
        failure.kind,
        ConnectionErrorKind::Transient(TransientReason::Network | TransientReason::Timeout | TransientReason::Transport)
    )
}

fn is_auth(failure: Option<&ConnectionError>) -> bool {
    failure.is_some_and(|failure| failure.kind == ConnectionErrorKind::Blocked(BlockedReason::Authentication))
}

fn locate(status: &ConnectionStatus, relay_only: bool) -> Option<Located> {
    let failure = status.last_failure.as_ref();
    if status.network == NetworkStatus::Offline || status.phase == ConnectionPhase::Offline {
        return Some(Located { hop: ConnectionHop::Device, cause: Some(Cause::Offline), guessed: false });
    }
    let failure = failure?;
    if failure.server_update_required {
        return Some(Located { hop: ConnectionHop::Server, cause: Some(Cause::Outdated), guessed: false });
    }
    if failure.kind == ConnectionErrorKind::Transient(TransientReason::SessionPending) {
        return Some(Located { hop: ConnectionHop::Device, cause: Some(Cause::SessionPending), guessed: false });
    }
    if let Some(hop) = failure.hop {
        return Some(Located { hop, cause: failure.failure_cause.map(Cause::Failure), guessed: false });
    }
    let via_relay = match failure.via {
        Some(via) => via == ConnectionVia::Relay,
        None => relay_only,
    };
    let hop = if is_unreachable(failure) && via_relay { ConnectionHop::Tunnel } else { ConnectionHop::Server };
    Some(Located { hop, cause: None, guessed: true })
}

fn headline(host: &str, located: &Located, failure: Option<&ConnectionError>) -> SharedString {
    use FailureCause as F;
    match located.cause {
        Some(Cause::Offline) => return t!("This device is offline"),
        Some(Cause::SessionPending) => return t!("This device is still loading its T3 Connect sign-in"),
        Some(Cause::Outdated) => return t!("{host} runs an older T3 Code", host = host),
        Some(Cause::Failure(cause)) => match cause {
            F::ApprovalPending => return t!("{host} is waiting for someone to approve this device", host = host),
            F::ClockSkew => return t!("This device's clock is off"),
            F::NotLinked => return t!("{host} is not linked to your T3 Connect account", host = host),
            F::RelayError => return t!("T3 Connect returned an error"),
            F::TunnelPending => return t!("{host}'s tunnel is still being set up", host = host),
            F::TunnelNotEnabled => return t!("T3 Connect is not turned on for {host}", host = host),
            F::TunnelDown => return t!("{host}'s tunnel is offline", host = host),
            F::OriginUnreachable => return t!("Tunnel works, but T3 Code on {host} did not answer", host = host),
            F::InvalidResponse => return t!("T3 Code on {host} sent an unexpected answer", host = host),
            F::Dns => return t!("{host}'s tunnel address did not resolve", host = host),
            F::Tls => return t!("Secure connection to {host}'s tunnel failed", host = host),
            F::Refused => return t!("{host}'s tunnel refused the connection", host = host),
            F::Timeout => return t!("{host} did not answer in time", host = host),
            F::Unknown => {}
        },
        None => {}
    }
    match located.hop {
        ConnectionHop::Device => t!("This device could not prepare the connection"),
        ConnectionHop::Relay if is_auth(failure) => t!("T3 Connect did not accept this device's sign-in"),
        ConnectionHop::Relay => t!("This device can't reach T3 Connect"),
        ConnectionHop::Tunnel if failure.is_some_and(is_unreachable) => {
            t!("This device can't reach {host}'s tunnel", host = host)
        }
        ConnectionHop::Tunnel => t!("T3 Connect could not reach {host}'s tunnel", host = host),
        ConnectionHop::Server
            if failure.is_some_and(|f| {
                matches!(f.kind, ConnectionErrorKind::Blocked(BlockedReason::Authentication | BlockedReason::Permission))
            }) =>
        {
            t!("T3 Code on {host} did not accept this device", host = host)
        }
        ConnectionHop::Server => t!("T3 Code on {host} did not answer", host = host),
    }
}

fn steps(host: &str, located: &Located, failure: Option<&ConnectionError>) -> Vec<SharedString> {
    use FailureCause as F;
    let check_host = || t!("On {host}, check Settings > Sharing & access or run `t3 connect status`.", host = host);
    match located.cause {
        Some(Cause::Offline) => return vec![t!("Connect this device to the internet. T3 Code reconnects on its own.")],
        Some(Cause::SessionPending) => {
            return vec![
                t!("Wait a moment. T3 Code retries on its own."),
                t!("If it keeps waiting, reload or restart T3 Code on this device."),
            ];
        }
        Some(Cause::Outdated) => return vec![t!("Update T3 Code on {host}.", host = host)],
        Some(Cause::Failure(cause)) => match cause {
            F::ApprovalPending => {
                let code = failure.and_then(|f| f.verification_code.clone());
                let first = match &code {
                    Some(code) => t!(
                        "On a device already connected to {host}, open Settings > Sharing & access and approve the device showing {code}.",
                        host = host,
                        code = code
                    ),
                    None => t!(
                        "On a device already connected to {host}, open Settings > Sharing & access and approve this device.",
                        host = host
                    ),
                };
                let code = code.unwrap_or_else(|| "<code>".into());
                return vec![first, t!("Or run `t3 auth connect-device approve {code}` on {host}.", code = code, host = host)];
            }
            F::ClockSkew => {
                return vec![t!("Turn on automatic date and time on this device and on {host}, then retry.", host = host)];
            }
            F::NotLinked => {
                return vec![
                    t!("Check that this device is signed in to the same T3 Connect account as the host."),
                    t!("On {host}, open Settings > Sharing & access and turn T3 Connect on.", host = host),
                ];
            }
            F::TunnelPending => return vec![t!("Wait a minute. T3 Code retries on its own."), check_host()],
            F::RelayError => {
                return vec![
                    t!("Wait a few minutes. T3 Code retries on its own."),
                    t!("If it keeps failing, copy the diagnostics and report it."),
                ];
            }
            F::TunnelNotEnabled => {
                return vec![t!("On {host}, open Settings > Sharing & access and turn T3 Connect on.", host = host)];
            }
            F::TunnelDown => {
                return vec![
                    t!("Make sure {host} is awake and T3 Code is running on it.", host = host),
                    check_host(),
                    t!(
                        "A VPN or Tailscale exit node on {host} can block the tunnel (port 7844). Turn it off or allow the tunnel.",
                        host = host
                    ),
                ];
            }
            F::OriginUnreachable | F::InvalidResponse => {
                return vec![t!("Open or restart T3 Code on {host}.", host = host), check_host()];
            }
            F::Dns | F::Tls | F::Refused | F::Timeout | F::Unknown => {
                return vec![t!("Make sure {host} is awake and online.", host = host), check_host()];
            }
        },
        None => {}
    }
    match located.hop {
        ConnectionHop::Device => vec![
            t!("Restart T3 Code on this device."),
            t!("If it keeps failing, remove {host} from this device and add it again.", host = host),
        ],
        ConnectionHop::Relay if is_auth(failure) => vec![t!("Sign out of T3 Connect and sign in again.")],
        ConnectionHop::Relay => vec![
            t!("Check this device's internet connection."),
            t!("A VPN, firewall, or ad blocker on this device can block T3 Connect."),
        ],
        ConnectionHop::Tunnel if failure.is_some_and(is_unreachable) => vec![
            t!("A VPN, firewall, or DNS filter on this device can block the tunnel."),
            t!("Try another network, or add a direct route such as a Tailscale address."),
        ],
        ConnectionHop::Tunnel => vec![t!("Make sure {host} is awake and online.", host = host), check_host()],
        ConnectionHop::Server if is_auth(failure) => vec![t!(
            "This device's sign-in to {host} expired or was revoked. Pair it again with a new pairing link from {host}.",
            host = host
        )],
        ConnectionHop::Server => vec![
            t!("Make sure T3 Code is running on {host}.", host = host),
            t!("Retry. If it keeps failing, copy the diagnostics."),
        ],
    }
}

fn hop_label(hop: ConnectionHop, host: &str) -> SharedString {
    match hop {
        ConnectionHop::Device => t!("This device"),
        ConnectionHop::Relay => T3_CONNECT_NAME.into(),
        ConnectionHop::Tunnel => t!("{host} tunnel", host = host),
        ConnectionHop::Server => t!("T3 Code on {host}", host = host),
    }
}

/// `useConnectionDiagnosis`: what failed, or `None` while connected, off,
/// unsupported, or not failed.
pub fn diagnose(environment: &EnvironmentState) -> Option<Diagnosis> {
    let phase = present_phase(environment);
    if !environment.entry.enabled || matches!(phase, PresentedPhase::Connected | PresentedPhase::Unsupported) {
        return None;
    }
    let status = environment.status.as_ref()?;
    let routes = &environment.entry.routes;
    let relay_only = routes.len() == 1 && route_kind(&routes[0]) == RouteKind::Relay;
    let located = locate(status, relay_only)?;
    let failure = status.last_failure.as_ref();
    let host = environment.entry.label.as_str();
    let path: &[ConnectionHop] = if relay_only || failure.and_then(|f| f.via) == Some(ConnectionVia::Relay) {
        &[ConnectionHop::Device, ConnectionHop::Relay, ConnectionHop::Tunnel, ConnectionHop::Server]
    } else {
        &[ConnectionHop::Device, ConnectionHop::Server]
    };
    let failed_at = path.iter().position(|hop| *hop == located.hop).unwrap_or(path.len() - 1);
    let hops = path
        .iter()
        .enumerate()
        .map(|(ix, hop)| {
            let result = match ix.cmp(&failed_at) {
                std::cmp::Ordering::Less if located.guessed => HopResult::Unchecked,
                std::cmp::Ordering::Less => HopResult::Ok,
                std::cmp::Ordering::Equal => HopResult::Failed,
                std::cmp::Ordering::Greater => HopResult::Unchecked,
            };
            (hop_label(*hop, host), result)
        })
        .collect();
    let mut facts = Vec::new();
    if let Some(Cause::Failure(cause)) = located.cause {
        facts.push((t!("Cause", context = "diagnostics field label"), SharedString::from(format!("{cause:?}"))));
    }
    if let Some(status) = failure.and_then(|f| f.http_status) {
        facts.push((t!("HTTP status"), status.to_string().into()));
    }
    if status.attempt > 0 {
        facts.push((t!("Attempts", context = "diagnostics field label"), status.attempt.to_string().into()));
    }
    facts.push((
        t!("Route", context = "diagnostics field label"),
        if relay_only { T3_CONNECT_NAME.into() } else { t!("Direct", context = "connection route") },
    ));
    facts.push((t!("App version"), env!("CARGO_PKG_VERSION").into()));
    if let Some(version) = &environment.server_version {
        facts.push((t!("Host version"), version.clone()));
    }
    if let Some(failure) = failure {
        facts.push((t!("Error", context = "diagnostics field label"), failure.detail.clone().into()));
        if let Some(trace) = &failure.trace_id {
            facts.push((t!("Trace ID"), trace.clone().into()));
        }
    }
    Some(Diagnosis { headline: headline(host, &located, failure), hops, steps: steps(host, &located, failure), facts })
}

/// `connectionDiagnosisText`: the plain-text report "Copy diagnostics" puts on the clipboard.
pub fn diagnosis_text(host: &str, diagnosis: &Diagnosis) -> String {
    let mut text = format!("{host}: {}\n", diagnosis.headline);
    for (label, result) in &diagnosis.hops {
        let mark = match result {
            HopResult::Ok => "✓",
            HopResult::Failed => "✕",
            HopResult::Unchecked => "–",
        };
        text.push_str(&format!("{mark} {label}\n"));
    }
    for (label, value) in &diagnosis.facts {
        text.push_str(&format!("{label}: {value}\n"));
    }
    text
}
