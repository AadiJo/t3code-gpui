//! The composer on live data (`ChatComposer` wiring in `ChatView.tsx`, `composerDraftStore.ts`):
//! its state from the thread, its run and the server config; its events as commands; its menu
//! over workspace file search, provider skills and the shell's threads; its drafts in the
//! runtime's persisted document.
//!
//! Not wired yet: attachment uploads (they need the signed HTTP upload), favourite toggles
//! (a server settings patch), `/clear` and `/side`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, Entity, SharedString, Task, WeakEntity};
use t3code_client_runtime::operations::{CommandError, DeliveryMode, EnvironmentCommands, SendMessage};
use t3code_client_runtime::state::client_store::ClientStore;
use t3code_client_runtime::state::drafts::{Drafts, ThreadDraft, scoped_thread_key};
use t3code_client_runtime::state::models::ThreadRow;
use t3code_client_runtime::state::EnvironmentStore;
use t3code_contracts::rpc::methods::ProjectsSearchEntries;
use t3code_contracts::{
    MessageId as WireMessageId, ProjectEntryKind, ProjectSearchEntriesInput, ProviderApprovalDecision, ProviderUserInputAnswers,
    RuntimeMode as WireRuntimeMode, RuntimeRequestId, ServerConfig,
};

use super::shell::{runtime_ids, thread_key};
use super::{LiveEnvironment, StateBridge};
use crate::components::chat::composer::{BannerTone, ChatComposer, ComposerBanner, ComposerContextStrip, ComposerEvent, ComposerState, FollowUpBehavior};
use crate::components::chat::composer_command_menu::{ComposerCommandAction, ComposerCommandIcon, ComposerCommandItem, ComposerMenuSource};
use crate::components::icons::IconName;
use crate::components::chat::composer_permissions::RuntimeMode;
use crate::components::chat::composer_prompt_history::PromptHistoryEntry;
use crate::components::chat::timeline_entries::{EntryKind, MessageRole, TimelineInput};
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::composer_draft_store::{ComposerAttachment, ComposerAttachmentKind, ComposerDraft, ComposerDraftKey, ComposerDraftStore};
use crate::composer_editor_mentions::serialize_file_link;
use crate::composer_logic::{ComposerTrigger, ComposerTriggerKind, SendShortcut};
use crate::i18n::t;
use crate::model_selection::{ModelCatalog, ModelSelection};
use crate::provider_models_live::{from_wire, model_catalog, to_wire};
use crate::thread_sync::LiveThread;

/// At most this many workspace entries per search.
const FILE_SEARCH_LIMIT: u64 = 40;

fn runtime_mode(mode: WireRuntimeMode) -> RuntimeMode {
    match mode {
        WireRuntimeMode::ApprovalRequired => RuntimeMode::ApprovalRequired,
        WireRuntimeMode::AutoAcceptEdits => RuntimeMode::AutoAcceptEdits,
        WireRuntimeMode::Auto => RuntimeMode::Auto,
        WireRuntimeMode::FullAccess => RuntimeMode::FullAccess,
    }
}

pub fn wire_runtime_mode(mode: RuntimeMode) -> WireRuntimeMode {
    match mode {
        RuntimeMode::ApprovalRequired => WireRuntimeMode::ApprovalRequired,
        RuntimeMode::AutoAcceptEdits => WireRuntimeMode::AutoAcceptEdits,
        RuntimeMode::Auto => WireRuntimeMode::Auto,
        RuntimeMode::FullAccess => WireRuntimeMode::FullAccess,
    }
}

/// The picker catalog of an environment, rebuilt only when its config `Arc` or the client
/// settings' legacy favourites change.
#[derive(Default)]
pub struct CatalogCache {
    source: Option<Arc<ServerConfig>>,
    legacy: Option<t3code_contracts::ModelFavorites>,
    catalog: Arc<ModelCatalog>,
}

impl CatalogCache {
    /// `legacy` is the client settings' favourites, shown while the server has none.
    pub fn get(&mut self, config: Option<&Arc<ServerConfig>>, legacy: &t3code_contracts::ModelFavorites) -> Arc<ModelCatalog> {
        let same = match (&self.source, config) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same || self.legacy.as_ref() != Some(legacy) {
            let mut catalog = config.map(|config| model_catalog(config)).unwrap_or_default();
            if config.is_none_or(|c| c.settings.favorites.is_none()) {
                catalog.favorites = legacy
                    .iter()
                    .map(|f| crate::model_selection::provider_model_key(&crate::model_selection::ProviderInstanceId(SharedString::from(f.provider.to_string())), &f.model))
                    .collect();
            }
            self.catalog = Arc::new(catalog);
            self.source = config.cloned();
            self.legacy = Some(legacy.clone());
        }
        Arc::clone(&self.catalog)
    }
}

/// The draft key the runtime stores a thread's draft under.
pub fn draft_key(key: &ThreadKey) -> ComposerDraftKey {
    let (environment_id, thread_id) = runtime_ids(key);
    ComposerDraftKey(SharedString::from(scoped_thread_key(&environment_id, &thread_id)))
}

