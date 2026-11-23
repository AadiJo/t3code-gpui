//! The settings stores every view reads. Ports `hooks/useSettings.ts`.
//!
//! - [`ClientSettingsStore`]: this device's [`ClientSettings`], persisted to
//!   `client-settings.json` in the app's data directory with the Electron
//!   app's JSON shape. Defaults come from the schema; keys this build does
//!   not know are kept and written back.
//! - [`DesktopSettingsStore`]: the few `desktop-settings.json` keys that still
//!   mean something without a local server.
//! - [`ServerSettingsStore`]: each connected environment's server config
//!   (settings, keybindings, environment themes) from its
//!   `subscribeServerConfig` stream, and [`ServerSettingsStore::update`],
//!   which applies a patch optimistically and calls `server.updateSettings`.
//!
//! Each store is a GPUI global, so a change notifies every observer before
//! the next frame. Views subscribe with [`observe_client_settings`], which
//! re-renders only when the selected value changes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, Context, Global, Subscription, Task};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_client_runtime::rpc::RpcSession;
use t3code_contracts::rpc::RpcMethod;
use t3code_contracts::rpc::methods::ServerUpdateSettings;
use t3code_contracts::*;

use crate::Assets;
use crate::client_persistence_storage::{
    CLIENT_SETTINGS_FILE, DESKTOP_SETTINGS_FILE, DataDir, DebouncedFile, read_json_object,
};
use crate::connection::{CommandError, ConnectionRuntime};

macro_rules! client_settings {
    (
        required { $($field:ident: $ty:ty,)* }
        optional { $($opt:ident: $opt_ty:ty,)* }
    ) => {
        /// `ClientSettings` with every schema default applied. Read it with
        /// [`ClientSettingsStore::get`]; change it with [`ClientSettingsStore::update`].
        #[derive(Clone, Debug, PartialEq)]
        pub struct ClientSettings {
            $(pub $field: $ty,)*
            $(pub $opt: Option<$opt_ty>,)*
        }

        impl ClientSettings {
            /// Each key from `primary`, else from `fallback`, which must set every required key.
            fn layered(primary: ClientSettingsSchema, fallback: &ClientSettingsSchema) -> Self {
                Self {
                    $($field: primary.$field.or_else(|| fallback.$field.clone())
                        .expect(concat!("the schema defaults set ", stringify!($field))),)*
                    $($opt: primary.$opt.or_else(|| fallback.$opt.clone()),)*
                }
            }

            /// The wire form, every key set.
            pub fn to_schema(&self) -> ClientSettingsSchema {
                ClientSettingsSchema {
                    $($field: Some(self.$field.clone()),)*
                    $($opt: self.$opt.clone(),)*
                }
            }
        }
    };
}

