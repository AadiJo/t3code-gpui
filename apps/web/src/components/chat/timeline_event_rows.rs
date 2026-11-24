//! Lifecycle rows: system dividers (`TimelineSystemDivider.tsx`), subagent
//! cards and groups (`V2LifecycleRow.tsx`, `TimelineV2EventRow.tsx`), pending
//! approvals (`TimelinePendingApprovalRow.tsx`), queued messages
//! (`QueuedRunsControl.tsx`), proposed plans (`ProposedPlanCard.tsx`), the
//! worktree setup card, and stand-ins for HTML renders and MCP app frames.

use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::chat::chat_style::chat_icon_stroke;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, linear_color_stop, linear_gradient, px,
};

use super::{
    chat_style::{ChatColors, TEXT_2XS, TEXT_3XS, TEXT_SM, TEXT_XS, alpha, mono},
    messages_timeline::{MessagesTimeline, TimelineAction},
    messages_timeline_logic::TimelineRow,
    timeline_entries::{
        ApprovalKind, Divider, ProposedPlan, QueuedMessage, Subagent, SubagentStatus, TimelineEvent, ToolStatus, WorkItem,
        WorkLogEntry, WorktreeSetup,
    },
    timeline_row_parts::{SMALL_ICON, tool_icon, work_log_line, xs_button},
    work_log_presentation::{format_clock, format_duration},
};
use crate::{
    components::chat_markdown::{MarkdownView, render_markdown},
    markdown_pipeline::MarkdownFlavor,
};

/// Subagent avatar `size-6` with a `size-2` status dot.
const AVATAR: gpui_kit::Pixels = px(24.);
const STATUS_DOT: gpui_kit::Pixels = px(8.);
/// Plan card body clips at `max-h-104` with a 6rem fade.
const PLAN_CLIP: gpui_kit::Pixels = px(416.);
const PLAN_FADE: gpui_kit::Pixels = px(96.);

impl MessagesTimeline {
    pub(super) fn render_event(
        &mut self,
        row: &TimelineRow,
        event: &TimelineEvent,
        subagents: &[Subagent],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match event {
            TimelineEvent::Divider(divider) => self.render_divider(divider),
            TimelineEvent::Subagent(_) if subagents.len() > 1 => self.render_subagent_group(row, subagents, cx),
            TimelineEvent::Subagent(first) => self.render_subagent(first, row.key),
        }
    }

    /// A rule-pill-rule divider; with an action the pill becomes a bordered button.
    fn render_divider(&self, divider: &Divider) -> AnyElement {
        let colors = self.colors;
        let rule = || div().h(px(1.)).flex_1().bg(alpha(colors.theme.border, 0.7));
        let glyph = match divider.kind {
            super::timeline_entries::DividerKind::RunInterrupted => IconName::CircleAlert,
            super::timeline_entries::DividerKind::Forked => IconName::GitFork,
            super::timeline_entries::DividerKind::ParentLink => IconName::Link,
            super::timeline_entries::DividerKind::SideChatParent => IconName::MessagesSquare,
            super::timeline_entries::DividerKind::NewSession => IconName::RotateCcw,
            super::timeline_entries::DividerKind::Handoff => IconName::ArrowUp,
        };
        let mut pill = div()
            .flex()
            .items_center()
            .gap_1()
            .max_w(px(320.))
            .rounded_full()
            .px_2()
            .py_1()
            .child(Icon::new(glyph).size(IconSize::Custom(SMALL_ICON)).color(colors.theme.muted_foreground).stroke(chat_icon_stroke(glyph)))
            .child(div().font_weight(FontWeight::MEDIUM).child(divider.label.clone()))
            .children(divider.detail.as_ref().map(|detail| div().opacity(0.7).truncate().child(format!("· {detail}"))));
        if divider.action {
            pill = pill
                .border_1()
                .border_color(alpha(colors.theme.border, 0.7))
                .bg(colors.theme.canvas)
                .px(px(10.));
        }
        div()
            .flex()
            .items_center()
            .gap_2()
            .py_2()
            .text_size(TEXT_2XS.size)
            .line_height(TEXT_2XS.line_height)
            .text_color(colors.theme.muted_foreground)
            .child(rule())
            .child(pill)
            .child(rule())
            .into_any_element()
    }

