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
                div().text_color(colors.theme.muted_foreground).child(format_clock(message.updated_at.0, cx))
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

    /// `ChangedFilesCard`: "N changed files" with diff stats, then the tree
    /// `buildTurnDiffTree` builds. Folders start collapsed (`allDirectoriesExpanded`
    /// defaults to false); the header's chevrons button expands or collapses them all and
    /// drops the per-folder overrides, as the fork's tree remounts on that key.
    fn render_changed_files(&mut self, key: u64, summary: &TurnDiffSummary, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors;
        let turn = summary.assistant_message_id.clone();
        let (additions, deletions) = summary.files.iter().fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
        let fill = if colors.is_dark() { alpha(colors.theme.input, 0.2) } else { colors.theme.secondary };
        let all_toggle = EntryId(format!("changed-files:{}", turn.0).into());
        let all_expanded = self.expanded_entries.contains(&all_toggle);
        let tree = build_turn_diff_tree(&summary.files);
        let has_folders = tree.iter().any(|node| matches!(node, DiffNode::Dir { .. }));
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
                    .when(additions > 0 || deletions > 0, |d| d.child(diff_stat(additions, deletions, &colors, TEXT_XS.size))),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .when(has_folders, |d| {
                        let glyph = if all_expanded { IconName::ChevronsDownUp } else { IconName::ChevronsUpDown };
                        let (all_toggle, prefix) = (all_toggle.clone(), dir_toggle_prefix(&turn.0));
                        d.child(
                            xs_button(("expand-all", key), Some(glyph), None, colors.theme.muted_foreground, &colors)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.expanded_entries.retain(|id| !id.0.starts_with(prefix.as_str()));
                                    this.toggle_entry(all_toggle.clone(), cx)
                                }))
                                .test_support(),
                        )
                    })
                    .child(
                        xs_button(("open-diff-turn", key), Some(IconName::FileDiff), Some("Open diff".into()), colors.theme.muted_foreground, &colors)
                            .on_click({
                                let (turn, first) = (turn.clone(), summary.files.first().map(|f| f.path.clone()));
                                cx.listener(move |_, _, _, cx| cx.emit(TimelineAction::OpenDiff { turn: Some(turn.clone()), file: first.clone() }))
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
        let icon_size = colors.r(px(14.));
        let mut rows = Vec::new();
        let mut stack: Vec<(&[DiffNode], usize)> = vec![(&tree, 0)];
        while let Some((nodes, depth)) = stack.pop() {
            let Some((node, rest)) = nodes.split_first() else { continue };
            stack.push((rest, depth));
            let left = px(8. + depth as f32 * 14.);
            match node {
                DiffNode::Dir { name, path, additions, deletions, children } => {
                    let toggle = EntryId(format!("{}{path}", dir_toggle_prefix(&turn.0)).into());
                    let expanded = all_expanded != self.expanded_entries.contains(&toggle);
                    rows.push(
                        row()
                            .id(gpui_kit::SharedString::from(format!("changed-dir-{key}-{path}")))
                            .pl(left)
                            .cursor_pointer()
                            .hover(|s| s.bg(alpha(colors.theme.accent_surface, 0.6)))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_entry(toggle.clone(), cx)))
                            .child(
                                Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                    .size(IconSize::Custom(icon_size))
                                    .color(alpha(colors.theme.muted_foreground, 0.7)),
                            )
                            .child(
                                Icon::new(if expanded { IconName::Folder } else { IconName::FolderClosed })
                                    .size(IconSize::Custom(icon_size))
                                    .color(alpha(colors.theme.muted_foreground, 0.75)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(mono().family)
                                    .text_size(TEXT_2XS.size)
                                    .text_color(alpha(colors.theme.muted_foreground, 0.9))
                                    .child(name.clone()),
                            )
                            .when(*additions > 0 || *deletions > 0, |d| d.child(stat(*additions, *deletions)))
                            .into_any_element(),
                    );
                    if expanded {
                        stack.push((children, depth + 1));
                    }
                }
                DiffNode::File { name, file } => {
                    rows.push(
                        row()
                            .id(gpui_kit::SharedString::from(format!("changed-{}", file.path)))
                            .pl(left)
                            .cursor_pointer()
                            .hover(|s| s.bg(alpha(colors.theme.accent_surface, 0.6)))
                            .when(has_folders || depth > 0, |d| d.child(div().size(icon_size).flex_none()))
                            .child(changed_file_icon(&file.path, icon_size, &colors))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .font_family(mono().family)
                                    .text_size(TEXT_XS.size)
                                    .text_color(alpha(colors.theme.text, 0.85))
                                    .child(name.to_string()),
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

/// The id prefix of one turn's per-folder expansion overrides.
fn dir_toggle_prefix(turn: &str) -> String {
    format!("changed-dir:{turn}:")
}

/// `PierreEntryIcon` for a file at 14px: Pierre's icon in its token colour, or Lucide's
/// generic file in `text-icon-muted` when Pierre has nothing specific.
fn changed_file_icon(path: &str, size: gpui_kit::Pixels, colors: &super::chat_style::ChatColors) -> AnyElement {
    let icon = crate::pierre_icons::resolve_file_icon(path);
    if icon.is_default() {
        Icon::new(IconName::File).size(IconSize::Custom(size)).color(alpha(colors.theme.muted_foreground, 0.7)).into_any_element()
    } else {
        crate::pierre_icons::pierre_icon(icon, size, colors.appearance, colors.theme.muted_foreground).into_any_element()
    }
}

/// A node of `buildTurnDiffTree`: a folder with its summed counts, or a changed file.
enum DiffNode<'a> {
    Dir { name: String, path: String, additions: u32, deletions: u32, children: Vec<DiffNode<'a>> },
    File { name: &'a str, file: &'a super::timeline_entries::FileChange },
}

/// `buildTurnDiffTree`: folders before files, each sorted by name the way
/// `Intl.Collator(undefined, { numeric: true, sensitivity: "base" })` orders them, and a
/// folder whose only child is a folder merged into one `a/b` row.
fn build_turn_diff_tree(files: &[super::timeline_entries::FileChange]) -> Vec<DiffNode<'_>> {
    #[derive(Default)]
    struct Folder<'a> {
        additions: u32,
        deletions: u32,
        folders: Vec<(&'a str, Folder<'a>)>,
        files: Vec<(&'a str, &'a super::timeline_entries::FileChange)>,
    }
    fn into_nodes<'a>(folder: Folder<'a>, parent: &str) -> Vec<DiffNode<'a>> {
        let mut folders = folder.folders;
        folders.sort_by(|a, b| compare_names(a.0, b.0));
        let mut files = folder.files;
        files.sort_by(|a, b| compare_names(a.0, b.0));
        let mut nodes: Vec<DiffNode<'a>> = folders
            .into_iter()
            .map(|(name, sub)| {
                let path = if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") };
                let (additions, deletions) = (sub.additions, sub.deletions);
                let mut node = DiffNode::Dir { name: name.to_owned(), children: into_nodes(sub, &path), path, additions, deletions };
                // `compactDirectoryNode`: fold single-folder chains into one row.
                while let DiffNode::Dir { name, children, .. } = &mut node
                    && children.len() == 1
                    && matches!(children[0], DiffNode::Dir { .. })
                {
                    let DiffNode::Dir { name: child, path, additions, deletions, children: grand } = children.pop().unwrap() else { unreachable!() };
                    node = DiffNode::Dir { name: format!("{name}/{child}"), path, additions, deletions, children: grand };
                }
                node
            })
            .collect();
        nodes.extend(files.into_iter().map(|(name, file)| DiffNode::File { name, file }));
        nodes
    }
    let mut root = Folder::default();
    for file in files {
        let mut segments: Vec<&str> = file.path.split('/').filter(|s| !s.is_empty()).collect();
        let Some(name) = segments.pop() else { continue };
        let mut folder = &mut root;
        folder.additions += file.additions;
        folder.deletions += file.deletions;
        for segment in segments {
            let index = match folder.folders.iter().position(|(n, _)| *n == segment) {
                Some(index) => index,
                None => {
                    folder.folders.push((segment, Folder::default()));
                    folder.folders.len() - 1
                }
            };
            folder = &mut folder.folders[index].1;
            folder.additions += file.additions;
            folder.deletions += file.deletions;
        }
        folder.files.push((name, file));
    }
    into_nodes(root, "")
}

/// Case-insensitive name order with digit runs compared by value, the order of a
/// `numeric: true, sensitivity: "base"` collator for plain file names.
fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    let trimmed = digits.trim_start_matches('0').to_owned();
                    (trimmed.len(), trimmed)
                };
                let ordering = number(&mut a).cmp(&number(&mut b));
                if ordering.is_ne() {
                    return ordering;
                }
            }
            (Some(x), Some(y)) => {
                let ordering = x.to_lowercase().cmp(y.to_lowercase());
                if ordering.is_ne() {
                    return ordering;
                }
                a.next();
                b.next();
            }
        }
    }
}
