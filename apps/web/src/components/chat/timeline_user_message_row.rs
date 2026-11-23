//! A user message: optional "Sent by" and steer lines, the bubble with
//! attachments and markdown, the long-message fold, and the hover actions row
//! (timestamp, Edit from here, Copy). Ports `TimelineUserMessageRow.tsx`.

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, Window, div, img, linear_color_stop, linear_gradient, px, relative,
};

use super::{
    chat_style::{TEXT_2XS, TEXT_XS, alpha},
    messages_timeline::MessagesTimeline,
    messages_timeline_logic::{MessageRow, TimelineRow},
    timeline_entries::{AttachmentKind, InputIntent},
    timeline_row_parts::{SMALL_ICON, xs_button},
    work_log_presentation::format_clock,
};
use crate::{
    components::chat_markdown::{MarkdownView, render_markdown},
    markdown_pipeline::MarkdownFlavor,
};

/// Bubble: `max-w-[80%] rounded-2xl p-3`; grouped corners tighten to 6px.
const BUBBLE_RADIUS: gpui_kit::Pixels = px(16.);
const BUBBLE_GROUPED_RADIUS: gpui_kit::Pixels = px(6.);
const BUBBLE_PADDING: gpui_kit::Pixels = px(12.);
const BUBBLE_MAX_WIDTH: f32 = 0.8;
/// Long messages fold at `2 × 11rem` and clip to `max-h-44` with a 1.75rem fade.
const FOLD_THRESHOLD: gpui_kit::Pixels = px(352.);
const FOLD_CLIP: gpui_kit::Pixels = px(176.);
const FOLD_FADE: gpui_kit::Pixels = px(28.);
/// Characters per wrapped line in a full-width bubble, for the fold estimate.
const BUBBLE_CHARS_PER_LINE: f32 = 72.;
/// Images inside a text bubble: two columns, `max-w-[210px]`, 4:3, radius 8px.
const INLINE_MEDIA_WIDTH: gpui_kit::Pixels = px(210.);
/// A lone image without text: `h-48`, radius 16px.
const LONE_IMAGE_HEIGHT: gpui_kit::Pixels = px(192.);

