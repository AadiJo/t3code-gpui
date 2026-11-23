//! The WebSocket RPC client: one socket, many concurrent calls and streams.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures::{SinkExt, Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use t3code_contracts::rpc::{RpcMethod, RpcStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::tungstenite::{self, Message};

use super::protocol::{
    ClientMessage, ExitEncoded, ExitKind, FrameJoiner, PING_INTERVAL, PING_TIMEOUT, RequestId,
    SOCKET_OPEN_TIMEOUT, decode_frame,
};
use crate::rpc::http::tls_config;

/// Which way a frame travelled, for [`FrameTap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDirection {
    Sent,
    Received,
}

/// Observer for every raw text frame on the socket, parts included. Used for
/// wire transcripts; keep it cheap, it runs on the socket task.
pub type FrameTap = Arc<dyn Fn(FrameDirection, &str) + Send + Sync>;

/// Why a socket is no longer usable. Every pending call and stream on it
/// ends with this.
#[derive(Clone, Debug, thiserror::Error)]
pub enum CloseReason {
    #[error("the WebSocket did not open within the timeout")]
    OpenTimedOut,
    /// The server answered the upgrade with an HTTP error, such as 401 or
    /// 426 (orchestration protocol mismatch).
    #[error("the server refused the WebSocket upgrade with HTTP {status}")]
    UpgradeRejected { status: u16, body: String },
    #[error("could not open the WebSocket: {0}")]
    OpenFailed(String),
    #[error("no frame arrived for {}s", PING_TIMEOUT.as_secs())]
    PingTimeout,
    #[error("the server closed the socket (code {code:?}): {reason}")]
    ServerClosed { code: Option<u16>, reason: String },
    #[error("the socket failed: {0}")]
    Transport(String),
    /// A socket-wide `Defect` envelope. It fails every pending request.
    #[error("the server reported a protocol defect: {0}")]
    Defect(String),
    #[error("the client closed the socket")]
    ClientClosed,
}

/// How one call or stream failed. `E` is the method's typed error.
#[derive(Debug, thiserror::Error)]
pub enum RpcError<E: std::fmt::Debug> {
    #[error("the request failed: {0:?}")]
    Failure(E),
    /// A typed failure whose JSON did not decode as `E`.
    #[error("the request failed with an unrecognized error: {0}")]
    UnknownFailure(Box<RawValue>),
    /// The handler died: a bug, a schema-invalid payload, or an unknown method.
    #[error("the server reported a defect: {0}")]
    Defect(String),
    #[error("the request was interrupted")]
    Interrupted,
    #[error(transparent)]
    Closed(#[from] CloseReason),
    #[error("the response did not decode: {0}")]
    Decode(String),
}

enum Outbound {
    Text(String),
    Close,
}

enum CallSink {
    Unary(oneshot::Sender<Result<Box<RawValue>, CloseReason>>),
    Stream(mpsc::UnboundedSender<StreamEvent>),
}

enum StreamEvent {
    Chunk(Vec<Box<RawValue>>),
    Exit(Box<RawValue>),
    Closed(CloseReason),
}

#[derive(Default)]
struct CoreState {
    pending: HashMap<RequestId, CallSink>,
    closed: Option<CloseReason>,
}

/// State shared by the socket task and every handle.
struct Core {
    state: Mutex<CoreState>,
    inbound_frames: AtomicU64,
    closed_tx: tokio::sync::watch::Sender<Option<CloseReason>>,
}

impl Core {
    fn register(&self, id: RequestId, sink: CallSink) -> Result<(), CloseReason> {
        let mut state = self.state.lock().expect("rpc state lock");
        if let Some(reason) = &state.closed {
            return Err(reason.clone());
        }
        state.pending.insert(id, sink);
        Ok(())
    }

    fn unregister(&self, id: RequestId) -> bool {
        self.state.lock().expect("rpc state lock").pending.remove(&id).is_some()
    }

    fn close(&self, reason: CloseReason) {
        let pending = {
            let mut state = self.state.lock().expect("rpc state lock");
            if state.closed.is_some() {
                return;
            }
            state.closed = Some(reason.clone());
            std::mem::take(&mut state.pending)
        };
        for sink in pending.into_values() {
            match sink {
                CallSink::Unary(tx) => _ = tx.send(Err(reason.clone())),
                CallSink::Stream(tx) => _ = tx.send(StreamEvent::Closed(reason.clone())),
            }
        }
        self.closed_tx.send_replace(Some(reason));
    }

