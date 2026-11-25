//! The one dictation this app runs (`voiceInputSession.ts`, `browserVoiceInput.ts`,
//! `browserPcmCapture.ts`, `useEnvironmentSpeechInput.ts`): the destination from the audio
//! grant, its speech status, a controller bound to that destination, cpal capture, and either
//! the live socket or a finished-recording upload, whichever the environment and settings
//! choose. The recording belongs to the draft that started it, not to whichever composer is
//! on screen; while that draft is off screen the global indicator shows it.
//!
//! Controller work runs only in spawned tasks, so its callbacks can update the app.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::FutureExt as _;
use futures::future::LocalBoxFuture;
use gpui_kit::{App, AppContext as _, AsyncApp, Context, Entity, Global, SharedString, Task, WeakEntity, Window};
use t3code_client_runtime::preview::environment_link::EnvironmentLink;
use t3code_client_runtime::voice_input::environment as env;
use t3code_client_runtime::voice_input::speech::{
    CLOUD_SPEECH_LANGUAGES, SpeechStreamText, SpeechTranscriptionOptions, VoiceTranscriptionDestination, choose_setup_speech_model,
    effective_speech_language, pcm_bytes, resolve_speech_vocabulary,
};
use t3code_client_runtime::voice_input::stream::SpeechStream;
use t3code_client_runtime::voice_input::{
    CancelToken, PreparedVoiceTranscription, RecorderError, Recording, VoiceDraftSnapshot, VoiceInputController,
    VoiceInputControllerDependencies, VoiceInputErrorCode, VoiceInputPhase, VoiceInputState, VoiceMessage, VoiceRecorder, VoiceText,
    VoiceTranscriber, VoiceTranscriptionError, VoiceTranscriptionErrorCode,
};
use t3code_contracts::{
    CloudSpeechProvider, EnvironmentId, EnvironmentSpeechPostProcessingRequest, EnvironmentSpeechPostProcessingRequestDraft,
    EnvironmentSpeechPostProcessingRequestDraftSelection, EnvironmentSpeechState, EnvironmentSpeechStatus, SidebarAutoSettleAfterDays, SpeechCorrectionCue,
    SpeechCustomWords, SpeechPostProcessingOptions,
};

use super::pcm_capture::{CaptureEvent, CaptureFailure, CaptureOptions, OpenError, PcmCapture, STOP_TIMEOUT};
use super::{microphones, voice_consent};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::t;
use crate::state::StateBridge;

/// The destination a session is bound to; a change discards the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionKey {
    pub grant_id: String,
    pub destination: VoiceTranscriptionDestination,
}

/// The destination as the UI shows it (`useVoiceTranscriptionDestination`).
#[derive(Clone, Debug)]
pub struct ResolvedDestination {
    pub key: SessionKey,
    /// "OpenAI via Studio", or the environment's label.
    pub label: SharedString,
    /// Connected and able to transcribe (`prepared`).
    pub ready: bool,
}

/// What a recording needs from its draft, read when it starts.
#[derive(Clone)]
pub struct VoiceRecordingContext {
    /// The draft's own environment, where cleanup runs; `None` for a draft not bound to one yet.
    pub environment_id: Option<EnvironmentId>,
    pub project_name: Option<String>,
    pub project_words: SpeechCustomWords,
    /// False keeps the transcript off the cleanup provider (the transcription test).
    pub cleanup: bool,
}

/// The draft a recording writes into (`VoiceInputTarget`).
#[derive(Clone)]
pub struct VoiceTarget {
    pub owner_key: SharedString,
    /// Names the draft in the indicator while its editor is off screen.
    pub label: Option<SharedString>,
    pub read_draft: Rc<dyn Fn(&App) -> Option<(String, Range<usize>)>>,
    pub commit_draft: Rc<dyn Fn(String, Range<usize>, &mut App)>,
    /// Shows the draft again (the indicator's label button).
    pub open: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    pub context: VoiceRecordingContext,
}

/// `VoiceInputSessionActivity`: changes many times a second, read only by the owner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceActivity {
    pub level: f32,
    /// The waveform's history, newest last: each level shifts the bars left (`VoiceWaveform`).
    pub levels: [f32; WAVEFORM_BARS],
    pub preview: Option<SpeechStreamText>,
}

/// `WAVEFORM_BAR_COUNT`.
pub const WAVEFORM_BARS: usize = 24;

/// A send waiting for the transcript (`submitAfterDictation`).
struct PendingSubmission {
    owner_key: SharedString,
    committed: bool,
    submit: Box<dyn FnOnce(&mut App)>,
}

struct Session {
    key: SessionKey,
    controller: Rc<VoiceInputController>,
    platform: Rc<Platform>,
}

/// The app's dictation. One per app ([`VoiceInput::global`]).
pub struct VoiceInput {
    session: Option<Session>,
    state: VoiceInputState,
    target: Option<VoiceTarget>,
    recording_started_at: Option<Instant>,
    activity: VoiceActivity,
    presented: HashMap<SharedString, usize>,
    /// The status probe of a destination, by session key.
    status: Option<(SessionKey, Option<EnvironmentSpeechStatus>)>,
    probing: Option<(SessionKey, Task<()>)>,
    /// A start waiting for its status refresh (`WAITING_STATE`).
    queued_start: Option<SharedString>,
    pending_submission: Option<PendingSubmission>,
    /// False once the grant a session was made under is no longer in force.
    grant_valid: Arc<AtomicBool>,
    /// Ticks the elapsed time while recording.
    _clock: Option<Task<()>>,
    /// Bumped on every start request; stale status replies are dropped.
    start_request: u64,
    /// A hold released before recording began; it finishes once recording starts.
    stop_on_record: Option<SharedString>,
    reading_session: Option<(t3code_contracts::EnvironmentId, Task<()>)>,
    /// The model setup a start opened (`setup`), and the draft it records into once ready.
    setup: VoiceSetup,
    setup_target: Option<VoiceTarget>,
    /// The destination the setup was opened for; a change discards it.
    setup_key: Option<SessionKey>,
    _setup_poll: Option<Task<()>>,
}

/// `INITIAL_SETUP`: what the voice setup dialog shows when a start finds no model.
#[derive(Clone, Debug, Default)]
pub struct VoiceSetup {
    pub open: bool,
    /// 0 offers the download, 1 picks a microphone.
    pub step: u8,
    /// The offered model; its `recommended` is the setup's own pick, not the catalog's.
    pub model: Option<t3code_contracts::EnvironmentSpeechModel>,
    /// The language the offered model was chosen for.
    pub language: Option<String>,
    pub downloading: bool,
    pub catalog_failed: bool,
    pub error: Option<SharedString>,
}

/// Each destination environment's session, read once per run (`useEnvironmentScope`).
#[derive(Default)]
struct EnvironmentSessions(HashMap<t3code_contracts::EnvironmentId, Option<t3code_contracts::AuthSessionState>>);
impl Global for EnvironmentSessions {}

struct VoiceInputGlobal(Entity<VoiceInput>);
impl Global for VoiceInputGlobal {}

/// The waiting state shown while a start refreshes the status.
const WAITING: VoiceInputState =
    VoiceInputState { phase: VoiceInputPhase::Preparing, error: None, error_action: None, error_code: None };

