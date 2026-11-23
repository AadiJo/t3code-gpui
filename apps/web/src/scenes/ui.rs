//! The `ui` scene: every primitive in every variant and state, plus the
//! theme's colour swatches, for screenshots. `ui` is dark, `ui-light` light.
//!
//! The swatch strip at the top is laid out on a fixed grid so a script can
//! sample each swatch's pixels: cell `i` of a strip sits at
//! `(SWATCH_X + (i % SWATCH_COLUMNS) * SWATCH_PITCH, strip_y + (i / SWATCH_COLUMNS) * SWATCH_PITCH)`.

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, Hsla, IntoElement, Render, SharedString, Window,
    component::input::{InputState, TextareaState},
    div, prelude::*, px,
};

use crate::appearance_fonts::{TextSize, ui_font, weight};
use crate::components::icons::{BrandIcon, Icon, IconName, IconSize};
use crate::components::ui::{
    ControlState, TextStyleExt as _,
    badge::{Badge, BadgeSize, BadgeVariant},
    button::{Button, ButtonSize, ButtonVariant},
    checkbox::{CheckState, Checkbox},
    empty::{Empty, EmptySize},
    input::{InputSize, TextArea, TextInput},
    kbd::{Kbd, KbdVariant},
    separator::Separator,
    skeleton::{Skeleton, SkeletonShape},
    spinner::{Spinner, SpinnerSize},
    switch::{Switch, SwitchSize},
    toggle::{Toggle, ToggleSize, ToggleVariant},
};
use crate::theme_color::{flatten, with_alpha};
use crate::theme_palette::{ThemeMode, ThemeSettings};
use crate::{ActiveAppTheme as _, AppTheme};

/// Left edge of every swatch strip.
pub const SWATCH_X: f32 = 24.;
/// Swatch side and grid pitch.
pub const SWATCH_SIZE: f32 = 24.;
pub const SWATCH_PITCH: f32 = 28.;
pub const SWATCH_COLUMNS: usize = 31;
/// Top of the role, derived and fixed strips.
pub const ROLES_Y: f32 = 24.;
pub const DERIVED_Y: f32 = ROLES_Y + 2. * SWATCH_PITCH + 12.;
pub const FIXED_Y: f32 = DERIVED_Y + 2. * SWATCH_PITCH + 12.;
const STRIPS_HEIGHT: f32 = FIXED_Y + SWATCH_PITCH + 12.;

/// The dark gallery.
pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    ThemeSettings::set_mode(ThemeMode::Dark, cx);
    cx.new(|cx| Gallery::new(window, cx)).into()
}

/// The light gallery.
pub fn build_light(window: &mut Window, cx: &mut App) -> AnyView {
    ThemeSettings::set_mode(ThemeMode::Light, cx);
    cx.new(|cx| Gallery::new(window, cx)).into()
}

struct Gallery {
    inputs: Vec<Entity<InputState>>,
    filled: Entity<InputState>,
    textarea: Entity<TextareaState>,
}

impl Gallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inputs = (0..ControlState::ALL.len() * 2)
            .map(|_| cx.new(|cx| InputState::new(window, cx).placeholder("Search threads")))
            .collect();
        let filled = cx.new(|cx| InputState::new(window, cx).default_value("feature/gpui-port"));
        let textarea = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 12)
                .placeholder("Ask anything, @ to mention files")
        });
        Self { inputs, filled, textarea }
    }
}

/// Every palette role in `THEME_COLOR_ROLES` order.
fn role_swatches(theme: &AppTheme) -> Vec<Hsla> {
    let c = &theme.colors;
    vec![
        c.canvas, c.chrome, c.toolbar, c.toolbar_foreground, c.toolbar_border, c.toolbar_control,
        c.toolbar_control_foreground, c.toolbar_control_hover, c.surface, c.surface_raised,
        c.surface_overlay, c.text, c.text_muted, c.border, c.input, c.focus, c.accent,
        c.accent_foreground, c.secondary, c.secondary_foreground, c.muted, c.muted_foreground,
        c.placeholder, c.secondary_label, c.icon_muted, c.error, c.error_foreground, c.error_surface,
        c.warning, c.warning_foreground, c.warning_surface, c.update, c.update_foreground,
        c.update_surface, c.accent_surface, c.accent_surface_foreground, c.message_surface,
        c.message_foreground, c.message_action, c.message_action_foreground, c.message_action_hover,
        c.code_background, c.code_foreground, c.search_match_background, c.search_match_foreground,
        c.search_match_active_background, c.search_match_active_foreground, c.sidebar,
        c.sidebar_foreground, c.sidebar_muted_foreground, c.sidebar_control_surface,
        c.sidebar_row_hover, c.sidebar_row_active, c.sidebar_row_selected, c.sidebar_border,
        c.terminal_background, c.terminal_foreground, c.terminal_cursor, c.terminal_selection,
        c.terminal_scrollbar, c.terminal_scrollbar_hover,
    ]
}

