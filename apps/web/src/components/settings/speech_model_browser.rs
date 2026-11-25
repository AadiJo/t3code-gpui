//! The model half of Settings → Voice (`VoiceSettingsPanel.tsx`: `useVoiceModels`,
//! `SpeechModelBrowser`, `ModelCard`, `ModelBenchmarks`, `TranscriptionTest`,
//! `AdvancedSpeechSettings`; `speechModelBrowser.ts`, `SpeechModelComparison.tsx`): the chosen
//! environment's model catalog with search, a language filter, sort and "Downloaded only";
//! download with progress, select, cancel, delete and "Use existing"; a test of the active
//! model that records without cleanup; and the advanced streaming, hardware and unload rows.
//! One browser belongs to one destination environment; Settings → Voice makes a new one when
//! the destination changes.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Task, Window, div, prelude::*, px, relative, svg,
};
use t3code_client_runtime::preview::environment_link::EnvironmentLink;
use t3code_client_runtime::voice_input::environment as env;
use t3code_client_runtime::voice_input::speech::{SpeechRecommendationRole, benchmark_seconds, recommend_speech_models};
use t3code_client_runtime::voice_input::{VoiceInputPhase, VoiceInputState};
use t3code_contracts::{
    AuthEnvironmentScope, EnvironmentId, EnvironmentSpeechModel, EnvironmentSpeechModelState, EnvironmentSpeechStatus,
    EnvironmentSpeechStatusSupported, SpeechAcceleration, SpeechAccelerationKnown, SpeechBenchmarkTier, SpeechModelUnloadTimeout,
};

use super::settings_layout::SettingsRow;
use super::settings_panels::advanced_settings;
use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::chat::composer_speech::{PillActions, recording_pill};
use crate::components::chat::composer_theme::ComposerColors;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::TextInput;
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::components::ui::popover::{Popover, PopupHandle, glass_surface};
use crate::components::ui::select::{Select, SelectSize};
use crate::components::ui::switch::Switch;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::{t, t_rich};
use crate::speech::language_names::language_label;
use crate::speech::voice_input_session::{VoiceInput, VoiceRecordingContext, VoiceTarget, preferred_speech_languages};
use crate::theme_color::with_alpha;

/// The browser Settings → Voice shows now, for scripted checks.
pub struct ShownSpeechModelBrowser(pub gpui_kit::WeakEntity<SpeechModelBrowser>);
impl gpui_kit::Global for ShownSpeechModelBrowser {}

/// `SpeechModelSort`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeechModelSort {
    Recommended,
    Size,
    Accuracy,
    Speed,
}

const SORTS: [SpeechModelSort; 4] = [SpeechModelSort::Recommended, SpeechModelSort::Size, SpeechModelSort::Accuracy, SpeechModelSort::Speed];

/// `UNLOAD_LABELS`, in the fork's order.
fn unload_label(timeout: SpeechModelUnloadTimeout) -> SharedString {
    match timeout {
        SpeechModelUnloadTimeout::Never => t!("Never", context = "unload the speech model"),
        SpeechModelUnloadTimeout::Immediately => t!("Immediately"),
        SpeechModelUnloadTimeout::Min2 => t!("After 2 minutes"),
        SpeechModelUnloadTimeout::Min5 => t!("After 5 minutes"),
        SpeechModelUnloadTimeout::Min10 => t!("After 10 minutes"),
        SpeechModelUnloadTimeout::Min15 => t!("After 15 minutes"),
        SpeechModelUnloadTimeout::Hour1 => t!("After 1 hour"),
    }
}

const UNLOADS: [SpeechModelUnloadTimeout; 7] = [
    SpeechModelUnloadTimeout::Never,
    SpeechModelUnloadTimeout::Immediately,
    SpeechModelUnloadTimeout::Min2,
    SpeechModelUnloadTimeout::Min5,
    SpeechModelUnloadTimeout::Min10,
    SpeechModelUnloadTimeout::Min15,
    SpeechModelUnloadTimeout::Hour1,
];

/// `modelSortOrder`.
fn model_sort_order(model: &EnvironmentSpeechModel) -> usize {
    match model.state {
        _ if model.active => 0,
        EnvironmentSpeechModelState::Installed => 1,
        EnvironmentSpeechModelState::Downloading | EnvironmentSpeechModelState::Verifying => 2,
        _ => 3,
    }
}

/// `filterSpeechModels`: the models matching the search, languages and filter, sorted.
pub fn filter_speech_models<'a>(
    models: &'a [EnvironmentSpeechModel],
    search: &str,
    languages: &[String],
    downloaded_only: bool,
    sort: SpeechModelSort,
    accuracy_language: &str,
    tier: Option<SpeechBenchmarkTier>,
    recommendations: &[(t3code_contracts::SpeechModelId, SpeechRecommendationRole)],
) -> Vec<&'a EnvironmentSpeechModel> {
    let query = search.trim().to_lowercase();
    let rank = |model: &EnvironmentSpeechModel| recommendations.iter().position(|(id, _)| *id == model.id).unwrap_or(3) as f64;
    let score = |model: &EnvironmentSpeechModel| match sort {
        SpeechModelSort::Size => model.size,
        SpeechModelSort::Accuracy => {
            model.benchmark.as_ref().and_then(|b| b.accuracy.get(accuracy_language)).map_or(f64::INFINITY, |score| score.error_percent)
        }
        SpeechModelSort::Speed => tier.and_then(|tier| benchmark_seconds(model, tier).flatten()).unwrap_or(f64::INFINITY),
        SpeechModelSort::Recommended => model_sort_order(model) as f64 * 4. + rank(model),
    };
    let mut matched: Vec<&EnvironmentSpeechModel> = models
        .iter()
        .filter(|model| {
            languages.iter().all(|language| model.languages.contains(language))
                && (!downloaded_only || model.state == EnvironmentSpeechModelState::Installed)
                && (query.is_empty() || format!("{} {}", model.name, model.description).to_lowercase().contains(&query))
        })
        .collect();
    matched.sort_by(|a, b| {
        score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal).then(model_sort_order(a).cmp(&model_sort_order(b)))
    });
    matched
}

