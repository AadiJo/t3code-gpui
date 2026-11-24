//! Menus. Ports `menu.tsx`: a glass popup of items, checkbox, radio and
//! switch items, group labels, separators, shortcuts and submenu triggers.
//! [`DropdownMenu`] opens one below a trigger; [`ContextMenu`] opens one at
//! the pointer on right click (the fork's web-styled context menus).

use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, ElementId, IntoElement, MouseButton, Pixels, Point, RenderOnce, SharedString, Window,
    anchored, deferred, div, prelude::*, px, svg,
};

use super::popover::{Popover, PopupHandle, PopupOpen, glass_surface};
use super::{DISABLED_OPACITY, TextStyleExt as _, ControlHighlightExt as _};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::{ActiveAppTheme as _, AppTheme};

type Action = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// What kind of row an item is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuItemKind {
    #[default]
    Plain,
    Checkbox(bool),
    Radio(bool),
    Switch(bool),
    /// Opens a submenu; shows a trailing chevron.
    Submenu,
}

/// One selectable row.
#[derive(Clone)]
pub struct MenuItem {
    label: SharedString,
    icon: Option<IconName>,
    shortcut: Option<SharedString>,
    kind: MenuItemKind,
    destructive: bool,
    disabled: bool,
    inset: bool,
    highlighted: bool,
    detail: Option<SharedString>,
    id: Option<SharedString>,
    on_select: Option<Action>,
    on_hover: Option<Rc<dyn Fn(&bool, &mut Window, &mut App) + 'static>>,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            shortcut: None,
            kind: MenuItemKind::Plain,
            destructive: false,
            disabled: false,
            inset: false,
            highlighted: false,
            detail: None,
            id: None,
            on_select: None,
            on_hover: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn shortcut(mut self, keys: impl Into<SharedString>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }

    pub fn kind(mut self, kind: MenuItemKind) -> Self {
        self.kind = kind;
        self
    }

    /// `variant="destructive"`.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Indents the label to line up with items that have icons (`ps-8`).
    pub fn inset(mut self) -> Self {
        self.inset = true;
        self
    }

    /// Draws the keyboard/pointer highlight without a pointer (gallery).
    pub fn highlighted(mut self) -> Self {
        self.highlighted = true;
        self
    }

    pub fn on_select(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(action));
        self
    }

    /// A stable element id, for scripted checks; rows are otherwise numbered.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Trailing muted text (`ml-auto text-xs text-muted-foreground`), such as a time.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Runs when the pointer enters (`true`) or leaves the row; submenu triggers open on hover.
    pub fn on_hover(mut self, hover: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(hover));
        self
    }
}

/// A row of a menu.
#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItem),
    /// `MenuGroupLabel`.
    Label(SharedString),
    Separator,
}

impl From<MenuItem> for MenuEntry {
    fn from(item: MenuItem) -> Self {
        Self::Item(item)
    }
}

/// The menu popup surface. Selecting an item runs its action, then dismisses.
#[derive(IntoElement, Clone, Default)]
pub struct MenuPopup {
    entries: Vec<MenuEntry>,
    min_width: Option<Pixels>,
    dismiss: Option<Action>,
    context: bool,
}

impl MenuPopup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entry(mut self, entry: impl Into<MenuEntry>) -> Self {
        self.entries.push(entry.into());
        self
    }

    pub fn item(self, item: MenuItem) -> Self {
        self.entry(item)
    }

    pub fn label(self, label: impl Into<SharedString>) -> Self {
        self.entry(MenuEntry::Label(label.into()))
    }

    pub fn separator(self) -> Self {
        self.entry(MenuEntry::Separator)
    }

    /// At least this wide; a select's list matches its trigger.
    pub fn min_width(mut self, width: Pixels) -> Self {
        self.min_width = Some(width);
        self
    }

    /// Styles it as the app's context menus (`contextMenuFallback.ts`): at least 8rem wide,
    /// disabled rows in the muted colour.
    pub fn context(mut self) -> Self {
        self.context = true;
        self
    }

    /// Called after an item is selected, to close the owning popup.
    pub fn on_dismiss(mut self, dismiss: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.dismiss = Some(Rc::new(dismiss));
        self
    }
}