/// The user's earlier prompts in this thread, newest last, for ↑ recall.
fn prompt_history(input: &TimelineInput) -> Arc<Vec<PromptHistoryEntry>> {
    Arc::new(
        input
            .entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                EntryKind::Message(message) if message.role == MessageRole::User && !message.text.trim().is_empty() => {
                    Some(PromptHistoryEntry { id: message.id.0.clone(), prompt: message.text.to_string() })
                }
                _ => None,
            })
            .collect(),
    )
}

/// The first instance's first model: what a thread with no saved choice starts on.
pub fn default_selection(catalog: &ModelCatalog) -> ModelSelection {
    let instance = catalog.instances.first();
    ModelSelection {
        instance_id: instance.map(|i| i.instance_id.clone()).unwrap_or_else(|| crate::model_selection::ProviderInstanceId(SharedString::default())),
        model: instance.and_then(|i| catalog.models.get(&i.instance_id)).and_then(|m| m.first()).map(|m| m.slug.clone()).unwrap_or_default(),
        effort: None,
        speed: None,
        ultracode: false,
    }
}

/// The composer's state for a thread: the thread's model and mode, overridden by the draft's
/// unsent picks (`draftModelSelectionApplies`), and the `/clear` banner while armed.
pub fn composer_state(key: &ThreadKey, row: Option<&ThreadRow>, thread: &LiveThread, environment: &LiveEnvironment, catalog: Arc<ModelCatalog>, draft: Option<&ThreadDraft>, fresh_armed: bool) -> ComposerState {
    let input = thread.input();
    let projection_thread = thread.projection().map(|p| Arc::clone(&p.thread));
    let wire_selection = draft
        .filter(|d| d.model_selection_explicit)
        .and_then(|d| d.active_provider.as_ref().and_then(|p| d.model_selection_by_provider.get(p)).cloned())
        .or_else(|| row.map(|row| row.source.model_selection.clone()))
        .or_else(|| projection_thread.as_ref().map(|t| t.model_selection.clone()));
    let selection = wire_selection.as_ref().map(from_wire).unwrap_or_else(|| default_selection(&catalog));
    let mode = draft
        .and_then(|d| d.runtime_mode)
        .or_else(|| row.map(|row| row.source.runtime_mode))
        .or_else(|| projection_thread.as_ref().map(|t| t.runtime_mode));
    let default_mode = environment.config.as_ref().and_then(|c| c.settings.default_runtime_mode).unwrap_or(WireRuntimeMode::FullAccess);
    let connected = environment.store.session().is_some();
    let banners = if fresh_armed && fresh_session_available(input, environment) {
        vec![ComposerBanner {
            id: FRESH_SESSION_BANNER.into(),
            tone: BannerTone::Neutral,
            icon: IconName::Eraser,
            title: t!("Your next message starts a fresh session"),
            description: Some(t!("The agent won't see earlier messages in this thread.")),
            action: Some(t!("Keep the current session")),
        }]
    } else {
        Vec::new()
    };
    ComposerState {
        draft_key: draft_key(key),
        is_draft_thread: false,
        catalog,
        selection,
        runtime_mode: runtime_mode(mode.unwrap_or(default_mode)),
        default_runtime_mode: runtime_mode(default_mode),
        running: input.is_working,
        can_interrupt: input.is_working,
        follow_up: FollowUpBehavior::Steer,
        send_shortcut: SendShortcut::Enter,
        rich_text: true,
        send_disabled_reason: (!connected).then(|| t!("Not connected")),
        placeholder: None,
        project_required: false,
        is_send_busy: false,
        history: prompt_history(input),
        banners,
        context_strip: row.map(|row| ComposerContextStrip {
            environment: environment
                .config
                .as_ref()
                .map(|c| SharedString::from(c.environment.label.clone()))
                .unwrap_or_else(|| SharedString::from(row.environment_id.to_string())),
            workspace: if row.source.worktree_path.is_some() { t!("Worktree") } else { t!("Local checkout") },
            branch: row.source.branch.clone().map(SharedString::from).unwrap_or_default(),
        }),
        voice_available: false,
        validation: None,
        file_attachment_limit: super::attachments::file_limit(environment.config.as_deref()),
    }
}

/// The id of the banner `/clear` shows; its action disarms.
pub const FRESH_SESSION_BANNER: &str = "fresh-session";

/// `/clear` is honoured once the thread has messages and the server starts fresh contexts.
fn fresh_session_available(input: &TimelineInput, environment: &LiveEnvironment) -> bool {
    let has_messages = input.entries.iter().any(|e| matches!(e.kind, EntryKind::Message(_)));
    has_messages && environment.config.as_ref().is_some_and(|c| c.environment.capabilities.fresh_context == Some(true))
}

/// Threads whose next message starts a fresh session (`freshSessionArms.ts`).
#[derive(Default)]
pub struct FreshSessionArms(std::collections::HashSet<ThreadKey>);