    /// Routes one whole frame. A socket-wide `Defect` closes the socket.
    fn dispatch(&self, frame: &str) -> Result<(), CloseReason> {
        let envelopes = match decode_frame(frame) {
            Ok(envelopes) => envelopes,
            Err(error) => {
                tracing::warn!(%error, "ignoring an undecodable RPC frame");
                return Ok(());
            }
        };
        for envelope in envelopes {
            let id = envelope.request_id.and_then(|id| id.ours());
            match (&*envelope.tag, id) {
                ("Chunk", Some(id)) => {
                    let values = envelope.values.unwrap_or_default();
                    let state = self.state.lock().expect("rpc state lock");
                    if let Some(CallSink::Stream(tx)) = state.pending.get(&id) {
                        _ = tx.send(StreamEvent::Chunk(values.into_iter().map(RawValue::to_owned).collect()));
                    }
                }
                ("Exit", Some(id)) => {
                    let Some(exit) = envelope.exit else { continue };
                    let sink = self.state.lock().expect("rpc state lock").pending.remove(&id);
                    match sink {
                        Some(CallSink::Unary(tx)) => _ = tx.send(Ok(exit.to_owned())),
                        Some(CallSink::Stream(tx)) => _ = tx.send(StreamEvent::Exit(exit.to_owned())),
                        None => {}
                    }
                }
                ("Defect", _) => {
                    return Err(CloseReason::Defect(defect_message(envelope.defect)));
                }
                ("Pong", _) => {}
                (tag, _) => tracing::debug!(tag, "ignoring an unexpected RPC envelope"),
            }
        }
        Ok(())
    }
}

/// `Schema.Defect` encodes an Error as `{name, message}`, anything else as
/// its JSON value or a string.
fn defect_message(defect: Option<&RawValue>) -> String {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Defect {
        Text(String),
        Error { message: String },
    }
    let Some(defect) = defect else { return "unknown defect".into() };
    match serde_json::from_str::<Defect>(defect.get()) {
        Ok(Defect::Text(text) | Defect::Error { message: text }) => text,
        Err(_) => defect.get().to_owned(),
    }
}

/// Holds the outbound channel. When the last handle drops, the socket task
/// sees the channel close and shuts the socket.
struct Shared {
    outbound: mpsc::UnboundedSender<Outbound>,
    next_id: AtomicU64,
    core: Arc<Core>,
}

impl Shared {
    fn send(&self, message: &ClientMessage<'_>) {
        let text = serde_json::to_string(message).expect("client envelopes serialize");
        _ = self.outbound.send(Outbound::Text(text));
    }

    /// Cancels a request the caller no longer wants. Not sent once the
    /// socket closed: there is nothing left to cancel.
    fn interrupt(&self, id: RequestId) {
        if self.core.unregister(id) {
            self.send(&ClientMessage::Interrupt { request_id: id });
        }
    }
}

/// A connected RPC socket. Cheap to clone; every clone shares the socket.
#[derive(Clone)]
pub struct RpcClient {
    shared: Arc<Shared>,
}

impl RpcClient {
    /// Opens the socket at `url` (already carrying its ticket and query
    /// parameters) and starts its reader, writer and keepalive task.
    pub async fn connect(url: &str, tap: Option<FrameTap>) -> Result<Self, CloseReason> {
        let connector = tokio_tungstenite::Connector::Rustls(tls_config());
        let open = tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(connector));
        let (socket, _response) = match tokio::time::timeout(SOCKET_OPEN_TIMEOUT, open).await {
            Err(_) => return Err(CloseReason::OpenTimedOut),
            Ok(Err(tungstenite::Error::Http(response))) => {
                let status = response.status().as_u16();
                let body = response
                    .body()
                    .as_deref()
                    .map(|body| String::from_utf8_lossy(body).into_owned())
                    .unwrap_or_default();
                return Err(CloseReason::UpgradeRejected { status, body });
            }
            Ok(Err(error)) => return Err(CloseReason::OpenFailed(error.to_string())),
            Ok(Ok(opened)) => opened,
        };
        let (closed_tx, _) = tokio::sync::watch::channel(None);
        let core = Arc::new(Core {
            state: Mutex::default(),
            inbound_frames: AtomicU64::new(0),
            closed_tx,
        });
        let (outbound, outbound_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_socket(socket, core.clone(), outbound_rx, tap));
        Ok(Self {
            shared: Arc::new(Shared { outbound, next_id: AtomicU64::new(0), core }),
        })
    }