client_settings! {
    required {
        notification_mode: NotificationMode,
        in_app_notifications_enabled: bool,
        diff_color_scheme: DiffColorScheme,
        chat_width: ChatWidth,
        effort_display: EffortDisplay,
        load_balancing_enabled: bool,
        load_balancing_weights: LoadBalancingWeights,
        appearance_contrast: AppearanceContrast,
        panel_animation_duration_ms: PanelAnimationDurationMs,
        browser_default_viewport: PreviewViewportSetting,
        browser_default_zoom_factor: PreviewZoomFactor,
        browser_default_appearance: PreviewAppearancePreference,
        browser_recording_frame_rate: BrowserRecordingFrameRate,
        browser_recording_show_key_presses: bool,
        browser_recording_show_mouse_presses: bool,
        browser_link_target: BrowserLinkTarget,
        browser_search_engine: BrowserSearchEngine,
        browser_custom_search_url: BrowserCustomSearchUrl,
        browser_auto_show_floating_preview: bool,
        browser_profiles: Vec<BrowserProfile>,
        browser_default_profile_id: String,
        confirm_quit: ClientSettingsSchemaConfirmQuit,
        confirm_thread_archive: bool,
        confirm_terminal_close: bool,
        confirm_thread_delete: bool,
        confirm_thread_unpin: bool,
        diff_files_collapsed: bool,
        diff_ignore_whitespace: bool,
        diff_layout: DiffLayout,
        environment_identification_mode: EnvironmentIdentificationMode,
        glass_opacity: GlassOpacity,
        font_size_interface: InterfaceFontSize,
        font_size_prompt: PromptFontSize,
        font_size_code: CodeFontSize,
        font_size_terminal: TerminalFontSize,
        font_family_code: FontFamilyPreference,
        font_family_composer: FontFamilyPreference,
        font_family_sans: FontFamilyPreference,
        font_family_terminal: FontFamilyPreference,
        font_smoothing: bool,
        persist_composer_context_strip: bool,
        favorites: ModelFavorites,
        provider_model_preferences: std::collections::BTreeMap<String, ClientSettingsSchemaProviderModelPreferencesValue>,
        pull_request_merge_method_overrides: std::collections::BTreeMap<String, PullRequestMergeMethod>,
        pull_request_links_open_in_app: bool,
        composer_rich_text_enabled: bool,
        send_shortcut: ClientSettingsSchemaSendShortcut,
        follow_up_behavior: ClientSettingsSchemaFollowUpBehavior,
        proactive_panels_enabled: bool,
        show_skills_in_slash_menu: bool,
        boring_sidebar_enabled: bool,
        boring_sidebar_multiline_rows: bool,
        tool_call_visibility: ToolCallVisibility,
        screen_reader_announcements: ScreenReaderAnnouncements,
        screen_reader_window_title: bool,
        thread_details_usage_enabled: bool,
        thread_details_items: std::collections::BTreeMap<String, bool>,
        notification_inbox_enabled: bool,
        app_icon_badge_enabled: bool,
        sidebar_working_shelf_enabled: bool,
        sidebar_project_grouping_mode: SidebarProjectGroupingMode,
        sidebar_project_grouping_overrides: std::collections::BTreeMap<String, SidebarProjectGroupingMode>,
        sidebar_project_sort_order: SidebarProjectSortOrder,
        sidebar_thread_sort_order: SidebarThreadSortOrder,
        timestamp_format: TimestampFormat,
        voice_microphone: String,
        voice_microphones: Vec<VoiceMicrophoneEntry>,
        voice_shortcut_mode: VoiceShortcutMode,
        speech_language: SpeechLanguage,
        speech_translate_to_english: bool,
        speech_custom_words: SpeechCustomWords,
        speech_silence_filtering: bool,
        speech_streaming: bool,
        speech_remove_filler_words: bool,
        speech_post_processing_prompt: SpeechPostProcessingPromptSettings,
        speech_correction_word: SpeechCorrectionCue,
        language: String,
        snap_shot_enabled: bool,
        snap_shot_include_accessibility: bool,
        snap_shot_shortcut: SnapShotShortcut,
        snap_shot_play_sound: bool,
        snap_shot_sound: SnapShotSound,
        snap_shot_flash: bool,
        snap_shot_animations: bool,
        word_wrap: bool,
        math_rendering_enabled: bool,
    }
    optional {
        onboarding_completed_at: String,
    }
}

/// `DEFAULT_CLIENT_SETTINGS`, dumped from the TypeScript schema by node into
/// `assets/settings/client-settings-defaults.json`.
static DEFAULTS_DOCUMENT: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    let bytes = Assets::get("settings/client-settings-defaults.json").expect("client settings defaults are embedded");
    serde_json::from_slice(&bytes).expect("client settings defaults parse")
});

static DEFAULTS: LazyLock<ClientSettings> = LazyLock::new(|| {
    let (schema, rest) = decode_known(DEFAULTS_DOCUMENT.clone());
    assert!(rest.is_empty(), "every default decodes: {:?}", rest.keys().collect::<Vec<_>>());
    ClientSettings::layered(schema, &ClientSettingsSchema::default())
});

impl Default for ClientSettings {
    fn default() -> Self {
        DEFAULTS.clone()
    }
}