impl gpui_kit::Global for FreshSessionArms {}

impl FreshSessionArms {
    pub fn armed(key: &ThreadKey, cx: &App) -> bool {
        cx.try_global::<Self>().is_some_and(|arms| arms.0.contains(key))
    }

    pub fn set(key: &ThreadKey, armed: bool, cx: &mut App) {
        let arms = &mut cx.default_global::<Self>().0;
        if armed {
            arms.insert(key.clone());
        } else {
            arms.remove(key);
        }
    }
}

fn toast_error(title: SharedString, error: impl ToString, cx: &mut App) {
    Toaster::push(ToastData::new(ToastKind::Error, title).description(SharedString::from(error.to_string())), cx);
}

/// The title a send proposes (`deriveThreadTitleSeed`): the prose with image chips dropped and
/// other chips as their labels, else "Attached image(s)", else the first file's name.
pub fn title_seed(text: &str, attachments: &[ComposerAttachment]) -> String {
    use crate::composer_editor_mentions::{ChipKind, ComposerPromptSegment, ContextKind, split_prompt_into_segments};
    let mut prose = String::new();
    let mut images = 0;
    for segment in split_prompt_into_segments(text) {
        match segment {
            ComposerPromptSegment::Text(text) => prose.push_str(text),
            ComposerPromptSegment::Chip(token) => match token.kind {
                ChipKind::Context { kind: ContextKind::Image, .. } => images += 1,
                ChipKind::Model { model, .. } => prose.push_str(&model),
                ChipKind::Mention { .. } | ChipKind::Skill { .. } => prose.push_str(&text_of(text, &token.range)),
                _ => prose.push_str(&token.label),
            },
        }
    }
    let prose = prose.split_whitespace().collect::<Vec<_>>().join(" ");
    let attached = |count: usize| if count == 1 { t!("Attached image").to_string() } else { t!("Attached images").to_string() };
    let seed = if !prose.is_empty() {
        prose
    } else if images > 0 {
        attached(images)
    } else {
        let tiles = attachments.iter().filter(|a| matches!(a.kind, ComposerAttachmentKind::Image { .. })).count();
        match (tiles, attachments.first()) {
            (n, _) if n > 0 => attached(n),
            (_, Some(first)) if !first.name.trim().is_empty() => t!("File: {name}", name = first.name.split_whitespace().collect::<Vec<_>>().join(" ")).to_string(),
            _ => t!("New thread").to_string(),
        }
    };
    truncate_title(&seed)
}

fn text_of(text: &str, range: &std::ops::Range<usize>) -> String {
    text.get(range.clone()).unwrap_or_default().to_owned()
}

/// `truncate(text, 50)` from `shared/String.ts`.
fn truncate_title(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 50 { trimmed.to_owned() } else { format!("{}...", trimmed.chars().take(50).collect::<String>()) }
}

/// What a composer event left for the app to do.
pub enum EventOutcome {
    Done,
    /// A side chat started; show it.
    OpenThread(ThreadKey),
}