impl VoiceInput {
    /// The app's instance, created on first use.
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<VoiceInputGlobal>() {
            return global.0.clone();
        }
        let entity = cx.new(|_| Self {
            session: None,
            state: VoiceInputState::IDLE,
            target: None,
            recording_started_at: None,
            activity: VoiceActivity::default(),
            presented: HashMap::new(),
            status: None,
            probing: None,
            queued_start: None,
            pending_submission: None,
            grant_valid: Arc::new(AtomicBool::new(true)),
            _clock: None,
            start_request: 0,
            stop_on_record: None,
            reading_session: None,
            setup: VoiceSetup::default(),
            setup_target: None,
            setup_key: None,
            _setup_poll: None,
        });
        cx.set_global(VoiceInputGlobal(entity.clone()));
        // Losing the destination's connection, or its capability, discards the recording.
        if ConnectionRuntime::is_running(cx) {
            let connections = ConnectionRuntime::state(cx);
            entity.update(cx, |_, cx| cx.observe(&connections, |this, _, cx| this.destination_changed(cx)).detach());
        }
        entity.update(cx, |this, cx| this.ensure_status(cx));
        entity
    }

    /// The instance if one exists, for reads during render.
    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<VoiceInputGlobal>().map(|global| global.0.clone())
    }

    /// The chosen destination, resolved against the connections now.
    pub fn destination(cx: &App) -> Option<ResolvedDestination> {
        let grant = voice_consent::audio_grant(cx)?;
        let environment_id = grant.destination.environment_id().clone();
        let connections = ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::state(cx));
        let environment = connections.as_ref().and_then(|state| state.read(cx).environment(&environment_id).cloned());
        let env_label = environment.as_ref().map(|env| SharedString::from(env.entry.label.clone()));
        let provider = grant.destination.cloud_provider();
        let label = match (provider, &env_label) {
            (Some(provider), Some(environment)) => {
                t!("{provider} via {environment}", provider = env::cloud_speech_label(provider), environment = environment.clone())
            }
            _ => env_label.clone().unwrap_or_default(),
        };
        let supported = StateBridge::environment(&environment_id, cx)
            .and_then(|live| live.read(cx).config.clone())
            .map(|config| {
                let capabilities = &config.environment.capabilities;
                if provider.is_some() { capabilities.cloud_voice_transcription } else { capabilities.voice_transcription }
            })
            .is_some_and(|flag| flag == Some(true));
        let connected = environment.as_ref().is_some_and(|env| env.is_connected());
        // `canDictate`: the session must grant dictation (orchestration:operate) on the destination.
        let supported = supported
            && Self::grants(&environment_id, t3code_contracts::AuthEnvironmentScope::OrchestrationOperate, cx) == Some(true);
        Some(ResolvedDestination { key: SessionKey { grant_id: grant.id, destination: grant.destination }, label, ready: supported && connected })
    }

    fn link(key: &SessionKey, cx: &App) -> Option<EnvironmentLink> {
        ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::global(cx).environment_link(key.destination.environment_id())).flatten()
    }

    /// The probed status of the chosen environment (`status`), when it is the current one.
    pub fn status(&self, cx: &App) -> Option<EnvironmentSpeechStatus> {
        let destination = Self::destination(cx)?;
        match &self.status {
            Some((key, status)) if *key == destination.key => status.clone(),
            _ => None,
        }
    }

    /// Whether `owner` can start dictation now (`available`): a ready destination whose
    /// status is not unsupported, and no other draft holding the microphone.
    pub fn available(&self, owner: &str, cx: &App) -> bool {
        let Some(destination) = Self::destination(cx) else { return false };
        if !destination.ready {
            return false;
        }
        let probed = destination.key.destination.cloud_provider().is_some()
            || matches!(&self.status, Some((key, status)) if *key == destination.key
                && !matches!(status, Some(EnvironmentSpeechStatus::NotSupported(_))));
        probed && !self.busy_elsewhere(owner)
    }

    /// Another draft's recording holds the one microphone.
    pub fn busy_elsewhere(&self, owner: &str) -> bool {
        self.state.blocks_submission() && self.target.as_ref().is_some_and(|target| target.owner_key.as_ref() != owner)
    }

    /// The state `owner` shows (`state`): its own, or idle.
    pub fn state_for(&self, owner: &str) -> VoiceInputState {
        if self.queued_start.as_deref() == Some(owner) {
            return WAITING;
        }
        if self.target.as_ref().is_some_and(|target| target.owner_key.as_ref() == owner) { self.state.clone() } else { VoiceInputState::IDLE }
    }

    /// The level and live text `owner` shows, if it owns the recording.
    pub fn activity_for(&self, owner: &str) -> Option<&VoiceActivity> {
        self.target.as_ref().filter(|target| target.owner_key.as_ref() == owner).map(|_| &self.activity)
    }

    pub fn recording_started_at(&self, owner: &str) -> Option<Instant> {
        self.target.as_ref().filter(|target| target.owner_key.as_ref() == owner).and(self.recording_started_at)
    }

    /// The recording's draft while it is off screen, for the global indicator.
    pub fn off_screen(&self) -> Option<(&VoiceTarget, &VoiceInputState)> {
        let target = self.target.as_ref()?;
        (self.state.phase != VoiceInputPhase::Idle && !self.presented.contains_key(&target.owner_key)).then_some((target, &self.state))
    }

    /// `presentVoiceOwner`: an editor shows `owner`; the indicator hides for it.
    pub fn present(&mut self, owner: SharedString, cx: &mut Context<Self>) {
        *self.presented.entry(owner).or_default() += 1;
        cx.notify();
    }

    /// The editor stopped showing `owner`.
    pub fn unpresent(&mut self, owner: &str, cx: &mut Context<Self>) {
        if let Some(count) = self.presented.get_mut(owner) {
            *count -= 1;
            if *count == 0 {
                self.presented.remove(owner);
            }
        }
        cx.notify();
    }

    /// Whether the session on `environment` grants `scope`, once read.
    pub fn grants(environment: &t3code_contracts::EnvironmentId, scope: t3code_contracts::AuthEnvironmentScope, cx: &App) -> Option<bool> {
        let session = cx.try_global::<EnvironmentSessions>()?.0.get(environment)?.as_ref();
        Some(session.is_some_and(|session| env::session_grants_scope(session, scope)))
    }

    /// Reads the grant's environment session once, so the destination can say whether it may dictate.
    fn ensure_session(&mut self, cx: &mut Context<Self>) {
        let Some(grant) = voice_consent::audio_grant(cx) else { return };
        let environment = grant.destination.environment_id().clone();
        let reading = self.reading_session.as_ref().is_some_and(|(id, _)| *id == environment);
        let known = cx.try_global::<EnvironmentSessions>().is_some_and(|sessions| sessions.0.contains_key(&environment));
        if known || reading {
            return;
        }
        let Some(link) = (ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::global(cx).environment_link(&environment))).flatten() else { return };
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        let id = environment.clone();
        let task = cx.spawn(async move |this, cx| {
            let session = tokio.spawn(async move { link.session_state().await }).await.ok().and_then(Result::ok);
            _ = this.update(cx, |this, cx| {
                cx.default_global::<EnvironmentSessions>().0.insert(id, session);
                this.reading_session = None;
                this.ensure_status(cx);
                cx.notify();
            });
        });
        self.reading_session = Some((environment, task));
    }

    /// Probes the chosen environment's status once per destination.
    pub fn ensure_status(&mut self, cx: &mut Context<Self>) {
        self.ensure_session(cx);
        let Some(destination) = Self::destination(cx) else { return };
        if !destination.ready || destination.key.destination.cloud_provider().is_some() {
            return;
        }
        let known = matches!(&self.status, Some((key, _)) if *key == destination.key);
        let probing = matches!(&self.probing, Some((key, _)) if *key == destination.key);
        if !known && !probing {
            self.probe(destination.key, cx);
        }
    }

    fn probe(&mut self, key: SessionKey, cx: &mut Context<Self>) {
        let Some(link) = Self::link(&key, cx) else { return };
        let task_key = key.clone();
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        let task = cx.spawn(async move |this, cx| {
            let result = tokio.spawn(async move { env::speech_status(&link).await }).await;
            let status = result.ok().and_then(Result::ok);
            _ = this.update(cx, |this, cx| {
                this.status = Some((task_key, status));
                this.probing = None;
                cx.notify();
            });
        });
        self.probing = Some((key, task));
    }

    /// The session for the chosen destination, replacing one bound to another.
    fn session_for(&mut self, key: &SessionKey, cx: &mut Context<Self>) -> Option<(Rc<VoiceInputController>, Rc<Platform>)> {
        if self.session.as_ref().is_some_and(|session| session.key != *key) {
            self.dispose_session(cx);
        }
        if self.session.is_none() {
            self.grant_valid = Arc::new(AtomicBool::new(true));
            let platform = Rc::new(Platform::new(key.clone(), self.grant_valid.clone(), cx.weak_entity(), cx.to_async()));
            let controller = make_controller(platform.clone(), cx.weak_entity(), cx.to_async());
            *platform.controller.borrow_mut() = Rc::downgrade(&controller);
            self.session = Some(Session { key: key.clone(), controller, platform });
        }
        self.session.as_ref().map(|session| (session.controller.clone(), session.platform.clone()))
    }

    fn dispose_session(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.session.take() {
            self.grant_valid.store(false, Ordering::SeqCst);
            let cancel = session.controller.cancel();
            cx.spawn(async move |_, _| cancel.await).detach();
            self.state = VoiceInputState::IDLE;
            self.activity = VoiceActivity::default();
            self.pending_submission = None;
            cx.notify();
        }
    }

    /// The grant or the connection changed: a session bound to another destination ends.
    pub fn destination_changed(&mut self, cx: &mut Context<Self>) {
        let current = Self::destination(cx).filter(|destination| destination.ready).map(|destination| destination.key);
        if self.session.as_ref().is_some_and(|session| Some(&session.key) != current.as_ref()) {
            self.dispose_session(cx);
            self.queued_start = None;
        }
        if self.setup_key.as_ref() != current.as_ref() && (self.setup.open || self.setup_target.is_some()) {
            self.reset_setup();
        }
        self.ensure_status(cx);
        cx.notify();
    }

    /// `start`: refreshes the environment's status, then records into `target`. With no
    /// model on the environment it opens the model setup instead.
    pub fn start(&mut self, target: VoiceTarget, cx: &mut Context<Self>) {
        if self.setup.open && self.setup.downloading {
            return;
        }
        let Some(destination) = Self::destination(cx).filter(|destination| destination.ready) else { return };
        if self.busy_elsewhere(&target.owner_key) {
            return;
        }
        self.start_request += 1;
        let request = self.start_request;
        let key = destination.key.clone();
        if key.destination.cloud_provider().is_some() {
            self.begin(key, target, cx);
            return;
        }
        let Some(link) = Self::link(&key, cx) else { return };
        self.reset_setup();
        self.queued_start = Some(target.owner_key.clone());
        cx.notify();
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        cx.spawn(async move |this, cx| {
            let result = tokio.spawn(async move { env::speech_status(&link).await }).await;
            _ = this.update(cx, |this, cx| {
                if this.start_request != request || this.queued_start.as_ref() != Some(&target.owner_key) {
                    return;
                }
                this.queued_start = None;
                cx.notify();
                match result.map_err(|error| error.to_string()).and_then(|status| status.map_err(|error| error.to_string())) {
                    Err(error) => toast(ToastKind::Error, t!("Could not start voice input"), Some(error.into()), cx),
                    Ok(status) => {
                        this.status = Some((key.clone(), Some(status.clone())));
                        match status {
                            EnvironmentSpeechStatus::NotSupported(status) => {
                                toast(ToastKind::Error, t!("Voice input is not available"), Some(status.reason.into()), cx)
                            }
                            EnvironmentSpeechStatus::Supported(status) if status.state == EnvironmentSpeechState::MissingModel => {
                                this.load_setup_catalog(key, target, status.model_id.as_str().to_owned(), status.benchmark_tier, request, cx)
                            }
                            _ => this.begin(key, target, cx),
                        }
                    }
                }
            });
        })
        .detach();
    }

    /// The model setup as the dialog shows it.
    pub fn setup(&self) -> &VoiceSetup {
        &self.setup
    }

    /// The environment the setup transcribes on, while it is open.
    pub fn setup_environment(&self) -> Option<&EnvironmentId> {
        self.setup_key.as_ref().map(|key| key.destination.environment_id())
    }

    fn reset_setup(&mut self) {
        self.setup = VoiceSetup::default();
        self.setup_target = None;
        self.setup_key = None;
        self._setup_poll = None;
    }

    /// `setOpen`: closing keeps the setup; the next start begins again.
    pub fn set_setup_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.setup.open = open;
        if !open {
            self._setup_poll = None;
        }
        cx.notify();
    }

    /// `loadSetupCatalog`: opens the dialog, then offers the model `chooseSetupSpeechModel` picks.
    fn load_setup_catalog(
        &mut self,
        key: SessionKey,
        target: VoiceTarget,
        selected_model_id: String,
        tier: Option<t3code_contracts::SpeechBenchmarkTier>,
        request: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(link) = Self::link(&key, cx) else { return };
        self.setup = VoiceSetup { open: true, ..VoiceSetup::default() };
        self.setup_target = Some(target);
        self.setup_key = Some(key);
        cx.notify();
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        cx.spawn(async move |this, cx| {
            let catalog = tokio.spawn(async move { env::speech_models(&link, true).await }).await.ok().and_then(Result::ok);
            _ = this.update(cx, |this, cx| {
                if this.start_request != request {
                    return;
                }
                let intent = ClientSettingsStore::get(cx).speech_language.as_str().to_owned();
                let models = catalog.as_ref().map(|catalog| catalog.models.as_slice()).unwrap_or_default();
                let choice = choose_setup_speech_model(models, &selected_model_id, &intent, &preferred_speech_languages(), tier);
                this.setup.catalog_failed = catalog.is_none();
                this.setup.error = match (&choice, &catalog) {
                    (Some(_), _) => None,
                    (None, Some(_)) => Some(t!("No model matches your language. Choose a model or change the language in Voice settings.")),
                    (None, None) => Some(t!("Could not load transcription models. Check the connection and try again.")),
                };
                this.setup.language = choice.as_ref().and_then(|choice| choice.language.clone());
                this.setup.model = choice.map(|choice| t3code_contracts::EnvironmentSpeechModel { recommended: choice.recommended, ..choice.model });
                cx.notify();
            });
        })
        .detach();
    }

    /// `retryCatalog`: offered while the catalog failed and the status is supported.
    pub fn can_retry_setup_catalog(&self, cx: &App) -> bool {
        self.setup.open && self.setup.catalog_failed && matches!(self.status(cx), Some(EnvironmentSpeechStatus::Supported(_)))
    }

    pub fn retry_setup_catalog(&mut self, cx: &mut Context<Self>) {
        let (Some(EnvironmentSpeechStatus::Supported(status)), Some(key), Some(target)) =
            (self.status(cx), self.setup_key.clone(), self.setup_target.clone())
        else {
            return;
        };
        self.start_request += 1;
        let request = self.start_request;
        self.load_setup_catalog(key, target, status.model_id.as_str().to_owned(), status.benchmark_tier, request, cx);
    }

    /// `downloadSetupModel`: downloads the offered model, selects it if the download did not,
    /// and moves to the microphone step once the environment reports it ready.
    pub fn download_setup_model(&mut self, cx: &mut Context<Self>) {
        let supported = matches!(self.status(cx), Some(EnvironmentSpeechStatus::Supported(_)));
        let (Some(key), Some(model)) = (self.setup_key.clone(), self.setup.model.as_ref()) else { return };
        if !supported || self.setup.downloading {
            return;
        }
        let Some(link) = Self::link(&key, cx) else { return };
        let model_id = model.id.as_str().to_owned();
        self.start_request += 1;
        let request = self.start_request;
        self.setup.downloading = true;
        self.setup.error = None;
        self.poll_setup_download(link.clone(), model_id.clone(), cx);
        cx.notify();
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        cx.spawn(async move |this, cx| {
            let (download_link, id) = (link.clone(), model_id.clone());
            let downloaded = tokio.spawn(async move { env::download_speech_model(&download_link, &id).await }).await.ok().and_then(Result::ok);
            let stale = |this: &WeakEntity<Self>, cx: &mut AsyncApp| this.read_with(cx, |this, _| this.start_request != request).unwrap_or(true);
            let fail = |this: &WeakEntity<Self>, cx: &mut AsyncApp, error: SharedString| {
                _ = this.update(cx, |this, cx| {
                    this.setup.downloading = false;
                    this._setup_poll = None;
                    this.setup.error = Some(error);
                    cx.notify();
                });
            };
            let Some(downloaded) = downloaded else {
                if !stale(&this, cx) {
                    fail(&this, cx, t!("Could not download the model. Check this environment's connection and try again."));
                }
                return;
            };
            if stale(&this, cx) {
                return;
            }
            let outcome = env::speech_model_download_outcome(&downloaded, &model_id);
            if outcome == env::SpeechModelDownloadOutcome::NeedsSelection {
                let (select_link, id) = (link.clone(), model_id.clone());
                let selected = tokio.spawn(async move { env::select_speech_model(&select_link, &id).await }).await.ok().and_then(Result::ok);
                if stale(&this, cx) {
                    return;
                }
                if selected.is_none() {
                    fail(&this, cx, t!("The model downloaded, but this environment could not select it. Try again."));
                    return;
                }
            }
            let status_link = link.clone();
            let next = tokio.spawn(async move { env::speech_status(&status_link).await }).await.ok().and_then(Result::ok);
            if stale(&this, cx) {
                return;
            }
            let Some(next) = next else {
                fail(&this, cx, t!("Could not download the model. Check this environment's connection and try again."));
                return;
            };
            _ = this.update(cx, |this, cx| {
                let ready = outcome != env::SpeechModelDownloadOutcome::Incomplete
                    && matches!(&next, EnvironmentSpeechStatus::Supported(status)
                        if status.model_id.as_str() == model_id && status.state != EnvironmentSpeechState::MissingModel);
                this.status = Some((key, Some(next)));
                this.setup.downloading = false;
                this._setup_poll = None;
                if ready {
                    this.setup.step = 1;
                    this.setup.error = None;
                } else {
                    this.setup.error = Some(t!("The model is not ready. Try downloading it again."));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Refreshes the offered model's progress once a second while the dialog owns a download.
    fn poll_setup_download(&mut self, link: EnvironmentLink, model_id: String, cx: &mut Context<Self>) {
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        self._setup_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let poll_link = link.clone();
                let models = tokio.spawn(async move { env::speech_models(&poll_link, false).await }).await.ok().and_then(Result::ok);
                let keep = this.update(cx, |this, cx| {
                    if !(this.setup.open && this.setup.downloading) {
                        return false;
                    }
                    if let Some(model) = models.and_then(|models| models.models.into_iter().find(|model| model.id.as_str() == model_id)) {
                        let recommended = this.setup.model.as_ref().is_some_and(|current| current.recommended);
                        this.setup.model = Some(t3code_contracts::EnvironmentSpeechModel { recommended, ..model });
                        cx.notify();
                    }
                    true
                });
                if !matches!(keep, Ok(true)) {
                    return;
                }
            }
        }));
    }

    /// `cancelSetupDownload`: closes the dialog and asks the environment to stop.
    pub fn cancel_setup_download(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(model)) = (self.setup_key.clone(), self.setup.model.as_ref()) else { return };
        let Some(link) = Self::link(&key, cx) else { return };
        let model_id = model.id.as_str().to_owned();
        self.start_request += 1;
        self.setup.open = false;
        self.setup.downloading = false;
        self._setup_poll = None;
        cx.notify();
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        cx.spawn(async move |this, cx| {
            let result = tokio.spawn(async move { env::cancel_speech_model_download(&link, &model_id).await }).await;
            let error = match result {
                Ok(Ok(_)) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(error) => Some(error.to_string()),
            };
            if let Some(error) = error {
                _ = this.update(cx, |this, cx| {
                    this.setup.error = Some(error.into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// `startRecording`: closes the dialog and starts the recording it was opened for.
    pub fn start_setup_recording(&mut self, cx: &mut Context<Self>) {
        self.setup.open = false;
        if let Some(target) = self.setup_target.clone() {
            self.start(target, cx);
        }
        cx.notify();
    }

    fn begin(&mut self, key: SessionKey, target: VoiceTarget, cx: &mut Context<Self>) {
        let Some((controller, _)) = self.session_for(&key, cx) else { return };
        self.target = Some(target);
        self.activity = VoiceActivity::default();
        cx.notify();
        let start = controller.start();
        cx.spawn(async move |_, _| start.await).detach();
    }

    /// `stop`: finishes `owner`'s recording.
    pub fn stop(&mut self, owner: &str, cx: &mut Context<Self>) {
        if !self.owns(owner) {
            return;
        }
        if let Some(session) = &self.session {
            let stop = session.controller.stop();
            cx.spawn(async move |_, _| stop.await).detach();
        }
    }

    /// A held shortcut came up: finish now, or as soon as the recording starts.
    pub fn stop_when_recording(&mut self, owner: &str, cx: &mut Context<Self>) {
        if !self.owns(owner) && self.queued_start.as_deref() != Some(owner) {
            return;
        }
        if self.state.phase == VoiceInputPhase::Recording && self.owns(owner) {
            self.stop(owner, cx);
        } else {
            self.stop_on_record = Some(SharedString::from(owner.to_owned()));
        }
    }

    /// `cancel`: abandons `owner`'s dictation and any send waiting on it.
    pub fn cancel(&mut self, owner: &str, cx: &mut Context<Self>) {
        self.pending_submission = None;
        self.stop_on_record = None;
        self.start_request += 1;
        if self.setup_target.as_ref().is_some_and(|target| target.owner_key.as_ref() == owner) {
            self.reset_setup();
            cx.notify();
        }
        if self.queued_start.as_deref() == Some(owner) {
            self.queued_start = None;
            cx.notify();
        }
        if !self.owns(owner) {
            return;
        }
        if let Some(session) = &self.session {
            let cancel = session.controller.cancel();
            cx.spawn(async move |_, _| cancel.await).detach();
        }
    }

    /// `skipPostProcessing`.
    pub fn skip_post_processing(&mut self, owner: &str) {
        if self.owns(owner)
            && let Some(session) = &self.session
        {
            session.controller.skip_post_processing();
        }
    }

    /// `ownerChanged`: the editor that showed `owner` shows another draft. A thread draft
    /// keeps recording (its transcript lands in the stored draft); this app's targets all
    /// persist, so only a disabled owner cancels.
    pub fn owner_disabled(&mut self, owner: &str, cx: &mut Context<Self>) {
        self.cancel(owner, cx);
    }

    /// Re-binds a running recording to a remounted editor for the same draft (`adopt`).
    pub fn adopt(&mut self, target: VoiceTarget) {
        if self.state.blocks_submission() && self.target.as_ref().is_some_and(|current| current.owner_key == target.owner_key) {
            self.target = Some(target);
        }
    }

    fn owns(&self, owner: &str) -> bool {
        self.target.as_ref().is_some_and(|target| target.owner_key.as_ref() == owner)
    }

    /// `submitAfterDictation`: sends now when no dictation blocks, else finishes the recording
    /// and sends once its transcript is in the draft.
    pub fn submit_after_dictation(&mut self, owner: SharedString, submit: Box<dyn FnOnce(&mut App)>, cx: &mut Context<Self>) {
        let state = self.state_for(&owner);
        if state.phase == VoiceInputPhase::Preparing {
            return;
        }
        if !state.blocks_submission() {
            cx.defer(submit);
            return;
        }
        if self.pending_submission.is_some() {
            return;
        }
        self.pending_submission = Some(PendingSubmission { owner_key: owner.clone(), committed: false, submit });
        if state.phase == VoiceInputPhase::Recording {
            self.stop(&owner, cx);
        }
    }

    fn set_state(&mut self, state: VoiceInputState, cx: &mut Context<Self>) {
        if state.phase == VoiceInputPhase::Recording {
            self.recording_started_at.get_or_insert_with(Instant::now);
            if self._clock.is_none() {
                // The elapsed label changes once a second; tick four times a second as the fork does.
                self._clock = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_millis(250)).await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                }));
            }
        } else {
            self.recording_started_at = None;
            self._clock = None;
        }
        if !matches!(state.phase, VoiceInputPhase::Recording | VoiceInputPhase::Transcribing) {
            self.activity = VoiceActivity::default();
        }
        if state.phase == VoiceInputPhase::Error {
            self.pending_submission = None;
        }
        let idle = state.phase == VoiceInputPhase::Idle;
        let recording = state.phase == VoiceInputPhase::Recording;
        if !recording && state.phase != VoiceInputPhase::Preparing {
            self.stop_on_record = None;
        }
        self.state = state;
        cx.notify();
        if recording
            && let Some(owner) = self.stop_on_record.take()
        {
            self.stop(&owner, cx);
        }
        if idle
            && let Some(pending) = self.pending_submission.take()
        {
            let owner_matches = self.target.as_ref().is_some_and(|target| target.owner_key == pending.owner_key);
            if owner_matches && pending.committed {
                cx.defer(pending.submit);
            }
        }
    }

    fn publish_activity(&mut self, change: impl FnOnce(&mut VoiceActivity), cx: &mut Context<Self>) {
        let before = self.activity.clone();
        change(&mut self.activity);
        if self.activity != before {
            cx.notify();
        }
    }

    fn read_draft(&self, cx: &App) -> Option<VoiceDraftSnapshot> {
        let target = self.target.as_ref()?;
        let (text, selection) = (target.read_draft)(cx)?;
        Some(VoiceDraftSnapshot { owner_key: target.owner_key.to_string(), text, selection })
    }

    fn commit_draft(&mut self, text: String, selection: Range<usize>, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else { return };
        (target.commit_draft)(text, selection, cx);
        if let Some(pending) = self.pending_submission.as_mut().filter(|pending| pending.owner_key == target.owner_key) {
            pending.committed = true;
        }
    }

    /// The options every recording carries, from the settings and the draft's project.
    fn transcription_options(&self, cx: &App) -> Option<(SpeechTranscriptionOptions, Option<SpeechPostProcessingOptions>, Option<EnvironmentId>)> {
        let context = &self.target.as_ref()?.context;
        let settings = ClientSettingsStore::get(cx);
        let cleanup_grant = context.cleanup.then(|| voice_consent::cleanup_grant(cx)).flatten();
        let cleanup_environment = cleanup_grant.as_ref().and(context.environment_id.clone());
        let cue = if cleanup_grant.is_some() { settings.speech_correction_word.as_str().to_owned() } else { String::new() };
        let words = resolve_speech_vocabulary(
            &settings.speech_custom_words,
            &context.project_words,
            &[cue.as_str(), context.project_name.as_deref().unwrap_or("")],
        );
        let cue = (!cue.trim().is_empty()).then_some(cue);
        let cleanup = cleanup_grant.is_some().then(|| SpeechPostProcessingOptions {
            speech_custom_words: words.clone(),
            speech_post_processing_prompt: settings.speech_post_processing_prompt.clone(),
            speech_correction_word: cue.clone().map(SpeechCorrectionCue::new),
        });
        let options = SpeechTranscriptionOptions {
            speech_language: settings.speech_language.as_str().to_owned(),
            speech_translate_to_english: settings.speech_translate_to_english,
            speech_custom_words: words,
            speech_remove_filler_words: settings.speech_remove_filler_words,
            speech_correction_word: cue,
            speech_silence_filtering: settings.speech_silence_filtering,
        };
        Some((options, cleanup, cleanup_environment))
    }
}

/// `preferredSpeechLanguages`: the system's preferred languages as primary subtags, most
/// preferred first.
pub fn preferred_speech_languages() -> Vec<String> {
    crate::i18n::runtime::system_languages()
        .into_iter()
        .filter_map(|tag| tag.to_lowercase().split(['-', '_']).next().map(str::to_owned))
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// The Settings → Voice page (the error capsule's Settings button).
pub fn open_voice_settings(window: &mut Window, cx: &mut App) {
    navigate(crate::router::Route::settings(crate::components::settings::settings_search::SettingsPath::Voice), window, cx);
}

/// Navigates through the window root's router.
pub fn navigate(route: crate::router::Route, window: &mut Window, cx: &mut App) {
    if let Some(navigate) = cx.try_global::<VoiceNavigator>().map(|navigator| navigator.0.clone()) {
        navigate(route, window, cx);
    }
}

/// How voice UI outside a route navigates (the indicator's draft button, Settings → Voice).
/// The window root sets it.
#[derive(Clone)]
pub struct VoiceNavigator(pub Rc<dyn Fn(crate::router::Route, &mut Window, &mut App)>);
impl Global for VoiceNavigator {}

impl VoiceNavigator {
    /// Routes voice navigation through `router`.
    pub fn set(router: WeakEntity<crate::router::Router>, cx: &mut App) {
        cx.set_global(AppRouter(router.clone()));
        cx.set_global(Self(Rc::new(move |route, _, cx| {
            _ = router.update(cx, |router, cx| router.navigate(route, cx));
        })));
    }
}

/// The window's router, for actions that run without a window (a banner button's task).
struct AppRouter(WeakEntity<crate::router::Router>);
impl Global for AppRouter {}

/// Replaces the current route, as `navigate({ to, replace: true })` does.
pub fn replace_route(route: crate::router::Route, cx: &mut App) {
    if let Some(router) = cx.try_global::<AppRouter>().map(|r| r.0.clone()) {
        _ = router.update(cx, |router, cx| router.replace(route, cx));
    }
}

fn toast(kind: ToastKind, title: SharedString, description: Option<SharedString>, cx: &mut App) {
    let mut data = ToastData::new(kind, title);
    if let Some(description) = description {
        data = data.description(description);
    }
    Toaster::push(data, cx);
}

/// Translates controller copy.
pub fn voice_text(text: &VoiceText) -> SharedString {
    match text {
        VoiceText::Raw(raw) => known_stream_message(raw).unwrap_or_else(|| raw.clone().into()),
        VoiceText::Known(message) => match message {
            VoiceMessage::DraftUnavailable => t!("This draft is no longer available."),
            VoiceMessage::AnotherRecordingActive => t!("Another voice recording is already active."),
            VoiceMessage::TranscriptionUnavailable => t!("Voice transcription is not available."),
            VoiceMessage::MicrophoneRequired => t!("Microphone access is required for voice input."),
            VoiceMessage::MicrophonePermission => t!(
                "Microphone access is required for voice input. Allow access in your browser or system settings and try again."
            ),
            VoiceMessage::StartFailed => t!("Could not start voice recording."),
            VoiceMessage::StillFinishing => t!("Voice transcription is still finishing. Try again shortly."),
            VoiceMessage::LanguageUnsupported => t!("Voice transcription is not available for this language."),
            VoiceMessage::PrepareFailed => t!("Could not prepare voice transcription."),
            VoiceMessage::TranscribeFailed => t!("Could not transcribe this recording."),
            VoiceMessage::Interrupted => t!("Voice recording was interrupted."),
            VoiceMessage::FinishFailed => t!("Could not finish voice recording."),
            VoiceMessage::DraftChanged => t!("The draft changed while voice input was running. The transcript was not added."),
            VoiceMessage::NoSpeech => t!("No speech was detected."),
            VoiceMessage::Cancelled => t!("Voice transcription was cancelled."),
            VoiceMessage::MicrophoneProcessingFailed => t!("Microphone processing failed."),
            VoiceMessage::MicrophoneDisconnected => t!("The microphone disconnected."),
            VoiceMessage::CaptureDidNotStop => t!("Microphone capture did not stop."),
            VoiceMessage::DestinationFailed => t!("Voice transcription failed. Check the destination in Settings → Voice."),
            VoiceMessage::ChooseOpenRouterModel => t!("Choose an OpenRouter model in Settings → Voice."),
        },
    }
}

/// The live stream's English errors, translated (`stream.ts` `translatableError`).
fn known_stream_message(raw: &str) -> Option<SharedString> {
    Some(match raw {
        "Live transcription stopped responding." => t!("Live transcription stopped responding."),
        "Voice transcription was cancelled." => t!("Voice transcription was cancelled."),
        "Voice consent was revoked." => t!("Voice consent was revoked."),
        "Unexpected speech stream response." => t!("Unexpected speech stream response."),
        "Invalid speech stream response." => t!("Invalid speech stream response."),
        "The voice connection closed. Please record again." => t!("The voice connection closed. Please record again."),
        "Could not connect to live transcription." => t!("Could not connect to live transcription."),
        "Voice recordings are limited to five minutes." => t!("Voice recordings are limited to five minutes."),
        "Could not send microphone audio." => t!("Could not send microphone audio."),
        "Could not finish live transcription." => t!("Could not finish live transcription."),
        _ => return None,
    })
}

/// Runs `future` on tokio; `None` when cancelled first (the task is aborted).
async fn on_tokio<T: Send + 'static>(
    tokio: &tokio::runtime::Handle,
    cancel: &CancelToken,
    future: impl std::future::Future<Output = T> + Send + 'static,
) -> Option<T> {
    let task = tokio.spawn(future);
    let abort = task.abort_handle();
    let cancelled = cancel.cancelled();
    futures::pin_mut!(cancelled);
    match futures::future::select(task, cancelled).await {
        futures::future::Either::Left((result, _)) => result.ok(),
        futures::future::Either::Right(_) => {
            abort.abort();
            None
        }
    }
}

fn request_failure(error: env::VoiceRequestError) -> VoiceTranscriptionError {
    match error {
        env::VoiceRequestError::Cloud(cloud) => VoiceTranscriptionError::new(
            VoiceTranscriptionErrorCode::TranscriptionFailed,
            VoiceText::Raw(env::CloudSpeechErrorMessage::message(&cloud)),
        ),
        _ => VoiceTranscriptionError::new(VoiceTranscriptionErrorCode::TranscriptionFailed, VoiceMessage::DestinationFailed),
    }
}

fn make_controller(platform: Rc<Platform>, owner: WeakEntity<VoiceInput>, cx: AsyncApp) -> Rc<VoiceInputController> {
    let (read_owner, commit_owner, state_owner, post_owner) = (owner.clone(), owner.clone(), owner.clone(), owner);
    let (read_cx, commit_cx, state_cx, post_cx, error_cx) = (cx.clone(), cx.clone(), cx.clone(), cx.clone(), cx);
    let post_platform = platform.clone();
    let release_platform = platform.clone();
    VoiceInputController::new(VoiceInputControllerDependencies {
        recorder: Rc::new(PlatformHandle(platform.clone())),
        get_transcriber: Box::new(move || Some(Rc::new(PlatformHandle(platform.clone())) as Rc<dyn VoiceTranscriber>)),
        request_permission: Box::new(|| async { (true, true) }.boxed_local()),
        release_recording: Box::new(move || release_platform.release()),
        retry_failed_recording: true,
        post_process: Some(Box::new(move |transcript, draft, cancel| {
            post_process(&post_platform, &post_owner, &post_cx, transcript, draft, cancel)
        })),
        on_post_processing_error: Box::new(move || {
            error_cx.update(|cx| {
                toast(ToastKind::Warning, t!("Transcript cleanup failed"), Some(t!("The original transcript was added.")), cx)
            })
        }),
        read_draft: Box::new(move || read_cx.update(|cx| read_owner.upgrade().and_then(|entity| entity.read(cx).read_draft(cx)))),
        commit_draft: Box::new(move |text, selection| {
            commit_cx.update(|cx| _ = commit_owner.update(cx, |this, cx| this.commit_draft(text, selection, cx)));
        }),
        on_state_change: Box::new(move |state| {
            let state = state.clone();
            state_cx.update(|cx| _ = state_owner.update(cx, |this, cx| this.set_state(state, cx)));
        }),
    })
}

/// Cleanup runs on the draft's own environment, with its own grant, never on audio.
fn post_process(
    platform: &Platform,
    owner: &WeakEntity<VoiceInput>,
    cx: &AsyncApp,
    transcript: &str,
    draft: &VoiceDraftSnapshot,
    cancel: CancelToken,
) -> Option<LocalBoxFuture<'static, Result<String, String>>> {
    let preferences = platform.recording.borrow().clone()?;
    let cleanup = preferences.cleanup?;
    let still_granted = cx.update(|cx| {
        voice_consent::grant_still_holds(&platform.key.grant_id, cx)
            && voice_consent::cleanup_grant(cx) == preferences.cleanup_grant
            && owner.upgrade().is_some_and(|entity| entity.read(cx).owns(&draft.owner_key))
    });
    if !still_granted {
        return None;
    }
    let link = preferences.cleanup_environment.as_ref().and_then(|id| {
        cx.update(|cx| ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::global(cx).environment_link(id)).flatten())
    });
    let Some(link) = link else {
        return Some(async { Err("The draft's environment is unavailable for transcript cleanup.".to_owned()) }.boxed_local());
    };
    let (text, selection) = cleanup_context(&draft.text, &draft.selection);
    let request = EnvironmentSpeechPostProcessingRequest {
        options: cleanup,
        transcript: transcript.to_owned(),
        draft: EnvironmentSpeechPostProcessingRequestDraft {
            text,
            selection: EnvironmentSpeechPostProcessingRequestDraftSelection {
                start: SidebarAutoSettleAfterDays::Number(selection.start as f64),
                end: SidebarAutoSettleAfterDays::Number(selection.end as f64),
            },
        },
    };
    let tokio = cx.update(|cx| ConnectionRuntime::global(cx).handle().clone());
    Some(
        async move {
            match on_tokio(&tokio, &cancel, async move { env::post_process_transcript(&link, &request).await }).await {
                Some(Ok(text)) => Ok(text),
                Some(Err(error)) => Err(error.to_string()),
                None => Err("cancelled".into()),
            }
        }
        .boxed_local(),
    )
}

/// `cleanupContext`: up to 1,024 characters either side of the selection. Offsets in the
/// result count UTF-16 units, as the server reads them.
fn cleanup_context(text: &str, selection: &Range<usize>) -> (String, Range<usize>) {
    const LIMIT: usize = 1_024;
    let start = selection.start.min(text.len());
    let end = selection.end.clamp(start, text.len());
    let before_chars: Vec<char> = text[..start].chars().collect();
    let before: String = before_chars[before_chars.len().saturating_sub(LIMIT)..].iter().collect();
    let selected: String = text[start..end].chars().take(LIMIT).collect();
    let after: String = text[end..].chars().take(LIMIT).collect();
    let utf16 = |s: &str| s.encode_utf16().count();
    let range = utf16(&before)..utf16(&before) + utf16(&selected);
    (format!("{before}{selected}{after}"), range)
}

/// What a recording chose when it started.
#[derive(Clone)]
struct RecordingPreferences {
    cleanup: Option<SpeechPostProcessingOptions>,
    cleanup_grant: Option<String>,
    cleanup_environment: Option<EnvironmentId>,
}

#[derive(Default)]
struct CaptureState {
    capture: Option<PcmCapture>,
    /// Audio kept for a finished-recording upload.
    chunks: Vec<f32>,
    finished: Option<Recording>,
    /// Audio goes here while a live stream runs.
    stream: Option<Rc<SpeechStream>>,
    stopped: Option<futures::channel::oneshot::Sender<()>>,
    events: Option<Task<()>>,
    limit: Option<Task<()>>,
}

/// Capture and transcription for one destination (`createBrowserVoiceInputPlatform`).
struct Platform {
    key: SessionKey,
    grant_valid: Arc<AtomicBool>,
    owner: WeakEntity<VoiceInput>,
    cx: AsyncApp,
    controller: RefCell<std::rc::Weak<VoiceInputController>>,
    capture: RefCell<CaptureState>,
    recording: RefCell<Option<RecordingPreferences>>,
}

impl Platform {
    fn new(key: SessionKey, grant_valid: Arc<AtomicBool>, owner: WeakEntity<VoiceInput>, cx: AsyncApp) -> Self {
        Self {
            key,
            grant_valid,
            owner,
            cx,
            controller: RefCell::new(std::rc::Weak::new()),
            capture: RefCell::default(),
            recording: RefCell::new(None),
        }
    }

    fn can_send(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let valid = self.grant_valid.clone();
        Arc::new(move || valid.load(Ordering::SeqCst))
    }

    /// `cleanupCapture`.
    fn release(&self) {
        let mut state = self.capture.borrow_mut();
        state.capture = None;
        state.events = None;
        state.limit = None;
        state.chunks.clear();
        state.stream = None;
        if let Some(stopped) = state.stopped.take() {
            let _ = stopped.send(());
        }
        drop(state);
        self.with_owner(|this, cx| this.publish_activity(|activity| activity.level = 0., cx));
    }

    fn with_owner(&self, update: impl FnOnce(&mut VoiceInput, &mut Context<VoiceInput>)) {
        let owner = self.owner.clone();
        self.cx.update(|cx| _ = owner.update(cx, update));
    }

    fn controller(&self) -> Option<Rc<VoiceInputController>> {
        self.controller.borrow().upgrade()
    }

    fn on_capture_event(self: &Rc<Self>, event: CaptureEvent) {
        match event {
            CaptureEvent::Level(level) => self.with_owner(|this, cx| {
                this.publish_activity(
                    |activity| {
                        activity.level = level;
                        activity.levels.copy_within(1.., 0);
                        activity.levels[WAVEFORM_BARS - 1] = level;
                    },
                    cx,
                )
            }),
            CaptureEvent::Pcm(pcm) => {
                let mut state = self.capture.borrow_mut();
                match &state.stream {
                    Some(stream) => stream.feed(&pcm),
                    None => state.chunks.extend_from_slice(&pcm),
                }
            }
            CaptureEvent::Stopped => {
                if let Some(stopped) = self.capture.borrow_mut().stopped.take() {
                    let _ = stopped.send(());
                }
            }
            CaptureEvent::Limit => self.spawn_controller(|controller| controller.stop()),
            CaptureEvent::Recorded { .. } => {}
            CaptureEvent::Failed(failure) => {
                let (message, code) = match failure {
                    CaptureFailure::Processing => (VoiceMessage::MicrophoneProcessingFailed, VoiceInputErrorCode::MicrophoneFailed),
                    CaptureFailure::Disconnected => (VoiceMessage::MicrophoneDisconnected, VoiceInputErrorCode::MicrophoneDisconnected),
                };
                self.spawn_controller(move |controller| controller.interrupt_recording(Some(message.into()), Some(code)));
            }
        }
    }

    fn spawn_controller(&self, run: impl FnOnce(&Rc<VoiceInputController>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> + 'static) {
        if let Some(controller) = self.controller() {
            let future = run(&controller);
            self.cx.spawn(async move |_| future.await).detach();
        }
    }
}

/// The platform as the controller's capture and transcriber.
#[derive(Clone)]
struct PlatformHandle(Rc<Platform>);

impl std::ops::Deref for PlatformHandle {
    type Target = Rc<Platform>;
    fn deref(&self) -> &Rc<Platform> {
        &self.0
    }
}

impl VoiceRecorder for PlatformHandle {
    fn recording(&self) -> Option<Recording> {
        self.capture.borrow().finished.clone()
    }

    fn prepare(&self, cancel: CancelToken) -> LocalBoxFuture<'static, Result<(), RecorderError>> {
        let this = self.clone();
        async move {
            this.capture.borrow_mut().finished = None;
            this.capture.borrow_mut().chunks.clear();
            // Remember newly seen microphones, then record from the first connected one.
            let background = this.cx.background_executor().clone();
            let devices = background.spawn(async { microphones::list_audio_inputs() }).await;
            let device = this.cx.update(|cx| {
                let order = microphones::current_order(cx);
                let order = match microphones::remember_microphones(&order, &devices) {
                    Some(next) => {
                        microphones::save_order(next.clone(), cx);
                        next
                    }
                    None => order,
                };
                microphones::preferred_device(&order, &devices)
            });
            if cancel.is_cancelled() {
                return Err(RecorderError::Failed("cancelled".into()));
            }
            let opened = background.spawn(async move { PcmCapture::open(CaptureOptions { device, meter_only: false }) }).await;
            let (capture, mut events) = opened.map_err(|error| match error {
                OpenError::PermissionDenied => RecorderError::PermissionDenied,
                OpenError::NotFound => RecorderError::Failed("No microphone found.".into()),
                OpenError::Failed(message) => RecorderError::Failed(message),
            })?;
            if cancel.is_cancelled() {
                return Err(RecorderError::Failed("cancelled".into()));
            }
            let pump = this.clone();
            let task = this.cx.spawn(async move |_| {
                use futures::StreamExt as _;
                while let Some(event) = events.next().await {
                    pump.on_capture_event(event);
                }
            });
            let mut state = this.capture.borrow_mut();
            state.capture = Some(capture);
            state.events = Some(task);
            Ok(())
        }
        .boxed_local()
    }

    fn record(&self, limit_seconds: u64) {
        let mut state = self.capture.borrow_mut();
        if let Some(capture) = &state.capture {
            capture.start();
        }
        let this = self.clone();
        state.limit = Some(self.cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(limit_seconds)).await;
            this.spawn_controller(|controller| controller.stop());
        }));
    }

    fn stop(&self) -> LocalBoxFuture<'static, Result<(), String>> {
        let this = self.clone();
        async move {
            let receiver = {
                let mut state = this.capture.borrow_mut();
                let Some(capture) = &state.capture else { return Ok(()) };
                capture.stop();
                let (sender, receiver) = futures::channel::oneshot::channel();
                state.stopped = Some(sender);
                receiver
            };
            let timeout = this.cx.background_executor().timer(STOP_TIMEOUT);
            let stopped = futures::future::select(receiver, timeout).await;
            let result = match stopped {
                futures::future::Either::Left(_) => {
                    let mut state = this.capture.borrow_mut();
                    if state.stream.is_none() && !state.chunks.is_empty() {
                        state.finished = Some(Arc::from(pcm_bytes(&state.chunks)));
                    }
                    Ok(())
                }
                futures::future::Either::Right(_) => Err("Microphone capture did not stop.".to_owned()),
            };
            this.release();
            result
        }
        .boxed_local()
    }
}

