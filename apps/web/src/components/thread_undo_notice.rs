//! The sidebar footer's undo notice (`SidebarThreadUndoNotice.tsx`): "Settled, ⌘Z to undo"
//! in the free space of the utility row, while [`ThreadUndo`] has a notice.

use gpui_kit::*;
use t3code_shared::keybindings::{KeybindingCommand, StaticCommand};

use crate::ActiveAppTheme as _;
use crate::components::sidebar::style::ShellColors;
use crate::i18n::t;
use crate::keybindings::shortcut_label_for_command;
use crate::shortcuts::keymap::ShortcutState;
use crate::thread_undo::ThreadUndo;

/// `threadUndoLabel`: "⌘Z to undo", or "Undo" with no shortcut.
pub fn thread_undo_label(cx: &App) -> SharedString {
    let label = cx.try_global::<ShortcutState>().and_then(|state| {
        shortcut_label_for_command(&state.config, &KeybindingCommand::Static(StaticCommand::ThreadUndo), state.state, state.platform)
    });
    match label {
        Some(shortcut) => t!("{shortcut} to undo", shortcut = shortcut.as_ref()),
        None => t!("Undo"),
    }
}

/// The notice, filling the row's free space (`flex-1`, `h-6`, `text-2xs`, `leading-6`), or
/// nothing when there is nothing to undo. As in the fork, a reversed wrapping row: a summary
/// that does not fit beside the button drops to a second line the row's height clips, and a
/// label that does not fit drops away the same way, leaving the undo icon.
pub fn thread_undo_notice(colors: &ShellColors, cx: &App) -> Option<AnyElement> {
    let (action, count) = ThreadUndo::notice(cx)?;
    let theme = cx.app_theme();
    let line = theme.rem(1.5);
    let button = div()
        .id("thread-undo-button")
        .min_w_0()
        .flex_shrink(1.)
        .cursor_pointer()
        .font(crate::appearance_fonts::ui_font(crate::appearance_fonts::weight::MEDIUM))
        .text_color(colors.foreground)
        .hover(|s| s.underline())
        .child(
            div()
                .flex()
                .h(line)
                .min_w_0()
                .flex_wrap()
                .justify_end()
                .overflow_hidden()
                // The icon's negative margin cancels its width, so beside the label it sits in
                // clipped space; once the label wraps away, the icon shows.
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(line)
                        .w(theme.sp(4.))
                        .ml(-theme.sp(4.))
                        .child(crate::components::icons::Icon::new(crate::components::icons::IconName::Undo2).size(crate::components::icons::IconSize::Custom(theme.sp(4.))).color(colors.foreground)),
                )
                .child(thread_undo_label(cx)),
        )
        .on_click(|_, _, cx| {
            ThreadUndo::undo_latest(cx);
        });
    Some(
        div()
            .id("thread-undo-notice")
            .flex()
            .flex_row_reverse()
            .flex_wrap()
            .flex_1()
            .min_w_0()
            .h(line)
            .gap_x(theme.sp(1.))
            .overflow_hidden()
            .whitespace_nowrap()
            .font(crate::appearance_fonts::ui_font(crate::appearance_fonts::weight::NORMAL))
            .text_size(theme.rem(0.6875))
            .line_height(line)
            .text_color(colors.sidebar_muted_foreground)
            .child(button)
            .child(div().child(action.short_summary(count)))
            .into_any_element(),
    )
}
