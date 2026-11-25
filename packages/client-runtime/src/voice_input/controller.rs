//! One dictation from start to transcript (`voice-input/controller.ts`). The client supplies
//! capture and transcription; the controller owns the phases, binds the transcriber for the
//! whole recording, and inserts the transcript only into the draft it started from, after
//! anything typed meanwhile, so a late transcript never overwrites an edit.
//!
//! Runs on one thread: methods return futures that hold the controller, and callbacks must
//! not call back into it synchronously.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt as _;
use futures::future::{LocalBoxFuture, Shared};

/// `VOICE_RECORDING_LIMIT_SECONDS`.
pub const VOICE_RECORDING_LIMIT_SECONDS: u64 = 5 * 60;

/// Cooperative cancellation shared with work on other threads. Work settles only after it
/// has stopped.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<(AtomicBool, tokio::sync::Notify)>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.0.store(true, Ordering::SeqCst);
        self.0.1.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.0.load(Ordering::SeqCst)
    }

    /// Resolves once cancelled.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.0.1.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// `VoiceInputPhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceInputPhase {
    Idle,
    Preparing,
    Recording,
    Transcribing,
    PostProcessing,
    Error,
}

/// What the error's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceErrorAction {
    Retry,
    Settings,
}

/// `VoiceInputErrorCode`: which known failure an error is, so clients label it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceInputErrorCode {
    MicrophoneBlocked,
    DraftUnavailable,
    AlreadyRecording,
    Unavailable,
    StillTranscribing,
    LanguageUnsupported,
    StartFailed,
    Interrupted,
    FinishFailed,
    TranscriptionFailed,
    DraftChanged,
    NoSpeech,
    MicrophoneFailed,
    MicrophoneDisconnected,
}

/// Copy the voice code shows, kept as keys so the client translates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceMessage {
    DraftUnavailable,
    AnotherRecordingActive,
    TranscriptionUnavailable,
    MicrophoneRequired,
    /// `MICROPHONE_PERMISSION_ERROR`.
    MicrophonePermission,
    StartFailed,
    StillFinishing,
    LanguageUnsupported,
    PrepareFailed,
    TranscribeFailed,
    Interrupted,
    FinishFailed,
    DraftChanged,
    NoSpeech,
    Cancelled,
    MicrophoneProcessingFailed,
    MicrophoneDisconnected,
    CaptureDidNotStop,
    /// "Voice transcription failed. Check the destination in Settings → Voice."
    DestinationFailed,
    ChooseOpenRouterModel,
}

/// Error copy: a known message or text from a server or provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceText {
    Known(VoiceMessage),
    Raw(String),
}

impl From<VoiceMessage> for VoiceText {
    fn from(message: VoiceMessage) -> Self {
        Self::Known(message)
    }
}

/// `VoiceInputState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceInputState {
    pub phase: VoiceInputPhase,
    pub error: Option<VoiceText>,
    pub error_action: Option<VoiceErrorAction>,
    pub error_code: Option<VoiceInputErrorCode>,
}

impl VoiceInputState {
    pub const IDLE: Self = Self { phase: VoiceInputPhase::Idle, error: None, error_action: None, error_code: None };

    fn phase(phase: VoiceInputPhase) -> Self {
        Self { phase, ..Self::IDLE }
    }

    /// `voiceInputBlocksSubmission`: a send waits for the transcript.
    pub fn blocks_submission(&self) -> bool {
        matches!(
            self.phase,
            VoiceInputPhase::Preparing | VoiceInputPhase::Recording | VoiceInputPhase::Transcribing | VoiceInputPhase::PostProcessing
        )
    }
}

/// `VoiceDraftSnapshot`. Offsets are UTF-8 byte offsets on character boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceDraftSnapshot {
    pub owner_key: String,
    pub text: String,
    pub selection: Range<usize>,
}

/// `VoiceTranscriptionErrorCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceTranscriptionErrorCode {
    Unavailable,
    UnsupportedLocale,
    PreparationFailed,
    TranscriptionFailed,
    Cancelled,
}

/// `VoiceTranscriptionError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceTranscriptionError {
    pub code: VoiceTranscriptionErrorCode,
    pub message: VoiceText,
}

impl VoiceTranscriptionError {
    pub fn new(code: VoiceTranscriptionErrorCode, message: impl Into<VoiceText>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn cancelled() -> Self {
        Self::new(VoiceTranscriptionErrorCode::Cancelled, VoiceMessage::Cancelled)
    }
}

/// A finished recording: mono 16 kHz `f32` little-endian PCM.
pub type Recording = Arc<[u8]>;

/// How capture failed to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderError {
    /// The system refused the microphone (`NotAllowedError`).
    PermissionDenied,
    Failed(String),
}

/// `VoiceRecorder`: the client's capture.
pub trait VoiceRecorder {
    /// The finished recording, once `stop` buffered one (batch transcription only).
    fn recording(&self) -> Option<Recording>;
    /// Opens the microphone.
    fn prepare(&self, cancel: CancelToken) -> LocalBoxFuture<'static, Result<(), RecorderError>>;
    /// Starts keeping audio; the client stops it at the limit.
    fn record(&self, limit_seconds: u64);
    /// Stops capture and settles once the last audio is delivered.
    fn stop(&self) -> LocalBoxFuture<'static, Result<(), String>>;
}

/// Transcribes a finished recording.
pub type TranscribeFn = Rc<dyn Fn(Recording, CancelToken) -> LocalBoxFuture<'static, Result<String, VoiceTranscriptionError>>>;
/// Finishes a live stream and returns its transcript.
pub type FinishStreamFn = Rc<dyn Fn(CancelToken) -> LocalBoxFuture<'static, Result<String, VoiceTranscriptionError>>>;

/// `PreparedVoiceTranscription`: bound for one whole recording.
#[derive(Clone)]
pub struct PreparedVoiceTranscription {
    pub locale: String,
    pub transcribe: TranscribeFn,
    /// Present when audio streams during capture; it replaces `transcribe`.
    pub streaming: Option<FinishStreamFn>,
}

/// `VoiceTranscriber`.
pub trait VoiceTranscriber {
    fn prepare(&self, cancel: CancelToken) -> LocalBoxFuture<'static, Result<PreparedVoiceTranscription, VoiceTranscriptionError>>;
}

/// Optional transcript cleanup. `None` skips it for this recording; a failure falls back to
/// the raw transcript.
pub type PostProcessFn = Box<dyn Fn(&str, &VoiceDraftSnapshot, CancelToken) -> Option<LocalBoxFuture<'static, Result<String, String>>>>;

/// `VoiceInputControllerDependencies`.
pub struct VoiceInputControllerDependencies {
    pub recorder: Rc<dyn VoiceRecorder>,
    pub get_transcriber: Box<dyn Fn() -> Option<Rc<dyn VoiceTranscriber>>>,
    /// `{granted, canAskAgain}`.
    pub request_permission: Box<dyn Fn() -> LocalBoxFuture<'static, (bool, bool)>>,
    /// Releases the audio session after capture.
    pub release_recording: Box<dyn Fn()>,
    /// Keep a recording whose transcription failed so the next start retries it.
    pub retry_failed_recording: bool,
    pub post_process: Option<PostProcessFn>,
    pub on_post_processing_error: Box<dyn Fn()>,
    pub read_draft: Box<dyn Fn() -> Option<VoiceDraftSnapshot>>,
    pub commit_draft: Box<dyn Fn(String, Range<usize>)>,
    pub on_state_change: Box<dyn Fn(&VoiceInputState)>,
}

/// `TranscriptCommitResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptCommit {
    Commit { text: String, cursor: usize },
    Stale,
    Empty,
}

const WORD_SPACELESS_LANGUAGES: &[&str] = &["ja", "zh", "yue", "th", "lo", "km", "my"];

