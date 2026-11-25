//! The browser tab's "More" menu. Ports `PreviewMoreMenu.tsx` with the actions
//! `PreviewView.tsx` gives a streamed tab (`moreMenuActions` when the server owns
//! rendering): hard reload, the device toolbar, appearance, zoom, and clearing the
//! profile's cookies and cache. Find, DevTools and the separate window exist only for
//! tabs the desktop app draws itself, so a streamed tab's menu has none of them.

use std::rc::Rc;

use gpui_kit::{Anchor, App, ElementId, IntoElement, RenderOnce, SharedString, Window, anchored, deferred, div, point, prelude::*, px};
use t3code_contracts::PreviewAppearancePreference;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::menu::{MenuEntry, MenuItem, MenuItemKind, MenuPopup};
use crate::components::ui::popover::PopupOpen;
use crate::components::ui::tooltip::tooltip;
use crate::i18n::t;

/// What the menu does; each is one `preview.adjust`, `preview.resize` or stream input.
#[derive(Clone)]
pub struct PreviewMoreMenuActions {
    pub hard_reload: Rc<dyn Fn(&mut App)>,
    pub set_color_scheme: Rc<dyn Fn(PreviewAppearancePreference, &mut App)>,
    pub zoom: Rc<dyn Fn(i8, &mut App)>,
    pub clear_cookies: Rc<dyn Fn(&mut App)>,
    pub clear_cache: Rc<dyn Fn(&mut App)>,
    pub toggle_device_toolbar: Rc<dyn Fn(&mut App)>,
}

/// The tab state the menu shows.
#[derive(Clone)]
pub struct PreviewMoreMenuState {
    pub enabled: bool,
    pub zoom_factor: f64,
    pub color_scheme: PreviewAppearancePreference,
    pub device_toolbar_visible: bool,
    pub profile_name: Option<SharedString>,
}

/// The `MoreVertical` trigger and its menu, aligned to the trigger's end 6px below it.
#[derive(IntoElement)]
pub struct PreviewMoreMenu {
    id: ElementId,
    state: PreviewMoreMenuState,
    actions: PreviewMoreMenuActions,
}

impl PreviewMoreMenu {
    pub fn new(id: impl Into<ElementId>, state: PreviewMoreMenuState, actions: PreviewMoreMenuActions) -> Self {
        Self { id: id.into(), state, actions }
    }
}

/// Whether the Appearance submenu is open, kept with the menu's open state.
#[derive(Default)]
struct SubmenuOpen(bool);

const COLOR_SCHEMES: [PreviewAppearancePreference; 3] =
    [PreviewAppearancePreference::System, PreviewAppearancePreference::Light, PreviewAppearancePreference::Dark];

fn color_scheme_label(scheme: PreviewAppearancePreference) -> SharedString {
    match scheme {
        PreviewAppearancePreference::Light => t!("Light", context = "color scheme"),
        PreviewAppearancePreference::Dark => t!("Dark", context = "color scheme"),
        _ => t!("System", context = "color scheme"),
    }
}

