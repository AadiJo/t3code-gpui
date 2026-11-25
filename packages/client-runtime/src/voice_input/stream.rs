//! The live transcription socket (`voice-input/stream.ts`): audio leaves during capture in
//! one-second chunks with at most two unacknowledged, the environment answers each with the
//! text so far, and `finish` waits for the final transcript.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use super::speech::{
    SPEECH_STREAM_MAX_CHUNK_BYTES, SPEECH_STREAM_MAX_QUEUED_BYTES, SpeechStreamCommand, SpeechStreamEvent, SpeechStreamText,
    SpeechTranscriptionOptions,
};
use crate::rpc::http::tls_config;

/// No answer for this long ends the stream.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
/// Chunks sent and not yet acknowledged.
const SEND_WINDOW: usize = 2;

/// Whether the consent the recording started under still holds.
pub type CanSend = Arc<dyn Fn() -> bool + Send + Sync>;

enum Command {
    Feed(Vec<u8>),
    Finish,
}

/// How the stream ended.
type Outcome = Option<Result<String, String>>;

/// One open live transcription. Dropping it cancels the stream.
pub struct SpeechStream {
    commands: mpsc::UnboundedSender<Command>,
    cancel: watch::Sender<bool>,
    outcome: watch::Receiver<Outcome>,
}

impl SpeechStream {
    /// `openSpeechStream`. `on_text` gets every update; `on_error` gets a failure that is not
    /// a cancellation, which also settles `finish`.
    pub fn open(
        runtime: &tokio::runtime::Handle,
        url: Url,
        options: SpeechTranscriptionOptions,
        can_send: CanSend,
        on_text: impl Fn(SpeechStreamText) + Send + Sync + 'static,
        on_error: impl Fn(String) + Send + 'static,
    ) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (outcome_tx, outcome) = watch::channel(None);
        let (cancel, cancelled) = watch::channel(false);
        runtime.spawn(async move {
            let result = run(url, options, can_send, receiver, cancelled, &on_text).await;
            if let Err(Failure { message, notify: true }) = &result {
                on_error(message.clone());
            }
            let _ = outcome_tx.send(Some(result.map_err(|failure| failure.message)));
        });
        Self { commands, cancel, outcome }
    }

    /// Queues captured mono 16 kHz samples.
    pub fn feed(&self, pcm: &[f32]) {
        let _ = self.commands.send(Command::Feed(super::speech::pcm_bytes(pcm)));
    }

    /// Sends what is queued, then `finish`, and waits for the final transcript.
    pub async fn finish(&self) -> Result<String, String> {
        let _ = self.commands.send(Command::Finish);
        self.settled().await
    }

    /// Ends the stream without a transcript.
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    async fn settled(&self) -> Result<String, String> {
        let mut outcome = self.outcome.clone();
        loop {
            if let Some(result) = outcome.borrow_and_update().clone() {
                return result;
            }
            if outcome.changed().await.is_err() {
                return Err("The voice connection closed. Please record again.".into());
            }
        }
    }
}

impl Drop for SpeechStream {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
    }
}

struct Failure {
    message: String,
    notify: bool,
}

fn fail(message: &str) -> Failure {
    Failure { message: message.to_owned(), notify: true }
}

