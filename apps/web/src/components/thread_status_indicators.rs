//! Thread status indicators: the nine sidebar statuses (`05-ui-shell.md` 6.8),
//! folder dots and the terminal activity icon. Ports the indicator parts of
//! `ThreadStatusIndicators.tsx` and `SidebarThreadRow.tsx:560-574`.

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::*;

use super::sidebar::style::ShellColors;
use super::sidebar_logic::{FolderDot, TopStatus};

/// Icon, label and colour of a status.
#[derive(Clone, Copy, Debug)]
pub struct StatusLook {
    pub icon: IconName,
    pub label: &'static str,
    pub color: Hsla,
}

/// How a [`TopStatus`] looks (table 6.8).
pub fn status_look(status: TopStatus, colors: &ShellColors) -> StatusLook {
    let look = |icon, label, color| StatusLook { icon, label, color };
    match status {
        TopStatus::Working { preparing: true, .. } => {
            look(IconName::CircleDashed, "Setting up", colors.info_foreground)
        }
        TopStatus::Working { goal: true, .. } => {
            look(IconName::CircleDashed, "Goal", colors.info_foreground)
        }
        TopStatus::Working { .. } => look(IconName::CircleDashed, "Working", colors.info_foreground),
        TopStatus::Waiting => look(IconName::CircleDotDashed, "Waiting", colors.muted_foreground),
        TopStatus::Approval => look(IconName::ShieldQuestionMark, "Approval", colors.warning_foreground),
        TopStatus::Input => look(IconName::MessageCircleQuestionMark, "Input", colors.input_status),
        TopStatus::Request => look(IconName::UserRound, "Request", colors.guest_foreground),
        TopStatus::Limited => look(IconName::CircleAlert, "Limited", colors.warning_foreground),
        TopStatus::Failed => look(IconName::CircleAlert, "Failed", colors.error_foreground),
        TopStatus::Woke => look(IconName::AlarmClock, "Woke", colors.warning_foreground),
        TopStatus::Done => look(IconName::CircleCheck, "Done", colors.success_foreground),
    }
}

/// Status icon + label (`inline-flex items-center gap-1 text-xs font-medium`),
/// with an optional ticking duration after the label.
pub fn status_indicator(
    status: TopStatus,
    icon_size: Pixels,
    duration: Option<SharedString>,
    colors: &ShellColors,
) -> Div {
    let look = status_look(status, colors);
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(look.color)
        .child(Icon::new(look.icon).size(IconSize::Custom(icon_size)).color(look.color))
        .child(look.label)
        .children(duration)
}

/// The 6px dot on a collapsed project folder.
pub fn folder_dot(dot: FolderDot, colors: &ShellColors) -> Div {
    let color = match dot {
        FolderDot::Attention => colors.warning,
        FolderDot::Failed => colors.error,
        FolderDot::Unread => colors.success,
        FolderDot::Working => colors.info,
    };
    div().size(px(6.)).flex_none().rounded_full().bg(color)
}

/// 12px terminal icon for threads with running terminal processes. It pulses
/// (`animate-status-pulse`, 2s, opacity 1 to 0.5 in six steps) when `phase` is
/// driven by the caller's stepped clock; pass `None` to draw it still.
pub fn terminal_status_icon(phase: Option<u8>, colors: &ShellColors) -> Icon {
    let opacity = phase.map_or(1., |step| {
        let t = f32::from(step % 6) / 5.;
        1. - 0.5 * (1. - (2. * t - 1.).abs())
    });
    Icon::new(IconName::Terminal).size(IconSize::Custom(px(12.))).color(colors.terminal_running.opacity(opacity))
}
