//! A question the agent is waiting on, drawn as the drawer attached to the
//! composer's top edge (`ComposerPendingUserInputPanel.tsx`). The composer
//! owns where it mounts; this draws the drawer from the timeline's data.

use crate::components::icons::{Icon, IconName, IconSize};
use std::rc::Rc;

use gpui_kit::{rems, prelude::FluentBuilder as _, 
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use super::{
    chat_style::{ChatColors, TEXT_2XS, TEXT_SM, TEXT_SM_RELAXED, TEXT_XS, alpha},
    timeline_entries::PendingQuestion,
};

/// Option radio: `sm:size-4` circle with a `sm:size-1.5` dot when chosen.
const RADIO: gpui_kit::Rems = gpui_kit::rems(1.0);
const RADIO_DOT: gpui_kit::Rems = gpui_kit::rems(0.375);

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
    // `ComposerBanner.Root`: `px-1 py-1` around the rows. The mount's 1px outline takes
    // layout space here, unlike the fork's `::before` outline, so it comes off the padding.
    let root_pad = rems(0.25).to_pixels(gpui_kit::px(colors.rem * 16.)) - px(1.);
    let header = div()
        .flex()
        .items_center()
        .gap(rems(0.25))
        .px_4()
        .py(rems(0.375))
        .text_size(TEXT_XS.size)
        .line_height(TEXT_XS.line_height)
        .text_color(colors.theme.muted_foreground)
        .child(div().flex_1().min_w_0().pl(rems(0.375)).truncate().child(question.header.clone()))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .when(question.count > 1, |d| {
                    d.child(
                        div()
                            .min_w(rems(1.5))
                            .flex()
                            .justify_center()
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("{}/{}", question.index + 1, question.count)),
                    )
                })
                .child(
                    div()
                        .size(rems(1.5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::Custom(colors.r(px(14.)))).color(colors.theme.muted_foreground)),
                ),
        );
    let options = question.options.iter().enumerate().map(|(index, (label, description))| {
        let chosen = selected == Some(index);
        let radio = div().size(RADIO).flex_none().rounded_full().border_1().flex().items_center().justify_center();
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
        // The `h-5` slots match the label's line box, so the radio and key stay on its first line.
        let slot = || div().h(rems(1.25)).flex().flex_none().items_center();
        div()
            .id(("question-option", index))
            .flex()
            .w_full()
            .items_start()
            .gap(rems(0.625))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(move |_, window, cx| on_pick(index, window, cx))
            .rounded(rems(0.375))
            .py(rems(0.375))
            .pl(rems(0.375))
            .text_color(if chosen { colors.theme.text } else { alpha(colors.theme.text, 0.7) })
            .child(slot().child(radio))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(rems(0.125))
                    .child(div().text_size(TEXT_SM.size).line_height(TEXT_SM.line_height).font_weight(FontWeight::MEDIUM).child(label.clone()))
                    .when(!description.is_empty() && description != label, |d| {
                        d.child(div().text_size(TEXT_2XS.size).line_height(TEXT_2XS.line_height).text_color(colors.theme.secondary_label).child(description.clone()))
                    }),
            )
            .when(index < 9, |d| {
                d.child(
                    slot().w(rems(1.5)).justify_center().child(
                        div()
                            .h(rems(1.25))
                            .min_w(rems(1.25))
                            .px(rems(0.25))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(rems(0.25))
                            .bg(alpha(colors.theme.text, 0.07))
                            .text_size(TEXT_XS.size)
                            .line_height(TEXT_XS.line_height)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.theme.muted_foreground)
                            .child((index + 1).to_string()),
                    ),
                )
            })
    });
    div()
        .w_full()
        .px(root_pad)
        .pt(root_pad)
        .child(header)
        .child(
            div()
                .pl(rems(1.375))
                .pr_4()
                .pb_1()
                .child(
                    div()
                        .pt(rems(0.125))
                        .text_size(TEXT_SM_RELAXED.size)
                        .line_height(TEXT_SM_RELAXED.line_height)
                        .text_color(alpha(colors.theme.text, 0.85))
                        .child(question.question.clone()),
                )
                .child(div().mt(rems(0.375)).ml(rems(-0.375)).flex().flex_col().children(options)),
        )
        .into_any_element()
}
