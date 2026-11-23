//! Mirrors [`AppTheme`] onto gpui-component's own `Theme`, so any stock
//! component (text inputs, scrollbars, lists) draws with the same palette.

use gpui_kit::{
    App,
    base::{ScrollbarStyles, ScrollbarTheme, ScrollbarThumbStyle, Theme as BaseTheme},
    component::theme::{Theme, ThemeMode},
    px,
};

use crate::appearance_fonts::{mono_font_family, ui_font_family};
use crate::theme_palette::AppTheme;

/// Installs the palette, fonts and radii on gpui-component's theme.
pub fn apply_component_theme(theme: &AppTheme, cx: &mut App) {
    let mode = if theme.is_dark() { ThemeMode::Dark } else { ThemeMode::Light };
    if !cx.has_global::<Theme>() || Theme::global(cx).mode != mode {
        Theme::change(mode, None, cx);
    }
    let c = theme.colors;
    let d = theme.derived;
    let f = theme.fixed;
    Theme::update(cx, |kit| {
        kit.font_family = ui_font_family();
        kit.font_size = px(theme.fonts.interface);
        kit.mono_font_family = mono_font_family();
        kit.mono_font_size = px(theme.fonts.code);
        kit.radius = theme.radii.lg;
        kit.radius_lg = theme.radii.lg;
        kit.shadow = true;

        kit.background = c.background();
        kit.foreground = c.foreground();
        kit.border = c.border;
        kit.input = c.input;
        kit.ring = c.ring();
        kit.caret = c.foreground();
        kit.selection = d.input_focus_ring;
        kit.muted = c.muted;
        // Stock text inputs draw their placeholder in `muted_foreground`.
        kit.muted_foreground = c.placeholder;
        kit.accent = c.highlight();
        kit.accent_foreground = c.highlight_foreground();
        kit.popover = d.dropdown_glass_opaque;
        kit.popover_foreground = c.foreground();
        kit.overlay = d.dialog_backdrop;
        kit.window_border = c.border;
        kit.link = c.focus;
        kit.link_hover = c.focus;
        kit.link_active = c.focus;

        kit.primary = c.primary();
        kit.primary_hover = d.primary_hover;
        kit.primary_active = d.primary_hover;
        kit.primary_foreground = c.primary_foreground();
        kit.secondary = c.secondary;
        kit.secondary_hover = c.highlight();
        kit.secondary_active = c.highlight();
        kit.secondary_foreground = c.secondary_foreground;
        kit.danger = c.destructive();
        kit.danger_hover = c.destructive();
        kit.danger_active = c.destructive();
        kit.danger_foreground = c.destructive_foreground();
        kit.warning = c.warning;
        kit.warning_hover = c.warning;
        kit.warning_active = c.warning;
        kit.warning_foreground = c.warning_foreground;
        kit.success = f.success;
        kit.success_hover = f.success;
        kit.success_active = f.success;
        kit.success_foreground = f.success_foreground;
        kit.info = f.info;
        kit.info_hover = f.info;
        kit.info_active = f.info;
        kit.info_foreground = f.info_foreground;

        kit.colors.list = c.background();
        kit.list_hover = c.highlight();
        kit.list_active = c.highlight();
        kit.list_active_border = c.ring();
        kit.list_even = c.background();
        kit.list_head = c.background();

        kit.scrollbar = gpui_kit::Hsla::transparent_black();
        kit.scrollbar_thumb = d.scrollbar_thumb;
        kit.scrollbar_thumb_hover = d.scrollbar_thumb_hover;
        kit.skeleton = d.skeleton;
        kit.switch = c.input;
        kit.switch_thumb = c.background();
        kit.slider_bar = c.primary();
        kit.slider_thumb = c.primary();
        kit.progress_bar = c.primary();
        kit.drag_border = c.ring();
        kit.drop_target = d.drop_badge;

        kit.sidebar = c.sidebar;
        kit.sidebar_foreground = c.sidebar_foreground;
        kit.sidebar_border = c.sidebar_border;
        kit.sidebar_accent = c.sidebar_row_hover;
        kit.sidebar_accent_foreground = c.sidebar_foreground;
        kit.sidebar_primary = c.primary();
        kit.sidebar_primary_foreground = c.primary_foreground();

        kit.title_bar = c.toolbar;
        kit.title_bar_border = c.toolbar_border;
        kit.tab_bar = c.background();
        kit.tab = c.background();
        kit.tab_active = c.highlight();
        kit.tab_active_foreground = c.foreground();
        kit.tab_foreground = c.muted_foreground;
        kit.table = c.background();
        kit.table_hover = c.highlight();
        kit.table_active = c.highlight();
        kit.table_head = c.background();
        kit.table_head_foreground = c.muted_foreground;
        kit.table_row_border = c.border;
    });
    apply_scrollbar_style(theme, cx);
}

/// The app's overlay scrollbar (`scroll-area.tsx:79-99`): a 6px rounded
/// thumb 1px from the edge on a transparent track, the same width on hover.
fn apply_scrollbar_style(theme: &AppTheme, cx: &mut App) {
    let d = theme.derived;
    let clear = gpui_kit::Hsla::transparent_black();
    let base = cx.global_mut::<BaseTheme>();
    let (mode, motion) = (base.scrollbar.mode(), base.scrollbar.motion());
    let thumb = |color: gpui_kit::Hsla| {
        move |style: ScrollbarThumbStyle| style.bg(color).width(px(6.)).inset(px(1.)).radius(px(3.))
    };
    base.scrollbar = ScrollbarTheme::new().with_mode(mode).with_motion(motion).with_styles(
        ScrollbarStyles::default()
            .track(|s| s.bg(clear))
            .track_hover(|s| s.bg(clear))
            .track_active(|s| s.bg(clear))
            .thumb(thumb(d.scrollbar_thumb))
            .thumb_hover(thumb(d.scrollbar_thumb_hover))
            .thumb_active(thumb(d.scrollbar_thumb_hover)),
    );
}
