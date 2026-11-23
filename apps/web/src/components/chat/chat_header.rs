//! The 52px chat header (`ChatHeader.tsx`, `WorkspaceBreadcrumb.tsx`) and the
//! titlebar panel toggles at its right edge (`PanelLayoutControls.tsx`).
//! The panels themselves do not exist yet; the toggles only flip their state.

use gpui_kit::{
    AnyElement, App, ClickEvent, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use super::{
    chat_icons::{ChatIcon, icon},
    chat_style::{ChatColors, TEXT_SM},
};

/// `--workspace-topbar-height`.
pub const HEADER_HEIGHT: gpui_kit::Pixels = px(52.);
/// Header side padding at ≥640px (`px-5`); 12px below.
pub const HEADER_PADDING: gpui_kit::Pixels = px(20.);
/// Room kept clear of the fixed toggles (`pr-24`).
const HEADER_TOGGLE_CLEARANCE: gpui_kit::Pixels = px(96.);
/// Breadcrumb gap (`gap-3`), project crumb gap (`gap-1.5`), favicon 14px.
const CRUMB_GAP: gpui_kit::Pixels = px(12.);
const PROJECT_CRUMB_GAP: gpui_kit::Pixels = px(6.);
/// Toggle buttons: 28×28, radius 8px, 16px icons, `gap-1`, 12px from the right.
const TOGGLE_SIZE: gpui_kit::Pixels = px(28.);
const TOGGLE_ICON: gpui_kit::Pixels = px(16.);
const TOGGLE_GAP: gpui_kit::Pixels = px(4.);
const TOGGLES_RIGHT: gpui_kit::Pixels = px(12.);

/// Which side panels are open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelState {
    pub thread_details: bool,
    pub terminal: bool,
    pub right_panel: bool,
}

/// A panel toggle in the titlebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelToggle {
    ThreadDetails,
    Terminal,
    RightPanel,
}

/// The header row: project crumb, separator, thread title.
pub fn chat_header(project: &SharedString, title: &SharedString, colors: &ChatColors) -> AnyElement {
    div()
        .flex_none()
        .h(HEADER_HEIGHT)
        .w_full()
        .flex()
        .items_center()
        .px(HEADER_PADDING)
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
                        .child(icon(ChatIcon::Folder, px(14.), colors.theme.muted_foreground))
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
                                .child(icon(ChatIcon::ChevronDown, px(14.), colors.theme.muted_foreground)),
                        ),
                ),
        )
        .into_any_element()
}

/// The toggles fixed at the titlebar's right: thread details, terminal, right panel.
pub fn panel_toggles(
    state: PanelState,
    colors: &ChatColors,
    on_toggle: impl Fn(PanelToggle, &mut Window, &mut App) + Clone + 'static,
) -> AnyElement {
    let button = |toggle: PanelToggle, glyph: ChatIcon, pressed: bool| {
        let on_toggle = on_toggle.clone();
        div()
            .id(match toggle {
                PanelToggle::ThreadDetails => "toggle-thread-details",
                PanelToggle::Terminal => "toggle-terminal",
                PanelToggle::RightPanel => "toggle-right-panel",
            })
            .size(TOGGLE_SIZE)
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .when_pressed(pressed, colors)
            .hover(|s| s.bg(colors.theme.toolbar_control_hover))
            .child(icon(glyph, TOGGLE_ICON, colors.theme.muted_foreground))
            .on_click(move |_: &ClickEvent, window, cx| on_toggle(toggle, window, cx))
    };
    div()
        .absolute()
        .top_0()
        .right(TOGGLES_RIGHT)
        .h(HEADER_HEIGHT)
        .flex()
        .items_center()
        .gap(TOGGLE_GAP)
        .child(button(PanelToggle::ThreadDetails, ChatIcon::SquareMenu, state.thread_details))
        .child(button(
            PanelToggle::Terminal,
            if state.terminal { ChatIcon::PanelBottomClose } else { ChatIcon::PanelBottom },
            state.terminal,
        ))
        .child(button(
            PanelToggle::RightPanel,
            if state.right_panel { ChatIcon::PanelRightClose } else { ChatIcon::PanelRight },
            state.right_panel,
        ))
        .into_any_element()
}

trait PressedExt {
    fn when_pressed(self, pressed: bool, colors: &ChatColors) -> Self;
}

impl PressedExt for gpui_kit::Stateful<gpui_kit::Div> {
    /// A pressed toggle keeps the hover fill.
    fn when_pressed(self, pressed: bool, colors: &ChatColors) -> Self {
        if pressed { self.bg(colors.theme.toolbar_control_hover) } else { self }
    }
}