/// Each derived colour of `graphite-tokens.json` in file order, with the
/// surface the token file flattens it over (`None` for opaque values).
fn derived_swatches(theme: &AppTheme) -> Vec<(Hsla, Option<Hsla>)> {
    let c = &theme.colors;
    let d = &theme.derived;
    let glass_over_canvas = flatten(d.dropdown_glass, c.canvas);
    vec![
        (d.sidebar_icon, None),
        (d.dropdown_glass, Some(c.canvas)),
        (d.dropdown_glass_border, Some(glass_over_canvas)),
        (d.surface_glass, Some(c.canvas)),
        (d.dialog_glass_border, Some(c.canvas)),
        (d.dialog_backdrop, None),
        (d.titlebar_toggle_icon, None),
        (d.titlebar_toggle_icon_disabled, None),
        (d.sidebar_edge, Some(c.sidebar)),
        (d.sidebar_menu_button_text, Some(c.sidebar)),
        (d.receded_row_text, Some(c.sidebar)),
        (d.receded_card_title, Some(c.sidebar)),
        (d.failed_card_title, Some(c.sidebar)),
        (d.folder_name, Some(c.sidebar)),
        (d.slim_row_read_title, Some(c.sidebar)),
        (d.pin_icon, Some(c.sidebar)),
        (d.empty_state_text, Some(c.sidebar)),
        (d.section_header_rule, Some(c.sidebar)),
        (d.drag_boundary_rule, Some(c.sidebar)),
        (d.draft_row, Some(c.sidebar)),
        (d.draft_row_hover, Some(c.sidebar)),
        (d.drop_badge, Some(c.sidebar)),
        (d.drop_badge_border, Some(c.sidebar)),
        (d.focus_outline, Some(c.canvas)),
        (d.input_focus_ring, Some(c.canvas)),
        (d.sidebar_search_focus_ring, Some(c.sidebar)),
        // The token file lists `input/32` and `input/64` literally, even for light.
        (with_alpha(c.input, 0.32), Some(c.canvas)),
        (with_alpha(c.input, 0.64), Some(c.canvas)),
        (d.toggle_pressed, Some(c.canvas)),
        (d.primary_hover, Some(c.canvas)),
        (d.kbd, Some(c.popover())),
        (d.item_selected, Some(c.popover())),
        (d.skeleton, Some(c.canvas)),
        (d.dialog_footer, Some(c.canvas)),
        (d.terminal_drawer_border, Some(c.canvas)),
        (d.destructive_highlight, Some(c.popover())),
        (d.inline_code, Some(c.canvas)),
        (d.scrollbar_thumb, Some(c.canvas)),
        (d.scrollbar_thumb_hover, Some(c.canvas)),
    ]
}

fn fixed_swatches(theme: &AppTheme) -> Vec<Hsla> {
    let f = &theme.fixed;
    vec![
        f.info, f.info_foreground, f.success, f.success_foreground, f.guest, f.guest_foreground,
        f.input_status, f.diff_addition_foreground, f.diff_deletion_foreground,
    ]
}

fn strip(y: f32, swatches: Vec<(Hsla, Option<Hsla>)>) -> impl Iterator<Item = AnyElement> {
    swatches.into_iter().enumerate().map(move |(i, (color, surface))| {
        let x = SWATCH_X + (i % SWATCH_COLUMNS) as f32 * SWATCH_PITCH;
        let y = y + (i / SWATCH_COLUMNS) as f32 * SWATCH_PITCH;
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .size(px(SWATCH_SIZE))
            .when_some(surface, |s, surface| s.bg(surface))
            .child(div().size_full().bg(color))
            .into_any_element()
    })
}

fn swatch_strips(theme: &AppTheme) -> impl IntoElement {
    let roles = role_swatches(theme).into_iter().map(|c| (c, None)).collect();
    let fixed = fixed_swatches(theme).into_iter().map(|c| (c, None)).collect();
    div()
        .relative()
        .w_full()
        .h(px(STRIPS_HEIGHT))
        .children(strip(ROLES_Y, roles))
        .children(strip(DERIVED_Y, derived_swatches(theme)))
        .children(strip(FIXED_Y, fixed))
}

