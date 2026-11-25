//! The thread sidebar. Ports `Sidebar.tsx` (this module) and the pieces under
//! `components/sidebar/` (submodules). The sidebar renders a
//! [`model::SidebarSnapshot`] and reports what the user did as
//! [`SidebarEvent`]s; it owns only UI state (hover, selection, shelves, search).

pub mod context_menu;
pub mod fixtures;
pub mod model;
pub mod sidebar_chrome;
pub mod sidebar_draft_block;
pub mod sidebar_drag;
pub mod sidebar_project_scope;
pub mod sidebar_list_markers;
pub mod sidebar_notifications;
pub mod sidebar_project_grouping;
pub mod sidebar_thread_header;
pub mod sidebar_thread_row;
pub mod style;
pub mod surface_grain;

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use self::context_menu::{ContextMenu, MenuCommand, MenuTarget};
use self::model::{ProjectRef, SidebarSnapshot, ThreadKey, TimestampMs};
use self::style::{CONTENT_INSET, LIST_FADE, LIST_GAP, ShellColors};
use super::sidebar_logic::{
    SidebarList, SidebarListEntry, SidebarListOptions, Shelf, build_sidebar_list, entry_height,
};
use crate::ActiveAppTheme as _;

actions!(
    sidebar,
    [
        SelectPrevious,
        SelectNext,
        SelectFirst,
        SelectLast,
        Confirm,
        Cancel,
        FocusSearch
    ]
);

/// Key context for the sidebar list.
pub const KEY_CONTEXT: &str = "Sidebar";

/// Binds the sidebar's keys. Call once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(KEY_CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(KEY_CONTEXT)),
        KeyBinding::new(
            if cfg!(target_os = "macos") { "cmd-f" } else { "ctrl-f" },
            FocusSearch,
            Some(KEY_CONTEXT),
        ),
    ]);
}

/// What the user asked the sidebar to do. The app routes these to the runtime.
#[derive(Clone, Debug)]
pub enum SidebarEvent {
    /// Open a thread in the main pane.
    ActivateThread(ThreadKey),
    /// Start a new thread, in a project when one is given.
    NewThread(Option<ProjectRef>),
    AddProject,
    OpenProjectFilter,
    OpenSettings,
    OpenPullRequests,
    OpenIssues,
    OpenUsage,
    /// A row action button or context-menu command on threads.
    ThreadCommand(Vec<ThreadKey>, MenuCommand),
    /// A context-menu command on a project.
    ProjectCommand(ProjectRef, MenuCommand),
    DiscardDraft(ThreadKey),
    /// Open a new-thread draft (`SidebarDraftBlock` row).
    ActivateDraft(SharedString),
    /// A draft's context menu command: `CopyPath`, `ProjectSettings`, or
    /// `Delete` (discard the draft).
    DraftCommand(SharedString, MenuCommand),
    /// Discard a new-thread draft (its row's X button).
    DiscardDraftSession(SharedString),
    /// The inline rename field committed a new, non-empty title.
    RenameThread(ThreadKey, String),
    /// The search field's text changed; the data source may add server matches.
    SearchChanged(SharedString),
    /// The Settled shelf opened, closed, showed more or retried; older pages load to fill it.
    SettledShelf(model::SettledShelfRequest),
}

/// The row whose title is being edited (`ThreadRenameInput`).
pub(crate) struct Renaming {
    pub(crate) key: ThreadKey,
    original: SharedString,
    pub(crate) field: Entity<InputState>,
    _subscription: Subscription,
}

