//! A question the agent is waiting on, drawn as the drawer attached to the
//! composer's top edge (`ComposerPendingUserInputPanel.tsx`). The composer
//! owns where it mounts; this draws the drawer from the timeline's data.

use crate::components::icons::{Icon, IconName, IconSize};
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use super::{
    chat_style::{ChatColors, TEXT_2XS, TEXT_SM, TEXT_XS, alpha},
    timeline_entries::PendingQuestion,
};

/// Option radio: 16px circle with a 6px dot when chosen.
const RADIO: gpui_kit::Pixels = px(16.);
const RADIO_DOT: gpui_kit::Pixels = px(6.);

/// Called with an option's index when it is clicked.
pub type OnPick = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The drawer for `question`, with option `selected` highlighted. Clicking an
/// option calls `on_pick`.
pub fn pending_user_input_panel(
    question: &PendingQuestion,
    selected: Option<usize>,
    on_pick: OnPick,
    colors: &ChatColors,
) -> AnyElement {
    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .px_4()
        .py(px(6.))
        .text_size(TEXT_XS.size)
        .text_color(colors.theme.muted_foreground)
        .child(div().flex_1().child(question.header.clone()))
        .child(format!("{}/{}", question.index + 1, question.count))
        .child(Icon::new(IconName::ChevronDown).size(IconSize::Custom(px(14.))).color(colors.theme.muted_foreground));
    let options = question.options.iter().enumerate().map(|(index, (label, description))| {
        let chosen = selected == Some(index);
        let radio = div()
            .mt(px(2.))
            .size(RADIO)
            .flex_none()
            .rounded_full()
            .border_1()
            .flex()
            .items_center()
            .justify_center();
        let radio = if chosen {
            radio.border_color(colors.theme.message_action).bg(colors.theme.message_action).child(
                div().size(RADIO_DOT).rounded_full().bg(colors.theme.message_action_foreground),
            )
        } else {
            radio
                .border_color(alpha(colors.theme.text, 0.55))
                .bg(if colors.is_dark() { alpha(colors.theme.input, 0.32) } else { colors.theme.canvas })
        };
        let on_pick = on_pick.clone();
        let hover = alpha(colors.theme.muted, 0.3);
        div()
            .id(("question-option", index))
            .flex()
            .w_full()
            .items_start()
            .gap(px(10.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(move |_, window, cx| on_pick(index, window, cx))
            .rounded(px(6.))
            .py(px(6.))
            .pl(px(6.))
            .text_color(if chosen { colors.theme.text } else { alpha(colors.theme.text, 0.7) })
            .child(radio)
            .child(
                div()
                    .flex_1()
                    .child(div().text_size(TEXT_SM.size).font_weight(FontWeight::MEDIUM).child(label.clone()))
                    .child(div().text_size(TEXT_2XS.size).text_color(colors.theme.secondary_label).child(description.clone())),
            )
            .child(
                div()
                    .w(px(24.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(alpha(colors.theme.text, 0.07))
                    .text_size(TEXT_XS.size)
                    .text_color(colors.theme.muted_foreground)
                    .child((index + 1).to_string()),
            )
    });
    div()
        .w_full()
        .child(header)
        .child(
            div()
                .pl(px(22.))
                .pr_4()
                .pb_1()
                .child(div().pt(px(2.)).text_size(TEXT_SM.size).text_color(alpha(colors.theme.text, 0.85)).child(question.question.clone()))
                .child(div().mt(px(6.)).ml(px(-6.)).flex().flex_col().children(options)),
        )
        .into_any_element()
}
