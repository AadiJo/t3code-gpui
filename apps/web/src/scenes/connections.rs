//! Settings on the Environments page with fixture environments: connected,
//! connecting, waiting for device approval, failing, and switched off; the
//! Add environment dialog; T3 Connect signed in and out; and the empty
//! main pane.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use t3code_client_runtime::connection::catalog::{
    BearerProfile, CatalogEntry, ConnectionProfile, ConnectionRoute, ConnectionTarget, ProfileNetwork, TargetRoute,
};
use t3code_client_runtime::connection::model::{
    AttemptStage, BlockedReason, ConnectionHop, ConnectionVia, FailureCause, NetworkStatus, TransientReason,
};
use t3code_client_runtime::connection::{ConnectionError, ConnectionPhase, ConnectionStatus};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_client_runtime::relay::{CloudAccount, CloudAccountState, RelayEndpoint, RelayEnvironmentRecord};
use url::Url;

use crate::AppRoot;
use crate::components::hosted_empty_state::HostedEmptyState;
use crate::components::settings::settings_search::SettingsPath;
use crate::components::ui::dialog::Modal;
use crate::connection::{
    CloudState, ConnectionsState, EnvironmentState, RelayAvailability, RelayDiscovery, RelayEnvironment,
};
use crate::routes::settings::SettingsRoute;

fn bearer(env: &str, label: &str, url: &str, tailscale: bool) -> ConnectionRoute {
    let http = Url::parse(url).expect("fixture url");
    let mut ws = http.clone();
    ws.set_scheme(if http.scheme() == "https" { "wss" } else { "ws" }).ok();
    let connection_id = format!("bearer:{env}:{}", http.origin().ascii_serialization());
    ConnectionRoute {
        target: ConnectionTarget {
            environment_id: env.into(),
            label: label.into(),
            route: TargetRoute::Bearer { connection_id: connection_id.clone() },
        },
        profile: Some(ConnectionProfile::Bearer(BearerProfile {
            connection_id,
            environment_id: env.into(),
            label: label.into(),
            http_base_url: http,
            ws_base_url: ws,
            learned: false,
            authorization: None,
            network: tailscale.then_some(ProfileNetwork::Tailscale),
        })),
    }
}

fn relay(env: &str, label: &str) -> ConnectionRoute {
    ConnectionRoute {
        target: ConnectionTarget { environment_id: env.into(), label: label.into(), route: TargetRoute::Relay },
        profile: None,
    }
}

fn entry(env: &str, label: &str, routes: Vec<ConnectionRoute>, enabled: bool) -> CatalogEntry {
    CatalogEntry { environment_id: env.into(), label: label.into(), routes: routes.into(), enabled }
}

fn status(phase: ConnectionPhase) -> ConnectionStatus {
    ConnectionStatus { desired: true, network: NetworkStatus::Online, phase, ..ConnectionStatus::available() }
}

/// Five environments, one per state the rows draw.
pub fn fixture_environments() -> Vec<EnvironmentState> {
    let studio_lan = bearer("env-studio", "Studio Mac", "http://192.168.1.20:3773/", false);
    let studio_tailnet = bearer("env-studio", "Studio Mac", "https://studio.example-tailnet.ts.net/", true);
    let studio_active = t3code_client_runtime::connection::routes::route_id(&studio_tailnet.target).to_owned();
    let connected = EnvironmentState {
        entry: entry("env-studio", "Studio Mac", vec![studio_lan, studio_tailnet], true),
        status: Some(ConnectionStatus {
            generation: 3,
            active_route_id: Some(studio_active),
            ..status(ConnectionPhase::Connected)
        }),
        latency: Some(Duration::from_millis(23)),
        server_version: Some("0.1.11".into()),
    };
    let connecting = EnvironmentState {
        entry: entry("env-build", "Build box", vec![bearer("env-build", "Build box", "https://build.example.com/", false)], true),
        status: Some(ConnectionStatus { attempt: 1, stage: Some(AttemptStage::Opening), ..status(ConnectionPhase::Connecting) }),
        latency: None,
        server_version: None,
    };
    let mut approval = ConnectionError::transient(
        TransientReason::ApprovalPending,
        "Approve this device on Office PC. It shows code 7KQ4-M2XR.",
    )
    .at(ConnectionHop::Server)
    .via(ConnectionVia::Relay)
    .cause(FailureCause::ApprovalPending);
    approval.verification_code = Some("7KQ4-M2XR".into());
    let pending = EnvironmentState {
        entry: entry("env-office", "Office PC", vec![relay("env-office", "Office PC")], true),
        status: Some(ConnectionStatus {
            attempt: 6,
            last_failure: Some(approval),
            retry_at: Some(SystemTime::now() + Duration::from_millis(3_400)),
            ..status(ConnectionPhase::Backoff)
        }),
        latency: None,
        server_version: None,
    };
    let refused = ConnectionError::blocked(BlockedReason::Authentication, "The environment rejected this device's credential.")
        .at(ConnectionHop::Server)
        .via(ConnectionVia::Direct);
    let failing = EnvironmentState {
        entry: entry("env-lab", "Lab server", vec![bearer("env-lab", "Lab server", "http://10.0.4.7:3773/", false)], true),
        status: Some(ConnectionStatus { attempt: 2, last_failure: Some(refused), ..status(ConnectionPhase::Blocked) }),
        latency: None,
        server_version: None,
    };
    let disabled = EnvironmentState {
        entry: entry("env-old", "Old laptop", vec![bearer("env-old", "Old laptop", "http://192.168.1.31:3773/", false)], false),
        status: None,
        latency: None,
        server_version: None,
    };
    vec![connected, connecting, pending, failing, disabled]
}

