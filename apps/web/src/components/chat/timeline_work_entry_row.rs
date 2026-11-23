//! One work-log entry and its expanded detail. Ports `TimelineWorkEntryRow.tsx`
//! and `V2ItemInspector.tsx`: the line with icon, label, failure mark,
//! chevron and hover timestamp, then per-tool detail when opened. Output
//! bodies render only while the entry is open, matching the web client's
//! fetch-on-expand.

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, TextRun, div, px,
};

use super::{
    chat_style::{ChatColors, TEXT_CODE, TEXT_SM_RELAXED, TEXT_XS, alpha, mono},
    messages_timeline::MessagesTimeline,
    messages_timeline_logic::TimelineRow,
    rich_text::RichText,
    syntax_highlighting::highlight,
    syntax_theme::SyntaxTheme,
    timeline_entries::{TodoState, ToolStatus, WorkItem, WorkLogEntry},
    timeline_row_parts::{
        SMALL_ICON, disclosure, failed_mark, hover_timestamp, tool_icon, work_log_line, work_log_row, xs_button,
    },
    work_log_presentation::{entry_icon, entry_label, indicates_failure, is_tool_like},
};

/// `ms-7`: detail indents past the icon slot.
const DETAIL_INDENT: gpui_kit::Pixels = px(28.);
/// Scroll caps: command output `max-h-80`, read output `max-h-64`, structured `max-h-80`.
const OUTPUT_MAX_HEIGHT: gpui_kit::Pixels = px(320.);
const READ_MAX_HEIGHT: gpui_kit::Pixels = px(256.);
/// Monospace detail: `--font-size-code` with `leading-relaxed`.
const CODE_LINE_HEIGHT: gpui_kit::Pixels = px(21.125);