async fn run(
    url: Url,
    options: SpeechTranscriptionOptions,
    can_send: CanSend,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut cancelled: watch::Receiver<bool>,
    on_text: &(impl Fn(SpeechStreamText) + Send + Sync),
) -> Result<String, Failure> {
    let cancellation = || Failure { message: "Voice transcription was cancelled.".into(), notify: false };
    let connector = tokio_tungstenite::Connector::Rustls(tls_config());
    let connect = tokio_tungstenite::connect_async_tls_with_config(url.as_str(), None, false, Some(connector));
    let mut socket = tokio::select! {
        result = tokio::time::timeout(RESPONSE_TIMEOUT, connect) => match result {
            Ok(Ok((socket, _))) => socket,
            Ok(Err(_)) => return Err(fail("Could not connect to live transcription.")),
            Err(_) => return Err(fail("Live transcription stopped responding.")),
        },
        () = cancel_requested(&mut cancelled) => return Err(cancellation()),
    };
    if !can_send() {
        return Err(fail("Voice consent was revoked."));
    }
    let start = serde_json::to_string(&SpeechStreamCommand::Start { options: &options }).unwrap_or_default();
    socket.send(Message::text(start)).await.map_err(|_| fail("Could not connect to live transcription."))?;

    let mut queue: VecDeque<Vec<u8>> = VecDeque::new();
    let mut queued_bytes = 0usize;
    let mut in_flight: VecDeque<usize> = VecDeque::new();
    let mut in_flight_bytes = 0usize;
    let mut ready = false;
    let mut finishing = false;
    let mut finish_sent = false;
    let mut revision = -1i64;
    let mut deadline = Some(tokio::time::Instant::now() + RESPONSE_TIMEOUT);

    loop {
        // `pump`: send while the window has room, then `finish` once everything is acknowledged.
        if ready && !finish_sent {
            if !can_send() {
                return Err(fail("Voice consent was revoked."));
            }
            while in_flight.len() < SEND_WINDOW {
                let Some(chunk) = queue.pop_front() else { break };
                in_flight.push_back(chunk.len());
                in_flight_bytes += chunk.len();
                queued_bytes -= chunk.len();
                socket.send(Message::binary(chunk)).await.map_err(|_| fail("Could not send microphone audio."))?;
                deadline = Some(tokio::time::Instant::now() + RESPONSE_TIMEOUT);
            }
            if finishing && queue.is_empty() && in_flight.is_empty() {
                finish_sent = true;
                let finish = serde_json::to_string(&SpeechStreamCommand::Finish).unwrap_or_default();
                socket.send(Message::text(finish)).await.map_err(|_| fail("Could not finish live transcription."))?;
                deadline = Some(tokio::time::Instant::now() + RESPONSE_TIMEOUT);
            }
        }
        let timeout = async {
            match deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Feed(bytes)) => {
                    if finishing {
                        continue;
                    }
                    if queued_bytes + in_flight_bytes + bytes.len() > SPEECH_STREAM_MAX_QUEUED_BYTES {
                        return Err(fail("Voice recordings are limited to five minutes."));
                    }
                    push_chunks(&mut queue, &mut queued_bytes, &bytes);
                }
                Some(Command::Finish) => finishing = true,
                None => {
                    let _ = socket.close(None).await;
                    return Err(cancellation());
                }
            },
            () = cancel_requested(&mut cancelled) => {
                let _ = socket.close(None).await;
                return Err(cancellation());
            }
            message = socket.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Binary(_))) => return Err(fail("Invalid speech stream response.")),
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                        return Err(fail("The voice connection closed. Please record again."));
                    }
                    Some(Ok(_)) => continue,
                };
                if !can_send() {
                    return Err(fail("Voice consent was revoked."));
                }
                let event: SpeechStreamEvent = serde_json::from_str(&text).map_err(|_| fail("Invalid speech stream response."))?;
                match event {
                    SpeechStreamEvent::Ready => {
                        if ready {
                            return Err(fail("Unexpected speech stream response."));
                        }
                        ready = true;
                        deadline = None;
                    }
                    SpeechStreamEvent::Update { revision: next, text } => {
                        let Some(acknowledged) = (next >= revision).then(|| in_flight.pop_front()).flatten() else {
                            return Err(fail("Unexpected speech stream response."));
                        };
                        revision = next;
                        in_flight_bytes -= acknowledged;
                        deadline = None;
                        if let Some(text) = text {
                            on_text(text);
                        }
                        if !in_flight.is_empty() {
                            deadline = Some(tokio::time::Instant::now() + RESPONSE_TIMEOUT);
                        }
                    }
                    SpeechStreamEvent::Finished { text } => {
                        if !finish_sent {
                            return Err(fail("Unexpected speech stream response."));
                        }
                        let _ = socket.close(None).await;
                        return Ok(text);
                    }
                    SpeechStreamEvent::Error { message } => return Err(Failure { message, notify: true }),
                }
            }
            () = timeout => return Err(fail("Live transcription stopped responding.")),
        }
    }
}

/// Resolves once the handle asked to cancel or went away.
async fn cancel_requested(cancelled: &mut watch::Receiver<bool>) {
    loop {
        if *cancelled.borrow_and_update() {
            return;
        }
        if cancelled.changed().await.is_err() {
            return;
        }
    }
}

/// Appends `bytes` to the queue, topping up the last chunk to one second first.
fn push_chunks(queue: &mut VecDeque<Vec<u8>>, queued_bytes: &mut usize, bytes: &[u8]) {
    let mut offset = 0;
    while offset < bytes.len() {
        if let Some(tail) = queue.back_mut().filter(|tail| tail.len() < SPEECH_STREAM_MAX_CHUNK_BYTES) {
            let appended = (SPEECH_STREAM_MAX_CHUNK_BYTES - tail.len()).min(bytes.len() - offset);
            tail.extend_from_slice(&bytes[offset..offset + appended]);
            *queued_bytes += appended;
            offset += appended;
            continue;
        }
        let end = (offset + SPEECH_STREAM_MAX_CHUNK_BYTES).min(bytes.len());
        queue.push_back(bytes[offset..end].to_vec());
        *queued_bytes += end - offset;
        offset = end;
    }
}
