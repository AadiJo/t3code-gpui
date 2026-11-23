//! One pull request: its header with state, branches and actions, and the
//! Summary, Guide, Timeline and Code tabs. Mounts on the pull requests page's
//! right side or as a right-panel tab. Ports `PullRequestDetailPanel.tsx`,
//! `PullRequestDetailActions.tsx`, `PullRequestSummaryTab.tsx`,
//! `PullRequestGuideTab.tsx`, `PullRequestTimelineTab.tsx` and
//! `PullRequestCodeTab.tsx`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::component::input::InputState;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
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
use super::pull_request_source::PullRequestDetailSource;
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
use crate::diff_patch::{FileDiff, HunkBlock, parse_patch};
use crate::i18n::t;
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
    /// Fixed clock for scenes; `None` reads the system clock.
    pub now_ms: Option<i64>,
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

    /// Reads the detail, activity, stack, files viewed and guide again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
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

    fn load_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let started = Instant::now();
        let view = cx.new(|cx| CodeView::new(ViewMode::Diff, window, cx));
        let events = cx.subscribe(&view, |this, _, event: &CodeViewEvent, cx| {
            if let CodeViewEvent::CollapsedChanged { key, collapsed } = event {
                this.folds = this.folds.with_fold(false, key, Some(*collapsed));
            }
            let _ = cx;
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
        let k = hash_key(&format!("{key}\0{text}"));
        let pieces = self.markdown.entry(k).or_insert_with(|| parse_message_blocks(&expand_github_emoji_aliases(text), MarkdownFlavor::ASSISTANT, false)).clone();
        (k, pieces)
    }

    fn markdown_actions(&self, cx: &mut Context<Self>) -> MarkdownActions {
        let this = cx.entity().downgrade();
        Rc::new(move |action, _, cx| {
            this.update(cx, |this, cx| {
                match action {
                    MarkdownAction::OpenLink(url) => cx.emit(PullRequestDetailEvent::OpenUrl(url)),
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
                        .mx(px(12.))
                        .my(px(4.))
                        .rounded(theme.radii.md)
                        .border_1()
                        .border_color(theme.colors.border)
                        .bg(theme.colors.card())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(24.))
                                .px(px(10.))
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
                                .px(px(10.))
                                .flex()
                                .gap(px(8.))
                                .border_t_1()
                                .border_color(theme.colors.border.opacity(0.6))
                                .pt(px(6.))
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
                .gap(px(4.))
                .h(px(20.))
                .px(px(6.))
                .rounded(px(4.))
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

fn section(theme: &AppTheme, title: SharedString) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(div().ui_text(theme, TextSize::Xs, weight::SEMIBOLD).text_color(theme.colors.muted_foreground).child(title))
}

impl PullRequestDetailPanel {
    fn render_header(&mut self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let Some(detail) = self.detail.clone() else {
            return div().p(px(16.)).text_color(theme.colors.muted_foreground).child(t!("Loading...")).into_any_element();
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
        let mut actions = div().flex().flex_wrap().items_center().gap(px(6.));
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
        if detail.state == PullRequestState::Open && can(PullRequestAction::Close) {
            actions = actions.child(Button::new("pr-close").size(ButtonSize::Sm).variant(ButtonVariant::Outline).label(t!("Close")).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.run_action(PullRequestAction::Close, cx))));
        }
        if detail.state == PullRequestState::Closed && can(PullRequestAction::Reopen) {
            actions = actions.child(Button::new("pr-reopen").size(ButtonSize::Sm).variant(ButtonVariant::Outline).label(t!("Reopen")).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.run_action(PullRequestAction::Reopen, cx))));
        }
        actions = actions.child(Button::new("pr-checkout").size(ButtonSize::Sm).variant(ButtonVariant::Ghost).icon(IconName::GitBranch).label(t!("Check out")).on_click(cx.listener(|this, _, _, cx| this.checkout_into_thread(cx))));
        let confirm = self.confirming_merge.then(|| {
            let text = SharedString::from(format!("{} #{} → {}", t!("Confirm merge?"), detail.number, detail.base_branch));
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(8.))
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
            .gap(px(10.))
            .px(px(20.))
            .pt(px(16.))
            .pb(px(12.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(div().pt(px(3.)).child(Icon::new(icon).size(IconSize::Lg).color(tone)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .ui_text(theme, TextSize::Lg, weight::SEMIBOLD)
                            .text_color(theme.colors.text)
                            .child(SharedString::from(detail.title.clone()))
                            .child(div().ui_text(theme, TextSize::Lg, weight::NORMAL).text_color(theme.colors.muted_foreground).child(format!(" #{}", detail.number))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .h(px(20.))
                            .px(px(8.))
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
                            .gap(px(4.))
                            .mono_text(px(11.), weight::NORMAL)
                            .child(div().px(px(4.)).rounded(px(4.)).bg(theme.colors.muted).child(SharedString::from(detail.head_branch.clone())))
                            .child("→")
                            .child(div().px(px(4.)).rounded(px(4.)).bg(theme.colors.muted).child(SharedString::from(detail.base_branch.clone()))),
                    )
                    .children(diff_stat(theme, detail.additions, detail.deletions))
                    .when_some(self.stack.as_ref(), |el, stack| {
                        let pos = stack.layers.iter().position(|l| l.number == detail.number).map_or(0, |p| p + 1);
                        el.child(div().flex().items_center().gap(px(3.)).child(Icon::new(IconName::Layers).size(IconSize::Xs)).child(format!("{pos}/{}", stack.layers.len())))
                    }),
            )
            .child(actions)
            .children(confirm)
            .when_some(self.status.clone(), |el, s| el.child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.destructive()).child(s)))
            .into_any_element()
    }

    fn render_tabs(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let comments = self.activity.as_ref().map_or(0, |a| a.comment_count);
        let files = self.detail.as_ref().map_or(0, |d| d.changed_files);
        div()
            .flex()
            .items_end()
            .gap(px(4.))
            .px(px(16.))
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
                    .gap(px(6.))
                    .h(px(36.))
                    .px(px(8.))
                    .border_b_2()
                    .border_color(if active { theme.colors.text } else { gpui_kit::transparent_black() })
                    .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                    .text_color(if active { theme.colors.text } else { theme.colors.muted_foreground })
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.colors.text))
                    .child(tab.label())
                    .when_some(count, |el, n| el.child(div().ui_text(theme, TextSize::Xs3, weight::MEDIUM).px(px(5.)).rounded_full().bg(theme.colors.muted).child(n.to_string())))
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
            .gap(px(20.))
            .p(px(20.))
            .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(body))
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
                            .gap(px(8.))
                            .h(px(28.))
                            .px(px(8.))
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
            .child(
                section(theme, t!("Reviewers")).children(reviewers.into_iter().map(|(login, outcome, stale)| {
                    let (icon, color, text) = match outcome {
                        Some(PullRequestReviewOutcome::Approved) => (IconName::Check, open_tone(theme), t!("Approved", context = "review decision")),
                        Some(PullRequestReviewOutcome::ChangesRequested) => (IconName::MessageSquareWarning, closed_tone(theme), t!("Changes requested")),
                        _ => (IconName::CircleDashed, pending_tone(theme), t!("Review required")),
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(theme.colors.text)
                        .child(avatar(theme, Some(&login), 18.))
                        .child(SharedString::from(login))
                        .child(div().ml_auto().flex().items_center().gap(px(4.)).text_color(color).child(Icon::new(icon).size(IconSize::Xs)).child(text))
                        .when(stale, |el| el.opacity(0.6))
                })),
            )
            .child(section(theme, t!("Labels")).child(div().flex().flex_wrap().gap(px(6.)).children(detail.labels.iter().map(|l| label_chip(theme, &l.name, l.color.as_deref())))))
            .when(!closes.is_empty(), |el| {
                el.child(section(theme, t!("Linked issues")).children(closes.into_iter().map(|r| {
                    div().flex().items_center().gap(px(6.)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.text).child(Icon::new(IconName::CircleDot).size(IconSize::Xs).color(open_tone(theme))).child(SharedString::from(r))
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
        let mut row = div().flex().flex_wrap().items_center().gap(px(4.));
        for reaction in reactions {
            let s = subject.clone();
            let content = reaction.content;
            row = row.child(
                div()
                    .id(SharedString::from(format!("react-{key}-{}", content.as_str())))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(22.))
                    .px(px(7.))
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
                .gap(px(6.))
                .child(TextInput::new(input))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
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
            .gap(px(10.))
            .child(avatar(theme, Some(&login), 24.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(theme.colors.muted_foreground)
                            .child(div().text_color(theme.colors.text).font_weight(weight::SEMIBOLD).child(SharedString::from(login.clone())))
                            .child(verb)
                            .child(relative_label(&comment.created_at, now))
                            .when(comment.edited_at.is_some(), |el| el.child("edited"))
                            .when_some(comment.path.clone().filter(|_| thread.is_none()), |el, p| el.child(div().mono_text(px(11.), weight::NORMAL).child(SharedString::from(p))))
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
            return div().p(px(20.)).text_color(theme.colors.muted_foreground).child(t!("Loading timeline")).into_any_element();
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
        let mut column = div().flex().flex_col().gap(px(16.)).p(px(20.));
        column = column.child(
            div()
                .flex()
                .gap(px(6.))
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
                    let mut card = div().flex().flex_col().gap(px(10.)).child(self.render_comment(&comment, event.thread.as_ref(), theme, window, cx));
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
                            .gap(px(6.))
                            .mono_text(px(11.), weight::NORMAL)
                            .text_color(theme.colors.muted_foreground)
                            .child(Icon::new(IconName::FileCodeCorner).size(IconSize::Xs))
                            .child(SharedString::from(format!("{}{}", t.path.clone().unwrap_or_default(), t.line.map(|l| format!(":{l}")).unwrap_or_default())))
                            .when(resolved, |el| el.child(div().ui_text(theme, TextSize::Xs3, weight::MEDIUM).px(px(6.)).rounded_full().bg(theme.colors.muted).child(t!("Resolved", context = "review thread state"))));
                        let mut replies = div().flex().flex_col().gap(px(10.)).pl(px(34.));
                        for reply in &thread.replies {
                            if let Some(c) = &reply.comment {
                                replies = replies.child(self.render_comment(c, Some(thread), theme, window, cx));
                            }
                        }
                        let reply_box = match self.reply.as_ref().filter(|(tid, _)| *tid == id) {
                            Some((_, input)) => div()
                                .flex()
                                .gap(px(6.))
                                .child(div().flex_1().child(TextInput::new(input)))
                                .child(Button::new(SharedString::from(format!("reply-send-{id}"))).size(ButtonSize::Sm).label(t!("Comment", context = "verb, submit button")).on_click(cx.listener(|this, _, _, cx| this.send_reply(cx))))
                                .into_any_element(),
                            None => {
                                let rid = id.clone();
                                let sid = id.clone();
                                div()
                                    .flex()
                                    .gap(px(6.))
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
                            .gap(px(10.))
                            .p(px(12.))
                            .rounded(theme.radii.lg)
                            .border_1()
                            .border_color(theme.colors.border)
                            .child(header)
                            .child(card)
                            .child(replies)
                            .child(div().pl(px(34.)).child(reply_box));
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
                            .gap(px(8.))
                            .pl(px(4.))
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
        let composer = match &self.composer {
            Some(input) => div()
                .flex()
                .gap(px(6.))
                .child(div().flex_1().child(TextInput::new(input)))
                .child(Button::new("tl-comment-send").size(ButtonSize::Sm).label(t!("Comment", context = "verb, submit button")).on_click(cx.listener(|this, _, _, cx| this.send_comment(cx))))
                .into_any_element(),
            None => Button::new("tl-comment").size(ButtonSize::Sm).variant(ButtonVariant::Outline).icon(IconName::MessageSquarePlus).label(t!("Leave a comment")).on_click(cx.listener(|this, _, window, cx| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Leave a comment")));
                input.update(cx, |i, cx| i.focus(window, cx));
                this.composer = Some(input);
                cx.notify();
            }))
            .into_any_element(),
        };
        column.child(composer).into_any_element()
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
                    .gap(px(8.))
                    .h(px(36.))
                    .px(px(16.))
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
                .gap(px(10.))
                .py(px(48.))
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
        let mut excerpts = div().flex().flex_col().gap(px(12.));
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
                    .child(div().px(px(10.)).py(px(6.)).bg(theme.colors.muted).mono_text(px(11.), weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(format!("{} · {}", excerpt.path, excerpt.caption))))
                    .when(cut.is_empty(), |el| el.child(div().p(px(10.)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(t!("This file is not in the loaded diff."))))
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
                            .mono_text(px(12.), weight::NORMAL)
                            .text_color(theme.colors.text)
                            .child(div().w(px(40.)).pr(px(6.)).flex().justify_end().text_color(theme.colors.muted_foreground).child(line.to_string()))
                            .child(div().w(px(14.)).child(sign))
                            .child(div().flex_1().whitespace_nowrap().overflow_hidden().child(SharedString::from(row.text.trim_end_matches('\n').to_owned())))
                            .children((!marks.is_empty()).then(|| div().px(px(6.)).mr(px(6.)).rounded_full().bg(theme.colors.primary().opacity(0.2)).ui_text(theme, TextSize::Xs3, weight::SEMIBOLD).child(marks.join(" "))))
                    })),
            );
        }
        let summary = self.render_markdown(&format!("stop{position}"), &stop.summary, window, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(20.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.muted_foreground).child(t!("Review tour")))
                    .child(div().flex_1())
                    .child(Button::new("guide-prev").size(ButtonSize::Xs).variant(ButtonVariant::Outline).label(t!("Previous stop")).disabled(position == 0).on_click(cx.listener(|this, _, _, cx| this.step_tour(false, cx))))
                    .child(Button::new("guide-next").size(ButtonSize::Xs).variant(ButtonVariant::Outline).label(t!("Next stop")).disabled(position + 1 >= guide.stops.len()).on_click(cx.listener(|this, _, _, cx| this.step_tour(true, cx)))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .children(guide.stops.iter().enumerate().map(|(i, s)| {
                        div()
                            .id(("guide-stop", i))
                            .test_support()
                            .flex_1()
                            .h(px(4.))
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
                        .gap(px(8.))
                        .p(px(10.))
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
                        .gap(px(8.))
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
        let header = self.render_header(&theme, cx);
        let tabs = self.render_tabs(&theme, cx);
        let body = match self.tab {
            DetailTab::Summary => self.render_summary(&theme, window, cx),
            DetailTab::Guide => self.render_guide(&theme, window, cx),
            DetailTab::Timeline => self.render_timeline(&theme, window, cx),
            DetailTab::Code => self.render_code(&theme, cx),
        };
        let scrolls = self.tab != DetailTab::Code;
        div()
            .id("pr-detail")
            .test_support()
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