impl VoiceTranscriber for PlatformHandle {
    fn prepare(&self, cancel: CancelToken) -> LocalBoxFuture<'static, Result<PreparedVoiceTranscription, VoiceTranscriptionError>> {
        let this = self.clone();
        async move {
            let unavailable = || VoiceTranscriptionError::new(VoiceTranscriptionErrorCode::Unavailable, VoiceMessage::TranscriptionUnavailable);
            let (options, cleanup, cleanup_environment, link, tokio, streaming_enabled, cleanup_grant) = this.cx.update(|cx| {
                let owner = this.owner.upgrade()?;
                let (options, cleanup, cleanup_environment) = owner.read(cx).transcription_options(cx)?;
                let link = VoiceInput::link(&this.key, cx)?;
                let tokio = ConnectionRuntime::global(cx).handle().clone();
                Some((options, cleanup, cleanup_environment, link, tokio, ClientSettingsStore::get(cx).speech_streaming, voice_consent::cleanup_grant(cx)))
            })
            .ok_or_else(unavailable)?;
            *this.recording.borrow_mut() = Some(RecordingPreferences { cleanup, cleanup_grant, cleanup_environment });
            let transcribe = batch_transcriber(this.key.destination.clone(), link.clone(), options.clone(), tokio.clone(), this.can_send());
            if let VoiceTranscriptionDestination::Cloud { provider, model, .. } = &this.key.destination {
                if *provider == CloudSpeechProvider::Openrouter && model.is_none() {
                    return Err(VoiceTranscriptionError::new(VoiceTranscriptionErrorCode::Unavailable, VoiceMessage::ChooseOpenRouterModel));
                }
                let provider = *provider;
                let check_link = link.clone();
                match on_tokio(&tokio, &cancel, async move { env::require_cloud_speech_key(&check_link, provider).await }).await {
                    None => return Err(VoiceTranscriptionError::cancelled()),
                    Some(Err(env::VoiceRequestError::Cloud(cloud))) => {
                        return Err(VoiceTranscriptionError::new(
                            VoiceTranscriptionErrorCode::Unavailable,
                            VoiceText::Raw(env::CloudSpeechErrorMessage::message(&cloud)),
                        ));
                    }
                    Some(Err(_)) => return Err(unavailable()),
                    Some(Ok(())) => {}
                }
                let languages: Vec<String> = CLOUD_SPEECH_LANGUAGES.iter().map(|language| (*language).to_owned()).collect();
                let locale = effective_speech_language(&languages, true, &options.speech_language);
                return Ok(PreparedVoiceTranscription { locale, transcribe, streaming: None });
            }
            let status_link = link.clone();
            let status = match on_tokio(&tokio, &cancel, async move { env::speech_status(&status_link).await }).await {
                None => return Err(VoiceTranscriptionError::cancelled()),
                Some(Err(_)) => return Err(VoiceTranscriptionError::new(VoiceTranscriptionErrorCode::PreparationFailed, VoiceMessage::PrepareFailed)),
                Some(Ok(status)) => status,
            };
            let status = match status {
                EnvironmentSpeechStatus::Supported(status) => status,
                EnvironmentSpeechStatus::NotSupported(status) => {
                    return Err(VoiceTranscriptionError::new(VoiceTranscriptionErrorCode::Unavailable, VoiceText::Raw(status.reason)));
                }
                EnvironmentSpeechStatus::Unknown(_) => return Err(unavailable()),
            };
            let language = match &status.languages {
                Some(languages) => effective_speech_language(languages, status.supports_language_detection.unwrap_or(false), &options.speech_language),
                None => "auto".to_owned(),
            };
            let native_streaming = status.supports_streaming && streaming_enabled;
            let locale =
                if !native_streaming && status.supports_translation && options.speech_translate_to_english { "en".to_owned() } else { language };
            let streams = if status.supports_streaming { native_streaming } else { status.supports_buffered_streaming == Some(true) };
            if !streams {
                return Ok(PreparedVoiceTranscription { locale, transcribe, streaming: None });
            }
            let url_link = link.clone();
            let url = match on_tokio(&tokio, &cancel, async move { env::speech_stream_url(&url_link).await }).await {
                None => return Err(VoiceTranscriptionError::cancelled()),
                Some(Err(_)) => return Err(VoiceTranscriptionError::new(VoiceTranscriptionErrorCode::PreparationFailed, VoiceMessage::PrepareFailed)),
                Some(Ok(url)) => url,
            };
            let (text_tx, mut text_rx) = futures::channel::mpsc::unbounded::<Result<SpeechStreamText, String>>();
            let error_tx = text_tx.clone();
            let stream = Rc::new(SpeechStream::open(
                &tokio,
                url,
                options,
                this.can_send(),
                move |text| _ = text_tx.unbounded_send(Ok(text)),
                move |error| _ = error_tx.unbounded_send(Err(error)),
            ));
            // Live text and failures come back on the UI thread.
            let relay = this.clone();
            this.cx
                .spawn(async move |_| {
                    use futures::StreamExt as _;
                    while let Some(update) = text_rx.next().await {
                        match update {
                            Ok(text) => relay.with_owner(|this, cx| this.publish_activity(|activity| activity.preview = Some(text), cx)),
                            Err(error) => relay.spawn_controller(move |controller| controller.interrupt_recording(Some(VoiceText::Raw(error)), None)),
                        }
                    }
                })
                .detach();
            this.capture.borrow_mut().stream = Some(stream.clone());
            let finish: t3code_client_runtime::voice_input::controller::FinishStreamFn = Rc::new(move |cancel: CancelToken| {
                let stream = stream.clone();
                async move {
                    let finished = stream.finish();
                    let cancelled = cancel.cancelled();
                    futures::pin_mut!(finished, cancelled);
                    match futures::future::select(finished, cancelled).await {
                        futures::future::Either::Left((Ok(text), _)) => Ok(text),
                        futures::future::Either::Left((Err(error), _)) => {
                            Err(VoiceTranscriptionError::new(VoiceTranscriptionErrorCode::TranscriptionFailed, VoiceText::Raw(error)))
                        }
                        futures::future::Either::Right(_) => {
                            stream.cancel();
                            Err(VoiceTranscriptionError::cancelled())
                        }
                    }
                }
                .boxed_local()
            });
            Ok(PreparedVoiceTranscription { locale, transcribe, streaming: Some(finish) })
        }
        .boxed_local()
    }
}