/// Handles a composer event for the thread it edits.
pub fn handle_event(event: &ComposerEvent, thread: &Entity<LiveThread>, composer: &WeakEntity<ChatComposer>, cx: &mut App) -> EventOutcome {
    let key = thread.read(cx).key().clone();
    let (environment_id, thread_id) = runtime_ids(&key);
    let Some(environment) = StateBridge::environment(&environment_id, cx) else { return EventOutcome::Done };
    let (commands, config) = {
        let env = environment.read(cx);
        (env.commands.clone(), env.config.clone())
    };
    let drafts = StateBridge::drafts(cx);
    let draft_key_text = draft_key(&key).0.to_string();
    match event {
        ComposerEvent::Submit { prompt, attachments, model, dispatch, .. } => {
            let mode = match dispatch {
                Some(FollowUpBehavior::Queue) => DeliveryMode::Queue,
                Some(FollowUpBehavior::Steer) => DeliveryMode::Steer,
                None => DeliveryMode::Auto,
            };
            let fresh = FreshSessionArms::armed(&key, cx) && fresh_session_available(thread.read(cx).input(), environment.read(cx));
            let draft_mode = drafts.current().drafts_by_thread_key.get(&draft_key_text).and_then(|d| d.runtime_mode);
            let thread_mode = StateBridge::shell(cx).read(cx).row(&key).map(|row| row.source.runtime_mode);
            let set_mode = draft_mode.filter(|mode| Some(*mode) != thread_mode);
            send_turn(SendInput { key: key.clone(), thread: thread.clone(), prompt, attachments, model, mode, fresh, set_mode }, commands, config, cx);
            drafts.update(|d| {
                if let Some(draft) = d.drafts_by_thread_key.get_mut(&draft_key_text) {
                    draft.runtime_mode = None;
                    draft.model_selection_explicit = false;
                }
            });
        }
        ComposerEvent::ClearSession => {
            if fresh_session_available(thread.read(cx).input(), environment.read(cx)) {
                FreshSessionArms::set(&key, true, cx);
                _ = composer.update(cx, |_, cx| cx.notify());
            } else {
                // Without fresh contexts `/clear` is an ordinary message, as on the web.
                let model = composer.upgrade().map(|c| c.read(cx).state().selection.clone());
                if let Some(model) = model {
                    let input = SendInput { key, thread: thread.clone(), prompt: "/clear", attachments: &[], model: &model, mode: DeliveryMode::Auto, fresh: false, set_mode: None };
                    send_turn(input, commands, config, cx);
                }
            }
        }
        ComposerEvent::BannerAction(id) if id.as_ref() == FRESH_SESSION_BANNER => FreshSessionArms::set(&key, false, cx),
        ComposerEvent::SideChat { prompt } => {
            let model = composer.upgrade().map(|c| c.read(cx).state().selection.clone());
            let runtime = composer.upgrade().map(|c| wire_runtime_mode(c.read(cx).state().runtime_mode));
            return start_side_chat(&key, prompt.clone(), model, runtime, cx);
        }
        ComposerEvent::Stop => {
            StateBridge::spawn(async move { commands.interrupt(&thread_id, None, None).await }, cx).detach();
        }
        ComposerEvent::ModelChanged(selection) => {
            let catalog = config.as_ref().map(|c| model_catalog(c)).unwrap_or_default();
            let wire = to_wire(selection, &catalog, config.as_deref());
            let instance = wire.instance_id.clone();
            drafts.update(|d| {
                let draft = d.drafts_by_thread_key.entry(draft_key_text.clone()).or_default();
                draft.model_selection_by_provider.insert(instance.clone(), wire.clone());
                draft.active_provider = Some(instance.clone());
                draft.model_selection_explicit = true;
                d.sticky_model_selection_by_provider.insert(instance.clone(), wire.clone());
                d.sticky_active_provider = Some(instance.clone());
            });
            // A pick on the thread's own provider applies now (`useApplyThreadModelPick`); a
            // provider switch waits for the next message, which carries the selection.
            let current = StateBridge::shell(cx).read(cx).row(&key).map(|row| row.source.model_selection.instance_id.clone());
            if current.as_deref() == Some(instance.as_str()) {
                let drafts = drafts.clone();
                let task = StateBridge::spawn(async move { commands.set_model_selection(&thread_id, wire).await }, cx);
                cx.spawn(async move |cx| match task.await {
                    Some(Ok(_)) => drafts.update(|d| {
                        if let Some(draft) = d.drafts_by_thread_key.get_mut(&draft_key_text) {
                            draft.model_selection_explicit = false;
                        }
                    }),
                    Some(Err(error)) => cx.update(|cx| toast_error(t!("Could not apply model selection"), error, cx)),
                    None => {}
                })
                .detach();
            }
        }
        ComposerEvent::RuntimeModeChanged(mode) => {
            // Kept in the draft and applied before the next message (`persistThreadSettingsForNextTurn`).
            let mode = wire_runtime_mode(*mode);
            drafts.update(|d| d.drafts_by_thread_key.entry(draft_key_text).or_default().runtime_mode = Some(mode));
        }
        ComposerEvent::AttachmentsAdded(attachments) => super::attachments::start_uploads(&environment_id, attachments, composer, cx),
        ComposerEvent::AttachmentRemoved(context_id) => super::attachments::AttachmentUploads::release(context_id, cx),
        ComposerEvent::FavoriteToggled { instance_id, model } => super::favorites::toggle(&environment_id, instance_id, model, cx),
        ComposerEvent::AttachRequested => pick_files(composer, cx),
        _ => {}
    }
    EventOutcome::Done
}

/// Opens the file picker and hands the picks to the composer like a drop (the attach button).
pub fn pick_files(composer: &WeakEntity<ChatComposer>, cx: &mut App) {
    let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
    let composer = composer.clone();
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = paths.await {
            cx.update(|cx| {
                _ = composer.update(cx, |composer, cx| composer.attach_paths(paths, cx));
            });
        }
    })
    .detach();
}

struct SendInput<'a> {
    key: ThreadKey,
    thread: Entity<LiveThread>,
    prompt: &'a str,
    attachments: &'a [ComposerAttachment],
    model: &'a ModelSelection,
    mode: DeliveryMode,
    fresh: bool,
    set_mode: Option<WireRuntimeMode>,
}