fn section(theme: &AppTheme, title: &'static str, body: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .px(px(24.))
        .py(px(16.))
        .border_t_1()
        .border_color(theme.colors.border)
        .child(div().ui_text(theme, TextSize::Xs, weight::SEMIBOLD).text_color(theme.colors.muted_foreground).child(title))
        .child(body)
}

fn caption(theme: &AppTheme, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .w(px(132.))
        .flex_none()
        .ui_text(theme, TextSize::Xs2, weight::NORMAL)
        .text_color(theme.colors.muted_foreground)
        .child(text.into())
}

fn row(children: impl IntoIterator<Item = AnyElement>) -> impl IntoElement {
    div().flex().flex_row().flex_wrap().items_center().gap(px(10.)).children(children)
}

fn typography(theme: &AppTheme) -> impl IntoElement {
    let c = &theme.colors;
    let sample = |size: TextSize, label: &'static str, w| {
        div()
            .flex()
            .items_baseline()
            .gap(px(12.))
            .child(caption(theme, label))
            .child(div().ui_text(theme, size, w).text_color(c.foreground()).child("Pick a thread to continue 0123456789"))
            .into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(sample(TextSize::Xl, "xl 600", weight::SEMIBOLD))
        .child(sample(TextSize::Base, "base 400", weight::NORMAL))
        .child(sample(TextSize::Sm, "sm 500", weight::MEDIUM))
        .child(sample(TextSize::Sm, "sm 400", weight::NORMAL))
        .child(sample(TextSize::Xs, "xs 500", weight::MEDIUM))
        .child(sample(TextSize::Xs2, "2xs 400", weight::NORMAL))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(12.))
                .child(caption(theme, "mono 13"))
                .child(
                    div()
                        .mono_text(px(theme.fonts.code), weight::NORMAL)
                        .text_color(c.code_foreground)
                        .child("fn main() { println!(\"0O1lI\"); }"),
                ),
        )
}

fn icons(theme: &AppTheme) -> impl IntoElement {
    let c = &theme.colors;
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div().flex().flex_wrap().gap(px(8.)).children(
                IconName::ALL
                    .iter()
                    .map(|&name| Icon::new(name).size(IconSize::Md).color(c.foreground()).into_any_element()),
            ),
        )
        .child(
            div().flex().flex_wrap().gap(px(10.)).children(
                BrandIcon::ALL
                    .iter()
                    .map(|&icon| Icon::new(icon).size(IconSize::Xl).color(c.foreground()).into_any_element()),
            ),
        )
        .child(row([
            caption(theme, "12 / 14 / 16 / 18 / 20").into_any_element(),
            Icon::new(IconName::Settings).size(IconSize::Xs).color(c.muted_foreground).into_any_element(),
            Icon::new(IconName::Settings).size(IconSize::Sm).color(c.muted_foreground).into_any_element(),
            Icon::new(IconName::Settings).size(IconSize::Md).color(c.muted_foreground).into_any_element(),
            Icon::new(IconName::Settings).size(IconSize::Lg).color(c.muted_foreground).into_any_element(),
            Icon::new(IconName::Settings).size(IconSize::Xl).color(c.muted_foreground).into_any_element(),
            caption(theme, "stroke 2.25").into_any_element(),
            Icon::new(IconName::X).size(IconSize::Xs).stroke(2.25).color(c.foreground()).into_any_element(),
            Icon::new(IconName::ChevronRight).size(IconSize::Sm).stroke(2.25).color(c.foreground()).into_any_element(),
            Icon::new(IconName::ChevronDown).size(IconSize::Md).rotate(-90.).color(c.foreground()).into_any_element(),
        ]))
}

