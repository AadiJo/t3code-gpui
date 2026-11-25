//! One pull request: its header with state, branches and actions, and the
//! Summary, Guide, Timeline and Code tabs. Mounts on the pull requests page's
//! right side or as a right-panel tab. Ports `PullRequestDetailPanel.tsx`,
//! `PullRequestDetailActions.tsx`, `PullRequestSummaryTab.tsx`,
//! `PullRequestGuideTab.tsx`, `PullRequestTimelineTab.tsx` and
//! `PullRequestCodeTab.tsx`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{rems, 
    AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity,
    Window, div, prelude::FluentBuilder as _, px,
};
use t3code_client_runtime::pull_request_diff_fold::{DiffFoldOverride, PullRequestDiffFolds};
use t3code_client_runtime::pull_request_editing::{can_edit_pull_request_comment, can_resolve_pull_request_thread};
use t3code_client_runtime::pull_request_markdown::expand_github_emoji_aliases;
use t3code_client_runtime::pull_request_reactions::{
    PULL_REQUEST_REACTION_ORDER, PendingReactions, apply_pending_pull_request_reactions, can_react, pull_request_reaction_emoji,
};
use t3code_client_runtime::review_guide_tour as tour;
use t3code_contracts::*;

use super::pull_request_detail_logic::*;
use super::pull_request_presentation::*;
use super::pull_request_candidate_picker::{CandidatePicker, CandidatesChanged, PickerKind};
use super::pull_request_source::PullRequestDetailSource;
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::input::TextArea;
use crate::components::ui::popover::glass_surface;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::chat::chat_style::ChatColors;
use crate::components::chat::markdown_stream_blocks::parse_message_blocks;
use crate::components::chat_markdown::{MarkdownAction, MarkdownActions, MarkdownUiState, MarkdownView, render_markdown};
use crate::components::diffs::layout::Side;
use crate::components::diffs::{CodeView, CodeViewDecorations, CodeViewEvent, CodeViewFile, ViewMode};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::TextInput;
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::diff_patch::{FileContents, FileDiff, HunkBlock, parse_patch};
use crate::i18n::{t, t_rich};
use crate::markdown_pipeline::{MarkdownFlavor, ParsedBlocks};
use crate::{ActiveAppTheme as _, AppTheme};

/// The detail's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailTab {
    Summary,
    Guide,
    Timeline,
    Code,
}

impl DetailTab {
    /// Tab order.
    pub const ALL: [Self; 4] = [Self::Summary, Self::Guide, Self::Timeline, Self::Code];

    fn label(self) -> SharedString {
        match self {
            Self::Summary => t!("Summary", context = "pull request tab"),
            Self::Guide => t!("Guide", context = "pull request tab"),
            Self::Timeline => t!("Timeline", context = "pull request tab"),
            Self::Code => t!("Code", context = "pull request tab"),
        }
    }
}

/// What the detail asks its owner to do.
#[derive(Clone, Debug)]
pub enum PullRequestDetailEvent {
    /// The host changed the pull request; lists showing it should refresh.
    Acted(PullRequestAction),
    /// Open a link in the browser.
    OpenUrl(SharedString),
    /// Search the command palette for this pull request's URL, listing the threads that link it.
    SearchLinkedThreads { url: SharedString, threads: Vec<ThreadId> },
}

/// A reaction subject: the pull request itself or one comment.
type Subject = Option<String>;

/// The Code tab's state once its patch has loaded.
struct CodeState {
    view: Entity<CodeView>,
    files: Vec<std::sync::Arc<FileDiff>>,
    /// Milliseconds from opening the tab to its first frame.
    opened_at: Option<Instant>,
    _events: Subscription,
}

/// One pull request's detail.
pub struct PullRequestDetailPanel {
    source: Rc<dyn PullRequestDetailSource>,
    reference: PullRequestRef,
    detail: Option<PullRequestDetail>,
    activity: Option<PullRequestActivity>,
    guide: Option<PullRequestReviewGuideResult>,
    stack: Option<PullRequestStack>,
    viewed: HashMap<String, PullRequestFileViewedState>,
    viewed_overlay: HashMap<String, bool>,
    folds: PullRequestDiffFolds,
    tab: DetailTab,
    error: Option<SharedString>,
    status: Option<SharedString>,
    merge_method: Option<PullRequestMergeMethod>,
    confirming_merge: bool,
    busy: Option<PullRequestAction>,
    pending_reactions: HashMap<Subject, PendingReactions>,
    reply: Option<(String, Entity<InputState>)>,
    edit: Option<(String, PullRequestCommentKind, Entity<InputState>)>,
    composer: Option<Entity<InputState>>,
    picker_open: Option<Subject>,
    tour_position: usize,
    code: Option<CodeState>,
    /// Time from the Code tab click to its first painted frame, once measured.
    pub code_first_paint_ms: Option<f64>,
    markdown: HashMap<u64, Vec<ParsedBlocks>>,
    markdown_state: MarkdownUiState,
    show_bots: bool,
    show_resolved: bool,
    title_edit: Option<Entity<InputState>>,
    title_saving: bool,
    body_edit: Option<Entity<TextareaState>>,
    body_saving: bool,
    reviewer_picker: Option<Entity<CandidatePicker>>,
    label_picker: Option<Entity<CandidatePicker>>,
    composer_open: bool,
    composer_mode: ComposerMode,
    review_summary: Option<Entity<TextareaState>>,
    review_verdict: PullRequestReviewVerdict,
    review_pending: bool,
    confirm_delete_branch: bool,
    /// `pullRequests.changeBreakdown`, read with the detail (`eager`).
    breakdown: Option<PullRequestChangeBreakdownValue>,
    /// `pullRequests.linkedThreads`: `None` before the first answer, `Err` when it failed.
    linked_threads: Option<Result<Vec<ThreadId>, SharedString>>,
    /// Conversations the timeline shows, newest first (`COMMENT_PAGE` at a time).
    shown_conversations: usize,
    _picker_events: Vec<Subscription>,
    /// Fixed clock for scenes; `None` reads the system clock.
    pub now_ms: Option<i64>,
}

/// What the floating composer writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerMode {
    Comment,
    Review,
}

impl EventEmitter<PullRequestDetailEvent> for PullRequestDetailPanel {}