impl ClientSettings {
    /// The schema defaults, for per-setting reset.
    pub fn defaults() -> &'static Self {
        &DEFAULTS
    }

    /// Decodes a settings document, `{ ...DEFAULT_CLIENT_SETTINGS, ...persisted }`.
    /// Returns the settings and the keys this build could not decode, which
    /// are kept so a newer app's settings survive.
    pub fn from_document(document: Map<String, Value>) -> (Self, Map<String, Value>) {
        let (schema, rest) = decode_known(document);
        (Self::layered(schema, &DEFAULTS.to_schema()), rest)
    }

    /// The document written to disk: every setting, then the kept keys.
    pub fn to_document(&self, kept: &Map<String, Value>) -> Map<String, Value> {
        let Ok(Value::Object(mut document)) = serde_json::to_value(self.to_schema()) else {
            unreachable!("client settings encode as an object")
        };
        for (key, value) in kept {
            document.entry(key.clone()).or_insert_with(|| value.clone());
        }
        document
    }
}

/// Splits a document into what decodes as `ClientSettingsSchema`, key by
/// key so one bad value does not lose the rest, and everything else.
fn decode_known(document: Map<String, Value>) -> (ClientSettingsSchema, Map<String, Value>) {
    let mut known = Map::new();
    let mut rest = Map::new();
    for (key, value) in document {
        if !DEFAULTS_DOCUMENT.contains_key(&key) || value.is_null() {
            rest.insert(key, value);
            continue;
        }
        let single = Map::from_iter([(key.clone(), value.clone())]);
        if ClientSettingsSchema::deserialize(Value::Object(single)).is_ok() {
            known.insert(key, value);
        } else {
            rest.insert(key, value);
        }
    }
    let schema = ClientSettingsSchema::deserialize(Value::Object(known)).expect("keys decoded one by one");
    (schema, rest)
}

/// The Electron app wraps the document as `{ "settings": … }` in some versions.
fn unwrap_document(mut document: Map<String, Value>) -> Map<String, Value> {
    match document.remove("settings") {
        Some(Value::Object(inner)) => inner,
        Some(other) => {
            document.insert("settings".into(), other);
            document
        }
        None => document,
    }
}

/// This device's client settings. See the module docs.
pub struct ClientSettingsStore {
    settings: Arc<ClientSettings>,
    /// Keys from the file this build does not decode, written back unchanged.
    kept: Map<String, Value>,
    file: Option<DebouncedFile>,
}

impl Global for ClientSettingsStore {}

impl ClientSettingsStore {
    /// Loads `client-settings.json` from [`DataDir`], or starts from the
    /// defaults in memory when there is no data directory. An unreadable
    /// file is left alone: the app runs on defaults and does not write.
    pub fn init(cx: &mut App) {
        let path = DataDir::file(cx, CLIENT_SETTINGS_FILE);
        let (document, writable) = match path.as_deref().map(read_json_object) {
            Some(Ok(document)) => (document.map(unwrap_document).unwrap_or_default(), true),
            Some(Err(error)) => {
                tracing::warn!(%error, "client settings unreadable; using defaults without saving");
                (Map::new(), false)
            }
            None => (Map::new(), false),
        };
        let (settings, kept) = ClientSettings::from_document(document);
        let file = path.filter(|_| writable).map(DebouncedFile::new);
        let first = !cx.has_global::<Self>();
        cx.set_global(Self { settings: Arc::new(settings), kept, file });
        if !first {
            return;
        }
        cx.on_app_quit(|cx| {
            let this = cx.global_mut::<Self>();
            let document = this.settings.to_document(&this.kept);
            if let Some(file) = &mut this.file {
                file.flush(&document);
            }
            async {}
        })
        .detach();
    }

    /// The current settings.
    pub fn get(cx: &App) -> &Arc<ClientSettings> {
        &cx.global::<Self>().settings
    }

