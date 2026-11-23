//! What the streamed browser preview reads and sends, as one trait whose
//! methods are one RPC or one socket each (`client-runtime/state/preview.ts`,
//! `preview/serverBrowserStream.ts`). [`LivePreviewSource`] talks to an
//! environment; [`FixturePreviewSource`] paints generated frames for scenes.

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::SharedString;
use t3code_client_runtime::preview::environment_link::EnvironmentLink;
use t3code_client_runtime::preview::server_browser_stream::{
    Controller, LatestFrame, PreviewStreamClient, PreviewStreamControl, PreviewStreamEvent, PreviewStreamInput, PreviewStreamTarget,
    PreviewStreamViewport,
};
use t3code_contracts::rpc::methods::{
    PreviewAdjust, PreviewClose, PreviewList, PreviewOpen, PreviewResize, PreviewStopLocalServer, SubscribeDiscoveredLocalServers,
    SubscribePreviewEvents,
};
use t3code_contracts::{
    DiscoveredLocalServerList, EnvironmentId, PreviewAdjustInput, PreviewCloseInput, PreviewEvent, PreviewListResult, PreviewOpenInput,
    PreviewResizeInput, PreviewSessionSnapshot, PreviewStopLocalServerInput, SubscribeDiscoveredLocalServersPayload, ThreadId,
};
use tokio::sync::mpsc;

/// A failed call, worded for the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewError(pub SharedString);

/// One RPC's answer.
pub type PreviewFuture<T> = BoxFuture<'static, Result<T, PreviewError>>;

/// A running stream for one tab: the newest frame, other events, and input.
/// Dropping it closes the socket.
pub struct PreviewStream {
    pub frames: Arc<LatestFrame>,
    pub events: mpsc::UnboundedReceiver<PreviewStreamEvent>,
    send: Box<dyn Fn(PreviewStreamInput) -> bool>,
}

impl PreviewStream {
    /// Sends input as the server allows: only `TakeControl` without control.
    pub fn send(&self, input: PreviewStreamInput) -> bool {
        (self.send)(input)
    }
}

/// The preview's data for one environment.
pub trait PreviewSource: 'static {
    /// `preview.list`: the thread's tabs.
    fn list(&self, thread: &ThreadId) -> PreviewFuture<PreviewListResult>;
    /// `preview.open`: a new tab, or navigates the reveal target.
    fn open(&self, input: PreviewOpenInput) -> PreviewFuture<PreviewSessionSnapshot>;
    /// `preview.close`.
    fn close(&self, input: PreviewCloseInput) -> PreviewFuture<()>;
    /// `preview.resize`: the viewport setting (fill, preset or freeform).
    fn resize(&self, input: PreviewResizeInput) -> PreviewFuture<PreviewSessionSnapshot>;
    /// `preview.adjust`: zoom, colour scheme, hard reload, clearing data.
    fn adjust(&self, input: PreviewAdjustInput) -> PreviewFuture<PreviewSessionSnapshot>;
    /// `subscribePreviewEvents`.
    fn events(&self) -> BoxStream<'static, PreviewEvent>;
    /// `subscribeDiscoveredLocalServers`.
    fn discovered_servers(&self, input: SubscribeDiscoveredLocalServersPayload) -> BoxStream<'static, DiscoveredLocalServerList>;
    /// `preview.stopLocalServer`.
    fn stop_local_server(&self, input: PreviewStopLocalServerInput) -> PreviewFuture<()>;
    /// WS `/api/preview-stream/ws` for one tab.
    fn stream(&self, target: PreviewStreamTarget) -> PreviewStream;
}

/// Live preview for one environment.
pub struct LivePreviewSource {
    link: EnvironmentLink,
    handle: tokio::runtime::Handle,
}

impl LivePreviewSource {
    pub fn new(link: EnvironmentLink, handle: tokio::runtime::Handle) -> Rc<Self> {
        Rc::new(Self { link, handle })
    }

    /// The environment this source streams from.
    pub fn environment_id(&self) -> &EnvironmentId {
        &self.link.environment_id
    }