/// Sends one turn as `useComposerSend` does: the draft's mode first when it changed, then the
/// message with its attachments, context records, model selection and title seed.
fn send_turn(input: SendInput<'_>, commands: EnvironmentCommands, config: Option<Arc<ServerConfig>>, cx: &mut App) {
    let (environment_id, thread_id) = runtime_ids(&input.key);
    let Some(turn) = super::attachments::turn_attachments(&environment_id, input.attachments) else {
        toast_error(t!("Failed to send message."), t!("Retry or remove failed uploads before sending."), cx);
        return;
    };
    let text = input.prompt.trim();
    let text = if text.is_empty() && !input.attachments.is_empty() { super::attachments::ATTACHMENT_ONLY_PROMPT } else { text };
    let mut message = SendMessage::text(thread_id.clone(), text);
    message.mode = input.mode;
    message.attachments = turn.attachments;
    message.context = with_thread_records(turn.context, input.prompt, &environment_id);
    message.title_seed = Some(title_seed(input.prompt.trim(), input.attachments));
    message.fresh_context = input.fresh;
    let catalog = config.as_ref().map(|c| model_catalog(c)).unwrap_or_default();
    message.model_selection = Some(to_wire(input.model, &catalog, config.as_deref()));
    let message_id: WireMessageId = message.message_id.clone();
    // A queued message shows in the queue, not as a sent bubble.
    if message.mode != DeliveryMode::Queue {
        let (text, attachments) = (message.text.clone(), message.attachments.clone());
        input.thread.update(cx, |thread, cx| thread.push_optimistic(message_id.clone(), text, attachments, cx));
    }
    let sent: Vec<SharedString> = input.attachments.iter().map(|a| a.context_id.clone()).collect();
    let set_mode = input.set_mode;
    let task = StateBridge::spawn(
        async move {
            if let Some(mode) = set_mode {
                commands.set_runtime_mode(&thread_id, mode).await?;
            }
            commands.send_message(message).await
        },
        cx,
    );
    let (thread, key, fresh) = (input.thread.downgrade(), input.key, input.fresh);
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Some(Ok(_)) => {
                super::attachments::AttachmentUploads::forget_sent(sent);
                if fresh {
                    FreshSessionArms::set(&key, false, cx);
                }
            }
            Some(Err(error)) => {
                _ = thread.update(cx, |thread, cx| thread.remove_optimistic(&message_id, cx));
                toast_error(t!("Failed to send message."), error, cx);
            }
            None => {}
        });
    })
    .detach();
}

/// Adds a `threadContextRecord` for each thread chip in `prompt`, before the attachment records
/// (`buildMessageContext` order).
pub fn with_thread_records(
    context: Option<t3code_contracts::OrchestrationMessageContext>,
    prompt: &str,
    environment_id: &t3code_contracts::EnvironmentId,
) -> Option<t3code_contracts::OrchestrationMessageContext> {
    use crate::composer_editor_mentions::{ChipKind, ComposerPromptSegment, ContextKind, split_prompt_into_segments};
    let mut seen = std::collections::HashSet::new();
    let threads: Vec<serde_json::Value> = split_prompt_into_segments(prompt)
        .into_iter()
        .filter_map(|segment| match segment {
            ComposerPromptSegment::Chip(token) => match token.kind {
                ChipKind::Context { kind: ContextKind::Thread, context_id } => Some((context_id, token.label)),
                _ => None,
            },
            _ => None,
        })
        .filter(|(id, _)| seen.insert(id.clone()))
        .filter_map(|(context_id, label)| {
            let thread_id = context_id.strip_prefix("thread_")?.to_owned();
            Some(serde_json::json!({
                "version": 1,
                "kind": "thread",
                "contextId": context_id.as_ref(),
                "label": label.as_ref(),
                "environmentId": environment_id.as_str(),
                "threadId": thread_id,
                "title": label.as_ref(),
            }))
        })
        .collect();
    if threads.is_empty() {
        return context;
    }
    let mut context = context.unwrap_or(t3code_contracts::OrchestrationMessageContext { version: 1.0, guest: None, records: Vec::new() });
    context.records.splice(0..0, threads);
    Some(context)
}

/// Starts a side chat (`useSideChat`): forks the thread's latest stable point, applies the
/// composer's mode, sends `prompt` there when there is one, and opens it once it arrives.
fn start_side_chat(key: &ThreadKey, prompt: String, model: Option<ModelSelection>, runtime: Option<WireRuntimeMode>, cx: &mut App) -> EventOutcome {
    let (environment_id, source) = runtime_ids(key);
    let Some(environment) = StateBridge::environment(&environment_id, cx) else { return EventOutcome::Done };
    let (commands, config) = {
        let env = environment.read(cx);
        (env.commands.clone(), env.config.clone())
    };
    let target = t3code_contracts::ThreadId::new(uuid::Uuid::new_v4().to_string());
    let catalog = config.as_ref().map(|c| model_catalog(c)).unwrap_or_default();
    let wire_model = model.as_ref().map(|m| to_wire(m, &catalog, config.as_deref()));
    let seed = (!prompt.is_empty()).then(|| title_seed(&prompt, &[]));
    let shell = environment.read(cx).store.clone();
    let task = StateBridge::spawn(
        {
            let target = target.clone();
            async move {
                let point = t3code_contracts::OrchestrationV2ThreadForkSourcePoint::LatestStable(t3code_contracts::OrchestrationV2ThreadForkSourcePointLatestStable {});
                commands.fork(&source, target.clone(), point, Some("Side chat".into()), true).await?;
                wait_for_thread(&shell, &target).await?;
                if let Some(mode) = runtime {
                    commands.set_runtime_mode(&target, mode).await?;
                }
                if !prompt.is_empty() {
                    let mut message = SendMessage::text(target.clone(), prompt);
                    message.model_selection = wire_model;
                    message.title_seed = seed;
                    commands.send_message(message).await?;
                }
                Ok::<_, CommandError>(())
            }
        },
        cx,
    );
    let opened = thread_key(&environment_id, &target);
    cx.spawn(async move |cx| match task.await {
        Some(Ok(())) => cx.update(|cx| super::open_thread(opened, cx)),
        Some(Err(error)) => cx.update(|cx| toast_error(t!("Could not start a side chat."), error, cx)),
        None => {}
    })
    .detach();
    EventOutcome::Done
}