/// The 30x18-style switch inside a switch item: 12px thumb, 1px padding.
fn menu_switch(theme: &AppTheme, on: bool) -> impl IntoElement {
    let thumb = theme.sp(3.);
    let c = &theme.colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .w(thumb * 2. - px(2.))
        .h(thumb + px(2.))
        .p(px(1.))
        .rounded(theme.radii.full)
        .bg(if on { c.primary() } else { c.input })
        .child(
            div()
                .when(on, |t| t.ml(thumb - px(4.)))
                .size(thumb)
                .rounded(theme.radii.full)
                .bg(c.background())
                .shadow(theme.shadows.sm5.clone()),
        )
}

fn render_item(theme: &AppTheme, index: usize, item: MenuItem, dismiss: Option<Action>, context: bool) -> AnyElement {
    let c = &theme.colors;
    let text = if item.destructive {
        c.destructive_foreground()
    } else if context && item.disabled {
        c.muted_foreground
    } else {
        c.foreground()
    };
    let icon_color = if item.destructive { text } else { c.muted_foreground };
    let highlight = |el: gpui_kit::Stateful<gpui_kit::Div>| {
        el.bg(c.highlight()).text_color(if item.destructive { text } else { c.highlight_foreground() })
    };
    let radio_checked = item.kind == MenuItemKind::Radio(true);
    let leading_check = matches!(item.kind, MenuItemKind::Checkbox(_));
    let row = div()
        .id(item.id.clone().map_or_else(|| ElementId::from(("menu-item", index)), ElementId::from))
        .flex()
        .items_center()
        .gap(theme.sp(2.))
        .min_h(theme.sp(7.))
        .px(theme.sp(2.))
        .py(theme.sp(1.))
        .rounded(theme.radii.sm)
        .text_color(text)
        .ui_text(theme, TextSize::Sm, weight::NORMAL)
        .when(item.inset, |r| r.pl(theme.sp(8.)))
        .when(leading_check, |r| r.pr(theme.sp(4.)))
        .when(radio_checked, |r| r.bg(theme.derived.item_selected))
        .when(item.highlighted && !item.disabled, highlight)
        .when(item.disabled, |r| r.opacity(DISABLED_OPACITY));
    let row = if leading_check {
        row.child(div().w(theme.sp(4.)).flex_none().when(item.kind == MenuItemKind::Checkbox(true), |slot| {
            slot.child(svg().path("icons/ui/menu-check.svg").size(px(16.)).text_color(text))
        }))
    } else {
        row.children(item.icon.map(|icon| {
            div().mx(px(-2.)).child(Icon::new(icon).size(IconSize::Md).color(icon_color).opacity(0.8))
        }))
    };
    let row = row.child(div().flex_1().min_w_0().truncate().child(item.label));
    let row = match item.kind {
        MenuItemKind::Switch(on) => row.gap(theme.sp(4.)).pr(theme.sp(1.5)).child(menu_switch(theme, on)),
        MenuItemKind::Submenu => row.child(
            div().mr(px(-2.)).child(
                Icon::new(IconName::ChevronRight).size(IconSize::Md).color(c.muted_foreground).opacity(0.8),
            ),
        ),
        _ => row,
    };
    let row = row.children(item.detail.map(|detail| {
        div()
            .ml_auto()
            .pl(theme.sp(2.))
            .flex_none()
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(c.muted_foreground)
            .child(detail)
    }));
    let row = row.children(item.shortcut.map(|keys| {
        div()
            .ml_auto()
            .pl(theme.sp(4.))
            .flex_none()
            .ui_text(theme, TextSize::Xs, weight::MEDIUM)
            .text_color(c.secondary_label)
            .child(keys)
    }));
    if item.disabled {
        return row.into_any_element();
    }
    let row = match item.on_hover.clone() {
        Some(hover) => row.on_hover(move |hovered, window, cx| hover(hovered, window, cx)),
        None => row,
    };
    let action = item.on_select.clone();
    let (hover_bg, hover_text) = (c.highlight(), if item.destructive { text } else { c.highlight_foreground() });
    row.cursor_pointer()
        .hover(move |s| s.bg(hover_bg).text_color(hover_text))
        .on_click(move |_, window, cx| {
            if let Some(action) = &action {
                action(window, cx);
            }
            if let Some(dismiss) = &dismiss {
                dismiss(window, cx);
            }
        })
        .test_support()
        .into_any_element()
}