    fn render_subagent(&self, subagent: &Subagent, key: u64) -> AnyElement {
        let colors = self.colors;
        let failed = subagent.status == SubagentStatus::Failed;
        div()
            .id(("subagent", key))
            .flex()
            .w_full()
            .items_center()
            .gap(px(10.))
            .rounded(px(6.))
            .px_2()
            .py(px(6.))
            .hover(|s| s.bg(alpha(colors.theme.accent_surface, 0.2)))
            .child(avatar(subagent, &colors))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(TEXT_XS.size)
                            .line_height(TEXT_XS.line_height)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.theme.text)
                            .truncate()
                            .child(subagent.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(TEXT_2XS.size)
                            .line_height(px(17.875))
                            .text_color(if failed { colors.destructive() } else { colors.theme.muted_foreground })
                            .truncate()
                            .child(subagent.detail.clone().unwrap_or_else(|| status_label(subagent.status).into())),
                    ),
            )
            .children(subagent.elapsed_ms.map(|ms| {
                div().text_size(TEXT_XS.size).text_color(colors.theme.muted_foreground).child(format_duration(ms))
            }))
            .child(Icon::new(IconName::ChevronRight).size(IconSize::Custom(px(14.))).color(alpha(colors.theme.muted_foreground, 0.6)))
            .into_any_element()
    }

    /// Several subagents from one turn: overlapping avatars, count and status.
    fn render_subagent_group(&mut self, row: &TimelineRow, subagents: &[Subagent], cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors;
        let active = subagents.iter().filter(|s| s.status.is_live()).count();
        let failed = subagents.iter().filter(|s| s.status == SubagentStatus::Failed).count();
        let summary = if active > 0 {
            format!("{active} running")
        } else if failed > 0 {
            format!("{failed} failed")
        } else {
            "All done".to_owned()
        };
        let summary_color = if active > 0 { colors.info } else if failed > 0 { colors.destructive() } else { colors.theme.muted_foreground };
        let expanded = self.expansion.work_groups.contains(&row.id);
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .py_2()
            .pr(px(13.))
            .opacity(if active == 0 && !expanded { 0.55 } else { 1. })
            .child(
                div().flex().children(subagents.iter().take(3).enumerate().map(|(i, s)| {
                    div().ml(if i == 0 { px(0.) } else { px(-6.) }).child(avatar(s, &colors))
                })).children((subagents.len() > 3).then(|| {
                    div()
                        .ml(px(-6.))
                        .size(AVATAR)
                        .rounded_full()
                        .bg(colors.theme.muted)
                        .border_1()
                        .border_color(alpha(colors.theme.border, 0.7))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(TEXT_3XS.size)
                        .child(format!("+{}", subagents.len() - 3))
                })),
            )
            .child(
                div()
                    .flex_1()
                    .child(
                        div()
                            .text_size(TEXT_XS.size)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.theme.text)
                            .child(format!("{} subagents", subagents.len())),
                    )
                    .child(div().text_size(TEXT_3XS.size).text_color(summary_color).child(summary)),
            )
            .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Custom(px(14.))).color(colors.theme.muted_foreground).stroke(chat_icon_stroke(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })));
        let id = row.id.clone();
        let mut column = div()
            .id(("subagent-group", row.key))
            .cursor_pointer()
            .child(header)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_work_group(id.clone(), cx)));
        if expanded {
            column = column.child(
                div()
                    .mt_1()
                    .mb_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(alpha(colors.theme.border, 0.6))
                    .bg(alpha(colors.theme.surface, 0.3))
                    .p_1()
                    .children(subagents.iter().enumerate().map(|(i, s)| self.render_subagent(s, row.key.wrapping_add(i as u64 + 1)))),
            );
        }
        column.into_any_element()
    }

    /// An approval the agent waits on, inline in place of its tool row.
    pub(super) fn render_pending_approval(&mut self, entry: &WorkLogEntry, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors;
        let WorkItem::Approval { kind, detail, reason, request_id } = &entry.item else {
            return div().into_any_element();
        };
        let glyph = match kind {
            ApprovalKind::Command => IconName::Terminal,
            ApprovalKind::FileChange => IconName::SquarePen,
            ApprovalKind::FileRead => IconName::Eye,
            ApprovalKind::Permission => IconName::Wrench,
            ApprovalKind::AppAccess => IconName::Hammer,
        };
        let key = MessagesTimeline::key_of(entry.id.as_str());
        let text_button = |id: &'static str, label: &'static str, color: Hsla| {
            div()
                .id((id, key))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .hover(|s| s.underline())
                .child(label)
        };
        let code = div()
            .id(("approval-detail", key))
            .max_h(px(160.))
            .overflow_y_scroll()
            .font_family(mono().family)
            .text_size(TEXT_XS.size)
            .line_height(px(20.))
            .text_color(colors.theme.text)
            .child(detail.clone());
        div()
            .w_full()
            .child(work_log_line(Some(tool_icon(glyph, &colors, false)), code.into_any_element(), Vec::new(), &colors))
            .child(
                div()
                    .ml(px(30.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pb_1()
                    .text_size(TEXT_XS.size)
                    .line_height(TEXT_XS.line_height)
                    .children(reason.as_ref().map(|r| div().text_color(colors.theme.muted_foreground).child(r.clone())))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(Icon::new(IconName::Shield).size(IconSize::Custom(SMALL_ICON)).color(colors.theme.warning))
                            .child(div().text_color(colors.theme.warning).child("Waiting for approval"))
                            .child(div().text_color(colors.theme.muted_foreground).child("·"))
                            .child(text_button("decline", "Decline", colors.theme.muted_foreground).on_click({
                                let request = request_id.clone();
                                cx.listener(move |_, _, _, cx| {
                                    cx.emit(TimelineAction::RespondToApproval { request: request.clone(), approve: false })
                                })
                            }).test_support())
                            .child(text_button("approve", "Approve", colors.theme.text).on_click({
                                let request = request_id.clone();
                                cx.listener(move |_, _, _, cx| {
                                    cx.emit(TimelineAction::RespondToApproval { request: request.clone(), approve: true })
                                })
                            }).test_support())
                            .child(text_button("more", "More", colors.theme.muted_foreground)),
                    ),
            )
            .into_any_element()
    }

    /// Queued messages as dashed bubbles with their controls.
    pub(super) fn render_queued(&mut self, messages: &[QueuedMessage], cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors;
        let weak = cx.weak_entity();
        div()
            .flex()
            .flex_col()
            .items_end()
            .gap_3()
            .pb_4()
            .children(messages.iter().enumerate().map(|(index, queued)| {
                let key = MessagesTimeline::key_of(&queued.id);
                let emit = |action: TimelineAction| {
                    let weak = weak.clone();
                    move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut gpui_kit::App| {
                        weak.update(cx, |_, cx| cx.emit(action.clone())).ok();
                    }
                };
                let (border, ink) = if queued.editing {
                    (alpha(colors.theme.message_action, 0.6), colors.theme.text)
                } else if queued.guest {
                    (alpha(colors.guest, 0.5), colors.theme.muted_foreground)
                } else {
                    (alpha(colors.theme.muted_foreground, 0.4), colors.theme.muted_foreground)
                };
                let group = format!("queued-{index}");
                div()
                    .group(group.clone())
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_1()
                    .w_full()
                    .child(
                        div()
                            .max_w(gpui_kit::relative(0.8))
                            .rounded(px(16.))
                            .border_1()
                            .border_dashed()
                            .border_color(border)
                            .px_3()
                            .py_2()
                            .text_size(TEXT_SM.size)
                            .line_height(TEXT_SM.line_height)
                            .text_color(ink)
                            .child(queued.text.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .pr_1()
                            .text_size(TEXT_XS.size)
                            .text_color(colors.theme.muted_foreground)
                            .child(div().opacity(0.).group_hover(group, |s| s.opacity(1.)).mr_1().child(format_clock(queued.created_at.0)))
                            .children(if queued.editing {
                                vec![
                                    div().child("Editing in the composer").into_any_element(),
                                    xs_button(("queued-cancel", key), None, Some("Cancel".into()), colors.theme.muted_foreground, &colors)
                                        .on_click(emit(TimelineAction::RemoveQueued(queued.id.clone())))
                                        .test_support()
                                        .into_any_element(),
                                ]
                            } else {
                                vec![
                                    xs_button(("queued-send", key), Some(IconName::ArrowUp), Some("Send now".into()), colors.theme.muted_foreground, &colors)
                                        .on_click(emit(TimelineAction::SendQueuedNow(queued.id.clone())))
                                        .test_support()
                                        .into_any_element(),
                                    xs_button(("queued-edit", key), Some(IconName::Pencil), None, colors.theme.muted_foreground, &colors)
                                        .on_click(emit(TimelineAction::EditQueued(queued.id.clone())))
                                        .test_support()
                                        .into_any_element(),
                                    xs_button(("queued-remove", key), Some(IconName::X), None, colors.theme.muted_foreground, &colors)
                                        .on_click(emit(TimelineAction::RemoveQueued(queued.id.clone())))
                                        .test_support()
                                        .into_any_element(),
                                ]
                            }),
                    )
            }))
            .into_any_element()
    }

    /// The plan card: title, markdown clipped at 416px with a fade.
    pub(super) fn render_proposed_plan(
        &mut self,
        row: &TimelineRow,
        plan: &ProposedPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let actions = self.markdown_actions();
        let parsed = self.message_markdown(&row.id, &plan.markdown, false, MarkdownFlavor::ASSISTANT);
        let pieces = parsed.pieces.clone();
        let view = MarkdownView {
            key: row.key,
            pieces: &pieces,
            colors: &colors,
            user: false,
            streaming: false,
            piece_opacity: None,
            state: &self.markdown_state,
            actions,
            find: self.find_highlight(),
        };
        let card = alpha(colors.theme.surface, 0.7);
        div()
            .rounded(px(16.))
            .border_1()
            .border_color(alpha(colors.theme.border, 0.8))
            .bg(card)
            .p_5()
            .child(
                div()
                    .mb_3()
                    .text_size(TEXT_SM.size)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.theme.text)
                    .child(plan.title.clone()),
            )
            .child(
                div()
                    .relative()
                    .max_h(PLAN_CLIP)
                    .overflow_hidden()
                    .child(render_markdown(&view, window, cx))
                    .child(div().absolute().left_0().right_0().bottom_0().h(PLAN_FADE).bg(linear_gradient(
                        180.,
                        linear_color_stop(alpha(colors.theme.surface, 0.), 0.),
                        linear_color_stop(alpha(colors.theme.surface, 0.95), 1.),
                    ))),
            )
            .into_any_element()
    }

    /// Worktree setup steps before the agent starts.
    pub(super) fn render_worktree_setup(&mut self, setup: &WorktreeSetup) -> AnyElement {
        let colors = self.colors;
        div()
            .rounded(px(8.))
            .border_1()
            .border_color(alpha(colors.theme.border, 0.6))
            .px_3()
            .py_2()
            .text_size(TEXT_XS.size)
            .line_height(px(20.))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.theme.text)
                    .child(format!("Setting up worktree on {}", setup.branch)),
            )
            .children(setup.steps.iter().map(|(label, status)| {
                let (mark, color) = match status {
                    ToolStatus::Completed => ("✓", colors.success_foreground),
                    ToolStatus::InProgress => ("◉", colors.info_foreground),
                    ToolStatus::Failed => ("✕", colors.destructive()),
                    _ => ("○", colors.theme.muted_foreground),
                };
                div()
                    .flex()
                    .gap_2()
                    .text_color(colors.theme.muted_foreground)
                    .child(div().text_color(color).child(mark))
                    .child(label.clone())
            }))
            .into_any_element()
    }

    /// HTML renders and MCP apps are sandboxed web frames; the native client
    /// shows a card that opens them in the browser.
    pub(super) fn render_embedded_frame(&mut self, kind: &'static str, title: &gpui_kit::SharedString) -> AnyElement {
        let colors = self.colors;
        div()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(12.))
            .border_1()
            .border_color(alpha(colors.theme.border, 0.8))
            .bg(alpha(colors.theme.surface, 0.5))
            .px_3()
            .py(px(10.))
            .text_size(TEXT_SM.size)
            .child(Icon::new(IconName::CodeXml).size(IconSize::Custom(px(16.))).color(colors.theme.muted_foreground).stroke(2.))
            .child(div().text_color(colors.theme.muted_foreground).child(kind))
            .child(div().flex_1().truncate().font_weight(FontWeight::MEDIUM).text_color(colors.theme.text).child(title.clone()))
            .child(Icon::new(IconName::ExternalLink).size(IconSize::Custom(px(14.))).color(colors.theme.muted_foreground))
            .into_any_element()
    }
}