/// `commonLanguages` in `SpeechModelComparison.tsx`.
const COMMON_LANGUAGES: [&str; 28] = [
    "en", "zh", "hi", "es", "fr", "bn", "pt", "id", "ur", "ru", "de", "ja", "mr", "vi", "te", "sw", "ha", "tr", "tl", "ta", "jw", "ko",
    "am", "th", "it", "gu", "kn", "yo",
];

/// `orderSpeechLanguages`: preferred languages, then the most spoken, then by name.
pub fn order_speech_languages(languages: &[String], preferred: &[String]) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    for code in preferred.iter().map(String::as_str).chain(COMMON_LANGUAGES) {
        if !order.iter().any(|known| known == code) {
            order.push(code.to_owned());
        }
    }
    let rank = |code: &str| order.iter().position(|known| known == code).unwrap_or(order.len());
    let mut sorted = languages.to_vec();
    sorted.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| language_label(a).cmp(&language_label(b))));
    sorted
}

/// `useVoiceModels` refresh modes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refresh {
    Foreground,
    Progress,
}

/// The catalog, browser and model test for one destination environment.
pub struct SpeechModelBrowser {
    environment_id: EnvironmentId,
    models: Vec<EnvironmentSpeechModel>,
    status: Option<EnvironmentSpeechStatus>,
    busy: bool,
    requested_download: Option<String>,
    revision: u64,
    search: Entity<InputState>,
    language_search: Entity<InputState>,
    existing_path: Entity<InputState>,
    sort: SpeechModelSort,
    downloaded_only: bool,
    selection: Option<Vec<String>>,
    preferred: Vec<String>,
    advanced_open: bool,
    /// The model test's transcript (`transcript`) and whether its panel shows.
    test_transcript: Option<String>,
    test_shown: bool,
    test_presented: bool,
    _poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SpeechModelBrowser {
    /// A browser for `environment_id`; it loads the catalog at once.
    pub fn new(environment_id: EnvironmentId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search models")));
        let language_search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search languages")));
        let existing_path =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("Already downloaded elsewhere? Paste the model file or folder path")));
        let mut subscriptions: Vec<Subscription> = [&search, &language_search, &existing_path]
            .into_iter()
            .map(|state| cx.subscribe(state, |_, _, _: &InputEvent, cx| cx.notify()))
            .collect();
        let voice = VoiceInput::global(cx);
        subscriptions.push(cx.observe(&voice, |_, _, cx| cx.notify()));
        subscriptions.push(cx.on_release(|this, cx| {
            if this.test_presented {
                let owner = this.test_owner();
                VoiceInput::global(cx).update(cx, |voice, cx| {
                    voice.cancel(&owner, cx);
                    voice.unpresent(&owner, cx);
                });
            }
        }));
        let mut this = Self {
            environment_id,
            models: Vec::new(),
            status: None,
            busy: false,
            requested_download: None,
            revision: 0,
            search,
            language_search,
            existing_path,
            sort: SpeechModelSort::Recommended,
            downloaded_only: false,
            selection: None,
            preferred: preferred_speech_languages(),
            advanced_open: false,
            test_transcript: None,
            test_shown: false,
            test_presented: false,
            _poll: None,
            _subscriptions: subscriptions,
        };
        this.refresh(Refresh::Foreground, cx);
        cx.set_global(ShownSpeechModelBrowser(cx.weak_entity()));
        this
    }

    /// Sets the model search (scripted checks type into the field the same way).
    pub fn set_search(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |state, cx| state.set_value(text.to_owned(), window, cx));
        cx.notify();
    }

    /// The models the list shows now, in order, by name.
    pub fn shown_model_names(&self, cx: &App) -> Vec<String> {
        let languages = self.selected_languages();
        let tier = match &self.status {
            Some(EnvironmentSpeechStatus::Supported(status)) => status.benchmark_tier,
            _ => None,
        };
        let recommendations = recommend_speech_models(&self.models, &languages, tier);
        let accuracy = languages.first().cloned().unwrap_or_else(|| "en".to_owned());
        filter_speech_models(&self.models, &self.search.read(cx).value(), &languages, self.downloaded_only, self.sort, &accuracy, tier, &recommendations)
            .into_iter()
            .map(|m| m.name.clone())
            .collect()
    }

    /// The badge each listed model earns.
    pub fn recommendations(&self) -> Vec<(String, SpeechRecommendationRole)> {
        let tier = match &self.status {
            Some(EnvironmentSpeechStatus::Supported(status)) => status.benchmark_tier,
            _ => None,
        };
        recommend_speech_models(&self.models, &self.selected_languages(), tier).into_iter().map(|(id, role)| (id.as_str().to_owned(), role)).collect()
    }

    /// The environment this browser reads.
    pub fn environment_id(&self) -> &EnvironmentId {
        &self.environment_id
    }

    /// The catalog as last read (for scripted checks).
    pub fn models(&self) -> &[EnvironmentSpeechModel] {
        &self.models
    }

    /// The status as last read.
    pub fn status(&self) -> Option<&EnvironmentSpeechStatus> {
        self.status.as_ref()
    }

    /// Chooses the languages the browser compares (`setSelection`), as the language menu does.
    pub fn set_languages(&mut self, languages: Vec<String>, cx: &mut Context<Self>) {
        self.selection = Some(languages);
        cx.notify();
    }

    /// Whether a model change is in flight.
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// The model test's transcript.
    pub fn test_transcript(&self) -> Option<&str> {
        self.test_transcript.as_deref()
    }

    fn link(&self, cx: &App) -> Option<EnvironmentLink> {
        ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::global(cx).environment_link(&self.environment_id)).flatten()
    }

    fn can_manage(&self, cx: &App) -> bool {
        VoiceInput::grants(&self.environment_id, AuthEnvironmentScope::SettingsWrite, cx) == Some(true)
    }

    fn downloading(&self) -> bool {
        self.requested_download.is_some()
            || self.models.iter().any(|m| matches!(m.state, EnvironmentSpeechModelState::Downloading | EnvironmentSpeechModelState::Verifying))
    }

    /// `refresh`: status and models together; a progress refresh keeps the benchmarks it has.
    fn refresh(&mut self, mode: Refresh, cx: &mut Context<Self>) {
        let Some(link) = self.link(cx) else { return };
        self.revision += 1;
        let revision = self.revision;
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        cx.spawn(async move |this, cx| {
            let result = tokio
                .spawn(async move {
                    let status = env::speech_status(&link).await?;
                    let models = env::speech_models(&link, mode == Refresh::Foreground).await?;
                    Ok::<_, env::VoiceRequestError>((status, models))
                })
                .await;
            _ = this.update(cx, |this, cx| {
                if revision != this.revision {
                    return;
                }
                match result {
                    Ok(Ok((status, models))) => {
                        this.status = Some(status);
                        this.models = if mode == Refresh::Foreground {
                            models.models
                        } else {
                            models
                                .models
                                .into_iter()
                                .map(|mut model| {
                                    if let Some(previous) = this.models.iter().find(|p| p.id == model.id && p.benchmark.is_some()) {
                                        model.benchmark = previous.benchmark.clone();
                                    }
                                    model
                                })
                                .collect()
                        };
                    }
                    Ok(Err(error)) if mode == Refresh::Foreground => {
                        toast_error(t!("Could not load transcription models"), error.to_string(), cx)
                    }
                    _ => {}
                }
                this.sync_poll(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Polls progress once a second while a download runs here or on the environment.
    fn sync_poll(&mut self, cx: &mut Context<Self>) {
        if !self.downloading() {
            self._poll = None;
            return;
        }
        if self._poll.is_some() {
            return;
        }
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this.update(cx, |this, cx| {
                    this.refresh(Refresh::Progress, cx);
                    this.downloading()
                });
                if !matches!(keep, Ok(true)) {
                    _ = this.update(cx, |this, _| this._poll = None);
                    return;
                }
            }
        }));
    }

    /// `run`: one environment change; a failure shows `label`, and the catalog refreshes after.
    fn run<F>(&mut self, label: SharedString, download: Option<String>, work: impl FnOnce(EnvironmentLink) -> F, cx: &mut Context<Self>)
    where
        F: std::future::Future<Output = Result<(), env::VoiceRequestError>> + Send + 'static,
    {
        let Some(link) = self.link(cx) else { return };
        if !self.can_manage(cx) {
            return;
        }
        match &download {
            Some(id) => self.requested_download = Some(id.clone()),
            None => self.busy = true,
        }
        self.sync_poll(cx);
        cx.notify();
        let future = work(link);
        let tokio = ConnectionRuntime::global(cx).handle().clone();
        cx.spawn(async move |this, cx| {
            let result = tokio.spawn(future).await;
            _ = this.update(cx, |this, cx| {
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(error) => Some(error.to_string()),
                };
                if let Some(error) = error {
                    toast_error(label, error, cx);
                }
                if download.is_some() {
                    this.requested_download = None;
                } else {
                    this.busy = false;
                }
                this.refresh(Refresh::Foreground, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn download(&mut self, id: String, cx: &mut Context<Self>) {
        let model = id.clone();
        self.run(t!("Could not download the model"), Some(id), move |link| async move { env::download_speech_model(&link, &model).await.map(drop) }, cx);
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.run(t!("Could not select the model"), None, move |link| async move { env::select_speech_model(&link, &id).await.map(drop) }, cx);
    }

    fn cancel_download(&mut self, id: String, cx: &mut Context<Self>) {
        self.run(t!("Could not cancel the download"), None, move |link| async move { env::cancel_speech_model_download(&link, &id).await.map(drop) }, cx);
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        self.run(t!("Could not delete the model"), None, move |link| async move { env::remove_speech_model(&link, &id).await.map(drop) }, cx);
    }

    fn use_existing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.existing_path.read(cx).value().trim().to_owned();
        if path.is_empty() {
            return;
        }
        self.existing_path.update(cx, |state, cx| state.set_value(String::new(), window, cx));
        self.run(t!("Could not use a model from that path"), None, move |link| async move { env::link_speech_models(&link, &path).await.map(drop) }, cx);
    }

    pub fn set_acceleration(&mut self, acceleration: SpeechAcceleration, cx: &mut Context<Self>) {
        self.run(
            t!("Could not change transcription hardware"),
            None,
            move |link| async move { env::update_speech_acceleration(&link, &acceleration).await.map(drop) },
            cx,
        );
    }

    pub fn set_unload(&mut self, timeout: SpeechModelUnloadTimeout, cx: &mut Context<Self>) {
        self.run(
            t!("Could not change when the model unloads"),
            None,
            move |link| async move { env::update_speech_model_unload_timeout(&link, timeout).await.map(drop) },
            cx,
        );
    }

    /// The model test's draft key: one per environment (`[environmentId, projectId]`).
    fn test_owner(&self) -> SharedString {
        SharedString::from(format!("voice-test:{}", self.environment_id.as_str()))
    }

    /// The draft a test recording writes into: this browser's transcript, without cleanup.
    fn test_target(&self, cx: &mut Context<Self>) -> VoiceTarget {
        let (read, commit) = (cx.weak_entity(), cx.weak_entity());
        VoiceTarget {
            owner_key: self.test_owner(),
            label: None,
            read_draft: Rc::new(move |cx| {
                let text = read.upgrade().map(|b| b.read(cx).test_transcript.clone().unwrap_or_default())?;
                let end = text.len();
                Some((text, end..end))
            }),
            commit_draft: Rc::new(move |text, _, cx| {
                _ = commit.update(cx, |this, cx| {
                    this.test_transcript = Some(text);
                    cx.notify();
                });
            }),
            open: None,
            context: VoiceRecordingContext { environment_id: Some(self.environment_id.clone()), project_name: None, project_words: Vec::new(), cleanup: false },
        }
    }

    /// "Test model": shows the transcript panel and starts recording.
    pub fn start_test(&mut self, cx: &mut Context<Self>) {
        self.test_shown = true;
        let owner = self.test_owner();
        let target = self.test_target(cx);
        let present = !self.test_presented;
        self.test_presented = true;
        VoiceInput::global(cx).update(cx, |voice, cx| {
            if present {
                voice.present(owner, cx);
            }
            voice.start(target, cx);
        });
        cx.notify();
    }

    fn close_test(&mut self, cx: &mut Context<Self>) {
        let owner = self.test_owner();
        VoiceInput::global(cx).update(cx, |voice, cx| voice.cancel(&owner, cx));
        self.test_shown = false;
        cx.notify();
    }

    fn selected_languages(&self) -> Vec<String> {
        if let Some(selection) = &self.selection {
            return selection.clone();
        }
        let supported: Vec<&String> = self.models.iter().flat_map(|m| &m.languages).collect();
        self.preferred.iter().find(|code| supported.contains(code)).cloned().into_iter().collect()
    }

    fn sort_label(sort: SpeechModelSort) -> SharedString {
        match sort {
            SpeechModelSort::Recommended => t!("Recommended"),
            SpeechModelSort::Size => t!("Smallest"),
            SpeechModelSort::Accuracy => t!("Most accurate"),
            SpeechModelSort::Speed => t!("Fastest"),
        }
    }

    /// The Models row: nothing until the status is read, the reason when unsupported.
    pub fn models_row(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.status.clone()?;
        let status = match status {
            EnvironmentSpeechStatus::Supported(status) => status,
            EnvironmentSpeechStatus::NotSupported(status) => {
                return Some(
                    SettingsRow::new(t!("Models"))
                        .description(t!("This environment can’t transcribe audio: {reason}.", reason = status.reason))
                        .into_any_element(),
                );
            }
            EnvironmentSpeechStatus::Unknown(_) => return None,
        };
        let theme = cx.app_theme().clone();
        let selected_languages = self.selected_languages();
        let accuracy_language = selected_languages.first().cloned().unwrap_or_else(|| "en".to_owned());
        let tier = status.benchmark_tier;
        let recommendations = recommend_speech_models(&self.models, &selected_languages, tier);
        let search = self.search.read(cx).value().to_string();
        let sorted =
            filter_speech_models(&self.models, &search, &selected_languages, self.downloaded_only, self.sort, &accuracy_language, tier, &recommendations);
        let sorted_ids: Vec<String> = sorted.iter().map(|m| m.id.as_str().to_owned()).collect();
        let mut rendered: Vec<EnvironmentSpeechModel> = sorted.into_iter().cloned().collect();
        if let Some(active) = self.models.iter().find(|m| m.active)
            && !sorted_ids.iter().any(|id| id == active.id.as_str())
        {
            rendered.insert(0, active.clone());
        }
        let languages_label = if selected_languages.is_empty() {
            t!("All languages")
        } else {
            SharedString::from(selected_languages.iter().map(|code| language_label(code).to_string()).collect::<Vec<_>>().join(", "))
        };
        let tier_name = if tier == Some(SpeechBenchmarkTier::Cpu) { "CPU" } else { "GPU" };
        let controls_disabled = self.busy || self.downloading() || !self.can_manage(cx);
        let can_cancel = self.can_manage(cx) && !self.busy;
        let toolbar = self.toolbar(&theme, languages_label, &selected_languages, &accuracy_language, cx);
        let border = with_alpha(theme.colors.border, 0.6);
        let mut list = div()
            .id("speech-models-list")
            .mt(theme.sp(2.))
            .mb(theme.sp(2.))
            .max_h(theme.rem(24.))
            .overflow_y_scroll()
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(border)
            .flex()
            .flex_col();
        if sorted_ids.is_empty() {
            list = list.child(
                div()
                    .px(theme.sp(3.))
                    .py(theme.sp(3.))
                    .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .child(t_rich!("No models match.").plain()),
            );
        }
        for (ix, model) in rendered.into_iter().enumerate() {
            let mut model = model;
            if self.requested_download.as_deref() == Some(model.id.as_str()) && model.state == EnvironmentSpeechModelState::Downloadable {
                model.state = EnvironmentSpeechModelState::Downloading;
            }
            let role = recommendations.iter().find(|(id, _)| *id == model.id).map(|(_, role)| *role);
            let card = self.model_card(&theme, &model, role, &selected_languages, tier, controls_disabled, can_cancel, cx);
            list = list.child(div().when(ix > 0 || sorted_ids.is_empty(), |d| d.border_t_1().border_color(border)).child(card));
        }
        let mut body = div().flex().flex_col().child(toolbar).child(list.test_support());
        let can_read_files = VoiceInput::grants(&self.environment_id, AuthEnvironmentScope::FilesystemRead, cx) == Some(true);
        if status.supports_model_linking == Some(true) && can_read_files {
            let empty = self.existing_path.read(cx).value().trim().is_empty();
            body = body.child(
                div()
                    .mb(theme.sp(2.))
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(div().id("speech-existing-path").flex_1().min_w_0().child(TextInput::new(&self.existing_path)).test_support())
                    .child(
                        Button::new("speech-use-existing")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .label(t_rich!("Use existing").plain())
                            .disabled(controls_disabled || empty)
                            .on_click(cx.listener(|this, _, window, cx| this.use_existing(window, cx))),
                    ),
            );
        }
        Some(
            SettingsRow::new(t!("Models"))
                .description(t!(
                    "Grades and seconds per 30 s use Handy’s reference {tier}, not your hardware. Select languages for balanced recommendations.",
                    tier = tier_name
                ))
                .child(body)
                .into_any_element(),
        )
    }

    fn toolbar(
        &mut self,
        theme: &AppTheme,
        languages_label: SharedString,
        selected_languages: &[String],
        accuracy_language: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let all: Vec<String> = {
            let mut codes: Vec<String> = Vec::new();
            for code in self.models.iter().flat_map(|m| m.languages.iter()) {
                if !codes.contains(code) {
                    codes.push(code.clone());
                }
            }
            order_speech_languages(&codes, &self.preferred)
        };
        let needle = self.language_search.read(cx).value().trim().to_lowercase();
        let visible: Vec<String> =
            all.into_iter().filter(|code| needle.is_empty() || code.contains(&needle) || language_label(code).to_lowercase().contains(&needle)).collect();
        let selected = selected_languages.to_vec();
        let language_search = self.language_search.clone();
        let languages_trigger = Button::new("speech-language-filter")
            .variant(ButtonVariant::Outline)
            .icon(IconName::Languages)
            .label(languages_label)
            .label_max_width(theme.rem(14.) - theme.sp(10.));
        let language_menu = {
            let this = this.clone();
            Popover::new("speech-language-filter-menu", languages_trigger, move |_handle: PopupHandle, _, cx| {
                let theme = cx.app_theme().clone();
                let c = &theme.colors;
                let row = |id: SharedString, label: SharedString, checked: bool, pick: Rc<dyn Fn(&mut App)>| {
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .gap(theme.sp(2.))
                        .h(theme.sp(7.))
                        .flex_none()
                        .px(theme.sp(2.))
                        .pr(theme.sp(4.))
                        .rounded(theme.radii.sm)
                        .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                        .cursor_pointer()
                        .hover(|d| d.bg(c.highlight()).text_color(c.highlight_foreground()))
                        .child(div().w(theme.sp(4.)).flex_none().when(checked, |slot| {
                            slot.child(svg().path("icons/ui/menu-check.svg").size(theme.sp(4.)).text_color(c.foreground()))
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .on_click(move |_, _, cx| pick(cx))
                        .test_support()
                        .into_any_element()
                };
                let set = |next: Vec<String>, this: gpui_kit::WeakEntity<SpeechModelBrowser>| -> Rc<dyn Fn(&mut App)> {
                    Rc::new(move |cx| {
                        _ = this.update(cx, |this, cx| {
                            this.selection = Some(next.clone());
                            cx.notify();
                        });
                    })
                };
                let mut rows: Vec<AnyElement> = Vec::new();
                if needle.is_empty() {
                    rows.push(row("speech-language-all".into(), t_rich!("All languages").plain().into(), selected.is_empty(), set(Vec::new(), this.clone())));
                }
                for code in &visible {
                    let checked = selected.contains(code);
                    let next: Vec<String> =
                        if checked { selected.iter().filter(|c| *c != code).cloned().collect() } else { selected.iter().cloned().chain([code.clone()]).collect() };
                    rows.push(row(format!("speech-language-{code}").into(), language_label(code), checked, set(next, this.clone())));
                }
                if visible.is_empty() {
                    rows.push(
                        div()
                            .px(theme.sp(2.))
                            .py(theme.sp(1.5))
                            .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .child(t_rich!("No languages match.").plain())
                            .into_any_element(),
                    );
                }
                glass_surface(&theme)
                    .min_w(theme.rem(10.))
                    .px(theme.sp(1.))
                    .py(theme.sp(1.))
                    .child(div().mb(theme.sp(1.)).child(TextInput::new(&language_search).leading_icon(IconName::Search)))
                    .child(div().id("speech-language-list").max_h(theme.rem(18.)).overflow_y_scroll().flex().flex_col().children(rows))
                    .into_any_element()
            })
        };
        let sort = self.sort;
        let downloaded_only = self.downloaded_only;
        let accuracy = language_label(accuracy_language);
        let sort_menu = {
            let this = this.clone();
            dropdown_menu(
                "speech-model-sort",
                Button::new("speech-model-sort-trigger").variant(ButtonVariant::Outline).icon(IconName::ArrowDownUp).label(Self::sort_label(sort)),
                move |_, _| {
                    let mut menu = MenuPopup::new().label(t_rich!("Sort by").plain());
                    for option in SORTS {
                        let this = this.clone();
                        let label =
                            if option == SpeechModelSort::Accuracy { t!("Most accurate in {language}", language = accuracy.clone()) } else { Self::sort_label(option) };
                        menu = menu.item(MenuItem::new(label).kind(MenuItemKind::Radio(option == sort)).on_select(move |_, cx| {
                            _ = this.update(cx, |this, cx| {
                                this.sort = option;
                                cx.notify();
                            });
                        }));
                    }
                    let this = this.clone();
                    menu.separator().item(
                        MenuItem::new(t_rich!("Downloaded only").plain()).kind(MenuItemKind::Checkbox(downloaded_only)).keep_open().on_select(move |_, cx| {
                            _ = this.update(cx, |this, cx| {
                                this.downloaded_only = !this.downloaded_only;
                                cx.notify();
                            });
                        }),
                    )
                },
            )
        };
        div()
            .mt(theme.sp(3.))
            .flex()
            .min_w_0()
            .gap(theme.sp(2.))
            .child(div().id("speech-model-search").flex_1().min_w_0().child(TextInput::new(&self.search).leading_icon(IconName::Search)).test_support())
            .child(div().id("speech-language-filter-wrap").max_w(theme.rem(14.)).child(language_menu).test_support())
            .child(div().id("speech-model-sort-wrap").child(sort_menu).test_support())
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn model_card(
        &mut self,
        theme: &AppTheme,
        model: &EnvironmentSpeechModel,
        role: Option<SpeechRecommendationRole>,
        languages: &[String],
        tier: Option<SpeechBenchmarkTier>,
        busy: bool,
        can_cancel: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let muted = c.muted_foreground;
        let downloading = matches!(model.state, EnvironmentSpeechModelState::Downloading | EnvironmentSpeechModelState::Verifying);
        let progress = model.downloaded.map_or(0., |d| d / model.size * 100.);
        let id = model.id.as_str().to_owned();
        let slug = id.clone();
        let badge = |key: &str, icon: IconName, label: SharedString, tip: SharedString| {
            div()
                .id(SharedString::from(format!("speech-badge-{key}-{slug}")))
                .tooltip(tooltip(tip))
                .child(Badge::new(label).icon(icon).variant(BadgeVariant::Outline))
        };
        let mut badges = div().flex().flex_wrap().items_center().gap(theme.sp(1.5)).child(
            div().ui_text(theme, TextSize::Sm, weight::MEDIUM).line_height(theme.sp(5.)).child(model.name.clone()),
        );
        if let Some(role) = role {
            let (label, tip) = match role {
                SpeechRecommendationRole::Recommended => (
                    t!("Recommended"),
                    t!("Balanced accuracy and speed for your selected languages on Handy’s reference benchmarks."),
                ),
                SpeechRecommendationRole::Faster => (
                    t!("Faster", context = "speech model badge, quicker than Recommended"),
                    t!("Quicker than Recommended on this reference tier, with useful accuracy."),
                ),
                SpeechRecommendationRole::Accurate => (
                    t!("More accurate"),
                    t!("Higher accuracy with a longer reference processing budget for your selected languages."),
                ),
            };
            badges = badges.child(badge("role", IconName::Star, label, tip));
        }
        if model.supports_streaming {
            badges = badges.child(badge(
                "live",
                IconName::AudioLines,
                t!("Live preview"),
                t!("Shows words in your draft as you speak, before the recording finishes."),
            ));
        }
        if model.supports_translation {
            badges = badges.child(badge(
                "translate",
                IconName::Languages,
                t!("Translate"),
                t!("Can write English text from speech in another language. Turn on Translate to English to use it."),
            ));
        }
        let mut text = div().flex_1().min_w_0().flex().flex_col().child(badges);
        if !model.description.is_empty() {
            text = text.child(
                div()
                    .mt(theme.sp(0.5))
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .line_height(px(TextSize::Xs.size(theme.fonts.interface) * 1.5))
                    .text_color(muted)
                    .child(model.description.clone()),
            );
        }
        if let Some(linked) = &model.linked_from {
            text = text.child(
                div()
                    .mt(theme.sp(0.5))
                    .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                    .text_color(muted)
                    .child(t_rich!("Using your file at {linkedFrom}", linkedFrom = linked.as_str()).plain()),
            );
        }
        text = text.child(benchmarks(theme, model, languages, tier));
        if downloading {
            let label = if model.state == EnvironmentSpeechModelState::Verifying {
                t!("Verifying download…")
            } else {
                t!("{percent}% downloaded", percent = progress.round() as i64)
            };
            text = text.child(
                div()
                    .mt(theme.sp(2.))
                    .child(
                        div()
                            .h(theme.sp(1.))
                            .overflow_hidden()
                            .rounded_full()
                            .bg(c.muted)
                            .child(div().h_full().w(relative((progress / 100.).clamp(0., 1.) as f32)).bg(c.primary())),
                    )
                    .child(div().mt(theme.sp(1.)).ui_text(theme, TextSize::Xs2, weight::NORMAL).text_color(muted).child(label)),
            );
        }
        let mut controls = div().flex().flex_none().flex_wrap().items_center().gap(theme.sp(1.5));
        let mut test_panel: Option<AnyElement> = None;
        if model.active {
            let (test_controls, panel) = self.test_controls(theme, cx);
            controls = controls.child(test_controls);
            test_panel = panel;
        }
        let action: AnyElement = if downloading {
            let id = id.clone();
            Button::new(SharedString::from(format!("speech-cancel-{id}")))
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::IconSm)
                .icon(IconName::X)
                .icon_size(IconSize::Custom(theme.sp(3.5)))
                .disabled(!can_cancel)
                .on_click(cx.listener(move |this, _, _, cx| this.cancel_download(id.clone(), cx)))
                .into_any_element()
        } else if model.state == EnvironmentSpeechModelState::Downloadable {
            let id = id.clone();
            Button::new(SharedString::from(format!("speech-download-{id}")))
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .icon(IconName::Download)
                .label(t_rich!("Download").plain())
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.download(id.clone(), cx)))
                .into_any_element()
        } else if model.active {
            Button::new(SharedString::from(format!("speech-selected-{id}")))
                .variant(ButtonVariant::Secondary)
                .size(ButtonSize::Sm)
                .icon(IconName::Check)
                .label(t_rich!("Selected").plain())
                .disabled(true)
                .into_any_element()
        } else {
            let id = id.clone();
            Button::new(SharedString::from(format!("speech-use-{id}")))
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .label(t_rich!("Use model").plain())
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
                .into_any_element()
        };
        controls = controls.child(action);
        if model.state == EnvironmentSpeechModelState::Installed {
            let id = id.clone();
            controls = controls.child(
                Button::new(SharedString::from(format!("speech-delete-{id}")))
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::IconSm)
                    .icon(if model.linked_from.is_some() { IconName::Unlink } else { IconName::Trash2 })
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.delete(id.clone(), cx))),
            );
        }
        let stacked = model.active && self.test_shown;
        div()
            .id(SharedString::from(format!("speech-model-{}", model.id.as_str())))
            .px(theme.sp(3.))
            .py(theme.sp(3.))
            .child(
                div()
                    .flex()
                    .map(|d| if stacked { d.flex_col().gap(theme.sp(2.5)) } else { d.items_start().gap(theme.sp(4.)) })
                    .child(text)
                    .child(controls),
            )
            .children(test_panel)
            .test_support()
            .into_any_element()
    }

    /// `TranscriptionTest`'s controls and, once started, its transcript panel.
    fn test_controls(&mut self, theme: &AppTheme, cx: &mut Context<Self>) -> (AnyElement, Option<AnyElement>) {
        let owner = self.test_owner();
        let voice = VoiceInput::global(cx);
        let (state, started, levels, available, preview) = {
            let voice = voice.read(cx);
            (
                voice.state_for(&owner),
                voice.recording_started_at(&owner),
                voice.activity_for(&owner).map(|a| a.levels).unwrap_or_default(),
                voice.available(&owner, cx),
                voice.activity_for(&owner).and_then(|a| a.preview.clone()),
            )
        };
        let mut row = div().flex().min_w_0().items_center().gap(theme.sp(1.));
        if state.phase != VoiceInputPhase::Idle {
            row = row.child(self.test_pill(&state, started, &levels, cx));
        }
        if available {
            if state.phase == VoiceInputPhase::Idle {
                row = row.child(
                    Button::new("speech-test-model")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .icon(IconName::Mic)
                        .label(t_rich!("Test model").plain())
                        .on_click(cx.listener(|this, _, _, cx| this.start_test(cx))),
                );
            }
        } else {
            let reason = match voice.read(cx).status(cx) {
                Some(EnvironmentSpeechStatus::NotSupported(status)) => SharedString::from(status.reason),
                _ => t!("Waiting for the environment to connect"),
            };
            row = row.child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(reason));
        }
        let panel = self.test_shown.then(|| {
            let c = &theme.colors;
            let content: AnyElement = match (&preview, &self.test_transcript) {
                (None, None) => div()
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(c.muted_foreground)
                    .child(t_rich!("Say a sentence. Cleanup is skipped, so you see exactly what the model heard.").plain())
                    .into_any_element(),
                (Some(preview), _) => div()
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .line_height(px(TextSize::Sm.size(theme.fonts.interface) * 1.625))
                    .child(preview.committed.clone())
                    .child(div().text_color(c.muted_foreground).child(preview.tentative.clone()))
                    .into_any_element(),
                (None, Some(transcript)) => div()
                    .id("speech-test-transcript")
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .line_height(px(TextSize::Sm.size(theme.fonts.interface) * 1.625))
                    .child(if transcript.is_empty() { t!("No speech recognized. Try again.") } else { SharedString::from(transcript.clone()) })
                    .test_support()
                    .into_any_element(),
            };
            div()
                .mt(theme.sp(3.))
                .flex()
                .items_start()
                .gap(theme.sp(3.))
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(with_alpha(c.border, 0.6))
                .bg(with_alpha(c.muted, 0.3))
                .py(theme.sp(1.))
                .pl(theme.sp(3.))
                .pr(theme.sp(1.))
                .child(div().flex_1().min_w_0().py(theme.sp(1.5)).child(content))
                .child(
                    Button::new("speech-test-close")
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::IconSm)
                        .icon(IconName::X)
                        .on_click(cx.listener(|this, _, _, cx| this.close_test(cx))),
                )
                .into_any_element()
        });
        (div().id("speech-test-controls").child(row).test_support().into_any_element(), panel)
    }

    fn test_pill(&self, state: &VoiceInputState, started: Option<std::time::Instant>, levels: &[f32; 24], cx: &mut Context<Self>) -> AnyElement {
        let owner = self.test_owner();
        let colors = ComposerColors::new(cx);
        let (cancel, finish, skip) = (owner.clone(), owner.clone(), owner.clone());
        let this = cx.entity().downgrade();
        let actions = PillActions {
            cancel: Rc::new(move |_, cx| VoiceInput::global(cx).update(cx, |voice, cx| voice.cancel(&cancel, cx))),
            finish: Rc::new(move |_, cx| VoiceInput::global(cx).update(cx, |voice, cx| voice.stop(&finish, cx))),
            skip: Rc::new(move |_, cx| VoiceInput::global(cx).update(cx, |voice, _| voice.skip_post_processing(&skip))),
            retry: Rc::new(move |_, cx| {
                _ = this.update(cx, |this, cx| this.start_test(cx));
            }),
        };
        recording_pill(state, started, levels, None, false, actions, &colors)
    }

    /// `AdvancedSpeechSettings`: streaming, hardware and unload, folded under "Advanced".
    pub fn advanced(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(EnvironmentSpeechStatus::Supported(status)) = self.status.clone() else { return None };
        let rows = self.advanced_rows(&status, cx);
        let this = cx.entity().downgrade();
        Some(div().id("voice-advanced-fold").child(advanced_settings(
            "voice-advanced",
            self.advanced_open,
            Rc::new(move |_, cx| {
                _ = this.update(cx, |this, cx| {
                    this.advanced_open = !this.advanced_open;
                    cx.notify();
                });
            }),
            rows,
        )).test_support().into_any_element())
    }

    fn advanced_rows(&mut self, status: &EnvironmentSpeechStatusSupported, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let disabled = self.busy || self.downloading() || !self.can_manage(cx);
        let streaming = ClientSettingsStore::get(cx).speech_streaming;
        let streaming_row = SettingsRow::new(t!("Streaming transcription"))
            .description(if status.supports_streaming {
                t!("Transcribe while recording. Turn off to transcribe after stopping, which may help when live transcription falls behind.")
            } else {
                t!("This model doesn’t support native streaming.")
            })
            .control(
                Switch::new("switch-speech-streaming", streaming)
                    .disabled(!status.supports_streaming)
                    .on_change(|checked, _, cx| ClientSettingsStore::update(cx, |s| s.speech_streaming = checked)),
            );
        let mut hardware: Vec<(SpeechAcceleration, SharedString)> =
            vec![(SpeechAcceleration::Known(SpeechAccelerationKnown::Auto), t!("Auto")), (SpeechAcceleration::Known(SpeechAccelerationKnown::Cpu), t!("CPU"))];
        hardware.extend(status.gpu_devices.iter().map(|device| (SpeechAcceleration::Other(format!("gpu:{}", device.id)), SharedString::from(device.name.clone()))));
        let current = hardware.iter().position(|(value, _)| *value == status.acceleration);
        let hardware_label = current.map(|ix| hardware[ix].1.clone()).unwrap_or_else(|| t!("Selected GPU"));
        let values: Vec<SpeechAcceleration> = hardware.iter().map(|(value, _)| value.clone()).collect();
        let this = cx.entity().downgrade();
        let hardware_select = Select::new("select-speech-hardware", hardware.into_iter().map(|(_, label)| label).collect::<Vec<_>>(), current.unwrap_or(0), move |ix, _, cx| {
            if let Some(value) = values.get(ix).cloned().filter(|_| !disabled) {
                _ = this.update(cx, |this, cx| this.set_acceleration(value, cx));
            }
        })
        .size(SelectSize::Sm)
        .value_label(hardware_label);
        let this = cx.entity().downgrade();
        let unload_current = UNLOADS.iter().position(|t| *t == status.model_unload_timeout).unwrap_or(0);
        let unload_select = Select::new("select-speech-unload", UNLOADS.map(unload_label).to_vec(), unload_current, move |ix, _, cx| {
            if let Some(timeout) = UNLOADS.get(ix).copied().filter(|_| !disabled) {
                _ = this.update(cx, |this, cx| this.set_unload(timeout, cx));
            }
        })
        .size(SelectSize::Sm);
        let theme = cx.app_theme();
        let max = theme.rem(18.);
        let wrap = |id: &'static str, select: Select| {
            div().id(id).max_w(max).when(disabled, |d| d.opacity(0.64)).child(select).test_support()
        };
        vec![
            streaming_row.into_any_element(),
            SettingsRow::new(t!("Hardware"))
                .description(t!("Auto uses the environment’s GPU when it has one."))
                .control(wrap("speech-hardware", hardware_select))
                .into_any_element(),
            SettingsRow::new(t!("Unload model"))
                .description(t!("Release the model’s memory on the environment after it sits idle."))
                .control(wrap("speech-unload", unload_select))
                .into_any_element(),
        ]
    }
}