    /// Applies a change, notifies every observer, and schedules a write.
    /// A change that leaves the settings equal does nothing.
    pub fn update(cx: &mut App, change: impl FnOnce(&mut ClientSettings)) {
        let this = cx.global::<Self>();
        let mut next = ClientSettings::clone(&this.settings);
        change(&mut next);
        if next == *this.settings {
            return;
        }
        Self::replace(next, cx);
    }

    fn replace(next: ClientSettings, cx: &mut App) {
        let document = next.to_document(&cx.global::<Self>().kept);
        let mut file = cx.global_mut::<Self>().file.take();
        if let Some(file) = &mut file {
            file.schedule(document, cx);
        }
        let kept = std::mem::take(&mut cx.global_mut::<Self>().kept);
        cx.set_global(Self { settings: Arc::new(next), kept, file });
    }

    /// "Import settings from the desktop app": reads the Electron app's
    /// `client-settings.json` and applies every setting it holds over the
    /// current ones. Returns how many settings it applied.
    pub fn import_electron(path: &Path, cx: &mut App) -> anyhow::Result<usize> {
        let document = read_json_object(path)?
            .map(unwrap_document)
            .ok_or_else(|| anyhow::anyhow!("{} does not exist", path.display()))?;
        let (imported, _) = decode_known(document);
        let Ok(Value::Object(applied)) = serde_json::to_value(&imported) else { unreachable!() };
        let current = cx.global::<Self>().settings.to_schema();
        let next = ClientSettings::layered(imported, &current);
        Self::replace(next, cx);
        Ok(applied.len())
    }

    /// Where the Electron app keeps its client settings, when installed.
    pub fn electron_settings_path() -> Option<std::path::PathBuf> {
        t3code_client_runtime::platform::electron_import::ElectronPaths::detect()
            .map(|paths| paths.state_dir.join(CLIENT_SETTINGS_FILE))
    }
}

/// Re-renders the view when `select` of the client settings changes. Read
/// the value itself with [`ClientSettingsStore::get`] in render.
pub fn observe_client_settings<V: 'static, T: PartialEq + 'static>(
    cx: &mut Context<V>,
    select: impl Fn(&ClientSettings) -> T + 'static,
) -> Subscription {
    let mut last = select(ClientSettingsStore::get(cx));
    cx.observe_global::<ClientSettingsStore>(move |_, cx| {
        let next = select(ClientSettingsStore::get(cx));
        if next != last {
            last = next;
            cx.notify();
        }
    })
}

/// `cx.client_settings()` from any context that derefs to [`App`].
pub trait ActiveClientSettings {
    fn client_settings(&self) -> &ClientSettings;
}

impl ActiveClientSettings for App {
    fn client_settings(&self) -> &ClientSettings {
        ClientSettingsStore::get(self)
    }
}

/// `DesktopUpdateChannel`: which desktop release stream to follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Latest,
    Nightly,
}

/// The `DesktopSettings` keys that mean something without a local server.
/// Window bounds live in the shell's own `window-state.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopSettings {
    pub update_channel: UpdateChannel,
    pub update_channel_configured_by_user: bool,
}

/// `desktop-settings.json`. Keys it does not model are kept.
pub struct DesktopSettingsStore {
    settings: DesktopSettings,
    kept: Map<String, Value>,
    file: Option<DebouncedFile>,
}

impl Global for DesktopSettingsStore {}

