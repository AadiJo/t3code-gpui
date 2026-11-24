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
/// nothing when there is nothing to undo.
pub fn thread_undo_notice(colors: &ShellColors, cx: &App) -> Option<AnyElement> {
    let (action, count) = ThreadUndo::notice(cx)?;
    let theme = cx.app_theme();
    let button = div()
        .id("thread-undo-button")
        .flex_shrink_0()
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .text_color(colors.foreground)
        .hover(|s| s.underline())
        .child(thread_undo_label(cx))
        .on_click(|_, _, cx| {
            ThreadUndo::undo_latest(cx);
        });
    Some(
        div()
            .id("thread-undo-notice")
            .flex()
            .flex_1()
            .min_w_0()
            .h(theme.rem(1.5))
            .justify_end()
            .gap(theme.sp(1.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(theme.rem(0.6875))
            .line_height(theme.rem(1.5))
            .text_color(colors.sidebar_muted_foreground)
            .child(div().min_w_0().child(action.short_summary(count)))
            .child(button)
            .into_any_element(),
    )
}