impl Render for SpeechModelBrowser {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// `ModelBenchmarks`: a score per language, the reference speed, then plain facts.
fn benchmarks(theme: &AppTheme, model: &EnvironmentSpeechModel, languages: &[String], tier: Option<SpeechBenchmarkTier>) -> AnyElement {
    let c = &theme.colors;
    let strong = |text: SharedString| div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(text);
    let plain = |text: SharedString| div().child(text);
    let segments = |rich: crate::i18n::RichText| {
        div().flex().children(rich.segments.into_iter().map(|segment| {
            if segment.tag.as_deref() == Some("strong") { strong(segment.text).into_any_element() } else { plain(segment.text).into_any_element() }
        }))
    };
    let data = model.benchmark.as_ref();
    let visible: Vec<String> = match data {
        None => Vec::new(),
        Some(_) if !languages.is_empty() => languages.to_vec(),
        Some(data) => model.languages.iter().find(|code| data.accuracy.contains_key(code.as_str())).or(model.languages.first()).cloned().into_iter().collect(),
    };
    let mut row = div()
        .mt(theme.sp(1.))
        .flex()
        .flex_wrap()
        .gap_x(theme.sp(3.))
        .gap_y(theme.sp(1.))
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .text_color(c.muted_foreground);
    for code in &visible {
        let language = language_label(code);
        row = row.child(match data.and_then(|d| d.accuracy.get(code.as_str())) {
            Some(score) => segments(t_rich!(
                "{language}:{\" \"}<strong>{grade}</strong>{\" \"}({errorPercent}% error)",
                language = language.to_string(),
                grade = score.grade.as_str(),
                errorPercent = format_number(score.error_percent)
            )),
            None => segments(t_rich!("{language}: not benchmarked", language = language.to_string())),
        });
    }
    if data.is_some() {
        let seconds = tier.and_then(|tier| benchmark_seconds(model, tier));
        row = row.child(match seconds {
            None => segments(t_rich!("Speed: update server for this benchmark")),
            Some(None) => segments(t_rich!("Speed: not benchmarked")),
            Some(Some(seconds)) => {
                segments(t_rich!("Speed:{\" \"}<strong>{seconds} s</strong>{\" \"}/ 30 s", seconds = format_number((seconds * 100.).round() / 100.)))
            }
        });
    }
    let mut facts: Vec<SharedString> = Vec::new();
    if model.languages.len() > 1 {
        facts.push(t!(
            "{count, plural, one {{count} language} other {{count} languages}}",
            count = model.languages.len() as i64
        ));
    } else if model.benchmark.is_none() {
        facts.push(language_label(model.languages.first().map(String::as_str).unwrap_or("")));
    }
    facts.push(format!("{} MB", (model.size / 1024. / 1024.).round() as i64).into());
    row.children(facts.into_iter().map(|fact| div().child(fact))).into_any_element()
}

/// A number as JavaScript prints it: no trailing ".0".
fn format_number(value: f64) -> String {
    if value.fract() == 0. { format!("{}", value as i64) } else { format!("{value}") }
}

fn toast_error(title: SharedString, description: String, cx: &mut App) {
    Toaster::push(ToastData::new(ToastKind::Error, title).description(SharedString::from(description)), cx);
}
