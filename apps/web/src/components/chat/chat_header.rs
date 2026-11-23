//! The chat header (`ChatHeader.tsx`, `WorkspaceBreadcrumb.tsx`): the
//! project crumb and thread title in the titlebar row. The panel toggles at
//! its right edge belong to the window layout.

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::{
    AnyElement, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, div, px,
};

use super::{
    chat_style::{ChatColors, TEXT_SM},
};

/// Header side padding at ≥640px (`px-5`); 12px below.
pub const HEADER_PADDING: gpui_kit::Pixels = px(20.);
/// Room kept clear of the fixed toggles (`pr-24`).
const HEADER_TOGGLE_CLEARANCE: gpui_kit::Pixels = px(96.);
/// Breadcrumb gap (`gap-3`), project crumb gap (`gap-1.5`), favicon 14px.
const CRUMB_GAP: gpui_kit::Pixels = px(12.);
const PROJECT_CRUMB_GAP: gpui_kit::Pixels = px(6.);

/// The header row: project crumb, separator, thread title. `height` is the
/// window's topbar height; `leading_inset` clears the floating sidebar
/// toggle while the sidebar is collapsed.
pub fn chat_header(
    project: &SharedString,
    title: &SharedString,
    height: gpui_kit::Pixels,
    leading_inset: gpui_kit::Pixels,
    colors: &ChatColors,
) -> AnyElement {
    div()
        .flex_none()
        .h(height)
        .w_full()
        .flex()
        .items_center()
        .px(HEADER_PADDING)
        .pl(HEADER_PADDING + leading_inset)
        .bg(colors.theme.toolbar)
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(CRUMB_GAP)
                .pr(HEADER_TOGGLE_CLEARANCE)
                .text_size(TEXT_SM.size)
                .line_height(TEXT_SM.line_height)
                .font_weight(FontWeight::MEDIUM)
                .child(
                    div()
                        .id("project-crumb")
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(PROJECT_CRUMB_GAP)
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(colors.theme.muted_foreground)
                        .hover(|s| s.text_color(colors.theme.text))
                        .child(Icon::new(IconName::Folder).size(IconSize::Custom(px(14.))).color(colors.theme.muted_foreground))
                        .child(project.clone()),
                )
                .child(div().text_color(colors.theme.icon_muted).child("/"))
                .child(
                    div()
                        .id("thread-title")
                        .group("thread-title")
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .text_color(colors.theme.text)
                        .child(div().min_w_0().truncate().child(title.clone()))
                        .child(
                            div()
                                .opacity(0.)
                                .group_hover("thread-title", |s| s.opacity(1.))
                                .child(Icon::new(IconName::ChevronDown).size(IconSize::Custom(px(14.))).color(colors.theme.muted_foreground)),
                        ),
                ),
        )
        .into_any_element()
}
