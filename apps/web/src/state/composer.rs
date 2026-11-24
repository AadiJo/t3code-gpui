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
use t3code_client_runtime::operations::{DeliveryMode, SendMessage};
use t3code_client_runtime::state::client_store::ClientStore;
use t3code_client_runtime::state::drafts::{Drafts, ThreadDraft, scoped_thread_key};
use t3code_client_runtime::state::models::ThreadRow;
use t3code_client_runtime::state::EnvironmentStore;
use t3code_contracts::rpc::methods::ProjectsSearchEntries;
use t3code_contracts::{
    MessageId as WireMessageId, ProjectEntryKind, ProjectSearchEntriesInput, ProviderApprovalDecision, ProviderUserInputAnswers,
    RuntimeMode as WireRuntimeMode, RuntimeRequestId, ServerConfig,
};

use super::shell::runtime_ids;
use super::{LiveEnvironment, StateBridge};
use crate::components::chat::composer::{ChatComposer, ComposerContextStrip, ComposerEvent, ComposerState, FollowUpBehavior};
use crate::components::chat::composer_command_menu::{ComposerCommandAction, ComposerCommandIcon, ComposerCommandItem, ComposerMenuSource};
use crate::components::icons::IconName;
use crate::components::chat::composer_permissions::RuntimeMode;
use crate::components::chat::composer_prompt_history::PromptHistoryEntry;
use crate::components::chat::timeline_entries::{EntryKind, MessageRole, TimelineInput};
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::composer_draft_store::{ComposerDraft, ComposerDraftKey, ComposerDraftStore, UploadState};
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

fn wire_runtime_mode(mode: RuntimeMode) -> WireRuntimeMode {
    match mode {
        RuntimeMode::ApprovalRequired => WireRuntimeMode::ApprovalRequired,
        RuntimeMode::AutoAcceptEdits => WireRuntimeMode::AutoAcceptEdits,
        RuntimeMode::Auto => WireRuntimeMode::Auto,
        RuntimeMode::FullAccess => WireRuntimeMode::FullAccess,
    }
}

/// The picker catalog of an environment, rebuilt only when its config `Arc` changes.
#[derive(Default)]
pub struct CatalogCache {
    source: Option<Arc<ServerConfig>>,
    catalog: Arc<ModelCatalog>,
}