    fn call<M: t3code_contracts::rpc::RpcMethod>(&self, payload: M::Payload) -> PreviewFuture<M::Success>
    where
        M::Payload: Send + 'static,
        M::Success: Send + 'static,
        M::Error: std::fmt::Debug + Send,
    {
        let link = self.link.clone();
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.handle.spawn(async move {
            let result = async {
                let session = link.session(std::time::Duration::from_secs(10)).await.map_err(|e| PreviewError(e.to_string().into()))?;
                session.client.call::<M>(payload).await.map_err(|e| PreviewError(e.to_string().into()))
            }
            .await;
            _ = sender.send(result);
        });
        async move { receiver.await.unwrap_or_else(|_| Err(PreviewError("The app is shutting down.".into()))) }.boxed()
    }

    fn subscribe<M: t3code_contracts::rpc::RpcStream>(&self, payload: M::Payload) -> BoxStream<'static, M::Success>
    where
        M::Payload: Clone + Send + Sync + 'static,
        M::Success: Send + 'static,
        M::Error: std::fmt::Debug + Send,
    {
        let link = self.link.clone();
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        self.handle.spawn(async move {
            // Follows reconnects: each new session starts the stream again.
            loop {
                let Ok(session) = link.session(std::time::Duration::from_secs(3600)).await else { return };
                let mut stream = session.client.subscribe::<M>(payload.clone());
                while let Some(Ok(chunk)) = stream.next().await {
                    for item in chunk {
                        if sender.unbounded_send(item).is_err() {
                            return;
                        }
                    }
                }
                drop(session);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
        receiver.boxed()
    }
}

impl PreviewSource for LivePreviewSource {
    fn list(&self, thread: &ThreadId) -> PreviewFuture<PreviewListResult> {
        self.call::<PreviewList>(t3code_contracts::PreviewListInput { thread_id: thread.clone() })
    }

    fn open(&self, input: PreviewOpenInput) -> PreviewFuture<PreviewSessionSnapshot> {
        self.call::<PreviewOpen>(input)
    }

    fn close(&self, input: PreviewCloseInput) -> PreviewFuture<()> {
        self.call::<PreviewClose>(input)
    }

    fn resize(&self, input: PreviewResizeInput) -> PreviewFuture<PreviewSessionSnapshot> {
        self.call::<PreviewResize>(input)
    }

    fn adjust(&self, input: PreviewAdjustInput) -> PreviewFuture<PreviewSessionSnapshot> {
        self.call::<PreviewAdjust>(input)
    }

    fn events(&self) -> BoxStream<'static, PreviewEvent> {
        self.subscribe::<SubscribePreviewEvents>(Default::default())
    }

    fn discovered_servers(&self, input: SubscribeDiscoveredLocalServersPayload) -> BoxStream<'static, DiscoveredLocalServerList> {
        self.subscribe::<SubscribeDiscoveredLocalServers>(input)
    }

    fn stop_local_server(&self, input: PreviewStopLocalServerInput) -> PreviewFuture<()> {
        self.call::<PreviewStopLocalServer>(input)
    }

    fn stream(&self, target: PreviewStreamTarget) -> PreviewStream {
        let frames = Arc::new(LatestFrame::default());
        let (events_tx, events) = mpsc::unbounded_channel();
        let _guard = self.handle.enter();
        let client = PreviewStreamClient::start(self.link.clone(), target, frames.clone(), events_tx);
        PreviewStream { frames, events, send: Box::new(move |input| client.send(input)) }
    }
}

/// Generated pages for scenes and checks: each tab paints a frame showing
/// its URL's hue; input is recorded in [`Self::sent`].
pub struct FixturePreviewSource {
    sessions: Mutex<Vec<PreviewSessionSnapshot>>,
    /// Every input the stream accepted, in order.
    pub sent: Arc<Mutex<Vec<PreviewStreamInput>>>,
    viewport: (u32, u32),
}

/// Encodes a page-like JPEG of `width`×`height` tinted by `seed`.
pub fn fixture_frame(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let mut image = image::RgbImage::new(width, height);
    let hue = |s: u32| [(80 + s * 37 % 120) as u8, (120 + s * 53 % 100) as u8, (200 - s * 29 % 80) as u8];
    let accent = hue(seed);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let header = y < height / 9;
        let card = x > width / 10 && x < width * 9 / 10 && y > height / 4 && y < height * 3 / 5;
        let line = card && (y - height / 4) % (height / 20).max(1) < 3 && x < width * 7 / 10;
        *pixel = image::Rgb(if header {
            accent
        } else if line {
            [190, 190, 196]
        } else if card {
            [250, 250, 252]
        } else {
            [236, 238, 242]
        });
    }
    let mut out = Vec::new();
    _ = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&image);
    out
}

