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
    i18n::t,
};
use super::ui::menu::{MenuItem, MenuItemKind, MenuPopup};
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
        self.files = self
            .parsed()
            .map(|parsed| {
                parsed
                    .files
                    .iter()
                    .map(|file| {
                        let identity = file.identity_key();
                        let occurrence = occurrences.entry(identity.clone()).or_default();
                        let key = if *occurrence == 0 { identity.clone() } else { format!("{identity}\0{occurrence}") };
                        *occurrence += 1;
                        (file.clone(), SharedString::from(key))
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
                        .when(narrow_refs, |el| el.child("vs"))
                        .child(div().flex_none().max_w(px(192.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(base)),
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
            .child(toggle("diff-whitespace", "icons/diffs/pilcrow.svg", ignore_whitespace).tooltip(tooltip(if ignore_whitespace { t!("Show whitespace changes") } else { t!("Hide whitespace changes") })).on_click(cx.listener(move |_, _, _, cx| {
                cx.global_mut::<DiffPanelStore>().ignore_whitespace = Some(!ignore_whitespace);
                cx.emit(DiffPanelEvent::IgnoreWhitespaceChanged(!ignore_whitespace));
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
        let theme = cx.app_theme();
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
                    .kind(MenuItemKind::Radio(matches!(selection, DiffPanelSelection::Branch { .. })))
                    .on_hover(hover(None))
                    .on_select(pick(Rc::new(|store, thread| store.select_git_scope(thread, GitScope::Branch)))),
            )
            .item(
                MenuItem::new(t!("Uncommitted"))
                    .kind(MenuItemKind::Radio(selection == DiffPanelSelection::Unstaged))
                    .on_hover(hover(None))
                    .on_select(pick(Rc::new(|store, thread| store.select_git_scope(thread, GitScope::Unstaged)))),
            );
        let mut latest_item = MenuItem::new(t!("Latest turn")).kind(MenuItemKind::Radio(latest_checked)).on_hover(hover(None));
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
            match submenu {
                ScopeSubmenu::Turn => {
                    for turn in &self.input.turns {
                        let checked = matches!(&selection, DiffPanelSelection::Turn { turn_id, .. } if turn.turn_id.as_ref() == turn_id);
                        let id = turn.turn_id.clone();
                        popup = popup.item(
                            MenuItem::new(t!("Turn {count}", count = turn.turn_count))
                                .kind(MenuItemKind::Radio(checked))
                                .detail(turn.completed_label.clone())
                                .on_select(pick(Rc::new(move |store, thread| store.select_turn(thread, &id, None)))),
                        );
                    }
                }
                ScopeSubmenu::Commits => {
                    for commit in &self.input.commits {
                        let checked = matches!(&selection, DiffPanelSelection::Commit { sha, .. } if commit.sha.as_ref() == sha);
                        let sha = commit.sha.clone();
                        let short: SharedString = commit.sha.chars().take(7).collect::<String>().into();
                        popup = popup.item(
                            MenuItem::new(commit.subject.clone())
                                .kind(MenuItemKind::Radio(checked))
                                .detail(format!("{}  {short}", commit.relative_label))
                                .on_select(pick(Rc::new(move |store, thread| store.select_commit(thread, &sha)))),
                        );
                    }
                    if self.input.commits.is_empty() {
                        popup = popup.item(MenuItem::new(t!("No commits on this branch.")).disabled(true));
                    }
                }
            }
            let index = match submenu {
                ScopeSubmenu::Turn => 3.,
                ScopeSubmenu::Commits => 4.,
            };
            // Base UI's sub popup: inline-end, aligned to its trigger row 5px up.
            div()
                .mt(px(1.) + theme.sp(1.) + theme.sp(7.) * index - px(5.))
                .max_h(px(320.))
                .max_w(px(384.))
                .child(popup)
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
        let panel = cx.entity();
        div()
            .size_full()
            .relative()
            .child(DiffPanelShell::new(header, body))
            .children(menu)
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