/// The sidebar view.
pub struct Sidebar {
    pub(crate) snapshot: SidebarSnapshot,
    pub(crate) options: SidebarListOptions,
    pub(crate) list: SidebarList,
    /// Fixed height of each entry, gap included.
    heights: Vec<f32>,
    /// Unrounded top of each entry, with the shelf spacer at zero height.
    tops: Vec<f32>,
    spacer_ix: Option<usize>,
    pub(crate) list_state: ListState,
    spacer_height: f32,
    pub(crate) hovered: Option<usize>,
    pub(crate) hovered_pin: Option<usize>,
    /// Keyboard cursor, as an entry index.
    pub(crate) cursor: Option<usize>,
    /// The cursor moved by keyboard, so it draws a focus ring (`focus-visible`).
    pub(crate) keyboard_nav: bool,
    pub(crate) selected: HashSet<ThreadKey>,
    selection_anchor: Option<usize>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) search: Entity<InputState>,
    pub(crate) context_menu: Option<ContextMenu>,
    /// The row whose hover card shows, the pending open, and when one last closed.
    pub(crate) hover_card: Option<usize>,
    hover_card_open: Option<Task<()>>,
    hover_card_closed_at: Option<std::time::Instant>,
    pub(crate) hovered_control: Option<sidebar_thread_header::HeaderControl>,
    /// Header row height: the platform topbar height.
    pub(crate) topbar_height: Pixels,
    /// Left inset of the header row's content, clearing the sidebar toggle.
    pub(crate) header_inset: Pixels,
    /// Current clock; ticks once a second while something shows a duration.
    pub(crate) now: TimestampMs,
    /// Six-step phase of the terminal pulse.
    pub(crate) pulse: u8,
    pub(crate) renaming: Option<Renaming>,
    /// Scenes that show several setting combinations at once keep the
    /// snapshot's settings instead of the client settings.
    snapshot_settings_only: bool,
    /// The snapshot's own settings, which the client settings overlay.
    base_settings: model::SidebarSettings,
    /// Jump hints show: the jump modifier has been held 200ms.
    pub(crate) jump_hints: bool,
    /// The project filter popup, while open.
    pub(crate) scope_menu: Option<sidebar_project_scope::ProjectScopeMenu>,
    /// While dragging: the section under the pointer, shared with the drag preview.
    pub(crate) drag_target: std::rc::Rc<std::cell::Cell<Option<super::sidebar_logic::SidebarSection>>>,
    jump_hold: Option<Task<()>>,
    /// The Settled shelf state last reported with [`SidebarEvent::SettledShelf`].
    settled_request: model::SettledShelfRequest,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Sidebar {
    pub fn new(snapshot: SidebarSnapshot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe_in(&search, window, |this, state, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.options.search = state.read(cx).value();
                cx.emit(SidebarEvent::SearchChanged(this.options.search.clone()));
                this.rebuild(cx);
            }
        });
        let now = snapshot.now;
        let snapshot_settings = snapshot.settings;
        let mut this = Self {
            snapshot,
            options: SidebarListOptions::default(),
            list: SidebarList::default(),
            heights: Vec::new(),
            tops: Vec::new(),
            spacer_ix: None,
            // Every row is measured after a reset: rows above a scrolled viewport left
            // unmeasured count as zero height, and the wheel then stopped short of the top.
            list_state: ListState::new(0, ListAlignment::Top, px(200.)).measure_all(),
            spacer_height: 0.,
            hovered: None,
            hovered_pin: None,
            cursor: None,
            keyboard_nav: false,
            selected: HashSet::new(),
            selection_anchor: None,
            focus_handle: cx.focus_handle(),
            search,
            context_menu: None,
            hover_card: None,
            hover_card_open: None,
            hover_card_closed_at: None,
            hovered_control: None,
            topbar_height: style::TOPBAR_HEIGHT_OVERLAY,
            header_inset: px(53.),
            now,
            pulse: 0,
            renaming: None,
            snapshot_settings_only: false,
            base_settings: snapshot_settings,
            jump_hints: false,
            scope_menu: None,
            drag_target: Default::default(),
            jump_hold: None,
            settled_request: Default::default(),
            _subscriptions: vec![subscription],
        };
        if cx.has_global::<crate::hooks::use_settings::ClientSettingsStore>() {
            let settings = cx.observe_global_in::<crate::hooks::use_settings::ClientSettingsStore>(window, |this, _, cx| this.rebuild(cx));
            this._subscriptions.push(settings);
        }
        if cx.has_global::<crate::shortcut_modifier_state::GlobalStore>() {
            let store = crate::shortcut_modifier_state::ShortcutModifierStore::global(cx);
            let hints = cx.observe(&store, |this, store, cx| {
                let show = store.read(cx).show_thread_jump_hints();
                this.jump_modifier_changed(show, cx);
            });
            this._subscriptions.push(hints);
        }
        if let Some(store) = cx.try_global::<crate::ui_state_store::UiStateStore>() {
            let ui = &store.ui;
            this.options.working_expanded = ui.sidebar_working_expanded;
            this.options.snoozed_expanded = ui.sidebar_snoozed_expanded;
            this.options.settled_expanded = ui.sidebar_settled_expanded;
            this.options.project_scope = ui.sidebar_project_scope_keys.iter().map(|k| SharedString::from(k.clone())).collect();
        }
        this.rebuild(cx);
        // Nobody listens yet; the first snapshot reports the shelf.
        this.settled_request = Default::default();
        this
    }

    /// Replaces the data. Rows are re-derived once here, not per frame.
    pub fn set_snapshot(&mut self, snapshot: SidebarSnapshot, cx: &mut Context<Self>) {
        self.now = snapshot.now;
        self.base_settings = snapshot.settings;
        self.snapshot = snapshot;
        self.rebuild(cx);
    }

    /// Sets the header row height and the inset that clears the sidebar toggle.
    pub fn set_chrome_metrics(&mut self, topbar_height: Pixels, header_inset: Pixels, cx: &mut Context<Self>) {
        self.topbar_height = topbar_height;
        self.header_inset = header_inset;
        cx.notify();
    }

    /// Marks the thread the main pane shows (navigation from outside the
    /// sidebar). Only the active key changes, so nothing is rebuilt.
    pub fn set_active_thread(&mut self, key: Option<ThreadKey>, cx: &mut Context<Self>) {
        if self.snapshot.active_thread != key {
            self.snapshot.active_thread = key;
            cx.notify();
        }
    }

    /// Replaces the unsent drafts and marks the shown one. Rebuilds only when the drafts changed.
    pub fn set_drafts(&mut self, drafts: Vec<model::SidebarDraft>, active: Option<SharedString>, cx: &mut Context<Self>) {
        if self.snapshot.active_draft != active {
            self.snapshot.active_draft = active;
            cx.notify();
        }
        if self.snapshot.drafts != drafts {
            self.snapshot.drafts = drafts;
            self.rebuild(cx);
        }
    }

    /// Swaps the row's title for a focused field holding it, all selected (`startThreadRename`).
    /// Enter or leaving the field commits; Escape cancels.
    pub fn start_rename(&mut self, key: ThreadKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self.snapshot.threads.iter().find(|t| t.key == key).map(|t| t.title.clone()) else { return };
        let field = cx.new(|cx| InputState::new(window, cx).default_value(title.clone()));
        let subscription = cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(window, cx),
            _ => {}
        });
        field.update(cx, |field, cx| {
            field.focus(window, cx);
            field.select_all(window, cx);
        });
        self.hover_card = None;
        self.renaming = Some(Renaming { key, original: title, field, _subscription: subscription });
        cx.notify();
    }

    /// The thread whose title is being edited.
    pub fn renaming(&self) -> Option<(&ThreadKey, &Entity<InputState>)> {
        self.renaming.as_ref().map(|r| (&r.key, &r.field))
    }

    /// `commitThreadRename`: an empty title warns, an unchanged one does nothing.
    pub fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else { return };
        let title = renaming.field.read(cx).value().trim().to_owned();
        window.focus(&self.focus_handle, cx);
        cx.notify();
        if title.is_empty() {
            let toast = crate::components::ui::toast::ToastData::new(
                crate::components::ui::toast::ToastKind::Warning,
                crate::i18n::t!("Thread title cannot be empty"),
            );
            crate::components::ui::toast::Toaster::push(toast, cx);
        } else if title != renaming.original.as_ref() {
            cx.emit(SidebarEvent::RenameThread(renaming.key, title));
        }
    }

    /// Escape in the field: the title stays as it was.
    pub fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// The thread the main pane shows.
    pub fn active_thread(&self) -> Option<&ThreadKey> {
        self.snapshot.active_thread.as_ref()
    }

    /// Whether a context menu is open.
    pub fn is_context_menu_open(&self) -> bool {
        self.context_menu.is_some()
    }

    /// Bounds of list entry `ix` from the last frame, if laid out.
    pub fn entry_bounds(&self, ix: usize) -> Option<Bounds<Pixels>> {
        self.list_state.bounds_for_item(ix)
    }

    /// Renders with the snapshot's settings, ignoring the client settings (scenes).
    pub fn use_snapshot_settings(&mut self, cx: &mut Context<Self>) {
        self.snapshot_settings_only = true;
        self.rebuild(cx);
    }

    /// The data the sidebar renders.
    pub fn snapshot(&self) -> &SidebarSnapshot {
        &self.snapshot
    }

    /// Scrolls the list so entry `ix` is in view (scenes and checks).
    pub fn reveal_entry(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.list_state.scroll_to_reveal_item(ix);
        cx.notify();
    }

    /// The flattened list the sidebar draws.
    pub fn list(&self) -> &SidebarList {
        &self.list
    }

    /// Puts `text` in the search field, as typing it would.
    pub fn set_search(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |state, cx| state.set_value(text.to_owned(), window, cx));
        self.options.search = SharedString::from(text.to_owned());
        cx.emit(SidebarEvent::SearchChanged(self.options.search.clone()));
        self.rebuild(cx);
    }

    /// Mutable list options (shelves, folders, search), for scenes and restores.
    pub fn options_mut(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut SidebarListOptions)) {
        f(&mut self.options);
        self.rebuild(cx);
    }

    /// Forces a hover state (scenes), as if the pointer sat on `entry`.
    pub fn set_hovered(&mut self, entry: Option<usize>, cx: &mut Context<Self>) {
        self.hovered = entry;
        cx.notify();
    }

    /// Opens the context menu for the thread or folder at `entry` (scenes and tests).
    pub fn open_context_menu_at(
        &mut self,
        entry: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_entry_menu(entry, position, cx);
    }

    /// Starts the once-a-second clock (live durations, relative times) and the
    /// stepped terminal pulse. Scenes leave it off so screenshots are stable.
    pub fn start_clock(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                // 2s / 6 steps for the pulse; the clock advances every third step.
                cx.background_executor()
                    .timer(Duration::from_millis(333))
                    .await;
                tick += 1;
                let alive = this.update(cx, |this, cx| {
                    this.pulse = (this.pulse + 1) % 6;
                    if tick.is_multiple_of(3) {
                        this.now = TimestampMs(this.now.0 + 1000);
                    }
                    if this.has_live_rows() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn has_live_rows(&self) -> bool {
        self.list.entries.iter().any(|entry| match entry {
            SidebarListEntry::Thread { thread, .. } => {
                thread.running_terminal_count > 0
                    || thread
                        .runtime
                        .is_some_and(|r| r.status != model::RuntimeStatus::Run(model::RunStatus::Completed))
            }
            _ => false,
        })
    }

    /// Applies the client settings the web sidebar reads with `useClientSettings`.
    fn apply_client_settings(&mut self, cx: &App) {
        use crate::hooks::use_settings::ClientSettingsStore;
        use t3code_contracts::{SidebarProjectGroupingMode as G, SidebarProjectSortOrder as O};
        self.snapshot.settings = self.base_settings;
        if self.snapshot_settings_only || !cx.has_global::<ClientSettingsStore>() {
            return;
        }
        let client = ClientSettingsStore::get(cx);
        let grouping = |mode: &G| match mode {
            G::RepositoryPath => model::ProjectGroupingMode::RepositoryPath,
            G::Separate => model::ProjectGroupingMode::Separate,
            _ => model::ProjectGroupingMode::Repository,
        };
        let settings = &mut self.snapshot.settings;
        settings.project_folders = client.boring_sidebar_enabled;
        settings.folder_rows_multiline = client.boring_sidebar_multiline_rows;
        settings.working_shelf = client.sidebar_working_shelf_enabled;
        settings.notification_inbox = client.notification_inbox_enabled;
        settings.grouping_mode = grouping(&client.sidebar_project_grouping_mode);
        settings.project_sort_order = match client.sidebar_project_sort_order {
            O::CreatedAt => model::ProjectSortOrder::CreatedAt,
            O::Manual => model::ProjectSortOrder::Manual,
            _ => model::ProjectSortOrder::UpdatedAt,
        };
        self.options.grouping_overrides =
            client.sidebar_project_grouping_overrides.iter().map(|(key, mode)| (key.clone(), grouping(mode))).collect();
    }

    /// `useThreadJumpHintsVisible`: hints appear once the thread-jump
    /// modifier has been held for `THREAD_JUMP_HINT_SHOW_DELAY_MS` (200ms),
    /// so a quick shortcut never flashes them, and hide on release.
    fn jump_modifier_changed(&mut self, show: bool, cx: &mut Context<Self>) {
        if !show {
            self.jump_hold = None;
            if std::mem::take(&mut self.jump_hints) {
                cx.notify();
            }
            return;
        }
        if self.jump_hints || self.jump_hold.is_some() {
            return;
        }
        self.jump_hold = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            this.update(cx, |this, cx| {
                this.jump_hold = None;
                this.jump_hints = true;
                cx.notify();
            })
            .ok();
        }));
    }

    /// The jump shortcut label for the thread at `key`, while hints show
    /// (the first nine threads in list order).
    pub(crate) fn jump_label(&self, key: &ThreadKey, cx: &App) -> Option<SharedString> {
        use crate::keybindings::{shortcut_label_for_command, thread_jump_command_for_index};
        use crate::shortcuts::keymap::ShortcutState;
        if !self.jump_hints {
            return None;
        }
        let index = self.list.thread_order.iter().position(|k| k == key)?;
        let command = thread_jump_command_for_index(index)?;
        let state = cx.try_global::<ShortcutState>()?;
        shortcut_label_for_command(&state.config, &t3code_shared::keybindings::KeybindingCommand::Static(command), state.state, state.platform)
    }

    /// Saves shelf expansion and the project scope (`useLocalStorage`, `uiStateStore`).
    fn persist_ui(&self, cx: &mut App) {
        if !cx.has_global::<crate::ui_state_store::UiStateStore>() {
            return;
        }
        let options = &self.options;
        let (working, snoozed, settled) = (options.working_expanded, options.snoozed_expanded, options.settled_expanded);
        let scope: Vec<String> = options.project_scope.iter().map(|k| k.to_string()).collect();
        crate::ui_state_store::UiStateStore::update_ui(cx, |ui| {
            ui.sidebar_working_expanded = working;
            ui.sidebar_snoozed_expanded = snoozed;
            ui.sidebar_settled_expanded = settled;
            ui.sidebar_project_scope_keys = scope;
        });
    }

    pub(crate) fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.apply_client_settings(cx);
        self.list = build_sidebar_list(&self.snapshot, &self.options);
        let rem_scale = ShellColors::from_theme(cx.app_theme()).rem_scale;
        self.report_settled_shelf(false, cx);
        let previous_heights = std::mem::take(&mut self.heights);
        self.heights = self
            .list
            .entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| entry_height(entry, rem_scale) + if ix > 0 { f32::from(LIST_GAP) } else { 0. })
            .collect();
        self.tops = self
            .heights
            .iter()
            .scan(0., |top, height| {
                let this = *top;
                *top += height;
                Some(this)
            })
            .collect();
        self.spacer_ix = self.list.entries.iter().position(|e| matches!(e, SidebarListEntry::ShelfSpacer));
        self.spacer_height = 0.;
        self.hovered = None;
        self.cursor = self.cursor.filter(|&c| c < self.list.entries.len());
        // Rows of the same heights (a status or title change, as a streaming turn makes many
        // times a second) keep the measured list: `reset` drops wheel events until the next
        // paint, so resetting on every update froze wheel scrolling while an agent worked.
        if previous_heights != self.heights {
            // `reset` drops the scroll anchor; a rebuild keeps the list where the user left it.
            let top = self.list_state.logical_scroll_top();
            self.list_state.reset(self.list.entries.len());
            self.list_state.scroll_to(top);
        }
        cx.notify();
    }

    /// Sum of entry heights, without the shelf spacer.
    fn content_height(&self) -> f32 {
        self.heights.iter().sum()
    }

    /// Entry `ix`'s height between its top and bottom edges snapped to whole pixels.
    /// Rem-scaled rows are fractional (77.875px at 15px); Chromium keeps the fraction
    /// and snaps each row edge to a whole pixel, so rows drift the way GPUI's rounding
    /// of each row on its own would not.
    fn snapped_height(&self, ix: usize) -> Option<f32> {
        let top = self.tops.get(ix)? + self.spacer_ix.filter(|&s| s < ix).map_or(0., |s| self.spacer_height - self.heights[s]);
        let height = if self.spacer_ix == Some(ix) { self.spacer_height } else { self.heights[ix] };
        let snap = |y: f32| y.round();
        Some(snap(top + height) - snap(top))
    }

    /// Height of entry `ix` as drawn, including the spacer's current height.
    pub(crate) fn drawn_height(&self, ix: usize) -> f32 {
        match self.list.entries.get(ix) {
            Some(SidebarListEntry::ShelfSpacer) => self.spacer_height,
            Some(_) => self.heights[ix],
            None => 0.,
        }
    }

    pub(crate) fn is_active(&self, key: &ThreadKey) -> bool {
        self.snapshot.active_thread.as_ref() == Some(key)
    }

    /// Opens a thread. Only the active key changes, so nothing is rebuilt.
    pub(crate) fn activate_thread(&mut self, key: ThreadKey, cx: &mut Context<Self>) {
        self.selected.clear();
        self.snapshot.active_thread = Some(key.clone());
        cx.emit(SidebarEvent::ActivateThread(key));
        cx.notify();
    }

    /// Click on a thread row: plain opens, secondary toggles, shift extends.
    pub(crate) fn click_thread(
        &mut self,
        entry: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some(SidebarListEntry::Thread { thread, .. }) = self.list.entries.get(entry) else {
            return;
        };
        let key = thread.key.clone();
        self.cursor = Some(entry);
        self.keyboard_nav = false;
        if modifiers.secondary(cx) {
            if !self.selected.remove(&key) {
                self.selected.insert(key);
            }
            self.selection_anchor = Some(entry);
            cx.notify();
        } else if modifiers.shift {
            let anchor = self.selection_anchor.unwrap_or(entry);
            let (lo, hi) = (anchor.min(entry), anchor.max(entry));
            self.selected = self.list.entries[lo..=hi]
                .iter()
                .filter_map(|e| match e {
                    SidebarListEntry::Thread { thread, .. } => Some(thread.key.clone()),
                    _ => None,
                })
                .collect();
            cx.notify();
        } else {
            self.selection_anchor = Some(entry);
            self.activate_thread(key, cx);
        }
    }

    pub(crate) fn toggle_shelf(&mut self, shelf: Shelf, cx: &mut Context<Self>) {
        let expanded = match shelf {
            Shelf::Working => &mut self.options.working_expanded,
            Shelf::Snoozed => &mut self.options.snoozed_expanded,
            Shelf::Settled => &mut self.options.settled_expanded,
        };
        *expanded = !*expanded;
        self.persist_ui(cx);
        self.rebuild(cx);
    }

    /// Sets the project filter to `scope` (group keys; empty shows all).
    pub(crate) fn set_project_scope(&mut self, scope: Vec<SharedString>, cx: &mut Context<Self>) {
        self.options.project_scope = scope;
        self.options.settled_visible = 0;
        self.persist_ui(cx);
        self.rebuild(cx);
    }

    /// Opens a draft. Only the active draft changes, so nothing is rebuilt.
    pub(crate) fn activate_draft(&mut self, id: SharedString, cx: &mut Context<Self>) {
        self.selected.clear();
        self.snapshot.active_draft = Some(id.clone());
        self.snapshot.active_thread = None;
        cx.emit(SidebarEvent::ActivateDraft(id));
        cx.notify();
    }

    pub(crate) fn toggle_folder(&mut self, group: usize, cx: &mut Context<Self>) {
        let Some(key) = self.list.groups.get(group).map(|g| g.key.clone()) else {
            return;
        };
        if !self.options.collapsed_folders.remove(&key) {
            self.options.collapsed_folders.insert(key);
        }
        self.rebuild(cx);
    }

    pub(crate) fn show_more_settled(&mut self, cx: &mut Context<Self>) {
        if self.snapshot.older_settled.loading {
            return;
        }
        self.report_settled_shelf(true, cx);
        self.options.settled_visible += model::SETTLED_TAIL_PAGE_COUNT;
        self.rebuild(cx);
    }

    /// Tells the data what the Settled shelf shows when that changed, or to retry.
    fn report_settled_shelf(&mut self, retry: bool, cx: &mut Context<Self>) {
        let request = model::SettledShelfRequest {
            expanded: self.options.settled_expanded,
            visible_count: self.snapshot.settings.settled_initial_count + self.options.settled_visible,
            projects: None,
            retry,
        };
        if retry || request != self.settled_request {
            self.settled_request = model::SettledShelfRequest { retry: false, ..request.clone() };
            cx.emit(SidebarEvent::SettledShelf(request));
        }
    }

    /// Keys a thread command acts on: the selection when the row is in it.
    pub(crate) fn command_targets(&self, key: &ThreadKey) -> Vec<ThreadKey> {
        if self.selected.contains(key) {
            self.selected.iter().cloned().collect()
        } else {
            vec![key.clone()]
        }
    }

    pub(crate) fn set_row_hover(&mut self, entry: usize, hovered: bool, cx: &mut Context<Self>) {
        let next = if hovered {
            Some(entry)
        } else if self.hovered == Some(entry) {
            None
        } else {
            return;
        };
        if self.hovered != next {
            self.hovered = next;
            self.schedule_hover_card(cx);
            cx.notify();
        }
    }

    /// The sidebar's `TooltipProvider delay={150} closeDelay={0} timeout={400}`:
    /// a row's card opens 150ms after the pointer settles on it, at once when
    /// another card closed less than 400ms ago, and closes when it leaves.
    fn schedule_hover_card(&mut self, cx: &mut Context<Self>) {
        const DELAY: std::time::Duration = std::time::Duration::from_millis(150);
        const WARM: std::time::Duration = std::time::Duration::from_millis(400);
        let target = self.hovered.filter(|ix| matches!(self.list.entries.get(*ix), Some(SidebarListEntry::Thread { .. })));
        if self.hover_card.is_some() && self.hover_card != target {
            self.hover_card = None;
            self.hover_card_closed_at = Some(std::time::Instant::now());
        }
        self.hover_card_open = None;
        let Some(target) = target.filter(|_| self.context_menu.is_none()) else { return };
        if self.hover_card_closed_at.is_some_and(|at| at.elapsed() < WARM) {
            self.hover_card = Some(target);
            return;
        }
        self.hover_card_open = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DELAY).await;
            this.update(cx, |this, cx| {
                if this.hovered == Some(target) && this.context_menu.is_none() {
                    this.hover_card = Some(target);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn focusable_entry(&self, ix: usize) -> bool {
        !matches!(
            self.list.entries.get(ix),
            None | Some(
                SidebarListEntry::ShelfSpacer
                    | SidebarListEntry::DraftDivider
                    | SidebarListEntry::DragBoundary(_)
                    | SidebarListEntry::DragPlaceholder
                    | SidebarListEntry::Empty(_)
                    | SidebarListEntry::SearchEmpty
            )
        )
    }

    fn move_cursor(&mut self, forward: bool, cx: &mut Context<Self>) {
        let len = self.list.entries.len();
        let mut ix = match (self.cursor, forward) {
            (None, true) => 0,
            (None, false) => len.saturating_sub(1),
            (Some(c), true) => (c + 1).min(len.saturating_sub(1)),
            (Some(c), false) => c.saturating_sub(1),
        };
        while !self.focusable_entry(ix) {
            match forward {
                true if ix + 1 < len => ix += 1,
                false if ix > 0 => ix -= 1,
                _ => return,
            }
        }
        self.set_cursor(ix, cx);
    }

    fn set_cursor(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        self.keyboard_nav = true;
        self.list_state.scroll_to_reveal_item(ix);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        if !self.menu_move(false, cx) {
            self.move_cursor(false, cx);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.menu_move(true, cx) {
            self.move_cursor(true, cx);
        }
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = (0..self.list.entries.len()).find(|&ix| self.focusable_entry(ix)) {
            self.set_cursor(ix, cx);
        }
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = (0..self.list.entries.len()).rev().find(|&ix| self.focusable_entry(ix)) {
            self.set_cursor(ix, cx);
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.context_menu.is_some() {
            self.menu_confirm(window, cx);
            return;
        }
        let Some(ix) = self.cursor else { return };
        match self.list.entries.get(ix).cloned() {
            Some(SidebarListEntry::Thread { thread, .. }) => self.activate_thread(thread.key.clone(), cx),
            Some(SidebarListEntry::Draft(draft)) => self.activate_draft(draft.id.clone(), cx),
            Some(SidebarListEntry::ShelfHeader { shelf, .. }) => self.toggle_shelf(shelf, cx),
            Some(SidebarListEntry::FolderHeader { group, .. }) => self.toggle_folder(group, cx),
            Some(SidebarListEntry::ShowMore { .. }) => self.show_more_settled(cx),
            _ => {}
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
        } else if !self.selected.is_empty() {
            self.selected.clear();
        } else {
            self.cursor = None;
        }
        cx.notify();
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Opens the menu for a thread row or project folder at `entry`.
    pub(crate) fn open_entry_menu(
        &mut self,
        entry: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.hover_card = None;
        self.hover_card_open = None;
        // Chromium reports whole-pixel client coordinates, and the fallback menu opens there.
        let position = point(position.x.floor(), position.y.floor());
        let menu = match self.list.entries.get(entry) {
            Some(SidebarListEntry::Thread { thread, section, .. }) => {
                let label = self
                    .list
                    .project_labels
                    .get(&thread.project_ref())
                    .map(|l| l.as_ref())
                    .unwrap_or("project");
                let items = context_menu::thread_menu_items(&context_menu::ThreadMenuState {
                    branch: thread.branch.as_deref(),
                    is_pinned: thread.pinned_at.is_some(),
                    is_settled: *section == super::sidebar_logic::SidebarSection::Settled,
                    is_snoozed: *section == super::sidebar_logic::SidebarSection::Snoozed,
                    is_running: !thread.runtime_can_archive(),
                    can_snooze_now: thread.can_snooze(self.snapshot.now),
                    project_label: label,
                    project_filter_active: !self.options.project_scope.is_empty(),
                    show_project_filter: true,
                    can_find: false,
                    auto_settle_enabled: !self.snapshot.auto_settle_disabled.contains(&thread.key),
                    snooze_presets: &self.snooze_presets(&thread.key),
                });
                ContextMenu {
                    target: MenuTarget::Thread(thread.key.clone()),
                    position,
                    items,
                    highlighted: None,
                    submenu: None,
                }
            }
            Some(SidebarListEntry::Draft(draft)) => ContextMenu {
                target: MenuTarget::Draft(draft.id.clone()),
                position,
                items: context_menu::draft_menu_items(),
                highlighted: None,
                submenu: None,
            },
            Some(SidebarListEntry::FolderHeader { group, .. }) => {
                let Some(group) = self.list.groups.get(*group) else { return };
                let Some(project) = group.member_refs.first().cloned() else { return };
                ContextMenu {
                    target: MenuTarget::Project(project),
                    position,
                    items: context_menu::project_menu_items(group.pinned),
                    highlighted: None,
                    submenu: None,
                }
            }
            _ => return,
        };
        self.context_menu = Some(menu);
        cx.notify();
    }

    /// The snooze menu for a thread, from its environment's presets at the sidebar's clock.
    fn snooze_presets(&self, key: &ThreadKey) -> Vec<crate::state::sidebar_actions::ResolvedSnoozePreset> {
        let now = jiff::Timestamp::from_millisecond(self.now.0)
            .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()))
            .unwrap_or_else(|_| jiff::ZonedDateTime::now());
        let settings = self.snapshot.snooze_presets.get(&key.environment_id);
        crate::state::sidebar_actions::resolve_snooze_presets(settings.map(|s| &s[..]), &now)
    }

    /// Opens the snooze presets for a thread row (its Snooze button).
    pub(crate) fn open_snooze_menu(&mut self, entry: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(SidebarListEntry::Thread { thread, .. }) = self.list.entries.get(entry) else {
            return;
        };
        self.context_menu = Some(ContextMenu {
            target: MenuTarget::Thread(thread.key.clone()),
            position,
            items: context_menu::snooze_items(&self.snooze_presets(&thread.key)),
            highlighted: None,
            submenu: None,
        });
        cx.notify();
    }

    /// Moves the menu highlight; false when no menu is open.
    fn menu_move(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        let len = menu.items.len();
        let enabled: Vec<usize> = (0..len).filter(|&i| !menu.items[i].disabled).collect();
        if enabled.is_empty() {
            return true;
        }
        let pos = menu
            .highlighted
            .and_then(|h| enabled.iter().position(|&i| i == h));
        let next = match (pos, forward) {
            (None, true) => 0,
            (None, false) => enabled.len() - 1,
            (Some(p), true) => (p + 1) % enabled.len(),
            (Some(p), false) => (p + enabled.len() - 1) % enabled.len(),
        };
        menu.highlighted = Some(enabled[next]);
        menu.submenu = None;
        cx.notify();
        true
    }

    fn menu_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &self.context_menu else { return };
        let Some(ix) = menu.highlighted else { return };
        if menu.items[ix].children.is_empty() {
            self.run_menu_item(ix, None, window, cx);
        } else if let Some(menu) = self.context_menu.as_mut() {
            menu.submenu = Some((ix, Some(0)));
            cx.notify();
        }
    }

    /// Runs a menu row (`child` picks a submenu row) and closes the menu.
    pub(crate) fn run_menu_item(&mut self, ix: usize, child: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else { return };
        if let Some(command) = context_menu::picked_command(&menu, ix, child) {
            match menu.target {
                MenuTarget::Thread(key) if command == MenuCommand::Rename => self.start_rename(key, window, cx),
                MenuTarget::Thread(key) if command == MenuCommand::FilterByProject => {
                    let scope = if self.options.project_scope.is_empty() {
                        let project = self.snapshot.threads.iter().find(|t| t.key == key).map(|t| t.project_ref());
                        project
                            .and_then(|p| self.list.groups.iter().find(|g| g.member_refs.contains(&p)))
                            .map(|g| vec![g.key.clone()])
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    self.set_project_scope(scope, cx);
                }
                MenuTarget::Thread(key) => {
                    let targets = self.command_targets(&key);
                    cx.emit(SidebarEvent::ThreadCommand(targets, command));
                }
                MenuTarget::Draft(id) => cx.emit(SidebarEvent::DraftCommand(id, command)),
                MenuTarget::Project(project) => {
                    cx.emit(SidebarEvent::ProjectCommand(project, command));
                }
            }
        }
        cx.notify();
    }

    fn render_list_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = ShellColors::from_theme(cx.app_theme());
        let gap = if ix > 0 { LIST_GAP } else { px(0.) };
        let Some(entry) = self.list.entries.get(ix).cloned() else {
            return div().into_any_element();
        };
        let body = match entry {
            SidebarListEntry::Draft(draft) => self.render_draft_row(ix, &draft, &colors, window, cx),
            SidebarListEntry::DraftDivider => sidebar_draft_block::draft_divider(&colors),
            SidebarListEntry::DragBoundary(marker) => self.render_drag_boundary(marker, &colors, cx),
            SidebarListEntry::DragPlaceholder => self.render_drag_placeholder(&colors, cx),
            SidebarListEntry::Thread { thread, variant, section } => {
                self.render_thread_row(ix, &thread, variant, section, &colors, window, cx)
            }
            SidebarListEntry::ShelfHeader { shelf, count, expanded } => {
                self.render_shelf_header(ix, shelf, count, expanded, &colors, window, cx)
            }
            SidebarListEntry::FolderHeader {
                group,
                collapsed,
                hidden_count,
                dot,
                first,
            } => self.render_folder_header(ix, group, collapsed, hidden_count, dot, first, &colors, window, cx),
            SidebarListEntry::ShowMore { count } => self.render_show_more(ix, count, &colors, window, cx),
            SidebarListEntry::ShelfSpacer => {
                return div().h(px(self.snapped_height(ix).unwrap_or(self.spacer_height))).into_any_element();
            }
            SidebarListEntry::Empty(state) => self.render_empty(&state, &colors, cx),
            SidebarListEntry::SearchEmpty => sidebar_list_markers::search_empty(&colors),
        };
        let snapped = self.snapped_height(ix);
        div().pt(gap).when_some(snapped, |d, h| d.h(px(h))).child(body).into_any_element()
    }

    /// Recomputes the `mt-auto` spacer from the viewport height.
    fn update_spacer(&mut self) {
        let Some(spacer_ix) = self.spacer_ix else { return };
        let viewport = f32::from(self.list_state.viewport_bounds().size.height);
        // The spacer draws without the list gap its `heights` entry counts.
        let wanted = (viewport - self.content_height() + self.heights[spacer_ix]).max(0.);
        if (wanted - self.spacer_height).abs() >= 0.01 {
            self.spacer_height = wanted;
            self.list_state.remeasure_items(spacer_ix..self.list.entries.len());
        }
    }

    fn list_overflows(&self) -> (bool, bool) {
        let viewport = f32::from(self.list_state.viewport_bounds().size.height);
        let total = self.content_height() + self.spacer_height;
        if total <= viewport + 0.5 {
            return (false, false);
        }
        let top = self.list_state.logical_scroll_top();
        let offset: f32 = (0..top.item_ix).map(|ix| self.drawn_height(ix)).sum::<f32>()
            + f32::from(top.offset_in_item);
        (offset > 0.5, offset + viewport < total - 0.5)
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ShellColors::from_theme(cx.app_theme());
        if self.options.dragging.is_some() && !cx.has_active_drag() {
            // The drag ended outside any drop target.
            cx.defer_in(window, |this, _, cx| this.end_drag(cx));
        }
        self.update_spacer();
        let (fade_top, fade_bottom) = self.list_overflows();
        let fade = |top: bool| {
            let (from, to) = (colors.sidebar, colors.sidebar.opacity(0.));
            div()
                .absolute()
                .left_0()
                .right_0()
                .h(LIST_FADE)
                .when(top, |d| d.top_0())
                .when(!top, |d| d.bottom_0())
                .bg(linear_gradient(
                    if top { 180. } else { 0. },
                    linear_color_stop(from, 0.),
                    linear_color_stop(to, 1.),
                ))
        };

        div()
            .id("sidebar")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::focus_search))

            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_color(colors.sidebar_foreground)
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<sidebar_drag::DraggedThread>, _, cx| {
                this.drag_moved(event.event.position, cx);
            }))
            .on_drop(cx.listener(|this, dragged: &sidebar_drag::DraggedThread, _, cx| {
                match this.drag_target.get() {
                    Some(target) => this.drop_into(dragged, target, cx),
                    None => this.end_drag(cx),
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| {
                cx.defer_in(window, |this, _, cx| this.end_drag(cx));
                let _ = this;
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, window, cx| {
                cx.defer_in(window, |this, _, cx| this.end_drag(cx));
                let _ = this;
            }))
            .child(self.render_header_row(&colors, window, cx))
            .child(self.render_search_row(&colors, window, cx))
            .child(
                div()
                    .id("sidebar-list")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .px(colors.s(CONTENT_INSET))
                    .pb(colors.s(CONTENT_INSET))
                    .child(
                        list(
                            self.list_state.clone(),
                            cx.processor(|this, ix, window, cx| this.render_list_entry(ix, window, cx)),
                        )
                        .size_full(),
                    )
                    .when(fade_top, |d| d.child(fade(true)))
                    .when(fade_bottom, |d| d.child(fade(false))),
            )
            .child(self.render_footer(&colors, window, cx))
            .children(self.render_context_menu(&colors, window, cx))
    }
}
