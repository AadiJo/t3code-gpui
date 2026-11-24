//! Select. Ports `select.tsx`: an input-like trigger with a trailing chevron
//! that opens a glass list of options; the chosen option is tinted.

use std::rc::Rc;

use gpui_kit::{rems, AnyElement, App, ElementId, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use super::menu::{MenuItem, MenuItemKind, MenuPopup};
use super::popover::{Popover, PopupHandle};
use super::{ControlState, DISABLED_OPACITY, TextStyleExt as _, ControlHighlightExt as _, input_focus_ring};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::{ActiveAppTheme as _, AppTheme};

/// `selectTriggerVariants.variant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectVariant {
    #[default]
    Default,
    Ghost,
}

/// `selectTriggerVariants.size`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectSize {
    Xs,
    Sm,
    #[default]
    Default,
    Compact,
    Lg,
}

impl SelectSize {
    pub const ALL: [SelectSize; 5] = [Self::Xs, Self::Sm, Self::Default, Self::Compact, Self::Lg];
}

/// The trigger button alone, also drawn statically by the gallery.
#[derive(IntoElement)]
pub struct SelectTrigger {
    label: SharedString,
    icon: Option<IconName>,
    leading: Option<AnyElement>,
    fit: bool,
    variant: SelectVariant,
    size: SelectSize,
    state: ControlState,
    open: bool,
}

impl SelectTrigger {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            leading: None,
            fit: false,
            variant: SelectVariant::Default,
            size: SelectSize::Default,
            state: ControlState::Idle,
            open: false,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: SelectVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: SelectSize) -> Self {
        self.size = size;
        self
    }

    /// Drawn before the value, such as the diff colour dots.
    pub fn leading(mut self, leading: Option<AnyElement>) -> Self {
        self.leading = leading;
        self
    }

    /// `w-auto min-w-0`: as wide as the value instead of filling its parent.
    pub fn fit(mut self, fit: bool) -> Self {
        self.fit = fit;
        self
    }

    pub fn state(mut self, state: ControlState) -> Self {
        self.state = state;
        self
    }

    /// `data-pressed` while the list is open.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
}

fn trigger_geometry(theme: &AppTheme, size: SelectSize) -> (gpui_kit::Pixels, gpui_kit::Pixels, gpui_kit::Pixels, TextSize) {
    let sp = |n: f32| theme.sp(n);
    match size {
        SelectSize::Xs => (sp(6.), sp(2.) - px(1.), theme.radii.md, TextSize::Xs),
        SelectSize::Sm => (sp(7.), sp(2.5) - px(1.), theme.radii.lg, TextSize::Sm),
        SelectSize::Default => (sp(8.), sp(3.) - px(1.), theme.radii.lg, TextSize::Sm),
        SelectSize::Compact => (sp(7.), sp(2.) - px(1.), theme.radii.md, TextSize::Xs),
        SelectSize::Lg => (sp(9.), sp(3.) - px(1.), theme.radii.lg, TextSize::Sm),
    }
}

