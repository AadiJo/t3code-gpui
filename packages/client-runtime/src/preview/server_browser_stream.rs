//! The streamed browser preview (`serverBrowserStream.ts`): one WebSocket to
//! `/api/preview-stream/ws` per tab. The server sends JPEG frames as binary
//! messages and JSON control messages; the client acknowledges each frame and
//! sends JSON input in page CSS pixels.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use t3code_contracts::PreviewViewportSetting;
use tokio::sync::{Notify, mpsc, watch};
use tokio_tungstenite::tungstenite::{self, Message};

use super::environment_link::{EnvironmentLink, MediaAccess};
use crate::rpc::http::tls_config;

/// `PREVIEW_STREAM_BASE_PATH`.
pub const PREVIEW_STREAM_BASE_PATH: &str = "/api/preview-stream";
/// `PREVIEW_STREAM_HOST_SETUP_CLOSE_CODE`: the host cannot run the browser.
pub const PREVIEW_STREAM_HOST_SETUP_CLOSE_CODE: u16 = 4503;
const ACK_MESSAGE: &str = r#"{"type":"ack"}"#;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_UNAUTHORIZED_REFUSALS: u32 = 3;

/// A mouse button as the stream names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    None,
    Left,
    Middle,
    Right,
}

/// A press, release or move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseAction {
    Move,
    Down,
    Up,
}

/// A key's press or release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAction {
    Down,
    Up,
}

/// Modifier bits: alt 1, ctrl 2, meta 4, shift 8.
pub fn modifier_bits(alt: bool, ctrl: bool, meta: bool, shift: bool) -> u8 {
    u8::from(alt) | (u8::from(ctrl) << 1) | (u8::from(meta) << 2) | (u8::from(shift) << 3)
}

/// What the client sends. Coordinates are page CSS pixels.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PreviewStreamInput {
    TakeControl,
    ReleaseControl,
    Dialog {
        accept: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_text: Option<String>,
    },
    Viewport { setting: PreviewViewportSetting },
    Mouse { action: MouseAction, x: f64, y: f64, button: MouseButton, buttons: u8, click_count: u32, modifiers: u8 },
    Wheel { x: f64, y: f64, delta_x: f64, delta_y: f64, modifiers: u8 },
    Key {
        action: KeyAction,
        key: String,
        code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        key_code: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        modifiers: u8,
    },
    Text { text: String },
    Resize { width: u32, height: u32 },
    Navigate { url: String },
    History { delta: i8 },
    Reload {
        #[serde(skip_serializing_if = "Option::is_none")]
        ignore_cache: Option<bool>,
    },
    StopLoading,
    Probe { x: f64, y: f64 },
}

/// Who drives the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Controller {
    Agent,
    You,
    AnotherViewer,
    Unclaimed,
}

/// An `alert`, `confirm` or `prompt` the page is waiting on.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDialog {
    pub r#type: String,
    pub message: String,
    pub default_value: String,
}

/// The control state the server last sent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStreamControl {
    pub can_operate: bool,
    pub controller: Controller,
    pub generation: u64,
    pub dialog: Option<PageDialog>,
}

/// The page's viewport in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct PreviewStreamViewport {
    pub width: f64,
    pub height: f64,
}

/// A file picker the page opened; POST multipart `file` parts to `upload_url`.
#[derive(Clone, Debug, PartialEq)]
pub struct FileChooser {
    pub id: String,
    pub multiple: bool,
    pub accept: String,
    pub upload_url: String,
}

