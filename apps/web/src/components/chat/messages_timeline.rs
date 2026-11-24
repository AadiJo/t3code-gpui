//! The message timeline: a virtual list of derived rows inside the chat
//! content lane. Ports `MessagesTimeline.tsx` and `useTimelineFollow.ts`:
//! the 736px lane, per-kind bottom spacing, follow and the scroll-to-end
//! button, per-thread scroll memory, the composer inset at the bottom, and a
//! stepped animation clock for live rows and fading markdown blocks.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{DefaultHasher, Hash, Hasher},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{prelude::FluentBuilder as _, 
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Task, WeakEntity, Window, div, px,
};

use super::{
    chat_style::ChatColors,
    markdown_stream_blocks::parse_message_blocks,
    messages_timeline_logic::{RowKind, TimelineExpansion, TimelineRow, derive_timeline_rows},
    timeline_entries::{AttemptId, Attachment, EntryId, EntryKind, MessageId, MessageRole, RequestId, RunId, TimelineInput},
    thread_find::{FindHighlight, FindIndex, FindMatch, SharedMatches},
    timeline_viewport::TimelineFollow,
    virtual_list::{AT_END_THRESHOLD, ListViewport, RowSlot, ScrollMemory, VirtualListState, virtual_list},
};
use crate::{
    components::chat_markdown::{MarkdownAction, MarkdownActions, MarkdownUiState},
    markdown_pipeline::{MarkdownFlavor, ParsedBlocks},
};

/// `--chat-content-max-width` for the "comfortable" chat width (46rem).
/// Timeline gutter at ≥640px (`padding-inline: 3rem`); 12px below that.
pub const TIMELINE_GUTTER: Pixels = px(48.);
pub const TIMELINE_GUTTER_NARROW: Pixels = px(12.);
/// The small-screen breakpoint (`sm:`).
pub const SM_BREAKPOINT: Pixels = px(640.);
/// List header: the titlebar scroll fade height when no banner sits on top.
pub const TIMELINE_TOP_INSET: Pixels = px(24.);
/// Space under the composer spacer (`h-3 sm:h-4`).
pub const TIMELINE_BOTTOM_PAD: Pixels = px(16.);
/// A newly sent message is pinned this far below the top (`CHAT_TIMELINE_ANCHOR_OFFSET`).
pub const NEW_TURN_ANCHOR_OFFSET: Pixels = px(24.);
/// The reply-end rule's slack above the composer (fork, `MessagesTimeline.tsx:1000-1027`).
const REPLY_END_SLACK: Pixels = px(8.);
/// Showing the scroll-to-end button waits this long; hiding is immediate.
const SCROLL_BUTTON_SHOW_DELAY: Duration = Duration::from_millis(150);
/// One step of the shimmer: 30 steps over 2.2s, about 13.6 per second.
pub const ANIMATION_STEP: Duration = Duration::from_micros(73_333);
pub const SHIMMER_STEPS: u64 = 30;
/// New markdown blocks fade in over 600ms while a reply streams.
pub const BLOCK_FADE: Duration = Duration::from_millis(600);
/// A match scrolled into view sits this far below the top.
const FIND_SCROLL_MARGIN: Pixels = px(96.);
/// Copy buttons show a check this long (`MessageCopyButton`, code blocks use 1200ms).
const COPIED_FOR: Duration = Duration::from_millis(1_200);

/// What a timeline button asks the chat's owner to do.
#[derive(Clone, Debug)]
pub enum TimelineAction {
    /// Approve or decline a pending approval.
    RespondToApproval { request: RequestId, approve: bool },
    /// Fork the thread after the run that ends with this reply.
    Fork(MessageId),
    /// Quote this reply in the composer.
    Reply { message: MessageId, text: SharedString },
    /// "Edit from here": revert to before this user message.
    EditFrom(MessageId),
    SendQueuedNow(SharedString),
    EditQueued(SharedString),
    RemoveQueued(SharedString),
    /// Open the diff panel on a turn (by its reply) or, with no turn, the
    /// branch, revealing `file` when given.
    OpenDiff { turn: Option<MessageId>, file: Option<SharedString> },
    /// Open the media viewer on a message's image attachments at `index`.
    OpenImage { attachments: Arc<[Attachment]>, index: usize },
    /// A markdown link or file chip that is not a web URL.
    OpenLink(SharedString),
    /// A code block's Run button: run this shell command in a terminal.
    RunShellCommand(SharedString),
}