impl MessagesTimeline {
    pub(super) fn render_user_message(
        &mut self,
        row: &TimelineRow,
        message_row: &MessageRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let message = &message_row.message;
        let key = row.key;
        let group = format!("user-row-{key}");
        let images: Vec<_> = message.attachments.iter().filter(|a| a.kind != AttachmentKind::File).collect();
        let files: Vec<_> = message.attachments.iter().filter(|a| a.kind == AttachmentKind::File).collect();
        let has_text = !message.text.trim().is_empty();

        let mut column = div().group(group.clone()).flex().flex_col().items_end().gap_1().w_full();
        if message.sent_by_app {
            column = column.child(
                div().mr_1().text_size(TEXT_2XS.size).text_color(alpha(colors.theme.muted_foreground, 0.7)).child("Sent by T3 Code"),
            );
        }
        if let Some(guest) = &message.guest {
            column = column.child(div().mr_1().text_size(TEXT_2XS.size).text_color(colors.guest).child(guest.clone()));
        }
        if message.input_intent != InputIntent::Prompt && !message_row.continues_steer {
            column = column.child(
                div()
                    .mr_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(TEXT_XS.size)
                    .line_height(px(12.))
                    .text_color(colors.theme.muted_foreground)
                    .child(Icon::new(IconName::Redo2).size(IconSize::Custom(SMALL_ICON)).color(colors.theme.muted_foreground))
                    .child("Steer"),
            );
        }

        if !has_text && files.is_empty() && images.len() == 1 {
            // Media-only messages drop the bubble.
            column = column.child(
                img(images[0].source.clone())
                    .h(LONE_IMAGE_HEIGHT)
                    .min_w(px(96.))
                    .max_w_full()
                    .rounded(BUBBLE_RADIUS)
                    .border_1()
                    .border_color(if colors.is_dark() { alpha(gpui_kit::white(), 0.12) } else { alpha(gpui_kit::black(), 0.09) }),
            );
        } else {
            let tl = BUBBLE_RADIUS;
            let tr = if message_row.grouped_with_previous { BUBBLE_GROUPED_RADIUS } else { BUBBLE_RADIUS };
            let br = if message_row.grouped_with_next { BUBBLE_GROUPED_RADIUS } else { BUBBLE_RADIUS };
            let mut bubble = div()
                .relative()
                .min_w_0()
                .max_w(relative(BUBBLE_MAX_WIDTH))
                .rounded_tl(tl)
                .rounded_bl(BUBBLE_RADIUS)
                .rounded_tr(tr)
                .rounded_br(br)
                .bg(colors.theme.message_surface)
                .text_color(colors.theme.message_foreground)
                .p(BUBBLE_PADDING);
            if !images.is_empty() {
                bubble = bubble.child(
                    div().mb_2().max_w(INLINE_MEDIA_WIDTH).flex().flex_wrap().gap_2().children(images.iter().map(|image| {
                        img(image.source.clone())
                            .w(px(101.))
                            .h(px(75.75))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(alpha(colors.theme.border, 0.8))
                    })),
                );
            }
            if !files.is_empty() {
                bubble = bubble.child(div().mb_2().flex().flex_col().gap_1().children(files.iter().map(|file| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py_1()
                        .text_size(px(14.))
                        .child(Icon::new(IconName::FileText).size(IconSize::Custom(px(16.))).color(colors.theme.muted_foreground))
                        .child(div().flex_1().truncate().child(file.name.clone()))
                        .child(Icon::new(IconName::Eye).size(IconSize::Custom(px(16.))).color(colors.theme.muted_foreground).stroke(2.))
                })));
            }
            if has_text {
                let estimated_lines: f32 = message
                    .text
                    .lines()
                    .map(|line| (line.chars().count() as f32 / BUBBLE_CHARS_PER_LINE).ceil().max(1.))
                    .sum();
                let foldable = estimated_lines * 22.75 >= f32::from(FOLD_THRESHOLD);
                let unfolded = self.unfolded_messages.contains(&message.id.0);
                let actions = self.markdown_actions();
                let parsed = self.message_markdown(&message.id.0, &message.text, false, MarkdownFlavor::USER);
                let pieces = parsed.pieces.clone();
                let view = MarkdownView {
                    key,
                    pieces: &pieces,
                    colors: &colors,
                    user: true,
                    streaming: false,
                    piece_opacity: None,
                    state: &self.markdown_state,
                    actions,
                };
                let markdown = render_markdown(&view, window, cx);
                if foldable && !unfolded {
                    bubble = bubble.child(
                        div()
                            .relative()
                            .max_h(FOLD_CLIP)
                            .overflow_hidden()
                            .child(markdown)
                            .child(
                                div().absolute().bottom_0().left_0().right_0().h(FOLD_FADE).bg(linear_gradient(
                                    180.,
                                    linear_color_stop(alpha(colors.theme.message_surface, 0.), 0.),
                                    linear_color_stop(colors.theme.message_surface, 1.),
                                )),
                            ),
                    );
                } else {
                    bubble = bubble.child(markdown);
                }
                if foldable {
                    let id = message.id.0.clone();
                    bubble = bubble.child(
                        div().mt(px(6.)).ml(px(-4.)).child(
                            xs_button(
                                ("fold", key),
                                None,
                                Some(if unfolded { "Show less".into() } else { "Show full message".into() }),
                                colors.theme.muted_foreground,
                                &colors,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_message_fold(id.clone(), cx))),
                        ),
                    );
                }
            }
            column = column.child(bubble);
        }

        // Actions: timestamp, Edit from here, Copy; revealed on hover.
        let copy_key = key.wrapping_add(1);
        let copied = self.markdown_state.copied == Some(copy_key);
        let text = message.text.clone();
        let actions = div()
            .flex()
            .w_full()
            .max_w(relative(BUBBLE_MAX_WIDTH))
            .justify_end()
            .items_center()
            .gap(px(2.))
            .pr_1()
            .text_size(TEXT_XS.size)
            .opacity(0.)
            .group_hover(group, |s| s.opacity(1.))
            .child(div().mr_1().text_color(colors.theme.muted_foreground).child(format_clock(message.created_at.0)))
            .children(message_row.revert_turn_count.map(|_| {
                xs_button(("revert", key), Some(IconName::Pencil), None, colors.theme.text, &colors)
            }))
            .child(
                xs_button(
                    ("copy-user", key),
                    Some(if copied { IconName::Check } else { IconName::Copy }),
                    None,
                    if copied { colors.theme.message_action } else { colors.theme.muted_foreground },
                    &colors,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.copy(copy_key, text.clone(), cx))),
            );
        column.child(actions).into_any_element()
    }
}