impl MessagesTimeline {
    /// A work-log entry, standalone or inside an expanded group.
    pub(super) fn render_work_entry(
        &mut self,
        row: &TimelineRow,
        entry: &WorkLogEntry,
        display_label: Option<SharedString>,
        grouped: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = row;
        let colors = self.colors;
        if let WorkItem::Error { message, usage_limit_retry, retryable } = &entry.item {
            return self.render_error_entry(entry, message, usage_limit_retry.as_ref(), *retryable);
        }
        let failed = indicates_failure(entry);
        let severe = matches!(entry.item, WorkItem::SystemNotice { info: false, .. });
        let expandable = has_detail(entry);
        let open = self.expanded_entries.contains(&entry.id);
        let label = display_label.unwrap_or_else(|| entry_label(entry));
        let label_element = if severe {
            div().font_weight(FontWeight::MEDIUM).text_color(colors.destructive()).child(label).into_any_element()
        } else if is_tool_like(entry) || matches!(entry.item, WorkItem::Reasoning { .. }) {
            // Tool and thinking rows read `secondary-label`; info rows `foreground/80`.
            label.into_any_element()
        } else {
            div().text_color(alpha(colors.theme.text, 0.8)).child(label).into_any_element()
        };
        let mut trailing = Vec::new();
        if failed && !severe {
            trailing.push(failed_mark(&colors));
        }
        if expandable {
            trailing.push(disclosure(open, px(12.), alpha(colors.theme.icon_muted, 0.7)));
        }
        let line = work_log_line(Some(tool_icon(entry_icon(entry), &colors, failed)), label_element, trailing, &colors);
        let id = entry.id.clone();
        let mut container = work_log_row(("work-entry", MessagesTimeline::key_of(entry.id.as_str())), grouped)
            .child(line)
            .children(hover_timestamp(Some(entry.created_at.0), &colors));
        if expandable {
            container = container
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_entry(id.clone(), cx)));
        }
        let detail = if let WorkItem::QuestionAnswer { pairs } = &entry.item {
            Some(question_answers(pairs, &colors))
        } else if open && expandable {
            Some(self.inspector(entry))
        } else {
            None
        };
        div().w_full().child(container).children(detail).into_any_element()
    }

    /// A turn failure: alert icon, bold label, message and an optional Retry.
    fn render_error_entry(
        &self,
        entry: &WorkLogEntry,
        message: &SharedString,
        usage_limit_retry: Option<&SharedString>,
        retryable: bool,
    ) -> AnyElement {
        let colors = self.colors;
        let tone = if usage_limit_retry.is_some() { colors.theme.warning } else { colors.destructive() };
        let title: SharedString = match usage_limit_retry {
            Some(at) => format!("Usage limit reached. Retry after {at}").into(),
            None => entry.label.clone(),
        };
        div()
            .w_full()
            .child(work_log_line(
                Some(Icon::new(IconName::CircleAlert).size(IconSize::Custom(px(16.))).color(tone).into_any_element()),
                div().font_weight(FontWeight::MEDIUM).text_color(colors.theme.text).child(title).into_any_element(),
                vec![
                    div()
                        .text_size(TEXT_XS.size)
                        .text_color(colors.theme.muted_foreground)
                        .pr_1()
                        .child(super::work_log_presentation::format_clock(entry.created_at.0))
                        .into_any_element(),
                ],
                &colors,
            ))
            .child(
                div()
                    .ml(DETAIL_INDENT)
                    .py_1()
                    .text_size(TEXT_SM_RELAXED.size)
                    .line_height(TEXT_SM_RELAXED.line_height)
                    .text_color(alpha(colors.theme.text, 0.8))
                    .child(message.clone()),
            )
            .children(retryable.then(|| {
                div().ml(DETAIL_INDENT).flex().child(
                    xs_button(("retry", MessagesTimeline::key_of(entry.id.as_str())), Some(IconName::RotateCcw), Some("Retry".into()), colors.theme.text, &colors)
                        .border_1()
                        .border_color(colors.theme.border),
                )
            }))
            .into_any_element()
    }

    /// Per-tool detail of an open entry (`V2ItemInspector`).
    fn inspector(&self, entry: &WorkLogEntry) -> AnyElement {
        let colors = self.colors;
        let theme = SyntaxTheme::for_appearance(colors.appearance);
        let key = MessagesTimeline::key_of(entry.id.as_str());
        let mono_block = |id: &'static str, text: SharedString, color: Hsla| {
            div().text_size(TEXT_CODE.size).line_height(CODE_LINE_HEIGHT).child(
                RichText::new((id, key), text.clone(), vec![mono_run(text.len(), color)]).selection_color(colors.selection),
            )
        };
        let body: Vec<AnyElement> = match &entry.item {
            WorkItem::Command { command, output, exit_code } => {
                let command_runs = highlighted_runs(command, "bash", theme, alpha(colors.theme.text, 0.85));
                let mut parts = vec![
                    div()
                        .text_size(TEXT_CODE.size)
                        .line_height(CODE_LINE_HEIGHT)
                        .child(RichText::new(("command", key), command.clone(), command_runs).selection_color(colors.selection))
                        .into_any_element(),
                ];
                let output_element = match output {
                    Some(output) if !output.is_empty() => mono_block("output", output.clone(), colors.theme.muted_foreground).into_any_element(),
                    _ => div().text_color(colors.theme.muted_foreground).child("No output.").into_any_element(),
                };
                parts.push(
                    div()
                        .id(("command-output", key))
                        .max_h(OUTPUT_MAX_HEIGHT)
                        .overflow_y_scroll()
                        .child(output_element)
                        .into_any_element(),
                );
                if let Some(code) = exit_code.filter(|c| *c != 0) {
                    parts.push(div().text_color(colors.destructive()).child(format!("exit {code}")).into_any_element());
                }
                parts
            }
            WorkItem::FileChange { changes, raw_diff } => {
                let mut parts: Vec<AnyElement> = changes
                    .iter()
                    .enumerate()
                    .map(|(index, change)| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().font_family(mono().family).text_color(colors.theme.muted_foreground).truncate().child(change.path.clone()))
                            .child(div().font_family(mono().family).text_color(colors.diff_addition).child(format!("+{}", change.additions)))
                            .child(div().font_family(mono().family).text_color(colors.diff_deletion).child(format!("-{}", change.deletions)))
                            .child(
                                xs_button(("open-diff", key.wrapping_add(index as u64)), None, Some("Open diff".into()), colors.theme.text, &colors)
                                    .border_1()
                                    .border_color(colors.theme.border),
                            )
                            .into_any_element()
                    })
                    .collect();
                if entry.status == ToolStatus::Failed && let Some(diff) = raw_diff {
                    parts.push(mono_block("raw-diff", diff.clone(), colors.theme.muted_foreground).into_any_element());
                }
                parts
            }
            WorkItem::FileRead { output, paths } => {
                let text = output.clone().unwrap_or_else(|| paths.join("\n").into());
                vec![
                    div()
                        .id(("read-output", key))
                        .max_h(READ_MAX_HEIGHT)
                        .overflow_y_scroll()
                        .child(mono_block("read", text, colors.theme.secondary_label))
                        .into_any_element(),
                ]
            }
            WorkItem::FileSearch { pattern, matches } => {
                let mut parts = vec![mono_block("pattern", pattern.clone(), alpha(colors.theme.text, 0.85)).into_any_element()];
                parts.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(matches.iter().map(|m| {
                            div()
                                .child(div().font_family(mono().family).text_color(alpha(colors.theme.text, 0.8)).child(format!("{}:{}:{}", m.path, m.line, m.column)))
                                .child(div().mt(px(2.)).text_color(colors.theme.muted_foreground).child(m.preview.clone()))
                        }))
                        .into_any_element(),
                );
                parts
            }
            WorkItem::WebSearch { queries, results } => {
                let mut parts: Vec<AnyElement> =
                    queries.iter().map(|q| div().font_family(mono().family).child(q.clone()).into_any_element()).collect();
                parts.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .children(results.iter().map(|r| {
                            div()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(colors.theme.text)
                                        .child(r.title.clone())
                                        .child(Icon::new(IconName::ExternalLink).size(IconSize::Custom(SMALL_ICON)).color(colors.theme.muted_foreground)),
                                )
                                .child(div().text_color(colors.theme.muted_foreground).child(r.snippet.clone()))
                        }))
                        .into_any_element(),
                );
                parts
            }
            WorkItem::DynamicTool { args, output, .. } => {
                let mut parts: Vec<AnyElement> = args
                    .iter()
                    .map(|(name, value)| {
                        div()
                            .flex()
                            .gap_2()
                            .font_family(mono().family)
                            .child(div().text_color(colors.theme.muted_foreground).child(name.clone()))
                            .child(div().flex_1().min_w_0().child(value.clone()))
                            .into_any_element()
                    })
                    .collect();
                if let Some(output) = output {
                    let runs = highlighted_runs(output, "json", theme, colors.theme.muted_foreground);
                    parts.push(
                        div()
                            .id(("tool-output", key))
                            .max_h(OUTPUT_MAX_HEIGHT)
                            .overflow_y_scroll()
                            .text_size(TEXT_CODE.size)
                            .line_height(CODE_LINE_HEIGHT)
                            .child(RichText::new(("tool-output-text", key), output.clone(), runs).selection_color(colors.selection))
                            .into_any_element(),
                    );
                }
                parts
            }
            WorkItem::Reasoning { text } => vec![
                div()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(alpha(colors.theme.border, 0.45))
                    .bg(alpha(colors.theme.muted, 0.15))
                    .p_2()
                    .italic()
                    .text_color(colors.theme.muted_foreground)
                    .child(text.clone())
                    .into_any_element(),
            ],
            WorkItem::TodoList { steps } => steps
                .iter()
                .map(|(state, step)| {
                    let mark = match state {
                        TodoState::Completed => "✓",
                        TodoState::Running => "◉",
                        TodoState::Pending => "○",
                    };
                    div().child(format!("{mark} {step}")).into_any_element()
                })
                .collect(),
            WorkItem::Approval { detail, reason, .. } => {
                let mut parts = vec![div().child(detail.clone()).into_any_element()];
                parts.extend(reason.iter().map(|r| div().child(format!("• {r}")).into_any_element()));
                parts
            }
            WorkItem::SystemNotice { text, .. } | WorkItem::Notification { text } => vec![div().child(text.clone()).into_any_element()],
            _ => Vec::new(),
        };
        div()
            .ml(DETAIL_INDENT)
            .mt(px(2.))
            .mb(px(6.))
            .pb_2()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(TEXT_XS.size)
            .line_height(px(19.5))
            .text_color(colors.theme.text)
            .children(body)
            .into_any_element()
    }
}