/// The open find's query and where it stands.
struct TimelineFind {
    query: SharedString,
    folded: SharedString,
    matches: SharedMatches,
    active: usize,
}

/// Parsed markdown of one message, reused across frames.
pub(super) struct MessageMarkdown {
    text: SharedString,
    streaming: bool,
    pub(super) pieces: Vec<ParsedBlocks>,
    /// When each piece first appeared, for the streaming fade.
    pub(super) first_seen: Vec<Instant>,
}

/// Threads whose derived rows are kept for a fast switch back.
const ROW_CACHE_THREADS: usize = 64;

/// Rows derived from `input` with nothing expanded.
struct CachedRows {
    input: TimelineInput,
    rows: Arc<Vec<TimelineRow>>,
}

/// Whether row derivation would give the same rows for `a` and `b`. The
/// entries must be the same allocation; the small fields compare by value.
fn same_input(a: &TimelineInput, b: &TimelineInput) -> bool {
    Arc::ptr_eq(&a.entries, &b.entries)
        && a.pending_approval_ids == b.pending_approval_ids
        && a.latest_run == b.latest_run
        && a.is_working == b.is_working
        && a.active_turn_started_at == b.active_turn_started_at
        && a.turn_diff_summaries == b.turn_diff_summaries
        && a.supports_rollback == b.supports_rollback
        && a.worktree_setup == b.worktree_setup
        && a.queued == b.queued
        && a.pending_question == b.pending_question
}

/// The chat timeline for one thread at a time.
pub struct MessagesTimeline {
    pub(super) input: TimelineInput,
    pub(super) rows: Arc<Vec<TimelineRow>>,
    pub(super) expansion: TimelineExpansion,
    /// Work entries opened to show their detail.
    pub(super) expanded_entries: HashSet<EntryId>,
    /// User messages unfolded past the long-message clip.
    pub(super) unfolded_messages: HashSet<SharedString>,
    pub(super) list: VirtualListState,
    /// Nested lists of expanded tool groups, by group id.
    pub(super) group_lists: HashMap<SharedString, VirtualListState>,
    pub(super) markdown_state: MarkdownUiState,
    pub(super) markdown: HashMap<SharedString, MessageMarkdown>,
    pub(super) colors: ChatColors,
    /// The message column's width at the Chat width setting.
    lane: super::chat_style::ChatLane,
    /// Advances every [`ANIMATION_STEP`] while something animates on screen.
    pub(super) animation_step: u64,
    animation_task: Option<Task<()>>,
    copied_task: Option<Task<()>>,
    scroll_button_task: Option<Task<()>>,
    pub(super) show_scroll_button: bool,
    thread_key: Option<SharedString>,
    scroll_memory: HashMap<SharedString, ScrollMemory>,
    /// Rows derived for recently shown threads, so switching back to an
    /// unchanged thread skips row derivation (3,000 rows take ~4 ms).
    row_cache: HashMap<SharedString, CachedRows>,
    row_cache_order: VecDeque<SharedString>,
    composer_inset: Pixels,
    /// Folded text of searchable rows, for in-thread find.
    find_index: FindIndex,
    /// Folding every row for find, on a background thread.
    find_preparing: Option<Task<()>>,
    find_ready: bool,
    find: Option<TimelineFind>,
    /// The row being rendered, so its markdown can highlight its matches.
    pub(super) rendering_row: usize,
    pub(super) now: Instant,
    /// Wall-clock milliseconds the fixtures treat as "now", for running timers.
    pub(super) now_millis: i64,
    /// A side chat's timeline in the right panel: no "Fork from here" (a fork would promote it).
    pub(super) embedded: bool,
    /// The list's top spacer at a 16px root: the header fade's height, or a side chat's `pt-2`
    /// above its "Context from" divider (the web's list header swaps the spacer for the divider block).
    top_inset: Pixels,
    weak: WeakEntity<Self>,
}