/// The finished-recording upload for the destination (`transcribeRecording`).
fn batch_transcriber(
    destination: VoiceTranscriptionDestination,
    link: EnvironmentLink,
    options: SpeechTranscriptionOptions,
    tokio: tokio::runtime::Handle,
    can_send: Arc<dyn Fn() -> bool + Send + Sync>,
) -> t3code_client_runtime::voice_input::controller::TranscribeFn {
    Rc::new(move |recording: Recording, cancel: CancelToken| {
        let (destination, link, options, tokio, can_send) = (destination.clone(), link.clone(), options.clone(), tokio.clone(), can_send.clone());
        async move {
            if !can_send() {
                return Err(VoiceTranscriptionError::cancelled());
            }
            let upload = async move {
                match &destination {
                    VoiceTranscriptionDestination::Cloud { provider, model, .. } => {
                        env::transcribe_cloud_pcm(&link, *provider, model.as_deref(), &recording, &options).await
                    }
                    VoiceTranscriptionDestination::Environment { .. } => env::transcribe_pcm(&link, &recording, &options).await,
                }
            };
            match on_tokio(&tokio, &cancel, upload).await {
                None => Err(VoiceTranscriptionError::cancelled()),
                Some(Ok(text)) => Ok(text),
                Some(Err(error)) => Err(request_failure(error)),
            }
        }
        .boxed_local()
    })
}
