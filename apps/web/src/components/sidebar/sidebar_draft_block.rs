//! Unsent new-thread drafts above the thread list. Ports `SidebarDraftBlock.tsx`:
//! a card per draft (pen, project favicon and name, the prompt's first line,
//! a discard button on hover) and a divider under the block.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::model::SidebarDraft;
use super::sidebar_thread_row::project_favicon;
use super::style::*;
use super::{Sidebar, SidebarEvent};
use crate::components::icons::{Icon, IconName, IconSize};

impl Sidebar {
    /// One draft card (`SidebarDraftRow`).
    pub(crate) fn render_draft_row(
        &mut self,
        ix: usize,
        draft: &SidebarDraft,
        colors: &ShellColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hovered = self.hovered == Some(ix);
        let active = self.snapshot.active_draft.as_ref() == Some(&draft.id);
        let focused = self.keyboard_nav && self.cursor == Some(ix) && self.focus_handle.is_focused(window);
        let label = self.list.project_labels.get(&draft.project).cloned().unwrap_or_default();
        let title = self.list.project_titles.get(&draft.project).cloned().unwrap_or_else(|| label.clone());
        let fill = if active {
            colors.sidebar_row_active
        } else {
            colors.warning.opacity(if hovered { 0.08 } else { 0.04 })
        };
        let id = draft.id.clone();
        let discard_id = draft.id.clone();
        let line1 = div()
            .flex()
            .h(colors.r(20.))
            .min_w_0()
            .items_center()
            .gap(colors.r(6.))
            .child(Icon::new(IconName::SquarePen).size(IconSize::Custom(colors.r(12.))).color(colors.warning_foreground))
            .child(project_favicon(title, self.list.project_icons.get(&draft.project), colors))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .line_height(colors.r(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.secondary_label)
                    .child(label),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .h(colors.r(20.))
                    .min_w(colors.r(20.))
                    .flex_none()
                    .items_center()
                    .justify_end()
                    .when(hovered, |d| {
                        d.child(
                            div()
                                .id(("discard-draft-session", ix))
                                .occlude()
                                .flex()
                                .items_center()
                                .px(colors.r(4.))
                                .rounded(colors.s(ROW_RADIUS))
                                .cursor_pointer()
                                .text_color(colors.muted_foreground)
                                .child(Icon::new(IconName::X).size(IconSize::Custom(colors.r(12.))).color(colors.muted_foreground))
                                .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!("Discard draft")))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(SidebarEvent::DiscardDraftSession(discard_id.clone()));
                                })),
                        )
                    }),
            );
        let line2 = div()
            .mt(colors.r(2.))
            .truncate()
            .text_sm()
            .line_height(colors.r(20.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(colors.foreground.opacity(0.9))
            .child(draft.preview.clone());
        div()
            .py(colors.s(CARD_ROW_PADDING_Y))
            .child(
                div()
                    .id(("sidebar-draft", ix))
                    .relative()
                    .w_full()
                    .overflow_hidden()
                    .rounded(colors.s(ROW_RADIUS))
                    .cursor_pointer()
                    .bg(fill)
                    .when(focused, |d| d.border_2().border_color(colors.ring))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.set_row_hover(ix, *hovered, cx)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                        this.cursor = Some(ix);
                        this.keyboard_nav = false;
                        this.activate_draft(id.clone(), cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| this.open_entry_menu(ix, event.position, cx)),
                    )
                    .child(
                        div()
                            .h(colors.s(CARD_ROW_HEIGHT))
                            .px(colors.s(ROW_CONTENT_INSET))
                            .py(colors.s(CONTENT_INSET))
                            .flex()
                            .flex_col()
                            .child(line1)
                            .child(line2),
                    ),
            )
            .into_any_element()
    }
}

/// The rule under the drafts: `mx-2.5 my-1.5 h-px bg-sidebar-border/60`.
pub(crate) fn draft_divider(colors: &ShellColors) -> AnyElement {
    div()
        .py(colors.r(6.))
        .px(colors.r(10.))
        .child(div().h(px(1.)).bg(colors.sidebar_border.opacity(0.6)))
        .into_any_element()
}