fn status_label(status: SubagentStatus) -> &'static str {
    match status {
        SubagentStatus::Queued => "Queued",
        SubagentStatus::Running => "Running",
        SubagentStatus::Waiting => "Waiting",
        SubagentStatus::Completed => "Completed",
        SubagentStatus::Failed => "Failed",
        SubagentStatus::Idle => "Idle",
        SubagentStatus::Stopped => "Stopped",
    }
}

/// The subagent avatar: a muted circle with the model's initial and a status dot.
fn avatar(subagent: &Subagent, colors: &ChatColors) -> AnyElement {
    let dot = match subagent.status {
        SubagentStatus::Running | SubagentStatus::Queued | SubagentStatus::Waiting => colors.info,
        SubagentStatus::Completed => colors.success,
        SubagentStatus::Failed => colors.destructive(),
        SubagentStatus::Idle | SubagentStatus::Stopped => colors.theme.muted_foreground,
    };
    let initial: String = subagent.model.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    div()
        .relative()
        .size(AVATAR)
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(alpha(colors.theme.border, 0.7))
        .bg(colors.theme.muted)
        .flex()
        .items_center()
        .justify_center()
        .text_size(TEXT_3XS.size)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.theme.text)
        .child(initial)
        .child(
            div()
                .absolute()
                .right(px(-1.))
                .bottom(px(-1.))
                .size(STATUS_DOT)
                .rounded_full()
                .border_1()
                .border_color(colors.theme.canvas)
                .bg(dot),
        )
        .into_any_element()
}
