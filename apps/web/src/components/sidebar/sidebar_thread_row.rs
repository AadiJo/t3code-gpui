//! Thread rows. Ports `SidebarThreadRow.tsx`: the 78px card (pinned, active,
//! working), the 36px slim row (snoozed, settled) and the folder row.
//! Rows derive everything from the thread and the sidebar's hover, cursor and
//! selection, so hovering re-renders only the visible rows.

use crate::ActiveAppTheme as _;
use crate::components::project_favicon::ProjectFavicon;
use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::*;

use super::context_menu::MenuCommand;
use super::model::{PullRequestState, SidebarProvider, SidebarProviderMark, SidebarThread};
use super::style::*;
use super::{Sidebar, SidebarEvent};
use crate::components::sidebar_logic::*;
use crate::components::thread_status_indicators::{status_indicator, terminal_status_icon};

/// What hovering a slim row offers in its time slot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SlimAction {
    Settle,
    Unsettle,
    Wake,
}

/// Row facts shared by every variant.
struct RowState {
    status: ThreadStatus,
    is_unread: bool,
    is_woke: bool,
    active: bool,
    selected: bool,
    hovered: bool,
    focused: bool,
    recede: bool,
    top: Option<TopStatus>,
}

impl Sidebar {
    fn row_state(&self, ix: usize, thread: &SidebarThread, window: &Window) -> RowState {
        let status = resolve_row_status(thread);
        let is_unread = has_unseen_completion(thread);
        let is_woke = is_woke(thread);
        let active = self.is_active(&thread.key);
        let selected = self.selected.contains(&thread.key);
        RowState {
            status,
            is_unread,
            is_woke,
            active,
            selected,
            hovered: self.hovered == Some(ix),
            focused: self.keyboard_nav && self.cursor == Some(ix) && self.focus_handle.is_focused(window),
            recede: should_recede(status, is_unread, is_woke, active, selected),
            top: resolve_top_status(thread, status, is_unread, is_woke),
        }
    }