/// `mapPosition`: where `position` in `before` lands in `text`, after anything inserted there.
/// Compares characters, so a boundary never splits one.
fn map_position(before: &str, text: &str, position: usize, suffix_first: bool) -> usize {
    let b: Vec<(usize, char)> = before.char_indices().collect();
    let t: Vec<(usize, char)> = text.char_indices().collect();
    let shared = b.len().min(t.len());
    let matches_from_end = |offset: usize| b[b.len() - 1 - offset].1 == t[t.len() - 1 - offset].1;
    let (mut prefix, mut suffix) = (0usize, 0usize);
    if suffix_first {
        while suffix < shared && matches_from_end(suffix) {
            suffix += 1;
        }
    }
    while prefix < shared - suffix && b[prefix].1 == t[prefix].1 {
        prefix += 1;
    }
    if !suffix_first {
        while suffix < shared - prefix && matches_from_end(suffix) {
            suffix += 1;
        }
    }
    let byte_at = |chars: &[(usize, char)], full: &str, index: usize| chars.get(index).map_or(full.len(), |(at, _)| *at);
    let prefix_bytes = byte_at(&b, before, prefix);
    let suffix_start_before = byte_at(&b, before, b.len() - suffix);
    if position < prefix_bytes {
        return position;
    }
    if position >= suffix_start_before {
        return position + text.len() - before.len();
    }
    byte_at(&t, text, t.len() - suffix)
}

fn is_latin_or_number(c: char) -> bool {
    c.is_ascii_alphanumeric() || (c.is_alphabetic() && (c as u32) < 0x0250) || c.is_numeric()
}

/// `resolveTranscriptCommit`: where and how the transcript goes into the current draft.
pub fn resolve_transcript_commit(
    captured: &VoiceDraftSnapshot,
    current: Option<&VoiceDraftSnapshot>,
    transcript: &str,
    locale: &str,
) -> TranscriptCommit {
    let Some(current) = current.filter(|current| current.owner_key == captured.owner_key) else {
        return TranscriptCommit::Stale;
    };
    let replacement = transcript.trim();
    if replacement.is_empty() {
        return TranscriptCommit::Empty;
    }
    let text = current.text.as_str();
    // An edit made while recording is never replaced. Its position is ambiguous over repeated
    // text, so insert after the selection under both extreme alignments.
    let unchanged = text == captured.text;
    let end = if unchanged {
        captured.selection.end
    } else {
        map_position(&captured.text, text, captured.selection.end, false).max(map_position(&captured.text, text, captured.selection.end, true))
    };
    let start = if unchanged { captured.selection.start } else { end };
    let (start, end) = (start.min(text.len()), end.min(text.len()));
    let language = locale.split(['-', '_']).next().unwrap_or("").to_lowercase();
    let mut insertion = replacement.to_owned();
    if start == end && !WORD_SPACELESS_LANGUAGES.contains(&language.as_str()) {
        let auto = language == "auto";
        let left: String = text[..start].chars().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect();
        let right: String = text[start..].chars().take(2).collect();
        let left_last = left.chars().last();
        let right_first = right.chars().next();
        let left_edge = left_last.is_some_and(|c| {
            if auto {
                c.is_ascii_alphanumeric() || ".!?,:;)]}'\"".contains(c)
            } else {
                c.is_alphanumeric() || ".!?,:;)]}'\"".contains(c)
            }
        });
        let right_edge = right_first.is_some_and(|c| {
            if auto { c.is_ascii_alphanumeric() || "([{'\"".contains(c) } else { c.is_alphanumeric() || "([{'\"".contains(c) }
        });
        let left_needs = (!auto || replacement.chars().next().is_some_and(is_latin_or_number))
            && left_edge
            && right_first.is_none_or(char::is_whitespace);
        let right_needs = (!auto || replacement.chars().last().is_some_and(is_latin_or_number))
            && right_edge
            && left_last.is_none_or(char::is_whitespace);
        insertion = format!("{}{replacement}{}", if left_needs { " " } else { "" }, if right_needs { " " } else { "" });
    }
    let mut result = String::with_capacity(text.len() + insertion.len());
    result.push_str(&text[..start]);
    result.push_str(&insertion);
    let cursor = result.len();
    result.push_str(&text[end..]);
    TranscriptCommit::Commit { text: result, cursor }
}

