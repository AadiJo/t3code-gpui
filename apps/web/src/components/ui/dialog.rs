//! Dialog. Ports `dialog.tsx` and `dialog-styles.ts`: a `dialog-glass` card
//! (radius 16px, max 512px) centred over a `dialog-backdrop`, with header,
//! panel and a tinted footer.

use crate::components::ui::GlassExt as _;
use std::rc::Rc;

use gpui_kit::{rems, 
    AnyElement, App, Div, ElementId, FocusHandle, IntoElement, ParentElement, RenderOnce, SharedString, Window,
    anchored, deferred, div, point, prelude::*, px,
};

use super::TextStyleExt as _;
use super::motion::Transition;
use super::sheet::SheetSide;
use super::button::{Button, ButtonSize, ButtonVariant};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize, IconSource};
use crate::{ActiveAppTheme as _, AppTheme};

type Close = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// The dialog card. Children go in the scrolling panel; `footer` holds buttons.
#[derive(IntoElement)]
pub struct DialogSurface {
    title: SharedString,
    title_icon: Option<IconSource>,
    /// `max-w-*` in rem; `max-w-lg` (32rem) unless set.
    max_width_rem: f32,
    /// `DialogFooter variant="bare"`: no rule or tinted band behind the buttons.
    bare_footer: bool,
    description: Option<SharedString>,
    children: Vec<AnyElement>,
    footer: Vec<AnyElement>,
    on_close: Option<Close>,
}

impl DialogSurface {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            title_icon: None,
            max_width_rem: 32.,
            bare_footer: false,
            description: None,
            children: Vec::new(),
            footer: Vec::new(),
            on_close: None,
        }
    }

    /// A 1rem icon before the title (`<Icon className="me-2 size-4" />`).
    pub fn title_icon(mut self, icon: impl Into<IconSource>) -> Self {
        self.title_icon = Some(icon.into());
        self
    }

    /// `DialogFooter variant="bare"`.
    pub fn bare_footer(mut self) -> Self {
        self.bare_footer = true;
        self
    }

    /// The card's `max-w-*` in rem (`max-w-xl` is 36).
    pub fn max_width_rem(mut self, rem: f32) -> Self {
        self.max_width_rem = rem;
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Adds a footer element, typically a button; footers align right.
    pub fn footer(mut self, element: impl IntoElement) -> Self {
        self.footer.push(element.into_any_element());
        self
    }

    /// Shows the close button and runs this when it is pressed.
    pub fn on_close(mut self, close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(close));
        self
    }
}

impl ParentElement for DialogSurface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// `dialog-glass`: background at the glass opacity over the blurred backdrop, 1px glass border, dialog shadow.
pub fn dialog_glass(theme: &AppTheme) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .rounded(theme.radii.xxl)
        .border_1()
        .border_color(theme.derived.dialog_glass_border)
        .glass(theme.derived.glass_surface)
        .shadow(theme.shadows.dialog.clone())
        .text_color(theme.foregrounds.popover)
}

impl RenderOnce for DialogSurface {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let has_panel = !self.children.is_empty();
        dialog_glass(theme)
            .w_full()
            .max_w(theme.rem(self.max_width_rem))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.))
                    .p(sp(6.))
                    .when(has_panel, |h| h.pb(sp(3.)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .ui_text(theme, TextSize::Xl, weight::SEMIBOLD)
                            .line_height(px(TextSize::Xl.size(theme.fonts.interface) * 1.375))
                            .children(self.title_icon.map(|icon| {
                                div().mr(sp(2.)).child(Icon::new(icon).size(IconSize::Custom(theme.rem(1.))))
                            }))
                            .child(self.title),
                    )
                    .children(self.description.map(|d| {
                        div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(d)
                    })),
            )
            .when(has_panel, |s| {
                s.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(4.))
                        .px(sp(6.))
                        .pt(sp(1.))
                        .pb(sp(6.))
                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                        .children(self.children),
                )
            })
            .when(!self.footer.is_empty(), |s| {
                s.child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(sp(2.))
                        .px(sp(6.))
                        .py(sp(4.))
                        .when(!self.bare_footer, |f| f.border_t_1().border_color(c.border).bg(theme.derived.dialog_footer))
                        .rounded_b(theme.radii.xxl - px(1.))
                        .children(self.footer),
                )
            })
            .when_some(self.on_close, |s, close| {
                s.child(
                    div().absolute().top(sp(2.)).right(sp(2.)).child(
                        Button::new("dialog-close")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Icon)
                            .icon(IconName::X)
                            .on_click(move |_, window, cx| close(window, cx)),
                    ),
                )
            })
    }
}