impl RenderOnce for SelectTrigger {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let (height, padding, radius, text) = trigger_geometry(theme, self.size);
        let focused = self.state == ControlState::FocusVisible;
        let disabled = self.state == ControlState::Disabled;
        let hovered = matches!(self.state, ControlState::Hover | ControlState::Active) || self.open;
        let trigger = div()
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .gap(match self.size {
                SelectSize::Xs | SelectSize::Compact => theme.sp(1.),
                SelectSize::Sm => theme.sp(1.5),
                _ => theme.sp(2.),
            })
            .min_h(height)
            .px(padding)
            .rounded(radius)
            .border_1()
            .ui_text(theme, text, weight::NORMAL)
            .when(disabled, |t| t.opacity(DISABLED_OPACITY));
        let trigger = match self.variant {
            // `w-full min-w-36`, or `w-auto min-w-0` when it fits its value.
            SelectVariant::Default => trigger
                .when(!self.fit, |t| t.w_full().min_w(theme.rem(9.)))
                .border_color(if focused { c.ring() } else { c.input })
                .bg(theme.derived.input_fill)
                .text_color(c.foreground())
                .shadow(if focused {
                    vec![input_focus_ring(theme.derived.input_focus_ring)]
                } else if disabled || self.open {
                    vec![]
                } else {
                    theme.shadows.xs5.clone()
                })
                .when(!focused && !disabled && !self.open, |t| t.control_highlight(theme)),
            SelectVariant::Ghost => trigger
                .border_color(gpui_kit::Hsla::transparent_black())
                .when(hovered, |t| t.bg(c.highlight()))
                .text_color(if hovered { c.foreground() } else { c.secondary_label })
                .when(focused, |t| t.shadow(super::focus_ring_inset(theme, super::BORDER))),
        };
        trigger
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .min_w_0()
                    .children(self.icon.map(|icon| Icon::new(icon).size(IconSize::Md).color(c.icon_muted).opacity(0.8)))
                    .children(self.leading)
                    .child(div().truncate().child(self.label)),
            )
            .child(
                div().mr(rems(-0.25)).child(Icon::new(IconName::ChevronDown).size(IconSize::Sm).opacity(0.7).color(
                    match self.variant {
                        SelectVariant::Default => c.foreground(),
                        SelectVariant::Ghost if hovered => c.foreground(),
                        SelectVariant::Ghost => c.secondary_label,
                    },
                )),
            )
    }
}

/// The option list: menu rows with the chosen one tinted (`foreground/8%`).
pub fn select_list(options: &[SharedString], selected: usize, on_pick: Rc<dyn Fn(usize, &mut Window, &mut App)>) -> MenuPopup {
    options.iter().enumerate().fold(MenuPopup::new().min_width(rems(9.)), |menu, (index, option)| {
        let on_pick = on_pick.clone();
        menu.item(
            MenuItem::new(option.clone())
                .kind(MenuItemKind::Radio(index == selected))
                .on_select(move |window, cx| on_pick(index, window, cx)),
        )
    })
}

/// A select with string options.
#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    options: Rc<[SharedString]>,
    selected: usize,
    variant: SelectVariant,
    size: SelectSize,
    value_label: Option<SharedString>,
    leading: Option<AnyElement>,
    fit: bool,
    on_change: Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>,
}

impl Select {
    pub fn new(
        id: impl Into<ElementId>,
        options: impl Into<Rc<[SharedString]>>,
        selected: usize,
        on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            options: options.into(),
            selected,
            variant: SelectVariant::Default,
            size: SelectSize::Default,
            value_label: None,
            leading: None,
            fit: false,
            on_change: Rc::new(on_change),
        }
    }

    pub fn variant(mut self, variant: SelectVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: SelectSize) -> Self {
        self.size = size;
        self
    }
}

impl Select {
    /// `SelectValue` children: the trigger's text when it differs from the
    /// chosen item's, as "Comfortable" for "Comfortable (default)".
    pub fn value_label(mut self, label: impl Into<SharedString>) -> Self {
        self.value_label = Some(label.into());
        self
    }

    /// Drawn before the trigger's value.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// `w-auto min-w-0` on the trigger.
    pub fn fit(mut self) -> Self {
        self.fit = true;
        self
    }
}

impl RenderOnce for Select {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let label = self.value_label.unwrap_or_else(|| self.options.get(self.selected).cloned().unwrap_or_default());
        let trigger =
            SelectTrigger::new(label).variant(self.variant).size(self.size).leading(self.leading).fit(self.fit);
        let (options, selected, on_change) = (self.options, self.selected, self.on_change);
        Popover::new(self.id, trigger, move |handle: PopupHandle, _, _| {
            select_list(&options, selected, on_change.clone())
                .on_dismiss(move |_, cx| handle.close(cx))
                .into_any_element()
        })
    }
}