/// The microphone is one device: one session at a time, process-wide.
struct SessionToken {
    abandoned: Cell<bool>,
    released: Shared<futures::channel::oneshot::Receiver<()>>,
    release: RefCell<Option<futures::channel::oneshot::Sender<()>>>,
}

thread_local! {
    static ACTIVE_SESSION: RefCell<Option<Rc<SessionToken>>> = const { RefCell::new(None) };
    static TRANSCRIPTION_BUSY: Cell<bool> = const { Cell::new(false) };
}

fn acquire_session() -> Option<Rc<SessionToken>> {
    ACTIVE_SESSION.with(|active| {
        if active.borrow().is_some() {
            return None;
        }
        let (release, released) = futures::channel::oneshot::channel();
        let token = Rc::new(SessionToken { abandoned: Cell::new(false), released: released.shared(), release: RefCell::new(Some(release)) });
        *active.borrow_mut() = Some(token.clone());
        Some(token)
    })
}

fn release_session(token: Option<Rc<SessionToken>>) {
    let Some(token) = token else { return };
    ACTIVE_SESSION.with(|active| {
        let mut active = active.borrow_mut();
        if active.as_ref().is_some_and(|current| Rc::ptr_eq(current, &token)) {
            *active = None;
            if let Some(release) = token.release.borrow_mut().take() {
                let _ = release.send(());
            }
        }
    });
}

/// `runTranscriptionOperation`: one transcription operation at a time.
async fn run_transcription_operation<T>(operation: impl Future<Output = Result<T, VoiceTranscriptionError>>) -> Result<T, OperationError> {
    if TRANSCRIPTION_BUSY.with(Cell::get) {
        return Err(OperationError::Busy);
    }
    TRANSCRIPTION_BUSY.with(|busy| busy.set(true));
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            TRANSCRIPTION_BUSY.with(|busy| busy.set(false));
        }
    }
    let _clear = Clear;
    operation.await.map_err(OperationError::Failed)
}

enum OperationError {
    Busy,
    Failed(VoiceTranscriptionError),
}

struct Failure {
    message: VoiceText,
    code: Option<VoiceInputErrorCode>,
}

fn preparation_failure(error: OperationError) -> Failure {
    match error {
        OperationError::Busy => Failure { message: VoiceMessage::StillFinishing.into(), code: Some(VoiceInputErrorCode::StillTranscribing) },
        OperationError::Failed(error) if error.code == VoiceTranscriptionErrorCode::UnsupportedLocale => {
            Failure { message: VoiceMessage::LanguageUnsupported.into(), code: Some(VoiceInputErrorCode::LanguageUnsupported) }
        }
        OperationError::Failed(error) => match error.message {
            VoiceText::Raw(text) if text.trim().is_empty() => {
                Failure { message: VoiceMessage::PrepareFailed.into(), code: Some(VoiceInputErrorCode::StartFailed) }
            }
            message => Failure { message, code: None },
        },
    }
}

fn transcription_failure(error: OperationError) -> Failure {
    match error {
        OperationError::Busy => Failure { message: VoiceMessage::StillFinishing.into(), code: Some(VoiceInputErrorCode::StillTranscribing) },
        OperationError::Failed(error) => Failure { message: error.message, code: None },
    }
}

/// A recording kept after its transcription failed, retried by the next start.
struct RetryRecording {
    recording: Recording,
    owner_key: String,
    transcription: PreparedVoiceTranscription,
}

