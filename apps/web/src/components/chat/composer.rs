//! The chat composer: the floating card around the prompt editor, its
//! attachments, attached banners, the footer controls, the command menu and
//! the model picker. Ports `ChatComposer.tsx`, `ComposerSurface.tsx`,
//! `ComposerPrimaryActions.tsx`, `ComposerControlsBar.tsx` and
//! `ComposerBanner.tsx` with the fork's changes (wider card, quiet 28px icon
//! actions, `CornerDownLeft` send).
//!
//! The composer never talks to the network. It raises [`ComposerEvent`]s for
//! the chat view, which feeds state back through [`ChatComposer::set_state`].

use crate::components::ui::GlassExt as _;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AnyElement, App, AppContext as _, BoxShadow, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, Modifiers, ModifiersChangedEvent, ParentElement as _,
    Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, canvas,
    deferred, div, hsla, img, StyledImage as _, point, prelude::FluentBuilder as _, px, relative,
};

use crate::components::chat::composer_command_menu::{
    ComposerCommandAction, ComposerMenuSource, ComposerMenuState, SlashCommand, SlashMenuContext, builtin_slash_items, render_menu,
};
use crate::components::chat::composer_icons::{composer_icon_stroke, provider_icon};
use crate::components::chat::composer_permissions::{RuntimeMode, runtime_mode_icon};
use crate::components::chat::composer_prompt_history::{PromptHistoryEntry, PromptHistoryPosition, step_prompt_history};
use crate::components::chat::composer_theme::{
    ATTACHMENT_OVERLAP, BODY_PADDING_BOTTOM, BORDER, BODY_PADDING_TOP, BODY_PADDING_X, COMPOSER_MAX_WIDTH, CONTROL_HEIGHT,
    CONTROL_RADIUS, ComposerColors, DRAWER_INSET, FOOTER_COMPACT_BREAKPOINT, FOOTER_PADDING_BOTTOM, FOOTER_PADDING_X,
    ICON_ACTION_RADIUS, SURFACE_RADIUS, ui_font_family,
};
use crate::components::chat::model_picker::{ModelPicker, model_picker_size, ModelPickerEvent, input_handler_canvas, popup_shadow};
use crate::components::composer_prompt_editor::{
    self as editor_mod, ComposerEditorEvent, ComposerPromptEditor, PastedContent,
};
use crate::composer_draft_store::{
    ComposerAttachment, ComposerAttachmentKind, ComposerDraft, ComposerDraftKey, ComposerDraftStore,
    DRAFT_PERSIST_DEBOUNCE, UploadState,
};
use crate::composer_editor_mentions::format_context_reference;
use crate::composer_logic::{
    ComposerSubmissionIntent, ComposerSubmitContext, ComposerTrigger, ComposerTriggerKind, SendShortcut,
    detect_composer_trigger, extend_replacement_range_for_trailing_space, is_clear_command,
};
use crate::model_selection::{EffortGlyph, ModelCatalog, ModelSelection, ProviderIcon, display_model_name, effort_bar_heights, resolve_effort_glyph};

/// Stop stays disabled this long while the interrupt is in flight.
const STOP_PENDING_MAX: Duration = Duration::from_secs(10);
/// The action that replaces Stop ignores clicks this long.
const STOP_LANDED_CLICK_GUARD: Duration = Duration::from_millis(500);
/// Image tile size in the attachment strip.
const IMAGE_TILE: Pixels = px(64.);

/// Registers the composer's key bindings (editor and picker).
pub fn init(cx: &mut App) {
    editor_mod::bind_keys(cx);
    crate::components::chat::model_picker::bind_keys(cx);
    crate::composer_code_languages::warm_highlighter();
}

/// What a send while a turn runs does by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FollowUpBehavior {
    #[default]
    Steer,
    Queue,
}

impl FollowUpBehavior {
    fn flipped(self) -> Self {
        match self {
            Self::Steer => Self::Queue,
            Self::Queue => Self::Steer,
        }
    }
}

/// Stop button phase. Ports `useStopPhase`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StopPhase {
    #[default]
    Idle,
    Pending,
    TimedOut,
    Landed,
}

/// Tone of an attached banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerTone {
    Neutral,
    Warning,
    Error,
}

/// A banner attached to the composer's top edge.
#[derive(Clone, Debug)]
pub struct ComposerBanner {
    pub id: SharedString,
    pub tone: BannerTone,
    pub icon: IconName,
    pub title: SharedString,
    /// Muted text after the title, truncated first (`ComposerBannerStackItem.description`).
    pub description: Option<SharedString>,
    pub action: Option<SharedString>,
    /// A second action after `action`; its click raises `BannerAction("<id>#secondary")`.
    pub secondary_action: Option<SharedString>,
    /// Shows the dismiss button (`ComposerBanner.Dismiss`) with this accessible label.
    pub dismiss: Option<SharedString>,
    /// Drawn under the row (`ComposerBannerStackItem.children`).
    pub body: Option<Arc<super::composer_usage_limits::UsageLimitsBody>>,
}

/// The environment, workspace and branch strip under the card.
#[derive(Clone, Debug)]
pub struct ComposerContextStrip {
    pub environment: SharedString,
    pub workspace: SharedString,
    pub branch: SharedString,
}

/// Everything the chat view tells the composer.
#[derive(Clone)]
pub struct ComposerState {
    pub draft_key: ComposerDraftKey,
    pub is_draft_thread: bool,
    pub catalog: Arc<ModelCatalog>,
    pub selection: ModelSelection,
    pub runtime_mode: RuntimeMode,
    /// The user's default mode; the permissions chip shows only when the thread differs.
    pub default_runtime_mode: RuntimeMode,
    pub running: bool,
    pub can_interrupt: bool,
    pub follow_up: FollowUpBehavior,
    pub send_shortcut: SendShortcut,
    pub rich_text: bool,
    /// Why sending is blocked, shown as the send button's label.
    pub send_disabled_reason: Option<SharedString>,
    /// No project chosen: the card dims and the placeholder changes.
    pub project_required: bool,
    pub is_send_busy: bool,
    pub history: Arc<Vec<PromptHistoryEntry>>,
    pub banners: Vec<ComposerBanner>,
    pub context_strip: Option<ComposerContextStrip>,
    /// Shows the dictation button. As in the fork, it is shown only when a
    /// transcription destination is chosen and ready, never as a setup prompt.
    pub voice_available: bool,
    pub validation: Option<SharedString>,
    /// Replaces the placeholder while the agent waits: a pending approval
    /// ("Send a message to decline…") or question ("Type your own answer…").
    pub placeholder: Option<SharedString>,
    /// The largest file the server stages (`fileAttachmentStagingLimit`); `None` when it takes
    /// no files.
    pub file_attachment_limit: Option<u64>,
    /// `/usage-limits` opens the limits banner (`onUsageLimitsCommand`): the provider reports
    /// limits and the thread can show them.
    pub usage_limits: bool,
}

/// What the composer asks the chat view to do.
#[derive(Clone, Debug)]
pub enum ComposerEvent {
    Submit {
        prompt: String,
        attachments: Vec<ComposerAttachment>,
        model: ModelSelection,
        intent: ComposerSubmissionIntent,
        /// Set while a turn runs.
        dispatch: Option<FollowUpBehavior>,
    },
    /// `/clear` as the whole message.
    ClearSession,
    /// `/side` or `/btw`.
    SideChat { prompt: String },
    Stop,
    /// The attach button: open the file picker.
    AttachRequested,
    AttachmentsAdded(Vec<ComposerAttachment>),
    /// A loaded draft holds attachments that are not uploaded (restored after a restart).
    AttachmentsRestored(Vec<ComposerAttachment>),
    AttachmentRemoved(SharedString),
    ModelChanged(ModelSelection),
    RuntimeModeChanged(RuntimeMode),
    FavoriteToggled { instance_id: crate::model_selection::ProviderInstanceId, model: SharedString },
    /// The editor cannot page; scroll the timeline.
    PageScroll { down: bool },
    VoiceRequested,
    ContextStripClicked(ContextStripControl),
    /// A banner's action button, by banner id.
    BannerAction(SharedString),
    /// A banner's dismiss button, by banner id.
    BannerDismissed(SharedString),
    /// `/usage-limits`, picked from the menu or sent alone.
    UsageLimits,
    /// The limits banner's confirmed reset credit, for an account of its report.
    RedeemResetCredit { account: SharedString, choice: crate::components::usage::usage_limits::ResetCreditChoice },
    OpenUrl(SharedString),
    /// Stashed files leaving the stash unrestored (deleted or evicted): delete their uploads.
    ReleaseStashedFiles(Vec<crate::prompt_stash_store::StashedFile>),
}

/// A control in the context strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextStripControl {
    Environment,
    Workspace,
    Branch,
}

