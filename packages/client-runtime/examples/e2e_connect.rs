//! End-to-end check against a real T3 Code server.
//!
//! Usage: `e2e_connect <out_dir>` with two unused pairing URLs (tokens are
//! single-use) in `T3_PAIRING_URL` and `T3_BEARER_PAIRING_URL`.
//! Pairs (DPoP when the host renews tokens), connects through the registry
//! and supervisor, runs `subscribeServerConfig` to its first snapshot,
//! subscribes to the shell, makes unary calls, forces a socket drop and
//! waits for the reconnect, then pairs again as a bearer client. Writes
//! `<out_dir>/e2e-transcript.txt` with every frame seen.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use t3code_client_runtime::authorization::remote::{
    ConnectionMethod, RequestCredential, SubjectToken, exchange_token, resolve_socket_url,
};
use t3code_client_runtime::authorization::{DpopRenewal, HttpAuthorization, RemoteAuthorization};
use t3code_client_runtime::connection::compatibility::append_orchestration_protocol;
use t3code_client_runtime::connection::credential_store::CredentialStore;
use t3code_client_runtime::connection::onboarding::{pair, parse_pairing_url};
use t3code_client_runtime::connection::{
    ConnectionContext, ConnectionCredential, ConnectionPhase, ConnectionRegistry, ConnectionStatus, SupervisorHandle,
};
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore, SecretStore};
use t3code_client_runtime::relay::DpopKey;
use t3code_client_runtime::rpc::{FrameDirection, FrameTap, RpcClient, RpcError, RpcSessionOptions};
use t3code_contracts::rpc::methods::{OrchestrationSubscribeShell as SubscribeShell, ServerProbe};
use t3code_contracts::rpc::{RpcMethod, RpcStream};
use t3code_contracts::wire::Empty;
use t3code_contracts::{OrchestrationV2ShellStreamItem, OrchestrationV2SubscribeShellInput as SubscribeShellInput};

/// A method the server does not have, to see a `Die` exit on the wire.
struct NoSuchMethod;

impl RpcMethod for NoSuchMethod {
    const TAG: &'static str = "no.such.method";
    const SCOPE: t3code_contracts::AuthEnvironmentScope = <ServerProbe as RpcMethod>::SCOPE;
    const CLIENT_GUARDED: bool = false;
    type Payload = Empty;
    type Success = Empty;
    type Error = Empty;
}

/// The shell stream with a payload that fails the server's schema.
struct ShellWithBadPayload;

impl RpcMethod for ShellWithBadPayload {
    const TAG: &'static str = "orchestration.subscribeShell";
    const SCOPE: t3code_contracts::AuthEnvironmentScope = <SubscribeShell as RpcMethod>::SCOPE;
    const CLIENT_GUARDED: bool = false;
    type Payload = serde_json::Value;
    type Success = serde_json::Value;
    type Error = serde_json::Value;
}

impl RpcStream for ShellWithBadPayload {}

struct Transcript {
    started: Instant,
    lines: Mutex<Vec<String>>,
    kinds: Mutex<BTreeMap<String, usize>>,
}

impl Transcript {
    fn note(&self, text: &str) {
        let line = format!("{:>9.3}s  ## {text}", self.started.elapsed().as_secs_f64());
        println!("{line}");
        self.lines.lock().unwrap().push(line);
    }

    fn frame(&self, direction: FrameDirection, text: &str) {
        let arrow = arrow(direction);
        *self.kinds.lock().unwrap().entry(frame_kind(direction, text)).or_default() += 1;
        let shown: String = if text.chars().count() > 360 {
            let head: String = text.chars().take(360).collect();
            format!("{head}… ({} bytes)", text.len())
        } else {
            text.to_owned()
        };
        let shown = shown.replace('\u{1e}', "\\u001e");
        self.lines
            .lock()
            .unwrap()
            .push(format!("{:>9.3}s  {arrow} {shown}", self.started.elapsed().as_secs_f64()));
    }
}

fn arrow(direction: FrameDirection) -> &'static str {
    match direction {
        FrameDirection::Sent => "->",
        FrameDirection::Received => "<-",
    }
}