#[derive(Default)]
struct Inner {
    operation_token: u64,
    session_token: Option<Rc<SessionToken>>,
    transcription: Option<PreparedVoiceTranscription>,
    transcription_cancel: Option<CancelToken>,
    post_processing_cancel: Option<CancelToken>,
    skip_post_processing_requested: bool,
    captured_draft: Option<VoiceDraftSnapshot>,
    recording: Option<Recording>,
    recording_configured: bool,
    finishing: bool,
    retry: Option<RetryRecording>,
}

/// `VoiceInputController`. Hold it in an `Rc` and drive its futures on one thread.
pub struct VoiceInputController {
    deps: VoiceInputControllerDependencies,
    state: RefCell<VoiceInputState>,
    inner: RefCell<Inner>,
}

type LocalFuture = Pin<Box<dyn Future<Output = ()>>>;

impl VoiceInputController {
    pub fn new(deps: VoiceInputControllerDependencies) -> Rc<Self> {
        Rc::new(Self { deps, state: RefCell::new(VoiceInputState::IDLE), inner: RefCell::default() })
    }

    pub fn state(&self) -> VoiceInputState {
        self.state.borrow().clone()
    }

    fn phase(&self) -> VoiceInputPhase {
        self.state.borrow().phase
    }

    fn is_current(&self, token: u64) -> bool {
        self.inner.borrow().operation_token == token
    }

    fn set_state(&self, state: VoiceInputState) {
        *self.state.borrow_mut() = state.clone();
        (self.deps.on_state_change)(&state);
    }

    fn set_error(&self, message: impl Into<VoiceText>, action: Option<VoiceErrorAction>, code: Option<VoiceInputErrorCode>) {
        self.set_state(VoiceInputState { phase: VoiceInputPhase::Error, error: Some(message.into()), error_action: action, error_code: code });
    }

    fn set_failure(&self, failure: Failure, action: Option<VoiceErrorAction>) {
        self.set_error(failure.message, action, failure.code);
    }

    /// `start`: from idle or an error, prepare the destination, open the microphone, record.
    pub fn start(self: &Rc<Self>) -> LocalFuture {
        let this = self.clone();
        Box::pin(async move { this.run_start().await })
    }

    async fn run_start(self: Rc<Self>) {
        if !matches!(self.phase(), VoiceInputPhase::Idle | VoiceInputPhase::Error) {
            return;
        }
        let Some(initiating) = (self.deps.read_draft)() else {
            self.set_error(VoiceMessage::DraftUnavailable, Some(VoiceErrorAction::Retry), Some(VoiceInputErrorCode::DraftUnavailable));
            return;
        };
        let token = {
            let mut inner = self.inner.borrow_mut();
            inner.operation_token += 1;
            inner.operation_token
        };
        self.set_state(VoiceInputState::phase(VoiceInputPhase::Preparing));
        let session = self.clone().start_session(token, initiating).await;
        // Release on an error, or when this operation was replaced and nothing finishes it.
        if let Some(session) = session {
            let owned = self.inner.borrow().session_token.as_ref().is_some_and(|current| Rc::ptr_eq(current, &session));
            if owned {
                let current = self.is_current(token);
                let finishing = self.inner.borrow().finishing;
                if (current && self.phase() == VoiceInputPhase::Error) || (!current && !finishing) {
                    self.release_resources().await;
                }
            }
        }
    }