fn hash_key(parts: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

impl PullRequestDetailPanel {
    /// The detail of `reference`, read through `source`. Starts loading at once.
    pub fn new(source: Rc<dyn PullRequestDetailSource>, reference: PullRequestRef, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            source,
            reference,
            detail: None,
            activity: None,
            guide: None,
            stack: None,
            viewed: HashMap::new(),
            viewed_overlay: HashMap::new(),
            folds: PullRequestDiffFolds::default(),
            tab: DetailTab::Summary,
            error: None,
            status: None,
            merge_method: None,
            confirming_merge: false,
            busy: None,
            pending_reactions: HashMap::new(),
            reply: None,
            edit: None,
            composer: None,
            picker_open: None,
            tour_position: 0,
            code: None,
            code_first_paint_ms: None,
            markdown: HashMap::new(),
            markdown_state: MarkdownUiState::default(),
            show_bots: true,
            show_resolved: true,
            title_edit: None,
            title_saving: false,
            body_edit: None,
            body_saving: false,
            reviewer_picker: None,
            label_picker: None,
            composer_open: false,
            composer_mode: ComposerMode::Comment,
            review_summary: None,
            review_verdict: PullRequestReviewVerdict::Comment,
            review_pending: false,
            confirm_delete_branch: false,
            breakdown: None,
            linked_threads: None,
            shown_conversations: COMMENT_PAGE,
            _picker_events: Vec::new(),
            now_ms: None,
        };
        panel.refresh(cx);
        panel
    }

    /// The tab shown.
    pub fn tab(&self) -> DetailTab {
        self.tab
    }

    /// The detail as last read, with any checks read laid over it.
    pub fn detail(&self) -> Option<&PullRequestDetail> {
        self.detail.as_ref()
    }

    /// The activity as last read.
    pub fn activity(&self) -> Option<&PullRequestActivity> {
        self.activity.as_ref()
    }

    /// Reads the detail, activity, stack, files viewed, guide and change breakdown again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let linked = self.source.linked_threads(self.reference.clone());
        cx.spawn(async move |this, cx| {
            let result = linked.await;
            this.update(cx, |this, cx| {
                // A failed refresh keeps the last count, as the fork does.
                match result {
                    Ok(answer) => this.linked_threads = Some(Ok(answer.threads.into_iter().map(|t| t.id).collect())),
                    Err(e) if this.linked_threads.is_none() => this.linked_threads = Some(Err(e.0)),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        let breakdown = self.source.change_breakdown(self.reference.clone());
        cx.spawn(async move |this, cx| {
            if let Ok(value) = breakdown.await {
                this.update(cx, |this, cx| {
                    this.breakdown = value;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        let r = self.reference.clone();
        let (detail, checks, activity, stack, viewed, guide) = (
            self.source.detail(r.clone()),
            self.source.checks(r.clone()),
            self.source.activity(r.clone()),
            self.source.stack(r.clone()),
            self.source.files_viewed(r.clone()),
            self.source.review_guide(r),
        );
        cx.spawn(async move |this, cx| {
            let detail = detail.await;
            let checks = checks.await.ok().flatten();
            let (activity, stack, viewed, guide) = (activity.await, stack.await, viewed.await, guide.await);
            this.update(cx, |this, cx| {
                match detail {
                    Ok(d) => this.detail = t3code_client_runtime::pull_request_checks::resolve_pull_request_detail_checks(Some(&d), checks.as_ref(), true),
                    Err(e) => this.error = Some(e.0),
                }
                if let Ok(a) = activity {
                    this.activity = Some(a);
                    this.pending_reactions.clear();
                }
                this.stack = stack.ok().flatten();
                if let Ok(v) = viewed {
                    this.viewed = v.files.into_iter().map(|f| (f.path, f.state)).collect();
                    let viewed = &this.viewed;
                    this.viewed_overlay.retain(|path, pressed| viewed.get(path).map(|s| *s == PullRequestFileViewedState::Viewed) != Some(*pressed));
                }
                this.guide = guide.ok();
                this.sync_code_decorations(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn now(&self) -> i64 {
        self.now_ms.unwrap_or_else(crate::timestamp_format::now_ms)
    }

    /// Shows `tab`, loading the patch the first time the Code tab opens.
    pub fn set_tab(&mut self, tab: DetailTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == DetailTab::Code && self.code.is_none() {
            self.load_code(window, cx);
        }
        cx.notify();
    }

    /// Reads both sides of one file so its collapsed context can expand (`pullRequests.diffFileContents`).
    fn load_contents(&mut self, key: SharedString, cx: &mut Context<Self>) {
        let Some(code) = &self.code else { return };
        let Some(file) = code.files.iter().find(|f| f.path == key).cloned() else { return };
        let r = &self.reference;
        let change_type = ReviewDiffChangeType::from_wire(file.change.as_str()).unwrap_or(ReviewDiffChangeType::Change);
        let request = self.source.diff_file_contents(PullRequestDiffFileContentsInput {
            project_id: r.project_id.clone(),
            host: r.host.clone(),
            expected_account_id: r.expected_account_id.clone(),
            allow_stale: r.allow_stale,
            repository: r.repository.clone(),
            number: r.number,
            commit: None,
            change_type,
            old_path: file.prev_path.clone().unwrap_or_else(|| file.path.clone()).to_string(),
            new_path: file.path.to_string(),
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| match result {
                Ok(contents) => {
                    if let Some(code) = &this.code {
                        let contents = std::sync::Arc::new(FileContents::new(&contents.old_contents, &contents.new_contents));
                        code.view.update(cx, |view, cx| view.set_contents(&key, contents, cx));
                    }
                }
                Err(error) => this.status = Some(error.0),
            })
            .ok();
        })
        .detach();
    }

    fn load_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let started = Instant::now();
        let view = cx.new(|cx| CodeView::new(ViewMode::Diff, window, cx));
        let events = cx.subscribe(&view, |this, _, event: &CodeViewEvent, cx| match event {
            CodeViewEvent::CollapsedChanged { key, collapsed } => this.folds = this.folds.with_fold(false, key, Some(*collapsed)),
            CodeViewEvent::RequestContents { key } => this.load_contents(key.clone(), cx),
            _ => {}
        });
        self.code = Some(CodeState { view, files: Vec::new(), opened_at: Some(started), _events: events });
        let r = &self.reference;
        let request = self.source.diff(PullRequestDiffInput {
            project_id: r.project_id.clone(),
            host: r.host.clone(),
            expected_account_id: None,
            allow_stale: None,
            repository: r.repository.clone(),
            number: r.number,
            cursor: None,
            commit: None,
            path: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = request.await else { return };
            this.update(cx, |this, cx| this.set_patch(&result.patch, cx)).ok();
        })
        .detach();
    }

    /// Shows `patch` in the Code tab.
    pub fn set_patch(&mut self, patch: &str, cx: &mut Context<Self>) {
        let files = parse_patch(patch).files;
        let Some(code) = self.code.as_mut() else { return };
        code.files = files.clone();
        let entries = files
            .iter()
            .map(|diff| {
                let key = diff.path.clone();
                let viewed = self.viewed_overlay.get(key.as_ref()).copied().unwrap_or_else(|| self.viewed.get(key.as_ref()) == Some(&PullRequestFileViewedState::Viewed));
                CodeViewFile { collapsed: self.folds.is_collapsed(&key, false, viewed), diff: diff.clone(), key, stat: None }
            })
            .collect();
        code.view.update(cx, |view, cx| view.set_files(entries, cx));
        self.sync_code_decorations(cx);
        cx.notify();
    }

    fn sync_code_decorations(&mut self, cx: &mut Context<Self>) {
        let Some(code) = &self.code else { return };
        let threads = self.activity.as_ref().map(|a| a.review_threads.clone()).unwrap_or_default();
        let viewed: HashSet<String> = code
            .files
            .iter()
            .map(|f| f.path.to_string())
            .filter(|p| self.is_viewed(p))
            .collect();
        let decorations = Rc::new(DetailDecorations { panel: cx.entity().downgrade(), threads, viewed });
        code.view.update(cx, |view, cx| view.set_decorations(Some(decorations), cx));
    }

    /// Whether `path` shows as viewed, the reader's press winning until the host agrees.
    pub fn is_viewed(&self, path: &str) -> bool {
        self.viewed_overlay.get(path).copied().unwrap_or_else(|| self.viewed.get(path) == Some(&PullRequestFileViewedState::Viewed))
    }

    /// Marks `path` viewed or not, folding it to match, and writes it to the host.
    pub fn toggle_viewed(&mut self, path: &str, cx: &mut Context<Self>) {
        let viewed = !self.is_viewed(path);
        self.viewed_overlay.insert(path.to_owned(), viewed);
        self.folds = self.folds.with_fold(false, path, None);
        if let Some(code) = &self.code {
            code.view.update(cx, |v, cx| v.set_collapsed(path, viewed, cx));
        }
        let r = &self.reference;
        let request = self.source.set_files_viewed(PullRequestSetFilesViewedInput {
            project_id: r.project_id.clone(),
            host: r.host.clone(),
            expected_account_id: None,
            allow_stale: None,
            repository: r.repository.clone(),
            number: r.number,
            files: vec![PullRequestSetFilesViewedInputFilesItem { path: path.to_owned(), viewed, revision: None }],
        });
        let viewed_read = self.source.files_viewed(r.clone());
        let path = path.to_owned();
        cx.spawn(async move |this, cx| {
            let wrote = request.await.is_ok();
            let read = if wrote { viewed_read.await.ok() } else { None };
            this.update(cx, |this, cx| {
                match read {
                    Some(v) => this.viewed = v.files.into_iter().map(|f| (f.path, f.state)).collect(),
                    None if !wrote => {
                        this.viewed_overlay.remove(&path);
                    }
                    None => {}
                }
                if this.viewed.get(&path).map(|s| *s == PullRequestFileViewedState::Viewed) == this.viewed_overlay.get(&path).copied() {
                    this.viewed_overlay.remove(&path);
                }
                this.sync_code_decorations(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        self.sync_code_decorations(cx);
        cx.notify();
    }

    fn reference_fields(&self) -> (ProjectId, Option<String>, String, u64) {
        let r = &self.reference;
        (r.project_id.clone(), r.host.clone(), r.repository.clone(), r.number)
    }

    /// Runs a header action. A merge uses the chosen method.
    pub fn run_action(&mut self, action: PullRequestAction, cx: &mut Context<Self>) {
        let (project_id, host, repository, number) = self.reference_fields();
        let merge_method = (action == PullRequestAction::Merge).then(|| self.current_merge_method()).flatten();
        let update_method = (action == PullRequestAction::UpdateBranch).then(|| self.detail.as_ref().and_then(resolve_base_freshness).and_then(|(_, m)| m.first().copied())).flatten();
        let request = self.source.run_action(PullRequestActionInput {
            stack_number: None,
            expected_stack_heads: None,
            expected_head_sha: self.detail.as_ref().and_then(|d| d.head_sha.clone()),
            project_id,
            host,
            expected_account_id: None,
            allow_stale: None,
            repository,
            number,
            action,
            merge_method,
            update_method,
            reply_with_summary: None,
        });
        self.busy = Some(action);
        self.confirming_merge = false;
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(_) => {
                        this.status = None;
                        cx.emit(PullRequestDetailEvent::Acted(action));
                        this.refresh(cx);
                    }
                    Err(e) => this.status = Some(e.0),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn current_merge_method(&self) -> Option<PullRequestMergeMethod> {
        let detail = self.detail.as_ref()?;
        let allowed = allowed_pull_request_merge_methods(detail);
        (!allowed.is_empty()).then(|| resolve_pull_request_merge_method(&allowed, self.merge_method, None, PullRequestMergeMethod::Squash))
    }

    /// Asks for confirmation before merging.
    pub fn begin_merge(&mut self, cx: &mut Context<Self>) {
        self.confirming_merge = true;
        cx.notify();
    }

    /// Checks the branch out into a new thread.
    pub fn checkout_into_thread(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let request = self.source.prepare_thread(GitPreparePullRequestThreadInput {
            cwd: detail.workspace_root.clone(),
            reference: detail.url.clone(),
            mode: ThreadEnvMode::Worktree,
            thread_id: None,
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.status = result.err().map(|e| e.0);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // --- editing, pickers, review and checkout ---------------------------

    fn update_input(&self) -> PullRequestUpdateInput {
        let r = &self.reference;
        PullRequestUpdateInput {
            project_id: r.project_id.clone(),
            host: r.host.clone(),
            expected_account_id: r.expected_account_id.clone(),
            allow_stale: r.allow_stale,
            repository: r.repository.clone(),
            number: r.number,
            title: None,
            body: None,
            maintainer_can_modify: None,
        }
    }

    /// Opens the title editor (`PullRequestTitleEditor`).
    pub fn begin_title_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(detail.title.clone()));
        input.update(cx, |i, cx| i.focus(window, cx));
        let events = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.save_title(cx);
            }
        });
        self._picker_events.push(events);
        self.title_edit = Some(input);
        cx.notify();
    }

    /// Saves the edited title; the editor closes at once and reopens if the host refuses it.
    pub fn save_title(&mut self, cx: &mut Context<Self>) {
        let (Some(input), Some(detail)) = (&self.title_edit, &self.detail) else { return };
        if self.title_saving {
            return;
        }
        let title = input.read(cx).value().trim().to_owned();
        if title.is_empty() || title == detail.title {
            self.title_edit = None;
            cx.notify();
            return;
        }
        self.title_saving = true;
        let previous = detail.title.clone();
        if let Some(d) = &mut self.detail {
            d.title = title.clone();
        }
        let request = self.source.update(PullRequestUpdateInput { title: Some(title), ..self.update_input() });
        let kept = self.title_edit.take();
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.title_saving = false;
                if let Err(error) = result {
                    if let Some(d) = &mut this.detail {
                        d.title = previous;
                    }
                    this.title_edit = kept;
                    Toaster::push(ToastData::new(ToastKind::Error, t!("The title could not be saved")).description(error.0), cx);
                } else {
                    this.refresh(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Opens the description editor.
    pub fn begin_body_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let body = detail.body.clone();
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 12).default_value(body));
        input.update(cx, |i, cx| i.focus(window, cx));
        self.body_edit = Some(input);
        cx.notify();
    }

    /// Saves the edited description (an empty one is allowed).
    pub fn save_body(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.body_edit else { return };
        if self.body_saving {
            return;
        }
        let body = input.read(cx).value().to_string();
        self.body_saving = true;
        let request = self.source.update(PullRequestUpdateInput { body: Some(body.clone()), ..self.update_input() });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.body_saving = false;
                match result {
                    Ok(()) => {
                        if let Some(d) = &mut this.detail {
                            d.body = body;
                        }
                        this.body_edit = None;
                        this.refresh(cx);
                    }
                    Err(error) => {
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not save the description")).description(error.0), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Creates the reviewer and label pickers once the detail says what the host allows.
    fn ensure_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        if self.reviewer_picker.is_none() && detail.capabilities.reviewers.request && detail.capabilities.reviewers.list_candidates {
            let (source, reference) = (self.source.clone(), self.reference.clone());
            let allowed = detail.viewer_permissions.request_reviewers;
            let searchable = detail.capabilities.reviewers.search_candidates == Some(true);
            let picker = cx.new(|cx| CandidatePicker::new(PickerKind::Reviewers, source, reference, allowed, searchable, window, cx));
            self._picker_events.push(cx.subscribe(&picker, |this, _, _: &CandidatesChanged, cx| this.refresh(cx)));
            self.reviewer_picker = Some(picker);
        }
        if self.label_picker.is_none() && detail.capabilities.labels == Some(true) {
            let (source, reference) = (self.source.clone(), self.reference.clone());
            let allowed = detail.viewer_permissions.labels != Some(false);
            let picker = cx.new(|cx| CandidatePicker::new(PickerKind::Labels, source, reference, allowed, false, window, cx));
            self._picker_events.push(cx.subscribe(&picker, |this, _, _: &CandidatesChanged, cx| this.refresh(cx)));
            self.label_picker = Some(picker);
        }
    }

    /// The verdicts the host offers and the viewer may give.
    fn offered_verdicts(&self) -> Vec<PullRequestReviewVerdict> {
        let Some(detail) = &self.detail else { return vec![] };
        [PullRequestReviewVerdict::Comment, PullRequestReviewVerdict::Approve, PullRequestReviewVerdict::RequestChanges]
            .into_iter()
            .filter(|v| detail.capabilities.review.verdicts.contains(v) && detail.viewer_permissions.verdicts.contains(v))
            .collect()
    }

    /// Forgejo needs a summary to request changes; an approval needs nothing.
    fn can_submit_review(&self, verdict: PullRequestReviewVerdict, summary: &str) -> bool {
        let summary_required = self.detail.as_ref().is_some_and(|d| d.provider.as_str() == "forgejo");
        match verdict {
            PullRequestReviewVerdict::RequestChanges if summary_required => !summary.trim().is_empty(),
            PullRequestReviewVerdict::Approve => true,
            _ => !summary.trim().is_empty(),
        }
    }

    /// Opens or closes the floating composer, creating its fields the first time.
    pub fn toggle_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer_open = !self.composer_open;
        if self.composer.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Leave a comment")));
            self.composer = Some(input);
        }
        if self.review_summary.is_none() {
            let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 12).placeholder(t!("Summarize your review (optional)")));
            self.review_summary = Some(input);
        }
        if self.composer_open {
            let started = self.review_summary.as_ref().is_some_and(|s| !s.read(cx).value().trim().is_empty());
            let can_comment = self.detail.as_ref().is_some_and(|d| d.capabilities.comment && d.viewer_permissions.comment);
            self.composer_mode = if started || !can_comment { ComposerMode::Review } else { ComposerMode::Comment };
            match self.composer_mode {
                ComposerMode::Comment => self.composer.as_ref().map(|c| c.update(cx, |c, cx| c.focus(window, cx))),
                ComposerMode::Review => self.review_summary.as_ref().map(|c| c.update(cx, |c, cx| c.focus(window, cx))),
            };
        }
        cx.notify();
    }

    /// Sends the review with the chosen verdict (`pullRequests.submitReview`).
    pub fn submit_review(&mut self, cx: &mut Context<Self>) {
        let Some(summary) = &self.review_summary else { return };
        let offered = self.offered_verdicts();
        let verdict = if offered.contains(&self.review_verdict) { self.review_verdict } else { match offered.first() { Some(v) => *v, None => return } };
        let body = summary.read(cx).value().to_string();
        if self.review_pending || !self.can_submit_review(verdict, &body) {
            return;
        }
        self.review_pending = true;
        let r = &self.reference;
        let request = self.source.submit_review(PullRequestSubmitReviewInput {
            project_id: r.project_id.clone(),
            host: r.host.clone(),
            expected_account_id: r.expected_account_id.clone(),
            allow_stale: r.allow_stale,
            repository: r.repository.clone(),
            number: r.number,
            verdict,
            body,
            comments: Vec::new(),
        });
        let term = self.detail.as_ref().map(|d| term_select(d.provider.as_str())).unwrap_or("pr");
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.review_pending = false;
                match result {
                    Ok(()) => {
                        this.review_summary = None;
                        this.composer_open = false;
                        let title = match verdict {
                            PullRequestReviewVerdict::Comment => t!("Review submitted"),
                            PullRequestReviewVerdict::Approve => {
                                t!("{term, select, mr {Merge request} cr {Change request} other {Pull request}} approved", term = term)
                            }
                            PullRequestReviewVerdict::RequestChanges => t!("Changes requested"),
                        };
                        Toaster::push(ToastData::new(ToastKind::Success, title), cx);
                        this.refresh(cx);
                    }
                    Err(error) => {
                        Toaster::push(ToastData::new(ToastKind::Error, t!("The review could not be submitted")).description(error.0), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Checks the branch out into a new thread: in a separate worktree or in this repository.
    pub fn checkout(&mut self, mode: ThreadEnvMode, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let request = self.source.prepare_thread(GitPreparePullRequestThreadInput {
            cwd: detail.workspace_root.clone(),
            reference: detail.url.clone(),
            mode,
            thread_id: None,
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.status = result.err().map(|e| e.0);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `canDeletePullRequestBranch`: a closed or merged pull request the host lets the viewer prune.
    fn can_delete_branch(&self) -> bool {
        self.detail.as_ref().is_some_and(|d| {
            d.state != PullRequestState::Open
                && d.capabilities.delete_branch == Some(true)
                && d.capabilities.actions.contains(&PullRequestAction::DeleteBranch)
                && d.viewer_permissions.actions.contains(&PullRequestAction::DeleteBranch)
        })
    }

    fn render_delete_branch_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.confirm_delete_branch {
            return None;
        }
        let detail = self.detail.as_ref()?;
        let repository = detail.head_repository_name_with_owner.clone().unwrap_or_else(|| detail.repository.clone());
        let close = cx.listener(|this, _: &(), _, cx| {
            this.confirm_delete_branch = false;
            cx.notify();
        });
        let close = Rc::new(close);
        let surface = DialogSurface::new(t!("Delete branch {branch}?", branch = detail.head_branch.as_str()))
            .description(t!(
                "This deletes {branch} from {repository}. Local branches and worktrees are not changed.",
                branch = detail.head_branch.as_str(),
                repository = repository.as_str()
            ))
            .footer(Button::new("pr-delete-branch-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).on_click({
                let close = close.clone();
                move |_, window, cx| close(&(), window, cx)
            }))
            .footer(Button::new("pr-delete-branch-confirm").variant(ButtonVariant::Destructive).label(t!("Delete branch")).on_click(cx.listener(|this, _, _, cx| {
                this.confirm_delete_branch = false;
                this.run_action(PullRequestAction::DeleteBranch, cx);
            })));
        let theme = cx.app_theme();
        Some(
            Modal::dialog("pr-delete-branch", div().id("dialog-content").occlude().w_full().max_w(theme.rem(32.)).child(surface), move |window, cx| close(&(), window, cx))
                .into_any_element(),
        )
    }

    /// The floating composer: a glass button and, while open, the Comment / Review popover above it.
    fn render_composer(&self, theme: &AppTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let detail = self.detail.as_ref()?;
        let can_comment = detail.capabilities.comment && detail.viewer_permissions.comment;
        let verdicts = self.offered_verdicts();
        if !can_comment && verdicts.is_empty() {
            return None;
        }
        let term = term_select(detail.provider.as_str());
        let mode = if !can_comment { ComposerMode::Review } else if verdicts.is_empty() { ComposerMode::Comment } else { self.composer_mode };
        let c = &theme.colors;
        let button = Button::new("pr-composer-trigger")
            .variant(ButtonVariant::Glass)
            .size(ButtonSize::Icon)
            .icon(IconName::MessageSquare)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_composer(window, cx)));
        let popover = self.composer_open.then(|| {
            let header = if can_comment && !verdicts.is_empty() {
                let segment = |id: &'static str, label: SharedString, value: ComposerMode| {
                    let on = mode == value;
                    div()
                        .id(id)
                        .test_support()
                        .flex()
                        .items_center()
                        .h(theme.sp(6.))
                        .px(theme.sp(2.5))
                        .rounded(theme.radii.sm)
                        .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                        .text_color(if on { c.foreground() } else { c.muted_foreground })
                        .when(on, |el| el.bg(c.background()).shadow(theme.shadows.sm5.clone()))
                        .child(label)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.composer_mode = value;
                            cx.notify();
                        }))
                };
                div()
                    .flex()
                    .p(rems(0.125))
                    .gap(rems(0.125))
                    .rounded(theme.radii.md)
                    .bg(c.muted)
                    .child(segment("pr-composer-comment", t!("Comment", context = "noun, composer mode"), ComposerMode::Comment))
                    .child(segment("pr-composer-review", t!("Review", context = "noun, composer mode"), ComposerMode::Review))
                    .into_any_element()
            } else {
                let title = if mode == ComposerMode::Review {
                    t!("Review {term, select, mr {merge request} cr {change request} other {pull request}}", term = term)
                } else {
                    t!("Comment on {term, select, mr {merge request} cr {change request} other {pull request}}", term = term)
                };
                div().ui_text(theme, TextSize::Sm, weight::SEMIBOLD).child(title).into_any_element()
            };
            let body: AnyElement = match mode {
                ComposerMode::Comment => match &self.composer {
                    Some(input) => div()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(2.))
                        .child(TextInput::new(input))
                        .child(
                            div().flex().justify_end().child(
                                Button::new("pr-comment-send")
                                    .size(ButtonSize::Xs)
                                    .label(t!("Comment", context = "verb, submit button"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.send_comment(cx);
                                        this.composer_open = false;
                                        cx.notify();
                                    })),
                            ),
                        )
                        .into_any_element(),
                    None => div().into_any_element(),
                },
                ComposerMode::Review => {
                    let summary = self.review_summary.as_ref().map(|s| s.read(cx).value().to_string()).unwrap_or_default();
                    let verdict = if verdicts.contains(&self.review_verdict) { self.review_verdict } else { verdicts.first().copied().unwrap_or(PullRequestReviewVerdict::Comment) };
                    let can_submit = self.can_submit_review(verdict, &summary);
                    let this = cx.entity();
                    let verdict_menu = (verdicts.len() > 1).then(|| {
                        let options = verdicts.clone();
                        dropdown_menu(
                            "pr-review-verdict",
                            Button::new("pr-review-verdict-trigger")
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Xs)
                                .icon(verdict_icon(verdict))
                                .icon_size(IconSize::Xs)
                                .label(verdict_label(verdict))
                                .trailing_icon(IconName::ChevronsUpDown)
                                .disabled(self.review_pending),
                            move |_, _| {
                                let mut menu = MenuPopup::new();
                                for v in options.clone() {
                                    let this = this.clone();
                                    menu = menu.item(
                                        MenuItem::new(verdict_label(v))
                                            .icon(verdict_icon(v))
                                            .icon_size(IconSize::Xs)
                                            .kind(MenuItemKind::Radio(v == verdict))
                                            .on_select(move |_, cx| {
                                                this.update(cx, |p, cx| {
                                                    p.review_verdict = v;
                                                    cx.notify();
                                                })
                                            }),
                                    );
                                }
                                menu
                            },
                        )
                    });
                    div()
                        .flex()
                        .flex_col()
                        .children(self.review_summary.as_ref().map(TextArea::new))
                        .child(
                            div()
                                .mt(theme.sp(2.))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_end()
                                .gap(theme.sp(2.))
                                .children(verdict_menu)
                                .child(
                                    Button::new("pr-review-submit")
                                        .size(ButtonSize::Xs)
                                        .label(if self.review_pending { t!("Submitting...") } else { t!("Submit review") })
                                        .disabled(self.review_pending || !can_submit)
                                        .on_click(cx.listener(|this, _, _, cx| this.submit_review(cx))),
                                ),
                        )
                        .into_any_element()
                }
            };
            glass_surface(theme)
                .id("pr-composer")
                .occlude()
                .w(theme.rem(24.))
                .p(theme.sp(4.))
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .child(
                    div()
                        .mb(theme.sp(3.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme.sp(2.))
                        .child(header)
                        .child(
                            Button::new("pr-composer-close")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::IconXs)
                                .icon(IconName::X)
                                .icon_size(IconSize::Sm)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.composer_open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(body)
        });
        Some(
            div()
                .absolute()
                .right(theme.sp(4.))
                .bottom(theme.sp(3.))
                .flex()
                .flex_col()
                .items_end()
                .gap(rems(0.5))
                .children(popover)
                .child(button)
                .into_any_element(),
        )
    }

    /// Toggles the viewer's `content` reaction on `subject`.
    pub fn toggle_reaction(&mut self, subject: Subject, content: IssueCommentReactionsItemContent, cx: &mut Context<Self>) {
        let current = self.reactions_of(&subject);
        let reacted = !current.iter().any(|r| r.content == content && r.viewer_has_reacted);
        self.pending_reactions.entry(subject.clone()).or_default().insert(content, reacted);
        self.picker_open = None;
        let (project_id, host, repository, number) = self.reference_fields();
        let request = self.source.set_reaction(PullRequestReactionInput {
            project_id,
            host,
            expected_account_id: None,
            allow_stale: None,
            repository,
            number,
            subject_id: subject,
            content: PullRequestReactionInputContent::from_wire(content.as_str()).expect("same reaction set"),
            reacted,
        });
        self.after_write(request, cx);
        cx.notify();
    }

    fn raw_reactions(&self, subject: &Subject) -> Vec<IssueCommentReactionsItem> {
        let Some(activity) = &self.activity else { return vec![] };
        match subject {
            None => activity.reactions.clone().unwrap_or_default(),
            Some(id) => activity.comments.iter().find(|c| &c.id == id).and_then(|c| c.reactions.clone()).unwrap_or_default(),
        }
    }

    fn reactions_of(&self, subject: &Subject) -> Vec<IssueCommentReactionsItem> {
        let raw = self.raw_reactions(subject);
        match self.pending_reactions.get(subject) {
            Some(pending) => apply_pending_pull_request_reactions(&raw, pending),
            None => raw,
        }
    }

    fn after_write(&mut self, request: super::pull_request_source::RpcFuture<()>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.status = result.err().map(|e| e.0);
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens the reply box under a review thread.
    pub fn begin_reply(&mut self, thread_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Reply", context = "noun, placeholder")));
        input.update(cx, |i, cx| i.focus(window, cx));
        self.reply = Some((thread_id.to_owned(), input));
        cx.notify();
    }

    /// Sends the open reply.
    pub fn send_reply(&mut self, cx: &mut Context<Self>) {
        let Some((thread_id, input)) = self.reply.take() else { return };
        let body = input.read(cx).value().to_string();
        if body.trim().is_empty() {
            return;
        }
        let (project_id, host, repository, number) = self.reference_fields();
        let request = self.source.reply_to_thread(PullRequestThreadReplyInput {
            project_id,
            host,
            expected_account_id: None,
            allow_stale: None,
            repository,
            number,
            thread_id,
            body,
        });
        self.after_write(request, cx);
        cx.notify();
    }

    /// Resolves or unresolves a review thread.
    pub fn set_resolved(&mut self, thread_id: &str, resolved: bool, cx: &mut Context<Self>) {
        let (project_id, host, repository, number) = self.reference_fields();
        let request = self.source.set_thread_resolution(PullRequestThreadResolutionInput {
            project_id,
            host,
            expected_account_id: None,
            allow_stale: None,
            repository,
            number,
            thread_id: thread_id.to_owned(),
            resolved,
        });
        self.after_write(request, cx);
    }

    /// Opens the editor on one of the viewer's comments.
    pub fn begin_edit(&mut self, comment: &PullRequestComment, window: &mut Window, cx: &mut Context<Self>) {
        let body = comment.body.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(body));
        self.edit = Some((comment.id.clone(), comment.kind, input));
        cx.notify();
    }

    /// Saves the comment being edited.
    pub fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some((comment_id, kind, input)) = self.edit.take() else { return };
        let body = input.read(cx).value().to_string();
        let kind = match kind {
            PullRequestCommentKind::ReviewComment => PullRequestCommentUpdateInputKind::ReviewComment,
            _ => PullRequestCommentUpdateInputKind::IssueComment,
        };
        let (project_id, host, repository, number) = self.reference_fields();
        let request = self.source.update_comment(PullRequestCommentUpdateInput {
            project_id,
            host,
            expected_account_id: None,
            allow_stale: None,
            repository,
            number,
            comment_id,
            kind,
            body,
        });
        self.after_write(request, cx);
    }

    /// Posts a top-level comment from the composer.
    pub fn send_comment(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.composer else { return };
        let body = input.read(cx).value().to_string();
        if body.trim().is_empty() {
            return;
        }
        self.composer = None;
        let (project_id, host, repository, number) = self.reference_fields();
        let request = self.source.comment(PullRequestCommentInput { project_id, host, expected_account_id: None, allow_stale: None, repository, number, body });
        self.after_write(request, cx);
    }

    /// Asks the server to write a review guide.
    pub fn generate_guide(&mut self, cx: &mut Context<Self>) {
        let request = self.source.generate_review_guide(self.reference.clone());
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(g) => this.guide = Some(g),
                    Err(e) => this.status = Some(e.0),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Steps the guide tour.
    pub fn step_tour(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.guide.as_ref().and_then(|g| g.guide.as_ref()).map_or(0, |g| g.stops.len());
        self.tour_position = tour::step_tour_position(self.tour_position, forward, count);
        cx.notify();
    }

    /// Expands or folds every file.
    pub fn fold_all(&mut self, folded: bool, cx: &mut Context<Self>) {
        self.folds = self.folds.fold_all(if folded { DiffFoldOverride::Folded } else { DiffFoldOverride::Expanded }, None);
        if let Some(code) = &self.code {
            let keys: Vec<SharedString> = code.files.iter().map(|f| f.path.clone()).collect();
            code.view.update(cx, |v, cx| {
                for key in keys {
                    v.set_collapsed(&key, folded, cx);
                }
            });
        }
        cx.notify();
    }

    fn markdown(&mut self, key: &str, text: &str) -> (u64, Vec<ParsedBlocks>) {
        let k = hash_key(&format!("{key}\0{}\0{text}", crate::markdown_pipeline::math_enabled()));
        let pieces = self.markdown.entry(k).or_insert_with(|| parse_message_blocks(&expand_github_emoji_aliases(text), MarkdownFlavor::ASSISTANT, false)).clone();
        (k, pieces)
    }

    fn markdown_actions(&self, cx: &mut Context<Self>) -> MarkdownActions {
        let this = cx.entity().downgrade();
        Rc::new(move |action, _, cx| {
            this.update(cx, |this, cx| {
                match action {
                    MarkdownAction::OpenLink(url) => cx.emit(PullRequestDetailEvent::OpenUrl(url)),
                    MarkdownAction::RunShellCommand(_) => {}
                    MarkdownAction::ToggleDetails(k) => {
                        if !this.markdown_state.open_details.remove(&k) {
                            this.markdown_state.open_details.insert(k);
                        }
                    }
                    MarkdownAction::ToggleWrap(k) => {
                        if !this.markdown_state.unwrapped.remove(&k) {
                            this.markdown_state.unwrapped.insert(k);
                        }
                    }
                    MarkdownAction::Copy { key, text } => {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.to_string()));
                        this.markdown_state.copied = Some(key);
                    }
                }
                cx.notify();
            })
            .ok();
        })
    }

    fn render_markdown(&mut self, key: &str, text: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (k, pieces) = self.markdown(key, text);
        let colors = ChatColors::get(cx);
        let actions = self.markdown_actions(cx);
        let view = MarkdownView { key: k, pieces: &pieces, colors: &colors, user: false, streaming: false, piece_opacity: None, find: None, state: &self.markdown_state, actions };
        render_markdown(&view, window, cx).into_any_element()
    }
}

/// The Code tab's review threads and Viewed toggles, drawn into the code view.
struct DetailDecorations {
    panel: WeakEntity<PullRequestDetailPanel>,
    threads: Vec<PullRequestReviewThread>,
    viewed: HashSet<String>,
}

const THREAD_COMMENT_HEIGHT: f32 = 46.;

impl CodeViewDecorations for DetailDecorations {
    fn annotations(&self, file_key: &SharedString) -> Vec<((Side, u32), f32)> {
        self.threads
            .iter()
            .filter(|t| t.path == file_key.as_ref())
            .filter_map(|t| {
                let side = if t.side == PullRequestDiffSide::Left { Side::Old } else { Side::New };
                Some(((side, t.line? as u32), 34. + THREAD_COMMENT_HEIGHT * t.comments.len() as f32))
            })
            .collect()
    }

    fn render_annotation(&self, file_key: &SharedString, anchor: (Side, u32), _: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let theme = cx.app_theme().clone();
        let threads: Vec<&PullRequestReviewThread> = self
            .threads
            .iter()
            .filter(|t| t.path == file_key.as_ref() && t.line.map(|l| l as u32) == Some(anchor.1))
            .collect();
        if threads.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .children(threads.into_iter().map(|thread| {
                    let id = thread.id.clone();
                    let resolved = thread.is_resolved;
                    let panel = self.panel.clone();
                    div()
                        .mx(rems(0.75))
                        .my(rems(0.25))
                        .rounded(theme.radii.md)
                        .border_1()
                        .border_color(theme.colors.border)
                        .bg(theme.colors.card())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rems(0.375))
                                .h(rems(1.5))
                                .px(rems(0.625))
                                .ui_text(&theme, TextSize::Xs3, weight::MEDIUM)
                                .text_color(theme.colors.muted_foreground)
                                .child(Icon::new(IconName::MessageSquare).size(IconSize::Xs))
                                .child(if resolved { t!("Resolved", context = "review thread state") } else { t!("Open", context = "review thread state") })
                                .child(
                                    div()
                                        .id(SharedString::from(format!("code-resolve-{id}")))
                                        .test_support()
                                        .ml_auto()
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(theme.colors.text))
                                        .child(if resolved { t!("Unresolve") } else { t!("Resolve") })
                                        .on_click(move |_, _, cx| {
                                            panel.update(cx, |p, cx| p.set_resolved(&id, !resolved, cx)).ok();
                                        }),
                                ),
                        )
                        .children(thread.comments.iter().map(|c| {
                            div()
                                .h(px(THREAD_COMMENT_HEIGHT))
                                .px(rems(0.625))
                                .flex()
                                .gap(rems(0.5))
                                .border_t_1()
                                .border_color(theme.colors.border.opacity(0.6))
                                .pt(rems(0.375))
                                .child(avatar(&theme, c.author.as_ref().map(|a| a.login.as_str()), 18.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w_0()
                                        .child(div().ui_text(&theme, TextSize::Xs, weight::SEMIBOLD).text_color(theme.colors.text).child(SharedString::from(c.author.as_ref().map_or("ghost", |a| a.login.as_str()).to_owned())))
                                        .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.text).overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(c.body.clone()))),
                                )
                        }))
                }))
                .into_any_element(),
        )
    }

    fn render_header_accessory(&self, file_key: &SharedString, _: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let theme = cx.app_theme().clone();
        let viewed = self.viewed.contains(file_key.as_ref());
        let panel = self.panel.clone();
        let key = file_key.clone();
        Some(
            div()
                .id(SharedString::from(format!("viewed-{file_key}")))
                .test_support()
                .flex()
                .items_center()
                .gap(rems(0.25))
                .h(rems(1.25))
                .px(rems(0.375))
                .rounded(rems(0.25))
                .border_1()
                .border_color(if viewed { theme.colors.border } else { theme.colors.border.opacity(0.6) })
                .when(viewed, |el| el.bg(theme.colors.accent_surface))
                .ui_text(&theme, TextSize::Xs3, weight::MEDIUM)
                .text_color(if viewed { theme.colors.text } else { theme.colors.muted_foreground })
                .child(Icon::new(if viewed { IconName::Check } else { IconName::Eye }).size(IconSize::Xs))
                .child(t!("Viewed", context = "file marked as viewed"))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    panel.update(cx, |p, cx| p.toggle_viewed(&key, cx)).ok();
                })
                .into_any_element(),
        )
    }
}

/// A review guide excerpt's rows, cut from the parsed file.
fn tour_file(file: &FileDiff) -> tour::FileDiff {
    let line = |lines: &crate::syntax::TextLines, i: usize| format!("{}\n", lines.line(i));
    tour::FileDiff {
        hunks: file
            .hunks
            .iter()
            .map(|h| {
                let (mut old, mut new) = (h.old_index as usize, h.new_index as usize);
                tour::DiffHunk {
                    deletion_start: h.old_start as usize,
                    addition_start: h.new_start as usize,
                    hunk_content: h
                        .blocks
                        .iter()
                        .map(|b| match *b {
                            HunkBlock::Context { lines } => {
                                let c = tour::HunkContent::Context { lines: lines as usize, addition_line_index: new };
                                new += lines as usize;
                                old += lines as usize;
                                c
                            }
                            HunkBlock::Change { deletions, additions } => {
                                let c = tour::HunkContent::Change { deletions: deletions as usize, additions: additions as usize, deletion_line_index: old, addition_line_index: new };
                                old += deletions as usize;
                                new += additions as usize;
                                c
                            }
                        })
                        .collect(),
                }
            })
            .collect(),
        addition_lines: (0..file.new_lines.len()).map(|i| line(&file.new_lines, i)).collect(),
        deletion_lines: (0..file.old_lines.len()).map(|i| line(&file.old_lines, i)).collect(),
    }
}

/// Conversations shown per page (`COMMENT_PAGE`).
const COMMENT_PAGE: usize = 10;

/// `ChangeBreakdownGrid` in a tooltip: lines per kind of file, then the total.
struct BreakdownTooltip {
    breakdown: PullRequestChangeBreakdownValue,
    additions: u64,
    deletions: u64,
}

impl Render for BreakdownTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let b = &self.breakdown;
        let kinds = [
            (t!("Code", context = "kind of changed file"), b.code.additions, b.code.deletions),
            (t!("Tests", context = "kind of changed file"), b.tests.additions, b.tests.deletions),
            (t!("Mocks", context = "kind of changed file"), b.mocks.additions, b.mocks.deletions),
            (t!("Generated", context = "kind of changed file"), b.generated.additions, b.generated.deletions),
            (t!("Docs", context = "kind of changed file"), b.docs.as_ref().map_or(0, |d| d.additions), b.docs.as_ref().map_or(0, |d| d.deletions)),
        ];
        let row = |label: SharedString, add: u64, del: u64| {
            div()
                .flex()
                .gap(theme.sp(4.))
                .child(div().flex_1().ui_text(theme, TextSize::Xs, weight::NORMAL).child(label))
                .child(div().w(theme.sp(12.)).flex().justify_end().text_color(theme.fixed.diff_addition_foreground).child(format!("+{}", crate::diff_count::format_diff_count(add))))
                .child(div().w(theme.sp(12.)).flex().justify_end().text_color(theme.fixed.diff_deletion_foreground).child(format!("-{}", crate::diff_count::format_diff_count(del))))
        };
        div().pb(rems(0.25)).child(
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(1.))
                .px(theme.sp(2.))
                .py(theme.sp(1.5))
                .rounded(theme.radii.md)
                .border_1()
                .border_color(c.border)
                .bg(c.popover())
                .shadow(theme.shadows.md5.clone())
                .mono_text(px(TextSize::Xs.size(theme.fonts.interface)), weight::NORMAL)
                .text_color(theme.foregrounds.popover)
                .children(kinds.into_iter().filter(|(_, a, d)| *a > 0 || *d > 0).map(|(l, a, d)| row(l, a, d)))
                .child(div().h(px(1.)).bg(c.border))
                .child(row(t!("Total", context = "sum of changed lines"), self.additions, self.deletions)),
        )
    }
}

