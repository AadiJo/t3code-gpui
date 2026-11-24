//! The diff panel. Ports `DiffPanel.tsx`: a scope menu (branch changes,
//! uncommitted, a turn, a commit), the toolbar toggles, the code view and the
//! file tree. Data comes from the owner as a [`DiffPanelInput`]; the panel
//! answers with [`DiffPanelEvent`]s that name the RPC to make
//! (`orchestration.getTurnDiff`, `review.getDiffPreview`, `review.getDiffFileContents`).

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use gpui_kit::{base::TestSupportExt as _, prelude::FluentBuilder as _, *};
use gpui_kit::component::input::{Input, InputEvent, InputState};

use super::{
    diff_panel_shell::{DiffPanelLoadingState, DiffPanelShell},
    diffs::{
        CodeView, CodeViewEvent, CodeViewFile, CodeViewOptions, ViewMode,
        diff_file_tree::{DiffFileTree, SelectFile, diff_file_tree_entries},
        review_comment::ReviewComment,
    },
};
use crate::{
    ActiveAppTheme as _,
    diff_count::format_compact_diff_count,
    diff_file_kind::is_test_changed_file,
    diff_panel_store::{DiffLayout, DiffPanelSelection, DiffPanelStore, GitScope},
    diff_patch::{FileContents, FileDiff, ParsedPatch, RenderablePatch},
    file_context_menu::{FileContextMenuAction, FileContextMenuCapabilities, file_context_menu_items},
    i18n::t,
};
use super::icons::{Icon, IconName, IconSize};
use super::ui::menu::{MenuItem, MenuItemKind, MenuPopup};
use super::ui::popover::glass_surface;
use super::ui::switch::{Switch, SwitchSize};
use super::ui::{ControlHighlightExt as _, TextStyleExt as _};
use crate::appearance_fonts::{TextSize, weight};
use super::ui::tooltip::tooltip;

/// A completed turn the scope menu lists, newest first.
#[derive(Clone, Debug)]
pub struct TurnSummary {
    pub turn_id: SharedString,
    pub turn_count: u32,
    /// `formatShortTimestamp(completedAt)`, preformatted by the owner.
    pub completed_label: SharedString,
}

/// A commit on the branch (`review.getBranchCommits`).
#[derive(Clone, Debug)]
pub struct CommitSummary {
    pub sha: SharedString,
    pub subject: SharedString,
    /// `formatRelativeTimeLabel(committedAt)`.
    pub relative_label: SharedString,
    /// `projectCommitUrl`: the commit on the repository's host, when known.
    pub url: Option<SharedString>,
}

/// Where the Commits submenu's list stands.
#[derive(Clone, Debug)]
pub enum CommitsStatus {
    Loading,
    /// `truncated` when the server cut the list at its limit.
    Loaded { truncated: bool },
    Error(SharedString),
}

/// One entry of the base-ref picker (`BaseRefChoice`): a local branch with its
/// remote twin, a local branch alone, or a remote-only ref.
#[derive(Clone, Debug)]
pub struct BaseRefChoice {
    pub label: SharedString,
    pub local: Option<SharedString>,
    pub remote: Option<SharedString>,
}

impl BaseRefChoice {
    /// `filterBaseRefChoices`: label or either ref name contains the query.
    fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || [Some(&self.label), self.local.as_ref(), self.remote.as_ref()]
                .into_iter()
                .flatten()
                .any(|name| name.to_lowercase().contains(&query))
    }
}

/// What the body shows.
#[derive(Clone, Debug)]
pub enum DiffPanelContent {
    NoThread,
    NotGitRepo,
    NoTurns,
    Loading(SharedString),
    Error(SharedString),
    /// The selection has no patch; `no_net_changes` when it resolved to an empty diff.
    Empty { no_net_changes: bool },
    Patch {
        patch: Arc<RenderablePatch>,
        /// Banner text such as the size-limit notice.
        notices: Vec<SharedString>,
        /// `+A −D` per path from metadata, when patches are partial.
        stats: Option<Arc<HashMap<SharedString, (u32, u32)>>>,
    },
}

/// Everything the panel shows that comes from the server.
#[derive(Clone, Debug)]
pub struct DiffPanelInput {
    /// `scopedThreadKey`: `environmentId:threadId`.
    pub thread_key: Option<SharedString>,
    pub workspace_root: Option<SharedString>,
    pub turns: Vec<TurnSummary>,
    pub commits: Vec<CommitSummary>,
    pub commits_status: CommitsStatus,
    /// `buildBaseRefChoices` over `vcs.listRefs` (local and remote, head excluded).
    pub base_ref_choices: Vec<BaseRefChoice>,
    /// Branch being compared, and the comparison target label.
    pub head_ref: Option<SharedString>,
    pub base_ref: Option<SharedString>,
    pub can_refresh: bool,
    pub refreshing: bool,
    pub content: DiffPanelContent,
}

impl Default for DiffPanelInput {
    fn default() -> Self {
        Self {
            thread_key: None,
            workspace_root: None,
            turns: Vec::new(),
            commits: Vec::new(),
            commits_status: CommitsStatus::Loaded { truncated: false },
            base_ref_choices: Vec::new(),
            head_ref: None,
            base_ref: None,
            can_refresh: false,
            refreshing: false,
            content: DiffPanelContent::NoThread,
        }
    }
}

/// Requests from the panel to its owner.
#[derive(Clone, Debug)]
pub enum DiffPanelEvent {
    /// The scope changed; fetch its patch.
    SelectionChanged(DiffPanelSelection),
    /// Refetch the current git diff.
    Refresh,
    /// Whitespace hiding changed; refetch with `ignoreWhitespace`.
    IgnoreWhitespaceChanged(bool),
    /// Open a file in the file viewer (`openDiffFilePrimaryAction`).
    OpenFile(SharedString),
    /// Load full contents so context can expand (`review.getDiffFileContents`).
    RequestFileContents { key: SharedString, file: Arc<FileDiff> },
    /// Add a review comment to the composer draft.
    ReviewComment(ReviewComment),
    DeleteReviewComment(SharedString),
    /// A row of a file header's context menu (`useFileContextMenuHandler`).
    FileMenuAction { action: FileContextMenuAction, path: SharedString },
}

/// The scope menu's submenus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeSubmenu {
    Turn,
    Commits,
}