    /// The body of `start`; returns the session it acquired.
    async fn start_session(self: Rc<Self>, token: u64, initiating: VoiceDraftSnapshot) -> Option<Rc<SessionToken>> {
        loop {
            let previous = ACTIVE_SESSION.with(|active| active.borrow().clone());
            match previous {
                Some(previous) if previous.abandoned.get() => {
                    previous.released.clone().await.ok();
                    if !self.is_current(token) {
                        return None;
                    }
                }
                _ => break,
            }
        }
        let Some(session) = acquire_session() else {
            self.set_error(VoiceMessage::AnotherRecordingActive, Some(VoiceErrorAction::Retry), Some(VoiceInputErrorCode::AlreadyRecording));
            return None;
        };
        let cancel = CancelToken::default();
        {
            let mut inner = self.inner.borrow_mut();
            inner.session_token = Some(session.clone());
            inner.transcription_cancel = Some(cancel.clone());
            if inner.retry.as_ref().is_some_and(|retry| retry.owner_key != initiating.owner_key) {
                inner.retry = None;
            }
        }
        let retry = self.inner.borrow_mut().retry.take();
        if let Some(retry) = retry {
            {
                let mut inner = self.inner.borrow_mut();
                inner.recording = Some(retry.recording.clone());
                inner.captured_draft = Some(initiating);
                inner.transcription = Some(retry.transcription);
            }
            self.finish_recording(true, Some(retry.recording)).await;
            return Some(session);
        }
        let Some(transcriber) = (self.deps.get_transcriber)() else {
            self.set_error(VoiceMessage::TranscriptionUnavailable, None, Some(VoiceInputErrorCode::Unavailable));
            return Some(session);
        };
        let (granted, can_ask_again) = (self.deps.request_permission)().await;
        if !self.is_current(token) {
            return Some(session);
        }
        if !granted {
            let action = if can_ask_again { VoiceErrorAction::Retry } else { VoiceErrorAction::Settings };
            self.set_error(VoiceMessage::MicrophoneRequired, Some(action), Some(VoiceInputErrorCode::MicrophoneBlocked));
            return Some(session);
        }
        let prepared = run_transcription_operation(transcriber.prepare(cancel.clone())).await;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                if self.is_current(token) {
                    self.set_failure(preparation_failure(error), Some(VoiceErrorAction::Retry));
                }
                return Some(session);
            }
        };
        if !self.is_current(token) {
            return Some(session);
        }
        self.inner.borrow_mut().transcription = Some(prepared);
        self.inner.borrow_mut().recording_configured = true;
        if let Err(error) = self.deps.recorder.prepare(cancel.clone()).await {
            if self.is_current(token) {
                let (message, code) = match error {
                    RecorderError::PermissionDenied => (VoiceMessage::MicrophonePermission, VoiceInputErrorCode::MicrophoneBlocked),
                    RecorderError::Failed(_) => (VoiceMessage::StartFailed, VoiceInputErrorCode::StartFailed),
                };
                self.set_error(message, Some(VoiceErrorAction::Retry), Some(code));
            }
            return Some(session);
        }
        if !self.is_current(token) {
            return Some(session);
        }
        let captured = (self.deps.read_draft)().filter(|draft| draft.owner_key == initiating.owner_key);
        let Some(captured) = captured else {
            self.set_error(VoiceMessage::DraftUnavailable, Some(VoiceErrorAction::Retry), Some(VoiceInputErrorCode::DraftUnavailable));
            return Some(session);
        };
        self.inner.borrow_mut().captured_draft = Some(captured);
        self.deps.recorder.record(VOICE_RECORDING_LIMIT_SECONDS);
        self.set_state(VoiceInputState::phase(VoiceInputPhase::Recording));
        Some(session)
    }

    /// `stop`: finish the recording and insert its transcript.
    pub fn stop(self: &Rc<Self>) -> LocalFuture {
        let this = self.clone();
        Box::pin(async move {
            if this.phase() == VoiceInputPhase::Recording {
                this.finish_recording(false, None).await;
            }
        })
    }

    /// `cancel`: abandons whatever runs and returns to idle.
    pub fn cancel(self: &Rc<Self>) -> LocalFuture {
        let this = self.clone();
        Box::pin(async move {
            match this.phase() {
                VoiceInputPhase::Idle => {}
                VoiceInputPhase::Error => {
                    this.inner.borrow_mut().retry = None;
                    this.set_state(VoiceInputState::IDLE);
                }
                VoiceInputPhase::Preparing | VoiceInputPhase::Transcribing | VoiceInputPhase::PostProcessing => {
                    this.invalidate_operation();
                    this.set_state(VoiceInputState::IDLE);
                }
                VoiceInputPhase::Recording => this.discard_recording(None).await,
            }
        })
    }

    /// `skipPostProcessing`: insert the raw transcript now.
    pub fn skip_post_processing(&self) {
        if self.phase() != VoiceInputPhase::PostProcessing {
            return;
        }
        let cancel = {
            let mut inner = self.inner.borrow_mut();
            inner.skip_post_processing_requested = true;
            inner.post_processing_cancel.clone()
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
    }

    /// `interruptRecording`: capture failed or the device went away.
    pub fn interrupt_recording(self: &Rc<Self>, message: Option<VoiceText>, code: Option<VoiceInputErrorCode>) -> LocalFuture {
        let this = self.clone();
        Box::pin(async move {
            let (message, code) = match message {
                None => (VoiceMessage::Interrupted.into(), Some(VoiceInputErrorCode::Interrupted)),
                Some(message) => (message, code),
            };
            match this.phase() {
                VoiceInputPhase::Preparing => {
                    this.invalidate_operation();
                    this.set_failure(Failure { message, code }, Some(VoiceErrorAction::Retry));
                }
                VoiceInputPhase::Recording => this.discard_recording(Some(Failure { message, code })).await,
                _ => {}
            }
        })
    }

    /// `ownerChanged`: the draft's editor shows another draft.
    pub fn owner_changed(self: &Rc<Self>) -> LocalFuture {
        if self.phase() == VoiceInputPhase::Idle {
            return Box::pin(async {});
        }
        self.cancel()
    }

    async fn finish_recording(self: &Rc<Self>, already_stopped: bool, completed: Option<Recording>) {
        if self.inner.borrow().finishing || !matches!(self.phase(), VoiceInputPhase::Recording | VoiceInputPhase::Preparing) {
            return;
        }
        self.inner.borrow_mut().finishing = true;
        let token = self.inner.borrow().operation_token;
        self.set_state(VoiceInputState::phase(VoiceInputPhase::Transcribing));
        self.clone().transcribe_and_commit(token, already_stopped, completed).await;
        self.inner.borrow_mut().finishing = false;
        self.release_resources().await;
    }

    async fn transcribe_and_commit(self: Rc<Self>, token: u64, already_stopped: bool, completed: Option<Recording>) {
        if !already_stopped && self.deps.recorder.stop().await.is_err() {
            if self.is_current(token) {
                self.set_error(VoiceMessage::FinishFailed, Some(VoiceErrorAction::Retry), Some(VoiceInputErrorCode::FinishFailed));
            }
            return;
        }
        self.release_audio_session();
        let recording = completed.or_else(|| self.deps.recorder.recording()).or_else(|| self.inner.borrow().recording.clone());
        self.inner.borrow_mut().recording = recording.clone();
        if !self.is_current(token) {
            return;
        }
        let (transcription, cancel, captured) = {
            let inner = self.inner.borrow();
            (inner.transcription.clone(), inner.transcription_cancel.clone(), inner.captured_draft.clone())
        };
        let (Some(transcription), Some(cancel), Some(captured)) = (transcription, cancel, captured) else {
            self.set_error(VoiceMessage::FinishFailed, Some(VoiceErrorAction::Retry), Some(VoiceInputErrorCode::FinishFailed));
            return;
        };
        if transcription.streaming.is_none() && recording.is_none() {
            self.set_error(VoiceMessage::FinishFailed, Some(VoiceErrorAction::Retry), Some(VoiceInputErrorCode::FinishFailed));
            return;
        }
        let result = match (&transcription.streaming, &recording) {
            (Some(finish), _) => run_transcription_operation(finish(cancel.clone())).await,
            (None, Some(recording)) => run_transcription_operation((transcription.transcribe)(recording.clone(), cancel.clone())).await,
            (None, None) => unreachable!("checked above"),
        };
        let mut transcript = match result {
            Ok(transcript) => transcript,
            Err(error) => {
                if self.is_current(token) {
                    if self.deps.retry_failed_recording
                        && transcription.streaming.is_none()
                        && let Some(recording) = recording
                    {
                        self.inner.borrow_mut().retry =
                            Some(RetryRecording { recording, owner_key: captured.owner_key.clone(), transcription: transcription.clone() });
                    }
                    self.set_failure(transcription_failure(error), Some(VoiceErrorAction::Retry));
                }
                return;
            }
        };
        if !self.is_current(token) {
            return;
        }
        let post_cancel = CancelToken::default();
        let cleanup = if transcript.trim().is_empty() {
            None
        } else {
            self.deps.post_process.as_ref().and_then(|post| post(&transcript, &captured, post_cancel.clone()))
        };
        if let Some(cleanup) = cleanup {
            self.set_state(VoiceInputState::phase(VoiceInputPhase::PostProcessing));
            {
                let mut inner = self.inner.borrow_mut();
                inner.post_processing_cancel = Some(post_cancel.clone());
                inner.skip_post_processing_requested = false;
            }
            // Skip resolves at once with the raw transcript; the request is not awaited.
            let skipped = post_cancel.cancelled();
            futures::pin_mut!(skipped);
            match futures::future::select(cleanup, skipped).await {
                futures::future::Either::Left((Ok(cleaned), _)) => transcript = cleaned,
                futures::future::Either::Left((Err(_), _)) => {
                    if !self.inner.borrow().skip_post_processing_requested {
                        (self.deps.on_post_processing_error)();
                    }
                }
                futures::future::Either::Right(_) => {}
            }
            if !self.is_current(token) {
                return;
            }
        }
        match resolve_transcript_commit(&captured, (self.deps.read_draft)().as_ref(), &transcript, &transcription.locale) {
            TranscriptCommit::Stale => {
                self.set_error(VoiceMessage::DraftChanged, Some(VoiceErrorAction::Retry), Some(VoiceInputErrorCode::DraftChanged));
            }
            TranscriptCommit::Empty => {
                self.set_error(VoiceMessage::NoSpeech, Some(VoiceErrorAction::Retry), Some(VoiceInputErrorCode::NoSpeech));
            }
            TranscriptCommit::Commit { text, cursor } => {
                (self.deps.commit_draft)(text, cursor..cursor);
                self.set_state(VoiceInputState::IDLE);
            }
        }
    }

    async fn discard_recording(self: &Rc<Self>, error: Option<Failure>) {
        self.invalidate_operation();
        match error {
            Some(failure) => self.set_failure(failure, Some(VoiceErrorAction::Retry)),
            None => self.set_state(VoiceInputState::IDLE),
        }
        self.release_resources().await;
    }

    async fn release_resources(&self) {
        let configured = {
            let inner = self.inner.borrow();
            if let Some(cancel) = &inner.transcription_cancel {
                cancel.cancel();
            }
            if let Some(cancel) = &inner.post_processing_cancel {
                cancel.cancel();
            }
            inner.recording_configured
        };
        if configured {
            let _ = self.deps.recorder.stop().await;
        }
        self.release_audio_session();
        let session = {
            let mut inner = self.inner.borrow_mut();
            inner.recording = None;
            inner.captured_draft = None;
            inner.transcription = None;
            inner.transcription_cancel = None;
            inner.post_processing_cancel = None;
            inner.skip_post_processing_requested = false;
            inner.session_token.take()
        };
        release_session(session);
    }

    fn release_audio_session(&self) {
        let configured = std::mem::take(&mut self.inner.borrow_mut().recording_configured);
        if configured {
            (self.deps.release_recording)();
        }
    }

    fn invalidate_operation(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.operation_token += 1;
        if let Some(session) = &inner.session_token {
            session.abandoned.set(true);
        }
        if let Some(cancel) = &inner.transcription_cancel {
            cancel.cancel();
        }
        if let Some(cancel) = &inner.post_processing_cancel {
            cancel.cancel();
        }
    }
}