impl DesktopSettingsStore {
    /// Loads from [`DataDir`], or defaults in memory.
    pub fn init(cx: &mut App) {
        let path = DataDir::file(cx, DESKTOP_SETTINGS_FILE);
        let (document, writable) = match path.as_deref().map(read_json_object) {
            Some(Ok(document)) => (document.unwrap_or_default(), true),
            Some(Err(error)) => {
                tracing::warn!(%error, "desktop settings unreadable; using defaults without saving");
                (Map::new(), false)
            }
            None => (Map::new(), false),
        };
        let settings = DesktopSettings::deserialize(Value::Object(document.clone())).unwrap_or_default();
        let kept = document
            .into_iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "updateChannel" | "updateChannelConfiguredByUser"
                )
            })
            .collect();
        let first = !cx.has_global::<Self>();
        cx.set_global(Self { settings, kept, file: path.filter(|_| writable).map(DebouncedFile::new) });
        if !first {
            return;
        }
        cx.on_app_quit(|cx| {
            let this = cx.global_mut::<Self>();
            let document = this.document();
            if let Some(file) = &mut this.file {
                file.flush(&document);
            }
            async {}
        })
        .detach();
    }

    pub fn get(cx: &App) -> &DesktopSettings {
        &cx.global::<Self>().settings
    }

    fn document(&self) -> Map<String, Value> {
        let Ok(Value::Object(mut document)) = serde_json::to_value(&self.settings) else { unreachable!() };
        for (key, value) in &self.kept {
            document.entry(key.clone()).or_insert_with(|| value.clone());
        }
        document
    }

    /// Applies a change and schedules a write.
    pub fn update(cx: &mut App, change: impl FnOnce(&mut DesktopSettings)) {
        let mut settings = cx.global::<Self>().settings.clone();
        change(&mut settings);
        if settings == cx.global::<Self>().settings {
            return;
        }
        let mut this = cx.remove_global::<Self>();
        this.settings = settings;
        let document = this.document();
        if let Some(file) = &mut this.file {
            file.schedule(document, cx);
        }
        cx.set_global(this);
    }
}

/// One environment's server config as last received, with local edits
/// applied while their `server.updateSettings` is in flight.
#[derive(Clone)]
pub struct EnvironmentConfig {
    pub config: Arc<ServerConfig>,
    /// Seeded by a scene or check: updates apply locally, no server call.
    pub fixture: bool,
    /// Bumped by each local update, so a failed save only reverts its own edit.
    revision: u64,
}

/// What the config follower hands from tokio to GPUI.
enum ConfigUpdate {
    Event(EnvironmentId, Box<ServerConfigStreamEvent>),
    Ended(EnvironmentId),
}

/// Every environment's server config. See the module docs.
#[derive(Default)]
pub struct ServerSettingsStore {
    environments: HashMap<EnvironmentId, EnvironmentConfig>,
    /// Environments with a running follower. A follower ends by itself when
    /// its supervisor goes away and its session watch closes.
    followers: std::collections::HashSet<EnvironmentId>,
    updates: Option<mpsc::UnboundedSender<ConfigUpdate>>,
}

impl Global for ServerSettingsStore {}

impl ServerSettingsStore {
    /// Installs the store and, when the connection runtime runs, follows
    /// each environment's config stream as it connects.
    pub fn init(cx: &mut App) {
        if cx.try_global::<Self>().is_some_and(|this| this.updates.is_some()) {
            return;
        }
        let fixtures = cx.try_global::<Self>().map(|this| this.environments.clone()).unwrap_or_default();
        cx.set_global(Self { environments: fixtures, ..Self::default() });
        if !ConnectionRuntime::is_running(cx) {
            return;
        }
        let (sender, mut receiver) = mpsc::unbounded();
        cx.global_mut::<Self>().updates = Some(sender);
        cx.spawn(async move |cx| {
            while let Some(update) = receiver.next().await {
                cx.update(|cx| Self::apply(update, cx));
            }
        })
        .detach();
        let state = ConnectionRuntime::state(cx);
        cx.observe(&state, |_, cx| Self::sync_followers(cx)).detach();
        Self::sync_followers(cx);
    }

    /// Starts a follower for each environment with a supervisor.
    fn sync_followers(cx: &mut App) {
        let Some(sender) = cx.global::<Self>().updates.clone() else { return };
        let ids: Vec<EnvironmentId> =
            ConnectionRuntime::state(cx).read(cx).environments.iter().map(|env| env.id().clone()).collect();
        for id in ids {
            if cx.global::<Self>().followers.contains(&id) {
                continue;
            }
            let runtime = ConnectionRuntime::global(cx);
            let Some(handle) = runtime.supervisor(&id) else { continue };
            let follower = follow_config(id.clone(), handle.session.clone(), sender.clone());
            runtime.spawn(async move { follower.await; Ok(()) }, cx).detach();
            cx.global_mut::<Self>().followers.insert(id);
        }
    }