    /// One thread row of the given variant.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_thread_row(
        &mut self,
        ix: usize,
        thread: &SidebarThread,
        variant: RowVariant,
        section: SidebarSection,
        colors: &ShellColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.row_state(ix, thread, window);
        let surface = self.row_surface(ix, thread, &state, colors, cx);
        match variant {
            RowVariant::Card => div()
                .py(colors.s(CARD_ROW_PADDING_Y))
                .child(surface.child(self.card_body(ix, thread, &state, colors, cx)))
                .into_any_element(),
            RowVariant::Slim | RowVariant::Folder | RowVariant::FolderMultiline => {
                let action = match section {
                    SidebarSection::Settled => SlimAction::Unsettle,
                    SidebarSection::Snoozed => SlimAction::Wake,
                    _ => SlimAction::Settle,
                };
                surface
                    .child(self.slim_body(ix, thread, &state, variant, action, colors, cx))
                    .into_any_element()
            }
        }
    }

    /// The clickable row box: fill by state, focus ring, pointer handlers.
    fn row_surface(
        &self,
        ix: usize,
        thread: &SidebarThread,
        state: &RowState,
        colors: &ShellColors,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let fill = if state.active {
            Some(colors.sidebar_row_active)
        } else if state.selected {
            Some(colors.sidebar_row_selected)
        } else if thread.has_unsent_draft {
            Some(colors.warning.opacity(if state.hovered { 0.08 } else { 0.04 }))
        } else if state.hovered {
            Some(colors.sidebar_row_hover)
        } else {
            None
        };
        let light_ring = !colors.dark && (state.active || state.selected);
        let dim_working = state.recede
            && matches!(state.status, ThreadStatus::Working | ThreadStatus::Preparing)
            && !state.hovered;
        div()
            .id(("sidebar-row", ix))
            .relative()
            .w_full()
            .overflow_hidden()
            .rounded(colors.s(ROW_RADIUS))
            .cursor_pointer()
            .text_color(if state.recede && !state.hovered {
                colors.sidebar_muted_foreground.opacity(0.75)
            } else {
                colors.sidebar_foreground
            })
            .when_some(fill, |d, fill| d.bg(fill))
            .when(light_ring, |d| d.border_1().border_color(colors.sidebar_border.opacity(0.7)))
            .when(state.focused, |d| d.border_2().border_color(colors.ring))
            .when(dim_working, |d| d.opacity(0.7))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_row_hover(ix, *hovered, cx);
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                // A plain double-click renames (`handleDoubleClick`).
                if event.click_count() == 2 && event.modifiers() == Modifiers::default() {
                    if let Some(SidebarListEntry::Thread { thread, .. }) = this.list.entries.get(ix) {
                        let key = thread.key.clone();
                        this.start_rename(key, window, cx);
                    }
                    return;
                }
                if this.renaming.is_some() {
                    return;
                }
                window.focus(&this.focus_handle, cx);
                this.click_thread(ix, event.modifiers(), cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_entry_menu(ix, event.position, cx);
                }),
            )
            .when_some(self.drag_value(ix, colors), |d, value| {
                d.on_drag(value, super::sidebar_drag::drag_preview(cx.weak_entity()))
            })

            .when(self.hover_card == Some(ix), |d| {
                let card = crate::components::thread_hover_card::thread_hover_card(
                    thread,
                    self.list.project_labels.get(&thread.project_ref()),
                    self.snapshot.hover_details.get(&thread.key),
                    colors,
                    cx,
                );
                let surface = crate::components::thread_hover_card::hover_card_surface(card, colors, cx);
                // `side="right" align="start" sideOffset={4}`. The fixed-width flex
                // wrapper gives the card room to size to its content up to `max-w-80`.
                d.child(
                    deferred(div().absolute().left_full().top_0().pl(colors.r(4.)).w(colors.r(324.)).flex().items_start().child(surface))
                        .with_priority(1),
                )
            })
    }

    fn card_body(
        &self,
        ix: usize,
        thread: &SidebarThread,
        state: &RowState,
        colors: &ShellColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let has_meta = card_has_meta(thread);
        let project_title = self.list.project_titles.get(&thread.project_ref()).cloned().unwrap_or_default();
        let project_label = self
            .list
            .project_labels
            .get(&thread.project_ref())
            .cloned()
            .unwrap_or_default();
        let title_color = if state.recede && !state.hovered {
            colors.foreground.opacity(0.75)
        } else if state.status == ThreadStatus::Failed && !state.is_unread && !state.is_woke {
            colors.foreground.opacity(0.95)
        } else {
            colors.foreground
        };
        let line1 = div()
            .flex()
            .h(colors.r(20.))
            .min_w_0()
            .items_center()
            .gap(colors.r(6.))
            .when(thread.has_unsent_draft, |d| {
                d.child(Icon::new(IconName::SquarePen).size(IconSize::Custom(colors.r(12.))).color(colors.warning_foreground))
            })
            .child(project_favicon(project_title, self.list.project_icons.get(&thread.project_ref()), colors))
            .when(thread.pinned_at.is_some(), |d| d.child(self.pin_indicator(ix, thread, colors, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .line_height(colors.r(16.))
                    .text_color(colors.secondary_label)
                    .child(project_label),
            )
            .child(self.card_status_slot(ix, thread, state, colors, cx));
        let title = match self.rename_field(thread, colors, cx) {
            Some(field) => field,
            None => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .line_height(colors.r(20.))
                .font_weight(if state.recede {
                    FontWeight::NORMAL
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(title_color)
                .child(thread.title.clone())
                .into_any_element(),
        };
        let line2 = div()
            .mt_1()
            .flex()
            .min_w_0()
            .items_center()
            .gap(colors.r(6.))
            .child(title)
            .when(!has_meta && thread.remote_machine, |d| d.child(remote_icon(colors)))
            .when(!has_meta, |d| d.children(thread.provider.as_ref().map(|p| provider_stack(p, colors, cx))));
        div()
            .relative()
            .h(colors.s(if has_meta { CARD_ROW_HEIGHT } else { CARD_ROW_HEIGHT_NO_META }))
            .px(colors.s(ROW_CONTENT_INSET))
            .py(colors.s(CONTENT_INSET))
            .flex()
            .flex_col()
            .child(line1)
            .child(line2)
            .when(has_meta, |d| d.child(self.meta_line(thread, colors, cx)))
    }

    /// Status label or relative time; on hover the action cluster.
    fn card_status_slot(
        &self,
        ix: usize,
        thread: &SidebarThread,
        state: &RowState,
        colors: &ShellColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let settings = self.snapshot.settings;
        let slot = div()
            .ml_auto()
            .flex()
            .h(colors.r(20.))
            .min_w(colors.r(32.))
            .flex_none()
            .items_center()
            .justify_end()
            .text_xs()
            .line_height(colors.r(16.))
            .font_features(super::style::tabular_figures());
        let woke = state.top == Some(TopStatus::Woke);
        let label = match state.top {
            Some(top) => {
                let duration = match top {
                    TopStatus::Working { .. } | TopStatus::Waiting => {
                        working_started_at(thread, state.status)
                            .map(|start| working_duration_label(self.now.0 - start.0))
                    }
                    _ => None,
                };
                status_indicator(top, colors.r(16.), duration, colors).into_any_element()
            }
            None => div()
                .text_color(colors.secondary_label)
                .child(compact_relative_time(thread_activity_timestamp(thread), self.now))
                .into_any_element(),
        };
        if let Some(hint) = self.jump_label(&thread.key, cx) {
            return slot.child(crate::components::ui::kbd::Kbd::new(hint));
        }
        if !state.hovered {
            return slot.child(label);
        }
        let key = thread.key.clone();
        let mut cluster = div().flex().items_stretch().h_full();
        if thread.has_unsent_draft {
            let key = key.clone();
            cluster = cluster.child(
                row_action(("discard-draft", ix), colors)
                    .child(Icon::new(IconName::X).size(IconSize::Custom(colors.r(14.))).color(colors.muted_foreground))
                    .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!("Discard draft")))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        cx.emit(SidebarEvent::DiscardDraft(key.clone()));
                    })),
            );
        }
        if settings.snooze_supported && thread.can_snooze(self.snapshot.now) {
            cluster = cluster.child(
                row_action(("snooze", ix), colors)
                    .child(Icon::new(IconName::Clock).size(IconSize::Custom(colors.r(12.))).color(colors.muted_foreground))
                    .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!("Snooze thread")))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_snooze_menu(ix, event.position(), cx);
                    })),
            );
        }
        if settings.settlement_supported {
            cluster = cluster.child(
                row_action(("settle", ix), colors)
                    .mr(colors.r(-4.))
                    .gap_1()
                    .text_color(colors.muted_foreground)
                    .child(Icon::new(IconName::Check).size(IconSize::Custom(colors.r(14.))).color(colors.muted_foreground))
                    .child("Settle")
                    .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!("Settle thread")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let targets = this.command_targets(&key);
                        cx.emit(SidebarEvent::ThreadCommand(targets, MenuCommand::Settle));
                    })),
            );
        }
        slot.gap_1().when(woke, |d| d.child(label)).child(cluster)
    }

    /// Branch, terminals, PR, issue and diff (`SidebarThreadRow.tsx:1281-1322`).
    fn meta_line(&self, thread: &SidebarThread, colors: &ShellColors, cx: &App) -> Div {
        let mut line = div()
            .mt(colors.r(2.))
            .flex()
            .min_w_0()
            .items_center()
            .gap(colors.r(6.))
            .text_xs()
            .line_height(colors.r(16.))
            .text_color(colors.secondary_label);
        match &thread.branch {
            Some(branch) => {
                if thread.in_worktree {
                    line = line.child(Icon::new(IconName::FolderGit2).size(IconSize::Custom(colors.r(14.))).color(colors.sidebar_icon));
                }
                line = line.child(middle_truncate(branch));
            }
            None => line = line.child(div().flex_1()),
        }
        if thread.running_terminal_count > 0 {
            line = line.child(terminal_status_icon(Some(self.pulse), colors));
        }
        if let Some(pr) = &thread.pull_request {
            let color = match pr.state {
                PullRequestState::Open => colors.success_foreground,
                PullRequestState::Draft | PullRequestState::Closed => colors.muted_foreground,
                PullRequestState::Merged => colors.input_status,
            };
            line = line.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(IconName::GitPullRequest).size(IconSize::Custom(colors.r(12.))).color(color))
                    .child(format!("#{}", pr.number)),
            );
        }
        if let Some(issue) = &thread.issue_key {
            line = line.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(IconName::CircleDot).size(IconSize::Custom(colors.r(12.))).color(colors.secondary_label))
                    .child(div().max_w(colors.r(64.)).truncate().child(issue.clone())),
            );
        }
        if let Some(diff) = thread.diff {
            line = line.child(
                div()
                    .flex()
                    .flex_none()
                    .gap_1()
                    .font_family(crate::appearance_fonts::mono_font(FontWeight::NORMAL).family)
                    .child(div().text_color(colors.diff_addition).child(format!("+{}", diff.insertions)))
                    .child(div().text_color(colors.diff_deletion).child(format!("−{}", diff.deletions))),
            );
        }
        if thread.remote_machine || thread.provider.is_some() {
            line = line.child(
                div()
                    .ml_auto()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .when(thread.remote_machine, |d| d.child(remote_icon(colors)))
                    .children(thread.provider.as_ref().map(|p| provider_stack(p, colors, cx))),
            );
        }
        line
    }

    fn pin_indicator(
        &self,
        ix: usize,
        thread: &SidebarThread,
        colors: &ShellColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hovered = self.hovered_pin == Some(ix);
        let key = thread.key.clone();
        div()
            .id(("unpin", ix))
            .flex_none()
            .cursor_pointer()
            .child(if hovered {
                Icon::new(IconName::PinOff).size(IconSize::Custom(colors.r(12.))).color(colors.foreground)
            } else {
                Icon::new(IconName::Pin).size(IconSize::Custom(colors.r(12.))).color(colors.muted_foreground.opacity(0.65))
            })
            .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!("Unpin thread")))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hovered_pin = if *hovered { Some(ix) } else { None };
                cx.notify();
            }))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(SidebarEvent::ThreadCommand(vec![key.clone()], MenuCommand::Unpin));
            }))
    }

    #[allow(clippy::too_many_arguments)]
    fn slim_body(
        &self,
        ix: usize,
        thread: &SidebarThread,
        state: &RowState,
        variant: RowVariant,
        action: SlimAction,
        colors: &ShellColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let multiline = variant == RowVariant::FolderMultiline;
        let in_folder = variant == RowVariant::Folder || multiline;
        let title_color = if state.hovered {
            colors.foreground
        } else if action == SlimAction::Unsettle && !state.active {
            colors.secondary_label
        } else if state.recede {
            colors.secondary_label
        } else if state.active
            || state.is_woke
            || matches!(state.status, ThreadStatus::Input | ThreadStatus::Request)
        {
            colors.foreground
        } else if state.is_unread {
            colors.muted_foreground
        } else {
            colors.secondary_label.opacity(0.7)
        };
        let favicon_full = state.active || state.hovered;
        let title = match self.rename_field(thread, colors, cx) {
            Some(field) => field,
            None => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .line_height(colors.r(20.))
                .font_weight(if state.recede {
                    FontWeight::NORMAL
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(title_color)
                .child(thread.title.clone())
                .into_any_element(),
        };
        let has_meta = card_has_meta(thread);
        let line = div()
            .flex()
            .when(multiline, |d| d.h(colors.r(24.)).min_w_0())
            .when(!multiline, |d| d.h(colors.s(SLIM_ROW_HEIGHT)).px(colors.r(10.)))
            .items_center()
            .gap(colors.r(10.))
            .when(in_folder && !multiline, |d| d.pl(colors.r(36.)))
            .when(!in_folder, |d| {
                let title = self.list.project_titles.get(&thread.project_ref()).cloned().unwrap_or_default();
                let source = self.list.project_icons.get(&thread.project_ref());
                d.child(div().flex_none().opacity(if favicon_full { 1. } else { 0.4 }).child(project_favicon(title, source, colors)))
            })
            .when(thread.has_unsent_draft, |d| {
                d.child(Icon::new(IconName::SquarePen).size(IconSize::Custom(colors.r(12.))).color(colors.warning_foreground))
            })
            .child(title)
            .when(multiline && !has_meta, |d| {
                d.when(thread.remote_machine, |d| d.child(remote_icon(colors)))
                    .children(thread.provider.as_ref().map(|p| provider_stack(p, colors, cx)))
            })
            .when(thread.pinned_at.is_some(), |d| d.child(self.pin_indicator(ix, thread, colors, cx)))
            .when(thread.running_terminal_count > 0 && !multiline, |d| {
                d.child(terminal_status_icon(Some(self.pulse), colors))
            })
            .child(self.slim_slot(ix, thread, state, in_folder, action, colors, cx));
        if !multiline {
            return line;
        }
        // `flex flex-col justify-center px-2.5 py-1.5 ps-9`: the title line,
        // then the card's meta line.
        div()
            .flex()
            .flex_col()
            .justify_center()
            .px(colors.r(10.))
            .pl(colors.r(36.))
            .py(colors.r(6.))
            .child(line)
            .when(has_meta, |d| d.child(self.meta_line(thread, colors, cx)))
    }

    /// The rename field in place of the row's title while it renames
    /// (`ThreadRenameInput`: `-mx-1 rounded-sm bg-card px-1`, focused `ring-foreground`).
    fn rename_field(&self, thread: &SidebarThread, colors: &ShellColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let renaming = self.renaming.as_ref().filter(|r| r.key == thread.key)?;
        let theme = cx.app_theme();
        Some(
            div()
                .id("sidebar-rename")
                .flex_1()
                .min_w_0()
                .mx(colors.r(-4.))
                .px(colors.r(4.))
                .rounded(colors.r(4.))
                .bg(theme.colors.card())
                .border_1()
                // The field holds focus while it exists; blurring commits it.
                .border_color(colors.foreground)
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.foreground)
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.cancel_rename(window, cx);
                    }
                }))
                .on_click(|_, _, cx| cx.stop_propagation())
                // The web's ring is inset; the 18.75px (`h-5`) box holds the border too.
                .h(colors.r(20.))
                .overflow_hidden()
                .flex()
                .items_center()
                .child(
                    gpui_kit::component::input::Input::new(&renaming.field)
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .xsmall()
                        .p_0()
                        .text_sm(),
                )
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn slim_slot(
        &self,
        ix: usize,
        thread: &SidebarThread,
        state: &RowState,
        in_folder: bool,
        action: SlimAction,
        colors: &ShellColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let slot = div()
            .ml_auto()
            .flex()
            .flex_none()
            .h(colors.r(24.))
            .min_w(colors.r(32.))
            .items_center()
            .justify_end()
            .text_xs()
            .line_height(colors.r(16.))
            .font_features(super::style::tabular_figures());
        let content = if action == SlimAction::Wake
            && let Some(until) = thread.snoozed_until
        {
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_color(colors.info_foreground)
                .child(Icon::new(IconName::AlarmClock).size(IconSize::Custom(colors.r(12.))).color(colors.info_foreground))
                .child(compact_relative_time(self.now, until))
        } else if state.is_woke || (in_folder && state.top.is_some()) {
            let top = state.top.unwrap_or(TopStatus::Woke);
            status_indicator(top, colors.r(12.), None, colors)
        } else {
            let stamp = if action == SlimAction::Unsettle {
                settled_timestamp(thread)
            } else {
                thread_activity_timestamp(thread)
            };
            div()
                .text_color(colors.secondary_label)
                .child(compact_relative_time(stamp, self.now))
        };
        if let Some(hint) = self.jump_label(&thread.key, cx) {
            return slot.child(crate::components::ui::kbd::Kbd::new(hint));
        }
        if !state.hovered {
            return slot.child(content);
        }
        let settings = self.snapshot.settings;
        let supported = match action {
            SlimAction::Wake => settings.snooze_supported,
            _ => settings.settlement_supported,
        };
        if !supported {
            return slot.child(content);
        }
        let (glyph, size, label, command) = match action {
            SlimAction::Settle => (IconName::Check, 12., "Settle thread", MenuCommand::Settle),
            SlimAction::Unsettle => (IconName::Undo2, 14., "Un-settle thread", MenuCommand::Unsettle),
            SlimAction::Wake => (IconName::AlarmClockOff, 12., "Wake thread now", MenuCommand::Wake),
        };
        let key = thread.key.clone();
        slot.gap_1().when(state.is_woke, |d| d.child(content)).child(
            row_action(("slim-action", ix), colors)
                .when(action == SlimAction::Settle, |d| d.px_2().mr(colors.r(-8.)))
                .when(action != SlimAction::Settle, |d| d.mr(colors.r(-4.)))
                .child(Icon::new(glyph).size(IconSize::Custom(px(size))).color(colors.muted_foreground))
                .tooltip(crate::components::ui::tooltip::tooltip(label))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let targets = this.command_targets(&key);
                    cx.emit(SidebarEvent::ThreadCommand(targets, command));
                })),
        )
    }
}

