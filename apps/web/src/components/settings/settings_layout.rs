//! Settings page building blocks. Ports `settingsLayout.tsx`
//! (`SettingsPageContainer`, `SettingsSection`, `SettingsRow`) and
//! `SettingsGroup.tsx`.

use gpui_kit::{AnyElement, App, Div, Global, IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Window, div, prelude::*, px};

use super::settings_search::SettingsPath;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::theme_color::with_alpha;

/// The settings page on screen, set by the settings route so every page's
/// container knows whether it carries the scope sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrentSettingsPath(pub SettingsPath);

impl Global for CurrentSettingsPath {}

/// `SettingsPageContainer`: `WorkspacePageContainer width="readable"` with
/// `gap-8` (`mx-auto max-w-4xl gap-8 px-6 pt-6 pb-12`), opened by the
/// `SettingsScopeSentence` on every page that is not saved on this device.
/// The settings scroll area reserves the 6px scrollbar gutter on both sides
/// (`scrollbar-gutter-both`).
pub fn page_container(cx: &App) -> Div {
    let theme = cx.app_theme();
    let scoped = cx.try_global::<CurrentSettingsPath>().is_some_and(|path| !path.0.is_device_only());
    div()
        .mx_auto()
        .flex()
        .flex_col()
        .w_full()
        .max_w(theme.rem(56.))
        .gap(theme.sp(8.))
        .px(theme.sp(6.))
        .pt(theme.sp(6.))
        .pb(theme.sp(12.))
        .when(scoped, |d| d.child(super::settings_controls::scope_sentence(cx)))
}

/// Opens another settings page, for links inside a page. The settings
/// route installs it while it is open.
#[derive(Clone)]
pub struct SettingsNavigator(pub std::rc::Rc<dyn Fn(SettingsPath, &mut App)>);

impl Global for SettingsNavigator {}

impl SettingsNavigator {
    /// Opens `path` in the open settings modal, if there is one.
    pub fn open(path: SettingsPath, cx: &mut App) {
        if let Some(navigator) = cx.try_global::<Self>().cloned() {
            (navigator.0)(path, cx);
        }
    }
}

/// The width the settings scroll area reserves for its scrollbar on each
/// side (`--app-scrollbar-width`, `scrollbar-gutter: stable both-edges`).
pub const SCROLLBAR_GUTTER: f32 = 6.;

/// `SettingsSection variant="grouped"`: a titled card whose children are
/// separated by hairlines.
#[derive(IntoElement)]
pub struct SettingsSection {
    title: SharedString,
    icon: Option<AnyElement>,
    title_action: Option<AnyElement>,
    header_action: Option<AnyElement>,
    plain: bool,
    hide_title: bool,
    children: Vec<AnyElement>,
}

impl SettingsSection {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            icon: None,
            title_action: None,
            header_action: None,
            plain: false,
            hide_title: false,
            children: Vec::new(),
        }
    }

    /// `icon`: drawn before the title, 8px from it.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// `titleAction`: drawn right after the title, 4px from it.
    pub fn title_action(mut self, action: impl IntoElement) -> Self {
        self.title_action = Some(action.into_any_element());
        self
    }

    /// `variant="plain"`: no card, children stacked 4px apart.
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }

    /// `hideTitle`: the title is for screen readers only.
    pub fn hide_title(mut self) -> Self {
        self.hide_title = true;
        self
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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let divider = with_alpha(c.border, 0.5);
        let mut grid = crate::pixel_grid::PixelGrid::new(window);
        let header_height = grid.next(theme.sp(7.));
        let header_gap = grid.next(theme.sp(2.5));
        let header = (!self.hide_title).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_start()
                .justify_between()
                .gap_x(theme.sp(4.))
                .gap_y(theme.sp(1.))
                .min_h(header_height)
                .px(theme.sp(4.))
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(theme.sp(1.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.sp(2.))
                                .min_h(theme.sp(7.))
                                .ui_text(theme, TextSize::Base, weight::SEMIBOLD)
                                .text_color(c.foreground())
                                .children(self.icon)
                                .child(self.title),
                        )
                        .children(self.title_action),
                )
                .children(self.header_action.map(|action| {
                    div().flex().items_center().justify_end().min_h(theme.sp(7.)).min_w(theme.sp(7.)).child(action)
                }))
        });
        let group = if self.plain {
            div().flex().flex_col().gap(theme.sp(1.)).text_color(c.foreground()).children(self.children)
        } else {
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
                }))
        };
        div().flex().flex_col().gap(if self.hide_title { theme.sp(2.5) } else { header_gap }).children(header).child(group)
    }
}

/// The hover group every settings row opens (`group/settings-row`).
pub const SETTINGS_ROW_GROUP: &str = "settings-row";

