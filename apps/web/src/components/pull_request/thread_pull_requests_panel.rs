//! The Linked items right panel tab (`ThreadPullRequestsPanel.tsx`): the thread's issues, then its
//! pull requests as stacks and chains newest first, each row with its state, checks, review,
//! watch, diff stat, stack, author, repository and branches, and a menu to copy, open, watch or
//! unlink it; a footer with the counts, the last sync and "Link pull request"; the empty and
//! unavailable states. Reads the live shell row of its thread.

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px, rems,
};
use t3code_client_runtime::state::thread_pull_requests::{
    ChainKind, can_start_thread_pull_request_watch, is_open, pull_request_list_lines, resolve_thread_pull_request_chains,
    visible_thread_pull_requests,
};
use t3code_contracts::{
    OrchestrationV2Command as Command, OrchestrationV2CommandThreadPullRequestUnlink, OrchestrationV2CommandThreadPullRequestWatch, ThreadIssueLink,
    ThreadPullRequestLink, ThreadPullRequestLinkSource,
};

use super::pull_request_presentation::{avatar, diff_stat, relative_label, state_glyph};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::empty::Empty;
use crate::components::ui::menu::{ContextMenu, MenuItem, MenuPopup, dropdown_menu};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::i18n::t;
use crate::right_panel_store::{RightPanelStore, RightPanelSurface};
use crate::state::StateBridge;
use crate::{ActiveAppTheme as _, AppTheme};

gpui_kit::actions!(t3, [
    /// Opens "Link pull request" for the thread on screen (`openLinkPullRequestDialog`).
    OpenLinkPullRequestDialog
]);

/// The Linked items tab of one thread.
pub struct ThreadPullRequestsPanel {
    key: ThreadKey,
    panels: Entity<RightPanelStore>,
    _shell: Option<Subscription>,
}

/// `SOURCE_LABELS`.
fn source_label(source: ThreadPullRequestLinkSource) -> SharedString {
    match source {
        ThreadPullRequestLinkSource::Manual => t!("Linked by you"),
        ThreadPullRequestLinkSource::Created => t!("Created from this thread"),
        ThreadPullRequestLinkSource::Agent => t!("Linked by the agent"),
        ThreadPullRequestLinkSource::Stack => t!("Found in the stack"),
        ThreadPullRequestLinkSource::StackDismissed => t!("Dismissed", context = "pull request link source"),
    }
}

/// What a row's menu and actions need from the thread.
#[derive(Clone)]
struct RowContext {
    supports_watch: bool,
    can_start_watch: bool,
    /// The thread's project, for opening a pull request in the app.
    project_id: Option<String>,
    opens_in_app: bool,
}

impl ThreadPullRequestsPanel {
    pub fn new(key: ThreadKey, panels: Entity<RightPanelStore>, cx: &mut Context<Self>) -> Self {
        let shell = StateBridge::is_running(cx).then(|| cx.observe(&StateBridge::shell(cx), |_, _, cx| cx.notify()));
        Self { key, panels, _shell: shell }
    }

    fn thread_key_text(&self) -> String {
        format!("{}:{}", self.key.environment_id.0, self.key.thread_id.0)
    }