/// The `{term, select, ...}` key for a host's change-request word.
fn term_select(provider: &str) -> &'static str {
    match crate::source_control_presentation::source_control_definition(Some(provider)).terminology.term_kind() {
        crate::source_control_presentation::ChangeRequestTermKind::Mr => "mr",
        crate::source_control_presentation::ChangeRequestTermKind::Cr => "cr",
        crate::source_control_presentation::ChangeRequestTermKind::Pr => "pr",
    }
}

fn verdict_label(verdict: PullRequestReviewVerdict) -> SharedString {
    match verdict {
        PullRequestReviewVerdict::Comment => t!("Comment", context = "review verdict"),
        PullRequestReviewVerdict::Approve => t!("Approve", context = "review verdict"),
        PullRequestReviewVerdict::RequestChanges => t!("Request changes"),
    }
}

fn verdict_icon(verdict: PullRequestReviewVerdict) -> IconName {
    match verdict {
        PullRequestReviewVerdict::Comment => IconName::MessageSquare,
        PullRequestReviewVerdict::Approve => IconName::Check,
        PullRequestReviewVerdict::RequestChanges => IconName::CircleX,
    }
}

/// `PullRequestEditButton`: a ghost pencil that shows while its group is hovered.
fn edit_button(id: &'static str, on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static) -> impl IntoElement {
    div()
        .flex_none()
        .invisible()
        .group_hover("pr-title", |s| s.visible())
        .group_hover("pr-description", |s| s.visible())
        .child(Button::new(id).size(ButtonSize::IconXs).variant(ButtonVariant::GhostMuted).icon(IconName::Pencil).icon_size(IconSize::Xs).on_click(on_click))
}

