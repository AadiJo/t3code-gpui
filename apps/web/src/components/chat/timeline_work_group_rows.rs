//! Tool groups and live activity. Ports `TimelineWorkGroupRows.tsx` and the
//! fold rows of `MessagesTimeline.tsx`: the group header, the expanded group
//! as a nested virtual list, the live tool row, "Thinking", "Working for …",
//! turn and attempt folds, compaction markers and viewed images.

use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::chat::chat_style::chat_icon_stroke;
use gpui_kit::{rems, 
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, img, px,
};

use super::{
    chat_style::{TEXT_2XS, TEXT_SM_RELAXED, TEXT_XS, alpha},
    messages_timeline::MessagesTimeline,
    messages_timeline_logic::{RowKind, TimelineRow},
    timeline_entries::{AttemptId, RunId, Timestamp, WorkItem, WorkLogEntry},
    timeline_row_parts::{
        ROW_GROUP, disclosure, hover_timestamp, shimmer_label, tool_icon, work_log_line, work_log_row,
    },
    virtual_list::{VirtualListState, RowSlot, virtual_list},
    work_log_presentation::{entry_icon, format_clock, format_duration, live_entry_label},
};

/// An expanded group's list is capped at `min(18rem, 50dvh)` plus room for open entries.
const GROUP_LIST_MAX: gpui_kit::Pixels = px(288.);
const OPEN_ENTRY_ALLOWANCE: gpui_kit::Pixels = px(240.);
/// Nested rows estimate (`estimatedItemSize 24`).
const GROUP_ROW_ESTIMATE: gpui_kit::Pixels = px(24.);
/// Viewed image thumbnails are at most 10rem tall.
const VIEWED_IMAGE_MAX: gpui_kit::Rems = gpui_kit::rems(10.0);