/// The diff panel view.
pub struct DiffPanel {
    input: DiffPanelInput,
    code_view: Entity<CodeView>,
    tree: Entity<DiffFileTree>,
    files: Vec<(Arc<FileDiff>, SharedString)>,
    test_file_count: usize,
    scope_menu_open: bool,
    scope_submenu: Option<ScopeSubmenu>,
    /// Where the scope trigger was painted, so the menu opens under it.
    scope_trigger: Option<Bounds<Pixels>>,
    wrap_override: Option<bool>,
    /// The base-ref picker's search field, while the picker is open.
    base_ref_picker: Option<(Entity<InputState>, Subscription)>,
    base_ref_trigger: Option<Bounds<Pixels>>,
    /// The file header menu: the file and where it was opened.
    file_menu: Option<(SharedString, Point<Pixels>)>,
    file_menu_capabilities: FileContextMenuCapabilities,
    /// The panel's width last frame, for the header's narrow layout.
    width: f32,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DiffPanelEvent> for DiffPanel {}

impl DiffPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let code_view = cx.new(|cx| CodeView::new(ViewMode::Diff, window, cx));
        let tree = cx.new(DiffFileTree::new);
        let subscriptions = vec![
            cx.subscribe(&code_view, Self::on_code_view_event),
            cx.subscribe(&tree, |this, _, SelectFile(path), cx| {
                this.reveal_path(path, cx);
            }),
        ];
        Self {
            input: DiffPanelInput::default(),
            code_view,
            tree,
            files: Vec::new(),
            test_file_count: 0,
            scope_menu_open: false,
            scope_submenu: None,
            scope_trigger: None,
            wrap_override: None,
            base_ref_picker: None,
            base_ref_trigger: None,
            file_menu: None,
            file_menu_capabilities: FileContextMenuCapabilities::default(),
            width: 0.,
            _subscriptions: subscriptions,
        }
    }

    pub fn code_view(&self) -> &Entity<CodeView> {
        &self.code_view
    }

    pub fn file_tree(&self) -> &Entity<DiffFileTree> {
        &self.tree
    }

    fn selection(&self, cx: &App) -> DiffPanelSelection {
        let key = self.input.thread_key.as_deref().unwrap_or_default();
        cx.global::<DiffPanelStore>().selection(key)
    }

    /// `reviewSectionId`: which part of the thread is under review.
    fn section_id(&self, cx: &App) -> SharedString {
        match self.selection(cx) {
            DiffPanelSelection::ToolCall { thread_id, item_id, .. } => format!("tool-call:{thread_id}:{item_id}").into(),
            DiffPanelSelection::Turn { turn_id, .. } => format!("turn:{turn_id}").into(),
            DiffPanelSelection::Commit { sha, .. } => format!("commit:{sha}").into(),
            DiffPanelSelection::Unstaged => "unstaged".into(),
            DiffPanelSelection::Branch { .. } => "branch".into(),
        }
    }

    /// The key folds and scroll positions are remembered under.
    fn scope_key(&self, cx: &App) -> Option<SharedString> {
        let thread = self.input.thread_key.as_ref()?;
        Some(format!("{thread}:{}", self.section_id(cx)).into())
    }

    fn scope_label(&self, cx: &App) -> SharedString {
        match self.selection(cx) {
            DiffPanelSelection::ToolCall { .. } => t!("Tool call"),
            DiffPanelSelection::Turn { turn_id, .. } => {
                let latest = self.input.turns.first().is_some_and(|t| t.turn_id.as_ref() == turn_id);
                match self.input.turns.iter().find(|t| t.turn_id.as_ref() == turn_id) {
                    _ if latest => t!("Latest turn"),
                    Some(turn) => t!("Turn {count}", count = turn.turn_count),
                    None => t!("Turn", context = "agent turn"),
                }
            }
            DiffPanelSelection::Commit { sha, .. } => self
                .input
                .commits
                .iter()
                .find(|c| c.sha.as_ref() == sha)
                .map(|c| c.subject.clone())
                .unwrap_or_else(|| t!("Commit", context = "a git commit")),
            DiffPanelSelection::Unstaged => t!("Uncommitted"),
            DiffPanelSelection::Branch { .. } => t!("Changes"),
        }
    }

    /// Feeds new server data. Folds and scroll position for the scope are restored.
    pub fn set_input(&mut self, input: DiffPanelInput, cx: &mut Context<Self>) {
        self.input = input;
        self.rebuild_files(cx);
        let scroll = self.scope_key(cx).and_then(|key| cx.global::<DiffPanelStore>().scroll_offset(&key));
        let (section_id, title) = (self.section_id(cx), self.scope_label(cx));
        let root = self.input.workspace_root.clone();
        self.code_view.update(cx, |view, cx| {
            view.set_section(section_id, title);
            view.set_workspace_root(root);
            view.set_scroll_offset(scroll.unwrap_or(0.), cx);
        });
        cx.notify();
    }

    fn parsed(&self) -> Option<&ParsedPatch> {
        match &self.input.content {
            DiffPanelContent::Patch { patch, .. } => match &**patch {
                RenderablePatch::Files(parsed) => Some(parsed),
                RenderablePatch::Raw { .. } => None,
            },
            _ => None,
        }
    }

    /// Recomputes file keys, the shown set and folds, then pushes them to the views.
    fn rebuild_files(&mut self, cx: &mut Context<Self>) {
        let mut occurrences: HashMap<String, usize> = HashMap::new();
        // Tool-call diffs are not re-fetched, so whitespace is hidden here (`getRenderablePatch`).
        let hide_whitespace =
            matches!(self.selection(cx), DiffPanelSelection::ToolCall { .. }) && cx.global::<DiffPanelStore>().effective_ignore_whitespace();
        self.files = self
            .parsed()
            .map(|parsed| {
                parsed
                    .files
                    .iter()
                    .map(|file| {
                        let file = if hide_whitespace { Arc::new(crate::diff_patch::hide_whitespace_changes(file)) } else { file.clone() };
                        let identity = file.identity_key();
                        let occurrence = occurrences.entry(identity.clone()).or_default();
                        let key = if *occurrence == 0 { identity.clone() } else { format!("{identity}\0{occurrence}") };
                        *occurrence += 1;
                        (file, SharedString::from(key))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.test_file_count = self.files.iter().filter(|(file, _)| is_test_changed_file(&file.path)).count();
        let store = cx.global::<DiffPanelStore>();
        let hide_tests = store.hide_test_files;
        let default_collapsed = store.settings.diff_files_collapsed;
        let collapsed = self.scope_key(cx).and_then(|key| store.collapsed_files(&key).cloned());
        let selected_path = match self.selection(cx) {
            DiffPanelSelection::Turn { file_path, .. } => file_path,
            _ => None,
        };
        let stats = match &self.input.content {
            DiffPanelContent::Patch { stats, .. } => stats.clone(),
            _ => None,
        };
        let shown: Vec<CodeViewFile> = self
            .files
            .iter()
            .filter(|(file, _)| !hide_tests || selected_path.as_deref() == Some(&*file.path) || !is_test_changed_file(&file.path))
            .map(|(file, key)| CodeViewFile {
                diff: file.clone(),
                key: key.clone(),
                collapsed: collapsed.as_ref().map_or(default_collapsed, |set| set.contains(key)),
                stat: stats.as_ref().and_then(|stats| stats.get(&file.path).copied()),
            })
            .collect();
        let entries = diff_file_tree_entries(shown.iter().map(|f| &f.diff));
        let options = self.code_view_options(cx);
        let scheme = options.scheme;
        self.code_view.update(cx, |view, cx| {
            view.set_options(options, cx);
            view.set_files(shown, cx);
        });
        self.tree.update(cx, |tree, cx| {
            tree.set_scheme(scheme);
            tree.set_entries(entries, cx);
        });
    }

    fn code_view_options(&self, cx: &App) -> CodeViewOptions {
        let settings = cx.global::<DiffPanelStore>().settings;
        CodeViewOptions {
            layout: settings.diff_layout,
            wrap: self.wrap_override.unwrap_or(settings.word_wrap),
            scheme: settings.diff_color_scheme,
            ..CodeViewOptions::default()
        }
    }

    fn sync_options(&mut self, cx: &mut Context<Self>) {
        let options = self.code_view_options(cx);
        self.code_view.update(cx, |view, cx| view.set_options(options, cx));
        cx.notify();
    }

    fn collapsed_set(&self, cx: &App) -> HashSet<SharedString> {
        let store = cx.global::<DiffPanelStore>();
        self.scope_key(cx)
            .and_then(|key| store.collapsed_files(&key).cloned())
            .unwrap_or_else(|| {
                if store.settings.diff_files_collapsed { self.files.iter().map(|(_, key)| key.clone()).collect() } else { HashSet::new() }
            })
    }

    fn on_code_view_event(&mut self, _: Entity<CodeView>, event: &CodeViewEvent, cx: &mut Context<Self>) {
        match event {
            CodeViewEvent::CollapsedChanged { key, collapsed } => {
                let Some(scope) = self.scope_key(cx) else { return };
                let mut set = self.collapsed_set(cx);
                if *collapsed {
                    set.insert(key.clone());
                } else {
                    set.remove(key);
                }
                cx.global_mut::<DiffPanelStore>().set_collapsed_files(scope, set);
                cx.notify();
            }
            CodeViewEvent::Scrolled { offset } => {
                if let Some(scope) = self.scope_key(cx) {
                    cx.global_mut::<DiffPanelStore>().set_scroll_offset(scope, *offset);
                }
            }
            CodeViewEvent::OpenFile { path } => cx.emit(DiffPanelEvent::OpenFile(path.clone())),
            CodeViewEvent::HeaderContextMenu { path, position } => {
                self.file_menu = Some((path.clone(), *position));
                cx.notify();
            }
            CodeViewEvent::RequestContents { key } => {
                if let Some((file, _)) = self.files.iter().find(|(_, k)| k == key) {
                    cx.emit(DiffPanelEvent::RequestFileContents { key: key.clone(), file: file.clone() });
                }
            }
            CodeViewEvent::Comment(comment) => cx.emit(DiffPanelEvent::ReviewComment(comment.clone())),
            CodeViewEvent::DeleteComment { id } => cx.emit(DiffPanelEvent::DeleteReviewComment(id.clone())),
        }
    }

    /// Delivers contents requested with [`DiffPanelEvent::RequestFileContents`].
    pub fn set_file_contents(&mut self, key: &str, contents: FileContents, cx: &mut Context<Self>) {
        self.code_view.update(cx, |view, cx| view.set_contents(key, Arc::new(contents), cx));
    }

    /// Unfolds and scrolls to a file by path (`revealDiffFile`).
    pub fn reveal_path(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.files.iter().find(|(file, _)| file.path.as_ref() == path).map(|(_, key)| key.clone()) else {
            return false;
        };
        if let Some(scope) = self.scope_key(cx) {
            cx.global_mut::<DiffPanelStore>().discard_scroll_offset(&scope);
        }
        self.code_view.update(cx, |view, cx| view.reveal_file(&key, cx));
        self.tree.update(cx, |tree, cx| tree.reveal(path, cx));
        true
    }

    /// Folds every shown file, or unfolds them all when all are folded.
    pub fn toggle_all_files(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<SharedString> = self.shown_keys(cx);
        let mut set = self.collapsed_set(cx);
        let collapse = !keys.iter().all(|key| set.contains(key));
        for key in &keys {
            if collapse {
                set.insert(key.clone());
            } else {
                set.remove(key);
            }
            self.code_view.update(cx, |view, cx| view.set_collapsed(key, collapse, cx));
        }
        if let Some(scope) = self.scope_key(cx) {
            let store = cx.global_mut::<DiffPanelStore>();
            store.discard_scroll_offset(&scope);
            store.set_collapsed_files(scope, set);
        }
        self.code_view.update(cx, |view, cx| view.set_scroll_offset(0., cx));
        cx.notify();
    }

    fn shown_keys(&self, cx: &App) -> Vec<SharedString> {
        let hide = cx.global::<DiffPanelStore>().hide_test_files;
        self.files.iter().filter(|(file, _)| !hide || !is_test_changed_file(&file.path)).map(|(_, key)| key.clone()).collect()
    }

    fn select(&mut self, apply: impl FnOnce(&mut DiffPanelStore, &str), cx: &mut Context<Self>) {
        let Some(thread) = self.input.thread_key.clone() else { return };
        apply(cx.global_mut::<DiffPanelStore>(), &thread);
        self.scope_menu_open = false;
        self.scope_submenu = None;
        let selection = self.selection(cx);
        cx.emit(DiffPanelEvent::SelectionChanged(selection));
        cx.notify();
    }

    // --- rendering ----------------------------------------------------------

    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let colors = theme.colors;
        let dark = theme.appearance == crate::theme_palette::Appearance::Dark;
        let store = cx.global::<DiffPanelStore>();
        let settings = store.settings;
        let wrap = self.wrap_override.unwrap_or(settings.word_wrap);
        let ignore_whitespace = store.effective_ignore_whitespace();
        let hide_tests = store.hide_test_files;
        let tree_open = store.file_tree_open;
        let selection = self.selection(cx);
        let label = self.scope_label(cx);
        let has_files = !self.files.is_empty();
        let (additions, deletions) = self.parsed().map(|p| p.line_stat()).unwrap_or_default();
        let (additions, deletions) = match &self.input.content {
            DiffPanelContent::Patch { stats: Some(stats), .. } => {
                stats.values().fold((0u64, 0u64), |(a, d), (x, y)| (a + *x as u64, d + *y as u64))
            }
            _ => (additions, deletions),
        };
        let all_collapsed = {
            let set = self.collapsed_set(cx);
            let keys = self.shown_keys(cx);
            !keys.is_empty() && keys.iter().all(|k| set.contains(k))
        };
        let addition_fg = cx.app_theme().fixed.diff_addition_foreground;
        let deletion_fg = cx.app_theme().fixed.diff_deletion_foreground;
        let pressed_bg = colors.input.opacity(0.64);
        let icon = |path: &'static str| svg().path(path).size(px(14.)).text_color(colors.muted_foreground);

        let toggle = |id: &'static str, path: &'static str, pressed: bool| {
            div()
                .id(id)
                .test_support()
                .size(px(28.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .when(pressed, |el| el.bg(pressed_bg))
                .hover(|el| el.bg(colors.toolbar_control_hover))
                .child(icon(path).when(pressed, |s| s.text_color(colors.text)))
        };
        let segment = |id: &'static str, path: &'static str, pressed: bool| {
            div()
                .id(id)
                .test_support()
                .h(px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .when(pressed, |el| el.bg(colors.input.opacity(if dark { 0.72 } else { 1. })))
                .hover(|el| el.bg(colors.input.opacity(0.32)))
                .child(svg().path(path).size(px(14.)).text_color(if pressed { colors.text } else { colors.muted_foreground }))
        };
        let git_scope = matches!(selection, DiffPanelSelection::Branch { .. });
        // The refs line is a 20rem container query on the web; the toolbar is about 380px.
        let narrow_refs = self.width > 0. && self.width - 32. - 388. < 320.;

        let left = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .id("diff-scope")
                    .test_support()
                    .relative()
                    .h(px(24.))
                    .px(px(7.))
                    .flex()
                    .when(matches!(selection, DiffPanelSelection::Commit { .. }), |el| el.flex_shrink(1.).min_w_0())
                    .when(!matches!(selection, DiffPanelSelection::Commit { .. }), |el| el.flex_none())
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(8.))
                    .bg(colors.secondary)
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.secondary_foreground)
                    .hover(|el| el.bg(colors.secondary.opacity(0.9)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scope_menu_open = !this.scope_menu_open;
                        this.scope_submenu = None;
                        cx.notify();
                    }))
                    .child({
                        let panel = cx.entity();
                        canvas(
                            move |bounds, _, cx| {
                                panel.update(cx, |panel, cx| {
                                    if panel.scope_trigger != Some(bounds) {
                                        panel.scope_trigger = Some(bounds);
                                        if panel.scope_menu_open {
                                            cx.notify();
                                        }
                                    }
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                    })
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
                    .child(svg().path("icons/diffs/chevron-down.svg").size(px(14.)).flex_none().text_color(colors.secondary_foreground.opacity(0.7))),
            )
            .when(git_scope && self.input.head_ref.is_some(), |el| {
                let head = self.input.head_ref.clone().unwrap_or_default();
                let base = self.input.base_ref.clone().unwrap_or_else(|| t!("Automatic", context = "compare against the default base ref"));
                el.child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(px(8.))
                        .overflow_hidden()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .when(!narrow_refs, |el| {
                            el.child(div().min_w_0().max_w(px(192.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(head))
                                .child(svg().path("icons/diffs/arrow-right.svg").size(px(14.)).flex_none().text_color(colors.muted_foreground.opacity(0.7)))
                        })
                        .when(narrow_refs, |el| el.child(t!("vs")))
                        .child(self.render_base_ref_trigger(base, cx)),
                )
            });

        let right = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .when(has_files, |el| {
                el.child(
                    div()
                        .mr(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .font_family(self.code_view.read(cx).mono_family())
                        .text_size(px(11.))
                        .child(div().text_color(addition_fg).child(format!("+{}", format_compact_diff_count(additions))))
                        .child(div().text_color(deletion_fg).child(format!("-{}", format_compact_diff_count(deletions)))),
                )
            })
            .when(self.input.can_refresh, |el| {
                el.child(toggle("diff-refresh", "icons/diffs/refresh-cw.svg", false).tooltip(tooltip(if self.input.refreshing { t!("Refreshing diff…") } else { t!("Refresh diff") })).on_click(cx.listener(|_, _, _, cx| cx.emit(DiffPanelEvent::Refresh))))
            })
            .when(has_files, |el| {
                el.child(
                    toggle(
                        "diff-collapse-all",
                        if all_collapsed { "icons/diffs/chevrons-up-down.svg" } else { "icons/diffs/chevrons-down-up.svg" },
                        false,
                    )
                    .tooltip(tooltip(if all_collapsed { t!("Expand all files") } else { t!("Collapse all files") }))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_all_files(cx))),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(2.))
                    .p(px(2.))
                    .rounded(px(8.))
                    .bg(colors.input.opacity(0.4))
                    .child(segment("diff-stacked", "icons/diffs/rows-3.svg", settings.diff_layout == DiffLayout::Stacked).on_click(cx.listener(|this, _, _, cx| {
                        crate::hooks::settings_bridge::set_diff_layout(DiffLayout::Stacked, cx);
                        this.sync_options(cx);
                    })))
                    .child(segment("diff-split", "icons/diffs/columns-2.svg", settings.diff_layout == DiffLayout::Split).on_click(cx.listener(|this, _, _, cx| {
                        crate::hooks::settings_bridge::set_diff_layout(DiffLayout::Split, cx);
                        this.sync_options(cx);
                    }))),
            )
            .child(toggle("diff-wrap", "icons/diffs/text-wrap.svg", wrap).tooltip(tooltip(if wrap { t!("Disable line wrapping") } else { t!("Enable line wrapping") })).on_click(cx.listener(move |this, _, _, cx| {
                this.wrap_override = Some(!wrap);
                this.sync_options(cx);
            })))
            .child(toggle("diff-whitespace", "icons/diffs/pilcrow.svg", ignore_whitespace).tooltip(tooltip(if ignore_whitespace { t!("Show whitespace changes") } else { t!("Hide whitespace changes") })).on_click(cx.listener(move |this, _, _, cx| {
                cx.global_mut::<DiffPanelStore>().ignore_whitespace = Some(!ignore_whitespace);
                cx.emit(DiffPanelEvent::IgnoreWhitespaceChanged(!ignore_whitespace));
                this.rebuild_files(cx);
                cx.notify();
            })))
            .when(self.test_file_count > 0, |el| {
                el.child(toggle("diff-hide-tests", "icons/diffs/flask-conical-off.svg", hide_tests).tooltip(tooltip(if hide_tests { t!("{count, plural, one {Show {count} hidden test file} other {Show {count} hidden test files}}", count = self.test_file_count) } else { t!("Hide test files") })).on_click(cx.listener(move |this, _, _, cx| {
                    cx.global_mut::<DiffPanelStore>().hide_test_files = !hide_tests;
                    this.rebuild_files(cx);
                    cx.notify();
                })))
            })
            .when(has_files, |el| {
                el.child(toggle("diff-file-tree-toggle", "icons/diffs/folder-tree.svg", tree_open).tooltip(tooltip(if tree_open { t!("Hide file tree") } else { t!("Show file tree") })).on_click(cx.listener(move |_, _, _, cx| {
                    cx.global_mut::<DiffPanelStore>().file_tree_open = !tree_open;
                    cx.notify();
                })))
            });
        div().flex().flex_1().min_w_0().items_center().justify_between().gap(px(8.)).child(left).child(right).into_any_element()
    }

    /// Opens the scope menu, with the Turn (`turns`) or Commits submenu open. Used by scenes.
    pub fn open_scope_menu(&mut self, submenu: Option<bool>, cx: &mut Context<Self>) {
        self.scope_menu_open = true;
        self.scope_submenu = submenu.map(|turns| if turns { ScopeSubmenu::Turn } else { ScopeSubmenu::Commits });
        cx.notify();
    }

    /// The scope menu (`DiffPanel.tsx` header): Changes, Uncommitted and Latest turn,
    /// then the Turn and Commits submenus, which open on hover beside their row.
    fn render_scope_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.scope_menu_open {
            return None;
        }
        let trigger = self.scope_trigger?;
        let theme = &cx.app_theme().clone();
        let selection = self.selection(cx);
        let this = cx.weak_entity();
        let pick = {
            let this = this.clone();
            move |apply: Rc<dyn Fn(&mut DiffPanelStore, &str)>| {
                let this = this.clone();
                move |_: &mut Window, cx: &mut App| {
                    let apply = apply.clone();
                    this.update(cx, |panel, cx| panel.select(move |store, thread| apply(store, thread), cx)).ok();
                }
            }
        };
        let hover = {
            let this = this.clone();
            move |submenu: Option<ScopeSubmenu>| {
                let this = this.clone();
                move |hovered: &bool, _: &mut Window, cx: &mut App| {
                    if *hovered {
                        this.update(cx, |panel, cx| {
                            if panel.scope_submenu != submenu {
                                panel.scope_submenu = submenu;
                                cx.notify();
                            }
                        })
                        .ok();
                    }
                }
            }
        };
        let latest = self.input.turns.first().map(|t| t.turn_id.clone());
        let latest_checked = matches!((&selection, &latest), (DiffPanelSelection::Turn { turn_id, .. }, Some(latest)) if latest.as_ref() == turn_id);
        let mut menu = MenuPopup::new()
            .item(
                MenuItem::new(t!("Changes"))
                    .id("scope-branch")
                    .kind(MenuItemKind::Radio(matches!(selection, DiffPanelSelection::Branch { .. })))
                    .on_hover(hover(None))
                    .on_select(pick(Rc::new(|store, thread| store.select_git_scope(thread, GitScope::Branch)))),
            )
            .item(
                MenuItem::new(t!("Uncommitted"))
                    .id("scope-unstaged")
                    .kind(MenuItemKind::Radio(selection == DiffPanelSelection::Unstaged))
                    .on_hover(hover(None))
                    .on_select(pick(Rc::new(|store, thread| store.select_git_scope(thread, GitScope::Unstaged)))),
            );
        let mut latest_item = MenuItem::new(t!("Latest turn")).id("scope-latest").kind(MenuItemKind::Radio(latest_checked)).on_hover(hover(None));
        if let Some(latest) = latest {
            latest_item = latest_item.on_select(pick(Rc::new(move |store, thread| store.select_turn(thread, &latest, None))));
        }
        menu = menu.item(latest_item);
        let submenu_trigger = |label: SharedString, submenu: ScopeSubmenu| {
            let toggle = this.clone();
            let mut item = MenuItem::new(label).kind(MenuItemKind::Submenu).on_hover(hover(Some(submenu)));
            if self.scope_submenu == Some(submenu) {
                item = item.highlighted();
            }
            // A click opens the submenu too; it must not close the menu.
            item.on_select(move |_, cx| {
                toggle
                    .update(cx, |panel, cx| {
                        panel.scope_menu_open = true;
                        panel.scope_submenu = Some(submenu);
                        cx.notify();
                    })
                    .ok();
            })
        };
        menu = menu
            .item(submenu_trigger(t!("Turn", context = "agent turn"), ScopeSubmenu::Turn))
            .item(submenu_trigger(t!("Commits"), ScopeSubmenu::Commits));

        let submenu = self.scope_submenu.map(|submenu| {
            let mut popup = MenuPopup::new();
            let mut commit_status = None;
            match submenu {
                ScopeSubmenu::Turn => {
                    for turn in &self.input.turns {
                        let checked = matches!(&selection, DiffPanelSelection::Turn { turn_id, .. } if turn.turn_id.as_ref() == turn_id);
                        let id = turn.turn_id.clone();
                        popup = popup.item(
                            MenuItem::new(t!("Turn {count}", count = turn.turn_count))
                                .id(format!("scope-turn-{}", turn.turn_id))
                                .kind(MenuItemKind::Radio(checked))
                                .detail(turn.completed_label.clone())
                                .on_select(pick(Rc::new(move |store, thread| store.select_turn(thread, &id, None)))),
                        );
                    }
                }
                ScopeSubmenu::Commits => {
                    if let Some(status) = match &self.input.commits_status {
                        CommitsStatus::Error(error) => Some(error.clone()),
                        CommitsStatus::Loading => Some(t!("Loading commits...")),
                        CommitsStatus::Loaded { .. } if self.input.commits.is_empty() => Some(t!("No commits on this branch.")),
                        CommitsStatus::Loaded { truncated: true } => Some(t!(
                            "{count, plural, one {Showing latest {count} commits} other {Showing latest {count} commits}}",
                            count = self.input.commits.len()
                        )),
                        CommitsStatus::Loaded { .. } => None,
                    } {
                        commit_status = Some(status);
                    }
                }
            }
            let commits = (submenu == ScopeSubmenu::Commits).then(|| self.render_commit_rows(&selection, cx));
            let index = match submenu {
                ScopeSubmenu::Turn => 3.,
                ScopeSubmenu::Commits => 4.,
            };
            // Base UI's sub popup: inline-end, aligned to its trigger row 5px up.
            div()
                .mt(px(1.) + theme.sp(1.) + theme.sp(7.) * index - px(5.))
                .max_h(px(320.))
                .max_w(px(384.))
                .child(match commits {
                    // The commit rows sit above the status line, in the same surface.
                    Some(rows) => glass_surface(theme)
                        .id("diff-scope-commits")
                        .test_support()
                        .p(theme.sp(1.))
                        .control_highlight(theme)
                        .child(rows)
                        .children(commit_status.map(|status| {
                            div()
                                .min_h(theme.sp(7.))
                                .px(theme.sp(2.))
                                .py(theme.sp(1.))
                                .flex()
                                .items_center()
                                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                                .opacity(super::ui::DISABLED_OPACITY)
                                .child(status)
                        }))
                        .into_any_element(),
                    None => popup.into_any_element(),
                })
        });
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.scope_menu_open = false;
            this.scope_submenu = None;
            cx.notify();
        });
        let at = point(trigger.left(), trigger.bottom() + px(4.));
        Some(
            deferred(
                anchored().position(at).snap_to_window_with_margin(px(8.)).child(
                    div()
                        .id("diff-scope-menu")
                        .test_support()
                        .occlude()
                        .flex()
                        .items_start()
                        .on_mouse_down_out(close)
                        .child(menu.on_dismiss({
                            let this = this.clone();
                            move |_, cx| {
                                this.update(cx, |panel, cx| {
                                    if panel.scope_submenu.is_none() {
                                        panel.scope_menu_open = false;
                                    }
                                    cx.notify();
                                })
                                .ok();
                            }
                        }))
                        .children(submenu),
                ),
            )
            .with_priority(5)
            .into_any_element(),
        )
    }

    /// The Commits submenu's rows: the subject and its relative time select the
    /// commit; the short SHA beside them, in mono, opens the commit on its host
    /// when there is a URL (underlined), else is plain text.
    fn render_commit_rows(&self, selection: &DiffPanelSelection, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let mono = self.code_view.read(cx).mono_family();
        let this = cx.weak_entity();
        let rows = self.input.commits.iter().enumerate().map(|(index, commit)| {
            let checked = matches!(selection, DiffPanelSelection::Commit { sha, .. } if commit.sha.as_ref() == sha);
            let short: SharedString = commit.sha.chars().take(7).collect::<String>().into();
            let sha = commit.sha.clone();
            let pick = this.clone();
            let sha_label = div().font_family(mono.clone()).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(short.clone());
            let sha_part = match commit.url.clone() {
                Some(url) => div()
                    .id(("scope-commit-link", index))
                    .flex_none()
                    .min_h(theme.sp(7.))
                    .px(theme.sp(2.))
                    .flex()
                    .items_center()
                    .rounded(theme.radii.sm)
                    .cursor_pointer()
                    .tooltip(tooltip(t!("Open commit {sha}", sha = short.clone())))
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .child(sha_label.underline())
                    .into_any_element(),
                None => div().flex_none().px(theme.sp(2.)).child(sha_label).into_any_element(),
            };
            let subject = commit.subject.clone();
            div()
                .flex()
                .items_center()
                .rounded(theme.radii.sm)
                .when(checked, |row| row.bg(c.foreground().opacity(0.08)))
                .hover(|row| row.bg(c.highlight()))
                .child(
                    div()
                        .id(("scope-commit", index))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .min_h(theme.sp(7.))
                        .px(theme.sp(2.))
                        .py(theme.sp(1.))
                        .flex()
                        .items_center()
                        .gap(theme.sp(3.))
                        .cursor_pointer()
                        .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                        .text_color(c.foreground())
                        .tooltip(tooltip(subject.clone()))
                        .on_click(move |_, _, cx| {
                            let sha = sha.clone();
                            pick.update(cx, |panel, cx| panel.select(move |store, thread| store.select_commit(thread, &sha), cx)).ok();
                        })
                        .child(div().flex_1().min_w_0().truncate().child(subject))
                        .child(div().flex_none().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(commit.relative_label.clone())),
                )
                .child(sha_part)
                .into_any_element()
        });
        div().flex().flex_col().children(rows).into_any_element()
    }

    /// The comparison target button (`ComboboxTrigger`, ghost-muted xs).
    fn render_base_ref_trigger(&self, base: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let open = self.base_ref_picker.is_some();
        let panel = cx.entity();
        div()
            .id("diff-base-ref")
            .test_support()
            .relative()
            .flex_none()
            .min_w(theme.sp(9.))
            .max_w(theme.sp(48.))
            .h(theme.sp(6.))
            .px(theme.sp(2.))
            .flex()
            .items_center()
            .gap(theme.sp(1.))
            .rounded(theme.radii.md)
            .text_color(c.muted_foreground)
            .when(open, |el| el.bg(c.highlight()))
            .hover(|el| el.bg(c.highlight()).text_color(c.foreground()))
            .cursor_pointer()
            .tooltip(tooltip(t!("Change comparison target. Currently {base}", base = base.clone())))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_base_ref_picker(window, cx)))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        panel.update(cx, |panel, _| panel.base_ref_trigger = Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(div().min_w_0().truncate().child(base))
            .child(Icon::new(IconName::ChevronDown).size(IconSize::Custom(theme.sp(3.5))).opacity(0.7))
            .into_any_element()
    }

    /// Opens the file header menu for `path` at `position`. Used by scenes.
    pub fn open_file_menu(&mut self, path: SharedString, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.file_menu = Some((path, position));
        cx.notify();
    }

    /// The host's editors and reveal support, for the file header menu.
    pub fn set_file_menu_capabilities(&mut self, capabilities: FileContextMenuCapabilities, cx: &mut Context<Self>) {
        self.file_menu_capabilities = capabilities;
        cx.notify();
    }

    /// The file header menu: Open, Reveal, Open with › each editor.
    fn render_file_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (path, at) = self.file_menu.clone()?;
        let items = file_context_menu_items(&self.file_menu_capabilities, self.input.workspace_root.is_some());
        if items.is_empty() {
            return None;
        }
        let this = cx.weak_entity();
        let row = |item: &crate::file_context_menu::FileContextMenuItem| {
            let mut row = MenuItem::new(item.label.clone());
            if let Some(icon) = item.icon {
                row = row.icon(icon);
            }
            if let Some(action) = item.action.clone() {
                let (this, path) = (this.clone(), path.clone());
                row = row.on_select(move |_, cx| {
                    this.update(cx, |panel, cx| {
                        panel.file_menu = None;
                        cx.emit(DiffPanelEvent::FileMenuAction { action: action.clone(), path: path.clone() });
                        cx.notify();
                    })
                    .ok();
                });
            }
            row
        };
        let mut popup = MenuPopup::new().context();
        for item in &items {
            if item.children.is_empty() {
                popup = popup.item(row(item));
            } else {
                popup = popup.label(item.label.clone());
                for child in &item.children {
                    popup = popup.item(row(child).inset());
                }
            }
        }
        let dismiss = this.clone();
        let popup = popup.on_dismiss(move |_, cx| {
            dismiss.update(cx, |panel, cx| {
                panel.file_menu = None;
                cx.notify();
            })
            .ok();
        });
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.file_menu = None;
            cx.notify();
        });
        Some(
            deferred(anchored().position(at).snap_to_window_with_margin(px(8.)).child(div().id("diff-file-menu").test_support().occlude().on_mouse_down_out(close).child(popup)))
                .with_priority(5)
                .into_any_element(),
        )
    }

    /// Opens or closes the base-ref picker; opening focuses its search field.
    pub fn toggle_base_ref_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.base_ref_picker.take().is_some() {
            cx.notify();
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search refs...")));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        self.base_ref_picker = Some((input, subscription));
        cx.notify();
    }

    fn select_base_ref(&mut self, base_ref: Option<SharedString>, cx: &mut Context<Self>) {
        self.base_ref_picker = None;
        self.select(move |store, thread| store.select_branch_base_ref(thread, base_ref.as_deref()), cx);
    }

    /// The base-ref picker (`Combobox` in `DiffPanel.tsx`): search, a Branch /
    /// Remote header, Automatic, then each choice with a switch between its
    /// local and remote ref when it has both.
    fn render_base_ref_picker(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (input, _) = self.base_ref_picker.as_ref()?;
        let trigger = self.base_ref_trigger?;
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let query = input.read(cx).value().to_string();
        let selected = match self.selection(cx) {
            DiffPanelSelection::Branch { base_ref } => base_ref.map(SharedString::from),
            _ => None,
        };
        let item_row = |id: ElementId, checked: bool| {
            div()
                .id(id)
                .test_support()
                .w_full()
                .min_w_0()
                .min_h(theme.sp(7.))
                .px(theme.sp(2.))
                .py(theme.sp(1.))
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .rounded(theme.radii.sm)
                .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.foreground())
                .cursor_pointer()
                .hover(|el| el.bg(c.highlight()))
                .child(div().w(theme.sp(4.)).flex_none().when(checked, |slot| {
                    slot.child(svg().path("icons/ui/menu-check.svg").size(theme.sp(4.)).text_color(c.foreground()))
                }))
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        if query.trim().is_empty() {
            rows.push(
                item_row("base-ref-automatic".into(), selected.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.select_base_ref(None, cx)))
                    .child(div().min_w_0().truncate().child(t!("Automatic", context = "compare against the default base ref")))
                    .into_any_element(),
            );
        }
        for (index, choice) in self.input.base_ref_choices.iter().enumerate().filter(|(_, choice)| choice.matches(&query)) {
            let use_remote = selected.is_some() && selected == choice.remote;
            let value = if use_remote { choice.remote.clone() } else { choice.local.clone().or_else(|| choice.remote.clone()) };
            let checked = value.is_some() && value == selected;
            let pick = value.clone();
            let trailing: Option<AnyElement> = match (&choice.local, &choice.remote) {
                (Some(local), Some(remote)) => {
                    let (local, remote) = (local.clone(), remote.clone());
                    let panel = cx.weak_entity();
                    Some(
                        div()
                            .id(("base-ref-switch", index))
                            .flex()
                            .justify_end()
                            .tooltip(tooltip(t!("Use remote version of {ref}", r#ref = choice.label.clone())))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(Switch::new(("base-ref-remote", index), use_remote).size(SwitchSize::Sm).on_change(move |checked, _, cx| {
                                let next = if checked { remote.clone() } else { local.clone() };
                                panel.update(cx, |panel, cx| panel.select_base_ref(Some(next), cx)).ok();
                            }))
                            .into_any_element(),
                    )
                }
                (None, Some(_)) => Some(
                    div()
                        .id(("base-ref-remote-only", index))
                        .flex()
                        .justify_end()
                        .tooltip(tooltip(t!("Remote only")))
                        .child(Icon::new(IconName::Check).size(IconSize::Custom(theme.sp(3.))).color(c.muted_foreground))
                        .into_any_element(),
                ),
                _ => None,
            };
            rows.push(
                item_row(("base-ref", index).into(), checked)
                    .on_click(cx.listener(move |this, _, _, cx| this.select_base_ref(pick.clone(), cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .child(div().flex_1().min_w_0().pr(theme.sp(2.)).truncate().child(choice.label.clone()))
                            .child(div().w(theme.sp(8.)).flex_none().children(trailing)),
                    )
                    .into_any_element(),
            );
        }
        let empty = rows.is_empty();
        let close = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.base_ref_picker = None;
            cx.notify();
        });
        let at = point(trigger.left(), trigger.bottom() + px(4.));
        Some(
            deferred(
                anchored().position(at).snap_to_window_with_margin(px(8.)).child(
                    glass_surface(&theme)
                        .id("diff-base-ref-picker")
                        .test_support()
                        .occlude()
                        .w(theme.rem(18.))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .control_highlight(&theme)
                        .on_mouse_down_out(close)
                        .child(
                            div().px(theme.sp(3.)).pt(theme.sp(2.5)).child(
                                div()
                                    .relative()
                                    .border_b_1()
                                    .border_color(c.border.opacity(0.7))
                                    .pb(theme.sp(1.5))
                                    .child(
                                        div()
                                            .absolute()
                                            .top(theme.sp(1.5))
                                            .left_0()
                                            .child(Icon::new(IconName::Search).size(IconSize::Custom(theme.sp(4.))).color(c.muted_foreground.opacity(0.55))),
                                    )
                                    .child(
                                        div().pl(theme.sp(5.)).child(
                                            Input::new(input).appearance(false).bordered(false).focus_bordered(false).p_0().h(theme.sp(6.5)).ui_text(&theme, TextSize::Sm, weight::NORMAL).w_full(),
                                        ),
                                    ),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.sp(2.))
                                .pl(theme.sp(3.))
                                .pr(theme.sp(6.5))
                                .pt(theme.sp(2.))
                                .pb(theme.sp(1.5))
                                .border_b_1()
                                .border_color(c.border.opacity(0.7))
                                .text_size(theme.rem(0.625))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(c.muted_foreground)
                                .child(div().w(theme.sp(4.)).flex_none())
                                .child(div().flex_1().min_w_0().child(t!("Branch", context = "git branch").to_uppercase()))
                                .child(div().w(theme.sp(8.)).flex_none().flex().justify_end().child(t!("Remote", context = "git remote").to_uppercase())),
                        )
                        .when(empty, |el| {
                            el.child(
                                div().py(theme.sp(6.)).flex().justify_center().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(t!("No matching refs.")),
                            )
                        })
                        .child(div().id("diff-base-ref-list").max_h(theme.sp(64.)).overflow_y_scroll().p(theme.sp(1.)).children(rows)),
                ),
            )
            .with_priority(5)
            .into_any_element(),
        )
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.app_theme().colors;
        let message = |text: SharedString| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px(px(20.))
                .text_size(px(12.))
                .text_color(colors.muted_foreground.opacity(0.7))
                .child(text)
                .into_any_element()
        };
        match self.input.content.clone() {
            DiffPanelContent::NoThread => message(t!("Select a thread to inspect turn diffs.")),
            DiffPanelContent::NotGitRepo => message(t!("Turn diffs are unavailable because this project is not a git repository.")),
            DiffPanelContent::NoTurns => message(t!("No completed turns yet.")),
            DiffPanelContent::Loading(label) => DiffPanelLoadingState::new(label).into_any_element(),
            DiffPanelContent::Error(error) => div()
                .px(px(12.))
                .pt(px(8.))
                .text_size(px(11.))
                .text_color(colors.error.opacity(0.8))
                .child(error)
                .into_any_element(),
            DiffPanelContent::Empty { no_net_changes } => {
                message(if no_net_changes { t!("No net changes in this selection.") } else { t!("No patch available for this selection.") })
            }
            DiffPanelContent::Patch { patch, notices, .. } => {
                let banners = notices.into_iter().map(|notice| {
                    div()
                        .flex_none()
                        .px(px(12.))
                        .py(px(6.))
                        .border_b_1()
                        .border_color(colors.border.opacity(0.7))
                        .bg(colors.muted.opacity(0.4))
                        .text_size(px(11.))
                        .text_color(colors.muted_foreground)
                        .child(notice)
                });
                let content = match &*patch {
                    RenderablePatch::Raw { text, reason } => div()
                        .id("diff-raw")
                        .test_support()
                        .flex_1()
                        .overflow_y_scroll()
                        .p(px(8.))
                        .child(div().text_size(px(11.)).text_color(colors.muted_foreground.opacity(0.75)).child(reason.clone()))
                        .child(
                            div()
                                .mt(px(8.))
                                .p(px(12.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(colors.border.opacity(0.7))
                                .font_family(self.code_view.read(cx).mono_family())
                                .text_size(px(11.))
                                .text_color(colors.muted_foreground.opacity(0.9))
                                .child(text.clone()),
                        )
                        .into_any_element(),
                    RenderablePatch::Files(_) => {
                        let tree_open = cx.global::<DiffPanelStore>().file_tree_open;
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .child(div().flex_1().min_w_0().h_full().child(self.code_view.clone()))
                            .when(tree_open, |el| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .w(relative(0.4))
                                        .h_full()
                                        .border_l_1()
                                        .border_color(colors.border)
                                        .child(self.tree.clone()),
                                )
                            })
                            .into_any_element()
                    }
                };
                div().flex().flex_col().flex_1().min_h_0().children(banners).child(content).into_any_element()
            }
        }
    }
}

impl Render for DiffPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header(cx);
        let body = self.render_body(cx);
        let menu = self.render_scope_menu(cx);
        let base_ref_picker = self.render_base_ref_picker(cx);
        let file_menu = self.render_file_menu(cx);
        let panel = cx.entity();
        div()
            .size_full()
            .relative()
            .child(DiffPanelShell::new(header, body))
            .children(menu)
            .children(base_ref_picker)
            .children(file_menu)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let width = f32::from(bounds.size.width);
                        panel.update(cx, |panel, cx| {
                            if (panel.width - width).abs() > 0.5 {
                                panel.width = width;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}