    /// Sends one unary request and waits for its `Exit`. Dropping the future
    /// before it resolves sends `Interrupt`.
    pub async fn call<M: RpcMethod>(&self, payload: M::Payload) -> Result<M::Success, RpcError<M::Error>>
    where
        M::Error: std::fmt::Debug,
    {
        let payload = encode_payload::<M>(&payload)?;
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.shared.core.register(id, CallSink::Unary(tx))?;
        let mut guard = InterruptOnDrop { shared: &self.shared, id, armed: true };
        self.shared.send(&ClientMessage::Request { id, tag: M::TAG, payload: &payload, headers: [] });
        let exit = rx.await.map_err(|_| CloseReason::ClientClosed)??;
        guard.armed = false;
        decode_exit::<M::Success, M::Error>(&exit)
    }

    /// Starts a stream. Values arrive one chunk at a time; each chunk is
    /// acked when the consumer takes it, so a consumer that stops reading
    /// stops the server. Dropping the subscription sends `Interrupt`.
    pub fn subscribe<M: RpcStream>(&self, payload: M::Payload) -> RpcSubscription<M>
    where
        M::Error: std::fmt::Debug,
    {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        let start = encode_payload::<M>(&payload).and_then(|payload| {
            self.shared.core.register(id, CallSink::Stream(tx))?;
            self.shared.send(&ClientMessage::Request { id, tag: M::TAG, payload: &payload, headers: [] });
            Ok(())
        });
        RpcSubscription {
            shared: self.shared.clone(),
            id,
            rx,
            state: match start {
                Ok(()) => SubscriptionState::Open,
                Err(error) => SubscriptionState::Failed(Some(error)),
            },
            _method: PhantomData,
        }
    }

    /// Frames received so far, split parts included. A count that moves
    /// proves the socket is alive without a round trip.
    pub fn inbound_frames(&self) -> u64 {
        self.shared.core.inbound_frames.load(Ordering::Relaxed)
    }

    /// Resolves when the socket closes, with the reason.
    pub async fn closed(&self) -> CloseReason {
        let mut rx = self.shared.core.closed_tx.subscribe();
        loop {
            if let Some(reason) = rx.borrow_and_update().clone() {
                return reason;
            }
            if rx.changed().await.is_err() {
                return CloseReason::ClientClosed;
            }
        }
    }

    /// Closes the socket now. Pending requests fail with `ClientClosed`.
    pub fn close(&self) {
        _ = self.shared.outbound.send(Outbound::Close);
    }
}

fn encode_payload<M: RpcMethod>(payload: &M::Payload) -> Result<Box<RawValue>, RpcError<M::Error>>
where
    M::Error: std::fmt::Debug,
{
    serde_json::value::to_raw_value(payload).map_err(|error| RpcError::Decode(error.to_string()))
}

fn decode_exit<S: DeserializeOwned, E: DeserializeOwned + std::fmt::Debug>(
    exit: &RawValue,
) -> Result<S, RpcError<E>> {
    let exit: ExitEncoded<'_> =
        serde_json::from_str(exit.get()).map_err(|error| RpcError::Decode(error.to_string()))?;
    match exit.kind() {
        ExitKind::Success(value) => {
            let json = value.map_or("null", RawValue::get);
            serde_json::from_str(json).map_err(|error| RpcError::Decode(error.to_string()))
        }
        ExitKind::Fail(error) => Err(match serde_json::from_str(error.get()) {
            Ok(typed) => RpcError::Failure(typed),
            Err(_) => RpcError::UnknownFailure(error.to_owned()),
        }),
        ExitKind::Die(defect) => Err(RpcError::Defect(defect_message(Some(defect)))),
        ExitKind::Interrupted => Err(RpcError::Interrupted),
    }
}

struct InterruptOnDrop<'a> {
    shared: &'a Shared,
    id: RequestId,
    armed: bool,
}

impl Drop for InterruptOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.shared.interrupt(self.id);
        }
    }
}

enum SubscriptionState<E: std::fmt::Debug> {
    Open,
    Failed(Option<RpcError<E>>),
    Done,
}

/// A running stream RPC. Yields one `Vec` per server chunk, so a consumer
/// can apply a chunk atomically. Ends after the server's `Exit`; an error
/// item is always the last.
pub struct RpcSubscription<M: RpcStream>
where
    M::Error: std::fmt::Debug,
{
    shared: Arc<Shared>,
    id: RequestId,
    rx: mpsc::UnboundedReceiver<StreamEvent>,
    state: SubscriptionState<M::Error>,
    _method: PhantomData<fn() -> M>,
}

impl<M: RpcStream> RpcSubscription<M>
where
    M::Error: std::fmt::Debug,
{
    /// The request id on the socket, for logs and transcripts.
    pub fn request_id(&self) -> RequestId {
        self.id
    }
}

