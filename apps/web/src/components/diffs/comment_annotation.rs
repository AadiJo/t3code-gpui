//! The review comment block under a commented line (`DiffCommentAnnotation`): a filed comment
//! with its delete button, and the open form. The diff view and the file viewer both use it.

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, App, Div, KeyDownEvent, Stateful, Entity, Hsla, SharedString, Svg, TestSupportExt as _, Window, div, prelude::*, px, svg};

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;
use crate::theme_palette::AppTheme;

fn icon(path: &'static str, size: f32, color: Hsla) -> Svg {
    svg().path(path).size(px(size)).flex_none().text_color(color)
}

/// A filed comment: its text, and a delete button that shows on hover.
pub fn saved_comment(id: &SharedString, text: SharedString, theme: &AppTheme, on_delete: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let (c, primary) = (&theme.colors, theme.colors.accent);
    div()
        .id(SharedString::from(format!("comment-{id}")))
        .test_support()
        .group("comment")
        .flex()
        .items_start()
        .gap(px(10.))
        .border_l_2()
        .border_color(primary.opacity(0.55))
        .bg(primary.opacity(0.045))
        .px(px(12.))
        .py(px(10.))
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(c.text)
        .child(div().h(px(20.)).flex().items_center().child(icon("icons/diffs/message-circle.svg", 14., primary.opacity(0.7))))
        .child(div().flex_1().min_w_0().child(text))
        .child(
            div()
                .id(SharedString::from(format!("comment-delete-{id}")))
                .test_support()
                .invisible()
                .group_hover("comment", |s| s.visible())
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .hover(|s| s.bg(c.toolbar_control_hover))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on_delete(window, cx);
                })
                .child(icon("icons/diffs/trash-2.svg", 12., c.muted_foreground)),
        )
        .into_any_element()
}

/// The open form (`renderDraft`): the comment field, the send hint, Cancel and Comment, which
/// stays disabled while the field is empty. Escape in the field cancels.
pub fn draft_form(
    editor: &Entity<TextareaState>,
    theme: &AppTheme,
    cx: &App,
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
    on_submit: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let c = &theme.colors;
    let empty = editor.read(cx).value().trim().is_empty();
    let on_cancel = std::rc::Rc::new(on_cancel);
    let escape = on_cancel.clone();
    div()
        .id("comment-draft")
        .px(theme.sp(3.))
        .py(theme.sp(2.))
        .text_color(c.text)
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                escape(window, cx);
            }
        })
        .child(div().h(theme.rem(4.25)).child(Textarea::new(editor).h(theme.rem(4.25))))
        .child(
            div()
                .mt(theme.sp(1.5))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme.sp(1.))
                .child(div().mr_auto().text_size(theme.rem(0.625)).text_color(c.muted_foreground.opacity(0.7)).child(t!("⌘/Ctrl Enter to send")))
                .child(
                    Button::new("draft-cancel")
                        .label(t!("Cancel"))
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::Xs)
                        .on_click(move |_, window, cx| on_cancel(window, cx)),
                )
                .child(
                    Button::new("draft-submit")
                        .label(t!("Comment", context = "verb, submit button"))
                        .size(ButtonSize::Xs)
                        .disabled(empty)
                        .on_click(move |_, window, cx| on_submit(window, cx)),
                ),
        )
}

/// Height of a filed comment's block at `width`, estimated from its text as the views lay it
/// out before it draws.
pub fn saved_comment_height(text: &str, width: f32) -> f32 {
    let chars_per_line = ((width - 60.) / 7.).max(20.) as usize;
    let lines: usize = text.split('\n').map(|l| l.len().div_ceil(chars_per_line).max(1)).sum();
    21. + 20. * lines as f32
}

/// Height of the open form at `width` and interface size `rem` (`py-2`, a `min-h-17` field,
/// `mt-1.5`, `xs` buttons): its footer wraps Cancel and Comment onto a second row below the hint
/// when the row is narrower than the three (`flex-wrap`).
pub fn draft_height(width: f32, rem: f32) -> f32 {
    const FOOTER_WIDTH: f32 = 270.;
    rem * if width < FOOTER_WIDTH { 8.875 } else { 7.125 }
}