fn buttons(theme: &AppTheme) -> impl IntoElement {
    let matrix = ButtonVariant::ALL.iter().map(|&variant| {
        let cells = ControlState::ALL.iter().map(move |&state| {
            Button::new(SharedString::from(format!("b-{}-{}", variant.label(), state.label())))
                .label(state.label())
                .icon(IconName::Plus)
                .variant(variant)
                .state(state)
                .into_any_element()
        });
        row(std::iter::once(caption(theme, variant.label()).into_any_element()).chain(cells)).into_any_element()
    });
    let sizes = ButtonSize::ALL.iter().map(|&size| {
        let button = Button::new(SharedString::from(format!("s-{}", size.label()))).size(size).variant(ButtonVariant::Outline);
        let button = if size.is_icon() { button.icon(IconName::Settings) } else { button.icon(IconName::Plus).label(size.label()) };
        button.into_any_element()
    });
    let ghosts = ButtonSize::ALL.iter().filter(|s| s.is_icon()).map(|&size| {
        Button::new(SharedString::from(format!("g-{}", size.label())))
            .size(size)
            .variant(ButtonVariant::Ghost)
            .icon(IconName::PanelLeft)
            .into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(matrix)
        .child(row(std::iter::once(caption(theme, "sizes (outline)").into_any_element()).chain(sizes)))
        .child(row(std::iter::once(caption(theme, "icon sizes (ghost)").into_any_element()).chain(ghosts)))
}

fn badges_and_kbd(theme: &AppTheme) -> impl IntoElement {
    let variants = BadgeSize::ALL.iter().map(|&size| {
        let cells = BadgeVariant::ALL.iter().map(move |&variant| {
            Badge::new(variant.label()).variant(variant).size(size).into_any_element()
        });
        row(std::iter::once(caption(theme, format!("badge {}", size.label())).into_any_element()).chain(cells))
            .into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(variants)
        .child(row([
            caption(theme, "badge icon").into_any_element(),
            Badge::new("3").icon(IconName::GitBranch).variant(BadgeVariant::Secondary).into_any_element(),
            Badge::new("Done").icon(IconName::CircleCheck).variant(BadgeVariant::Success).into_any_element(),
        ]))
        .child(row([
            caption(theme, "kbd").into_any_element(),
            Kbd::new("⌘K").into_any_element(),
            Kbd::new("Ctrl").into_any_element(),
            Kbd::new("Enter").variant(KbdVariant::Outline).into_any_element(),
            Kbd::new("⇧").variant(KbdVariant::Outline).into_any_element(),
        ]))
}

fn toggles(theme: &AppTheme) -> impl IntoElement {
    let matrix = ToggleVariant::ALL.iter().map(|&variant| {
        let cells = [false, true].into_iter().flat_map(move |pressed| {
            ControlState::ALL.iter().map(move |&state| {
                let label = if pressed { "on" } else { "off" };
                Toggle::new(SharedString::from(format!("t-{}-{}-{}", variant.label(), label, state.label())))
                    .icon(IconName::Pin)
                    .label(label)
                    .variant(variant)
                    .size(if variant == ToggleVariant::Segmented { ToggleSize::Segmented } else { ToggleSize::Default })
                    .pressed(pressed)
                    .state(state)
                    .into_any_element()
            })
        });
        row(std::iter::once(caption(theme, variant.label()).into_any_element()).chain(cells)).into_any_element()
    });
    let sizes = ToggleSize::ALL.iter().map(|&size| {
        Toggle::new(SharedString::from(format!("ts-{}", size.label())))
            .icon(IconName::Terminal)
            .variant(ToggleVariant::Outline)
            .size(size)
            .into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(matrix)
        .child(row(std::iter::once(caption(theme, "sizes (outline)").into_any_element()).chain(sizes)))
}

fn choice_controls(theme: &AppTheme) -> impl IntoElement {
    let switches = [SwitchSize::Default, SwitchSize::Sm].into_iter().map(|size| {
        let cells = [false, true].into_iter().flat_map(move |checked| {
            ControlState::ALL.iter().map(move |&state| {
                Switch::new(SharedString::from(format!("sw-{size:?}-{checked}-{}", state.label())), checked)
                    .size(size)
                    .state(state)
                    .into_any_element()
            })
        });
        row(std::iter::once(caption(theme, format!("switch {size:?} off/on")).into_any_element()).chain(cells))
            .into_any_element()
    });
    let checks = [false, true].into_iter().map(|contrast| {
        let cells = [CheckState::Unchecked, CheckState::Checked, CheckState::Indeterminate].into_iter().flat_map(
            move |checked| {
                ControlState::ALL.iter().map(move |&state| {
                    let checkbox = Checkbox::new(
                        SharedString::from(format!("cb-{contrast}-{checked:?}-{}", state.label())),
                        checked,
                    )
                    .state(state);
                    if contrast { checkbox.contrast() } else { checkbox }.into_any_element()
                })
            },
        );
        let label = if contrast { "checkbox contrast" } else { "checkbox" };
        row(std::iter::once(caption(theme, label).into_any_element()).chain(cells)).into_any_element()
    });
    div().flex().flex_col().gap(px(10.)).children(switches).children(checks)
}

fn inputs(gallery: &Gallery, theme: &AppTheme) -> impl IntoElement {
    let states = ControlState::ALL.iter().enumerate().map(|(i, &state)| {
        div()
            .w(px(200.))
            .child(TextInput::new(&gallery.inputs[i]).state(state))
            .into_any_element()
    });
    let invalid = [ControlState::Idle, ControlState::FocusVisible].iter().enumerate().map(|(i, &state)| {
        div()
            .w(px(200.))
            .child(TextInput::new(&gallery.inputs[ControlState::ALL.len() + i]).invalid(true).state(state))
            .into_any_element()
    });
    let sizes = InputSize::ALL.iter().map(|&size| {
        div().w(px(160.)).child(TextInput::new(&gallery.filled).size(size).state(ControlState::Idle)).into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(row(std::iter::once(caption(theme, "input states").into_any_element()).chain(states)))
        .child(row(std::iter::once(caption(theme, "invalid / focused").into_any_element()).chain(invalid)))
        .child(row(std::iter::once(caption(theme, "sizes sm/default/lg/compact").into_any_element()).chain(sizes)))
        .child(row([
            caption(theme, "textarea").into_any_element(),
            div().w(px(420.)).child(TextArea::new(&gallery.textarea).state(ControlState::Idle)).into_any_element(),
            div().w(px(420.)).child(TextArea::new(&gallery.textarea).state(ControlState::FocusVisible)).into_any_element(),
        ]))
}

fn loading(theme: &AppTheme) -> impl IntoElement {
    row([
        caption(theme, "skeleton block/card/pill").into_any_element(),
        Skeleton::new(SkeletonShape::Block).w(px(160.)).h(px(16.)).into_any_element(),
        Skeleton::new(SkeletonShape::Card).w(px(120.)).h(px(48.)).into_any_element(),
        Skeleton::new(SkeletonShape::Pill).w(px(80.)).h(px(20.)).into_any_element(),
        caption(theme, "spinner xs/sm/md/lg").into_any_element(),
        Spinner::new(SpinnerSize::Xs).into_any_element(),
        Spinner::new(SpinnerSize::Sm).into_any_element(),
        Spinner::new(SpinnerSize::Md).muted().into_any_element(),
        Spinner::new(SpinnerSize::Lg).into_any_element(),
        caption(theme, "separator").into_any_element(),
        div().w(px(120.)).child(Separator::horizontal()).into_any_element(),
        div().h(px(24.)).flex().child(Separator::vertical()).into_any_element(),
    ])
}

fn empty_states(theme: &AppTheme) -> impl IntoElement {
    div()
        .flex()
        .gap(px(12.))
        .child(
            div().w(px(420.)).border_1().border_color(theme.colors.border).rounded(px(8.)).child(
                Empty::new("Pick a thread to continue")
                    .icon(IconName::MessageCircleQuestionMark)
                    .description("Select an existing thread or create a new one to get started.")
                    .child(Button::new("empty-new").label("New thread").icon(IconName::SquarePen).size(ButtonSize::Sm)),
            ),
        )
        .child(
            div().w(px(420.)).border_1().border_color(theme.colors.border).rounded(px(8.)).child(
                Empty::new("No matching threads")
                    .size(EmptySize::Compact)
                    .description("Try a different search."),
            ),
        )
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        div()
            .id("ui-gallery")
            .size_full()
            .overflow_y_scroll()
            .bg(theme.colors.background())
            .text_color(theme.colors.foreground())
            .font(ui_font(weight::NORMAL))
            .child(swatch_strips(theme))
            .child(section(theme, "TYPOGRAPHY", typography(theme)))
            .child(section(theme, "BUTTON  variants x states, sizes", buttons(theme)))
            .child(section(theme, "TOGGLE  variants x off/on x states", toggles(theme)))
            .child(section(theme, "SWITCH / CHECKBOX", choice_controls(theme)))
            .child(section(theme, "INPUT / TEXTAREA", inputs(self, theme)))
            .child(section(theme, "BADGE / KBD", badges_and_kbd(theme)))
            .child(section(theme, "SKELETON / SPINNER / SEPARATOR", loading(theme)))
            .child(section(theme, "EMPTY", empty_states(theme)))
            .child(section(theme, "ICONS  lucide 0.564.0 + brand", icons(theme)))
    }
}