/// Waits up to ten seconds for a new thread to reach the environment's shell.
pub async fn wait_for_thread(store: &EnvironmentStore, thread: &t3code_contracts::ThreadId) -> Result<(), CommandError> {
    let mut shell = store.shell().subscribe();
    let wait = async {
        loop {
            if shell.borrow_and_update().snapshot.as_ref().is_some_and(|s| s.threads.iter().any(|t| &t.id == thread)) {
                return;
            }
            if shell.changed().await.is_err() {
                return;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), wait)
        .await
        .map_err(|_| CommandError::Refused("The thread was created, but it did not reach this client.".into()))
}

/// Answers an approval request (`respondToApproval`).
pub fn respond_to_approval(key: &ThreadKey, request_id: &str, decision: ProviderApprovalDecision, cx: &mut App) {
    let (environment_id, thread_id) = runtime_ids(key);
    let Some(environment) = StateBridge::environment(&environment_id, cx) else { return };
    let commands = environment.read(cx).commands.clone();
    let request_id = RuntimeRequestId::new(request_id);
    let task = StateBridge::spawn(async move { commands.respond_to_approval(&thread_id, request_id, decision, None).await }, cx);
    cx.spawn(async move |cx| {
        if let Some(Err(error)) = task.await {
            cx.update(|cx| toast_error(t!("Something went wrong."), error, cx));
        }
    })
    .detach();
}

/// Answers a question, keyed by question id.
pub fn answer_question(key: &ThreadKey, request_id: &str, answers: ProviderUserInputAnswers, cx: &mut App) {
    let (environment_id, thread_id) = runtime_ids(key);
    let Some(environment) = StateBridge::environment(&environment_id, cx) else { return };
    let commands = environment.read(cx).commands.clone();
    let request_id = RuntimeRequestId::new(request_id);
    StateBridge::spawn(async move { commands.answer_question(&thread_id, request_id, answers).await }, cx).detach();
}

/// Drafts in the runtime's persisted document (`t3code:composer-drafts:v1`): the prompt, the
/// model and mode picks persist across restarts; attachments stay in memory with their uploads.
pub struct LiveDraftStore {
    drafts: Rc<ClientStore<Drafts>>,
    attachments: HashMap<ComposerDraftKey, Vec<crate::composer_draft_store::ComposerAttachment>>,
}

impl LiveDraftStore {
    pub fn new(drafts: Rc<ClientStore<Drafts>>) -> Self {
        Self { drafts, attachments: HashMap::new() }
    }
}

impl ComposerDraftStore for LiveDraftStore {
    fn load(&self, key: &ComposerDraftKey) -> Option<ComposerDraft> {
        let mut attachments = self.attachments.get(key).cloned().unwrap_or_default();
        for attachment in &mut attachments {
            if let Some(state) = super::attachments::AttachmentUploads::state(&attachment.context_id) {
                attachment.upload = state;
            }
        }
        let drafts = self.drafts.current();
        let draft = drafts.drafts_by_thread_key.get(key.0.as_ref());
        let model = draft
            .filter(|d| d.model_selection_explicit)
            .and_then(|d| d.active_provider.as_ref().and_then(|provider| d.model_selection_by_provider.get(provider)))
            .map(from_wire);
        if draft.is_none() && attachments.is_empty() {
            return None;
        }
        Some(ComposerDraft { prompt: draft.map(|d| d.prompt.clone()).unwrap_or_default(), attachments, model })
    }

    fn save(&mut self, key: &ComposerDraftKey, draft: &ComposerDraft) {
        if draft.attachments.is_empty() {
            self.attachments.remove(key);
        } else {
            self.attachments.insert(key.clone(), draft.attachments.clone());
        }
        let key = key.0.to_string();
        let prompt = draft.prompt.clone();
        self.drafts.update(|drafts| {
            let entry = drafts.drafts_by_thread_key.entry(key.clone()).or_default();
            entry.prompt = prompt;
            let empty = entry.prompt.trim().is_empty() && entry.runtime_mode.is_none() && !entry.model_selection_explicit && entry.attachments.is_empty();
            if empty {
                drafts.drafts_by_thread_key.remove(&key);
            }
        });
    }
}

type SearchKey = (ComposerTriggerKind, String);

/// A provider skill as the menu lists it.
#[derive(Clone)]
struct Skill {
    name: String,
    label: SharedString,
    description: SharedString,
    path: String,
}

/// Menu items for `@` (workspace files) and `$` / `/` (provider skills). File results arrive
/// from `projects.searchEntries` on tokio and come back through a channel, after which the
/// composer re-reads; until then the results of the longest cached prefix show with the
/// loading flag set. `items` runs inside the composer's update, so it only reads local state.
pub struct LiveMenuSource {
    store: EnvironmentStore,
    tokio: tokio::runtime::Handle,
    cwd: Option<String>,
    skills: RefCell<Arc<Vec<Skill>>>,
    results: RefCell<HashMap<SearchKey, Arc<Vec<ComposerCommandItem>>>>,
    pending: RefCell<Option<SearchKey>>,
    landed: futures::channel::mpsc::UnboundedSender<(SearchKey, Vec<ComposerCommandItem>)>,
    composer: RefCell<Option<WeakEntity<ChatComposer>>>,
    /// The shell's threads for `@` mentions, and the thread the composer is on.
    threads: Rc<crate::state::palette::LivePaletteSource>,
    environment_id: t3code_contracts::EnvironmentId,
    shown: RefCell<Option<ThreadKey>>,
    _receive: Task<()>,
}

impl LiveMenuSource {
    /// A source for a thread whose workspace is `cwd`; refreshes `composer` when results land.
    pub fn new(
        environment: &Entity<LiveEnvironment>,
        cwd: Option<String>,
        threads: Rc<crate::state::palette::LivePaletteSource>,
        cx: &mut App,
    ) -> Rc<Self> {
        let (landed, mut receiver) = futures::channel::mpsc::unbounded::<(SearchKey, Vec<ComposerCommandItem>)>();
        let env = environment.read(cx);
        let environment_id = env.environment_id().clone();
        let store = env.store.clone();
        let skills = Arc::new(skills_of(env.config.as_deref()));
        let tokio = StateBridge::global(cx).tokio.clone();
        Rc::new_cyclic(|weak: &std::rc::Weak<Self>| {
            let weak = weak.clone();
            let receive = cx.spawn(async move |cx| {
                use futures::StreamExt as _;
                while let Some((key, items)) = receiver.next().await {
                    let Some(this) = weak.upgrade() else { return };
                    if this.pending.borrow().as_ref() == Some(&key) {
                        *this.pending.borrow_mut() = None;
                    }
                    this.results.borrow_mut().insert(key, Arc::new(items));
                    let Some(composer) = this.composer.borrow().clone() else { continue };
                    cx.update(|cx| {
                        _ = composer.update(cx, |composer, cx| composer.refresh_menu(cx));
                    });
                }
            });
            Self {
                store,
                tokio,
                cwd,
                skills: RefCell::new(skills),
                results: RefCell::default(),
                pending: RefCell::default(),
                landed,
                composer: RefCell::default(),
                threads,
                environment_id,
                shown: RefCell::default(),
                _receive: receive,
            }
        })
    }

    /// The composer to refresh when results land.
    pub fn attach(&self, composer: &Entity<ChatComposer>) {
        *self.composer.borrow_mut() = Some(composer.downgrade());
    }

    /// The thread the composer is on, left out of `@` mentions.
    pub fn set_shown(&self, key: Option<ThreadKey>) {
        *self.shown.borrow_mut() = key;
    }

    /// Up to five threads matching `query` (`matchComposerThreadItems`): this environment's
    /// listed threads and the composer thread's own used side chats, without the composer's
    /// thread, ranked like the palette without picks; inserting one adds a thread chip.
    fn thread_items(&self, query: &str) -> Vec<ComposerCommandItem> {
        use t3code_shared::palette_search::{RankKey, SearchQuery, fold_search_query, rank_search_groups, score_thread_search};
        let folded = fold_search_query(query);
        if folded.is_empty() {
            return Vec::new();
        }
        let search = SearchQuery::new(&folded);
        let now = t3code_client_runtime::state::models::now_ms();
        let shown = self.shown.borrow().clone();
        let all = self.threads.mention_candidates();
        let own_side_chat = |t: &crate::state::palette::MentionCandidate| {
            t.side_chat_of.is_some() && t.side_chat_of == shown && t.has_user_message
        };
        let candidates: Vec<&Arc<crate::state::palette::MentionCandidate>> = all
            .iter()
            .filter(|t| {
                t.key.environment_id.0.as_ref() == self.environment_id.as_str()
                    && Some(&t.key) != shown.as_ref()
                    && (t.side_chat_of.is_none() || own_side_chat(t))
            })
            .collect();
        let ranked = rank_search_groups(&[candidates.len()], |_, item, typos| {
            let found = score_thread_search(&search, &candidates[item].subject, now, typos, false)?;
            let key = RankKey::from(&found);
            Some(((), key))
        });
        ranked
            .into_iter()
            .flat_map(|(_, items)| items)
            .take(5)
            .map(|(index, _, _)| {
                let thread = candidates[index];
                ComposerCommandItem {
                    id: SharedString::from(format!("thread:{}:{}", thread.key.environment_id.0, thread.key.thread_id.0)),
                    icon: ComposerCommandIcon::Icon(IconName::MessageSquare),
                    label: thread.title.clone(),
                    label_prefix: None,
                    description: if own_side_chat(thread) { t!("Side chat") } else { t!("Thread", context = "noun, search result type") },
                    badge: None,
                    action: ComposerCommandAction::Insert(SharedString::from(crate::composer_editor_mentions::format_context_reference(
                        "thread",
                        &format!("thread_{}", thread.key.thread_id.0),
                        &thread.title,
                    ))),
                }
            })
            .collect()
    }

    /// Takes the skills of a new server config.
    pub fn set_config(&self, config: Option<&ServerConfig>) {
        *self.skills.borrow_mut() = Arc::new(skills_of(config));
    }

    fn file_item(path: &str, kind: ProjectEntryKind) -> ComposerCommandItem {
        let trimmed = path.trim_end_matches('/');
        let (dir, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
        ComposerCommandItem {
            id: SharedString::from(format!("path:{path}")),
            icon: ComposerCommandIcon::Icon(if kind == ProjectEntryKind::Directory { IconName::Folder } else { IconName::File }),
            label: SharedString::from(name.to_owned()),
            label_prefix: None,
            description: SharedString::from(dir.to_owned()),
            badge: None,
            action: ComposerCommandAction::Insert(SharedString::from(serialize_file_link(path))),
        }
    }

    fn skill_items(&self, query: &str, slash: bool) -> Vec<ComposerCommandItem> {
        let query = query.to_lowercase();
        self.skills
            .borrow()
            .iter()
            .filter(|skill| skill.name.to_lowercase().contains(&query))
            .take(40)
            .map(|skill| ComposerCommandItem {
                id: SharedString::from(format!("skill:{}", skill.path)),
                icon: ComposerCommandIcon::None,
                label: skill.label.clone(),
                label_prefix: slash.then(|| SharedString::from("/skill:")),
                description: skill.description.clone(),
                badge: None,
                action: ComposerCommandAction::Insert(SharedString::from(format!("${} ", skill.name))),
            })
            .collect()
    }

    fn search_files(&self, key: SearchKey) {
        let (Some(cwd), Some(session)) = (self.cwd.clone(), self.store.session()) else { return };
        if self.pending.borrow().as_ref() == Some(&key) {
            return;
        }
        *self.pending.borrow_mut() = Some(key.clone());
        let landed = self.landed.clone();
        self.tokio.spawn(async move {
            let input = ProjectSearchEntriesInput { cwd, query: key.1.clone(), limit: FILE_SEARCH_LIMIT, kind: None, image_only: None };
            let entries = session.client.call::<ProjectsSearchEntries>(input).await.map(|r| r.entries).unwrap_or_default();
            let items = entries.iter().map(|entry| Self::file_item(&entry.path, entry.kind)).collect();
            _ = landed.unbounded_send((key, items));
        });
    }
}

fn skills_of(config: Option<&ServerConfig>) -> Vec<Skill> {
    config
        .iter()
        .flat_map(|config| config.providers.iter())
        .flat_map(|provider| provider.skills.iter().flatten())
        .filter(|skill| skill.enabled)
        .map(|skill| Skill {
            name: skill.name.clone(),
            label: SharedString::from(skill.display_name.clone().unwrap_or_else(|| skill.name.clone())),
            description: SharedString::from(skill.short_description.clone().or_else(|| skill.description.clone()).unwrap_or_default()),
            path: skill.path.clone(),
        })
        .collect()
}

impl ComposerMenuSource for LiveMenuSource {
    fn items(&self, trigger: &ComposerTrigger) -> (Vec<ComposerCommandItem>, bool) {
        match trigger.kind {
            ComposerTriggerKind::Skill => (self.skill_items(&trigger.query, false), false),
            ComposerTriggerKind::SlashCommand => (self.skill_items(&trigger.query, true), false),
            ComposerTriggerKind::PullRequest => (Vec::new(), false),
            ComposerTriggerKind::Path => {
                let key = (trigger.kind, trigger.query.clone());
                let mut threads = self.thread_items(&trigger.query);
                if let Some(items) = self.results.borrow().get(&key) {
                    threads.extend((**items).iter().cloned());
                    return (threads, false);
                }
                self.search_files(key);
                let results = self.results.borrow();
                let fallback = (0..trigger.query.len())
                    .rev()
                    .filter(|end| trigger.query.is_char_boundary(*end))
                    .find_map(|end| results.get(&(trigger.kind, trigger.query[..end].to_owned())))
                    .map(|items| (**items).clone())
                    .unwrap_or_default();
                threads.extend(fallback);
                (threads, true)
            }
        }
    }
}