impl FixturePreviewSource {
    pub fn new(sessions: Vec<PreviewSessionSnapshot>) -> Rc<Self> {
        Rc::new(Self { sessions: Mutex::new(sessions), sent: Arc::default(), viewport: (1280, 800) })
    }
}

impl PreviewSource for FixturePreviewSource {
    fn list(&self, _: &ThreadId) -> PreviewFuture<PreviewListResult> {
        let sessions = self.sessions.lock().unwrap().clone();
        async move { Ok(PreviewListResult { sessions, server_epoch: "fixture".into(), revision: 1 }) }.boxed()
    }

    fn open(&self, input: PreviewOpenInput) -> PreviewFuture<PreviewSessionSnapshot> {
        let mut sessions = self.sessions.lock().unwrap();
        let snapshot = fixture_session(&input.thread_id, &format!("tab-{}", sessions.len() + 1), input.url.as_deref().unwrap_or("about:blank"), "New tab");
        sessions.push(snapshot.clone());
        async move { Ok(snapshot) }.boxed()
    }

    fn close(&self, input: PreviewCloseInput) -> PreviewFuture<()> {
        self.sessions.lock().unwrap().retain(|s| Some(&s.tab_id) != input.tab_id.as_ref());
        async { Ok(()) }.boxed()
    }

    fn resize(&self, input: PreviewResizeInput) -> PreviewFuture<PreviewSessionSnapshot> {
        let found = self.sessions.lock().unwrap().iter().find(|s| s.tab_id == input.tab_id).cloned();
        async move { found.ok_or(PreviewError("No such tab".into())) }.boxed()
    }

    fn adjust(&self, input: PreviewAdjustInput) -> PreviewFuture<PreviewSessionSnapshot> {
        let found = self.sessions.lock().unwrap().iter().find(|s| s.tab_id == input.tab_id).cloned();
        async move { found.ok_or(PreviewError("No such tab".into())) }.boxed()
    }

    fn events(&self) -> BoxStream<'static, PreviewEvent> {
        futures::stream::pending().boxed()
    }

    fn discovered_servers(&self, _: SubscribeDiscoveredLocalServersPayload) -> BoxStream<'static, DiscoveredLocalServerList> {
        futures::stream::pending().boxed()
    }

    fn stop_local_server(&self, _: PreviewStopLocalServerInput) -> PreviewFuture<()> {
        async { Ok(()) }.boxed()
    }

    fn stream(&self, target: PreviewStreamTarget) -> PreviewStream {
        let frames = Arc::new(LatestFrame::default());
        let (events_tx, events) = mpsc::unbounded_channel();
        let (width, height) = self.viewport;
        let seed = target.tab_id.bytes().map(u32::from).sum();
        frames.put(1, fixture_frame(width, height, seed).into());
        _ = events_tx.send(PreviewStreamEvent::Connected(true));
        _ = events_tx.send(PreviewStreamEvent::Viewport(PreviewStreamViewport { width: width as f64, height: height as f64 }));
        let control = PreviewStreamControl { can_operate: true, controller: Controller::You, generation: 1, dialog: None };
        _ = events_tx.send(PreviewStreamEvent::Control(Some(control)));
        let sent = self.sent.clone();
        PreviewStream {
            frames,
            events,
            send: Box::new(move |input| {
                sent.lock().unwrap().push(input);
                true
            }),
        }
    }
}

/// A settled tab at `url` for fixtures.
pub fn fixture_session(thread: &ThreadId, tab: &str, url: &str, title: &str) -> PreviewSessionSnapshot {
    serde_json::from_value(serde_json::json!({
        "threadId": thread, "tabId": tab, "canGoBack": true, "canGoForward": false,
        "navStatus": { "_tag": "Success", "url": url, "title": title },
        "updatedAt": "2026-11-20T15:37:12.000Z", "runtime": "server",
    }))
    .expect("fixture session decodes")
}