impl RenderOnce for PreviewMoreMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let open_state = window.use_keyed_state(self.id.clone(), cx, |_, _| PopupOpen::default());
        let submenu_state = window.use_keyed_state(SharedString::from(format!("{:?}-submenu", self.id)), cx, |_, _| SubmenuOpen(false));
        let open = open_state.read(cx).open;
        let submenu_open = open && submenu_state.read(cx).0;
        let toggle = open_state.clone();
        let trigger = div()
            .id("preview-more-trigger")
            .tooltip(tooltip(t!("More", context = "more actions menu")))
            .child(Button::new("preview-more").variant(ButtonVariant::Ghost).size(ButtonSize::IconXs).icon(IconName::EllipsisVertical).pressed(open).on_click(
                move |event, _, cx| PopupOpen::toggle_from(&toggle, crate::components::ui::popover::press_position(event), cx),
            ));
        let menu = open.then(|| {
            let theme = cx.app_theme().clone();
            let (state, actions) = (self.state.clone(), self.actions.clone());
            let disabled = !state.enabled;
            let close = {
                let open_state = open_state.clone();
                move |cx: &mut App| PopupOpen::set(&open_state, false, None, cx)
            };
            let item = |label: SharedString, run: Rc<dyn Fn(&mut App)>| {
                let close = close.clone();
                MenuItem::new(label).disabled(disabled).on_select(move |_, cx| {
                    run(cx);
                    close(cx);
                })
            };
            let hover_off = {
                let submenu_state = submenu_state.clone();
                move |hovered: &bool, _: &mut Window, cx: &mut App| {
                    if *hovered {
                        submenu_state.update(cx, |s, cx| {
                            if s.0 {
                                s.0 = false;
                                cx.notify();
                            }
                        });
                    }
                }
            };
            let device_label = if state.device_toolbar_visible { t!("Hide device toolbar") } else { t!("Show device toolbar") };
            let appearance = {
                let submenu_state = submenu_state.clone();
                let open_on = submenu_state.clone();
                let mut row = MenuItem::new(t!("Appearance"))
                    .kind(MenuItemKind::Submenu)
                    .disabled(disabled)
                    .keep_open()
                    .on_hover(move |hovered, _, cx| {
                        if *hovered {
                            submenu_state.update(cx, |s, cx| {
                                s.0 = true;
                                cx.notify();
                            });
                        }
                    })
                    .on_select(move |_, cx| open_on.update(cx, |s, cx| {
                        s.0 = true;
                        cx.notify();
                    }));
                if submenu_open {
                    row = row.highlighted();
                }
                row
            };
            let zoom_label = format!("{}%", (state.zoom_factor * 100.).round() as i64);
            let zoom = actions.zoom.clone();
            let zoom_row = MenuEntry::Row(Rc::new(move |theme| {
                let step = |id: &'static str, icon: IconName, variant: ButtonVariant, direction: i8| {
                    let zoom = zoom.clone();
                    Button::new(id).variant(variant).size(ButtonSize::IconXs).icon(icon).disabled(disabled).on_click(move |_, _, cx| zoom(direction, cx))
                };
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.sp(2.))
                    .h(theme.sp(7.))
                    .flex_none()
                    .px(theme.sp(2.))
                    .rounded(theme.radii.sm)
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .text_color(theme.colors.foreground())
                    .when(disabled, |r| r.opacity(crate::components::ui::DISABLED_OPACITY))
                    .child(div().child(t!("Zoom")))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(1.))
                            .child(step("preview-zoom-out", IconName::Minus, ButtonVariant::Outline, -1))
                            .child(
                                div()
                                    .min_w(theme.sp(12.))
                                    .flex()
                                    .justify_center()
                                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                    .text_color(theme.colors.muted_foreground)
                                    .child(zoom_label.clone()),
                            )
                            .child(step("preview-zoom-in", IconName::Plus, ButtonVariant::Outline, 1))
                            .child(step("preview-zoom-reset", IconName::RotateCcw, ButtonVariant::Ghost, 0)),
                    )
                    .into_any_element()
            }));
            let mut popup = MenuPopup::new()
                .item(item(t!("Hard reload"), actions.hard_reload.clone()).on_hover(hover_off.clone()))
                .item(item(device_label, actions.toggle_device_toolbar.clone()).on_hover(hover_off.clone()))
                .item(appearance)
                .separator()
                .entry(zoom_row)
                .separator();
            if let Some(name) = state.profile_name.clone() {
                popup = popup.label(crate::i18n::t_rich!("Profile: {profileName}", profileName = name).plain());
            }
            let clear = |label: SharedString, run: Rc<dyn Fn(&mut App)>| {
                let close = close.clone();
                MenuItem::new(label).on_hover(hover_off.clone()).on_select(move |_, cx| {
                    run(cx);
                    close(cx);
                })
            };
            popup = popup.item(clear(t!("Clear cookies"), actions.clear_cookies.clone())).item(clear(t!("Clear cache"), actions.clear_cache.clone()));
            let submenu = submenu_open.then(|| {
                let current = state.color_scheme;
                let set = actions.set_color_scheme.clone();
                let items = COLOR_SCHEMES.into_iter().fold(MenuPopup::new(), |popup, scheme| {
                    let (set, close) = (set.clone(), close.clone());
                    popup.item(MenuItem::new(color_scheme_label(scheme)).kind(MenuItemKind::Radio(scheme == current)).on_select(move |_, cx| {
                        set(scheme, cx);
                        close(cx);
                    }))
                });
                // Base UI flips a submenu with no room at the end to the start side, its first
                // row level with the trigger: below two rows and the popup's top padding.
                div()
                    .absolute()
                    .right_full()
                    .mr(px(-4.))
                    .top(theme.sp(7.) * 2. - px(1.))
                    .child(items)
            });
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .offset(point(px(0.), px(6.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(PopupOpen::dismissable(&open_state, div().relative().child(popup).children(submenu))),
            )
            .with_priority(5)
        });
        div()
            .id(self.id)
            .relative()
            .flex_none()
            .child(trigger)
            .child(div().absolute().right_0().top_full().children(menu))
    }
}