/// Whether opening an entry shows anything.
fn has_detail(entry: &WorkLogEntry) -> bool {
    !matches!(entry.item, WorkItem::QuestionAnswer { .. } | WorkItem::ViewedImage { .. } | WorkItem::Compaction { .. })
}

fn mono_run(len: usize, color: Hsla) -> TextRun {
    TextRun { len, font: mono(), color, background_color: None, underline: None, strikethrough: None }
}

/// Mono runs for `code` coloured by the syntax theme.
pub(super) fn highlighted_runs(code: &str, language: &str, theme: &SyntaxTheme, fallback: Hsla) -> Vec<TextRun> {
    let Some(highlights) = highlight(language, code, false) else {
        return vec![mono_run(code.len(), fallback)];
    };
    let mut runs = Vec::new();
    let mut cursor = 0;
    let push = |len: usize, color: Hsla, runs: &mut Vec<TextRun>| {
        if len > 0 {
            runs.push(mono_run(len, color));
        }
    };
    for (range, name) in highlights.iter() {
        push(range.start.saturating_sub(cursor), fallback, &mut runs);
        let color = theme.style(name).and_then(|s| s.color).unwrap_or(fallback);
        push(range.len(), color, &mut runs);
        cursor = range.end;
    }
    push(code.len().saturating_sub(cursor), fallback, &mut runs);
    if runs.is_empty() {
        runs.push(mono_run(0, fallback));
    }
    runs
}

/// Answered questions under their line: question, then the indented answer.
fn question_answers(pairs: &[(SharedString, SharedString)], colors: &ChatColors) -> AnyElement {
    div()
        .ml(DETAIL_INDENT)
        .mt_2()
        .flex()
        .flex_col()
        .gap_2()
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(colors.theme.muted_foreground)
        .children(pairs.iter().map(|(question, answer)| {
            div().child(question.clone()).child(div().ml_3().child(answer.clone()))
        }))
        .into_any_element()
}