impl MessagesTimeline {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let list = VirtualListState::new();
        list.set_insets(TIMELINE_TOP_INSET, TIMELINE_BOTTOM_PAD);
        let weak = cx.weak_entity();
        let scroll_weak = weak.clone();
        list.on_scroll(move |viewport, follow, _window, cx| {
            let viewport = viewport.clone();
            scroll_weak
                .update(cx, |this, cx| this.viewport_changed(&viewport, follow, cx))
                .ok();
        });
        Self {
            input: TimelineInput::default(),
            rows: Arc::new(Vec::new()),
            expansion: TimelineExpansion::default(),
            expanded_entries: HashSet::new(),
            unfolded_messages: HashSet::new(),
            list,
            group_lists: HashMap::new(),
            markdown_state: MarkdownUiState { run_shell_commands: true, ..Default::default() },
            markdown: HashMap::new(),
            colors: ChatColors::get(cx),
            lane: super::chat_style::ChatLane::get(cx),
            animation_step: 0,
            animation_task: None,
            copied_task: None,
            scroll_button_task: None,
            show_scroll_button: false,
            thread_key: None,
            scroll_memory: HashMap::new(),
            row_cache: HashMap::new(),
            row_cache_order: VecDeque::new(),
            composer_inset: px(0.),
            find_index: FindIndex::default(),
            find_preparing: None,
            find_ready: false,
            find: None,
            rendering_row: 0,
            now: Instant::now(),
            now_millis: 0,
            embedded: false,
            top_inset: TIMELINE_TOP_INSET,
            weak,
        }
    }

    /// Shows a thread. Switching away remembers the scroll position, and
    /// coming back restores it (`useTimelinePositionRestore`).
    pub fn set_thread(&mut self, key: SharedString, input: TimelineInput, cx: &mut Context<Self>) {
        if let Some(previous) = self.thread_key.take() {
            self.scroll_memory.insert(previous.clone(), self.list.memory());
            self.remember_rows(previous);
        }
        self.expansion = TimelineExpansion::default();
        self.expanded_entries.clear();
        self.find = None;
        self.find_index.clear();
        self.find_preparing = None;
        self.find_ready = false;
        self.group_lists.clear();
        let cached = self.row_cache.remove(&key).filter(|cached| same_input(&cached.input, &input));
        self.input = input;
        match cached {
            Some(cached) => self.use_rows(cached.rows),
            None => self.rebuild_rows(),
        }
        let memory = self.scroll_memory.get(&key).copied();
        match memory {
            Some(memory) => self.list.restore(memory),
            None => self.list.follow_end(),
        }
        self.thread_key = Some(key);
        cx.notify();
    }

    /// Replaces the current thread's data, keeping scroll position and expansions.
    pub fn update_input(&mut self, input: TimelineInput, cx: &mut Context<Self>) {
        self.input = input;
        self.rebuild_rows();
        self.refresh_find();
        cx.notify();
    }

    /// Folds every row for find on a background thread (find opened). Until
    /// it lands, a query counts as pending ("1/…"); then the search runs and
    /// [`FindUpdated`] tells the find bar.
    pub fn prepare_find(&mut self, cx: &mut Context<Self>) {
        if self.find_preparing.is_some() {
            return;
        }
        let rows = self.rows.clone();
        let prepare = cx.background_spawn(async move { FindIndex::prepare(&rows) });
        self.find_preparing = Some(cx.spawn(async move |this, cx| {
            let prepared = prepare.await;
            this.update(cx, |this, cx| {
                this.find_index.merge(prepared);
                // Keep the finished task's slot so the index is not prepared twice.
                this.find_preparing = Some(Task::ready(()));
                this.find_ready = true;
                if let Some(query) = this.find.as_ref().map(|f| f.query.clone()) {
                    this.set_find_query(&query, cx);
                }
                cx.emit(FindUpdated);
            })
            .ok();
        }));
    }

    /// The open query still waits for the index.
    pub fn find_counting(&self) -> bool {
        self.find.is_some() && !self.find_ready
    }

    /// Searches every row for `query` and shows the first match at or below
    /// the top of the viewport. Returns (active, total).
    pub fn set_find_query(&mut self, query: &str, cx: &mut Context<Self>) -> (usize, usize) {
        if !self.find_ready && self.find_preparing.is_some() {
            self.find = (!query.is_empty()).then(|| TimelineFind {
                query: query.to_string().into(),
                folded: super::thread_find::fold(query).into(),
                matches: Arc::from([]),
                active: 0,
            });
            return (0, 0);
        }
        let matches: SharedMatches = self.find_index.search(&self.rows, query).into();
        let top = self.list.viewport().visible.start;
        let active = matches.iter().position(|m| m.row >= top).unwrap_or(0);
        self.find = (!query.is_empty()).then(|| TimelineFind {
            query: query.to_string().into(),
            folded: super::thread_find::fold(query).into(),
            matches,
            active,
        });
        self.reveal_active_match();
        cx.notify();
        self.find_count()
    }

    /// Moves to the next (`delta` 1) or previous (-1) match, wrapping.
    pub fn step_find(&mut self, delta: isize, cx: &mut Context<Self>) -> (usize, usize) {
        if let Some(find) = self.find.as_mut().filter(|f| !f.matches.is_empty()) {
            let total = find.matches.len() as isize;
            find.active = ((find.active as isize + delta).rem_euclid(total)) as usize;
            self.reveal_active_match();
            cx.notify();
        }
        self.find_count()
    }

    /// Closes find and drops its index, so a closed find holds no memory.
    pub fn clear_find(&mut self, cx: &mut Context<Self>) {
        self.find_index.clear();
        self.find_preparing = None;
        self.find_ready = false;
        if self.find.take().is_some() {
            cx.notify();
        }
    }

    /// (active, total) of the open find.
    pub fn find_count(&self) -> (usize, usize) {
        self.find.as_ref().map_or((0, 0), |f| (f.active, f.matches.len()))
    }

    /// The active match, for checks.
    pub fn active_match(&self) -> Option<FindMatch> {
        self.find.as_ref().and_then(|f| f.matches.get(f.active).copied())
    }

    /// Re-runs the open find after the rows changed, keeping the active index.
    fn refresh_find(&mut self) {
        let Some(find) = self.find.as_mut() else { return };
        find.matches = self.find_index.search(&self.rows, &find.query).into();
        find.active = find.active.min(find.matches.len().saturating_sub(1));
    }

    /// Scrolls the active match's row into view and unfolds a clipped user message.
    fn reveal_active_match(&mut self) {
        let Some(found) = self.active_match() else { return };
        if let Some(RowKind::Message(message)) = self.rows.get(found.row).map(|r| &r.kind) {
            self.unfolded_messages.insert(message.message.id.0.clone());
        }
        let visible = self.list.viewport().visible;
        if !visible.contains(&found.row) || found.row + 1 == visible.end {
            self.list.scroll_to_row(found.row, FIND_SCROLL_MARGIN);
        }
    }

    /// The highlights for the row being rendered, while find has a query.
    pub(super) fn find_highlight(&self) -> Option<FindHighlight> {
        let find = self.find.as_ref()?;
        let row = self.rendering_row;
        let active = find.matches.get(find.active).filter(|m| m.row == row).map(|m| m.occurrence);
        let theme = &self.colors.theme;
        Some(FindHighlight {
            query: find.folded.clone(),
            active,
            background: theme.search_match_background,
            foreground: theme.search_match_foreground,
            active_background: theme.search_match_active_background,
            active_foreground: theme.search_match_active_foreground,
        })
    }

    /// Whether the scroll-to-end button shows (after its 150ms delay).
    pub fn scroll_button_visible(&self) -> bool {
        self.show_scroll_button
    }

    /// Height of the composer overlay the list keeps clear.
    pub fn composer_bottom(&self) -> Pixels {
        self.composer_inset
    }

    /// A streaming delta: replaces one message's text without re-deriving
    /// rows. Finishing the stream (`streaming: false`) re-derives, since the
    /// meta row and folds depend on it.
    pub fn update_message_text(&mut self, id: &MessageId, text: SharedString, streaming: bool, cx: &mut Context<Self>) {
        let entries = Arc::make_mut(&mut self.input.entries);
        let Some(message) = entries.iter_mut().rev().find_map(|entry| match &mut entry.kind {
            EntryKind::Message(message) if message.id == *id => Some(message),
            _ => None,
        }) else {
            return;
        };
        let was_streaming = message.streaming;
        message.text = text.clone();
        message.streaming = streaming;
        if was_streaming && !streaming {
            self.rebuild_rows();
        } else {
            let rows = Arc::make_mut(&mut self.rows);
            for row in rows.iter_mut().rev() {
                if let RowKind::Message(row) = &mut row.kind
                    && row.message.id == *id
                {
                    row.message.text = text;
                    row.message.streaming = streaming;
                    break;
                }
            }
        }
        cx.notify();
    }

    pub fn input(&self) -> &TimelineInput {
        &self.input
    }

    /// Marks this as an embedded side chat's timeline (`embedded` in `ChatView`).
    pub fn set_embedded(&mut self, embedded: bool, cx: &mut Context<Self>) {
        self.embedded = embedded;
        cx.notify();
    }

    /// This is an embedded side chat's timeline.
    pub fn is_embedded(&self) -> bool {
        self.embedded
    }

    /// The milliseconds clock running timers read ("Working for 1m 23s").
    pub fn set_now_millis(&mut self, now: i64) {
        self.now_millis = now;
    }

    /// The composer overlay's measured height; the list reserves it at the bottom.
    pub fn set_composer_inset(&mut self, height: Pixels, cx: &mut Context<Self>) {
        if (self.composer_inset - height).abs() < px(0.5) {
            return;
        }
        self.composer_inset = height;
        self.apply_insets();
        cx.notify();
    }

    /// Replaces the top spacer, given at a 16px root (see `top_inset`).
    pub fn set_top_inset(&mut self, at16: Pixels, cx: &mut Context<Self>) {
        self.top_inset = at16;
        self.apply_insets();
        cx.notify();
    }

    /// The insets are rem lengths, so every change and every render sets them at the current root size.
    fn apply_insets(&self) {
        self.list.set_insets(self.colors.r(self.top_inset), self.composer_inset + self.colors.r(TIMELINE_BOTTOM_PAD));
    }

    /// Pins the last user message near the top, as sending a message does.
    pub fn anchor_latest_user_message(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self
            .rows
            .iter()
            .rposition(|row| matches!(&row.kind, RowKind::Message(m) if m.message.role == MessageRole::User))
        {
            self.list.pin_row_to_top(index, NEW_TURN_ANCHOR_OFFSET);
            cx.notify();
        }
    }

    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list.follow_end();
        self.show_scroll_button = false;
        cx.notify();
    }

    pub fn follow(&self) -> TimelineFollow {
        self.list.follow()
    }

    pub fn viewport(&self) -> ListViewport {
        self.list.viewport()
    }

    pub fn list_state(&self) -> &VirtualListState {
        &self.list
    }

    pub fn rows(&self) -> &[TimelineRow] {
        &self.rows
    }

    /// Keeps the rows of the thread being left, while nothing is expanded
    /// (a cached row set must match a fresh derivation with default expansion).
    fn remember_rows(&mut self, key: SharedString) {
        let unexpanded = self.expansion.runs.is_empty() && self.expansion.attempts.is_empty() && self.expansion.work_groups.is_empty();
        if !unexpanded || self.input.is_working {
            return;
        }
        self.row_cache_order.retain(|k| *k != key);
        self.row_cache_order.push_back(key.clone());
        self.row_cache.insert(key, CachedRows { input: self.input.clone(), rows: self.rows.clone() });
        while self.row_cache_order.len() > ROW_CACHE_THREADS {
            if let Some(old) = self.row_cache_order.pop_front() {
                self.row_cache.remove(&old);
            }
        }
    }

    fn use_rows(&mut self, rows: Arc<Vec<TimelineRow>>) {
        let slots: Vec<RowSlot> = rows.iter().map(|row| RowSlot { key: row.key, estimate: row.kind.estimate() }).collect();
        self.list.set_rows(&slots);
        self.rows = rows;
    }

    /// Re-derives rows from the input and current expansions.
    pub(super) fn rebuild_rows(&mut self) {
        let rows = derive_timeline_rows(&self.input, &self.expansion);
        let slots: Vec<RowSlot> = rows.iter().map(|row| RowSlot { key: row.key, estimate: row.kind.estimate() }).collect();
        self.list.set_rows(&slots);
        // A running turn that shows tool activity releases the new-turn pin.
        if self.list.is_pinned() && rows.iter().any(|r| matches!(r.kind, RowKind::WorkLive { .. } | RowKind::WorkToggle { .. }))
            && self.input.is_working
        {
            self.list.follow_end();
        }
        self.rows = Arc::new(rows);
    }

    pub(super) fn toggle_run(&mut self, run: RunId, cx: &mut Context<Self>) {
        if !self.expansion.runs.remove(&run) {
            self.expansion.runs.insert(run);
        }
        self.rebuild_rows();
        cx.notify();
    }

    pub(super) fn toggle_attempt(&mut self, attempt: AttemptId, cx: &mut Context<Self>) {
        if !self.expansion.attempts.remove(&attempt) {
            self.expansion.attempts.insert(attempt);
        }
        self.rebuild_rows();
        cx.notify();
    }

    pub(super) fn toggle_work_group(&mut self, group: SharedString, cx: &mut Context<Self>) {
        if !self.expansion.work_groups.remove(&group) {
            self.expansion.work_groups.insert(group);
        }
        self.rebuild_rows();
        cx.notify();
    }

    pub(super) fn toggle_entry(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        if !self.expanded_entries.remove(&entry) {
            self.expanded_entries.insert(entry);
        }
        cx.notify();
    }

    pub(super) fn toggle_message_fold(&mut self, id: SharedString, cx: &mut Context<Self>) {
        if !self.unfolded_messages.remove(&id) {
            self.unfolded_messages.insert(id);
        }
        cx.notify();
    }

    /// The parsed markdown of a message, parsing only what changed.
    pub(super) fn message_markdown(&mut self, id: &SharedString, text: &SharedString, streaming: bool, flavor: MarkdownFlavor) -> &MessageMarkdown {
        let now = self.now;
        let entry = self.markdown.entry(id.clone()).or_insert_with(|| MessageMarkdown {
            text: SharedString::default(),
            streaming,
            pieces: Vec::new(),
            first_seen: Vec::new(),
        });
        if entry.text != *text || entry.streaming != streaming || entry.pieces.is_empty() && !text.is_empty() {
            let was_streaming = entry.streaming;
            entry.pieces = parse_message_blocks(text, flavor, streaming);
            // Pieces seen before keep their time; new ones fade in only while streaming.
            let seen_at = if streaming || was_streaming { now } else { now - BLOCK_FADE };
            entry.first_seen.resize(entry.pieces.len(), seen_at);
            entry.text = text.clone();
            entry.streaming = streaming;
        }
        entry
    }

    /// Markdown actions dispatch back into this timeline.
    pub(super) fn markdown_actions(&self) -> MarkdownActions {
        let weak = self.weak.clone();
        Rc::new(move |action, _window, cx| {
            weak.update(cx, |this, cx| this.markdown_action(action, cx)).ok();
        })
    }

    fn markdown_action(&mut self, action: MarkdownAction, cx: &mut Context<Self>) {
        match action {
            MarkdownAction::Copy { key, text } => self.copy(key, text, cx),
            MarkdownAction::ToggleWrap(key) => {
                if !self.markdown_state.unwrapped.remove(&key) {
                    self.markdown_state.unwrapped.insert(key);
                }
            }
            MarkdownAction::ToggleDetails(key) => {
                if !self.markdown_state.open_details.remove(&key) {
                    self.markdown_state.open_details.insert(key);
                }
            }
            MarkdownAction::RunShellCommand(command) => cx.emit(TimelineAction::RunShellCommand(command)),
            MarkdownAction::OpenLink(target) => {
                if target.contains("://") {
                    cx.open_url(&target);
                } else {
                    cx.emit(TimelineAction::OpenLink(target));
                }
            }
        }
        cx.notify();
    }

    /// Copies text and shows the check mark on the button keyed `key`.
    pub(super) fn copy(&mut self, key: u64, text: SharedString, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        self.markdown_state.copied = Some(key);
        self.copied_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                if this.markdown_state.copied == Some(key) {
                    this.markdown_state.copied = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// Follow and button state after the list laid out or scrolled.
    fn viewport_changed(&mut self, viewport: &ListViewport, follow: TimelineFollow, cx: &mut Context<Self>) {
        let at_end = follow.is_following() || viewport.distance_from_end() <= AT_END_THRESHOLD || self.reply_end_visible(viewport);
        if at_end {
            self.scroll_button_task = None;
            if self.show_scroll_button {
                self.show_scroll_button = false;
                cx.notify();
            }
        } else if !self.show_scroll_button && self.scroll_button_task.is_none() {
            self.scroll_button_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SCROLL_BUTTON_SHOW_DELAY).await;
                this.update(cx, |this, cx| {
                    this.scroll_button_task = None;
                    this.show_scroll_button = true;
                    cx.notify();
                })
                .ok();
            }));
        }
        if self.needs_animation(viewport) {
            self.ensure_animating(cx);
        }
    }

    /// The fork's reply-end rule: the last row is an assistant meta row and
    /// the reply above it still ends at most 8px under the composer top.
    fn reply_end_visible(&self, viewport: &ListViewport) -> bool {
        let last = self.rows.len().checked_sub(1);
        let Some(last) = last.filter(|i| matches!(self.rows[*i].kind, RowKind::AssistantMeta { .. })) else {
            return false;
        };
        let composer_top = viewport.bounds.bottom() - self.composer_inset - self.colors.r(TIMELINE_BOTTOM_PAD);
        viewport
            .row_bounds
            .iter()
            .find(|(index, _)| *index + 1 == last)
            .is_some_and(|(_, bounds)| bounds.bottom() <= composer_top + REPLY_END_SLACK)
    }

    fn needs_animation(&self, viewport: &ListViewport) -> bool {
        let live = viewport.visible.clone().any(|index| {
            matches!(
                self.rows.get(index).map(|r| &r.kind),
                Some(RowKind::WorkLive { active: true, .. } | RowKind::Thinking { .. } | RowKind::Working { .. })
                    | Some(RowKind::ContextCompaction { active: true, .. })
            )
        });
        live || self.markdown.values().any(|m| m.first_seen.iter().any(|t| self.now.duration_since(*t) < BLOCK_FADE))
    }

    /// Runs the stepped animation clock while something on screen animates.
    /// Nothing repaints per vsync: each step is one notify.
    fn ensure_animating(&mut self, cx: &mut Context<Self>) {
        if self.animation_task.is_some() {
            return;
        }
        self.animation_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ANIMATION_STEP).await;
                let keep = this
                    .update(cx, |this, cx| {
                        this.animation_step += 1;
                        this.now = Instant::now();
                        this.now_millis += ANIMATION_STEP.as_millis() as i64;
                        cx.notify();
                        let viewport = this.list.viewport();
                        this.needs_animation(&viewport)
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
            this.update(cx, |this, _| this.animation_task = None).ok();
        }));
    }

    /// Opacity of a streamed markdown piece that appeared at `seen`.
    pub(super) fn fade_opacity(&self, seen: Instant) -> f32 {
        let t = (self.now.duration_since(seen).as_secs_f32() / BLOCK_FADE.as_secs_f32()).clamp(0., 1.);
        // ease-out, quantized to the animation steps
        1. - (1. - t).powi(3)
    }

    /// A stable key for buttons and block state of a message.
    pub fn key_of(id: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        hasher.finish()
    }

    /// Bottom padding of a row (`MessagesTimeline.tsx:1660-1683`).
    fn row_bottom_padding(row: &TimelineRow) -> Pixels {
        match &row.kind {
            RowKind::Work { .. } | RowKind::WorkLive { .. } | RowKind::WorkToggle { .. } | RowKind::Thinking { .. } => px(0.),
            RowKind::WorkGroupDetails { .. } => px(0.),
            RowKind::Event { event: super::timeline_entries::TimelineEvent::Subagent(_), .. } => px(0.),
            RowKind::TurnFold { .. } | RowKind::Working { .. } => px(6.),
            RowKind::Message(m) if m.message.role == MessageRole::User && m.grouped_with_next => px(4.),
            RowKind::Message(m) if m.message.role == MessageRole::Assistant && !m.show_meta => px(8.),
            RowKind::WorktreeSetup(_)
            | RowKind::Event { .. }
            | RowKind::AttemptFold { .. }
            | RowKind::HtmlRender { .. }
            | RowKind::SentMessage { .. }
            | RowKind::McpApp { .. }
            | RowKind::ViewedImages { .. }
            | RowKind::PendingApproval { .. }
            | RowKind::AssistantMeta { .. } => px(8.),
            RowKind::QueuedMessages { .. } => px(0.),
            _ => px(16.),
        }
    }

    /// Renders row `index` inside the lane frame.
    fn render_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.rendering_row = index;
        let rows = self.rows.clone();
        let Some(row) = rows.get(index) else {
            return div().into_any_element();
        };
        let content = match &row.kind {
            RowKind::Message(message) => match message.message.role {
                MessageRole::User => self.render_user_message(row, message, window, cx),
                MessageRole::Assistant => self.render_assistant_message(row, message, window, cx),
            },
            RowKind::AssistantMeta { message } => self.render_assistant_meta(message, true, cx),
            RowKind::SentMessage { text } => self.render_sent_message(row, text, window, cx),
            RowKind::Work { entry, display_label } => self.render_work_entry(row, entry, display_label.clone(), false, cx),
            RowKind::WorkToggle { .. } => self.render_work_toggle(row, cx),
            RowKind::WorkGroupDetails { group_id, entries } => self.render_group_details(group_id, entries, window, cx),
            RowKind::WorkLive { .. } => self.render_work_live(row, cx),
            RowKind::Working { started_at, compacting } => self.render_working(*started_at, *compacting),
            RowKind::Thinking { label } => self.render_thinking(label.clone()),
            RowKind::TurnFold { run_id, label, expanded } => self.render_turn_fold(row, run_id, label, *expanded, cx),
            RowKind::AttemptFold { attempt_id, label, expanded } => self.render_attempt_fold(attempt_id, label, *expanded, cx),
            RowKind::ContextCompaction { label, tokens, active, failed } => {
                self.render_compaction(label, *tokens, *active, *failed)
            }
            RowKind::ViewedImages { paths } => self.render_viewed_images(paths),
            RowKind::Event { event, subagents } => self.render_event(row, event, subagents, cx),
            RowKind::ProposedPlan(plan) => self.render_proposed_plan(row, plan, window, cx),
            RowKind::HtmlRender { title } => self.render_embedded_frame("HTML render", title),
            RowKind::McpApp { title } => self.render_embedded_frame("App", title),
            RowKind::WorktreeSetup(setup) => self.render_worktree_setup(setup),
            RowKind::PendingApproval { entry } => self.render_pending_approval(entry, cx),
            RowKind::QueuedMessages { messages } => self.render_queued(messages, cx),
        };
        let bottom = if row.continues_work_log { px(0.) } else { Self::row_bottom_padding(row) };
        let standalone_work = matches!(row.kind, RowKind::Work { .. } | RowKind::WorkToggle { .. } | RowKind::WorkLive { .. } | RowKind::Thinking { .. })
            && !row.continues_work_log;
        let bottom = self.colors.r(if standalone_work { px(8.) } else { bottom });
        let gutter = self.gutter(window);
        div()
            .w_full()
            .px(gutter)
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    .when_some(self.lane.content, |d, w| d.max_w(w))
                    .overflow_x_hidden()
                    .pb(bottom)
                    .child(content),
            )
            .into_any_element()
    }

    /// The lane's side padding (`px-3 sm:px-12`): rem lengths at a breakpoint in CSS px.
    pub(super) fn gutter(&self, window: &Window) -> Pixels {
        self.colors.r(if window.viewport_size().width >= SM_BREAKPOINT { TIMELINE_GUTTER } else { TIMELINE_GUTTER_NARROW })
    }
}

impl Render for MessagesTimeline {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.colors = ChatColors::get(cx);
        self.lane = super::chat_style::ChatLane::get(cx);
        self.apply_insets();
        self.now = Instant::now().max(self.now);
        let weak = self.weak.clone();
        let colors = self.colors;
        if self.rows.is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .pb(self.composer_inset)
                .text_size(colors.r(px(14.)))
                .text_color(colors.theme.muted_foreground)
                .child("Send a message to start the conversation.")
                .into_any_element();
        }
        virtual_list("messages-timeline", self.list.clone(), move |index, window, cx| {
            weak.update(cx, |this, cx| this.render_row(index, window, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .scrollbar_colors(colors.scrollbar, colors.scrollbar_hover)
        .into_any_element()
    }
}

impl gpui_kit::EventEmitter<TimelineAction> for MessagesTimeline {}

/// The open find's count changed without a keystroke (the index finished).
#[derive(Clone, Copy, Debug)]
pub struct FindUpdated;

impl gpui_kit::EventEmitter<FindUpdated> for MessagesTimeline {}

/// Creates a timeline entity.
pub fn messages_timeline(cx: &mut App) -> Entity<MessagesTimeline> {
    cx.new(MessagesTimeline::new)
}