impl RenderOnce for MenuPopup {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let dismiss = self.dismiss.clone();
        let rows = self.entries.into_iter().enumerate().map(|(index, entry)| match entry {
            MenuEntry::Item(item) => render_item(theme, index, item, dismiss.clone(), self.context),
            MenuEntry::Label(label) => div()
                .px(theme.sp(2.))
                .py(theme.sp(1.5))
                .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                .text_color(c.muted_foreground)
                .child(label)
                .into_any_element(),
            MenuEntry::Separator => div().mx(theme.sp(2.)).my(theme.sp(1.)).h(px(1.)).bg(c.border).into_any_element(),
        });
        glass_surface(theme)
            .min_w(self.min_width.unwrap_or(theme.rem(if self.context { 8. } else { 10. })))
            .p(theme.sp(1.))
            .control_highlight(theme)
            .children(rows)
    }
}

/// A trigger that opens a menu below itself.
pub fn dropdown_menu(
    id: impl Into<ElementId>,
    trigger: impl IntoElement,
    menu: impl Fn(&mut Window, &mut App) -> MenuPopup + 'static,
) -> Popover {
    Popover::new(id, trigger, move |handle: PopupHandle, window, cx| {
        menu(window, cx).on_dismiss(move |_, cx| handle.close(cx)).into_any_element()
    })
}

/// Where an open context menu sits.
#[derive(Default)]
struct ContextMenuAt(Option<Point<Pixels>>);

/// Wraps content so a right click opens a menu at the pointer.
#[derive(IntoElement)]
pub struct ContextMenu {
    id: ElementId,
    content: AnyElement,
    menu: Rc<dyn Fn(&mut Window, &mut App) -> MenuPopup + 'static>,
}

impl ContextMenu {
    pub fn new(
        id: impl Into<ElementId>,
        content: impl IntoElement,
        menu: impl Fn(&mut Window, &mut App) -> MenuPopup + 'static,
    ) -> Self {
        Self { id: id.into(), content: content.into_any_element(), menu: Rc::new(menu) }
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let at_state = window.use_keyed_state(self.id.clone(), cx, |_, _| ContextMenuAt::default());
        let open_state = window.use_keyed_state((self.id.clone(), "open"), cx, |_, _| PopupOpen::default());
        let at = at_state.read(cx).0.filter(|_| open_state.read(cx).open);
        let opener = (at_state.clone(), open_state.clone());
        let mut root = div()
            .id(self.id)
            .child(self.content)
            .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                let (at, open) = &opener;
                at.update(cx, |at, _| at.0 = Some(event.position));
                PopupOpen::set(open, true, None, cx);
            });
        if let Some(position) = at {
            let close = open_state.clone();
            let mut popup = (self.menu)(window, cx).on_dismiss(move |_, cx| PopupOpen::set(&close, false, None, cx));
            // The fork's fallback context menu: `min-w-32`, kept 4px inside the window.
            popup.min_width.get_or_insert(cx.app_theme().rem(8.));
            root = root.child(
                deferred(
                    anchored()
                        .position(position)
                        .snap_to_window_with_margin(px(4.))
                        .child(PopupOpen::dismissable(&open_state, popup)),
                )
                .with_priority(5),
            );
        }
        root
    }
}