/// A full-window modal layer: backdrop plus centred content. A press on the
/// backdrop or Escape runs `on_close`. Render it only while the dialog is open.
#[derive(IntoElement)]
pub struct Modal {
    id: ElementId,
    content: AnyElement,
    on_close: Close,
    /// `None` centres a dialog; a side docks a sheet there.
    sheet: Option<SheetSide>,
}

impl Modal {
    pub fn dialog(id: impl Into<ElementId>, content: impl IntoElement, on_close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self { id: id.into(), content: content.into_any_element(), on_close: Rc::new(on_close), sheet: None }
    }

    pub(super) fn sheet(id: impl Into<ElementId>, side: SheetSide, content: impl IntoElement, on_close: Close) -> Self {
        Self { id: id.into(), content: content.into_any_element(), on_close, sheet: Some(side) }
    }
}

impl RenderOnce for Modal {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window.use_keyed_state((self.id.clone(), "focus"), cx, |_, cx| cx.focus_handle());
        let handle: FocusHandle = focus.read(cx).clone();
        // Take focus once, when the modal opens. Taking it back on every
        // render would steal it from a dialog opened on top of this one.
        let opened = window.use_keyed_state((self.id.clone(), "opened"), cx, |_, _| false);
        if !*opened.read(cx) {
            opened.update(cx, |opened, _| *opened = true);
            if !handle.contains_focused(window, cx) {
                window.focus(&handle, cx);
            }
        }
        let theme = cx.app_theme();
        // Sheets use `background/60 backdrop-blur-xs`; dialogs use `dialog-backdrop`.
        let backdrop = if self.sheet.is_some() {
            crate::theme_tokens::Glass::overlay(crate::theme_color::with_alpha(theme.colors.background(), 0.6))
        } else {
            theme.derived.glass_dialog_backdrop
        };
        let viewport = window.viewport_size();
        let (on_backdrop, on_escape) = (self.on_close.clone(), self.on_close);
        // Backdrop and popup both fade in over 200ms (`data-starting-style:opacity-0`).
        deferred(
            anchored().position(point(px(0.), px(0.))).child(Transition::DIALOG.fade_in("modal-enter",
                div()
                    .id(self.id)
                    .track_focus(&handle)
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .relative()
                    .on_key_down(move |event, window, cx| {
                        if event.keystroke.key == "escape" {
                            on_escape(window, cx);
                        }
                    })
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .glass(backdrop)
                            .on_mouse_down(gpui_kit::MouseButton::Left, move |_, window, cx| on_backdrop(window, cx)),
                    )
                    .child(match self.sheet {
                        // `w-[calc(100%-48px)] max-w-md`.
                        Some(side) => div()
                            .absolute()
                            .size_full()
                            .flex()
                            .when(side == SheetSide::Right, |d| d.justify_end())
                            .child({
                                // `data-starting-style:translate-x-8` (left: `-translate-x-8`).
                                let slide = if side == SheetSide::Right { rems(2.) } else { rems(-2.) };
                                let panel = div()
                                    .relative()
                                    .occlude()
                                    .h_full()
                                    .w((viewport.width - px(48.)).min(theme.rem(28.)))
                                    .child(self.content);
                                Transition::DIALOG.run("sheet-slide", panel, move |p, t| p.left(slide * (1. - t)))
                            }),
                        None => div()
                            .absolute()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .p(theme.sp(4.))
                            // Presses inside the dialog must not reach the backdrop.
                            .child(div().occlude().w_full().flex().justify_center().child(self.content)),
                    }),
            )),
        )
        .with_priority(3)
    }
}