impl CatalogCache {
    pub fn get(&mut self, config: Option<&Arc<ServerConfig>>) -> Arc<ModelCatalog> {
        let same = match (&self.source, config) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.catalog = Arc::new(config.map(|config| model_catalog(config)).unwrap_or_default());
            self.source = config.cloned();
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

/// The composer's state for a thread.
pub fn composer_state(key: &ThreadKey, row: Option<&ThreadRow>, thread: &LiveThread, environment: &LiveEnvironment, catalog: Arc<ModelCatalog>) -> ComposerState {
    let input = thread.input();
    let projection_thread = thread.projection().map(|p| Arc::clone(&p.thread));
    let wire_selection = row
        .map(|row| row.source.model_selection.clone())
        .or_else(|| projection_thread.as_ref().map(|t| t.model_selection.clone()));
    let selection = wire_selection.as_ref().map(from_wire).unwrap_or_else(|| default_selection(&catalog));
    let mode = row.map(|row| row.source.runtime_mode).or_else(|| projection_thread.as_ref().map(|t| t.runtime_mode));
    let default_mode = environment.config.as_ref().and_then(|c| c.settings.default_runtime_mode).unwrap_or(WireRuntimeMode::FullAccess);
    let connected = environment.store.session().is_some();
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
        banners: Vec::new(),
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
    }
}

fn toast_error(title: SharedString, error: impl ToString, cx: &mut App) {
    Toaster::push(ToastData::new(ToastKind::Error, title).description(SharedString::from(error.to_string())), cx);
}

/// Handles a composer event for the thread it edits.
pub fn handle_event(event: &ComposerEvent, thread: &Entity<LiveThread>, composer: &WeakEntity<ChatComposer>, cx: &mut App) {
    let key = thread.read(cx).key().clone();
    let (environment_id, thread_id) = runtime_ids(&key);
    let Some(environment) = StateBridge::environment(&environment_id, cx) else { return };
    let (commands, config) = {
        let env = environment.read(cx);
        (env.commands.clone(), env.config.clone())
    };
    match event {
        ComposerEvent::Submit { prompt, model, dispatch, .. } => {
            let mut message = SendMessage::text(thread_id.clone(), prompt.clone());
            message.mode = match dispatch {
                Some(FollowUpBehavior::Queue) => DeliveryMode::Queue,
                Some(FollowUpBehavior::Steer) => DeliveryMode::Steer,
                None => DeliveryMode::Auto,
            };
            let catalog = config.as_ref().map(|c| model_catalog(c)).unwrap_or_default();
            message.model_selection = Some(to_wire(model, &catalog, config.as_deref()));
            let message_id: WireMessageId = message.message_id.clone();
            // A queued message shows in the queue, not as a sent bubble.
            if message.mode != DeliveryMode::Queue {
                thread.update(cx, |thread, cx| thread.push_optimistic(message_id.clone(), prompt.clone(), cx));
            }
            let task = StateBridge::spawn(async move { commands.send_message(message).await }, cx);
            let thread = thread.downgrade();
            cx.spawn(async move |cx| {
                if let Some(Err(error)) = task.await {
                    cx.update(|cx| {
                        _ = thread.update(cx, |thread, cx| thread.remove_optimistic(&message_id, cx));
                        toast_error(t!("Failed to send message."), error, cx);
                    });
                }
            })
            .detach();
        }
        ComposerEvent::Stop => {
            StateBridge::spawn(async move { commands.interrupt(&thread_id, None, None).await }, cx).detach();
        }
        ComposerEvent::ModelChanged(selection) => {
            let catalog = config.as_ref().map(|c| model_catalog(c)).unwrap_or_default();
            let wire = to_wire(selection, &catalog, config.as_deref());
            StateBridge::spawn(async move { commands.set_model_selection(&thread_id, wire).await }, cx).detach();
        }
        ComposerEvent::RuntimeModeChanged(mode) => {
            let mode = wire_runtime_mode(*mode);
            StateBridge::spawn(async move { commands.set_runtime_mode(&thread_id, mode).await }, cx).detach();
        }
        ComposerEvent::AttachmentsAdded(attachments) => {
            let ids: Vec<SharedString> = attachments.iter().map(|a| a.context_id.clone()).collect();
            _ = composer.update(cx, |composer, cx| {
                for id in &ids {
                    composer.set_upload_state(id, UploadState::Failed, cx);
                }
            });
            toast_error(t!("Upload failed"), "Uploads are not available in this build yet.", cx);
        }
        _ => {}
    }
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

/// Drafts in the runtime's persisted document (`t3code:composer-drafts:v1`).
pub struct LiveDraftStore {
    drafts: Rc<ClientStore<Drafts>>,
}

impl LiveDraftStore {
    pub fn new(drafts: Rc<ClientStore<Drafts>>) -> Self {
        Self { drafts }
    }
}

impl ComposerDraftStore for LiveDraftStore {
    fn load(&self, key: &ComposerDraftKey) -> Option<ComposerDraft> {
        let drafts = self.drafts.current();
        let draft = drafts.drafts_by_thread_key.get(key.0.as_ref())?;
        let model = draft
            .active_provider
            .as_ref()
            .and_then(|provider| draft.model_selection_by_provider.get(provider))
            .filter(|_| draft.model_selection_explicit)
            .map(from_wire);
        Some(ComposerDraft { prompt: draft.prompt.clone(), attachments: Vec::new(), model })
    }

    fn save(&mut self, key: &ComposerDraftKey, draft: &ComposerDraft) {
        let key = key.0.to_string();
        let prompt = draft.prompt.clone();
        self.drafts.update(|drafts| {
            if prompt.trim().is_empty() {
                drafts.drafts_by_thread_key.remove(&key);
            } else {
                drafts.drafts_by_thread_key.entry(key).or_insert_with(ThreadDraft::default).prompt = prompt;
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
    _receive: Task<()>,
}

impl LiveMenuSource {
    /// A source for a thread whose workspace is `cwd`; refreshes `composer` when results land.
    pub fn new(environment: &Entity<LiveEnvironment>, cwd: Option<String>, cx: &mut App) -> Rc<Self> {
        let (landed, mut receiver) = futures::channel::mpsc::unbounded::<(SearchKey, Vec<ComposerCommandItem>)>();
        let env = environment.read(cx);
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
                _receive: receive,
            }
        })
    }

    /// The composer to refresh when results land.
    pub fn attach(&self, composer: &Entity<ChatComposer>) {
        *self.composer.borrow_mut() = Some(composer.downgrade());
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
                if let Some(items) = self.results.borrow().get(&key) {
                    return ((**items).clone(), false);
                }
                self.search_files(key);
                let results = self.results.borrow();
                let fallback = (0..trigger.query.len())
                    .rev()
                    .filter(|end| trigger.query.is_char_boundary(*end))
                    .find_map(|end| results.get(&(trigger.kind, trigger.query[..end].to_owned())))
                    .map(|items| (**items).clone())
                    .unwrap_or_default();
                (fallback, true)
            }
        }
    }
}
