//! `CloudVoiceConfiguration.tsx`: the dialog that turns on a cloud transcription destination.
//! It reads whether the environment holds the provider's key and what this session may do there,
//! then offers the key field (OpenAI, xAI) or the model search and key (OpenRouter), saves the key
//! to the environment, and makes the cloud grant this device's destination once the save lands.
//! Secret-manager references (`SecretSourceSelect`) are not offered: the fork shows them only
//! behind its secrets feature flag.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use t3code_client_runtime::voice_input::environment as speech;
use t3code_client_runtime::voice_input::speech::VoiceTranscriptionDestination;
use t3code_contracts::{AuthEnvironmentScope, AuthSessionState, CloudSpeechKeys, CloudSpeechModel, CloudSpeechProvider, EnvironmentId};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::DialogSurface;
use crate::components::ui::input::TextInput;
use crate::connection::ConnectionRuntime;
use crate::i18n::t;
use crate::speech::voice_consent;

/// What the dialog asks its owner to do.
pub enum CloudVoiceEvent {
    Close,
}

/// A read from the environment: in flight, failed, or its value.
enum Read<T> {
    Loading,
    Failed,
    Done(T),
}

/// The dialog for one provider through one environment.
pub struct CloudVoiceConfiguration {
    environment_id: EnvironmentId,
    environment_label: SharedString,
    provider: CloudSpeechProvider,
    /// This device already dictates through this provider and environment.
    active: bool,
    /// The OpenRouter model the current grant names.
    current_model: Option<String>,
    session: Read<Option<AuthSessionState>>,
    keys: Read<CloudSpeechKeys>,
    models: Read<Vec<CloudSpeechModel>>,
    key: Entity<InputState>,
    model_query: Entity<InputState>,
    model: String,
    remove: bool,
    busy: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CloudVoiceEvent> for CloudVoiceConfiguration {}

/// `CloudSpeechApiKey`: a pasted key, not a URL or a reference.
fn is_api_key(text: &str) -> bool {
    !text.is_empty() && text.len() <= 4096 && !text.contains("://") && !text.chars().any(char::is_whitespace)
}

impl CloudVoiceConfiguration {
    /// Opens the dialog and starts its reads.
    pub fn new(environment_id: EnvironmentId, environment_label: SharedString, provider: CloudSpeechProvider, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let grant = voice_consent::audio_grant(cx).map(|grant| grant.destination);
        let (active, current_model) = match &grant {
            Some(VoiceTranscriptionDestination::Cloud { environment_id: id, provider: p, model }) if *id == environment_id && *p == provider => {
                (true, model.clone())
            }
            _ => (false, None),
        };
        let key = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Enter API key")).masked(true));
        let model_query = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search transcription models")));
        let subscriptions = vec![
            cx.subscribe(&key, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&model_query, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let mut this = Self {
            environment_id,
            environment_label,
            provider,
            active,
            model: current_model.clone().unwrap_or_default(),
            current_model,
            session: Read::Loading,
            keys: Read::Loading,
            models: Read::Loading,
            key,
            model_query,
            remove: false,
            busy: false,
            error: None,
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    fn routed(&self) -> bool {
        self.provider == CloudSpeechProvider::Openrouter
    }

    fn link(&self, cx: &App) -> Option<t3code_client_runtime::preview::environment_link::EnvironmentLink> {
        ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::global(cx).environment_link(&self.environment_id)).flatten()
    }

    /// Reads the session, the saved keys and (OpenRouter) the model catalog.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(link) = self.link(cx) else {
            self.session = Read::Done(None);
            return;
        };
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        let routed = self.routed();
        cx.spawn(async move |this, cx| {
            let l = link.clone();
            let session = tokio.spawn(async move { l.session_state().await }).await.ok().and_then(Result::ok);
            let can_use = session.as_ref().is_some_and(|s| speech::session_grants_scope(s, AuthEnvironmentScope::OrchestrationOperate));
            _ = this.update(cx, |this, cx| {
                this.session = Read::Done(session);
                cx.notify();
            });
            if !can_use {
                return;
            }
            let l = link.clone();
            let keys = tokio.spawn(async move { speech::cloud_speech_keys(&l).await }).await.ok().and_then(Result::ok);
            _ = this.update(cx, |this, cx| {
                this.keys = keys.map_or(Read::Failed, Read::Done);
                cx.notify();
            });
            if routed {
                let models = tokio.spawn(async move { speech::cloud_speech_models(&link).await }).await.ok().and_then(Result::ok);
                _ = this.update(cx, |this, cx| {
                    this.models = models.map_or(Read::Failed, |m| Read::Done(m.models));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn grants(&self, scope: AuthEnvironmentScope) -> bool {
        matches!(&self.session, Read::Done(Some(session)) if speech::session_grants_scope(session, scope))
    }

    /// `saved`: whether the environment holds this provider's key, once read.
    fn saved(&self) -> Option<bool> {
        match &self.keys {
            Read::Done(keys) => match self.provider {
                CloudSpeechProvider::Openai => Some(keys.openai),
                CloudSpeechProvider::Xai => Some(keys.xai),
                CloudSpeechProvider::Openrouter => keys.openrouter,
            },
            _ => None,
        }
    }

    fn destination(&self) -> VoiceTranscriptionDestination {
        VoiceTranscriptionDestination::Cloud {
            environment_id: self.environment_id.clone(),
            provider: self.provider,
            model: (self.routed() && !self.model.is_empty()).then(|| self.model.clone()),
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.busy {
            cx.emit(CloudVoiceEvent::Close);
        }
    }

    /// Writes the key (or removes it), then applies the pending grant when asked.
    fn save_key(&mut self, value: Option<String>, then_enable: bool, cx: &mut Context<Self>) {
        let Some(link) = self.link(cx) else { return };
        if self.busy || !self.grants(AuthEnvironmentScope::SettingsWrite) {
            return;
        }
        self.busy = true;
        self.error = None;
        let pending = then_enable.then(|| voice_consent::pending_cloud_choice(self.destination(), cx));
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        let provider = self.provider;
        cx.spawn(async move |this, cx| {
            let result = tokio.spawn(async move { speech::set_cloud_speech_key(&link, provider, value.as_deref()).await }).await;
            let ok = matches!(result, Ok(Ok(())));
            _ = this.update(cx, |this, cx| {
                this.busy = false;
                if ok {
                    if let Some(id) = &pending {
                        voice_consent::confirm_pending(id, cx);
                    }
                    cx.emit(CloudVoiceEvent::Close);
                } else {
                    this.error = Some(t!("Couldn’t save this change. Check your connection and try again."));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let draft = self.key.read(cx).value().trim().to_owned();
        let credential = is_api_key(&draft).then_some(draft.clone());
        let model_changed = self.model != self.current_model.clone().unwrap_or_default();
        let grant_changes = !self.active || model_changed;
        if self.routed() {
            let listed = matches!(&self.models, Read::Done(models) if models.iter().any(|m| m.id.as_str() == self.model));
            let key_ready = if draft.is_empty() { self.saved() == Some(true) } else { credential.is_some() };
            if !(listed && key_ready && (!self.active || model_changed || credential.is_some())) {
                return;
            }
            if let Some(credential) = credential {
                return self.save_key(Some(credential), grant_changes, cx);
            }
            // Model-only changes never write to the environment.
            if grant_changes {
                voice_consent::choose_audio_destination(Some(self.destination()), cx);
            }
            return self.close(cx);
        }
        if let Some(credential) = credential {
            self.save_key(Some(credential), grant_changes, cx);
        }
    }
}

impl Render for CloudVoiceConfiguration {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let name = speech::cloud_speech_label(self.provider);
        let company = speech::cloud_speech_provider_name(self.provider);
        let environment = self.environment_label.clone();
        let routed = self.routed();
        let can_use = self.grants(AuthEnvironmentScope::OrchestrationOperate);
        let can_manage = self.grants(AuthEnvironmentScope::SettingsWrite);
        let connected = can_use && self.link(cx).is_some();
        let saved = self.saved();
        let editing_key = can_manage && saved.is_some() && (routed || self.active || saved == Some(false));
        let draft = self.key.read(cx).value().trim().to_owned();
        let credential = is_api_key(&draft);
        let title = if self.remove {
            t!("Remove the {company} key?", company = company)
        } else if self.active {
            if routed { t!("Manage OpenRouter dictation") } else { t!("Manage {company} key", company = company) }
        } else {
            t!("Use {name} for dictation?", name = name)
        };
        let description = if self.remove {
            t!("Everyone using {name} for dictation on {environment} will lose access.", name = name, environment = environment.clone())
        } else if self.active && !routed {
            t!("Anyone with dictation access to {environment} can use this API key.", environment = environment.clone())
        } else if routed {
            t!(
                "After you stop, {environment} sends the recording to OpenRouter. OpenRouter chooses who runs your model. T3 requests zero data retention; your OpenRouter account may allow safety retention.",
                environment = environment.clone()
            )
        } else {
            t!(
                "After you stop, {environment} sends your recording and vocabulary hints, including project names, to {provider}.",
                environment = environment.clone(),
                provider = company
            )
        };
        let muted = |text: SharedString| div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(text);
        let label = |text: SharedString| div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(text);
        let key_hint = if !draft.is_empty() && !credential {
            t!("Enter an API key, not a URL or secret reference.")
        } else if self.active && !routed {
            t!("Updating the key keeps dictation enabled.")
        } else if routed && saved == Some(true) {
            t!("Leave blank to keep the saved key.")
        } else {
            t!("Anyone with dictation access to {environment} can bill this key.", environment = environment.clone())
        };
        let key_field = div()
            .flex()
            .flex_col()
            .gap(theme.sp(2.))
            .child(label(if routed && saved == Some(true) { t!("Replace API key") } else { t!("API key") }))
            .child(div().id("voice-cloud-key").child(TextInput::new(&self.key)).test_support())
            .child(muted(key_hint));
        let body: Option<gpui_kit::AnyElement> = (!self.remove).then(|| {
            let content: gpui_kit::AnyElement = if matches!(self.session, Read::Loading) {
                muted(t!("Checking for an API key…")).into_any_element()
            } else if !connected {
                div().child(t!("Connect to {environment} with dictation access to continue.", environment = environment.clone())).into_any_element()
            } else if matches!(self.keys, Read::Failed) {
                Button::new("voice-cloud-retry-keys").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Retry loading API key"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.keys = Read::Loading;
                        this.load(cx);
                    }))
                    .into_any_element()
            } else if routed && matches!(self.keys, Read::Done(_)) && saved.is_none() {
                div().child(t!("Update {environment} to use OpenRouter dictation.", environment = environment.clone())).into_any_element()
            } else if saved.is_none() {
                muted(t!("Checking for an API key…")).into_any_element()
            } else if routed {
                let query = self.model_query.read(cx).value().to_lowercase();
                let models: Vec<CloudSpeechModel> = match &self.models {
                    Read::Done(models) => models.iter().filter(|m| query.is_empty() || m.name.to_lowercase().contains(&query) || m.id.as_str().to_lowercase().contains(&query)).cloned().collect(),
                    _ => Vec::new(),
                };
                let picker: gpui_kit::AnyElement = match &self.models {
                    Read::Loading => muted(t!("Loading transcription models…")).into_any_element(),
                    Read::Failed => Button::new("voice-cloud-retry-models").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Retry loading models"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.models = Read::Loading;
                            this.load(cx);
                        }))
                        .into_any_element(),
                    Read::Done(_) => div()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(1.))
                        .child(div().id("voice-cloud-model").child(TextInput::new(&self.model_query)).test_support())
                        .child(
                            div()
                                .id("voice-cloud-model-list")
                                .flex()
                                .flex_col()
                                .max_h(theme.rem(16.))
                                .overflow_y_scroll()
                                .when(models.is_empty(), |d| d.child(muted(t!("No transcription model matches."))))
                                .children(models.into_iter().enumerate().map(|(i, model)| {
                                    let chosen = model.id.as_str() == self.model;
                                    let id = model.id.as_str().to_owned();
                                    div()
                                        .id(SharedString::from(format!("voice-cloud-model-{i}")))
                                        .flex()
                                        .flex_col()
                                        .px(theme.sp(2.))
                                        .py(theme.sp(1.))
                                        .rounded(theme.radii.sm)
                                        .cursor_pointer()
                                        .when(chosen, |d| d.bg(c.accent))
                                        .hover(|d| d.bg(theme.colors.accent))
                                        .child(div().truncate().ui_text(theme, TextSize::Sm, weight::NORMAL).child(model.name.clone()))
                                        .child(div().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(id.clone()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.model = id.clone();
                                            cx.notify();
                                        }))
                                        .test_support()
                                })),
                        )
                        .children(self.current_model.as_ref().filter(|current| !matches!(&self.models, Read::Done(models) if models.iter().any(|m| m.id.as_str() == current.as_str()))).map(|current| {
                            muted(t!("{model} is no longer listed.", model = current.clone()))
                        }))
                        .into_any_element(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(3.))
                    .child(div().flex().flex_col().gap(theme.sp(2.)).child(label(t!("Model"))).child(picker))
                    .child(if editing_key {
                        key_field.into_any_element()
                    } else if saved == Some(false) {
                        div().child(t!("Ask an administrator on {environment} to add an API key.", environment = environment.clone())).into_any_element()
                    } else {
                        muted(t!("Uses the API key saved on {environment}. This choice applies only to this device.", environment = environment.clone())).into_any_element()
                    })
                    .into_any_element()
            } else if editing_key {
                key_field.into_any_element()
            } else if saved == Some(false) {
                div().child(t!("Ask an administrator on {environment} to add an API key.", environment = environment.clone())).into_any_element()
            } else {
                muted(t!("This choice applies only to this device. Choose Off to stop sharing.")).into_any_element()
            };
            div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.foreground()).child(content).into_any_element()
        });
        let model_listed = matches!(&self.models, Read::Done(models) if models.iter().any(|m| m.id.as_str() == self.model));
        let model_changed = self.model != self.current_model.clone().unwrap_or_default();
        let key_ready = if draft.is_empty() { saved == Some(true) } else { credential };
        let can_submit = if routed { model_listed && key_ready && (!self.active || model_changed || credential) } else { credential };
        let busy = self.busy;
        let mut footer = div().flex().w_full().items_center().gap(theme.sp(2.));
        if connected && can_manage && saved == Some(true) && (self.active || routed) && !self.remove {
            footer = footer.child(
                div().mr_auto().child(
                    Button::new("voice-cloud-remove").variant(ButtonVariant::Ghost).size(ButtonSize::Sm).label(t!("Remove key…")).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.remove = true;
                            cx.notify();
                        })),
                ),
            );
        } else {
            footer = footer.child(div().flex_1());
        }
        footer = footer.child(
            Button::new("voice-cloud-cancel").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Cancel")).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
        );
        if connected {
            let primary: Option<Button> = if self.remove {
                Some(Button::new("voice-cloud-remove-confirm").variant(ButtonVariant::Destructive).size(ButtonSize::Sm).label(t!("Remove key")).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.save_key(None, false, cx))))
            } else if routed {
                (saved.is_some() && (saved == Some(true) || can_manage)).then(|| {
                    Button::new("voice-cloud-submit").size(ButtonSize::Sm).label(if self.active { t!("Save", context = "verb") } else { t!("Save and enable") }).disabled(busy || !can_submit)
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                })
            } else if editing_key {
                Some(Button::new("voice-cloud-submit").size(ButtonSize::Sm).label(if self.active { t!("Save key") } else { t!("Save and enable") }).disabled(busy || !credential)
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))))
            } else if saved == Some(true) && !self.active {
                Some(Button::new("voice-cloud-use").size(ButtonSize::Sm).label(t!("Use {name}", name = name))
                    .on_click(cx.listener(|this, _, _, cx| {
                        voice_consent::choose_audio_destination(Some(this.destination()), cx);
                        this.close(cx);
                    })))
            } else {
                None
            };
            footer = footer.children(primary);
        }
        let close = cx.entity().downgrade();
        let mut surface = DialogSurface::new(title).max_width_rem(28.).description(description).footer(footer);
        if let Some(body) = body {
            surface = surface.child(body);
        }
        if let Some(error) = self.error.clone() {
            surface = surface.child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.destructive_foreground()).child(error));
        }
        let surface = surface.on_close(move |_, cx| {
            close.update(cx, |this, cx| this.close(cx)).ok();
        });
        div().id("cloud-voice-configuration").child(surface).test_support()
    }
}