/// `SettingsRow`: title, muted description, a trailing control, and
/// optional content below.
#[derive(IntoElement)]
pub struct SettingsRow {
    title: SharedString,
    title_suffix: Option<AnyElement>,
    title_icon: Option<crate::components::icons::IconName>,
    reset: Option<AnyElement>,
    disabled: bool,
    description: Option<SharedString>,
    description_element: Option<AnyElement>,
    inheritance: Option<AnyElement>,
    status: Option<AnyElement>,
    control: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl SettingsRow {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            title_suffix: None,
            title_icon: None,
            reset: None,
            disabled: false,
            description: None,
            description_element: None,
            inheritance: None,
            status: None,
            control: None,
            children: Vec::new(),
        }
    }

    /// Replaces the title, for a row titled differently from its catalog entry.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self
    }

    /// Drawn inside the title, 8px after its text, on its baseline (the
    /// About page's version copy button).
    pub fn title_suffix(mut self, suffix: impl IntoElement) -> Self {
        self.title_suffix = Some(suffix.into_any_element());
        self
    }

    /// A 14px muted icon before the title text, 8px from it.
    pub fn title_icon(mut self, icon: crate::components::icons::IconName) -> Self {
        self.title_icon = Some(icon);
        self
    }

    /// The reset button beside the title, shown while the value is not its default.
    pub fn reset(mut self, reset: impl IntoElement) -> Self {
        self.reset = Some(reset.into_any_element());
        self
    }

    /// `aria-disabled`: the row dims to 64%.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// A description with inline links, drawn in the description's style.
    /// Replaces a plain description.
    pub fn description_element(mut self, description: impl IntoElement) -> Self {
        self.description = None;
        self.description_element = Some(description.into_any_element());
        self
    }

    /// `SettingInheritance`: the marker before the reset slot on rows saved
    /// on a server. `quiet` markers (a built-in default or a value set right
    /// here) show only while the row is hovered.
    pub fn inheritance(mut self, marker: impl IntoElement, quiet: bool) -> Self {
        self.inheritance = Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(20.))
                .when(quiet, |d| d.opacity(0.).group_hover(SETTINGS_ROW_GROUP, |s| s.opacity(1.)))
                .child(marker)
                .into_any_element(),
        );
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

/// A settings row's vertical lengths on the device-pixel grid (`pixel_grid`): `pt-3`, one
/// line of title (`min-h-5`, 2px taller with an inline icon), `gap-1`, then `pb-3` (`pb-1`
/// above nested content). A wrapped description keeps its measured height.
pub struct RowMetrics {
    pub top: Pixels,
    pub title_height: Pixels,
    pub gap: Pixels,
    pub bottom: Pixels,
}

impl RowMetrics {
    pub fn new(window: &Window, theme: &crate::AppTheme, title_icon: bool, nested: bool) -> Self {
        let mut grid = crate::pixel_grid::PixelGrid::new(window);
        Self {
            top: grid.next(theme.sp(3.)),
            title_height: grid.next(theme.sp(5.) + if title_icon { px(2.) } else { px(0.) }),
            gap: grid.next(theme.sp(1.)),
            bottom: grid.next(if nested { theme.sp(1.) } else { theme.sp(3.) }),
        }
    }
}

impl RenderOnce for SettingsRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let has_children = !self.children.is_empty();
        let RowMetrics { top, title_height, gap, bottom } =
            RowMetrics::new(window, theme, self.title_icon.is_some(), has_children);
        div()
            .group(SETTINGS_ROW_GROUP)
            .px(theme.sp(4.))
            .when(self.disabled, |d| d.opacity(0.64))
            .pt(top)
            .pb(bottom)
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
                            .gap(gap)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.sp(1.5))
                                    .h(title_height)
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(theme.sp(2.))
                                            // An inline icon makes the web's `h3` 2px taller than its line.
                                            .h(title_height)
                                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                                            .text_color(c.foreground())
                                            .children(self.title_icon.map(|icon| {
                                                div().self_center().child(
                                                    crate::components::icons::Icon::new(icon)
                                                        .size(crate::components::icons::IconSize::Sm)
                                                        .color(c.muted_foreground),
                                                )
                                            }))
                                            .child(self.title)
                                            .children(self.title_suffix),
                                    )
                                    .children(self.inheritance)
                                    .child(
                                        div()
                                            .flex()
                                            .flex_none()
                                            .items_center()
                                            .justify_center()
                                            .size(theme.sp(5.))
                                            .children(self.reset),
                                    ),
                            )
                            .children(self.description.map(|text| {
                                div()
                                    .max_w(theme.rem(42.))
                                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                    .line_height(px(TextSize::Xs.size(theme.fonts.interface) * 1.5))
                                    .text_color(c.muted_foreground)
                                    .child(text)
                            }))
                            .children(self.description_element.map(|element| {
                                div()
                                    .max_w(theme.rem(42.))
                                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                    .line_height(px(TextSize::Xs.size(theme.fonts.interface) * 1.5))
                                    .text_color(c.muted_foreground)
                                    .child(element)
                            }))
                            .children(self.status.map(|status| {
                                div().pt(theme.sp(0.5)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(status)
                            })),
                    )
                    .children(self.control.map(|control| div().flex().flex_none().items_center().gap(theme.sp(2.)).child(control))),
            )
            .when(has_children, |d| d.children(self.children))
    }
}