/// Why the host cannot run its browser, with the command that fixes it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HostSetup {
    pub need: HostSetupNeed,
    pub command: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostSetupNeed {
    Sandbox,
    Libraries,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum ServerMessage {
    FileChooser { id: String, multiple: bool, accept: String },
    FileChooserClosed { id: String },
    Pointer { phase: String, x: f64, y: f64, sequence: u64 },
    Popup { tab_id: String },
    Clipboard { text: String },
    Download { id: String, file_name: String, size_bytes: u64 },
    Viewport { width: f64, height: f64 },
    Probe { x: f64, y: f64, editable: bool },
    Control(PreviewStreamControl),
    #[serde(other)]
    Unknown,
}

/// Everything but frames, in arrival order.
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewStreamEvent {
    Connected(bool),
    Viewport(PreviewStreamViewport),
    Control(Option<PreviewStreamControl>),
    Probe { x: f64, y: f64, editable: bool },
    /// The agent's cursor.
    Pointer { click: bool, x: f64, y: f64, sequence: u64 },
    Clipboard(String),
    Download { file_name: String, size_bytes: u64, url: String },
    Popup(String),
    FileChooser(Option<FileChooser>),
    /// The upgrade was refused; mint new access and start a new client.
    Unauthorized,
    /// The tab is gone (close code 4404).
    Gone,
    HostSetup(HostSetup),
}

/// The newest JPEG frame not yet taken. A slow viewer skips frames instead
/// of queueing them, so at most one encoded frame waits here.
#[derive(Default)]
pub struct LatestFrame {
    frame: Mutex<Option<(u64, Arc<[u8]>)>>,
    ready: Notify,
}

impl LatestFrame {
    /// Waits for and takes the newest frame, with its sequence number.
    pub async fn next(&self) -> (u64, Arc<[u8]>) {
        loop {
            let notified = self.ready.notified();
            if let Some(frame) = self.frame.lock().unwrap().take() {
                return frame;
            }
            notified.await;
        }
    }

    /// Replaces the waiting frame; the stream calls it, fixtures and recordings may too.
    pub fn put(&self, sequence: u64, jpeg: Arc<[u8]>) {
        *self.frame.lock().unwrap() = Some((sequence, jpeg));
        self.ready.notify_one();
    }
}

/// Which tab to stream and the largest frame worth sending, in device pixels.
#[derive(Clone, Debug)]
pub struct PreviewStreamTarget {
    pub thread_id: String,
    pub tab_id: String,
    pub max_width: u32,
    pub max_height: u32,
    pub interactive: bool,
}

/// A running stream. Dropping it stops the socket and reconnects.
pub struct PreviewStreamClient {
    input: mpsc::UnboundedSender<PreviewStreamInput>,
    control: watch::Receiver<Option<PreviewStreamControl>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for PreviewStreamClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl PreviewStreamClient {
    /// Starts streaming on the current tokio runtime. Frames go to `frames`,
    /// other events to `events`.
    pub fn start(
        link: EnvironmentLink,
        target: PreviewStreamTarget,
        frames: Arc<LatestFrame>,
        events: mpsc::UnboundedSender<PreviewStreamEvent>,
    ) -> Self {
        let (input, inputs) = mpsc::unbounded_channel();
        let (control_tx, control) = watch::channel(None);
        let task = tokio::spawn(run(link, target, frames, events, inputs, control_tx));
        Self { input, control, task }
    }

    /// Sends input when the server lets this viewer operate; only
    /// `TakeControl` goes out without control. Returns whether it was sent.
    pub fn send(&self, input: PreviewStreamInput) -> bool {
        let control = self.control.borrow();
        let Some(control) = control.as_ref().filter(|c| c.can_operate) else { return false };
        if input != PreviewStreamInput::TakeControl && control.controller != Controller::You {
            return false;
        }
        self.input.send(input).is_ok()
    }
}

enum Ended {
    Stop,
    /// The server answered the upgrade with an HTTP error.
    Refused,
    Retry,
}

async fn run(
    link: EnvironmentLink,
    target: PreviewStreamTarget,
    frames: Arc<LatestFrame>,
    events: mpsc::UnboundedSender<PreviewStreamEvent>,
    mut inputs: mpsc::UnboundedReceiver<PreviewStreamInput>,
    control: watch::Sender<Option<PreviewStreamControl>>,
) {
    let mut failures = 0u32;
    let mut sequence = 0u64;
    let mut access: Option<MediaAccess> = None;
    let mut refusals = 0u32;
    loop {
        let current = match &access {
            Some(access) => access.clone(),
            None => match link.media_access().await {
                Ok(fresh) => {
                    access = Some(fresh.clone());
                    fresh
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }
            },
        };
        let ended = connect_once(&current, &target, &frames, &events, &mut inputs, &control, &mut sequence, &mut failures).await;
        _ = control.send(None);
        match ended {
            Ended::Stop => return,
            ended => {
                // A refused upgrade usually means the ticket is spent or expired;
                // three refusals in a row with fresh tickets mean access itself is refused.
                if matches!(ended, Ended::Refused) {
                    access = None;
                    refusals += 1;
                    if refusals >= MAX_UNAUTHORIZED_REFUSALS {
                        _ = events.send(PreviewStreamEvent::Unauthorized);
                        return;
                    }
                } else {
                    refusals = 0;
                }
                let delay = (500u64 << failures.min(5)).min(10_000);
                failures += 1;
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn connect_once(
    access: &MediaAccess,
    target: &PreviewStreamTarget,
    frames: &LatestFrame,
    events: &mpsc::UnboundedSender<PreviewStreamEvent>,
    inputs: &mut mpsc::UnboundedReceiver<PreviewStreamInput>,
    control: &watch::Sender<Option<PreviewStreamControl>>,
    sequence: &mut u64,
    failures: &mut u32,
) -> Ended {
    let (max_width, max_height) = (target.max_width.max(1).to_string(), target.max_height.max(1).to_string());
    let mut query =
        vec![("threadId", target.thread_id.as_str()), ("tabId", target.tab_id.as_str()), ("maxWidth", &max_width), ("maxHeight", &max_height)];
    if !target.interactive {
        query.push(("interactive", "false"));
    }
    let url = access.ws_url(&format!("{PREVIEW_STREAM_BASE_PATH}/ws"), &query);
    let connector = tokio_tungstenite::Connector::Rustls(tls_config());
    let open = tokio_tungstenite::connect_async_tls_with_config(url.as_str(), None, false, Some(connector));
    let socket = match tokio::time::timeout(OPEN_TIMEOUT, open).await {
        Ok(Ok((socket, _))) => socket,
        Ok(Err(tungstenite::Error::Http(response))) if response.status().as_u16() == 404 => {
            _ = events.send(PreviewStreamEvent::Gone);
            return Ended::Stop;
        }
        Ok(Err(tungstenite::Error::Http(_))) => return Ended::Refused,
        _ => return Ended::Retry,
    };
    _ = events.send(PreviewStreamEvent::Connected(true));
    let (mut sink, mut stream) = socket.split();
    let mut file_chooser: Option<String> = None;
    let ended = loop {
        tokio::select! {
            input = inputs.recv() => {
                let Some(input) = input else { break Ended::Stop };
                let Ok(text) = serde_json::to_string(&input) else { continue };
                if sink.send(Message::text(text)).await.is_err() {
                    break Ended::Retry;
                }
            }
            message = stream.next() => match message {
                Some(Ok(Message::Binary(jpeg))) => {
                    *failures = 0;
                    if sink.send(Message::text(ACK_MESSAGE)).await.is_err() {
                        break Ended::Retry;
                    }
                    *sequence += 1;
                    frames.put(*sequence, Arc::from(&jpeg[..]));
                }
                Some(Ok(Message::Text(text))) => {
                    let Ok(message) = serde_json::from_str::<ServerMessage>(&text) else { continue };
                    let event = match message {
                        ServerMessage::FileChooser { id, multiple, accept } => {
                            file_chooser = Some(id.clone());
                            let upload_url = access
                                .http_url(&format!("{PREVIEW_STREAM_BASE_PATH}/upload"), &[("threadId", &target.thread_id), ("tabId", &target.tab_id), ("chooser", &id)])
                                .to_string();
                            PreviewStreamEvent::FileChooser(Some(FileChooser { id, multiple, accept, upload_url }))
                        }
                        ServerMessage::FileChooserClosed { id } if file_chooser.as_deref() == Some(id.as_str()) => {
                            file_chooser = None;
                            PreviewStreamEvent::FileChooser(None)
                        }
                        ServerMessage::FileChooserClosed { .. } | ServerMessage::Unknown => continue,
                        ServerMessage::Pointer { phase, x, y, sequence } => match phase.as_str() {
                            "move" | "click" => PreviewStreamEvent::Pointer { click: phase == "click", x, y, sequence },
                            _ => continue,
                        },
                        ServerMessage::Popup { tab_id } => PreviewStreamEvent::Popup(tab_id),
                        ServerMessage::Clipboard { text } => PreviewStreamEvent::Clipboard(text),
                        ServerMessage::Download { id, file_name, size_bytes } => PreviewStreamEvent::Download {
                            file_name,
                            size_bytes,
                            url: access
                                .http_url(&format!("{PREVIEW_STREAM_BASE_PATH}/download"), &[("threadId", &target.thread_id), ("tabId", &target.tab_id), ("id", &id)])
                                .to_string(),
                        },
                        ServerMessage::Viewport { width, height } => {
                            *failures = 0;
                            PreviewStreamEvent::Viewport(PreviewStreamViewport { width, height })
                        }
                        ServerMessage::Probe { x, y, editable } => PreviewStreamEvent::Probe { x, y, editable },
                        ServerMessage::Control(next) => {
                            _ = control.send(Some(next.clone()));
                            PreviewStreamEvent::Control(Some(next))
                        }
                    };
                    _ = events.send(event);
                }
                Some(Ok(Message::Close(frame))) => {
                    let code = frame.as_ref().map(|f| f.code);
                    match code.map(u16::from) {
                        Some(4404) => {
                            _ = events.send(PreviewStreamEvent::Gone);
                            break Ended::Stop;
                        }
                        Some(PREVIEW_STREAM_HOST_SETUP_CLOSE_CODE) => {
                            let reason = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                            let setup = serde_json::from_str(&reason).unwrap_or(HostSetup {
                                need: HostSetupNeed::Sandbox,
                                command: "sudo npx t3 browser setup".into(),
                            });
                            _ = events.send(PreviewStreamEvent::HostSetup(setup));
                            break Ended::Stop;
                        }
                        Some(1008 | 4401) => {
                            _ = events.send(PreviewStreamEvent::Unauthorized);
                            break Ended::Stop;
                        }
                        _ => break Ended::Retry,
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break Ended::Retry,
            }
        }
    };
    if file_chooser.is_some() {
        _ = events.send(PreviewStreamEvent::FileChooser(None));
    }
    _ = events.send(PreviewStreamEvent::Control(None));
    _ = events.send(PreviewStreamEvent::Connected(false));
    ended
}

/// Answers a page's file chooser: POSTs each file as a multipart `file`
/// part to the chooser's upload URL. No files cancels the page's picker.
pub async fn upload_files(chooser: &FileChooser, files: Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let boundary = format!("t3code-{:016x}", rand::random::<u64>());
    let mut body = Vec::new();
    for (name, bytes) in files {
        let name = name.replace(['"', '\r', '\n'], "_");
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = crate::rpc::http::http_client()
        .post(&chooser.upload_url)
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        return Ok(());
    }
    let text = response.text().await.unwrap_or_default();
    Err(if text.is_empty() { "The upload was refused.".into() } else { text })
}