/// The composer view.
pub struct ChatComposer {
    editor: Entity<ComposerPromptEditor>,
    state: ComposerState,
    pub(super) attachments: Vec<ComposerAttachment>,
    /// HEIC photos converting off the UI thread, by draft; each holds an attachment slot.
    pub(super) converting: Vec<ComposerDraftKey>,
    picker: Option<(Entity<ModelPicker>, Subscription)>,
    menu: Option<ComposerMenuState>,
    dismissed_trigger: Option<Range<usize>>,
    menu_source: Rc<dyn ComposerMenuSource>,
    history_position: Option<PromptHistoryPosition>,
    modifier_held: bool,
    stop_phase: StopPhase,
    stop_task: Option<Task<()>>,
    draft_store: Rc<RefCell<dyn ComposerDraftStore>>,
    save_task: Option<Task<()>>,
    footer_width: Pixels,
    /// Limits banner accounts whose email is shown.
    pub(super) usage_revealed: std::collections::HashSet<SharedString>,
    /// The reset credit awaiting confirmation, by account.
    pub(super) usage_confirming: Option<(SharedString, crate::components::usage::usage_limits::ResetCreditChoice)>,
    /// The branch strip under the card: the host, workspace and branch shortcuts open its menus.
    context_controls: Option<gpui_kit::WeakEntity<crate::components::branch_toolbar::BranchToolbar>>,
    /// The permissions chip's menu is open, with the mode a hover previews.
    permissions_menu: Option<Option<RuntimeMode>>,
    /// The command whose chord is down: a held key repeats it, and the fork ignores repeats.
    held_command: Option<std::any::TypeId>,
    /// The last frame moved the permissions chip into the overflow button.
    chip_hidden: bool,
    /// Pages the timeline when Page Up/Down reach the prompt's edge.
    page_scroller: Option<super::composer_page_scroll::PageScroller>,
    /// The stash drawer is open on this row (`isStashMenuOpen`).
    pub(super) stash_menu: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for ChatComposer {}

impl Focusable for ChatComposer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl ChatComposer {
    pub fn new(
        state: ComposerState,
        menu_source: Rc<dyn ComposerMenuSource>,
        draft_store: Rc<RefCell<dyn ComposerDraftStore>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(ComposerPromptEditor::new);
        let subscription = cx.subscribe(&editor, Self::on_editor_event);
        let settings = cx.observe_global::<crate::hooks::use_settings::ClientSettingsStore>(|this, cx| {
            this.sync_editor(cx);
            cx.notify();
        });
        let mut this = Self {
            editor,
            state,
            attachments: Vec::new(),
            converting: Vec::new(),
            picker: None,
            menu: None,
            dismissed_trigger: None,
            menu_source,
            history_position: None,
            modifier_held: false,
            stop_phase: StopPhase::Idle,
            stop_task: None,
            draft_store,
            save_task: None,
            footer_width: COMPOSER_MAX_WIDTH,
            usage_revealed: Default::default(),
            usage_confirming: None,
            context_controls: None,
            permissions_menu: None,
            held_command: None,
            chip_hidden: false,
            page_scroller: None,
            stash_menu: None,
            _subscriptions: vec![subscription, settings],
        };
        this.load_draft(cx);
        this.sync_editor(cx);
        this
    }

    /// The prompt editor, for focus and scripted input.
    /// What the chat view last told the composer.
    pub fn state(&self) -> &ComposerState {
        &self.state
    }

    /// Takes files from the file picker, like a drop.
    pub fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.attach(PastedContent::Files(paths), cx);
    }

    /// The attachments waiting to send.
    pub fn attachments(&self) -> &[ComposerAttachment] {
        &self.attachments
    }

    pub fn editor(&self) -> &Entity<ComposerPromptEditor> {
        &self.editor
    }

    /// The open model picker, if any.
    pub fn picker(&self) -> Option<&Entity<ModelPicker>> {
        self.picker.as_ref().map(|(p, _)| p)
    }

    /// Whether the command menu is showing.
    pub fn menu(&self) -> Option<&ComposerMenuState> {
        self.menu.as_ref()
    }

    /// Updates what the chat view knows. Switching `draft_key` saves the old
    /// draft and loads the new one.
    pub fn set_state(&mut self, state: ComposerState, cx: &mut Context<Self>) {
        let switched = state.draft_key != self.state.draft_key;
        if switched {
            self.save_draft_now(cx);
        }
        let interrupted = self.state.can_interrupt && !state.can_interrupt;
        let resumed = !self.state.can_interrupt && state.can_interrupt;
        self.state = state;
        if switched {
            self.stop_phase = StopPhase::Idle;
            self.load_draft(cx);
        } else if interrupted && matches!(self.stop_phase, StopPhase::Pending | StopPhase::TimedOut) {
            self.set_stop_phase(StopPhase::Landed, cx);
        } else if resumed && self.stop_phase == StopPhase::Landed {
            self.stop_phase = StopPhase::Idle;
        }
        if let Some((picker, _)) = &self.picker {
            let (catalog, selection, mode) = (self.state.catalog.clone(), self.state.selection.clone(), self.state.runtime_mode);
            picker.update(cx, |p, cx| p.update_state(catalog, selection, mode, cx));
        }
        self.sync_editor(cx);
        // An open menu re-reads its source: provider commands or a model change may have
        // arrived after it opened.
        if self.menu.is_some() && !switched {
            self.refresh_menu(cx);
        }
        cx.notify();
    }

    fn sync_editor(&mut self, cx: &mut Context<Self>) {
        let context = ComposerSubmitContext {
            is_draft_thread: self.state.is_draft_thread,
            is_running: self.state.running,
            software_keyboard_open: false,
            send_shortcut: crate::hooks::settings_bridge::composer_send_shortcut(cx),
        };
        let placeholder: SharedString = if let Some(placeholder) = self.state.placeholder.clone() {
            placeholder
        } else if self.state.project_required {
            "Choose a project above to start a thread".into()
        } else {
            "Ask anything. @ files, $ skills, / commands".into()
        };
        let rich = crate::hooks::settings_bridge::composer_rich_text(cx);
        self.editor.update(cx, |editor, _| {
            editor.submit_context = context;
            editor.placeholder = placeholder;
            editor.rich_text = rich;
        });
    }

    // ---- Drafts ----

    fn load_draft(&mut self, cx: &mut Context<Self>) {
        let draft = self.draft_store.borrow().load(&self.state.draft_key).unwrap_or_default();
        self.attachments = draft.attachments;
        let waiting: Vec<ComposerAttachment> = self.attachments.iter().filter(|a| a.upload == UploadState::Uploading).cloned().collect();
        if !waiting.is_empty() {
            cx.emit(ComposerEvent::AttachmentsRestored(waiting));
        }
        self.history_position = None;
        self.menu = None;
        self.editor.update(cx, |editor, cx| editor.set_text(draft.prompt, cx));
    }

    fn current_draft(&self, cx: &App) -> ComposerDraft {
        ComposerDraft { prompt: self.editor.read(cx).text().to_owned(), attachments: self.attachments.clone(), model: None }
    }

    /// Writes the draft now (a stash or restore changed it wholesale).
    pub(super) fn save_stashed_draft(&mut self, cx: &mut Context<Self>) {
        self.save_draft_now(cx);
    }

    fn save_draft_now(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        let draft = self.current_draft(cx);
        self.draft_store.borrow_mut().save(&self.state.draft_key, &draft);
    }