/// `"<- Chunk snapshot"`, `"-> Ack"`, `"<- Exit Failure/Interrupt"`, …
fn frame_kind(direction: FrameDirection, text: &str) -> String {
    let arrow = arrow(direction);
    if text.starts_with('\u{1e}') {
        return format!("{arrow} split part (\\u001e prefix)");
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return format!("{arrow} last split part (no prefix)");
    };
    let envelopes = match &value {
        serde_json::Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    let kinds: Vec<String> = envelopes
        .iter()
        .map(|envelope| {
            let tag = envelope["_tag"].as_str().unwrap_or("?");
            let detail = match tag {
                "Request" => envelope["tag"].as_str().map(str::to_owned),
                "Exit" => {
                    let exit = &envelope["exit"];
                    match exit["_tag"].as_str() {
                        Some("Failure") => Some(format!(
                            "Failure/{}",
                            exit["cause"]
                                .as_array()
                                .map(|c| c.iter().filter_map(|r| r["_tag"].as_str()).collect::<Vec<_>>().join("+"))
                                .unwrap_or_else(|| "non-array cause".into())
                        )),
                        other => other.map(str::to_owned),
                    }
                }
                "Chunk" => envelope["values"].as_array().map(|values| {
                    values
                        .iter()
                        .map(|v| v["type"].as_str().or(v["kind"].as_str()).unwrap_or("?"))
                        .collect::<Vec<_>>()
                        .join(",")
                }),
                _ => None,
            };
            match detail {
                Some(detail) => format!("{tag} {detail}"),
                None => tag.to_owned(),
            }
        })
        .collect();
    let batched = if value.is_array() { " (array frame)" } else { "" };
    format!("{arrow} {}{batched}", kinds.join(" | "))
}

async fn wait_for(handle: &SupervisorHandle, what: &str, done: impl Fn(&ConnectionStatus) -> bool) -> ConnectionStatus {
    let mut status = handle.status.clone();
    let wait = async {
        loop {
            {
                let current = status.borrow_and_update();
                if done(&current) {
                    return current.clone();
                }
            }
            status.changed().await.expect("supervisor alive");
        }
    };
    tokio::time::timeout(Duration::from_secs(60), wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}: {:?}", handle.status.borrow()))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let out_dir = PathBuf::from(std::env::args().nth(1).expect("usage: e2e_connect <out_dir>"));
    let pairing_url = std::env::var("T3_PAIRING_URL").expect("T3_PAIRING_URL is set");
    let bearer_pairing_url = std::env::var("T3_BEARER_PAIRING_URL").expect("T3_BEARER_PAIRING_URL is set");
    let data_dir = out_dir.join("client-data");
    _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir)?;

    let transcript = Arc::new(Transcript { started: Instant::now(), lines: Mutex::default(), kinds: Mutex::default() });
    let tap: FrameTap = {
        let transcript = transcript.clone();
        Arc::new(move |direction, text| transcript.frame(direction, text))
    };
    let result = run(&transcript, tap, &pairing_url, &bearer_pairing_url, &data_dir).await;
    if let Err(error) = &result {
        transcript.note(&format!("FAILED: {error:#}"));
    }
    write_transcript(&transcript, &out_dir.join("e2e-transcript.txt"))?;
    result
}