    fn dispatch(&self, command: Command, cx: &mut App) {
        let (environment_id, _) = crate::state::shell::runtime_ids(&self.key);
        let Some(environment) = StateBridge::environment(&environment_id, cx) else { return };
        let commands = environment.read(cx).commands.clone();
        let task = StateBridge::spawn(async move { commands.dispatch(command).await }, cx);
        cx.spawn(async move |cx| {
            if let Some(Err(error)) = task.await.map(|r| r.map(|_| ())) {
                cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, t!("Could not update the link")).description(SharedString::from(error.to_string())), cx));
            }
        })
        .detach();
    }

    /// Unlinks (or, for a stack member, dismisses) a pull request from the thread.
    pub fn unlink(&self, link: &ThreadPullRequestLink, cx: &mut App) {
        let (_, thread_id) = crate::state::shell::runtime_ids(&self.key);
        self.dispatch(
            Command::ThreadPullRequestUnlink(OrchestrationV2CommandThreadPullRequestUnlink {
                command_id: t3code_client_runtime::operations::commands::new_command_id(),
                thread_id,
                host: link.host.clone(),
                repository: link.repository.clone(),
                number: link.number,
            }),
            cx,
        );
    }

    /// Starts or stops watching a pull request for the thread's agent.
    pub fn set_watching(&self, link: &ThreadPullRequestLink, watching: bool, cx: &mut App) {
        let (_, thread_id) = crate::state::shell::runtime_ids(&self.key);
        self.dispatch(
            Command::ThreadPullRequestWatch(OrchestrationV2CommandThreadPullRequestWatch {
                command_id: t3code_client_runtime::operations::commands::new_command_id(),
                thread_id,
                host: link.host.clone(),
                repository: link.repository.clone(),
                number: link.number,
                watching,
                until: None,
                checks_pending: None,
                link: None,
            }),
            cx,
        );
    }

    /// `useOpenPrLink`: in the app as a pull request tab when the setting asks for it and the
    /// thread has a project, else in the browser.
    fn open_link(&self, link: &ThreadPullRequestLink, context: &RowContext, cx: &mut App) {
        match context.project_id.as_ref().filter(|_| context.opens_in_app) {
            Some(project_id) => {
                let surface = RightPanelSurface::pull_request(project_id, &link.repository, link.number);
                let thread = self.thread_key_text();
                self.panels.update(cx, |store, cx| {
                    store.open_surface(&thread, surface);
                    cx.notify();
                });
            }
            None => cx.open_url(&link.url),
        }
    }

    /// The row's menu: copy, open, watch and unlink.
    fn row_menu(this: &Entity<Self>, link: &ThreadPullRequestLink, context: &RowContext) -> MenuPopup {
        let watching = link.watch.is_some();
        let open = is_open(link);
        let mut menu = MenuPopup::new()
            .item(MenuItem::new(t!("Copy link")).icon(IconName::Link).on_select({
                let url = link.url.clone();
                move |_, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url.clone()));
                    Toaster::push(ToastData::new(ToastKind::Success, t!("Link copied")), cx);
                }
            }))
            .item(MenuItem::new(t!("Open", context = "verb, open a link")).icon(IconName::ArrowUpRight).on_select({
                let (this, link, context) = (this.clone(), link.clone(), context.clone());
                move |_, cx| this.update(cx, |this, cx| this.open_link(&link, &context, cx))
            }));
        if context.supports_watch && open && (watching || context.can_start_watch) {
            menu = menu.item(
                MenuItem::new(if watching { t!("Stop watching") } else { t!("Watch for changes") })
                    .icon(if watching { IconName::EyeOff } else { IconName::Eye })
                    .on_select({
                        let (this, link) = (this.clone(), link.clone());
                        move |_, cx| this.update(cx, |this, cx| this.set_watching(&link, !watching, cx))
                    }),
            );
        }
        menu.item(
            MenuItem::new(if link.source == ThreadPullRequestLinkSource::Stack { t!("Dismiss from thread") } else { t!("Unlink from thread") })
                .icon(IconName::Unlink)
                .on_select({
                    let (this, link) = (this.clone(), link.clone());
                    move |_, cx| this.update(cx, |this, cx| this.unlink(&link, cx))
                }),
        )
    }

    fn render_row(
        this: &Entity<Self>,
        index: usize,
        link: &ThreadPullRequestLink,
        depth: usize,
        stack: Option<(ChainKind, usize)>,
        context: &RowContext,
        now: i64,
        theme: &AppTheme,
    ) -> AnyElement {
        let muted = theme.colors.muted_foreground;
        let snapshot = link.snapshot.as_ref();
        let open = is_open(link);
        let watching = link.watch.is_some();
        let xs = |theme: &AppTheme| theme.rem(0.875);
        let glyph = match snapshot {
            None => div().flex_none().w(theme.rem(1.)).child(Icon::new(IconName::GitPullRequest).size(IconSize::Custom(theme.rem(1.))).color(muted)),
            Some(s) => div().flex_none().w(theme.rem(1.)).flex().flex_col().items_center().child(state_glyph(theme, s.state, s.is_draft, s.mergeability)),
        };
        let number = div()
            .id(SharedString::from(format!("linked-pr-number-{index}")))
            .flex_none()
            .mono_text(theme.rem(0.75), weight::NORMAL)
            .text_color(muted)
            .child(format!("#{}", link.number))
            .tooltip(tooltip(SharedString::from(format!("{} · {}", source_label(link.source), relative_label(&link.linked_at, now)))));
        let signals = open.then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.rem(0.25))
                .when(watching, |el| {
                    el.child(
                        div()
                            .id(SharedString::from(format!("linked-pr-watch-{index}")))
                            .child(Icon::new(IconName::Eye).size(IconSize::Custom(xs(theme))).color(theme.colors.text))
                            .tooltip(tooltip(t!("Watching: the agent wakes when checks finish, someone comments, or the branch conflicts"))),
                    )
                })
                .when_some(snapshot.and_then(|s| s.checks_state), |el, state| {
                    let (icon, color, label) = match state {
                        // `CHECKS_STATE_PRESENTATION`.
                        t3code_contracts::PullRequestChecksState::Passing => (IconName::CircleCheck, super::pull_request_presentation::open_tone(theme), t!("All checks have passed")),
                        t3code_contracts::PullRequestChecksState::Failing => (IconName::CircleX, theme.colors.destructive(), t!("Some checks were not successful")),
                        t3code_contracts::PullRequestChecksState::Pending => (IconName::CircleDot, super::pull_request_presentation::pending_tone(theme), t!("Some checks haven't completed yet")),
                    };
                    el.child(div().id(SharedString::from(format!("linked-pr-checks-{index}"))).child(Icon::new(icon).size(IconSize::Custom(xs(theme))).color(color)).tooltip(tooltip(label)))
                })
                .when_some(snapshot.and_then(|s| s.review_decision), |el, decision| {
                    // `reviewDecisionPresentation`.
                    let (icon, color, label) = match decision {
                        t3code_contracts::PullRequestReviewDecision::Approved => (IconName::UserCheck, super::pull_request_presentation::open_tone(theme), t!("Approved", context = "review decision")),
                        t3code_contracts::PullRequestReviewDecision::ChangesRequested => (IconName::UserRoundX, super::pull_request_presentation::pending_tone(theme).opacity(0.8), t!("Changes requested")),
                        _ => (IconName::UserRound, muted.opacity(0.6), t!("Awaiting review")),
                    };
                    el.child(div().id(SharedString::from(format!("linked-pr-review-{index}"))).child(Icon::new(icon).size(IconSize::Custom(xs(theme))).color(color)).tooltip(tooltip(label)))
                })
        });
        let status = div()
            .ml_auto()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.rem(0.375))
            .ui_text(theme, TextSize::Xs2, weight::NORMAL)
            .children(diff_stat(theme, snapshot.and_then(|s| s.additions).unwrap_or(0), snapshot.and_then(|s| s.deletions).unwrap_or(0)));
        let line1 = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(theme.rem(0.375))
            .child(number)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .text_color(theme.colors.text)
                    .child(SharedString::from(snapshot.map_or(link.repository.clone(), |s| s.title.clone()))),
            )
            .children(signals)
            .child(status);
        let stack_meta = stack.map(|(kind, size)| {
            let label = match kind {
                ChainKind::Native => t!("{host} stack of {size}: merging a layer lands the ones below it.", host = host_label(&link.url), size = size),
                ChainKind::Derived => t!(
                    "{size} {terms, select, mr {merge requests} cr {change requests} other {pull requests}} chained by base branch.",
                    size = size,
                    terms = "pr"
                ),
            };
            div()
                .id(SharedString::from(format!("linked-pr-stack-{index}")))
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.rem(0.125))
                .text_color(theme.colors.text.opacity(0.7))
                .child(Icon::new(IconName::Layers).size(IconSize::Custom(theme.rem(0.75))).color(theme.colors.text.opacity(0.7)))
                .child(size.to_string())
                .tooltip(tooltip(label))
        });
        let meta = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(theme.rem(0.375))
            .overflow_hidden()
            .ui_text(theme, TextSize::Xs2, weight::NORMAL)
            .text_color(muted)
            .children(stack_meta)
            .children(snapshot.and_then(|s| s.author.as_ref()).map(|author| {
                let name = author.name.clone().filter(|name| name != &author.login).map(|name| format!("{name} (@{})", author.login)).unwrap_or_else(|| author.login.clone());
                div()
                    .id(SharedString::from(format!("linked-pr-author-{index}")))
                    .flex()
                    .flex_none()
                    .min_w_0()
                    .items_center()
                    .gap(theme.rem(0.25))
                    .child(avatar(theme, Some(&author.login), 14.))
                    .child(div().max_w(theme.rem(7.)).truncate().child(SharedString::from(author.login.clone())))
                    .tooltip(tooltip(SharedString::from(name)))
            }))
            .map(|el| match snapshot {
                Some(s) => el
                    .child(
                        div()
                            .id(SharedString::from(format!("linked-pr-repo-{index}")))
                            .min_w_0()
                            .max_w(theme.rem(8.))
                            .truncate()
                            .mono_text(theme.rem(0.6875), weight::NORMAL)
                            .child(SharedString::from(link.repository.clone()))
                            .tooltip(tooltip(SharedString::from(link.repository.clone()))),
                    )
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(theme.rem(0.25))
                            .mono_text(theme.rem(0.6875), weight::NORMAL)
                            .child(div().min_w_0().truncate().child(SharedString::from(s.head_branch.clone())))
                            .child(div().flex_none().child("→"))
                            .child(div().flex_shrink_0().max_w(rems(8.)).truncate().child(SharedString::from(s.base_branch.clone()))),
                    ),
                None => el.child(div().min_w_0().truncate().mono_text(theme.rem(0.6875), weight::NORMAL).child(SharedString::from(format!("{}/{}", link.host, link.repository)))),
            })
            .children(snapshot.and_then(|s| s.updated_at.as_deref()).map(|at| div().ml_auto().flex_none().whitespace_nowrap().child(relative_label(at, now))));
        let menu_this = this.clone();
        let (menu_link, menu_context) = (link.clone(), context.clone());
        let actions = div()
            .absolute()
            .right_0()
            .bottom(theme.rem(0.125))
            .flex()
            .items_center()
            .rounded_r(theme.radii.md)
            .bg(theme.colors.background())
            .pr(theme.rem(0.25))
            .pl(theme.rem(1.25))
            .invisible()
            .group_hover("linked-pr-row", |s| s.visible())
            .child(dropdown_menu(
                SharedString::from(format!("linked-pr-actions-{index}")),
                Button::new(SharedString::from(format!("linked-pr-actions-trigger-{index}")))
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconMicro)
                    .icon(IconName::Ellipsis)
                    .icon_size(IconSize::Custom(xs(theme))),
                move |_, _| Self::row_menu(&menu_this, &menu_link, &menu_context),
            ));
        let (open_this, open_link, open_context) = (this.clone(), link.clone(), context.clone());
        let row = div()
            .id(SharedString::from(format!("linked-pr-row-{index}")))
            .group("linked-pr-row")
            .relative()
            .w_full()
            .flex()
            .items_center()
            .gap(theme.rem(0.5))
            .rounded(theme.radii.md)
            .py(theme.rem(0.25))
            .pr(theme.rem(0.25))
            .pl(theme.rem(0.5 + depth.min(3) as f32 * 1.25))
            .hover(|s| s.bg(theme.colors.accent_surface.opacity(0.6)))
            .cursor_pointer()
            .when(depth > 0, |el| el.child(div().flex_none().ml(-theme.rem(0.5)).h(theme.rem(1.5)).w(px(1.)).bg(theme.colors.border.opacity(0.7))))
            .child(glyph)
            .child(div().flex().flex_col().flex_1().min_w_0().child(line1).child(meta))
            .on_click(move |_, _, cx| open_this.update(cx, |this, cx| this.open_link(&open_link, &open_context, cx)))
            .child(actions);
        let (menu_this, menu_link, menu_context) = (this.clone(), link.clone(), context.clone());
        ContextMenu::new(SharedString::from(format!("linked-pr-context-{index}")), row, move |_, _| Self::row_menu(&menu_this, &menu_link, &menu_context)).into_any_element()
    }

    /// `LinkedIssueRow` (not compact): the issue as a pull request row; opens its tab when the
    /// server offers issues, else the browser.
    fn render_issue(&self, index: usize, link: &ThreadIssueLink, issues: bool, now: i64, theme: &AppTheme) -> AnyElement {
        let muted = theme.colors.muted_foreground;
        let (surface, url, thread) = (
            issues.then(|| RightPanelSurface::Issue {
                id: format!("issue:{}:{}", link.project_id.as_str(), link.key),
                project_id: link.project_id.to_string(),
                tracker: link.tracker.as_str().to_owned(),
                scope: link.scope.clone(),
                key: link.key.clone(),
            }),
            link.url.clone(),
            self.thread_key_text(),
        );
        let panels = self.panels.clone();
        div()
            .id(SharedString::from(format!("linked-issue-row-{index}")))
            .w_full()
            .flex()
            .items_center()
            .gap(theme.rem(0.5))
            .rounded(theme.radii.md)
            .py(theme.rem(0.25))
            .pr(theme.rem(0.25))
            .px(theme.rem(0.5))
            .hover(|s| s.bg(theme.colors.accent_surface.opacity(0.6)))
            .cursor_pointer()
            .child(Icon::new(IconName::CircleDot).size(IconSize::Custom(theme.rem(1.))).color(muted))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(theme.rem(0.375))
                            .child(div().flex_none().mono_text(theme.rem(0.75), weight::NORMAL).text_color(muted).child(SharedString::from(link.display_key.clone())))
                            .child(div().min_w_0().truncate().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(SharedString::from(link.title.clone()))),
                    )
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(theme.rem(0.375))
                            .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                            .text_color(muted)
                            .child(Icon::new(tracker_icon(link.tracker.as_str())).size(IconSize::Custom(theme.rem(0.75))).color(muted))
                            .child(div().min_w_0().truncate().child(SharedString::from(link.scope.clone())))
                            .child(div().ml_auto().flex_none().whitespace_nowrap().child(relative_label(&link.linked_at, now))),
                    ),
            )
            .tooltip(tooltip(t!("Issue {key}: {title}", key = link.display_key.as_str(), title = link.title.as_str())))
            .on_click(move |_, _, cx| match &surface {
                Some(surface) => panels.update(cx, |store, cx| {
                    store.open_surface(&thread, surface.clone());
                    cx.notify();
                }),
                None => cx.open_url(&url),
            })
            .into_any_element()
    }
}