    /// Writes the draft after a 300 ms pause; each change restarts the wait.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_PERSIST_DEBOUNCE).await;
            this.update(cx, |this, cx| this.save_draft_now(cx)).ok();
        }));
    }

    // ---- Editor events ----

    fn on_editor_event(&mut self, _: Entity<ComposerPromptEditor>, event: &ComposerEditorEvent, cx: &mut Context<Self>) {
        match event {
            ComposerEditorEvent::Changed => {
                let text = self.editor.read(cx).text().to_owned();
                if self.history_position.as_ref().is_some_and(|p| p.recalled != text) {
                    self.history_position = None;
                }
                // The stash closes when the prompt changes (`stashMenuClosesOn`).
                self.stash_menu = None;
                self.refresh_menu(cx);
                self.schedule_save(cx);
                cx.notify();
            }
            ComposerEditorEvent::SelectionChanged => self.refresh_menu(cx),
            ComposerEditorEvent::Submit(intent) => self.submit(*intent, cx),
            ComposerEditorEvent::Tab => {}
            // A press whose key-up landed elsewhere (the branch picker took focus) is over.
            ComposerEditorEvent::FocusChanged(_) => self.held_command = None,
            ComposerEditorEvent::Escape => {
                if self.menu.is_some() {
                    self.dismiss_menu(cx);
                }
            }
            ComposerEditorEvent::PageScroll { down } => {
                let paged = self.page_scroller.as_mut().is_some_and(|s| s.press(*down, cx));
                if !paged {
                    cx.emit(ComposerEvent::PageScroll { down: *down });
                }
            }
            ComposerEditorEvent::Attach(content) => self.attach(content.clone(), cx),
        }
    }

    // ---- Menu ----

    /// Re-reads the menu source for the trigger at the cursor (async results landed).
    pub fn refresh_menu(&mut self, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let trigger = editor.selection().is_empty().then(|| detect_composer_trigger(editor.text(), editor.cursor())).flatten();
        let trigger = trigger.filter(|t| self.dismissed_trigger.as_ref().is_none_or(|d| d.start != t.range.start));
        if self.dismissed_trigger.is_some() && trigger.is_none() {
            let still_there = detect_composer_trigger(editor.text(), editor.cursor()).is_some();
            if !still_there {
                self.dismissed_trigger = None;
            }
        }
        let Some(trigger) = trigger.filter(|_| !editor.caret_in_code()) else {
            if self.menu.take().is_some() {
                cx.notify();
            }
            return;
        };
        let slash = (trigger.kind == ComposerTriggerKind::SlashCommand)
            .then(|| {
                let context = SlashMenuContext { prompt: editor.text(), instance_id: &self.state.selection.instance_id, has_attachments: !self.attachments.is_empty() };
                self.menu_source.slash_items(&trigger, &context)
            })
            .flatten();
        let (items, loading) = slash.unwrap_or_else(|| {
            let (mut items, loading) = self.menu_source.items(&trigger);
            if trigger.kind == ComposerTriggerKind::SlashCommand {
                let mut builtins = builtin_slash_items(&trigger.query);
                builtins.append(&mut items);
                items = builtins;
            }
            (items, loading)
        });
        let active = self.menu.as_ref().filter(|m| m.trigger.range.start == trigger.range.start).map_or(0, |m| m.active);
        let empty = self.menu_source.empty_text(&trigger);
        self.menu = Some(ComposerMenuState { active: active.min(items.len().saturating_sub(1)), trigger, items: Arc::new(items), loading, empty });
        self.stash_menu = None;
        cx.notify();
    }

    fn dismiss_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.take() {
            self.dismissed_trigger = Some(menu.trigger.range);
            cx.notify();
        }
    }

    /// Picks a menu item by index.
    pub fn pick_menu_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else { return };
        let Some(item) = menu.items.get(index).cloned() else { return };
        if item.action == ComposerCommandAction::Refused {
            self.menu = Some(menu);
            return;
        }
        let trigger: ComposerTrigger = menu.trigger;
        let text = self.editor.read(cx).text().to_owned();
        let replace = |this: &mut Self, replacement: String, cx: &mut Context<Self>| {
            let end = extend_replacement_range_for_trailing_space(&text, trigger.range.end, &replacement);
            this.editor.update(cx, |e, cx| e.replace_range(trigger.range.start..end, &replacement, cx));
        };
        match item.action {
            ComposerCommandAction::Insert(insert) => replace(self, format!("{insert} "), cx),
            ComposerCommandAction::Refused => {}
            ComposerCommandAction::Slash(SlashCommand::Model) => {
                replace(self, String::new(), cx);
                self.open_picker(window, cx);
            }
            ComposerCommandAction::Slash(SlashCommand::Clear) => {
                replace(self, String::new(), cx);
                cx.emit(ComposerEvent::ClearSession);
            }
            ComposerCommandAction::Slash(SlashCommand::UsageLimits) => {
                replace(self, String::new(), cx);
                cx.emit(ComposerEvent::UsageLimits);
            }
            ComposerCommandAction::Slash(command) => {
                let word = if command == SlashCommand::Btw { "/btw " } else { "/side " };
                replace(self, word.to_owned(), cx);
            }
            ComposerCommandAction::Model { instance_id, model, insert } => {
                replace(self, format!("{insert} "), cx);
                let mut selection = self.state.selection.clone();
                selection.instance_id = instance_id;
                selection.model = model;
                cx.emit(ComposerEvent::ModelChanged(selection));
            }
        }
        self.menu = None;
        cx.notify();
    }

    // ---- Picker ----

    /// Opens or closes the model picker.
    pub fn toggle_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            self.close_picker(window, cx);
        } else {
            self.open_picker(window, cx);
        }
    }

    pub fn open_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (catalog, selection, mode) = (self.state.catalog.clone(), self.state.selection.clone(), self.state.runtime_mode);
        let picker = cx.new(|cx| ModelPicker::new(catalog, selection, mode, cx));
        let subscription = cx.subscribe_in(&picker, window, |this, _, event: &ModelPickerEvent, window, cx| match event {
            ModelPickerEvent::Select(selection) => {
                this.state.selection = selection.clone();
                cx.emit(ComposerEvent::ModelChanged(selection.clone()));
                cx.notify();
            }
            ModelPickerEvent::ToggleFavorite { instance_id, model } => {
                let mut catalog = (*this.state.catalog).clone();
                catalog.toggle_favorite(instance_id, model);
                this.state.catalog = Arc::new(catalog);
                if let Some((picker, _)) = &this.picker {
                    let (c, s, m) = (this.state.catalog.clone(), this.state.selection.clone(), this.state.runtime_mode);
                    picker.update(cx, |p, cx| p.update_state(c, s, m, cx));
                }
                cx.emit(ComposerEvent::FavoriteToggled { instance_id: instance_id.clone(), model: model.clone() });
            }
            ModelPickerEvent::RuntimeModeChanged(mode) => {
                this.state.runtime_mode = *mode;
                cx.emit(ComposerEvent::RuntimeModeChanged(*mode));
                cx.notify();
            }
            ModelPickerEvent::Close => this.close_picker(window, cx),
        });
        window.focus(&picker.focus_handle(cx), cx);
        self.picker = Some((picker, subscription));
        cx.notify();
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            window.focus(&self.editor.focus_handle(cx), cx);
            cx.notify();
        }
    }

    // ---- Sending ----

    fn has_sendable_content(&self, cx: &App) -> bool {
        !self.editor.read(cx).text().trim().is_empty() || !self.attachments.is_empty()
    }

    fn uploads_pending(&self) -> bool {
        self.converting.contains(&self.state.draft_key) || self.attachments.iter().any(|a| a.upload != UploadState::Uploaded)
    }

    /// The follow-up action a send takes now, with Cmd/Ctrl held flipping it.
    pub fn running_dispatch(&self) -> FollowUpBehavior {
        if self.modifier_held { self.state.follow_up.flipped() } else { self.state.follow_up }
    }

    /// Sends the draft. Ports `useComposerSend`'s gatekeeping.
    pub fn submit(&mut self, intent: ComposerSubmissionIntent, cx: &mut Context<Self>) {
        if self.state.send_disabled_reason.is_some() || self.state.is_send_busy || self.uploads_pending() || !self.has_sendable_content(cx) {
            return;
        }
        let prompt = self.editor.read(cx).text().to_owned();
        // `isUsageLimitsCommand`: the chat view opens the banner and clears the prompt.
        if self.state.usage_limits && self.attachments.is_empty() && prompt.trim().eq_ignore_ascii_case("/usage-limits") {
            cx.emit(ComposerEvent::UsageLimits);
        } else if is_clear_command(&prompt) {
            cx.emit(ComposerEvent::ClearSession);
        } else if let Some(rest) = prompt.trim().strip_prefix("/side").or_else(|| prompt.trim().strip_prefix("/btw")) {
            cx.emit(ComposerEvent::SideChat { prompt: rest.trim().to_owned() });
        } else {
            let dispatch = self.state.running.then(|| match intent {
                ComposerSubmissionIntent::Alternate => self.state.follow_up.flipped(),
                _ => self.running_dispatch(),
            });
            cx.emit(ComposerEvent::Submit {
                prompt,
                attachments: std::mem::take(&mut self.attachments),
                model: self.state.selection.clone(),
                intent,
                dispatch,
            });
        }
        self.attachments.clear();
        self.history_position = None;
        self.menu = None;
        self.editor.update(cx, |e, cx| e.set_text(String::new(), cx));
        self.save_draft_now(cx);
        cx.notify();
    }

    fn set_stop_phase(&mut self, phase: StopPhase, cx: &mut Context<Self>) {
        self.stop_phase = phase;
        let wait = match phase {
            StopPhase::Pending => Some((STOP_PENDING_MAX, StopPhase::TimedOut)),
            StopPhase::Landed => Some((STOP_LANDED_CLICK_GUARD, StopPhase::Idle)),
            _ => None,
        };
        self.stop_task = wait.map(|(after, next)| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(after).await;
                this.update(cx, |this, cx| {
                    this.stop_phase = next;
                    cx.notify();
                })
                .ok();
            })
        });
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.stop_phase == StopPhase::Pending {
            return;
        }
        self.set_stop_phase(StopPhase::Pending, cx);
        cx.emit(ComposerEvent::Stop);
    }

    // ---- Attachments ----

    /// Takes pasted, dropped or picked content through the web's intake rules
    /// (`useComposerAttachmentIntake`); a refusal shows as an error toast.
    fn attach(&mut self, content: PastedContent, cx: &mut Context<Self>) {
        use crate::state::attachments::{Candidate, PASTED_TEXT_MIME, intake, mime_from_name};
        let candidates: Vec<Candidate> = match content {
            PastedContent::Images(images) => images
                .into_iter()
                .map(|image| {
                    let mime = format!("image/{:?}", image.format).to_lowercase();
                    let name = format!("image.{}", if mime == "image/jpeg" { "jpg" } else { mime.trim_start_matches("image/") });
                    Candidate { name, mime, size_bytes: image.bytes.len() as u64, kind: ComposerAttachmentKind::Image { bytes: image.bytes.clone().into(), mime: SharedString::default() } }
                })
                .collect(),
            PastedContent::Files(paths) => paths
                .into_iter()
                .filter(|path| path.is_file())
                .map(|path| {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    Candidate {
                        mime: mime_from_name(&name).unwrap_or_default().to_owned(),
                        size_bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                        kind: ComposerAttachmentKind::File { path: path.to_string_lossy().into_owned().into() },
                        name,
                    }
                })
                .collect(),
            PastedContent::LargeText(text) => {
                let taken: Vec<String> = self.attachments.iter().map(|a| a.name.to_lowercase()).collect();
                let name = std::iter::once("pasted-text.txt".to_owned())
                    .chain((2..).map(|n| format!("pasted-text-{n}.txt")))
                    .find(|name| !taken.contains(name))
                    .unwrap_or_default();
                vec![Candidate { name, mime: PASTED_TEXT_MIME.into(), size_bytes: text.len() as u64, kind: ComposerAttachmentKind::PastedText { text } }]
            }
        };
        let held = self.attachments.len() + self.converting.iter().filter(|k| **k == self.state.draft_key).count();
        let result = intake(candidates, held, self.state.file_attachment_limit);
        if let Some(error) = result.error {
            crate::components::ui::toast::Toaster::push(
                crate::components::ui::toast::ToastData::new(crate::components::ui::toast::ToastKind::Error, error),
                cx,
            );
        }
        self.add_attachments(result.accepted, cx);
        for job in result.converting {
            self.convert_heic(job, cx);
        }
    }

    /// Converts a HEIC photo on a background thread, then adds it to the draft it was dropped on
    /// (saved into that draft when the composer has moved to another thread).
    fn convert_heic(&mut self, job: crate::state::attachments::HeicJob, cx: &mut Context<Self>) {
        let key = self.state.draft_key.clone();
        self.converting.push(key.clone());
        let converted = cx.background_spawn(async move { job.convert() });
        cx.spawn(async move |this, cx| {
            let result = converted.await;
            _ = this.update(cx, |this, cx| {
                if let Some(at) = this.converting.iter().position(|k| *k == key) {
                    this.converting.remove(at);
                }
                match result {
                    Ok(attachment) if key == this.state.draft_key => this.add_attachments(vec![attachment], cx),
                    Ok(attachment) => {
                        let mut draft = this.draft_store.borrow().load(&key).unwrap_or_default();
                        draft.attachments.push(attachment.clone());
                        this.draft_store.borrow_mut().save(&key, &draft);
                        cx.emit(ComposerEvent::AttachmentsAdded(vec![attachment]));
                    }
                    Err(error) => {
                        crate::components::ui::toast::Toaster::push(
                            crate::components::ui::toast::ToastData::new(crate::components::ui::toast::ToastKind::Error, error),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Adds attachments whose chips the text already has (a rewound message), starting uploads.
    pub fn restore_attachments(&mut self, attachments: Vec<ComposerAttachment>, cx: &mut Context<Self>) {
        if attachments.is_empty() {
            return;
        }
        self.attachments.extend(attachments.iter().cloned());
        cx.emit(ComposerEvent::AttachmentsAdded(attachments));
        self.schedule_save(cx);
        cx.notify();
    }

    /// Adds attachments and inserts their chips at the caret.
    pub fn add_attachments(&mut self, attachments: Vec<ComposerAttachment>, cx: &mut Context<Self>) {
        if attachments.is_empty() {
            return;
        }
        // Images get chips only when the message already has text; files always do.
        let image_chips = crate::composer_editor_mentions::has_text_besides_context(self.editor.read(cx).text());
        let chips: String = attachments
            .iter()
            .filter_map(|a| {
                let kind = match a.kind {
                    ComposerAttachmentKind::Image { .. } if image_chips => "image",
                    ComposerAttachmentKind::Image { .. } => return None,
                    ComposerAttachmentKind::File { .. } | ComposerAttachmentKind::PastedText { .. } => "file",
                };
                Some(format!("{} ", format_context_reference(kind, &a.context_id, &a.name)))
            })
            .collect();
        if !chips.is_empty() {
            self.editor.update(cx, |e, cx| e.insert(&chips, cx));
        }
        self.attachments.extend(attachments.iter().cloned());
        cx.emit(ComposerEvent::AttachmentsAdded(attachments));
        cx.notify();
    }

    /// Marks an upload finished or failed.
    pub fn set_upload_state(&mut self, context_id: &str, upload: UploadState, cx: &mut Context<Self>) {
        if let Some(a) = self.attachments.iter_mut().find(|a| a.context_id.as_ref() == context_id) {
            a.upload = upload;
            cx.notify();
        }
    }

    /// Drops an attachment and its chips; the owner releases its upload.
    pub fn remove_attachment(&mut self, context_id: SharedString, cx: &mut Context<Self>) {
        self.attachments.retain(|a| a.context_id != context_id);
        let text = self.editor.read(cx).text().to_owned();
        let needle = format!("t3-context://v1/");
        if let Some(pos) = text.find(&format!("/{context_id})")).filter(|_| text.contains(&needle)) {
            let start = text[..pos].rfind('[').map(|s| if s > 0 && text.as_bytes()[s - 1] == b'!' { s - 1 } else { s });
            let end = pos + context_id.len() + 2;
            if let Some(start) = start {
                let end = if text.as_bytes().get(end) == Some(&b' ') { end + 1 } else { end };
                self.editor.update(cx, |e, cx| e.replace_range(start..end, "", cx));
            }
        }
        cx.emit(ComposerEvent::AttachmentRemoved(context_id));
        cx.notify();
    }

    // ---- Shortcut commands (`composer.*`) ----

    /// Hands the composer the branch strip drawn under it, whose menus the host, workspace and
    /// branch shortcuts open.
    pub fn set_context_controls(&mut self, strip: Option<gpui_kit::WeakEntity<crate::components::branch_toolbar::BranchToolbar>>) {
        self.context_controls = strip;
    }

    /// The timeline Page Up/Down scroll once the prompt cannot (`onPageScrollKeyDown`).
    pub fn set_page_target(&mut self, timeline: gpui_kit::WeakEntity<super::messages_timeline::MessagesTimeline>) {
        self.page_scroller = Some(super::composer_page_scroll::PageScroller::new(timeline));
    }

    fn with_strip<R>(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut crate::components::branch_toolbar::BranchToolbar, &mut Context<crate::components::branch_toolbar::BranchToolbar>) -> R) -> Option<R> {
        self.context_controls.as_ref()?.upgrade().map(|strip| strip.update(cx, f))
    }

    /// `composer.host`: opens the machine menu. False when the strip shows no machine picker.
    pub fn open_host(&mut self, cx: &mut Context<Self>) -> bool {
        self.with_strip(cx, |strip, cx| strip.open_host_menu(cx)).unwrap_or(false)
    }

    /// `composer.cycleHost`: moves a draft to its project's next machine.
    pub fn cycle_host(&mut self, cx: &mut Context<Self>) -> bool {
        self.with_strip(cx, |strip, cx| strip.cycle_environment(cx)).unwrap_or(false)
    }

    /// `composer.workspace`: opens the workspace menu.
    pub fn open_workspace(&mut self, cx: &mut Context<Self>) -> bool {
        self.with_strip(cx, |strip, cx| strip.open_workspace_menu(cx)).unwrap_or(false)
    }

    /// `composer.branch`: opens the branch picker.
    pub fn open_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(strip) = self.context_controls.as_ref().and_then(|s| s.upgrade()) else { return false };
        strip.update(cx, |strip, cx| strip.open_branch_menu(window, cx))
    }

    /// `composer.previousWorktree`: a draft takes the project's previous worktree, then the
    /// composer keeps focus (`onComposerFocusRequest`).
    pub fn use_previous_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let used = self.with_strip(cx, |strip, cx| strip.use_previous_worktree(cx)).unwrap_or(false);
        if used {
            window.focus(&self.editor.focus_handle(cx), cx);
        }
        used
    }

    /// `composer.effort`: opens the model picker with the keyboard on the selected effort level;
    /// an open picker moves there instead of closing.
    pub fn open_effort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_none() {
            self.open_picker(window, cx);
        }
        if let Some((picker, _)) = &self.picker {
            picker.update(cx, |p, cx| p.focus_effort(cx));
        }
    }

    /// `composer.mode`: opens the permissions chip's menu, or, while the chip is hidden, the
    /// model picker's permissions menu (`pickerOwnsModeShortcut`).
    pub fn open_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chip_shown = self.state.runtime_mode != self.state.default_runtime_mode && !self.chip_hidden;
        if chip_shown {
            self.close_picker(window, cx);
            self.permissions_menu = Some(None);
            return cx.notify();
        }
        if self.picker.is_none() {
            self.open_picker(window, cx);
        }
        if let Some((picker, _)) = &self.picker {
            picker.update(cx, |p, cx| p.open_permissions(cx));
        }
    }

    /// `composer.increaseEffort` / `composer.decreaseEffort`: the next effort level in catalog
    /// order, wrapping (`getReasoningTraitStep`). False when the model has under two levels.
    pub fn step_effort(&mut self, direction: EffortStep, cx: &mut Context<Self>) -> bool {
        let selection = &self.state.selection;
        let Some(descriptor) = self.state.catalog.model(&selection.instance_id, &selection.model).and_then(|m| m.effort.as_ref()) else {
            return false;
        };
        let Some(next) = next_effort(&descriptor.options, selection.effort.as_deref(), direction) else { return false };
        self.state.selection.effort = Some(next);
        cx.emit(ComposerEvent::ModelChanged(self.state.selection.clone()));
        if let Some((picker, _)) = &self.picker {
            let (c, s, m) = (self.state.catalog.clone(), self.state.selection.clone(), self.state.runtime_mode);
            picker.update(cx, |p, cx| p.update_state(c, s, m, cx));
        }
        cx.notify();
        true
    }

    /// `composer.dictation`: starts dictation. With no transcription destination ready the fork
    /// leaves the key alone, so this returns false and the key falls through.
    pub fn toggle_dictation(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.state.voice_available || self.state.project_required {
            return false;
        }
        cx.emit(ComposerEvent::VoiceRequested);
        true
    }

    /// Runs a command once per press: a held chord repeats the action, which the fork ignores.
    fn once<A: gpui_kit::Action>(&mut self, cx: &mut Context<Self>, run: impl FnOnce(&mut Self, &mut Context<Self>) -> bool) {
        let id = std::any::TypeId::of::<A>();
        if self.held_command == Some(id) {
            return;
        }
        if run(self, cx) {
            self.held_command = Some(id);
        } else {
            cx.propagate();
        }
    }

    /// Registers the `composer.*` commands on the composer's key context.
    fn command_handlers(&self, root: gpui_kit::Stateful<gpui_kit::Div>, cx: &mut Context<Self>) -> gpui_kit::Stateful<gpui_kit::Div> {
        use crate::shortcuts::commands as k;
        root.on_action(cx.listener(|this, _: &k::OpenComposerHost, _, cx| this.once::<k::OpenComposerHost>(cx, |t, cx| t.open_host(cx))))
            .on_action(cx.listener(|this, _: &k::CycleComposerHost, _, cx| this.once::<k::CycleComposerHost>(cx, |t, cx| t.cycle_host(cx))))
            .on_action(cx.listener(|this, _: &k::OpenComposerWorkspace, _, cx| this.once::<k::OpenComposerWorkspace>(cx, |t, cx| t.open_workspace(cx))))
            .on_action(cx.listener(|this, _: &k::OpenComposerBranch, window, cx| {
                this.once::<k::OpenComposerBranch>(cx, |t, cx| t.open_branch(window, cx))
            }))
            .on_action(cx.listener(|this, _: &k::UsePreviousWorktree, window, cx| {
                this.once::<k::UsePreviousWorktree>(cx, |t, cx| t.use_previous_worktree(window, cx))
            }))
            .on_action(cx.listener(|this, _: &k::OpenComposerEffort, window, cx| {
                this.once::<k::OpenComposerEffort>(cx, |t, cx| {
                    t.open_effort(window, cx);
                    true
                })
            }))
            .on_action(cx.listener(|this, _: &k::OpenComposerMode, window, cx| {
                this.once::<k::OpenComposerMode>(cx, |t, cx| {
                    t.open_mode(window, cx);
                    true
                })
            }))
            .on_action(cx.listener(|this, _: &k::IncreaseEffort, _, cx| {
                this.once::<k::IncreaseEffort>(cx, |t, cx| {
                    t.step_effort(EffortStep::Up, cx);
                    true
                })
            }))
            .on_action(cx.listener(|this, _: &k::DecreaseEffort, _, cx| {
                this.once::<k::DecreaseEffort>(cx, |t, cx| {
                    t.step_effort(EffortStep::Down, cx);
                    true
                })
            }))
            .on_action(cx.listener(|this, _: &k::ToggleDictation, _, cx| this.once::<k::ToggleDictation>(cx, |t, cx| t.toggle_dictation(cx))))
            .capture_action(cx.listener(|this, _: &k::StashComposer, window, cx| {
                // Taken in the capture phase, so the model picker's search cannot swallow it.
                cx.stop_propagation();
                this.once::<k::StashComposer>(cx, |t, cx| {
                    t.stash(window, cx);
                    true
                })
            }))
            // Any key release or modifier change ends the press that `once` remembered.
            .on_key_up(cx.listener(|this, _: &gpui_kit::KeyUpEvent, _, _| {
                this.held_command = None;
                if let Some(scroller) = this.page_scroller.as_mut() {
                    scroller.release();
                }
            }))
    }

    // ---- Captured keys ----

    fn capture_up(&mut self, _: &editor_mod::MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.stash_menu.is_some() {
            cx.stop_propagation();
            return self.step_stash_menu(false, cx);
        }
        if let Some(menu) = self.menu.as_mut() {
            menu.step(false);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.step_history(true, window, cx);
    }

    fn capture_down(&mut self, _: &editor_mod::MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.stash_menu.is_some() {
            cx.stop_propagation();
            return self.step_stash_menu(true, cx);
        }
        if let Some(menu) = self.menu.as_mut() {
            menu.step(true);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.step_history(false, window, cx);
    }

    fn step_history(&mut self, backward: bool, _: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let on_edge = if backward { editor.caret_on_first_row() } else { editor.caret_on_last_row() };
        if !on_edge || !self.attachments.is_empty() {
            return;
        }
        let Some(step) = step_prompt_history(backward, &self.state.history, self.history_position.as_ref(), editor.text()) else {
            return;
        };
        cx.stop_propagation();
        self.history_position = step.position;
        self.editor.update(cx, |e, cx| e.set_text(step.prompt, cx));
    }

    fn capture_enter(&mut self, _: &editor_mod::Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.read(cx).is_composing() {
            return;
        }
        if self.stash_menu.is_some() {
            cx.stop_propagation();
            return self.act_on_highlighted(true, window, cx);
        }
        if let Some(menu) = &self.menu
            && !menu.items.is_empty()
        {
            cx.stop_propagation();
            self.pick_menu_item(menu.active, window, cx);
        }
    }

    fn capture_tab(&mut self, _: &editor_mod::Tab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &self.menu
            && !menu.items.is_empty()
        {
            cx.stop_propagation();
            self.pick_menu_item(menu.active, window, cx);
        }
    }

    fn capture_escape(&mut self, _: &editor_mod::Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.stash_menu.is_some() {
            cx.stop_propagation();
            self.stash_menu = None;
            return cx.notify();
        }
        if self.picker.is_some() {
            cx.stop_propagation();
            self.close_picker(window, cx);
        } else if self.menu.is_some() {
            cx.stop_propagation();
            self.dismiss_menu(cx);
        }
    }

    /// Mod+Backspace deletes the highlighted stash entry while the drawer is open.
    fn capture_delete_line(&mut self, _: &editor_mod::DeleteToLineStart, window: &mut Window, cx: &mut Context<Self>) {
        if self.stash_menu.is_some() {
            cx.stop_propagation();
            self.act_on_highlighted(false, window, cx);
        }
    }

    fn capture_delete_word(&mut self, _: &editor_mod::DeleteWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.stash_menu.is_some() && !cfg!(target_os = "macos") {
            cx.stop_propagation();
            self.act_on_highlighted(false, window, cx);
        }
    }

    fn on_modifiers(&mut self, event: &ModifiersChangedEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.held_command = None;
        let held = secondary(&event.modifiers);
        if held != self.modifier_held {
            self.modifier_held = held;
            cx.notify();
        }
    }

    /// Sets the held-modifier state directly (scripted checks of the steer/queue flip).
    pub fn set_modifier_held(&mut self, held: bool, cx: &mut Context<Self>) {
        self.modifier_held = held;
        cx.notify();
    }
}

/// Which way `composer.increaseEffort` / `composer.decreaseEffort` steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffortStep {
    Up,
    Down,
}

/// The level after `current` (or the default) in catalog order, wrapping; `None` with fewer
/// than two levels.
fn next_effort(options: &[crate::model_selection::OptionChoice], current: Option<&str>, step: EffortStep) -> Option<SharedString> {
    if options.len() < 2 {
        return None;
    }
    let ordered: Vec<_> = match step {
        EffortStep::Up => options.iter().collect(),
        EffortStep::Down => options.iter().rev().collect(),
    };
    let position = current
        .and_then(|c| ordered.iter().position(|o| o.id.as_ref() == c))
        .or_else(|| ordered.iter().position(|o| o.is_default));
    let next = match position {
        Some(at) => ordered[(at + 1) % ordered.len()],
        None => ordered[0],
    };
    Some(next.id.clone())
}

fn secondary(modifiers: &Modifiers) -> bool {
    if cfg!(target_os = "macos") { modifiers.platform } else { modifiers.control }
}

// ---- Rendering ----

/// The quiet square icon action: send, attach, previous question.
fn icon_action(id: &'static str, glyph: IconName, size: Pixels, color: Hsla, disabled: bool, c: &ComposerColors) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = c.row_hover;
    let fg = c.foreground;
    let active = c.accent;
    div()
        .id(id)
        .size(c.r(CONTROL_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(c.r(ICON_ACTION_RADIUS))
        .when(disabled, |el| el.opacity(0.35))
        .when(!disabled, |el| el.cursor_pointer().hover(move |s| s.bg(hover).text_color(fg)).active(move |s| s.bg(active)))
        .text_color(color)
        .child(Icon::new(glyph).size(IconSize::Custom(size)).color(color).stroke(composer_icon_stroke(glyph)))
}

/// A composer control (`ComposerControl` size `sm`).
fn control(id: impl Into<gpui_kit::ElementId>, open: bool, c: &ComposerColors) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = c.row_hover;
    let fg = c.foreground;
    div()
        .id(id)
        .h(c.r(CONTROL_HEIGHT))
        .px(c.r(px(8.)))
        .flex()
        .flex_none()
        .items_center()
        .gap(c.r(px(6.)))
        .rounded(c.r(CONTROL_RADIUS))
        .cursor_pointer()
        .text_size(c.r(px(14.)))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if open { c.foreground } else { c.foreground.opacity(0.8) })
        .when(open, |el| el.bg(hover))
        .hover(move |s| s.bg(hover).text_color(fg))
}

/// The rising effort bars (`EffortGlyph`), 4px pitch, 14px tall.
fn effort_glyph(glyph: &EffortGlyph, color: Hsla) -> impl IntoElement {
    let height = 14.;
    let max_bar = height - 2.;
    let heights = effort_bar_heights(glyph.count, max_bar);
    div()
        .flex_none()
        .w(px(4. * glyph.count as f32 - 2.))
        .h(px(height))
        .relative()
        .children(heights.into_iter().enumerate().map(|(i, h)| {
            div()
                .absolute()
                .left(px(i as f32 * 4.))
                .top(px(max_bar - h))
                .w(px(2.))
                .h(px(h))
                .bg(if i < glyph.filled { color } else { color.opacity(0.4) })
        }))
}

fn text_width(text: &str, size: Pixels, weight: FontWeight, window: &mut Window) -> Pixels {
    let mut f = gpui_kit::font(ui_font_family());
    f.weight = weight;
    let run = gpui_kit::TextRun { len: text.len(), font: f, color: gpui_kit::black(), background_color: None, underline: None, strikethrough: None };
    window.text_system().layout_line(text, size, &[run], None).width
}

/// How the footer fits: blocks after the model trigger first go icon-only, then into the overflow menu.
/// Ports `resolveRestingComposerControlsLayout` for the permissions block.
fn resolve_footer_layout(host: Pixels, picker: Pixels, block: Option<(Pixels, Pixels)>, overflow: Pixels, gap: Pixels) -> (bool, bool) {
    let Some((natural, icon_only)) = block else { return (false, false) };
    if picker + gap + natural + gap <= host {
        (false, false)
    } else if picker + gap + icon_only + gap <= host {
        (true, false)
    } else {
        let _ = overflow;
        (true, true)
    }
}

impl ChatComposer {
    fn render_model_trigger(&self, compact: bool, window: &mut Window, cx: &mut Context<Self>) -> (AnyElement, Pixels) {
        let c = ComposerColors::new(cx);
        let open = self.picker.is_some();
        let selection = &self.state.selection;
        let entry = self.state.catalog.instance(&selection.instance_id).cloned();
        let model = self.state.catalog.model(&selection.instance_id, &selection.model).cloned();
        let label: SharedString = model.as_ref().map_or(selection.model.clone(), |m| display_model_name(m, true));
        let glyph = model.as_ref().and_then(|m| m.effort.as_ref()).and_then(|d| resolve_effort_glyph(d, selection.effort.as_deref()));
        let suffix_color = if open { c.foreground } else { c.muted_foreground };
        let mark = entry.as_ref().map_or(ProviderIcon::Initials, |e| ProviderIcon::for_model(&e.driver_kind, Some(&selection.model)));
        let initials = entry.as_ref().map_or(SharedString::default(), |e| e.display_name.clone());
        let label_width = text_width(&label, c.r(px(14.)), FontWeight::MEDIUM, window).min(c.r(px(224.)));
        let suffix_width = match &glyph {
            Some(g) if g.special.is_none() => px(4. * g.count as f32 - 2.),
            Some(g) => text_width(g.special.as_deref().unwrap_or(""), c.r(px(14.)), FontWeight::MEDIUM, window).min(c.r(px(96.))),
            None => px(0.),
        };
        let natural = c.r(px(8.)) + c.r(px(16.)) + c.r(px(6.)) + label_width + if glyph.is_some() { c.r(px(6.)) + suffix_width } else { px(0.) } + c.r(px(8.));
        let speed_fast = selection.speed.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("fast") || s.eq_ignore_ascii_case("ultrafast"));
        let element = control("composer-model-trigger", open, &c)
            .min_w(c.r(px(52.)))
            .flex_shrink(1.)
            .when(compact, |el| el.text_size(c.r(px(12.))))
            .child(super::timeline_row_parts::composer_provider_icon(entry.as_ref(), &self.state.catalog, &selection.model, &c, cx))
            .child(div().min_w_0().max_w(c.r(px(224.))).overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
            .when_some(glyph, |el, glyph| match glyph.special.clone() {
                Some(name) => el.child(div().max_w(c.r(px(96.))).overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(suffix_color).child(name)),
                None => el.child(effort_glyph(&glyph, suffix_color)),
            })
            .when(speed_fast, |el| el.child(Icon::new(IconName::ZapFilled).size(IconSize::Custom(c.r(px(12.)))).color(suffix_color)))
            .when(selection.ultracode, |el| el.child(Icon::new(IconName::Workflow).size(IconSize::Custom(c.r(px(12.)))).color(suffix_color)))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx)))
            .into_any_element();
        (element, natural)
    }

    fn render_permissions_chip(&self, icon_only: bool, c: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.state.runtime_mode;
        control("composer-permissions-chip", false, c)
            .relative()
            .pl(c.r(px(6.)))
            .child(Icon::new(runtime_mode_icon(mode)).size(IconSize::Custom(c.r(px(16.)))).color(c.foreground.opacity(0.8)).stroke(composer_icon_stroke(runtime_mode_icon(mode))))
            .when(!icon_only, |el| el.child(mode.label()))
            .when(self.permissions_menu.is_some(), |el| el.bg(c.row_hover))
            .on_click(cx.listener(|this, _, _, cx| {
                this.permissions_menu = if this.permissions_menu.is_some() { None } else { Some(None) };
                cx.notify();
            }))
            .children(self.render_permissions_menu(c, cx))
            .into_any_element()
    }

    /// The chip's menu (`MenuPopup align="start"` above the chip) holding the permission modes.
    fn render_permissions_menu(&self, c: &ComposerColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.permissions_menu?;
        let panel = crate::components::chat::model_picker::permissions_panel(
            self.state.runtime_mode,
            preview,
            *c,
            cx,
            |this: &mut Self, mode| this.permissions_menu = Some(mode),
            |this, mode, cx| {
                this.permissions_menu = None;
                if this.state.runtime_mode != mode {
                    this.state.runtime_mode = mode;
                    cx.emit(ComposerEvent::RuntimeModeChanged(mode));
                }
            },
            |this| this.permissions_menu = None,
        );
        Some(deferred(div().absolute().bottom_full().left_0().pb(c.r(px(4.))).child(panel)).with_priority(3).into_any_element())
    }

    fn render_primary_actions(&self, c: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let has_content = self.has_sendable_content(cx);
        let stop_visible = (self.state.can_interrupt || matches!(self.stop_phase, StopPhase::Pending | StopPhase::TimedOut)) && !has_content;
        let mut row = div().flex().flex_none().items_center().gap(c.r(px(4.)));
        if self.state.voice_available {
            row = row.child(
                icon_action("composer-voice", IconName::Mic, c.r(px(16.)), c.muted_foreground, false, c)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::VoiceRequested))),
            );
        }
        if stop_visible {
            let disabled = self.stop_phase == StopPhase::Pending;
            let hover = c.row_hover;
            let fg = c.foreground;
            return row
                .child(
                    div()
                        .id("composer-stop")
                        .size(c.r(CONTROL_HEIGHT))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(c.r(ICON_ACTION_RADIUS))
                        .when(disabled, |el| el.opacity(0.64))
                        .when(!disabled, |el| el.cursor_pointer().hover(move |s| s.bg(hover).text_color(fg)))
                        .child(Icon::new(IconName::Square).size(IconSize::Custom(c.r(px(14.)))).color(c.muted_foreground.opacity(0.7)).stroke(2.2))
                        .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                )
                .into_any_element();
        }
        let disabled = !has_content
            || self.state.send_disabled_reason.is_some()
            || self.uploads_pending()
            || self.stop_phase == StopPhase::Landed;
        let glyph = if self.state.is_send_busy {
            IconName::LoaderCircle
        } else if self.state.running && self.running_dispatch() == FollowUpBehavior::Queue {
            IconName::ListPlus
        } else if self.state.running {
            IconName::CornerUpRight
        } else {
            IconName::CornerDownLeft
        };
        row.child(
            icon_action("composer-send", glyph, c.r(px(14.)), c.secondary_label, disabled, c)
                .when(!disabled, |el| el.on_click(cx.listener(|this, _, _, cx| this.submit(ComposerSubmissionIntent::Foreground, cx)))),
        )
        .into_any_element()
    }

    fn render_attachments(&self, c: &ComposerColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() {
            return None;
        }
        let tiles = self.attachments.iter().filter_map(|a| {
            let ComposerAttachmentKind::Image { bytes, mime } = &a.kind else { return None };
            let id = a.context_id.clone();
            let format = match mime.as_ref() {
                "image/jpeg" => gpui_kit::ImageFormat::Jpeg,
                "image/gif" => gpui_kit::ImageFormat::Gif,
                "image/webp" => gpui_kit::ImageFormat::Webp,
                _ => gpui_kit::ImageFormat::Png,
            };
            let image = Arc::new(gpui_kit::Image::from_bytes(format, bytes.to_vec()));
            Some(
                div()
                    .id(SharedString::from(format!("tile-{id}")))
                    .relative()
                    .size(c.r(IMAGE_TILE))
                    .rounded(c.r(px(8.)))
                    .border_1()
                    .border_color(c.border.opacity(0.8))
                    .overflow_hidden()
                    .bg(c.muted)
                    .child(img(image).size_full().object_fit(gpui_kit::ObjectFit::Cover))
                    .when(a.upload == UploadState::Uploading, |el| {
                        el.child(div().absolute().inset_0().bg(hsla(0., 0., 0., 0.35)).flex().items_end().justify_center().pb(c.r(px(4.))).text_size(c.r(px(10.))).text_color(gpui_kit::white()).child("Uploading"))
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("tile-remove-{id}")))
                            .absolute()
                            .top(c.r(px(4.)))
                            .right(c.r(px(4.)))
                            .size(c.r(px(18.)))
                            .rounded_full()
                            .bg(hsla(0., 0., 0., 0.6))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child(Icon::new(IconName::X).size(IconSize::Custom(c.r(px(12.)))).color(gpui_kit::white()))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_attachment(id.clone(), cx))),
                    ),
            )
        });
        let files = self.attachments.iter().filter(|a| !matches!(a.kind, ComposerAttachmentKind::Image { .. })).map(|a| {
            let glyph = if matches!(a.kind, ComposerAttachmentKind::PastedText { .. }) { IconName::FileText } else { IconName::File };
            div()
                .flex()
                .items_center()
                .gap(c.r(px(8.)))
                .py(c.r(px(4.)))
                .text_size(c.r(px(14.)))
                .text_color(c.foreground)
                .child(Icon::new(glyph).size(IconSize::Custom(c.r(px(16.)))).color(c.muted_foreground).stroke(composer_icon_stroke(glyph)))
                .child(a.name.clone())
                .child(div().text_size(c.r(px(12.))).text_color(c.secondary_label).child(format_size(a.size_bytes)))
        });
        let tiles: Vec<_> = tiles.collect();
        let files: Vec<_> = files.collect();
        Some(
            div()
                .flex()
                .flex_col()
                .when(!tiles.is_empty(), |el| el.child(div().mb(c.r(px(12.))).flex().flex_wrap().gap(c.r(px(8.))).children(tiles)))
                .when(!files.is_empty(), |el| el.child(div().mb(c.r(px(12.))).flex().flex_col().gap(c.r(px(4.))).children(files)))
                .into_any_element(),
        )
    }

    fn render_banners(&self, c: &ComposerColors, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let badge = self.render_stash_badge(c, cx);
        if self.state.banners.is_empty() && badge.is_none() {
            return None;
        }
        // `ComposerBanner.Scroll`: `max-h-[min(24rem,40dvh)]`.
        let max_body = c.r(px(384.)).min(window.viewport_size().height * 0.4);
                let cards = self.state.banners.iter().enumerate().map(|(index, banner)| {
            // `variantColors`: warning and error cards take a tinted outline and an 8% wash.
            // `ComposerBanner.Icon` is muted whatever the tone.
            let ink = c.muted_foreground;
            let (tint, outline) = match banner.tone {
                BannerTone::Neutral => (None, c.outline),
                BannerTone::Warning => (Some(c.warning), c.warning.opacity(0.28)),
                BannerTone::Error => (Some(c.error), c.error.opacity(0.32)),
            };
            // `min-h-(--composer-banner-icon-column)`: spacing(6) from `sm` up.
            let row = div()
                .id(SharedString::from(format!("banner-{}", banner.id)))
                .min_h(c.r(px(24.)))
                .flex()
                .items_center()
                .gap(c.r(px(4.)))
                .rounded(c.r(px(12.)))
                .child(div().w(c.r(px(24.))).flex().justify_center().child(Icon::new(banner.icon).size(IconSize::Custom(c.r(px(14.)))).color(ink).stroke(composer_icon_stroke(banner.icon))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_baseline()
                        .gap(c.r(px(4.)))
                        .whitespace_nowrap()
                        .text_size(c.r(px(12.)))
                        .line_height(c.r(px(16.)))
                        .child(div().min_w_0().truncate().font_weight(gpui_kit::FontWeight::MEDIUM).text_color(c.foreground).child(banner.title.clone()))
                        .children(banner.description.clone().map(|description| {
                            div().w(px(0.)).min_w_0().flex_grow(1.).truncate().text_color(c.muted_foreground).child(description)
                        })),
                )
                .when_some(banner.action.clone(), |el, action| {
                    let id = banner.id.clone();
                    el.child(div().id(SharedString::from(format!("banner-action-{id}"))).px(c.r(px(8.))).h(c.r(px(24.))).flex().items_center().rounded(c.r(px(8.))).text_size(c.r(px(12.))).font_weight(FontWeight::MEDIUM).text_color(c.foreground).cursor_pointer().hover({
                        let h = c.foreground.opacity(0.05);
                        move |s| s.bg(h)
                    }).on_click(cx.listener(move |_, _, _, cx| cx.emit(ComposerEvent::BannerAction(id.clone())))).child(action))
                })
                .when_some(banner.secondary_action.clone(), |el, action| {
                    let id = SharedString::from(format!("{}#secondary", banner.id));
                    el.child(div().id(SharedString::from(format!("banner-action-{id}"))).px(c.r(px(8.))).h(c.r(px(24.))).flex().items_center().rounded(c.r(px(8.))).text_size(c.r(px(12.))).font_weight(FontWeight::MEDIUM).text_color(c.foreground).cursor_pointer().hover({
                        let h = c.foreground.opacity(0.05);
                        move |s| s.bg(h)
                    }).on_click(cx.listener(move |_, _, _, cx| cx.emit(ComposerEvent::BannerAction(id.clone())))).child(action))
                })
                .when(banner.dismiss.is_some(), |el| {
                    let id = banner.id.clone();
                    // `ComposerBanner.Actions min-w-12 justify-end` holding `ComposerBanner.Dismiss`.
                    el.child(
                        div().min_w(c.r(px(48.))).flex().justify_end().child(
                            Button::new(SharedString::from(format!("banner-dismiss-{id}")))
                                .icon(IconName::X)
                                .icon_size(IconSize::Custom(c.r(px(14.))))
                                .size(ButtonSize::IconXs)
                                .variant(ButtonVariant::Ghost)
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ComposerEvent::BannerDismissed(id.clone())))),
                        ),
                    )
                });
            // `ComposerBanner.Children`: the body under the row.
            let body = banner.body.as_ref().map(|body| self.render_usage_limits_body(&banner.id, body, c, max_body, cx));
            // Each notice is its own attached card (`NoticeCard`); a card below another joins it
            // edge to edge, square-topped with no top border. The web tucks each card 1rem under the
            // next one and masks that part away; here a card simply ends where the next begins.
            let joined = index > 0;
            div()
                .relative()
                .w_full()
                .pb(c.r(px(5.)))
                .px(c.r(px(4.)) - BORDER)
                .pt(c.r(px(5.)) - if joined { px(0.) } else { BORDER })
                .when(!joined, |el| el.rounded_t(c.r(SURFACE_RADIUS)))
                .glass(c.glass_surface)
                .border_1()
                .border_b_0()
                .when(joined, |el| el.border_t_0())
                .border_color(outline)
                .when_some(tint, |el, t| {
                    // The web's wash spans the border box, so the outline blends over it too.
                    el.child(
                        div()
                            .absolute()
                            .top(if joined { px(0.) } else { -BORDER })
                            .left(-BORDER)
                            .right(-BORDER)
                            .bottom(px(0.))
                            .when(!joined, |d| d.rounded_t(c.r(SURFACE_RADIUS)))
                            .bg(t.opacity(0.08)),
                    )
                })
                .child(div().relative().flex().flex_col().child(row).children(body))
        });
        let cards: Vec<_> = cards.collect();
        // `ComposerBanner.Dock`: the notices' column and the stash badge, bottoms aligned.
        Some(
            div()
                .mx(c.r(DRAWER_INSET))
                .flex()
                .items_end()
                .gap(c.r(px(4.)))
                .when(!cards.is_empty(), |el| el.child(div().flex().flex_1().min_w_0().flex_col().children(cards)))
                .when_some(badge, |el, badge| el.child(div().ml_auto().child(badge)))
                .into_any_element(),
        )
    }

    fn render_context_strip(&self, c: &ComposerColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let strip = self.state.context_strip.clone()?;
        let item = |id: &'static str, glyph: IconName, label: SharedString, control: ContextStripControl, cx: &mut Context<Self>| {
            let hover = c.row_hover;
            let fg = c.foreground;
            div()
                .id(id)
                .h(c.r(px(24.)))
                .px(c.r(px(7.)))
                .flex()
                .items_center()
                .gap(c.r(px(4.)))
                .rounded(c.r(CONTROL_RADIUS))
                .cursor_pointer()
                .text_size(c.r(px(12.)))
                .text_color(if c.is_dark() { c.muted_foreground } else { c.foreground.opacity(0.7) })
                .hover(move |s| s.bg(hover).text_color(fg))
                .child(Icon::new(glyph).size(IconSize::Custom(c.r(px(14.)))).color(c.muted_foreground).stroke(composer_icon_stroke(glyph)))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    // The strip's controls open what the branch strip's would.
                    let opened = match control {
                        ContextStripControl::Environment => this.open_host(cx),
                        ContextStripControl::Workspace => this.open_workspace(cx),
                        ContextStripControl::Branch => this.open_branch(window, cx),
                    };
                    if !opened {
                        cx.emit(ComposerEvent::ContextStripClicked(control));
                    }
                }))
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(c.r(px(4.)))
                .px(c.r(px(8.)))
                .pt(c.r(px(8.)))
                .child(item("ctx-environment", IconName::Monitor, strip.environment, ContextStripControl::Environment, cx))
                .child(item("ctx-workspace", IconName::FolderGit2, strip.workspace, ContextStripControl::Workspace, cx))
                .child(item("ctx-branch", IconName::GitBranch, strip.branch, ContextStripControl::Branch, cx))
                .into_any_element(),
        )
    }
}