/// A row action button: `px-1.5 text-xs text-muted-foreground hover:text-foreground`.
fn row_action(id: impl Into<ElementId>, colors: &ShellColors) -> Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .flex()
        .items_center()
        .h_full()
        .px(colors.r(6.))
        .rounded(colors.r(6.))
        .cursor_pointer()
        .text_xs()
        .text_color(colors.muted_foreground)
        .hover(|s| s.text_color(colors.foreground))
}

/// The `size-4` project favicon: the project's icon or favicon, else the monogram of its name.
pub(crate) fn project_favicon(
    name: SharedString,
    source: Option<&crate::components::project_favicon::ProjectIconSource>,
    colors: &ShellColors,
) -> ProjectFavicon {
    ProjectFavicon::new(name).source(source).size(colors.r(16.))
}

/// A provider instance's mark at `size`, without opacity or badge.
pub(crate) fn provider_mark(provider: &SidebarProvider, size: Pixels, colors: &ShellColors, cx: &App) -> AnyElement {
    match provider.mark {
        SidebarProviderMark::Brand(mark) => {
            let composer = crate::components::chat::composer_theme::ComposerColors::new(cx);
            crate::components::chat::composer_icons::provider_icon(mark, size, &provider.initials, &composer, cx)
        }
        SidebarProviderMark::AcpRegistry => Icon::new(crate::components::icons::BrandIcon::AcpRegistry)
            .size(IconSize::Custom(size))
            .color(colors.muted_foreground)
            .into_any_element(),
    }
}