impl MessagesTimeline {
    /// The header of a folded tool group: summary icon, summary, hover timestamp.
    pub(super) fn render_work_toggle(&mut self, row: &TimelineRow, cx: &mut Context<Self>) -> AnyElement {
        let RowKind::WorkToggle { group_id, summary, summary_kind, has_failure, .. } = &row.kind else {
            return div().into_any_element();
        };
        let colors = self.colors;
        let group = group_id.clone();
        work_log_row(("work-toggle", row.key), false)
            .cursor_pointer()
            .child(work_log_line(
                Some(tool_icon(summary_kind.icon(), &colors, *has_failure)),
                summary.clone().into_any_element(),
                Vec::new(),
                &colors,
            ))
            .children(hover_timestamp(row.created_at.map(|t| t.0), &colors))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_work_group(group.clone(), cx)))
            .into_any_element()
    }

    /// The live tool row of the running turn, with a thought preview above it.
    pub(super) fn render_work_live(&mut self, row: &TimelineRow, cx: &mut Context<Self>) -> AnyElement {
        let RowKind::WorkLive { entry, group_id, active, thought, .. } = &row.kind else {
            return div().into_any_element();
        };
        let colors = self.colors;
        let label = live_entry_label(entry, *active);
        let label = if *active {
            shimmer_label(label, self.animation_step, colors.theme.secondary_label, colors.theme.text)
        } else {
            label.into_any_element()
        };
        let glyph = Icon::new(entry_icon(entry)).size(IconSize::Custom(px(16.))).color(if *active { colors.theme.text } else { colors.theme.icon_muted }).stroke(chat_icon_stroke(entry_icon(entry)));
        let preview = thought.as_ref().and_then(|t| match &t.item {
            WorkItem::Reasoning { text } => Some(
                div()
                    .ml(rems(1.875))
                    .mb_1()
                    .line_clamp(4)
                    .text_size(TEXT_SM_RELAXED.size)
                    .line_height(TEXT_SM_RELAXED.line_height)
                    .text_color(colors.theme.muted_foreground)
                    .child(text.clone()),
            ),
            _ => None,
        });
        let group = group_id.clone();
        div()
            .w_full()
            .children(preview)
            .child(
                work_log_row(("work-live", row.key), false)
                    .cursor_pointer()
                    .child(work_log_line(Some(glyph.into_any_element()), label, Vec::new(), &colors))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_work_group(group.clone(), cx))),
            )
            .into_any_element()
    }

    /// "Thinking" while the agent has not started a tool yet.
    pub(super) fn render_thinking(&mut self, label: Option<SharedString>) -> AnyElement {
        let colors = self.colors;
        let label = label.unwrap_or_else(|| "Thinking".into());
        work_log_row("thinking", false)
            .child(work_log_line(
                Some(tool_icon(IconName::Brain, &colors, false)),
                shimmer_label(label, self.animation_step, colors.theme.secondary_label, colors.theme.text),
                Vec::new(),
                &colors,
            ))
            .into_any_element()
    }

    /// "Working for 1m 23s" under the running turn's prompt.
    pub(super) fn render_working(&mut self, started_at: Option<Timestamp>, compacting: bool) -> AnyElement {
        let colors = self.colors;
        let elapsed = started_at.map(|t| format_duration((self.now_millis - t.0).max(0)));
        let label: SharedString = match (compacting, elapsed) {
            (true, Some(e)) => format!("Compacting for {e}").into(),
            (true, None) => "Compacting".into(),
            (false, Some(e)) => format!("Working for {e}").into(),
            (false, None) => "Working".into(),
        };
        div()
            .border_b_1()
            .border_color(alpha(colors.theme.border, 0.6))
            .pb_2()
            .pt_1()
            .child(
                div()
                    .flex()
                    .h(rems(1.5))
                    .items_center()
                    .gap_2()
                    .px_1()
                    .text_size(TEXT_SM_RELAXED.size)
                    .line_height(TEXT_SM_RELAXED.line_height)
                    .text_color(colors.theme.muted_foreground)
                    .children(compacting.then(|| Icon::new(IconName::Minimize2).size(IconSize::Xs).color(colors.theme.muted_foreground)))
                    .child(label),
            )
            .into_any_element()
    }

    /// "Worked for 2m 10s", folding a finished turn's work.
    pub(super) fn render_turn_fold(
        &mut self,
        row: &TimelineRow,
        run_id: &RunId,
        label: &SharedString,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let run = run_id.clone();
        div()
            .border_b_1()
            .border_color(alpha(colors.theme.border, 0.6))
            .pb_2()
            .pt_1()
            .child(
                div()
                    .id(("turn-fold", row.key))
                    .group(ROW_GROUP)
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded(rems(0.375))
                    .px_1()
                    .cursor_pointer()
                    .text_size(TEXT_SM_RELAXED.size)
                    .line_height(TEXT_SM_RELAXED.line_height)
                    .text_color(colors.theme.muted_foreground)
                    .hover(|s| s.text_color(colors.theme.text))
                    .child(label.clone())
                    .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Custom(px(14.))).color(colors.theme.muted_foreground).stroke(chat_icon_stroke(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })))
                    .child(
                        div()
                            .ml_auto()
                            .text_size(TEXT_XS.size)
                            .text_color(colors.theme.muted_foreground)
                            .opacity(0.)
                            .group_hover(ROW_GROUP, |s| s.opacity(1.))
                            .children(row.created_at.map(|t| format_clock(t.0))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_run(run.clone(), cx))),
            )
            .into_any_element()
    }

    /// "Superseded attempt": work a steer cut off.
    pub(super) fn render_attempt_fold(
        &mut self,
        attempt_id: &AttemptId,
        label: &SharedString,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let attempt = attempt_id.clone();
        div()
            .id(("attempt-fold", MessagesTimeline::key_of(attempt_id.as_str())))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .rounded(rems(0.375))
            .border_1()
            .border_color(alpha(colors.theme.border, 0.6))
            .bg(alpha(colors.theme.muted, 0.2))
            .hover(|s| s.bg(alpha(colors.theme.muted, 0.35)))
            .px(rems(0.625))
            .py_2()
            .cursor_pointer()
            .child(disclosure(expanded, IconSize::Xs, colors.theme.muted_foreground))
            .child(
                div()
                    .text_size(TEXT_XS.size)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(alpha(colors.theme.text, 0.8))
                    .child(label.clone()),
            )
            .child(
                div()
                    .text_size(TEXT_2XS.size)
                    .text_color(colors.theme.muted_foreground)
                    .child("Cut off by a steer"),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_attempt(attempt.clone(), cx)))
            .into_any_element()
    }

    /// "Context compacted 182k → 24k tokens" between two rules.
    pub(super) fn render_compaction(&mut self, label: &SharedString, tokens: Option<(u64, u64)>, active: bool, failed: bool) -> AnyElement {
        let colors = self.colors;
        let (tone, rule) = if failed {
            (colors.destructive(), alpha(colors.destructive(), 0.3))
        } else {
            (colors.theme.muted_foreground, alpha(colors.theme.border, 0.7))
        };
        let text: SharedString = match tokens {
            Some((before, after)) if !active => format!("{label} {} → {} tokens", compact_count(before), compact_count(after)).into(),
            _ => label.clone(),
        };
        let text = if active {
            shimmer_label(text, self.animation_step, colors.theme.muted_foreground, colors.theme.text)
        } else {
            text.into_any_element()
        };
        div()
            .flex()
            .w_full()
            .items_center()
            .gap_3()
            .py_1()
            .text_size(TEXT_XS.size)
            .line_height(TEXT_XS.line_height)
            .text_color(tone)
            .child(div().h(px(1.)).flex_1().bg(rule))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(if failed { IconName::CircleAlert } else { IconName::Minimize2 }).size(IconSize::Custom(px(12.))).color(tone).stroke(chat_icon_stroke(if failed { IconName::CircleAlert } else { IconName::Minimize2 })))
                    .child(text),
            )
            .child(div().h(px(1.)).flex_1().bg(rule))
            .into_any_element()
    }

    /// The expanded group: a nested virtual list of entries.
    pub(super) fn render_group_details(
        &mut self,
        group_id: &SharedString,
        entries: &std::sync::Arc<[WorkLogEntry]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.group_lists.entry(group_id.clone()).or_insert_with(VirtualListState::new).clone();
        let slots: Vec<RowSlot> = entries
            .iter()
            .map(|e| RowSlot { key: MessagesTimeline::key_of(e.id.as_str()), estimate: GROUP_ROW_ESTIMATE })
            .collect();
        state.set_rows(&slots);
        let open = entries.iter().filter(|e| self.expanded_entries.contains(&e.id)).count();
        let cap = GROUP_LIST_MAX.min(window.viewport_size().height * 0.5) + OPEN_ENTRY_ALLOWANCE * open as f32;
        let entries = entries.clone();
        let weak = cx.weak_entity();
        let colors = self.colors;
        div()
            .pl(rems(1.75))
            .pb_1()
            .child(
                virtual_list(("group-list", MessagesTimeline::key_of(group_id)), state, move |index, _window, cx| {
                    let Some(entry) = entries.get(index).cloned() else {
                        return div().into_any_element();
                    };
                    weak.update(cx, |this, cx| {
                        let row = TimelineRow {
                            id: entry.id.0.clone(),
                            key: MessagesTimeline::key_of(entry.id.as_str()),
                            created_at: Some(entry.created_at),
                            continues_work_log: true,
                            kind: RowKind::Work { entry: entry.clone(), display_label: None },
                        };
                        this.render_work_entry(&row, &entry, None, true, cx)
                    })
                    .unwrap_or_else(|_| div().into_any_element())
                })
                .max_height(cap)
                .scrollbar_colors(colors.scrollbar, colors.scrollbar_hover),
            )
            .into_any_element()
    }

    /// Images the agent opened, kept visible while their group is folded.
    pub(super) fn render_viewed_images(&mut self, paths: &[SharedString]) -> AnyElement {
        let colors = self.colors;
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .px(rems(0.125))
            .children(paths.iter().map(|path| {
                img(path.clone())
                    .max_h(VIEWED_IMAGE_MAX)
                    .h(rems(6.))
                    .rounded(rems(0.5))
                    .border_1()
                    .border_color(alpha(colors.theme.border, 0.8))
            }))
            .into_any_element()
    }
}

/// `182340` → `182k`.
fn compact_count(count: u64) -> String {
    if count >= 1_000 { format!("{}k", count / 1_000) } else { count.to_string() }
}
