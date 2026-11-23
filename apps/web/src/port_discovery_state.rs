//! Port forwarding and discovered local servers. Ports `portDiscoveryState.ts`,
//! `state/portForwards.ts` and the data side of
//! `CommandPalettePortForwardItems.tsx`; the desktop app's main-process
//! `LoopbackTunnel` is [`LoopbackTunnel`] here.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt as _;
use gpui_kit::{App, ClipboardItem, Global, SharedString};
use t3code_client_runtime::preview::loopback_tunnel::{LoopbackTunnel, PortForward};
use t3code_contracts::{DiscoveredLocalServer, EnvironmentId, SubscribeDiscoveredLocalServersPayload, ThreadId};
use url::Url;

use crate::components::command_palette_logic::PortForwardRequest;
use crate::components::command_palette_lookups::PalettePort;
use crate::components::preview::preview_source::{LivePreviewSource, PreviewSource};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::{CommandError, ConnectionRuntime};
use crate::i18n::t;

/// `CONFIGURED_LOCAL_SERVER_URLS_MAX_ITEMS`.
const MAX_CONFIGURED_URLS: usize = 20;
/// `PREVIEW_URL_MAX_LENGTH`.
const PREVIEW_URL_MAX_LENGTH: usize = 2048;

/// `boundConfiguredLocalServerUrls`: loopback http(s) URLs, deduplicated
/// ignoring the fragment, at most 20.
pub fn bound_configured_local_server_urls(urls: &[String]) -> Vec<String> {
    let mut bounded: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for raw in urls {
        if raw.is_empty() || raw.len() > PREVIEW_URL_MAX_LENGTH || raw.trim().len() != raw.len() {
            continue;
        }
        let Ok(url) = Url::parse(raw) else { continue };
        let loopback = url.host_str().is_some_and(|h| {
            let h = h.trim_start_matches('[').trim_end_matches(']');
            h == "localhost" || h == "::1" || h.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
        });
        if !matches!(url.scheme(), "http" | "https") || !loopback || url.as_str().len() > PREVIEW_URL_MAX_LENGTH {
            continue;
        }
        let mut resource = url.clone();
        resource.set_fragment(None);
        if seen.contains(&resource.to_string()) {
            continue;
        }
        seen.push(resource.to_string());
        bounded.push(url.to_string());
        if bounded.len() >= MAX_CONFIGURED_URLS {
            break;
        }
    }
    bounded
}

/// The servers of `thread` among an environment's discovered ones.
pub fn thread_servers(servers: &[DiscoveredLocalServer], thread: &ThreadId) -> Vec<DiscoveredLocalServer> {
    servers
        .iter()
        .filter(|s| s.terminal.as_ref().is_some_and(|t| &t.thread_id == thread) || s.agent.as_ref().is_some_and(|a| &a.thread_id == thread))
        .cloned()
        .collect()
}

/// `forwardedUrl`: `url` moved to the forward's end on this machine.
pub fn forwarded_url(forward: &PortForward, url: Option<&str>) -> String {
    let mut local = url.and_then(|u| Url::parse(u).ok()).unwrap_or_else(|| Url::parse("http://localhost/").expect("valid"));
    _ = local.set_host(Some("localhost"));
    _ = local.set_port(Some(forward.local_port));
    local.to_string()
}

/// The app's forwards and the active thread's discovered servers.
pub struct PortForwards {
    tunnel: Arc<LoopbackTunnel>,
    forwards: Arc<[PortForward]>,
    /// The active thread and its servers, following `subscribeDiscoveredLocalServers`.
    watched: Option<(EnvironmentId, ThreadId)>,
    servers: Vec<DiscoveredLocalServer>,
    watch_task: Option<gpui_kit::Task<()>>,
}

impl Global for PortForwards {}

impl PortForwards {
    /// Installs the global and follows the tunnel's forwards. Needs [`ConnectionRuntime`].
    pub fn init(cx: &mut App) {
        let tunnel = Arc::new(LoopbackTunnel::default());
        let mut forwards = tunnel.forwards();
        cx.set_global(Self { tunnel, forwards: Arc::from(Vec::new()), watched: None, servers: Vec::new(), watch_task: None });
        cx.spawn(async move |cx| {
            while forwards.changed().await.is_ok() {
                let next = forwards.borrow_and_update().clone();
                cx.update(|cx| {
                    cx.global_mut::<Self>().forwards = next;
                    cx.refresh_windows();
                });
            }
        })
        .detach();
    }