async fn run(
    transcript: &Transcript,
    tap: FrameTap,
    pairing_url: &str,
    bearer_pairing_url: &str,
    data_dir: &std::path::Path,
) -> anyhow::Result<()> {
    // A file store under the run's own directory keeps the e2e off the user's keychain.
    let secrets: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(data_dir.join("secrets.json")));
    let key = DpopKey::load_or_create(secrets.as_ref())?;
    let presentation = ClientPresentation { label: "t3code-gpui e2e".into(), ..ClientPresentation::default() };
    let auth = Arc::new(RemoteAuthorization::new(
        presentation.clone(),
        key.clone(),
        CredentialStore::new(secrets.clone()),
        None,
    ));
    transcript.note(&format!("DPoP key thumbprint {} (verification code {})", key.thumbprint(), key.verification_code()));

    // 1. Pair: descriptor first, then the one-time credential.
    let request = parse_pairing_url(pairing_url)?;
    transcript.note(&format!("pairing with {}", request.http_base_url));
    let paired = pair(&auth, &request, None).await?;
    let credential_kind = match &paired.credential {
        ConnectionCredential::Bearer { .. } => "bearer",
        ConnectionCredential::Dpop { .. } => "DPoP",
    };
    transcript.note(&format!("paired environment {} ({}) with a {credential_kind} credential", paired.environment_id, paired.label));

    // 2. Registry + supervisor: connect, which runs subscribeServerConfig to its first snapshot.
    let context = Arc::new(ConnectionContext {
        auth: auth.clone(),
        session_options: RpcSessionOptions::default(),
        frame_tap: Some(tap.clone()),
    });
    let registry = ConnectionRegistry::open(context, data_dir).await?;
    registry.save_paired(&paired).await?;
    let handle = registry.supervisor(&paired.environment_id).expect("enabled environment has a supervisor");
    let connected = wait_for(&handle, "first connect", |s| s.phase == ConnectionPhase::Connected).await;
    transcript.note(&format!("connected: generation {} over route {:?}", connected.generation, connected.active_route_id));
    let session = handle.session.borrow().clone().expect("connected has a session");
    transcript.note(&format!(
        "initial config snapshot: {} bytes, {} direct endpoints reported",
        serde_json::to_string(&*session.initial_config).map_or(0, |json| json.len()),
        session.direct_endpoints.len()
    ));
    let client = session.client.clone();

    // 3. Shell stream until the server says it caught up.
    let mut shell = client.subscribe::<SubscribeShell>(SubscribeShellInput { request_completion_marker: Some(true), ..Default::default() });
    let mut kinds: Vec<String> = Vec::new();
    while !kinds.iter().any(|kind| kind == "synchronized") {
        let chunk = tokio::time::timeout(Duration::from_secs(20), shell.next()).await?.expect("shell stream open")?;
        kinds.extend(chunk.into_iter().map(|item| {
            match item {
                OrchestrationV2ShellStreamItem::Snapshot(_) => "snapshot",
                OrchestrationV2ShellStreamItem::Synchronized(_) => "synchronized",
                _ => "delta",
            }
            .to_owned()
        }));
    }
    transcript.note(&format!("shell stream items: {kinds:?}"));
    drop(shell);
    transcript.note("dropped the shell subscription (expect an Interrupt frame)");

    // 4. Unary calls: success, unknown method (Die), schema-invalid payload (Die).
    client.call::<ServerProbe>(Empty).await?;
    transcript.note("server.probe succeeded");
    match client.call::<NoSuchMethod>(Empty).await {
        Err(RpcError::Defect(message)) => transcript.note(&format!("no.such.method -> defect: {message}")),
        other => anyhow::bail!("unexpected result for unknown method: {other:?}"),
    }
    let mut bad = client.subscribe::<ShellWithBadPayload>(serde_json::json!({ "afterSequence": "not a number" }));
    match tokio::time::timeout(Duration::from_secs(10), bad.next()).await? {
        Some(Err(RpcError::Defect(message))) => {
            transcript.note(&format!("bad payload -> defect: {}", message.lines().next().unwrap_or_default()))
        }
        other => transcript.note(&format!("bad payload -> {other:?}")),
    }

    // 5. Force a socket drop; the supervisor reconnects after ~250 ms.
    transcript.note("closing the socket to force a reconnect");
    client.close();
    drop(client);
    drop(session);
    let lost = wait_for(&handle, "loss", |s| s.phase != ConnectionPhase::Connected).await;
    transcript.note(&format!(
        "after drop: phase {:?}, last failure {:?}",
        lost.phase,
        lost.last_failure.as_ref().map(|f| f.detail.clone())
    ));
    let reconnected = wait_for(&handle, "reconnect", |s| s.phase == ConnectionPhase::Connected && s.generation >= 2).await;
    transcript.note(&format!("reconnected: generation {}", reconnected.generation));
    let session = handle.session.borrow().clone().expect("reconnected has a session");
    session.client.call::<ServerProbe>(Empty).await?;
    transcript.note("server.probe succeeded on the new socket");

    // 6. DPoP renewal with the host, then a ticket minted with the renewed token.
    let current = handle.http_authorization.borrow().clone();
    if let Some(HttpAuthorization::Dpop { access_token, renewal, .. }) = current {
        let DpopRenewal::Direct { connection_id, http_base_url } = renewal else {
            anyhow::bail!("expected a direct DPoP session");
        };
        let (renewed, expires) = auth.direct_dpop_token(&connection_id, &http_base_url, Some(&access_token)).await?;
        anyhow::ensure!(renewed != *access_token, "renewal returned the same token");
        transcript.note(&format!("renewed the DPoP token with the host (expires in {}s)", (expires - now_ms()) / 1000));
        let url = resolve_socket_url(
            &http_base_url,
            &request.ws_base_url,
            RequestCredential::Dpop { access_token: &renewed, key: &key },
            &presentation,
            ConnectionMethod::Direct,
        )
        .await?;
        transcript.note(&format!("minted an rpc ticket with the renewed token ({} query params)", url.query_pairs().count()));
    }

    // 7. Bearer pairing path, connecting a bare RpcClient.
    let request = parse_pairing_url(bearer_pairing_url)?;
    let bearer = exchange_token(&request.http_base_url, SubjectToken::Bootstrap(&request.credential), None, &presentation).await?;
    transcript.note(&format!(
        "bearer exchange: token_type {:?}, expires_in {}s, scope {:?}",
        bearer.token_type, bearer.expires_in, bearer.scope
    ));
    let mut url = resolve_socket_url(
        &request.http_base_url,
        &request.ws_base_url,
        RequestCredential::Bearer(&bearer.access_token),
        &presentation,
        ConnectionMethod::Direct,
    )
    .await?;
    append_orchestration_protocol(&mut url);
    url.query_pairs_mut().append_pair("frameParts", "1");
    let bearer_client = RpcClient::connect(url.as_str(), Some(tap.clone())).await?;
    bearer_client.call::<ServerProbe>(Empty).await?;
    transcript.note("bearer socket: server.probe succeeded");
    bearer_client.close();

    // 8. A socket without orchestrationProtocol is refused with 426.
    let mut no_protocol = resolve_socket_url(
        &request.http_base_url,
        &request.ws_base_url,
        RequestCredential::Bearer(&bearer.access_token),
        &presentation,
        ConnectionMethod::Direct,
    )
    .await?;
    no_protocol.query_pairs_mut().append_pair("frameParts", "1");
    match RpcClient::connect(no_protocol.as_str(), None).await {
        Err(reason) => transcript.note(&format!("socket without orchestrationProtocol: {reason}")),
        Ok(_) => anyhow::bail!("server accepted a socket without orchestrationProtocol"),
    }

    // 9. Stay connected past two ping intervals so Ping/Pong shows up.
    tokio::time::sleep(Duration::from_secs(11)).await;
    let still = handle.status.borrow().clone();
    anyhow::ensure!(still.phase == ConnectionPhase::Connected, "lost the session while idle");
    transcript.note(&format!("idle 11 s: still connected, socket saw {} inbound frames", session.client.inbound_frames()));

    // 10. Route engine: a second address for the same server, served by a
    // local TCP proxy this test switches on and off.
    drop(session);
    route_engine(transcript, &registry, &auth, &paired, &handle).await?;

    let catalog = std::fs::read_to_string(data_dir.join("connection-catalog.json"))?;
    transcript.note(&format!("saved catalog:\n{catalog}"));
    Ok(())
}