    fn apply(update: ConfigUpdate, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        match update {
            ConfigUpdate::Ended(id) => {
                this.followers.remove(&id);
            }
            ConfigUpdate::Event(id, event) => match *event {
                ServerConfigStreamEvent::Snapshot(snapshot) => {
                    let revision = this.environments.get(&id).map_or(0, |env| env.revision);
                    this.environments
                        .insert(id, EnvironmentConfig { config: Arc::new(snapshot.config), fixture: false, revision });
                }
                ServerConfigStreamEvent::SettingsUpdated(event) => {
                    if let Some(env) = this.environments.get_mut(&id) {
                        Arc::make_mut(&mut env.config).settings = event.payload.settings;
                    }
                }
                ServerConfigStreamEvent::KeybindingsUpdated(event) => {
                    if let Some(env) = this.environments.get_mut(&id) {
                        let config = Arc::make_mut(&mut env.config);
                        config.keybindings = event.payload.keybindings.into_iter().map(keybinding_item).collect();
                        config.issues = event.payload.issues;
                    }
                }
                ServerConfigStreamEvent::EnvironmentThemesUpdated(event) => {
                    if let Some(env) = this.environments.get_mut(&id) {
                        Arc::make_mut(&mut env.config).environment_themes = Some(event.payload.themes);
                    }
                }
                _ => return,
            },
        }
        cx.refresh_windows();
    }

    /// Seeds an environment's config with no server behind it, for scenes
    /// and checks. Updates to it apply locally.
    pub fn insert_fixture(id: EnvironmentId, config: ServerConfig, cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
        cx.global_mut::<Self>()
            .environments
            .insert(id, EnvironmentConfig { config: Arc::new(config), fixture: true, revision: 0 });
        cx.refresh_windows();
    }

    /// Environments with a config, in no particular order.
    pub fn environment_ids(cx: &App) -> Vec<EnvironmentId> {
        cx.try_global::<Self>().map(|this| this.environments.keys().cloned().collect()).unwrap_or_default()
    }

    /// An environment's config, once its first snapshot arrived.
    pub fn config(cx: &App, id: &EnvironmentId) -> Option<Arc<ServerConfig>> {
        cx.try_global::<Self>()?.environments.get(id).map(|env| env.config.clone())
    }

    /// Applies `patch` to the environment's settings at once, then saves it
    /// with `server.updateSettings`. The server's answer replaces the local
    /// copy; a failure reverts this edit unless a newer one superseded it.
    pub fn update(id: EnvironmentId, patch: ClientServerSettingsPatch, cx: &mut App) -> Task<Result<(), CommandError>> {
        let Some(env) = cx.try_global::<Self>().and_then(|this| this.environments.get(&id)).cloned() else {
            return Task::ready(Err(CommandError("This environment is not connected.".into())));
        };
        let previous = env.config.settings.clone();
        let optimistic = apply_patch(&previous, &patch);
        let revision = env.revision + 1;
        if let Some(env) = cx.global_mut::<Self>().environments.get_mut(&id) {
            env.revision = revision;
            if let Some(settings) = optimistic {
                Arc::make_mut(&mut env.config).settings = settings;
            }
        }
        cx.refresh_windows();
        if env.fixture {
            return Task::ready(Ok(()));
        }
        let call = Self::call::<ServerUpdateSettings>(
            &id,
            ServerUpdateSettingsPayload { patch, provider_instance_mutation: None },
            cx,
        );
        cx.spawn(async move |cx| {
            let result = call.await;
            cx.update(|cx| {
                let this = cx.global_mut::<Self>();
                if let Some(env) = this.environments.get_mut(&id) {
                    match &result {
                        Ok(settings) => Arc::make_mut(&mut env.config).settings = settings.clone(),
                        Err(_) if env.revision == revision => Arc::make_mut(&mut env.config).settings = previous,
                        Err(_) => {}
                    }
                }
                cx.refresh_windows();
            });
            result.map(|_| ())
        })
    }

