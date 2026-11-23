//! Settings page building blocks. Ports `settingsLayout.tsx`
//! (`SettingsPageContainer`, `SettingsSection`, `SettingsRow`) and
//! `SettingsGroup.tsx`.

use gpui_kit::{AnyElement, App, Div, IntoElement, ParentElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::theme_color::with_alpha;

/// `SettingsPageContainer` / `WorkspacePageContainer width="readable"`:
/// `mx-auto max-w-4xl gap-6 px-6 pt-6 pb-12`.
pub fn page_container(cx: &App) -> Div {
    let theme = cx.app_theme();
    div()
        .mx_auto()
        .flex()
        .flex_col()
        .w_full()
        .max_w(theme.rem(56.))
        .gap(theme.sp(6.))
        .px(theme.sp(6.))
        .pt(theme.sp(6.))
        .pb(theme.sp(12.))
}

/// `SettingsSection variant="grouped"`: a titled card whose children are
/// separated by hairlines.
#[derive(IntoElement)]
pub struct SettingsSection {
    title: SharedString,
    header_action: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl SettingsSection {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { title: title.into(), header_action: None, children: Vec::new() }
    }

    pub fn header_action(mut self, action: impl IntoElement) -> Self {
        self.header_action = Some(action.into_any_element());
        self
    }
}

impl ParentElement for SettingsSection {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SettingsSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let divider = with_alpha(c.border, 0.5);
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(2.5))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(theme.sp(4.))
                    .min_h(theme.sp(7.))
                    .px(theme.sp(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_h(theme.sp(7.))
                            .ui_text(theme, TextSize::Base, weight::SEMIBOLD)
                            .text_color(c.foreground())
                            .child(self.title),
                    )
                    .children(self.header_action.map(|action| {
                        div().flex().items_center().justify_end().min_h(theme.sp(7.)).min_w(theme.sp(7.)).child(action)
                    })),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .rounded(theme.radii.xl)
                    .border_1()
                    .border_color(with_alpha(c.border, 0.6))
                    .bg(with_alpha(c.card(), 0.4))
                    .shadow(theme.shadows.xs5.clone())
                    .text_color(c.foreground())
                    .children(self.children.into_iter().enumerate().map(|(ix, child)| {
                        div().when(ix > 0, |d| d.border_t_1().border_color(divider)).child(child).into_any_element()
                    })),
            )
    }
}

/// `SettingsRow`: title, muted description, a trailing control, and
/// optional content below.
#[derive(IntoElement)]
pub struct SettingsRow {
    title: SharedString,
    description: Option<SharedString>,
    status: Option<AnyElement>,
    control: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl SettingsRow {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { title: title.into(), description: None, status: None, control: None, children: Vec::new() }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn status(mut self, status: impl IntoElement) -> Self {
        self.status = Some(status.into_any_element());
        self
    }

    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }
}

impl ParentElement for SettingsRow {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SettingsRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let has_children = !self.children.is_empty();
        div()
            .px(theme.sp(4.))
            .when(has_children, |d| d.pt(theme.sp(3.)).pb(theme.sp(1.)))
            .when(!has_children, |d| d.py(theme.sp(3.)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(8.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(theme.sp(1.))
                            .child(
                                div()
                                    .min_h(theme.sp(5.))
                                    .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                                    .text_color(c.foreground())
                                    .child(self.title),
                            )
                            .children(self.description.map(|text| {
                                div()
                                    .max_w(theme.rem(42.))
                                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                    .line_height(px(TextSize::Xs.size(theme.fonts.interface) * 1.5))
                                    .text_color(c.muted_foreground)
                                    .child(text)
                            }))
                            .children(self.status.map(|status| {
                                div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(status)
                            })),
                    )
                    .children(self.control.map(|control| div().flex().flex_none().items_center().gap(theme.sp(2.)).child(control))),
            )
            .when(has_children, |d| d.child(div().pt(theme.sp(3.)).children(self.children)))
    }
}