fn section(theme: &AppTheme, title: SharedString) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(rems(0.5))
        .child(div().ui_text(theme, TextSize::Xs, weight::SEMIBOLD).text_color(theme.colors.muted_foreground).child(title))
}

impl PullRequestDetailPanel {
    fn render_header(&mut self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let Some(detail) = self.detail.clone() else {
            return div().p(rems(1.)).text_color(theme.colors.muted_foreground).child(t!("Loading...")).into_any_element();
        };
        let (icon, tone, label) = pull_request_state_presentation(theme, detail.state, detail.is_draft);
        let allowed = allowed_pull_request_merge_methods(&detail);
        let can = |a| can_perform_pull_request_action(&detail, a);
        let primary = resolve_pull_request_primary_control(PrimaryControlInput {
            state: detail.state,
            is_draft: detail.is_draft,
            mergeability: detail.mergeability,
            checks_state: match t3code_client_runtime::pull_request_checks::classify_pull_request_checks(&detail.checks) {
                t3code_client_runtime::pull_request_checks::ChecksClass::None => None,
                t3code_client_runtime::pull_request_checks::ChecksClass::Passing => Some(PullRequestChecksState::Passing),
                t3code_client_runtime::pull_request_checks::ChecksClass::Failing => Some(PullRequestChecksState::Failing),
                t3code_client_runtime::pull_request_checks::ChecksClass::Pending => Some(PullRequestChecksState::Pending),
            },
            auto_merge_enabled: detail.auto_merge_enabled,
            has_merge_method: !allowed.is_empty(),
            can_merge: can(PullRequestAction::Merge),
            can_mark_ready: can(PullRequestAction::Ready),
            can_enable_auto_merge: can(PullRequestAction::EnableAutoMerge),
        });
        let method = self.current_merge_method();
        let busy = self.busy.is_some();
        let this = cx.entity();
        let mut actions = div().flex().flex_wrap().items_center().gap(rems(0.375));
        match primary {
            Some(PullRequestPrimaryControl::Merge) | Some(PullRequestPrimaryControl::EnableAutoMerge) => {
                let method_label = method.map(merge_method_label).unwrap_or_else(|| t!("Merge"));
                let this_menu = this.clone();
                let current = method;
                actions = actions
                    .child(Button::new("pr-merge").size(ButtonSize::Sm).icon(IconName::GitMerge).label(method_label).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.begin_merge(cx))))
                    .child(dropdown_menu("pr-merge-method", Button::new("pr-merge-method-trigger").size(ButtonSize::IconSm).variant(ButtonVariant::Outline).icon(IconName::ChevronDown), move |_, _| {
                        let mut menu = MenuPopup::new();
                        for m in allowed.clone() {
                            let this = this_menu.clone();
                            menu = menu.item(MenuItem::new(merge_method_label(m)).kind(MenuItemKind::Radio(current == Some(m))).on_select(move |_, cx| {
                                this.update(cx, |p, cx| {
                                    p.merge_method = Some(m);
                                    cx.notify();
                                })
                            }));
                        }
                        menu
                    }));
            }
            Some(PullRequestPrimaryControl::Ready) => {
                actions = actions.child(Button::new("pr-ready").size(ButtonSize::Sm).label(t!("Ready for review")).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.run_action(PullRequestAction::Ready, cx))));
            }
            Some(PullRequestPrimaryControl::Resolve) => {
                actions = actions.child(Button::new("pr-resolve-conflicts").size(ButtonSize::Sm).variant(ButtonVariant::WarningOutline).icon(IconName::TriangleAlert).label(t!("Resolve conflicts")));
            }
            Some(PullRequestPrimaryControl::AutoMergeArmed) => {
                actions = actions.child(Button::new("pr-auto-merge-off").size(ButtonSize::Sm).variant(ButtonVariant::Outline).label(t!("Disable auto-merge")).on_click(cx.listener(|this, _, _, cx| this.run_action(PullRequestAction::DisableAutoMerge, cx))));
            }
            _ => {}
        }
        if let Some((behind, methods)) = resolve_base_freshness(&detail)
            && !methods.is_empty()
        {
            let label = match behind {
                Some(n) => SharedString::from(format!("{} ({n})", t!("Update branch"))),
                None => t!("Update branch"),
            };
            actions = actions.child(Button::new("pr-update-branch").size(ButtonSize::Sm).variant(ButtonVariant::Outline).icon(IconName::RefreshCw).label(label).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.run_action(PullRequestAction::UpdateBranch, cx))));
        }
        // `PullRequestCheckoutMenu`: a separate worktree or this repository.
        let checkout_menu = {
            let this = this.clone();
            dropdown_menu(
                "pr-checkout",
                Button::new("pr-checkout-trigger")
                    .size(ButtonSize::Xs)
                    .variant(ButtonVariant::Outline)
                    .icon(IconName::GitBranch)
                    .icon_size(IconSize::Sm)
                    .label(t!("Check out"))
                    .trailing_icon(IconName::ChevronDown)
                    .trailing_icon_size(IconSize::Sm),
                move |_, _| {
                    let worktree = this.clone();
                    let local = this.clone();
                    MenuPopup::new()
                        .item(
                            MenuItem::new(t!("In a separate worktree"))
                                .id("pr-checkout-worktree")
                                .icon(IconName::GitBranch)
                                .icon_size(IconSize::Sm)
                                .detail(t!("Its own folder and thread. Nothing you have open moves."))
                                .on_select(move |_, cx| worktree.update(cx, |p, cx| p.checkout(ThreadEnvMode::Worktree, cx))),
                        )
                        .item(
                            MenuItem::new(t!("In this repository"))
                                .id("pr-checkout-local")
                                .icon(IconName::FolderGit2)
                                .icon_size(IconSize::Sm)
                                .detail(t!("Switches the branch you are working in."))
                                .on_select(move |_, cx| local.update(cx, |p, cx| p.checkout(ThreadEnvMode::Local, cx))),
                        )
                },
            )
        };
        actions = actions.child(checkout_menu);
        // `PullRequestActionsMenu`: the draft toggle, copy link, close or reopen, and Delete branch.
        let more_menu = {
            let this = this.clone();
            let detail = detail.clone();
            let deletable = self.can_delete_branch();
            let deleting = self.busy == Some(PullRequestAction::DeleteBranch);
            dropdown_menu(
                "pr-more",
                Button::new("pr-more-trigger").size(ButtonSize::IconXs).variant(ButtonVariant::GhostMuted).icon(IconName::Ellipsis),
                move |_, cx| {
                    let can = |a| can_perform_pull_request_action(&detail, a);
                    let mut menu = MenuPopup::new();
                    if detail.state == PullRequestState::Open && (can(PullRequestAction::Draft) || can(PullRequestAction::Ready)) {
                        let this = this.clone();
                        let action = if detail.is_draft { PullRequestAction::Ready } else { PullRequestAction::Draft };
                        menu = menu.item(
                            MenuItem::new(if detail.is_draft { t!("Ready for review") } else { t!("Convert to draft") })
                                .on_select(move |_, cx| this.update(cx, |p, cx| p.run_action(action, cx))),
                        );
                    }
                    let url = detail.url.clone();
                    menu = menu.item(MenuItem::new(t!("Copy link")).icon(IconName::Link).on_select(move |_, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url.clone()));
                    }));
                    if detail.state == PullRequestState::Open && can(PullRequestAction::Close) {
                        let this = this.clone();
                        menu = menu.separator().item(
                            MenuItem::new(t!("Close")).destructive().on_select(move |_, cx| this.update(cx, |p, cx| p.run_action(PullRequestAction::Close, cx))),
                        );
                    }
                    if detail.state == PullRequestState::Closed && can(PullRequestAction::Reopen) {
                        let this = this.clone();
                        menu = menu.separator().item(
                            MenuItem::new(t!("Reopen")).on_select(move |_, cx| this.update(cx, |p, cx| p.run_action(PullRequestAction::Reopen, cx))),
                        );
                    }
                    if deletable {
                        let this = this.clone();
                        menu = menu.separator().item(
                            MenuItem::new(if deleting { t!("Deleting branch...") } else { t!("Delete branch") })
                                .id("pr-delete-branch")
                                .icon(IconName::Trash2)
                                .icon_size(IconSize::Sm)
                                .destructive()
                                .disabled(deleting)
                                .detail(SharedString::from(detail.head_branch.clone()))
                                .on_select(move |_, cx| {
                                    this.update(cx, |p, cx| {
                                        p.confirm_delete_branch = true;
                                        cx.notify();
                                    })
                                }),
                        );
                    }
                    let _ = cx;
                    menu
                },
            )
        };
        // `PullRequestThreadLinks display="count"`: threads that link this pull request.
        if let Some(linked) = &self.linked_threads
            && !matches!(linked, Ok(t) if t.is_empty())
        {
            let count = linked.as_ref().map_or(0, |t| t.len());
            let label = if count > 0 {
                t!("{count, plural, one {Linked from {count} thread} other {Linked from {count} threads}}", count = count)
            } else {
                t!("Linked threads")
            };
            let url: SharedString = detail.url.clone().into();
            let threads = linked.clone().unwrap_or_default();
            actions = actions.child(
                div()
                    .id("pr-linked-threads")
                    .tooltip(crate::components::ui::tooltip::tooltip(crate::components::ui::tooltip::TooltipSurface::new(t!(
                        "{label}. Search in the command palette.",
                        label = label.as_ref()
                    ))))
                    .child(
                        Button::new("pr-linked-threads-button")
                            .size(ButtonSize::Xs)
                            .variant(ButtonVariant::Ghost)
                            .icon(IconName::MessageSquare)
                            .icon_size(IconSize::Sm)
                            .label(if count > 0 { SharedString::from(count.to_string()) } else { "?".into() })
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(PullRequestDetailEvent::SearchLinkedThreads { url: url.clone(), threads: threads.clone() })
                            })),
                    ),
            );
        }
        actions = actions.child(more_menu);
        let confirm = self.confirming_merge.then(|| {
            let text = SharedString::from(format!("{} #{} → {}", t!("Confirm merge?"), detail.number, detail.base_branch));
            div()
                .flex()
                .items_center()
                .gap(rems(0.5))
                .px(rems(0.625))
                .py(rems(0.5))
                .rounded(theme.radii.md)
                .border_1()
                .border_color(merged_tone(theme).opacity(0.4))
                .bg(merged_tone(theme).opacity(0.08))
                .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                .text_color(theme.colors.text)
                .child(div().flex_1().child(text))
                .child(Button::new("pr-merge-confirm").size(ButtonSize::Sm).label(t!("Merge now")).on_click(cx.listener(|this, _, _, cx| this.run_action(PullRequestAction::Merge, cx))))
                .child(Button::new("pr-merge-cancel").size(ButtonSize::Sm).variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                    this.confirming_merge = false;
                    cx.notify();
                })))
        });
        div()
            .flex()
            .flex_col()
            .gap(rems(0.625))
            .px(rems(1.25))
            .pt(rems(1.))
            .pb(rems(0.75))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(rems(0.5))
                    .child(div().pt(rems(0.1875)).child(Icon::new(icon).size(IconSize::Lg).color(tone)))
                    .child(match &self.title_edit {
                        // `PullRequestTitleEditor` while editing: the field, then Cancel and Save.
                        Some(input) => div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(theme.sp(2.))
                            .child(TextInput::new(input).size(crate::components::ui::input::InputSize::Sm).disabled(self.title_saving))
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(theme.sp(2.))
                                    .child(Button::new("pr-title-cancel").size(ButtonSize::Xs).variant(ButtonVariant::Ghost).label(t!("Cancel")).disabled(self.title_saving).on_click(cx.listener(|this, _, _, cx| {
                                        this.title_edit = None;
                                        cx.notify();
                                    })))
                                    .child(
                                        Button::new("pr-title-save")
                                            .size(ButtonSize::Xs)
                                            .variant(ButtonVariant::Outline)
                                            .label(if self.title_saving { t!("Saving...") } else { t!("Save") })
                                            .disabled(self.title_saving || input.read(cx).value().trim().is_empty())
                                            .on_click(cx.listener(|this, _, _, cx| this.save_title(cx))),
                                    ),
                            )
                            .into_any_element(),
                        None => div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_start()
                            .gap(theme.sp(1.))
                            .group("pr-title")
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .ui_text(theme, TextSize::Lg, weight::SEMIBOLD)
                                    .text_color(theme.colors.text)
                                    .child(SharedString::from(detail.title.clone()))
                                    .child(div().ui_text(theme, TextSize::Lg, weight::NORMAL).text_color(theme.colors.muted_foreground).child(format!(" #{}", detail.number))),
                            )
                            .when(t3code_client_runtime::pull_request_editing::can_edit_pull_request_change_request(&detail), |el| {
                                el.child(edit_button("pr-edit-title", cx.listener(|this, _, window, cx| this.begin_title_edit(window, cx))))
                            })
                            .into_any_element(),
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rems(0.5))
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rems(0.25))
                            .h(rems(1.25))
                            .px(rems(0.5))
                            .rounded_full()
                            .bg(tone.opacity(0.14))
                            .text_color(tone)
                            .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                            .child(Icon::new(icon).size(IconSize::Xs))
                            .child(label),
                    )
                    .child(avatar(theme, detail.author.as_ref().map(|a| a.login.as_str()), 16.))
                    .child(SharedString::from(detail.author.as_ref().map_or("ghost".into(), |a| a.login.clone())))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rems(0.25))
                            .mono_text(theme.rem(0.6875), weight::NORMAL)
                            .child(div().px(rems(0.25)).rounded(rems(0.25)).bg(theme.colors.muted).child(SharedString::from(detail.head_branch.clone())))
                            .child("→")
                            .child(div().px(rems(0.25)).rounded(rems(0.25)).bg(theme.colors.muted).child(SharedString::from(detail.base_branch.clone()))),
                    )
                    .children(diff_stat(theme, detail.additions, detail.deletions).map(|stat| {
                        // `PullRequestChangeBreakdownStat`: the split shows on hover once read.
                        let split = self.breakdown.clone().map(|b| (b, detail.additions, detail.deletions));
                        div()
                            .id("pr-diff-stat")
                            .child(stat)
                            .when_some(split, |el, (b, additions, deletions)| {
                                el.tooltip(move |_, cx| cx.new(|_| BreakdownTooltip { breakdown: b.clone(), additions, deletions }).into())
                            })
                    }))
                    .when_some(self.stack.as_ref(), |el, stack| {
                        let pos = stack.layers.iter().position(|l| l.number == detail.number).map_or(0, |p| p + 1);
                        el.child(div().flex().items_center().gap(rems(0.1875)).child(Icon::new(IconName::Layers).size(IconSize::Xs)).child(format!("{pos}/{}", stack.layers.len())))
                    }),
            )
            .child(actions)
            .children(confirm)
            .when_some(self.status.clone(), |el, s| el.child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.destructive()).child(s)))
            .into_any_element()
    }

    /// `PullRequestMetaRow`s: Reviewers with their outcomes and the picker, Labels with theirs.
    fn render_meta_rows(&self, detail: &PullRequestDetail, reviewers: &[(String, Option<PullRequestReviewOutcome>, bool)], theme: &AppTheme) -> AnyElement {
        let c = &theme.colors;
        let row = |icon: IconName, label: SharedString, value: AnyElement| {
            div()
                .flex()
                .items_center()
                .min_h(theme.sp(6.))
                .min_w_0()
                .gap(theme.sp(2.))
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .child(div().w(theme.rem(6.)).flex_none().flex().items_center().gap(theme.sp(1.5)).text_color(c.muted_foreground).child(Icon::new(icon).size(IconSize::Sm).color(c.muted_foreground)).child(label))
                .child(div().flex_1().min_w_0().text_color(c.foreground()).child(value))
        };
        let none = |context: SharedString| div().text_color(c.muted_foreground).child(context).into_any_element();
        let avatars = if reviewers.is_empty() {
            none(t!("None", context = "no reviewers"))
        } else {
            div()
                .flex()
                .items_center()
                .children(reviewers.iter().enumerate().map(|(i, (login, outcome, stale))| {
                    let ring = match outcome {
                        Some(PullRequestReviewOutcome::Approved) => Some(open_tone(theme)),
                        Some(PullRequestReviewOutcome::ChangesRequested) => Some(closed_tone(theme)),
                        _ => None,
                    };
                    div()
                        .when(i > 0, |el| el.ml(rems(-0.25)))
                        .rounded_full()
                        .when_some(ring, |el, ring| el.border_1().border_color(if *stale { ring.opacity(0.5) } else { ring }))
                        .child(avatar(theme, Some(login), 20.))
                }))
                .into_any_element()
        };
        let reviewers_value = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.sp(1.5))
            .child(avatars)
            .children(self.reviewer_picker.clone())
            .into_any_element();
        let labels_value = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.sp(1.))
            .when(detail.labels.is_empty(), |el| el.child(none(t!("None", context = "no labels"))))
            .children(detail.labels.iter().map(|l| label_chip(theme, &l.name, l.color.as_deref())))
            .children(self.label_picker.clone())
            .into_any_element();
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(2.))
            .child(row(IconName::Users, t!("Reviewers"), reviewers_value))
            .when(!detail.labels.is_empty() || detail.capabilities.labels == Some(true), |el| el.child(row(IconName::Tag, t!("Labels"), labels_value)))
            .into_any_element()
    }

    fn render_tabs(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let comments = self.activity.as_ref().map_or(0, |a| a.comment_count);
        let files = self.detail.as_ref().map_or(0, |d| d.changed_files);
        div()
            .flex()
            .items_end()
            .gap(rems(0.25))
            .px(rems(1.))
            .border_b_1()
            .border_color(theme.colors.border)
            .children(DetailTab::ALL.into_iter().map(|tab| {
                let active = tab == self.tab;
                let count = match tab {
                    DetailTab::Timeline => Some(comments),
                    DetailTab::Code => Some(files),
                    _ => None,
                };
                div()
                    .id(SharedString::from(format!("pr-tab-{}", tab.label())))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap(rems(0.375))
                    .h(rems(2.25))
                    .px(rems(0.5))
                    .border_b_2()
                    .border_color(if active { theme.colors.text } else { gpui_kit::transparent_black() })
                    .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                    .text_color(if active { theme.colors.text } else { theme.colors.muted_foreground })
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.colors.text))
                    .child(tab.label())
                    .when_some(count, |el, n| el.child(div().ui_text(theme, TextSize::Xs3, weight::MEDIUM).px(rems(0.3125)).rounded_full().bg(theme.colors.muted).child(n.to_string())))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
            }))
            .into_any_element()
    }

    fn render_summary(&mut self, theme: &AppTheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(detail) = self.detail.clone() else { return div().into_any_element() };
        let body = if detail.body.trim().is_empty() {
            self.render_markdown("body-empty", &t!("_No description provided._"), window, cx)
        } else {
            self.render_markdown("body", &detail.body, window, cx)
        };
        let outcomes = self.activity.as_ref().map(|a| latest_pull_request_review_outcomes(&a.comments, &a.commits)).unwrap_or_default();
        let checks = detail.checks.clone();
        let reviewers: Vec<(String, Option<PullRequestReviewOutcome>, bool)> = detail
            .reviewers
            .iter()
            .map(|r| {
                let o = outcomes.iter().find(|o| o.key == r.login);
                (r.login.clone(), o.map(|o| o.outcome), o.is_some_and(|o| o.stale))
            })
            .chain(outcomes.iter().filter(|o| !detail.reviewers.iter().any(|r| r.login == o.key)).map(|o| (o.key.clone(), Some(o.outcome), o.stale)))
            .collect();
        let closes: Vec<String> = linked_issue_references(&detail.body);
        let this = cx.entity();
        let reactions = self.render_reactions(None, theme, cx);
        div()
            .flex()
            .flex_col()
            .gap(rems(1.25))
            .p(rems(1.25))
            .child(self.render_meta_rows(&detail, &reviewers, theme))
            .child(match &self.body_edit {
                Some(input) => div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(2.))
                    .child(TextArea::new(input).disabled(self.body_saving))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(theme.sp(2.))
                            .child(Button::new("pr-body-cancel").size(ButtonSize::Xs).variant(ButtonVariant::Ghost).label(t!("Cancel")).disabled(self.body_saving).on_click(cx.listener(|this, _, _, cx| {
                                this.body_edit = None;
                                cx.notify();
                            })))
                            .child(
                                Button::new("pr-body-save")
                                    .size(ButtonSize::Xs)
                                    .variant(ButtonVariant::Outline)
                                    .label(if self.body_saving { t!("Saving...") } else { t!("Save") })
                                    .disabled(self.body_saving)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_body(cx))),
                            ),
                    )
                    .into_any_element(),
                None => div()
                    .flex()
                    .items_start()
                    .gap(theme.sp(1.))
                    .group("pr-description")
                    .child(div().flex_1().min_w_0().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(body))
                    .when(t3code_client_runtime::pull_request_editing::can_edit_pull_request_change_request(&detail), |el| {
                        el.child(edit_button("pr-edit-description", cx.listener(|this, _, window, cx| this.begin_body_edit(window, cx))))
                    })
                    .into_any_element(),
            })
            .child(reactions)
            .child(
                section(theme, t!("Checks", context = "CI checks section"))
                    .child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.text).child(describe_pull_request_checks(&checks)))
                    .children(checks.iter().enumerate().map(|(i, check)| {
                        let (icon, color) = check_status_icon(theme, check.status);
                        let url = check.url.clone();
                        let this = this.clone();
                        div()
                            .id(("pr-check", i))
                            .test_support()
                            .flex()
                            .items_center()
                            .gap(rems(0.5))
                            .h(rems(1.75))
                            .px(rems(0.5))
                            .rounded(theme.radii.md)
                            .hover(|s| s.bg(theme.colors.accent_surface.opacity(0.6)))
                            .child(Icon::new(icon).size(IconSize::Sm).color(color))
                            .child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.text).child(SharedString::from(check.name.clone())))
                            .when(check.required == Some(true), |el| el.child(div().ui_text(theme, TextSize::Xs3, weight::NORMAL).text_color(theme.colors.muted_foreground).child(t!("Required"))))
                            .child(div().flex_1().ui_text(theme, TextSize::Xs3, weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(check.description.clone().unwrap_or_else(|| check_status_label(check.status).to_string()))))
                            .when_some(url, |el, url| {
                                el.child(
                                    Button::new(("pr-check-logs", i)).size(ButtonSize::Xs).variant(ButtonVariant::Ghost).icon(IconName::ExternalLink).label(t!("Details")).on_click(move |_, _, cx| {
                                        this.update(cx, |_, cx| cx.emit(PullRequestDetailEvent::OpenUrl(url.clone().into())));
                                    }),
                                )
                            })
                    })),
            )
            .when(!closes.is_empty(), |el| {
                el.child(section(theme, t!("Linked issues")).children(closes.into_iter().map(|r| {
                    div().flex().items_center().gap(rems(0.375)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.text).child(Icon::new(IconName::CircleDot).size(IconSize::Xs).color(open_tone(theme))).child(SharedString::from(r))
                })))
            })
            .into_any_element()
    }

    fn render_reactions(&self, subject: Subject, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let Some(detail) = &self.detail else { return div().into_any_element() };
        let kind = match &subject {
            None => PullRequestCommentKind::IssueComment,
            Some(id) => self.activity.as_ref().and_then(|a| a.comments.iter().find(|c| &c.id == id)).map_or(PullRequestCommentKind::IssueComment, |c| c.kind),
        };
        if !can_react(detail.capabilities.reactions, &detail.provider, kind) {
            return div().into_any_element();
        }
        let reactions = self.reactions_of(&subject);
        let key = subject.clone().unwrap_or_else(|| "pr".into());
        let picker = self.picker_open == Some(subject.clone());
        let mut row = div().flex().flex_wrap().items_center().gap(rems(0.25));
        for reaction in reactions {
            let s = subject.clone();
            let content = reaction.content;
            row = row.child(
                div()
                    .id(SharedString::from(format!("react-{key}-{}", content.as_str())))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap(rems(0.25))
                    .h(rems(1.375))
                    .px(rems(0.4375))
                    .rounded_full()
                    .border_1()
                    .border_color(if reaction.viewer_has_reacted { theme.colors.primary().opacity(0.5) } else { theme.colors.border })
                    .when(reaction.viewer_has_reacted, |el| el.bg(theme.colors.primary().opacity(0.12)))
                    .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                    .text_color(theme.colors.text)
                    .cursor_pointer()
                    .child(pull_request_reaction_emoji(content))
                    .child(reaction.count.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_reaction(s.clone(), content, cx))),
            );
        }
        let s = subject.clone();
        row = row.child(
            div()
                .id(SharedString::from(format!("react-add-{key}")))
                .test_support()
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded_full()
                .border_1()
                .border_color(theme.colors.border)
                .cursor_pointer()
                .hover(|st| st.bg(theme.colors.accent_surface))
                .child(Icon::new(IconName::SmilePlus).size(IconSize::Xs).color(theme.colors.muted_foreground))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picker_open = if this.picker_open == Some(s.clone()) { None } else { Some(s.clone()) };
                    cx.notify();
                })),
        );
        if picker {
            row = row.child(div().flex().gap(px(2.)).p(px(2.)).rounded_full().border_1().border_color(theme.colors.border).bg(theme.colors.popover()).children(PULL_REQUEST_REACTION_ORDER.into_iter().map(|content| {
                let s = subject.clone();
                div()
                    .id(SharedString::from(format!("react-pick-{key}-{}", content.as_str())))
                    .test_support()
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|st| st.bg(theme.colors.accent_surface))
                    .child(pull_request_reaction_emoji(content))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_reaction(s.clone(), content, cx)))
            })));
        }
        row.into_any_element()
    }

    fn render_comment(&mut self, comment: &PullRequestComment, thread: Option<&TimelineThread>, theme: &AppTheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(detail) = self.detail.clone() else { return div().into_any_element() };
        let now = self.now();
        let login = comment.author.as_ref().map_or("ghost".to_string(), |a| a.login.clone());
        let verb = match (comment.kind, pull_request_review_outcome(comment.review_state.as_deref())) {
            (_, Some(PullRequestReviewOutcome::Approved)) => SharedString::new_static("approved these changes"),
            (_, Some(PullRequestReviewOutcome::ChangesRequested)) => SharedString::new_static("requested changes"),
            (PullRequestCommentKind::Review, _) => SharedString::new_static("reviewed"),
            _ => SharedString::new_static("commented"),
        };
        let editing = self.edit.as_ref().filter(|(id, ..)| *id == comment.id).map(|(_, _, i)| i.clone());
        let body = match (&editing, visible_body(&comment.body)) {
            (Some(input), _) => div()
                .flex()
                .flex_col()
                .gap(rems(0.375))
                .child(TextInput::new(input))
                .child(
                    div()
                        .flex()
                        .gap(rems(0.375))
                        .child(Button::new(SharedString::from(format!("edit-save-{}", comment.id))).size(ButtonSize::Xs).label(t!("Save")).on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))))
                        .child(Button::new(SharedString::from(format!("edit-cancel-{}", comment.id))).size(ButtonSize::Xs).variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                            this.edit = None;
                            cx.notify();
                        }))),
                )
                .into_any_element(),
            (None, Some(text)) => self.render_markdown(&comment.id, &text, window, cx),
            (None, None) => div().into_any_element(),
        };
        let can_edit = can_edit_pull_request_comment(&detail, comment.kind, comment.author.as_ref().map(|a| a.login.as_str()), comment.can_edit);
        let reactions = self.render_reactions(Some(comment.id.clone()), theme, cx);
        let edit_target = comment.clone();
        div()
            .id(SharedString::from(format!("comment-{}", comment.id)))
            .test_support()
            .flex()
            .gap(rems(0.625))
            .child(avatar(theme, Some(&login), 24.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(rems(0.25))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rems(0.375))
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(theme.colors.muted_foreground)
                            .child(div().text_color(theme.colors.text).font_weight(weight::SEMIBOLD).child(SharedString::from(login.clone())))
                            .child(verb)
                            .child(relative_label(&comment.created_at, now))
                            .when(comment.edited_at.is_some(), |el| el.child("edited"))
                            .when_some(comment.path.clone().filter(|_| thread.is_none()), |el, p| el.child(div().mono_text(theme.rem(0.6875), weight::NORMAL).child(SharedString::from(p))))
                            .when(can_edit && editing.is_none(), |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("edit-{}", comment.id)))
                                        .test_support()
                                        .ml_auto()
                                        .cursor_pointer()
                                        .child(Icon::new(IconName::Pencil).size(IconSize::Xs).color(theme.colors.muted_foreground))
                                        .on_click(cx.listener(move |this, _, window, cx| this.begin_edit(&edit_target, window, cx))),
                                )
                            }),
                    )
                    .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(body))
                    .child(reactions),
            )
            .into_any_element()
    }

    fn render_timeline(&mut self, theme: &AppTheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(detail), Some(activity)) = (self.detail.clone(), self.activity.clone()) else {
            return div().p(rems(1.25)).text_color(theme.colors.muted_foreground).child(t!("Loading timeline")).into_any_element();
        };
        let threads: Vec<ConversationThread> = activity.review_threads.iter().map(Into::into).collect();
        let author = detail.author.as_ref().map(|a| PullRequestCommentAuthor { is_bot: a.is_bot, login: a.login.clone(), name: a.name.clone(), avatar_url: a.avatar_url.clone() });
        let events = build_pull_request_timeline(TimelineSource {
            created_at: &detail.created_at,
            author,
            commits: &activity.commits,
            comments: &activity.comments,
            threads: &threads,
            merged_at: detail.merged_at.as_deref(),
            closed_at: detail.closed_at.as_deref(),
        });
        let visible: HashSet<String> = {
            let items = pull_request_conversation_items(&activity.comments, &threads);
            filter_pull_request_conversation_items(&items, self.show_bots, self.show_resolved).iter().flat_map(|i| i.comments().into_iter().map(|c| c.id.clone())).collect()
        };
        let now = self.now();
        // `pagePullRequestConversationItems`: the oldest conversations past the page stay hidden.
        let conversations = events
            .iter()
            .filter(|e| matches!(e.kind, TimelineEventKind::Comment | TimelineEventKind::Review) && e.comment.as_ref().is_some_and(|c| visible.contains(&c.id)))
            .count();
        let hidden = conversations.saturating_sub(self.shown_conversations);
        let mut skipped = 0;
        let mut column = div().flex().flex_col().gap(rems(1.)).p(rems(1.25));
        column = column.child(
            div()
                .flex()
                .gap(rems(0.375))
                .child(Button::new("tl-bots").size(ButtonSize::Xs).variant(if self.show_bots { ButtonVariant::Secondary } else { ButtonVariant::Outline }).label(t!("Bot comments")).on_click(cx.listener(|this, _, _, cx| {
                    this.show_bots = !this.show_bots;
                    cx.notify();
                })))
                .child(Button::new("tl-resolved").size(ButtonSize::Xs).variant(if self.show_resolved { ButtonVariant::Secondary } else { ButtonVariant::Outline }).label(t!("Resolved or dismissed")).on_click(cx.listener(|this, _, _, cx| {
                    this.show_resolved = !this.show_resolved;
                    cx.notify();
                }))),
        );
        for event in events {
            match event.kind {
                TimelineEventKind::Comment | TimelineEventKind::Review => {
                    let Some(comment) = event.comment.clone() else { continue };
                    if !visible.contains(&comment.id) {
                        continue;
                    }
                    if skipped < hidden {
                        skipped += 1;
                        if skipped == hidden {
                            column = column.child(
                                Button::new("tl-show-older")
                                    .size(ButtonSize::Sm)
                                    .variant(ButtonVariant::Outline)
                                    .label(t!(
                                        "{count, plural, one {Show {shown} older conversation ({count} hidden)} other {Show {shown} older conversations ({count} hidden)}}",
                                        count = hidden,
                                        shown = hidden.min(COMMENT_PAGE)
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.shown_conversations += COMMENT_PAGE;
                                        cx.notify();
                                    })),
                            );
                        }
                        continue;
                    }
                    let mut card = div().flex().flex_col().gap(rems(0.625)).child(self.render_comment(&comment, event.thread.as_ref(), theme, window, cx));
                    if let Some(thread) = &event.thread {
                        let t = &thread.thread;
                        let id = t.id.clone();
                        let resolved = t.is_resolved;
                        let can_resolve = can_resolve_pull_request_thread(&detail, &PullRequestReviewThread {
                            id: t.id.clone(),
                            path: t.path.clone().unwrap_or_default(),
                            line: t.line,
                            side: t.side,
                            is_resolved: t.is_resolved,
                            is_outdated: t.is_outdated,
                            comments: vec![],
                            comment_count: t.comment_count,
                            next_comments_cursor: None,
                            can_resolve: t.can_resolve,
                        });
                        let header = div()
                            .flex()
                            .items_center()
                            .gap(rems(0.375))
                            .mono_text(theme.rem(0.6875), weight::NORMAL)
                            .text_color(theme.colors.muted_foreground)
                            .child(Icon::new(IconName::FileCodeCorner).size(IconSize::Xs))
                            .child(SharedString::from(format!("{}{}", t.path.clone().unwrap_or_default(), t.line.map(|l| format!(":{l}")).unwrap_or_default())))
                            .when(resolved, |el| el.child(div().ui_text(theme, TextSize::Xs3, weight::MEDIUM).px(rems(0.375)).rounded_full().bg(theme.colors.muted).child(t!("Resolved", context = "review thread state"))));
                        let mut replies = div().flex().flex_col().gap(rems(0.625)).pl(rems(2.125));
                        for reply in &thread.replies {
                            if let Some(c) = &reply.comment {
                                replies = replies.child(self.render_comment(c, Some(thread), theme, window, cx));
                            }
                        }
                        let reply_box = match self.reply.as_ref().filter(|(tid, _)| *tid == id) {
                            Some((_, input)) => div()
                                .flex()
                                .gap(rems(0.375))
                                .child(div().flex_1().child(TextInput::new(input)))
                                .child(Button::new(SharedString::from(format!("reply-send-{id}"))).size(ButtonSize::Sm).label(t!("Comment", context = "verb, submit button")).on_click(cx.listener(|this, _, _, cx| this.send_reply(cx))))
                                .into_any_element(),
                            None => {
                                let rid = id.clone();
                                let sid = id.clone();
                                div()
                                    .flex()
                                    .gap(rems(0.375))
                                    .child(Button::new(SharedString::from(format!("reply-{id}"))).size(ButtonSize::Xs).variant(ButtonVariant::Outline).icon(IconName::Reply).label(t!("Reply to this conversation")).on_click(cx.listener(move |this, _, window, cx| this.begin_reply(&rid, window, cx))))
                                    .when(can_resolve, |el| {
                                        el.child(
                                            Button::new(SharedString::from(format!("resolve-{id}")))
                                                .size(ButtonSize::Xs)
                                                .variant(ButtonVariant::Ghost)
                                                .label(if resolved { t!("Unresolve") } else { t!("Resolve") })
                                                .on_click(cx.listener(move |this, _, _, cx| this.set_resolved(&sid, !resolved, cx))),
                                        )
                                    })
                                    .into_any_element()
                            }
                        };
                        card = div()
                            .flex()
                            .flex_col()
                            .gap(rems(0.625))
                            .p(rems(0.75))
                            .rounded(theme.radii.lg)
                            .border_1()
                            .border_color(theme.colors.border)
                            .child(header)
                            .child(card)
                            .child(replies)
                            .when(thread.has_more_replies, |el| {
                                // `PullRequestMoreReplies`: how many replies the host kept back, and where to read them.
                                let url = thread.url.clone();
                                el.child(
                                    div()
                                        .pl(rems(2.125))
                                        .flex()
                                        .flex_wrap()
                                        .items_baseline()
                                        .gap(theme.sp(2.))
                                        .py(theme.sp(1.))
                                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                        .text_color(theme.colors.muted_foreground)
                                        .child(if thread.more_replies > 0 {
                                            t!(
                                                "{count, plural, one {{formatted} more reply} other {{formatted} more replies}}",
                                                count = thread.more_replies,
                                                formatted = crate::i18n::number_format::format_count(thread.more_replies as f64, None).as_str()
                                            )
                                        } else {
                                            t!("More replies")
                                        })
                                        .when_some(url, |el, url| {
                                            el.child(
                                                div()
                                                    .id(SharedString::from(format!("more-replies-{id}")))
                                                    .cursor_pointer()
                                                    .hover(|s| s.underline().text_color(theme.colors.foreground()))
                                                    .child(t!("View on host"))
                                                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(PullRequestDetailEvent::OpenUrl(url.clone().into())))),
                                            )
                                        }),
                                )
                            })
                            .child(div().pl(rems(2.125)).child(reply_box));
                    }
                    column = column.child(card);
                }
                kind => {
                    let (icon, text): (IconName, SharedString) = match kind {
                        TimelineEventKind::Opened => (IconName::GitPullRequestArrow, t!("opened this {term, select, mr {merge request} cr {change request} other {pull request}}", term = "pr")),
                        TimelineEventKind::Commit => {
                            let sha = event.commit.as_ref().map_or(String::new(), |c| c.oid.chars().take(7).collect());
                            (IconName::GitCommitHorizontal, t!("Commit {sha}", sha = sha.as_str()))
                        }
                        TimelineEventKind::Merged => (IconName::GitMerge, t!("{term, select, mr {Merge request} cr {Change request} other {Pull request}} merged", term = "pr")),
                        _ => (IconName::GitPullRequestClosed, t!("{term, select, mr {Merge request} cr {Change request} other {Pull request}} closed", term = "pr")),
                    };
                    column = column.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rems(0.5))
                            .pl(rems(0.25))
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(theme.colors.muted_foreground)
                            .child(Icon::new(icon).size(IconSize::Sm))
                            .when_some(event.actor.as_ref(), |el, a| el.child(div().text_color(theme.colors.text).child(SharedString::from(a.login.clone()))))
                            .child(text)
                            .when_some(event.body.clone(), |el, b| el.child(div().text_color(theme.colors.text).overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(b))))
                            .child(div().ml_auto().child(relative_label(&event.at, now))),
                    );
                }
            }
        }
        if self.shown_conversations > COMMENT_PAGE {
            column = column.child(
                Button::new("tl-show-recent")
                    .size(ButtonSize::Sm)
                    .variant(ButtonVariant::Ghost)
                    .label(SharedString::from(t_rich!("Show only {COMMENT_PAGE} recent conversations", COMMENT_PAGE = COMMENT_PAGE).plain()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.shown_conversations = COMMENT_PAGE;
                        cx.notify();
                    })),
            );
        }
        column.into_any_element()
    }

    fn render_code(&mut self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let Some(code) = &self.code else { return div().into_any_element() };
        let total = code.files.len();
        let viewed = code.files.iter().filter(|f| self.is_viewed(&f.path)).count();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.5))
                    .h(rems(2.25))
                    .px(rems(1.))
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .child(SharedString::from(format!("{viewed}/{total} {}", t!("viewed", context = "files marked as viewed"))))
                    .child(div().flex_1())
                    .child(Button::new("code-expand-all").size(ButtonSize::Xs).variant(ButtonVariant::Ghost).label(t!("Expand all files")).on_click(cx.listener(|this, _, _, cx| this.fold_all(false, cx))))
                    .child(Button::new("code-collapse-all").size(ButtonSize::Xs).variant(ButtonVariant::Ghost).label(t!("Collapse all files")).on_click(cx.listener(|this, _, _, cx| this.fold_all(true, cx)))),
            )
            .child(div().flex_1().min_h_0().child(code.view.clone()))
            .into_any_element()
    }

    fn render_guide(&mut self, theme: &AppTheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let guide = self.guide.as_ref().and_then(|g| g.guide.clone());
        let Some(guide) = guide else {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .gap(rems(0.625))
                .py(rems(3.))
                .text_color(theme.colors.muted_foreground)
                .child(Icon::new(IconName::BookOpen).size(IconSize::Xl))
                .child(t!("No review guide yet"))
                .child(Button::new("guide-generate").size(ButtonSize::Sm).icon(IconName::BookOpen).label(t!("Write guide")).on_click(cx.listener(|this, _, _, cx| this.generate_guide(cx))))
                .into_any_element();
        };
        let position = self.tour_position.min(guide.stops.len().saturating_sub(1));
        let Some(stop) = guide.stops.get(position).cloned() else { return div().into_any_element() };
        let letters = tour::letter_stop_notes(&stop);
        let files: Vec<String> = tour::stop_file_paths(&stop).into_iter().map(str::to_owned).collect();
        let parsed: Vec<std::sync::Arc<FileDiff>> = self.code.as_ref().map(|c| c.files.clone()).unwrap_or_default();
        let mut excerpts = div().flex().flex_col().gap(rems(0.75));
        for (i, excerpt) in stop.excerpts.iter().enumerate() {
            let file = parsed.iter().find(|f| f.path.as_ref() == excerpt.path);
            let rows = file.map(|f| tour::file_diff_rows(&tour_file(f))).unwrap_or_default();
            let cut = tour::excerpt_rows(&rows, excerpt).to_vec();
            let markers = letters.by_excerpt.get(i).cloned().unwrap_or_default();
            excerpts = excerpts.child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(theme.radii.md)
                    .border_1()
                    .border_color(theme.colors.border)
                    .overflow_hidden()
                    .child(div().px(theme.rem(0.625)).py(theme.rem(0.375)).bg(theme.colors.muted).mono_text(theme.rem(0.6875), weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(format!("{} · {}", excerpt.path, excerpt.caption))))
                    .when(cut.is_empty(), |el| el.child(div().p(rems(0.625)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(t!("This file is not in the loaded diff."))))
                    .children(cut.into_iter().map(|row| {
                        let (bg, sign) = match row.kind {
                            tour::DiffRowKind::Addition => (theme.fixed.diff_addition_foreground.opacity(0.12), "+"),
                            tour::DiffRowKind::Deletion => (theme.fixed.diff_deletion_foreground.opacity(0.12), "-"),
                            tour::DiffRowKind::Context => (gpui_kit::transparent_black(), " "),
                        };
                        let line = row.new_line.or(row.old_line).unwrap_or(0);
                        let marks: Vec<String> = markers.iter().filter(|(l, _)| row.new_line.map(|n| n as u64) == Some(*l)).flat_map(|(_, m)| m.iter().map(|(mk, _)| mk.clone())).collect();
                        div()
                            .flex()
                            .bg(bg)
                            .mono_text(theme.rem(0.75), weight::NORMAL)
                            .text_color(theme.colors.text)
                            .child(div().w(rems(2.5)).pr(rems(0.375)).flex().justify_end().text_color(theme.colors.muted_foreground).child(line.to_string()))
                            .child(div().w(rems(0.875)).child(sign))
                            .child(div().flex_1().whitespace_nowrap().overflow_hidden().child(SharedString::from(row.text.trim_end_matches('\n').to_owned())))
                            .children((!marks.is_empty()).then(|| div().px(rems(0.375)).mr(rems(0.375)).rounded_full().bg(theme.colors.primary().opacity(0.2)).ui_text(theme, TextSize::Xs3, weight::SEMIBOLD).child(marks.join(" "))))
                    })),
            );
        }
        let summary = self.render_markdown(&format!("stop{position}"), &stop.summary, window, cx);
        div()
            .flex()
            .flex_col()
            .gap(rems(1.))
            .p(rems(1.25))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.5))
                    .child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.muted_foreground).child(t!("Review tour")))
                    .child(div().flex_1())
                    .child(Button::new("guide-prev").size(ButtonSize::Xs).variant(ButtonVariant::Outline).label(t!("Previous stop")).disabled(position == 0).on_click(cx.listener(|this, _, _, cx| this.step_tour(false, cx))))
                    .child(Button::new("guide-next").size(ButtonSize::Xs).variant(ButtonVariant::Outline).label(t!("Next stop")).disabled(position + 1 >= guide.stops.len()).on_click(cx.listener(|this, _, _, cx| this.step_tour(true, cx)))),
            )
            .child(
                div()
                    .flex()
                    .gap(rems(0.375))
                    .children(guide.stops.iter().enumerate().map(|(i, s)| {
                        div()
                            .id(("guide-stop", i))
                            .test_support()
                            .flex_1()
                            .h(rems(0.25))
                            .rounded_full()
                            .bg(if i == position { theme.colors.text } else { theme.colors.muted })
                            .cursor_pointer()
                            .tooltip(crate::components::ui::tooltip::tooltip(SharedString::from(s.title.clone())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tour_position = i;
                                cx.notify();
                            }))
                    })),
            )
            .child(div().ui_text(theme, TextSize::Base, weight::SEMIBOLD).text_color(theme.colors.text).child(SharedString::from(stop.title.clone())))
            .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(stop_subline(files.len())))
            .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(summary))
            .when_some(stop.attention.clone(), |el, a| {
                el.child(
                    div()
                        .flex()
                        .gap(rems(0.5))
                        .p(rems(0.625))
                        .rounded(theme.radii.md)
                        .bg(theme.colors.warning_surface)
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(theme.colors.text)
                        .child(Icon::new(IconName::TriangleAlert).size(IconSize::Sm).color(theme.colors.warning))
                        .child(SharedString::from(a.note)),
                )
            })
            .child(excerpts)
            .when(!letters.lettered.is_empty(), |el| {
                el.child(section(theme, t!("Notes on this stop")).children(letters.lettered.iter().map(|(m, note)| {
                    div()
                        .flex()
                        .gap(rems(0.5))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(theme.colors.text)
                        .child(div().flex_none().size(px(18.)).rounded_full().flex().items_center().justify_center().bg(theme.colors.primary().opacity(0.2)).font_weight(weight::SEMIBOLD).child(m.clone()))
                        .child(div().flex().flex_col().child(div().font_weight(weight::SEMIBOLD).child(SharedString::from(note.title.clone()))).child(div().text_color(theme.colors.muted_foreground).child(SharedString::from(note.body.clone()))))
                })))
            })
            .when(!stop.questions.is_empty(), |el| {
                el.child(section(theme, t!("Questions to consider")).children(stop.questions.iter().map(|q| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.text).child(SharedString::from(format!("• {q}"))))))
            })
            .into_any_element()
    }
}