    /// Calls an RPC method on an environment's live session.
    pub fn call<M>(id: &EnvironmentId, payload: M::Payload, cx: &App) -> Task<Result<M::Success, CommandError>>
    where
        M: RpcMethod + 'static,
        M::Payload: Send + 'static,
        M::Success: Send + 'static,
        M::Error: std::fmt::Debug + Send,
    {
        let Some(runtime) = cx.try_global::<ConnectionRuntime>() else {
            return Task::ready(Err(CommandError("The app is not connected.".into())));
        };
        let Some(session) = runtime.supervisor(id).and_then(|handle| handle.session.borrow().clone()) else {
            return Task::ready(Err(CommandError("This environment is not connected.".into())));
        };
        runtime.spawn(async move { session.client.call::<M>(payload).await.map_err(CommandError::from) }, cx)
    }
}

fn keybinding_item(item: ServerConfigKeybindingsUpdatedPayloadKeybindingsItem) -> ServerConfigKeybindingsItem {
    ServerConfigKeybindingsItem { command: item.command, shortcut: item.shortcut, when_ast: item.when_ast }
}

/// Follows one environment's sessions: decodes each session's initial
/// snapshot and later config events and hands them to GPUI.
async fn follow_config(
    id: EnvironmentId,
    mut sessions: tokio::sync::watch::Receiver<Option<Arc<RpcSession>>>,
    updates: mpsc::UnboundedSender<ConfigUpdate>,
) {
    loop {
        let session = sessions.borrow_and_update().clone();
        if let Some(session) = session {
            let mut events = session.config_events();
            let send = |raw: &serde_json::value::RawValue| {
                match serde_json::from_str::<ServerConfigStreamEvent>(raw.get()) {
                    Ok(event) => updates.unbounded_send(ConfigUpdate::Event(id.clone(), Box::new(event))).is_ok(),
                    Err(error) => {
                        tracing::warn!(%error, "could not decode a server config event");
                        true
                    }
                }
            };
            if !send(&session.initial_config) {
                return;
            }
            loop {
                tokio::select! {
                    changed = sessions.changed() => {
                        if changed.is_err() {
                            _ = updates.unbounded_send(ConfigUpdate::Ended(id));
                            return;
                        }
                        break;
                    }
                    event = events.recv() => match event {
                        Ok(raw) => if !send(&raw) { return },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            if sessions.changed().await.is_err() {
                                _ = updates.unbounded_send(ConfigUpdate::Ended(id));
                                return;
                            }
                            break;
                        }
                    },
                }
            }
        } else if sessions.changed().await.is_err() {
            _ = updates.unbounded_send(ConfigUpdate::Ended(id));
            return;
        }
    }
}

/// Patch keys that are operations, not settings.
const PATCH_OPERATIONS: [&str; 4] = ["setModelFavorites", "setModelsHidden", "setProviderModelOrder", "migrateModelPreferences"];

/// The settings with `patch` merged in (objects merge, other values
/// replace), or `None` when the result does not decode.
fn apply_patch(settings: &ServerSettings, patch: &ClientServerSettingsPatch) -> Option<ServerSettings> {
    let Ok(Value::Object(mut current)) = serde_json::to_value(settings) else { return None };
    let Ok(Value::Object(patch)) = serde_json::to_value(patch) else { return None };
    for (key, value) in patch {
        if PATCH_OPERATIONS.contains(&key.as_str()) {
            continue;
        }
        match current.get_mut(&key) {
            Some(existing) => merge(existing, value),
            None => {
                current.insert(key, value);
            }
        }
    }
    ServerSettings::deserialize(Value::Object(current)).ok()
}

fn merge(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                match target.get_mut(&key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (target, patch) => *target = patch,
    }
}