fn format_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / 1024. / 1024.),
        b if b >= 1024 => format!("{:.0} KB", b as f64 / 1024.),
        b => format!("{b} B"),
    }
}

impl Render for ChatComposer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = ComposerColors::new(cx);
        let compact = self.footer_width < c.r((FOOTER_COMPACT_BREAKPOINT));
        let disabled = self.state.project_required;
        self.editor.update(cx, |e, _| e.disabled = disabled);

        let (trigger, trigger_width) = self.render_model_trigger(compact, window, cx);
        let show_chip = self.state.runtime_mode != self.state.default_runtime_mode;
        let chip_widths = show_chip.then(|| {
            let label = text_width(&self.state.runtime_mode.label(), c.r(px(14.)), FontWeight::MEDIUM, window);
            (c.r(px(6.)) + c.r(px(16.)) + c.r(px(6.)) + label + c.r(px(8.)), c.r(px(6.)) + c.r(px(16.)) + c.r(px(8.)))
        });
        // Host: footer minus padding, attach (28 + 8) and primary actions.
        let actions_width = c.r((CONTROL_HEIGHT)) * if self.state.voice_available { 2. } else { 1. } + c.r(px(4.));
        let host = self.footer_width - c.r((FOOTER_PADDING_X)) * 2. - (c.r((CONTROL_HEIGHT)) + c.r(px(8.))) - actions_width + c.r(px(7.));
        let (chip_icon_only, chip_hidden) = resolve_footer_layout(host, trigger_width, chip_widths, c.r((CONTROL_HEIGHT)), c.r(px(2.)));
        self.chip_hidden = chip_hidden;
        let mut controls = div()
            .relative()
            .mr(c.r(px(-7.)))
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_end()
            .gap(c.r(px(2.)))
            .child(trigger);
        if show_chip && !chip_hidden {
            controls = controls.child(self.render_permissions_chip(chip_icon_only, &c, cx));
        }
        if show_chip && chip_hidden {
            controls = controls.child(
                icon_action("composer-overflow", IconName::Ellipsis, c.r(px(16.)), c.muted_foreground, false, &c)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx))),
            );
        }
        if let Some((picker, _)) = &self.picker {
            controls = controls.child(
                deferred(
                    div()
                        .absolute()
                        .bottom(c.r((CONTROL_HEIGHT)) + c.r(px(4.)))
                        .right(px(0.))
                        // Cached at its fixed size: composer keystrokes reuse the picker's last frame.
                        .child(picker.clone().cached(gpui_kit::StyleRefinement::default().w(model_picker_size(false, &c).width).h(model_picker_size(false, &c).height)))
                        .child(input_handler_canvas(picker.clone())),
                )
                .with_priority(1),
            );
        }

        let entity = cx.entity();
        let measure = canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, cx| {
                    if (this.footer_width - bounds.size.width).abs() > px(0.5) {
                        this.footer_width = bounds.size.width;
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        let footer = div()
            .relative()
            .flex()
            .flex_nowrap()
            .items_center()
            .min_w_0()
            .px(c.r(FOOTER_PADDING_X))
            .pb(c.r(FOOTER_PADDING_BOTTOM))
            .child(measure)
            .child(
                div().mr(c.r(px(8.))).child(
                    div()
                        .relative()
                        .left(px(0.5))
                        .top(px(1.))
                        .child(
                            icon_action("composer-attach", IconName::Paperclip, c.r(px(14.)), c.secondary_label, disabled, &c)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::AttachRequested))),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_nowrap()
                    .items_center()
                    .justify_end()
                    .gap(c.r(px(4.)))
                    .child(controls)
                    .child(self.render_primary_actions(&c, cx)),
            );

        let editor_element = ComposerPromptEditor::element(&self.editor, cx);
        let editor_focus = self.editor.focus_handle(cx);
        let editor_box = self.editor.update(cx, |_, ecx| ComposerPromptEditor::actions(div().track_focus(&editor_focus), ecx));
        let body = div()
            .relative()
            .px(c.r(BODY_PADDING_X))
            .pt(c.r(BODY_PADDING_TOP))
            .pb(c.r(BODY_PADDING_BOTTOM))
            .children(self.render_attachments(&c, cx))
            .child(editor_box.m(c.r(px(-4.))).p(c.r(px(4.))).child(editor_element))
            .when_some(self.state.validation.clone(), |el, message| {
                el.child(div().pt(c.r(px(6.))).text_size(c.r(px(12.))).text_color(c.error_foreground).child(message))
            });

        let card = div()
            .id("composer-card")
            .relative()
            .w_full()
            .rounded(c.r(SURFACE_RADIUS))
            .glass(c.glass_surface)
            .border_1()
            .border_color(c.outline)
            .when(!c.is_dark(), |el| {
                // `shadow-xs` (0 1px 2px).
                el.shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.05), offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])
            })
            .when(c.is_dark(), |el| {
                el.shadow(vec![BoxShadow { color: c.highlight, offset: point(px(0.), px(1.)), blur_radius: px(0.), spread_radius: px(0.), inset: true }])
            })
            .when(disabled, |el| el.opacity(0.75))
            .drag_over::<ExternalPaths>({
                let (accent, ring) = (c.accent.opacity(0.45), c.primary.opacity(0.7));
                move |style, _, _, _| style.bg(accent).border_color(ring)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach(PastedContent::Files(paths.paths().to_vec()), cx);
            }))
            .child(body)
            .child(footer);

        let stash_menu = if self.menu.is_none() { self.render_stash_menu(&c, window, cx) } else { None };
        let menu = self.menu.clone().map(|menu| {
            let pick = cx.entity();
            let hover = cx.entity();
            div()
                .absolute()
                .left(c.r(DRAWER_INSET))
                .right(c.r(DRAWER_INSET))
                .bottom(relative(1.))
                .mb(-c.r(ATTACHMENT_OVERLAP))
                .pb(c.r(ATTACHMENT_OVERLAP))
                .rounded_t(c.r(SURFACE_RADIUS))
                .glass(c.glass_surface)
                .border_1()
                .border_color(c.outline)
                .shadow(popup_shadow(&c))
                .child(render_menu(
                    &menu,
                    &c,
                    move |index, window, cx| pick.update(cx, |this, cx| this.pick_menu_item(index, window, cx)),
                    move |index, _, cx| {
                        hover.update(cx, |this, cx| {
                            if let Some(menu) = this.menu.as_mut()
                                && menu.active != index
                            {
                                menu.active = index;
                                cx.notify();
                            }
                        })
                    },
                    cx,
                ))
        });

        let root = div()
            .id("chat-composer")
            .key_context("ChatComposer")
            .on_action(cx.listener(|this, _: &crate::shortcuts::commands::ToggleModelPicker, window, cx| this.toggle_picker(window, cx)));
        self.command_handlers(root, cx)
            .font_family(ui_font_family())
            .relative()
            .w_full()
            .when_some(super::chat_style::ChatLane::get(cx).composer, |d, w| d.max_w(w))
            .mx_auto()
            .flex()
            .flex_col()
            .capture_action(cx.listener(Self::capture_up))
            .capture_action(cx.listener(Self::capture_down))
            .capture_action(cx.listener(Self::capture_enter))
            .capture_action(cx.listener(Self::capture_tab))
            .capture_action(cx.listener(Self::capture_escape))
            .capture_action(cx.listener(Self::capture_delete_line))
            .capture_action(cx.listener(Self::capture_delete_word))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .children(self.render_banners(&c, window, cx))
            .children(self.render_reset_credit_dialog(&c, cx))
            .child(div().relative().w_full().children(menu).children(stash_menu).child(card))
            .children(self.render_context_strip(&c, cx))
    }
}
