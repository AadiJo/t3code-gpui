//! Port forwarding (`apps/desktop/src/preview/LoopbackTunnel.ts`): a listener
//! on this machine's loopback whose every connection rides one WebSocket to the
//! environment's `/api/preview/tunnel?port=N`, which dials its own loopback
//! port N and relays raw bytes both ways.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::{SinkExt as _, StreamExt as _};
use t3code_contracts::EnvironmentId;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::{self, Message};

use super::environment_link::{EnvironmentLink, LinkError, MediaAccess};
use crate::rpc::http::tls_config;

/// The environment route the tunnel rides.
pub const PREVIEW_TUNNEL_PATH: &str = "/api/preview/tunnel";
/// Set on a refusal when the environment was reached and nothing listens on the port.
const PORT_REFUSED_HEADER: &str = "x-t3-tunnel-port-refused";
/// Set when the tunnel is switched off on the environment.
const DISABLED_HEADER: &str = "x-t3-tunnel-disabled";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections beyond this close at once, so a page hammering the port cannot queue tunnels.
const MAX_FORWARD_CONNECTIONS: usize = 64;
/// Media tickets live five minutes; reuse one a little less than that.
const TICKET_REUSE: Duration = Duration::from_secs(240);
const READ_CHUNK: usize = 64 * 1024;

/// A forwarded port: `localhost:local_port` here reaches `remote_port` on the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortForward {
    pub environment_id: EnvironmentId,
    pub remote_port: u16,
    pub local_port: u16,
}

/// Why a forward could not start.
#[derive(Clone, Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("This environment runs on this computer, so localhost:{port} already reaches it.")]
    SameMachine { port: u16 },
    #[error("This environment can't be reached right now. Try again once it reconnects.")]
    Unavailable,
    #[error("The preview tunnel is turned off on this T3 Code server.")]
    Disabled,
    #[error("Failed to forward port {port}: {detail}")]
    Bind { port: u16, detail: String },
}

/// What dialling the tunnel found.
enum Dial {
    Open(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>),
    PortRefused,
    Disabled,
    Refused,
    Failed,
}

/// Caches the media ticket per environment.
struct Access {
    link: EnvironmentLink,
    cached: Mutex<Option<(MediaAccess, Instant)>>,
}

impl Access {
    async fn get(&self, fresh: bool) -> Result<MediaAccess, LinkError> {
        if !fresh
            && let Some((access, at)) = self.cached.lock().unwrap().clone()
            && at.elapsed() < TICKET_REUSE
        {
            return Ok(access);
        }
        let access = self.link.media_access().await?;
        *self.cached.lock().unwrap() = Some((access.clone(), Instant::now()));
        Ok(access)
    }

    /// Opens a tunnel to `port`, minting a new ticket once if the cached one is refused.
    async fn dial(&self, port: u16) -> Dial {
        for fresh in [false, true] {
            let Ok(access) = self.get(fresh).await else { return Dial::Failed };
            match dial(&access, port).await {
                Dial::Refused if !fresh => continue,
                other => return other,
            }
        }
        Dial::Failed
    }
}

async fn dial(access: &MediaAccess, port: u16) -> Dial {
    let url = access.ws_url(PREVIEW_TUNNEL_PATH, &[("port", &port.to_string())]);
    let connector = tokio_tungstenite::Connector::Rustls(tls_config());
    let open = tokio_tungstenite::connect_async_tls_with_config(url.as_str(), None, false, Some(connector));
    match tokio::time::timeout(CONNECT_TIMEOUT * 2, open).await {
        Ok(Ok((socket, _))) => Dial::Open(socket),
        Ok(Err(tungstenite::Error::Http(response))) => {
            if response.headers().contains_key(DISABLED_HEADER) {
                Dial::Disabled
            } else if response.headers().contains_key(PORT_REFUSED_HEADER) {
                Dial::PortRefused
            } else {
                Dial::Refused
            }
        }
        _ => Dial::Failed,
    }
}

struct Forwarded {
    forward: PortForward,
    task: tokio::task::AbortHandle,
}

/// Every forward this app runs. Forwards end when the app quits or [`Self::stop`] runs.
pub struct LoopbackTunnel {
    forwarded: Mutex<HashMap<(EnvironmentId, u16), Forwarded>>,
    changes: watch::Sender<Arc<[PortForward]>>,
}

impl Default for LoopbackTunnel {
    fn default() -> Self {
        Self { forwarded: Mutex::default(), changes: watch::channel(Arc::from(Vec::new())).0 }
    }
}

impl LoopbackTunnel {
    /// The current forwards, updated as they start and stop.
    pub fn forwards(&self) -> watch::Receiver<Arc<[PortForward]>> {
        self.changes.subscribe()
    }