/// `#123` references the description says it closes or fixes.
fn linked_issue_references(body: &str) -> Vec<String> {
    static CLOSES: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+(#\d+)").unwrap());
    let mut out: Vec<String> = CLOSES.captures_iter(body).map(|c| c[1].to_owned()).collect();
    out.dedup();
    out
}

impl Render for PullRequestDetailPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        if let Some(code) = self.code.as_mut()
            && self.tab == DetailTab::Code
            && !code.files.is_empty()
            && let Some(start) = code.opened_at.take()
        {
            let this = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let elapsed = start.elapsed().as_secs_f64() * 1000.;
                this.update(cx, |p, _| p.code_first_paint_ms = Some(elapsed)).ok();
            });
        }
        self.ensure_pickers(window, cx);
        let header = self.render_header(&theme, cx);
        let tabs = self.render_tabs(&theme, cx);
        let body = match self.tab {
            DetailTab::Summary => self.render_summary(&theme, window, cx),
            DetailTab::Guide => self.render_guide(&theme, window, cx),
            DetailTab::Timeline => self.render_timeline(&theme, window, cx),
            DetailTab::Code => self.render_code(&theme, cx),
        };
        let scrolls = self.tab != DetailTab::Code;
        let composer = self.render_composer(&theme, cx);
        let delete_dialog = self.render_delete_branch_dialog(cx);
        div()
            .id("pr-detail")
            .test_support()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.background())
            .child(header)
            .child(tabs)
            .child(if scrolls {
                div().id("pr-detail-body").flex_1().min_h_0().overflow_y_scroll().child(body).into_any_element()
            } else {
                div().flex().flex_col().flex_1().min_h_0().child(body).into_any_element()
            })
            .children(composer)
            .children(delete_dialog)
    }
}

/// Files the Code tab shows, for scripts.
pub fn code_file_paths(panel: &PullRequestDetailPanel) -> Vec<String> {
    panel.code.as_ref().map(|c| c.files.iter().map(|f| f.path.to_string()).collect()).unwrap_or_default()
}

/// Calls made through the panel's source are recorded by the fixture; this lists the panel's
/// tabs in order for scripts.
pub fn tab_order() -> BTreeMap<usize, DetailTab> {
    DetailTab::ALL.into_iter().enumerate().collect()
}
