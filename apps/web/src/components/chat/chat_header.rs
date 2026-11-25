//! The chat header (`ChatHeader.tsx`, `WorkspaceBreadcrumb.tsx`): the
//! project crumb and thread title in the titlebar row. The panel toggles at
//! its right edge belong to the window layout.

use crate::components::project_favicon::ProjectFavicon;
use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{rems,
    AnyElement, App, ClickEvent, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, TextRun, Window, canvas, div, px,
};

use super::{
    chat_style::{ChatColors, TEXT_SM},
};

/// Header side padding at ≥640px (`px-5`); 12px below.
pub const HEADER_PADDING: gpui_kit::Pixels = px(20.);
/// Room kept clear of the fixed toggles (`pr-24`).
const HEADER_TOGGLE_CLEARANCE: gpui_kit::Pixels = px(96.);
/// Breadcrumb gap (`gap-3`), project crumb gap (`gap-1.5`), favicon 14px. Header sizes here
/// are at a 16px rem and scale with the interface size.
const CRUMB_GAP: gpui_kit::Pixels = px(12.);
const PROJECT_CRUMB_GAP: gpui_kit::Pixels = px(6.);

/// The parent crumb of a provider subagent thread ("Open parent thread: <title>").
#[derive(Clone)]
pub struct ParentCrumb {
    pub title: SharedString,
    pub on_click: std::rc::Rc<dyn Fn(&mut Window, &mut App)>,
}

/// The thread title as the "Thread actions for <title>" menu trigger, with
/// the open menu drawn below it.
pub struct ThreadActionsTrigger {
    pub open: bool,
    pub on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
    pub menu: Option<AnyElement>,
}

/// The header row: project crumb, separator, the parent crumb of a subagent thread and its
/// separator, thread title. `height` is the window's topbar height; `leading_inset` is the left
/// padding that clears the floating sidebar toggle while the sidebar is collapsed
/// (`--workspace-titlebar-content-left`).
pub fn chat_header(
    project: &SharedString,
    project_icon: Option<&crate::components::project_favicon::ProjectIconSource>,
    title: &SharedString,
    height: gpui_kit::Pixels,
    leading_inset: gpui_kit::Pixels,
    colors: &ChatColors,
    parent: Option<&ParentCrumb>,
    actions: Option<ThreadActionsTrigger>,
) -> AnyElement {
    div()
        .flex_none()
        .h(height)
        .w_full()
        .flex()
        .items_center()
        .px(colors.r(HEADER_PADDING))
        // A collapsed sidebar's titlebar inset replaces the padding (`workspaceTitlebar.ts`);
        // while it animates in, the larger of the two holds.
        .pl(colors.r(HEADER_PADDING).max(leading_inset))
        .bg(colors.theme.toolbar)
        .child(Crumbs {
            project: project.clone(),
            project_icon: project_icon.cloned(),
            title: title.clone(),
            colors: *colors,
            parent: parent.cloned(),
            actions,
        })
        .into_any_element()
}

/// The crumb row. The parent crumb shows `...` while the crumbs at their natural widths do
/// not fit the row (`ChatHeader.tsx`'s `collapseParentTitle`), measured from the row's width
/// last frame as the fork measures it after layout.
#[derive(IntoElement)]
struct Crumbs {
    project: SharedString,
    project_icon: Option<crate::components::project_favicon::ProjectIconSource>,
    title: SharedString,
    colors: ChatColors,
    parent: Option<ParentCrumb>,
    actions: Option<ThreadActionsTrigger>,
}

/// The crumb row's content width last frame.
#[derive(Default)]
struct CrumbRowWidth(Option<Pixels>);

impl RenderOnce for Crumbs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self { project, project_icon, title, colors, parent, actions } = self;
        let colors = &colors;
        let separator = || div().flex_none().font_weight(FontWeight::NORMAL).text_color(colors.theme.icon_muted).child("/");
        let row_width = window.use_keyed_state("chat-header-crumbs", cx, |_, _| CrumbRowWidth::default());
        let collapse = parent.as_ref().is_some_and(|parent| {
            let size = TEXT_SM.size.to_pixels(window.rem_size());
            let text_width = |text: &SharedString, weight: FontWeight| {
                let run = TextRun { len: text.len(), font: super::chat_style::sans(weight), color: colors.theme.text, background_color: None, underline: None, strikethrough: None };
                window.text_system().layout_line(text.as_ref(), size, &[run], None).width
            };
            let slash: SharedString = "/".into();
            let natural = colors.r(px(14.)) + colors.r(PROJECT_CRUMB_GAP) + text_width(&project, FontWeight::MEDIUM)
                + text_width(&parent.title, FontWeight::MEDIUM)
                + text_width(&title, FontWeight::MEDIUM)
                + rems(0.25).to_pixels(window.rem_size()) + IconSize::Sm.pixels()
                + text_width(&slash, FontWeight::NORMAL) * 2.
                + colors.r(CRUMB_GAP) * 4.;
            row_width.read(cx).0.is_some_and(|width| natural > width)
        });
        let measured = row_width.clone();
        let clearance = colors.r(HEADER_TOGGLE_CLEARANCE);
        div()
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(colors.r(CRUMB_GAP))
            .pr(clearance)
            .text_size(TEXT_SM.size)
            .line_height(TEXT_SM.line_height)
            .font_weight(FontWeight::MEDIUM)
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let width = Some(bounds.size.width - clearance);
                        if measured.read(cx).0 != width {
                            measured.update(cx, |row, _| row.0 = width);
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                div()
                    .id("project-crumb")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(colors.r(PROJECT_CRUMB_GAP))
                    .rounded(rems(0.25))
                    .cursor_pointer()
                    .text_color(colors.theme.muted_foreground)
                    .hover(|s| s.text_color(colors.theme.text))
                    .child(ProjectFavicon::new(project.clone()).source(project_icon.as_ref()).size(colors.r(px(14.))))
                    .child(project.clone()),
            )
            .child(separator())
            .when_some(parent, |d, parent| {
                let on_click = parent.on_click.clone();
                d.child(
                    div()
                        .id("parent-crumb")
                        .flex()
                        .flex_shrink(1.)
                        .min_w_0()
                        .items_center()
                        .rounded(rems(0.25))
                        .cursor_pointer()
                        .text_color(colors.theme.muted_foreground)
                        .hover(|s| s.text_color(colors.theme.text))
                        .map(|d| if collapse { d.flex_none().child("...") } else { d.child(div().min_w_0().truncate().child(parent.title.clone())) })
                        .tooltip(crate::components::ui::tooltip::tooltip(parent.title.clone()))
                        .on_click(move |_, window, cx| on_click(window, cx)),
                )
                .child(separator())
            })
            .child({
                let open = actions.as_ref().is_some_and(|a| a.open);
                let (on_click, menu) = actions.map(|a| (a.on_click, a.menu)).unzip();
                div()
                    .id("thread-title")
                    .group("thread-title")
                    .relative()
                    .flex()
                    .min_w_0()
                    // The trigger button is 22px tall; the menu opens 4px below it.
                    .h(rems(1.375))
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(colors.theme.text)
                    .child(div().min_w_0().truncate().child(title.clone()))
                    .child(
                        div()
                            .opacity(if open { 1. } else { 0. })
                            .group_hover("thread-title", |s| s.opacity(1.))
                            .child(Icon::new(IconName::ChevronDown).size(IconSize::Sm).color(colors.theme.muted_foreground)),
                    )
                    .when_some(on_click, |d, on_click| d.on_click(move |e, w, cx| on_click(e, w, cx)))
                    .children(menu.flatten())
            })
    }
}