    fn announce(&self) {
        let mut list: Vec<PortForward> = self.forwarded.lock().unwrap().values().map(|f| f.forward.clone()).collect();
        list.sort_by(|a, b| (a.environment_id.as_str(), a.remote_port).cmp(&(b.environment_id.as_str(), b.remote_port)));
        self.changes.send_replace(list.into());
    }

    /// Forwards `remote_port` of `link`'s environment to this machine's
    /// loopback, on the same port when it is free.
    pub async fn forward(&self, link: EnvironmentLink, remote_port: u16) -> Result<PortForward, TunnelError> {
        let key = (link.environment_id.clone(), remote_port);
        if let Some(existing) = self.forwarded.lock().unwrap().get(&key) {
            return Ok(existing.forward.clone());
        }
        let access = Arc::new(Access { link: link.clone(), cached: Mutex::default() });
        if shares_loopback(&access).await? {
            return Err(TunnelError::SameMachine { port: remote_port });
        }
        let listener = match TcpListener::bind(("127.0.0.1", remote_port)).await {
            Ok(listener) => listener,
            // Taken here, or privileged: any free port still reaches the environment's.
            Err(_) => TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|error| TunnelError::Bind { port: remote_port, detail: error.to_string() })?,
        };
        let local_port = listener.local_addr().map_err(|error| TunnelError::Bind { port: remote_port, detail: error.to_string() })?.port();
        let forward = PortForward { environment_id: link.environment_id.clone(), remote_port, local_port };
        let task = tokio::spawn(serve(listener, access, remote_port)).abort_handle();
        self.forwarded.lock().unwrap().insert(key, Forwarded { forward: forward.clone(), task });
        self.announce();
        Ok(forward)
    }

    /// Stops a forward and every connection it carries.
    pub fn stop(&self, environment_id: &EnvironmentId, remote_port: u16) {
        let removed = self.forwarded.lock().unwrap().remove(&(environment_id.clone(), remote_port));
        if let Some(removed) = removed {
            removed.task.abort();
            self.announce();
        }
    }
}

async fn serve(listener: TcpListener, access: Arc<Access>, port: u16) {
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((socket, _)) = accepted else { continue };
                if active.load(std::sync::atomic::Ordering::Relaxed) >= MAX_FORWARD_CONNECTIONS {
                    drop(socket);
                    continue;
                }
                active.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let (access, active) = (access.clone(), active.clone());
                connections.spawn(async move {
                    if let Dial::Open(tunnel) = access.dial(port).await {
                        pump(socket, tunnel).await;
                    }
                    active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

/// Relays bytes both ways until either side closes.
async fn pump(socket: TcpStream, tunnel: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>) {
    let (mut read, mut write) = socket.into_split();
    let (mut sink, mut stream) = tunnel.split();
    let upstream = async {
        let mut buffer = vec![0u8; READ_CHUNK];
        loop {
            match read.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                // `send` flushes, so a slow tunnel holds this read back.
                Ok(n) => {
                    if sink.send(Message::binary(buffer[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
            }
        }
        _ = sink.close().await;
    };
    let downstream = async {
        while let Some(Ok(message)) = stream.next().await {
            match message {
                Message::Binary(bytes) => {
                    if write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        _ = write.shutdown().await;
    };
    tokio::select! {
        () = upstream => {}
        () = downstream => {}
    }
}

/// Whether the environment's loopback is this machine's, by tunnelling a
/// nonce to a probe listener here. A forward there would loop through the tunnel.
async fn shares_loopback(access: &Access) -> Result<bool, TunnelError> {
    let probe = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|_| TunnelError::Unavailable)?;
    let port = probe.local_addr().map_err(|_| TunnelError::Unavailable)?.port();
    let nonce: [u8; 16] = rand::random();
    let tunnel = match access.dial(port).await {
        Dial::Open(tunnel) => tunnel,
        // The environment was reached and nothing listens on the probe port there.
        Dial::PortRefused => return Ok(false),
        Dial::Disabled => return Err(TunnelError::Disabled),
        Dial::Refused | Dial::Failed => return Err(TunnelError::Unavailable),
    };
    let (mut sink, _stream) = tunnel.split();
    if sink.send(Message::binary(nonce.to_vec())).await.is_err() {
        return Err(TunnelError::Unavailable);
    }
    let received = tokio::time::timeout(CONNECT_TIMEOUT, async {
        let (mut socket, _) = probe.accept().await.ok()?;
        let mut got = [0u8; 16];
        socket.read_exact(&mut got).await.ok()?;
        Some(got == nonce)
    })
    .await;
    // An answer from anything else means a service on another machine took the connection.
    Ok(matches!(received, Ok(Some(true))))
}