/// A TCP forwarder to the server, so one server answers at a second address.
struct Proxy {
    port: u16,
    target: std::net::SocketAddr,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Proxy {
    async fn start(&mut self) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", self.port)).await?;
        let target = self.target;
        self.task = Some(tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            while let Ok((mut inbound, _)) = listener.accept().await {
                connections.spawn(async move {
                    if let Ok(mut outbound) = tokio::net::TcpStream::connect(target).await {
                        _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        }));
        Ok(())
    }

    /// Stops listening and cuts every forwarded connection, like a network drop.
    fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn route_engine(
    transcript: &Transcript,
    registry: &ConnectionRegistry,
    auth: &RemoteAuthorization,
    paired: &t3code_client_runtime::connection::onboarding::PairedRoute,
    handle: &SupervisorHandle,
) -> anyhow::Result<()> {
    use t3code_client_runtime::connection::catalog::{ConnectionProfile, TargetRoute};
    use t3code_client_runtime::connection::routes::route_id;

    let direct_id = route_id(&paired.route.target).to_owned();
    let Some(ConnectionProfile::Bearer(direct_profile)) = &paired.route.profile else {
        anyhow::bail!("paired route has a bearer profile");
    };
    let target = std::net::SocketAddr::new(
        std::net::Ipv4Addr::LOCALHOST.into(),
        direct_profile.http_base_url.port().expect("test server URL has a port"),
    );
    let mut proxy = Proxy { port: 47914, target, task: None };
    let proxy_base = url::Url::parse(&format!("http://127.0.0.1:{}/", proxy.port))?;
    let proxy_id = format!("bearer:{}:{}", paired.environment_id, proxy_base.origin().ascii_serialization());
    let mut proxy_route = paired.route.clone();
    proxy_route.target.route = TargetRoute::Bearer { connection_id: proxy_id.clone() };
    let mut profile = direct_profile.clone();
    profile.connection_id = proxy_id.clone();
    profile.http_base_url = proxy_base.clone();
    profile.ws_base_url = url::Url::parse(&format!("ws://127.0.0.1:{}/", proxy.port))?;
    proxy_route.profile = Some(ConnectionProfile::Bearer(profile));
    // Same session credential on both addresses: a DPoP token is not origin-bound.
    let credential = auth.credentials().get(&direct_id).await?.expect("paired credential saved");
    let proxy_paired = t3code_client_runtime::connection::onboarding::PairedRoute {
        route: proxy_route,
        credential,
        ..paired.clone()
    };
    registry.save_paired(&proxy_paired).await?;
    registry.reorder_routes(&paired.environment_id, &[proxy_id.clone(), direct_id.clone()]).await?;
    transcript.note(&format!("routes now [{proxy_id} (proxy, off), {direct_id}]"));

    // a) Preferred route silent: it misses the 500 ms head start, the next route connects.
    // The previous session lived under 30 s, so this loss climbs the backoff ladder (2-4 s).
    let generation = handle.status.borrow().generation;
    handle.session.borrow().as_ref().expect("connected").client.close();
    let started = Instant::now();
    let status = wait_for(handle, "fallback connect", |s| {
        s.phase == ConnectionPhase::Connected && s.generation > generation
    })
    .await;
    transcript.note(&format!(
        "preferred route off: connected over {:?} after {} ms",
        status.active_route_id,
        started.elapsed().as_millis()
    ));
    anyhow::ensure!(status.active_route_id.as_deref() == Some(direct_id.as_str()), "fell back to the live route");

    // b) Preferred route returns: the 60 s better-route check moves the session to it.
    proxy.start().await?;
    let started = Instant::now();
    transcript.note("proxy on; waiting for the 60 s better-route check");
    let mut status = handle.status.clone();
    let promoted = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            {
                let current = status.borrow_and_update();
                if current.phase == ConnectionPhase::Connected && current.active_route_id.as_deref() == Some(proxy_id.as_str()) {
                    return current.clone();
                }
            }
            status.changed().await.expect("supervisor alive");
        }
    })
    .await?;
    transcript.note(&format!(
        "promoted to {:?} after {} s (generation {})",
        promoted.active_route_id,
        started.elapsed().as_secs(),
        promoted.generation
    ));