    /// Follows the discovered servers of the active thread; `None` stops.
    /// Only the active thread's servers are listed, so other clients never subscribe to them.
    pub fn watch_thread(thread: Option<(EnvironmentId, ThreadId)>, cx: &mut App) {
        if !cx.has_global::<Self>() || cx.global::<Self>().watched == thread {
            return;
        }
        let source = thread.as_ref().and_then(|(environment, _)| {
            let runtime = ConnectionRuntime::global(cx);
            Some(LivePreviewSource::new(runtime.environment_link(environment)?, runtime.handle().clone()))
        });
        let state = cx.global_mut::<Self>();
        state.watched = thread.clone();
        state.servers.clear();
        state.watch_task = None;
        let (Some(source), Some((_, thread_id))) = (source, thread) else { return };
        let mut stream = source.discovered_servers(SubscribeDiscoveredLocalServersPayload::default());
        let task = cx.spawn(async move |cx| {
            while let Some(list) = stream.next().await {
                let servers = thread_servers(&list.servers, &thread_id);
                cx.update(|cx| cx.global_mut::<Self>().servers = servers);
            }
        });
        cx.global_mut::<Self>().watch_task = Some(task);
    }

    /// The current forwards.
    pub fn forwards(cx: &App) -> Arc<[PortForward]> {
        cx.try_global::<Self>().map_or_else(|| Arc::from(Vec::new()), |s| s.forwards.clone())
    }

    /// The palette's ports: forwards first, then the active thread's servers not yet forwarded.
    pub fn palette_ports(cx: &App) -> Vec<PalettePort> {
        let Some(state) = cx.try_global::<Self>() else { return Vec::new() };
        let labels: HashMap<EnvironmentId, SharedString> = ConnectionRuntime::state(cx)
            .read(cx)
            .environments
            .iter()
            .map(|e| (e.id().clone(), SharedString::from(e.entry.label.clone())))
            .collect();
        let label = |id: &EnvironmentId| labels.get(id).cloned().unwrap_or_else(|| id.to_string().into());
        let palette_id = |id: &EnvironmentId| crate::components::sidebar::model::EnvironmentId(id.as_str().to_owned().into());
        let mut ports: Vec<PalettePort> = state
            .forwards
            .iter()
            .map(|f| PalettePort {
                environment_id: palette_id(&f.environment_id),
                environment_label: label(&f.environment_id),
                remote_port: f.remote_port,
                local_port: Some(f.local_port),
            })
            .collect();
        if let Some((environment, _)) = &state.watched {
            for server in &state.servers {
                let port = server.port as u16;
                if !ports.iter().any(|p| p.environment_id == palette_id(environment) && p.remote_port == port) {
                    ports.push(PalettePort { environment_id: palette_id(environment), environment_label: label(environment), remote_port: port, local_port: None });
                }
            }
        }
        ports
    }

    /// Runs a request from the Forwarded ports page.
    pub fn handle(request: PortForwardRequest, cx: &mut App) {
        let Some(state) = cx.try_global::<Self>() else { return };
        let find = |environment: &EnvironmentId, port: u16| {
            state.forwards.iter().find(|f| &f.environment_id == environment && f.remote_port == port).cloned()
        };
        let wire = |id: &crate::components::sidebar::model::EnvironmentId| EnvironmentId::new(id.0.as_ref());
        match request {
            PortForwardRequest::Start { environment_id, port } => {
                let environment_id = wire(&environment_id);
                let tunnel = state.tunnel.clone();
                let runtime = ConnectionRuntime::global(cx);
                let Some(link) = runtime.environment_link(&environment_id) else {
                    Toaster::push(ToastData::new(ToastKind::Error, t!("This environment can't be reached right now. Try again once it reconnects.")), cx);
                    return;
                };
                let task = runtime.spawn(async move { tunnel.forward(link, port).await.map_err(CommandError::from) }, cx);
                cx.spawn(async move |cx| {
                    let result = task.await;
                    cx.update(|cx| match result {
                        Ok(forward) => Toaster::push(
                            ToastData::new(ToastKind::Success, t!("Forwarded port {port}", port = port))
                                .description(t!("Apps on this machine reach it at localhost:{port}.", port = forward.local_port)),
                            cx,
                        ),
                        Err(error) => Toaster::push(ToastData::new(ToastKind::Error, error.0), cx),
                    });
                })
                .detach();
            }
            PortForwardRequest::Open { environment_id, port } => {
                let environment_id = wire(&environment_id);
                if let Some(forward) = find(&environment_id, port) {
                    cx.open_url(&forwarded_url(&forward, None));
                }
            }
            PortForwardRequest::CopyAddress { environment_id, port } => {
                let environment_id = wire(&environment_id);
                if let Some(forward) = find(&environment_id, port) {
                    cx.write_to_clipboard(ClipboardItem::new_string(format!("localhost:{}", forward.local_port)));
                }
            }
            PortForwardRequest::Stop { environment_id, port } => state.tunnel.stop(&wire(&environment_id), port),
        }
    }
}
