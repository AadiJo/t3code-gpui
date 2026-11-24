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
    ComposerCommandAction, ComposerMenuSource, ComposerMenuState, SlashCommand, builtin_slash_items, render_menu,
};
use crate::components::chat::composer_icons::{composer_icon_stroke, provider_icon};
use crate::components::chat::composer_permissions::{RuntimeMode, runtime_mode_icon};
use crate::components::chat::composer_prompt_history::{PromptHistoryEntry, PromptHistoryPosition, step_prompt_history};
use crate::components::chat::composer_theme::{
    ATTACHMENT_OVERLAP, BODY_PADDING_BOTTOM, BODY_PADDING_TOP, BODY_PADDING_X, COMPOSER_MAX_WIDTH, CONTROL_HEIGHT,
    CONTROL_RADIUS, ComposerColors, DRAWER_INSET, FOOTER_COMPACT_BREAKPOINT, FOOTER_PADDING_BOTTOM, FOOTER_PADDING_X,
    ICON_ACTION_RADIUS, SURFACE_RADIUS, ui_font_family,
};
use crate::components::chat::model_picker::{MODEL_PICKER_SIZE, ModelPicker, ModelPickerEvent, input_handler_canvas, popup_shadow};
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
    attachments: Vec<ComposerAttachment>,
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
        self.history_position = None;
        self.menu = None;
        self.editor.update(cx, |editor, cx| editor.set_text(draft.prompt, cx));
    }

    fn current_draft(&self, cx: &App) -> ComposerDraft {
        ComposerDraft { prompt: self.editor.read(cx).text().to_owned(), attachments: self.attachments.clone(), model: None }
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
                self.refresh_menu(cx);
                self.schedule_save(cx);
                cx.notify();
            }
            ComposerEditorEvent::SelectionChanged => self.refresh_menu(cx),
            ComposerEditorEvent::Submit(intent) => self.submit(*intent, cx),
            ComposerEditorEvent::Tab | ComposerEditorEvent::FocusChanged(_) => {}
            ComposerEditorEvent::Escape => {
                if self.menu.is_some() {
                    self.dismiss_menu(cx);
                }
            }
            ComposerEditorEvent::PageScroll { down } => cx.emit(ComposerEvent::PageScroll { down: *down }),
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
        let (mut items, loading) = self.menu_source.items(&trigger);
        if trigger.kind == ComposerTriggerKind::SlashCommand {
            let mut builtins = builtin_slash_items(&trigger.query);
            builtins.append(&mut items);
            items = builtins;
        }
        let active = self.menu.as_ref().filter(|m| m.trigger.range.start == trigger.range.start).map_or(0, |m| m.active);
        self.menu = Some(ComposerMenuState { active: active.min(items.len().saturating_sub(1)), trigger, items: Arc::new(items), loading });
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
        let trigger: ComposerTrigger = menu.trigger;
        let text = self.editor.read(cx).text().to_owned();
        let replace = |this: &mut Self, replacement: String, cx: &mut Context<Self>| {
            let end = extend_replacement_range_for_trailing_space(&text, trigger.range.end, &replacement);
            this.editor.update(cx, |e, cx| e.replace_range(trigger.range.start..end, &replacement, cx));
        };
        match item.action {
            ComposerCommandAction::Insert(insert) => replace(self, format!("{insert} "), cx),
            ComposerCommandAction::Slash(SlashCommand::Model) => {
                replace(self, String::new(), cx);
                self.open_picker(window, cx);
            }
            ComposerCommandAction::Slash(command) => {
                let word = match command {
                    SlashCommand::Side => "/side ",
                    SlashCommand::Btw => "/btw ",
                    _ => "/clear ",
                };
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
        self.attachments.iter().any(|a| a.upload != UploadState::Uploaded)
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
        if is_clear_command(&prompt) {
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
        let result = intake(candidates, self.attachments.len(), self.state.file_attachment_limit);
        if let Some(error) = result.error {
            crate::components::ui::toast::Toaster::push(
                crate::components::ui::toast::ToastData::new(crate::components::ui::toast::ToastKind::Error, error),
                cx,
            );
        }
        self.add_attachments(result.accepted, cx);
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

    // ---- Captured keys ----

    fn capture_up(&mut self, _: &editor_mod::MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.as_mut() {
            menu.step(false);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.step_history(true, window, cx);
    }

    fn capture_down(&mut self, _: &editor_mod::MoveDown, window: &mut Window, cx: &mut Context<Self>) {
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
        if self.picker.is_some() {
            cx.stop_propagation();
            self.close_picker(window, cx);
        } else if self.menu.is_some() {
            cx.stop_propagation();
            self.dismiss_menu(cx);
        }
    }

    fn on_modifiers(&mut self, event: &ModifiersChangedEvent, _: &mut Window, cx: &mut Context<Self>) {
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
        .size(CONTROL_HEIGHT)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(ICON_ACTION_RADIUS)
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
        .h(CONTROL_HEIGHT)
        .px(px(8.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .rounded(CONTROL_RADIUS)
        .cursor_pointer()
        .text_size(px(14.))
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
        let label_width = text_width(&label, px(14.), FontWeight::MEDIUM, window).min(px(224.));
        let suffix_width = match &glyph {
            Some(g) if g.special.is_none() => px(4. * g.count as f32 - 2.),
            Some(g) => text_width(g.special.as_deref().unwrap_or(""), px(14.), FontWeight::MEDIUM, window).min(px(96.)),
            None => px(0.),
        };
        let natural = px(8.) + px(16.) + px(6.) + label_width + if glyph.is_some() { px(6.) + suffix_width } else { px(0.) } + px(8.);
        let speed_fast = selection.speed.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("fast") || s.eq_ignore_ascii_case("ultrafast"));
        let element = control("composer-model-trigger", open, &c)
            .min_w(px(52.))
            .flex_shrink(1.)
            .when(compact, |el| el.text_size(px(12.)))
            .child(provider_icon(mark, px(16.), &initials, &c, cx))
            .child(div().min_w_0().max_w(px(224.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
            .when_some(glyph, |el, glyph| match glyph.special.clone() {
                Some(name) => el.child(div().max_w(px(96.)).overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(suffix_color).child(name)),
                None => el.child(effort_glyph(&glyph, suffix_color)),
            })
            .when(speed_fast, |el| el.child(Icon::new(IconName::ZapFilled).size(IconSize::Custom(px(12.))).color(suffix_color)))
            .when(selection.ultracode, |el| el.child(Icon::new(IconName::Workflow).size(IconSize::Custom(px(12.))).color(suffix_color)))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx)))
            .into_any_element();
        (element, natural)
    }

    fn render_permissions_chip(&self, icon_only: bool, c: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.state.runtime_mode;
        control("composer-permissions-chip", false, c)
            .pl(px(6.))
            .child(Icon::new(runtime_mode_icon(mode)).size(IconSize::Custom(px(16.))).color(c.foreground.opacity(0.8)).stroke(composer_icon_stroke(runtime_mode_icon(mode))))
            .when(!icon_only, |el| el.child(mode.label()))
            .on_click(cx.listener(|this, _, _, cx| {
                let next = this.state.runtime_mode.next();
                this.state.runtime_mode = next;
                cx.emit(ComposerEvent::RuntimeModeChanged(next));
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_primary_actions(&self, c: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let has_content = self.has_sendable_content(cx);
        let stop_visible = (self.state.can_interrupt || matches!(self.stop_phase, StopPhase::Pending | StopPhase::TimedOut)) && !has_content;
        let mut row = div().flex().flex_none().items_center().gap(px(4.));
        if self.state.voice_available {
            row = row.child(
                icon_action("composer-voice", IconName::Mic, px(16.), c.muted_foreground, false, c)
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
                        .size(CONTROL_HEIGHT)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(ICON_ACTION_RADIUS)
                        .when(disabled, |el| el.opacity(0.64))
                        .when(!disabled, |el| el.cursor_pointer().hover(move |s| s.bg(hover).text_color(fg)))
                        .child(Icon::new(IconName::Square).size(IconSize::Custom(px(14.))).color(c.muted_foreground.opacity(0.7)).stroke(2.2))
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
            icon_action("composer-send", glyph, px(14.), c.secondary_label, disabled, c)
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
                    .size(IMAGE_TILE)
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border.opacity(0.8))
                    .overflow_hidden()
                    .bg(c.muted)
                    .child(img(image).size_full().object_fit(gpui_kit::ObjectFit::Cover))
                    .when(a.upload == UploadState::Uploading, |el| {
                        el.child(div().absolute().inset_0().bg(hsla(0., 0., 0., 0.35)).flex().items_end().justify_center().pb(px(4.)).text_size(px(10.)).text_color(gpui_kit::white()).child("Uploading"))
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("tile-remove-{id}")))
                            .absolute()
                            .top(px(4.))
                            .right(px(4.))
                            .size(px(18.))
                            .rounded_full()
                            .bg(hsla(0., 0., 0., 0.6))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child(Icon::new(IconName::X).size(IconSize::Custom(px(12.))).color(gpui_kit::white()))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_attachment(id.clone(), cx))),
                    ),
            )
        });
        let files = self.attachments.iter().filter(|a| !matches!(a.kind, ComposerAttachmentKind::Image { .. })).map(|a| {
            let glyph = if matches!(a.kind, ComposerAttachmentKind::PastedText { .. }) { IconName::FileText } else { IconName::File };
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .py(px(4.))
                .text_size(px(14.))
                .text_color(c.foreground)
                .child(Icon::new(glyph).size(IconSize::Custom(px(16.))).color(c.muted_foreground).stroke(composer_icon_stroke(glyph)))
                .child(a.name.clone())
                .child(div().text_size(px(12.)).text_color(c.secondary_label).child(format_size(a.size_bytes)))
        });
        let tiles: Vec<_> = tiles.collect();
        let files: Vec<_> = files.collect();
        Some(
            div()
                .flex()
                .flex_col()
                .when(!tiles.is_empty(), |el| el.child(div().mb(px(12.)).flex().flex_wrap().gap(px(8.)).children(tiles)))
                .when(!files.is_empty(), |el| el.child(div().mb(px(12.)).flex().flex_col().gap(px(4.)).children(files)))
                .into_any_element(),
        )
    }

    fn render_banners(&self, c: &ComposerColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.state.banners.is_empty() {
            return None;
        }
        let rows = self.state.banners.iter().map(|banner| {
            let (tint, ink) = match banner.tone {
                BannerTone::Neutral => (None, c.muted_foreground),
                BannerTone::Warning => (Some(c.warning), c.warning),
                BannerTone::Error => (Some(c.error), c.error_foreground),
            };
            div()
                .id(SharedString::from(format!("banner-{}", banner.id)))
                .min_h(px(28.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(12.))
                .when_some(tint, |el, t| el.bg(t.opacity(0.08)))
                .child(div().w(px(24.)).flex().justify_center().child(Icon::new(banner.icon).size(IconSize::Custom(px(14.))).color(ink).stroke(composer_icon_stroke(banner.icon))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_baseline()
                        .gap(px(4.))
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .child(div().min_w_0().truncate().font_weight(gpui_kit::FontWeight::MEDIUM).text_color(c.foreground).child(banner.title.clone()))
                        .children(banner.description.clone().map(|description| {
                            div().w(px(0.)).min_w_0().flex_grow(1.).truncate().text_color(c.muted_foreground).child(description)
                        })),
                )
                .when_some(banner.action.clone(), |el, action| {
                    let id = banner.id.clone();
                    el.child(div().id(SharedString::from(format!("banner-action-{id}"))).px(px(8.)).h(px(24.)).flex().items_center().rounded(px(8.)).text_size(px(12.)).text_color(c.foreground).cursor_pointer().hover({
                        let h = c.foreground.opacity(0.05);
                        move |s| s.bg(h)
                    }).on_click(cx.listener(move |_, _, _, cx| cx.emit(ComposerEvent::BannerAction(id.clone())))).child(action))
                })
        });
        Some(
            div()
                .mx(DRAWER_INSET)
                .mb(-ATTACHMENT_OVERLAP)
                .pb(ATTACHMENT_OVERLAP)
                .px(px(4.))
                .pt(px(4.))
                .rounded_t(SURFACE_RADIUS)
                .glass(c.glass_surface)
                .border_1()
                .border_color(c.outline)
                .flex()
                .flex_col()
                .children(rows)
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
                .h(px(24.))
                .px(px(7.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(CONTROL_RADIUS)
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(if c.is_dark() { c.muted_foreground } else { c.foreground.opacity(0.7) })
                .hover(move |s| s.bg(hover).text_color(fg))
                .child(Icon::new(glyph).size(IconSize::Custom(px(14.))).color(c.muted_foreground).stroke(composer_icon_stroke(glyph)))
                .child(label)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ComposerEvent::ContextStripClicked(control))))
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(8.))
                .pt(px(8.))
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
        let compact = self.footer_width < FOOTER_COMPACT_BREAKPOINT;
        let disabled = self.state.project_required;
        self.editor.update(cx, |e, _| e.disabled = disabled);

        let (trigger, trigger_width) = self.render_model_trigger(compact, window, cx);
        let show_chip = self.state.runtime_mode != self.state.default_runtime_mode;
        let chip_widths = show_chip.then(|| {
            let label = text_width(&self.state.runtime_mode.label(), px(14.), FontWeight::MEDIUM, window);
            (px(6.) + px(16.) + px(6.) + label + px(8.), px(6.) + px(16.) + px(8.))
        });
        // Host: footer minus padding, attach (28 + 8) and primary actions.
        let actions_width = CONTROL_HEIGHT * if self.state.voice_available { 2. } else { 1. } + px(4.);
        let host = self.footer_width - FOOTER_PADDING_X * 2. - (CONTROL_HEIGHT + px(8.)) - actions_width + px(7.);
        let (chip_icon_only, chip_hidden) = resolve_footer_layout(host, trigger_width, chip_widths, CONTROL_HEIGHT, px(2.));
        let mut controls = div()
            .relative()
            .mr(px(-7.))
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_end()
            .gap(px(2.))
            .child(trigger);
        if show_chip && !chip_hidden {
            controls = controls.child(self.render_permissions_chip(chip_icon_only, &c, cx));
        }
        if show_chip && chip_hidden {
            controls = controls.child(
                icon_action("composer-overflow", IconName::Ellipsis, px(16.), c.muted_foreground, false, &c)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx))),
            );
        }
        if let Some((picker, _)) = &self.picker {
            controls = controls.child(
                deferred(
                    div()
                        .absolute()
                        .bottom(CONTROL_HEIGHT + px(4.))
                        .right(px(0.))
                        // Cached at its fixed size: composer keystrokes reuse the picker's last frame.
                        .child(picker.clone().cached(gpui_kit::StyleRefinement::default().w(MODEL_PICKER_SIZE.width).h(MODEL_PICKER_SIZE.height)))
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
            .px(FOOTER_PADDING_X)
            .pb(FOOTER_PADDING_BOTTOM)
            .child(measure)
            .child(
                div().mr(px(8.)).child(
                    div()
                        .relative()
                        .left(px(0.5))
                        .top(px(1.))
                        .child(
                            icon_action("composer-attach", IconName::Paperclip, px(14.), c.secondary_label, disabled, &c)
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
                    .gap(px(4.))
                    .child(controls)
                    .child(self.render_primary_actions(&c, cx)),
            );

        let editor_element = ComposerPromptEditor::element(&self.editor, cx);
        let editor_focus = self.editor.focus_handle(cx);
        let editor_box = self.editor.update(cx, |_, ecx| ComposerPromptEditor::actions(div().track_focus(&editor_focus), ecx));
        let body = div()
            .relative()
            .px(BODY_PADDING_X)
            .pt(BODY_PADDING_TOP)
            .pb(BODY_PADDING_BOTTOM)
            .children(self.render_attachments(&c, cx))
            .child(editor_box.m(px(-4.)).p(px(4.)).child(editor_element))
            .when_some(self.state.validation.clone(), |el, message| {
                el.child(div().pt(px(6.)).text_size(px(12.)).text_color(c.error_foreground).child(message))
            });

        let card = div()
            .id("composer-card")
            .relative()
            .w_full()
            .rounded(SURFACE_RADIUS)
            .glass(c.glass_surface)
            .border_1()
            .border_color(c.outline)
            .when(!c.is_dark(), |el| {
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

        let menu = self.menu.clone().map(|menu| {
            let pick = cx.entity();
            let hover = cx.entity();
            div()
                .absolute()
                .left(DRAWER_INSET)
                .right(DRAWER_INSET)
                .bottom(relative(1.))
                .mb(-ATTACHMENT_OVERLAP)
                .pb(ATTACHMENT_OVERLAP)
                .rounded_t(SURFACE_RADIUS)
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

        div()
            .id("chat-composer")
            .key_context("ChatComposer")
            .font_family(ui_font_family())
            .relative()
            .w_full()
            .max_w(COMPOSER_MAX_WIDTH)
            .mx_auto()
            .flex()
            .flex_col()
            .capture_action(cx.listener(Self::capture_up))
            .capture_action(cx.listener(Self::capture_down))
            .capture_action(cx.listener(Self::capture_enter))
            .capture_action(cx.listener(Self::capture_tab))
            .capture_action(cx.listener(Self::capture_escape))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .children(self.render_banners(&c, cx))
            .child(div().relative().w_full().children(menu).child(card))
            .children(self.render_context_strip(&c, cx))
    }
}
