//! Assistant replies: the markdown body (streamed block by block, new blocks
//! fading in on the animation clock), the changed-files card, and the meta
//! row with copy, fork, reply and timestamp. Ports `TimelineAssistantRows.tsx`
//! and `ChangedFilesTree.tsx`.

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{rems, 
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use super::{
    chat_style::{TEXT_2XS, TEXT_3XS, TEXT_XS, alpha, mono},
    messages_timeline::{MessagesTimeline, TimelineAction},
    messages_timeline_logic::{MessageRow, TimelineRow},
    timeline_entries::{ChatMessage, EntryId, TurnDiffSummary},
    timeline_row_parts::{diff_stat, xs_button},
    work_log_presentation::format_clock,
};
use crate::{
    components::chat_markdown::{MarkdownView, render_markdown},
    markdown_pipeline::MarkdownFlavor,
};

/// Changed-files card: `mt-4 rounded-lg`, header `px-3 py-2`, tree rows `h-7`.
const CARD_MARGIN_TOP: gpui_kit::Rems = gpui_kit::rems(1.0);
const TREE_ROW_HEIGHT: gpui_kit::Rems = gpui_kit::rems(1.75);

impl MessagesTimeline {
    pub(super) fn render_assistant_message(
        &mut self,
        row: &TimelineRow,
        message_row: &MessageRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let message = &message_row.message;
        let group = format!("assistant-row-{}", row.key);
        let streaming = message.streaming;
        let body = if message.text.trim().is_empty() && !streaming {
            div().text_color(colors.theme.muted_foreground).text_size(rems(0.875)).child("(empty response)").into_any_element()
        } else {
            let actions = self.markdown_actions();
            let parsed = self.message_markdown(&message.id.0, &message.text, streaming, MarkdownFlavor::ASSISTANT);
            let pieces = parsed.pieces.clone();
            let first_seen = parsed.first_seen.clone();
            let opacity: Vec<f32> = first_seen.iter().map(|seen| self.fade_opacity(*seen)).collect();
            let view = MarkdownView {
                key: row.key,
                pieces: &pieces,
                colors: &colors,
                user: false,
                streaming,
                piece_opacity: opacity.iter().any(|o| *o < 1.).then_some(opacity.as_slice()),
                state: &self.markdown_state,
                actions,
                find: self.find_highlight(),
            };
            render_markdown(&view, window, cx).into_any_element()
        };
        div()
            .group(group.clone())
            .relative()
            .min_w_0()
            .px_1()
            .py(rems(0.125))
            .child(body)
            .children(message_row.diff_summary.as_ref().map(|summary| self.render_changed_files(row.key, summary, cx)))
            .children(message_row.show_meta.then(|| {
                div().opacity(0.).group_hover(group, |s| s.opacity(1.)).child(self.render_assistant_meta(message, false, cx))
            }))
            .into_any_element()
    }

    /// Copy, Fork, Reply and the timestamp under a finished reply. The
    /// standalone meta row (after trailing tool groups) is always visible.
    pub(super) fn render_assistant_meta(&mut self, message: &ChatMessage, standalone: bool, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors;
        let key = MessagesTimeline::key_of(message.id.as_str()).wrapping_add(7);
        let copied = self.markdown_state.copied == Some(key);
        let text = message.text.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .mt(if standalone { rems(0.125) } else { rems(0.375) })
            .text_size(TEXT_XS.size)
            .child(
                xs_button(
                    ("copy-reply", key),
                    Some(if copied { IconName::Check } else { IconName::Copy }),
                    None,
                    if copied { colors.theme.message_action } else { colors.theme.muted_foreground },
                    &colors,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.copy(key, text.clone(), cx))),
            )
            // An embedded side chat offers no fork: it would promote the side chat.
            .when(!self.embedded, |row| {
                row.child(xs_button(("fork", key), Some(IconName::GitFork), None, colors.theme.text, &colors).on_click({
                    let id = message.id.clone();
                    cx.listener(move |_, _, _, cx| cx.emit(TimelineAction::Fork(id.clone())))
                }).test_support())
            })
            .child(xs_button(("reply", key), Some(IconName::Reply), None, colors.theme.muted_foreground, &colors).on_click({
                let (id, text) = (message.id.clone(), message.text.clone());
                cx.listener(move |_, _, _, cx| cx.emit(TimelineAction::Reply { message: id.clone(), text: text.clone() }))
            }).test_support())
            .children((!message.streaming).then(|| {
                div().text_color(colors.theme.muted_foreground).child(format_clock(message.updated_at.0))
            }))
            .into_any_element()
    }

    /// Agent `send_to_user` text: the assistant body without meta.
    pub(super) fn render_sent_message(
        &mut self,
        row: &TimelineRow,
        text: &gpui_kit::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let actions = self.markdown_actions();
        let parsed = self.message_markdown(&row.id, text, false, MarkdownFlavor::ASSISTANT);
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
        div().px_1().py(rems(0.125)).child(render_markdown(&view, window, cx)).into_any_element()
    }

    /// `ChangedFilesCard`: "N changed files" with diff stats, then the tree.
    /// Folders start collapsed (`allDirectoriesExpanded` defaults to false);
    /// the header's chevrons button expands or collapses them all.
    fn render_changed_files(&mut self, key: u64, summary: &TurnDiffSummary, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors;
        let turn = summary.assistant_message_id.clone();
        let (additions, deletions) = summary.files.iter().fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
        let fill = if colors.is_dark() { alpha(colors.theme.input, 0.2) } else { colors.theme.secondary };
        let toggle = EntryId(format!("changed-files:{}", summary.assistant_message_id.0).into());
        let expanded = self.expanded_entries.contains(&toggle);
        // Group files under their folder, as the tree's first level does.
        let mut folders: Vec<(String, Vec<&super::timeline_entries::FileChange>)> = Vec::new();
        for file in summary.files.iter() {
            let folder = file.path.rsplit_once('/').map_or(String::new(), |(dir, _)| dir.to_owned());
            match folders.iter_mut().find(|(name, _)| *name == folder) {
                Some((_, list)) => list.push(file),
                None => folders.push((folder, vec![file])),
            }
        }
        let has_folders = folders.iter().any(|(name, _)| !name.is_empty());
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_size(TEXT_XS.size)
                    .line_height(TEXT_XS.line_height)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.theme.text)
                    .child(match summary.files.len() {
                        1 => "1 changed file".to_owned(),
                        n => format!("{n} changed files"),
                    })
                    .child(diff_stat(additions, deletions, &colors, TEXT_XS.size)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .when(has_folders, |d| {
                        let toggle = toggle.clone();
                        let glyph = if expanded { IconName::ChevronsDownUp } else { IconName::ChevronsUpDown };
                        d.child(
                            xs_button(("expand-all", key), Some(glyph), None, colors.theme.muted_foreground, &colors)
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_entry(toggle.clone(), cx)))
                                .test_support(),
                        )
                    })
                    .child(
                        xs_button(("open-diff-turn", key), Some(IconName::FileDiff), Some("Open diff".into()), colors.theme.muted_foreground, &colors)
                            .on_click({
                                let turn = turn.clone();
                                cx.listener(move |_, _, _, cx| cx.emit(TimelineAction::OpenDiff { turn: Some(turn.clone()), file: None }))
                            })
                            .test_support(),
                    ),
            );
        let row = || div().h(TREE_ROW_HEIGHT).w_full().flex().items_center().gap_2().rounded(rems(0.375)).py(rems(0.375)).pr_2();
        // `DiffStatLabel` aligned: two right-aligned 4ch columns, 8px apart.
        let stat = move |a: u32, d: u32| {
            let column = |text: String, color| div().w(rems(1.5)).flex().justify_end().text_color(color).child(text);
            div()
                .ml_auto()
                .flex()
                .flex_none()
                .gap_2()
                .font_family(mono().family)
                .text_size(TEXT_3XS.size)
                .line_height(TEXT_3XS.line_height)
                .child(column(format!("+{a}"), colors.diff_addition))
                .child(column(format!("-{d}"), colors.diff_deletion))
        };
        let mut rows = Vec::new();
        for (folder, files) in folders {
            let depth = if folder.is_empty() { 0. } else { 1. };
            if !folder.is_empty() {
                let (a, d) = files.iter().fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
                let toggle = toggle.clone();
                rows.push(
                    row()
                        .id(gpui_kit::SharedString::from(format!("changed-dir-{key}-{folder}")))
                        .pl(rems(0.5))
                        .cursor_pointer()
                        .hover(|s| s.bg(alpha(colors.theme.accent_surface, 0.6)))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_entry(toggle.clone(), cx)))
                        .child(
                            Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                .size(IconSize::Sm)
                                .color(alpha(colors.theme.muted_foreground, 0.7)),
                        )
                        .child(
                            Icon::new(if expanded { IconName::Folder } else { IconName::FolderClosed })
                                .size(IconSize::Sm)
                                .color(alpha(colors.theme.muted_foreground, 0.75)),
                        )
                        .child(
                            div()
                                .truncate()
                                .font_family(mono().family)
                                .text_size(TEXT_2XS.size)
                                .text_color(alpha(colors.theme.muted_foreground, 0.9))
                                .child(folder.clone()),
                        )
                        .child(stat(a, d))
                        .into_any_element(),
                );
                if !expanded {
                    continue;
                }
            }
            for file in files {
                let name = file.path.rsplit('/').next().unwrap_or(&file.path).to_owned();
                rows.push(
                    row()
                        .id(gpui_kit::SharedString::from(format!("changed-{}", file.path)))
                        .pl(px(8. + depth * 14.))
                        .cursor_pointer()
                        .hover(|s| s.bg(alpha(colors.theme.accent_surface, 0.6)))
                        .when(has_folders, |d| d.child(div().size(rems(0.875)).flex_none()))
                        .child(Icon::new(IconName::File).size(IconSize::Sm).color(alpha(colors.theme.muted_foreground, 0.7)))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .font_family(mono().family)
                                .text_size(TEXT_XS.size)
                                .text_color(alpha(colors.theme.text, 0.85))
                                .child(name),
                        )
                        .child(stat(file.additions, file.deletions))
                        .on_click({
                            let (turn, path) = (turn.clone(), file.path.clone());
                            cx.listener(move |_, _, _, cx| cx.emit(TimelineAction::OpenDiff { turn: Some(turn.clone()), file: Some(path.clone()) }))
                        })
                        .test_support()
                        .into_any_element(),
                );
            }
        }
        div()
            .mt(CARD_MARGIN_TOP)
            .rounded(rems(0.5))
            .bg(fill)
            .child(header)
            .child(div().p_2().flex().flex_col().children(rows))
            .into_any_element()
    }
}