/// `issueTrackerPresentation(tracker).Icon`.
fn tracker_icon(tracker: &str) -> crate::components::icons::IconSource {
    use crate::components::icons::BrandIcon;
    match tracker {
        "github" => BrandIcon::GitHub.into(),
        "gitlab" => BrandIcon::GitLab.into(),
        "azure-devops" => BrandIcon::AzureDevOps.into(),
        "forgejo" => BrandIcon::Forgejo.into(),
        "bitbucket" => BrandIcon::Bitbucket.into(),
        _ => IconName::CircleDot.into(),
    }
}

/// `sourceControlClients.hostLabelForChangeRequestUrl`.
fn host_label(url: &str) -> String {
    t3code_client_runtime::change_request_links::parse_change_request_url(url).map_or_else(|| "GitHub".to_owned(), |(kind, _)| kind.label().to_owned())
}

/// The link button that opens "Link pull request".
fn link_button(id: &'static str, size: ButtonSize, variant: ButtonVariant, label: SharedString) -> Button {
    Button::new(id).size(size).variant(variant).icon(IconName::Plus).label(label).on_click(|_, window, cx| window.dispatch_action(Box::new(OpenLinkPullRequestDialog), cx))
}

impl Render for ThreadPullRequestsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let (environment_id, _) = crate::state::shell::runtime_ids(&self.key);
        let environment = StateBridge::is_running(cx).then(|| StateBridge::environment(&environment_id, cx)).flatten();
        let config = environment.as_ref().and_then(|env| env.read(cx).config.clone());
        let Some(config) = config else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Empty::new(t!("Linked items unavailable")).description(t!("The thread's environment is not available.")).icon(IconName::GitPullRequest))
                .into_any_element();
        };
        let row = StateBridge::shell(cx).read(cx).row(&self.key).cloned();
        let thread = row.as_ref().map(|row| &row.source);
        let all_links = thread.and_then(|t| t.pull_requests.clone()).unwrap_or_default();
        let issues = thread.and_then(|t| t.issues.clone()).unwrap_or_default();
        let links = visible_thread_pull_requests(&all_links);
        let link_count = links.len();
        let open_count = links.iter().filter(|link| is_open(link)).count();
        let last_synced = links.iter().filter_map(|link| link.snapshot.as_ref().map(|s| s.synced_at.clone())).max();
        let now = t3code_client_runtime::state::models::now_ms();
        if link_count == 0 && issues.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(theme.rem(0.5))
                .p(theme.rem(1.5))
                .child(Icon::new(IconName::Link).size(IconSize::Custom(theme.rem(1.5))).color(theme.colors.muted_foreground.opacity(0.6)))
                .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(theme.colors.text).child(t!("No linked pull requests")))
                .child(
                    div()
                        .max_w(theme.rem(15.))
                        .text_center()
                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                        .text_color(theme.colors.muted_foreground)
                        .child(t!("Pull requests the agent opens from this thread land here. Link one yourself from a URL or a number.")),
                )
                .child(link_button("linked-empty-link", ButtonSize::Sm, ButtonVariant::Outline, t!("Link pull request")))
                .into_any_element();
        }
        let context = RowContext {
            supports_watch: config.environment.capabilities.thread_pull_request_watch == Some(true),
            can_start_watch: thread.is_some_and(|t| can_start_thread_pull_request_watch(t)),
            project_id: thread.map(|t| t.project_id.to_string()),
            opens_in_app: crate::hooks::use_settings::ClientSettingsStore::get(cx).pull_request_links_open_in_app,
        };
        let chains = resolve_thread_pull_request_chains(&all_links);
        let lines = pull_request_list_lines(&chains);
        let this = cx.entity();
        let issue_capable = config.environment.capabilities.issues == Some(true);
        let header = (!issues.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .child(div().px(theme.rem(0.5)).py(theme.rem(0.25)).ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.muted_foreground).child(t!("Issues")))
                .children(issues.iter().enumerate().map(|(i, link)| self.render_issue(i, link, issue_capable, now, &theme)))
                .when(link_count > 0, |el| {
                    el.child(
                        div()
                            .mt(theme.rem(0.5))
                            .border_t_1()
                            .border_color(theme.colors.border.opacity(0.6))
                            .px(theme.rem(0.75))
                            .pt(theme.rem(0.5))
                            .pb(theme.rem(0.25))
                            .ui_text(&theme, TextSize::Xs, weight::MEDIUM)
                            .text_color(theme.colors.muted_foreground)
                            .child(t!("Pull requests")),
                    )
                })
        });
        let rows: Vec<AnyElement> = lines.iter().enumerate().map(|(i, line)| Self::render_row(&this, i, line.link, line.depth, line.stack, &context, now, &theme)).collect();
        let summary = (link_count > 0).then(|| {
            t!(
                "{openCount, plural, one {{openCount} open} other {{openCount} open}} · {linkCount, plural, one {{linkCount} linked} other {{linkCount} linked}}",
                openCount = open_count,
                linkCount = link_count
            )
        });
        let synced = last_synced.map(|at| t!("synced {time}", context = "followed by a relative time such as 5m ago or just now", time = relative_label(&at, now).as_ref()));
        let footer_text: SharedString = match (summary, synced) {
            (Some(summary), Some(synced)) => format!("{summary} · {synced}").into(),
            (Some(summary), None) => summary,
            (None, Some(synced)) => format!(" · {synced}").into(),
            (None, None) => SharedString::default(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .id("linked-items-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(theme.rem(0.375))
                    .children(header)
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(theme.colors.border.opacity(0.6))
                    .px(theme.rem(0.5))
                    .py(theme.rem(0.375))
                    .ui_text(&theme, TextSize::Xs2, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .child(div().child(footer_text))
                    .child(link_button("linked-footer-link", ButtonSize::Xs, ButtonVariant::Ghost, t!("Link pull request", context = "verb, button"))),
            )
            .into_any_element()
    }
}

/// Builds the tab's view for a right panel.
pub fn thread_pull_requests_panel(key: ThreadKey, panels: Entity<RightPanelStore>, cx: &mut App) -> gpui_kit::AnyView {
    cx.new(|cx| ThreadPullRequestsPanel::new(key, panels, cx)).into()
}