impl<M: RpcStream> Unpin for RpcSubscription<M> where M::Error: std::fmt::Debug {}

impl<M: RpcStream> Stream for RpcSubscription<M>
where
    M::Error: std::fmt::Debug,
{
    type Item = Result<Vec<M::Success>, RpcError<M::Error>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        loop {
            match &mut this.state {
                SubscriptionState::Done => return Poll::Ready(None),
                SubscriptionState::Failed(error) => {
                    let error = error.take();
                    this.state = SubscriptionState::Done;
                    return Poll::Ready(error.map(Err));
                }
                SubscriptionState::Open => {}
            }
            let Some(event) = std::task::ready!(this.rx.poll_recv(cx)) else {
                this.state = SubscriptionState::Done;
                return Poll::Ready(Some(Err(CloseReason::ClientClosed.into())));
            };
            match event {
                StreamEvent::Chunk(values) => {
                    this.shared.send(&ClientMessage::Ack { request_id: this.id });
                    let decoded: Vec<M::Success> = values
                        .iter()
                        .filter_map(|value| match serde_json::from_str(value.get()) {
                            Ok(decoded) => Some(decoded),
                            Err(error) => {
                                tracing::warn!(method = M::TAG, %error, "skipping a stream value that did not decode");
                                None
                            }
                        })
                        .collect();
                    if decoded.is_empty() {
                        continue;
                    }
                    return Poll::Ready(Some(Ok(decoded)));
                }
                StreamEvent::Exit(exit) => {
                    this.state = SubscriptionState::Done;
                    return Poll::Ready(match decode_exit::<serde::de::IgnoredAny, M::Error>(&exit) {
                        Ok(_) => None,
                        Err(error) => Some(Err(error)),
                    });
                }
                StreamEvent::Closed(reason) => {
                    this.state = SubscriptionState::Done;
                    return Poll::Ready(Some(Err(reason.into())));
                }
            }
        }
    }
}

impl<M: RpcStream> Drop for RpcSubscription<M>
where
    M::Error: std::fmt::Debug,
{
    fn drop(&mut self) {
        if matches!(self.state, SubscriptionState::Open) {
            self.shared.interrupt(self.id);
        }
    }
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Owns the socket: reads and routes frames, writes requests, pings every
/// 5 s, and drops the socket after 15 s without any inbound frame.
async fn run_socket(
    socket: Socket,
    core: Arc<Core>,
    mut outbound: mpsc::UnboundedReceiver<Outbound>,
    tap: Option<FrameTap>,
) {
    let (mut sink, mut stream) = socket.split();
    let mut joiner = FrameJoiner::default();
    let mut ping = tokio::time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    let mut last_frame = Instant::now();
    let ping_text = serde_json::to_string(&ClientMessage::Ping).expect("ping serializes");
    let tap = |direction, text: &str| {
        if let Some(tap) = &tap {
            tap(direction, text);
        }
    };
    let reason = loop {
        tokio::select! {
            inbound = stream.next() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    last_frame = Instant::now();
                    core.inbound_frames.fetch_add(1, Ordering::Relaxed);
                    tap(FrameDirection::Received, text.as_str());
                    if let Some(whole) = joiner.push(text.as_str().to_owned())
                        && let Err(reason) = core.dispatch(&whole)
                    {
                        break reason;
                    }
                }
                Some(Ok(Message::Close(frame))) => break CloseReason::ServerClosed {
                    code: frame.as_ref().map(|frame| u16::from(frame.code)),
                    reason: frame.map(|frame| frame.reason.to_string()).unwrap_or_default(),
                },
                Some(Ok(_)) => {}
                Some(Err(error)) => break CloseReason::Transport(error.to_string()),
                None => break CloseReason::ServerClosed { code: None, reason: String::new() },
            },
            command = outbound.recv() => match command {
                Some(Outbound::Text(text)) => {
                    tap(FrameDirection::Sent, &text);
                    if let Err(error) = sink.send(Message::text(text)).await {
                        break CloseReason::Transport(error.to_string());
                    }
                }
                Some(Outbound::Close) | None => {
                    _ = sink.close().await;
                    break CloseReason::ClientClosed;
                }
            },
            _ = ping.tick() => {
                tap(FrameDirection::Sent, &ping_text);
                if let Err(error) = sink.send(Message::text(ping_text.clone())).await {
                    break CloseReason::Transport(error.to_string());
                }
            }
            _ = sleep_until(last_frame + PING_TIMEOUT) => break CloseReason::PingTimeout,
        }
    };
    tracing::debug!(%reason, "RPC socket closed");
    core.close(reason);
}