/// `SidebarProviderStack` for one instance: the compact (14px) provider mark
/// at 60% opacity, with the instance-initials badge hanging off its corner.
pub(crate) fn provider_stack(provider: &SidebarProvider, colors: &ShellColors, cx: &App) -> Div {
    let size = colors.r(14.);
    let mark = provider_mark(provider, size, colors, cx);
    let card = cx.app_theme().colors.card();
    div()
        .relative()
        .flex()
        .flex_none()
        .size(size)
        .items_center()
        .justify_center()
        .child(div().opacity(0.6).child(mark))
        .when(provider.show_badge, |d| {
            d.child(
                div()
                    .absolute()
                    .right(colors.r(-3.))
                    .bottom(colors.r(-3.))
                    .flex()
                    .h(colors.r(12.))
                    .min_w(colors.r(12.))
                    .px(colors.r(2.))
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(card)
                    .bg(card)
                    .shadow(cx.app_theme().shadows.sm5.clone())
                    .text_size(colors.r(7.))
                    .line_height(colors.r(7.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.muted_foreground)
                    .child(provider.initials.clone()),
            )
        })
}

fn remote_icon(colors: &ShellColors) -> Icon {
    Icon::new(IconName::Monitor).size(IconSize::Custom(colors.r(14.))).color(colors.sidebar_muted_foreground.opacity(0.7))
}

/// `MiddleTruncate`: the head ellipsizes, the tail stays whole.
pub(crate) fn middle_truncate(value: &SharedString) -> Div {
    let row = div().flex().flex_1().min_w_0().overflow_hidden().whitespace_nowrap();
    match split_middle_truncate(value) {
        Some((head, tail)) => row
            .child(div().min_w_0().flex_shrink(1.).truncate().child(head))
            .child(div().flex_none().child(tail)),
        None => row.child(div().min_w_0().truncate().child(value.clone())),
    }
}