fn account() -> CloudAccount {
    CloudAccount {
        user_id: "user_fixture".into(),
        session_id: "sess_fixture".into(),
        email: Some("dev@example.com".into()),
        name: None,
    }
}

fn discovered() -> RelayDiscovery {
    let endpoint = RelayEndpoint {
        http_base_url: Url::parse("https://tunnel.example.com/").unwrap(),
        ws_base_url: Url::parse("wss://tunnel.example.com/").unwrap(),
        provider_kind: "cloudflare_tunnel".into(),
    };
    RelayDiscovery {
        refreshing: false,
        error: None,
        environments: vec![RelayEnvironment {
            record: RelayEnvironmentRecord {
                environment_id: EnvironmentId(Arc::from("env-home")),
                label: "Home server".into(),
                endpoint,
                linked_at: "2026-11-01T00:00:00Z".into(),
            },
            availability: RelayAvailability::Online,
        }],
    }
}

fn state(cx: &mut App, environments: Vec<EnvironmentState>, account: CloudAccountState) -> Entity<ConnectionsState> {
    let discovery = if matches!(account, CloudAccountState::SignedIn(_)) { discovered() } else { RelayDiscovery::default() };
    cx.new(|_| ConnectionsState {
        environments,
        cloud: Some(CloudState { account, discovery }),
        verification_code: "7KQ4-M2XR".into(),
        electron_import_available: true,
    })
}

/// The app shell under the open settings modal.
struct SettingsScene {
    app: Entity<AppRoot>,
    settings: Entity<SettingsRoute>,
}

impl Render for SettingsScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.app.clone()).child(Modal::dialog("settings-route", self.settings.clone(), |_, _| {}))
    }
}

/// What extra a scene opens on the page.
#[derive(Clone, Copy)]
enum Variant {
    Rows,
    Add,
    SignedOut,
    SignIn,
}

fn build_variant(variant: Variant, window: &mut Window, cx: &mut App) -> AnyView {
    let account = match variant {
        Variant::SignedOut | Variant::SignIn => CloudAccountState::SignedOut,
        _ => CloudAccountState::SignedIn(account()),
    };
    let state = state(cx, fixture_environments(), account);
    let app = cx.new(|cx| AppRoot::new(window, cx));
    let settings = cx.new(|cx| SettingsRoute::with_state(SettingsPath::Connections, state, window, cx));
    let page = settings.read(cx).environments().clone();
    page.update(cx, |page, cx| {
        match variant {
            Variant::Rows => {
                // Routes on the connected row, diagnosis details on the approval row.
                page.toggle_for_scene("env-studio", true, false, cx);
                page.toggle_for_scene("env-office", false, true, cx);
            }
            Variant::Add => page.open_add_dialog(None, window, cx),
            Variant::SignedOut => page.toggle_for_scene("env-lab", false, true, cx),
            Variant::SignIn => page.open_sign_in(window, cx),
        }
    });
    cx.new(|_| SettingsScene { app, settings }).into()
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Rows, window, cx)
}

pub fn build_add(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Add, window, cx)
}

pub fn build_signed_out(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::SignedOut, window, cx)
}

pub fn build_sign_in(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::SignIn, window, cx)
}

/// The main pane before any environment is saved.
pub fn build_empty(window: &mut Window, cx: &mut App) -> AnyView {
    let state = state(cx, Vec::new(), CloudAccountState::SignedOut);
    let app = cx.new(|cx| AppRoot::new(window, cx));
    let layout = app.read(cx).layout().clone();
    let (height, left) = {
        let layout = layout.read(cx);
        (layout.platform().topbar_height(), layout.header_content_left())
    };
    let empty = cx.new(|cx| HostedEmptyState::new(state, height, left, cx));
    layout.update(cx, |layout, cx| layout.set_main(empty.into(), cx));
    app.into()
}