    // c) The preferred route drops mid-session: reconnect falls back to the live route.
    proxy.stop();
    let started = Instant::now();
    let status = wait_for(handle, "reconnect after proxy cut", |s| {
        s.phase == ConnectionPhase::Connected && s.generation > promoted.generation
    })
    .await;
    transcript.note(&format!(
        "proxy cut mid-session: reconnected over {:?} after {} ms",
        status.active_route_id,
        started.elapsed().as_millis()
    ));
    anyhow::ensure!(status.active_route_id.as_deref() == Some(direct_id.as_str()), "fell back after the cut");
    Ok(())
}

fn now_ms() -> i64 {
    t3code_client_runtime::authorization::remote::now_epoch_ms()
}

fn write_transcript(transcript: &Transcript, path: &std::path::Path) -> anyhow::Result<()> {
    let mut text = String::from("# t3code-gpui client-runtime e2e transcript\n\n## Frame kinds seen\n\n");
    for (kind, count) in transcript.kinds.lock().unwrap().iter() {
        writeln!(text, "{count:>6}  {kind}")?;
    }
    text.push_str("\n## Timeline (frames truncated to 360 chars)\n\n");
    for line in transcript.lines.lock().unwrap().iter() {
        text.push_str(line);
        text.push('\n');
    }
    std::fs::write(path, text)?;
    println!("transcript written to {}", path.display());
    Ok(())
}
