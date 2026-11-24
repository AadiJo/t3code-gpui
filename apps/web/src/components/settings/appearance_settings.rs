//! Appearance. Ports `AppearanceSettingsPanel.tsx`, the theme library of
//! `ThemeSettings.tsx` with `ThemePreviewCircles` and `ThemeWireframe`,
//! `ThemeImportDialog` (paste a T3 or VS Code theme, or search Open VSX)
//! and `TypographySettings.tsx`.
//!
//! Contrast, Glass opacity and font smoothing write the client settings the
//! theme reads. Not built: the theme editor (Create theme and Duplicate add
//! a copy instead of opening it; custom themes have no Edit or Export), the
//! installed-font picker, and the live font previews under the font rows.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{
    TestSupportExt as _, Focusable as _, AnyElement, App, AppContext as _, Context, Entity, Hsla, IntoElement, Pixels, Render, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use t3code_contracts::{ChatWidth, EffortDisplay, EnvironmentIdentificationMode};

use super::settings_controls::{SelectWidth, client_row, client_select, client_switch, reset_button, sized_select};
use super::settings_slider::{SettingsSlider, slider_value};
use super::theme_sphere::{SphereColors, standard_sphere_colors, theme_sphere};
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::settings_panels::advanced_settings;
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::input::{TextArea, TextInput};
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::{ClientSettings, ClientSettingsStore};
use crate::hooks::use_theme::{CatalogTheme, ThemeSource, ThemeStore, separate_fonts, set_separate_fonts, standard_theme};
use crate::i18n::t;
use crate::open_vsx_themes::{OpenVsxThemeExtension, import_open_vsx_theme_extension, search_open_vsx_themes};
use crate::theme_color::{mix_oklab, with_alpha};
use crate::theme_palette::{Appearance, ThemeMode, ThemePalette};
use crate::vscode_theme_import::parse_theme_import;

/// The four font family settings, in the order their inputs are kept.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FontRole {
    Interface,
    Prompt,
    Code,
    Terminal,
}

impl FontRole {
    const ALL: [FontRole; 4] = [Self::Interface, Self::Prompt, Self::Code, Self::Terminal];

    fn family(self, s: &ClientSettings) -> &str {
        match self {
            Self::Interface => s.font_family_sans.as_str(),
            Self::Prompt => s.font_family_composer.as_str(),
            Self::Code => s.font_family_code.as_str(),
            Self::Terminal => s.font_family_terminal.as_str(),
        }
    }

    fn set_family(self, s: &mut ClientSettings, value: String) {
        let value = t3code_contracts::FontFamilyPreference::from(value.as_str());
        match self {
            Self::Interface => s.font_family_sans = value,
            Self::Prompt => s.font_family_composer = value,
            Self::Code => s.font_family_code = value,
            Self::Terminal => s.font_family_terminal = value,
        }
    }

    fn size(self, s: &ClientSettings) -> u64 {
        match self {
            Self::Interface => s.font_size_interface,
            Self::Prompt => s.font_size_prompt,
            Self::Code => s.font_size_code,
            Self::Terminal => s.font_size_terminal,
        }
    }

    fn set_size(self, s: &mut ClientSettings, value: u64) {
        match self {
            Self::Interface => s.font_size_interface = value,
            Self::Prompt => s.font_size_prompt = value,
            Self::Code => s.font_size_code = value,
            Self::Terminal => s.font_size_terminal = value,
        }
    }

    /// The input's placeholder: the family the default stack resolves to.
    fn default_family(self) -> SharedString {
        match self {
            Self::Interface | Self::Prompt if cfg!(target_os = "macos") => SharedString::new_static("SF Pro"),
            Self::Interface | Self::Prompt => t!("System default"),
            Self::Code | Self::Terminal => crate::appearance_fonts::mono_font_family(),
        }
    }

    /// `MIN_*_FONT_SIZE`..=`MAX_*_FONT_SIZE`.
    fn range(self) -> (u64, u64) {
        match self {
            Self::Interface | Self::Prompt => (12, 20),
            Self::Code => (10, 18),
            Self::Terminal => (8, 20),
        }
    }
}

/// The import dialog's state.
struct ImportDialog {
    paste: Entity<TextareaState>,
    query: Entity<InputState>,
    results: Vec<OpenVsxThemeExtension>,
    searching: bool,
    importing: Option<String>,
    error: Option<SharedString>,
}

/// The Appearance page.
pub struct AppearanceSettings {
    fonts: Vec<Entity<InputState>>,
    advanced_open: bool,
    typography_advanced_open: bool,
    import: Option<ImportDialog>,
    /// A custom theme waiting for "Remove theme" to be confirmed.
    remove: Option<CatalogTheme>,
    _subscriptions: Vec<Subscription>,
}

impl AppearanceSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = ClientSettingsStore::get(cx).clone();
        let mut subscriptions = Vec::new();
        let fonts: Vec<Entity<InputState>> = FontRole::ALL
            .iter()
            .map(|role| {
                let value = role.family(&settings).to_owned();
                cx.new(|cx| InputState::new(window, cx).placeholder(role.default_family()).default_value(value))
            })
            .collect();
        for (role, input) in FontRole::ALL.iter().zip(&fonts) {
            let role = *role;
            subscriptions.push(cx.subscribe(input, move |_, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    ClientSettingsStore::update(cx, |s| role.set_family(s, value));
                }
            }));
        }
        subscriptions.push(cx.observe_global_in::<ClientSettingsStore>(window, |this, window, cx| this.sync_inputs(window, cx)));
        Self { fonts, advanced_open: false, typography_advanced_open: false, import: None, remove: None, _subscriptions: subscriptions }
    }

    /// Puts reset or imported values into the inputs the user is not typing in.
    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = ClientSettingsStore::get(cx).clone();
        for (role, input) in FontRole::ALL.iter().zip(&self.fonts) {
            let wanted = role.family(&settings).to_owned();
            let focused = input.read(cx).focus_handle(cx).is_focused(window);
            if !focused && input.read(cx).value().as_ref() != wanted {
                input.update(cx, |state, cx| state.set_value(wanted, window, cx));
            }
        }
        cx.notify();
    }

    /// Opens the import dialog, for scenes and checks.
    pub fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.import = Some(ImportDialog {
            paste: cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 10).placeholder(t!("Theme JSON"))),
            query: cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search themes..."))),
            results: Vec::new(),
            searching: false,
            importing: None,
            error: None,
        });
        cx.notify();
    }

    /// Imports theme JSON (T3 or VS Code) as a custom theme and uses it.
    pub fn import_json(&mut self, source: &str, cx: &mut Context<Self>) -> Result<String, SharedString> {
        let parsed = parse_theme_import(source).map_err(|e| SharedString::from(e.0))?;
        let file = serde_json::to_vec(&parsed.to_file()).expect("theme encodes");
        let id = ThemeStore::add_custom(&file, cx).map_err(|e| SharedString::from(e.to_string()))?;
        use_imported(&id, &parsed.label, cx);
        Ok(id)
    }

    fn submit_paste(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &self.import else { return };
        let source = dialog.paste.read(cx).value().to_string();
        match self.import_json(&source, cx) {
            Ok(_) => self.import = None,
            Err(error) => {
                if let Some(dialog) = &mut self.import {
                    dialog.error = Some(error);
                }
            }
        }
        cx.notify();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.import else { return };
        let query = dialog.query.read(cx).value().to_string();
        if !ConnectionRuntime::is_running(cx) || query.trim().is_empty() {
            return;
        }
        dialog.searching = true;
        dialog.error = None;
        let task = ConnectionRuntime::global(cx).spawn(
            async move { search_open_vsx_themes(&query).await.map_err(|e| crate::connection::CommandError(e.0.into())) },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Some(dialog) = &mut this.import {
                    dialog.searching = false;
                    match result {
                        Ok(results) => dialog.results = results,
                        Err(error) => dialog.error = Some(error.0),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn import_extension(&mut self, extension: OpenVsxThemeExtension, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.import else { return };
        dialog.importing = Some(extension.id.clone());
        let task = ConnectionRuntime::global(cx).spawn(
            async move {
                import_open_vsx_theme_extension(&extension).await.map_err(|e| crate::connection::CommandError(e.0.into()))
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(themes) => {
                        let mut added = Vec::new();
                        for theme in &themes {
                            let file = serde_json::to_vec(&theme.to_file()).expect("theme encodes");
                            if let Ok(id) = ThemeStore::add_custom(&file, cx) {
                                added.push((id, theme.theme.label.clone()));
                            }
                        }
                        if let Some((id, label)) = added.first() {
                            use_imported(id, label, cx);
                        }
                        this.import = None;
                    }
                    Err(error) => {
                        if let Some(dialog) = &mut this.import {
                            dialog.importing = None;
                            dialog.error = Some(error.0);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_mode_tiles(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let mode = ThemeStore::global(cx).appearance_mode();
        let light = wireframe_colors(Appearance::Light, cx);
        let dark = wireframe_colors(Appearance::Dark, cx);
        let tile = |m: ThemeMode, label: SharedString, panes: AnyElement| {
            let active = m == mode;
            div()
                .id(SharedString::from(format!("appearance-mode-{}", mode_slug(m))))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(theme.sp(1.5))
                .p(theme.sp(2.))
                .rounded(theme.radii.xl)
                .border_1()
                .cursor_pointer()
                .relative()
                .when(active, |d| {
                    // `ring-2 ring-inset` inside the transparent 1px border.
                    d.border_color(gpui_kit::transparent_black()).bg(with_alpha(c.accent_surface, 0.3)).child(
                        div().absolute().inset_0().rounded(theme.radii.xl - px(1.)).shadow(vec![inset_ring(c.ring(), px(2.))]),
                    )
                })
                .when(!active, |d| d.border_color(with_alpha(c.border, 0.7)).bg(with_alpha(c.card(), 0.6)).hover(|s| s.bg(with_alpha(c.accent_surface, 0.1))))
                .on_click(move |_, _, cx| ThemeStore::set_appearance_mode(m, cx))
                .child(panes)
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                        .text_color(if active { c.foreground() } else { c.muted_foreground })
                        .child(label),
                )
        };
        div()
            .flex()
            .gap(theme.sp(3.))
            .w_full()
            .child(tile(ThemeMode::System, t!("System"), wireframe(&[(light, Some(Half::Left)), (dark, Some(Half::Right))], cx)))
            .child(tile(ThemeMode::Light, t!("Light"), wireframe(&[(light, None)], cx)))
            .child(tile(ThemeMode::Dark, t!("Dark"), wireframe(&[(dark, None)], cx)))
            .into_any_element()
    }

    fn render_card(&self, card: CatalogTheme, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let id = card.id().to_owned();
        let standard = card.document.id == standard_theme().document.id;
        let card_id: Option<String> = (!standard).then(|| id.clone());
        let picked: Vec<Appearance> =
            [Appearance::Light, Appearance::Dark].into_iter().filter(|m| owner_id(*m, cx) == card_id).collect();
        let modes = card.modes();
        let single = (modes.len() == 1).then(|| modes[0]);
        let tooltip_text = match single {
            None => t!("Use for both light and dark"),
            Some(Appearance::Light) => t!("Use for light mode only"),
            Some(Appearance::Dark) => t!("Use for dark mode only"),
        };
        let use_card = {
            let card_id = card_id.clone();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| match (&card_id, single) {
                (None, _) => {
                    let mode = ThemeStore::global(cx).appearance_mode();
                    ThemeStore::set_theme(mode_slug(mode), cx);
                }
                (Some(id), Some(appearance)) => ThemeStore::set_half(appearance, Some(id), cx),
                (Some(id), None) => ThemeStore::set_theme(id, cx),
            }
        };
        let (ring, backdrop) = (c.background(), mix_oklab(c.card(), 0.6, c.background()));
        let circles = modes.iter().map(|mode| {
            let mode = *mode;
            let colors = if standard {
                standard_sphere_colors(mode)
            } else {
                let palette = card.document.palette(mode);
                SphereColors { canvas: palette.canvas, accent: palette.accent, message_action: palette.message_action }
            };
            let is_picked = picked.contains(&mode);
            let card_id = card_id.clone();
            div()
                .id(SharedString::from(format!("theme-{id}-{}", if mode.is_dark() { "dark" } else { "light" })))
                .relative()
                .flex_none()
                .size(px(68.))
                .p(theme.sp(1.5))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .tooltip(tooltip(if mode.is_dark() { t!("Use for dark mode only") } else { t!("Use for light mode only") }))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    assign_half(mode, card_id.clone(), cx);
                })
                .child(theme_sphere(colors, mode, ring, backdrop))
                .when(is_picked, |d| {
                    d.child(div().absolute().inset_0().rounded_full().shadow(vec![inset_ring(c.ring(), px(2.))]))
                        .child(
                            div()
                                .absolute()
                                .bottom(theme.sp(0.5))
                                .right(theme.sp(0.5))
                                .size(theme.sp(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .border_1()
                                .border_color(with_alpha(c.border, 0.7))
                                .bg(c.background())
                                .shadow(theme.shadows.xs5.clone())
                                .child(Icon::new(if mode.is_dark() { IconName::Moon } else { IconName::Sun }).size(IconSize::Xs).color(c.foreground())),
                        )
                })
        });
        let removable = card.source == ThemeSource::Custom;
        let remove_card = card.clone();
        let remove_handle = cx.entity();
        let action = |slug: &str, tip: SharedString, icon: IconName| {
            div().id(SharedString::from(format!("theme-{slug}-tip-{id}"))).tooltip(tooltip(tip)).child(
                Button::new(SharedString::from(format!("theme-{slug}-{id}"))).variant(ButtonVariant::Ghost).size(ButtonSize::IconXs).icon(icon),
            )
        };
        let duplicate_id = id.clone();
        let duplicate = div()
            .id(SharedString::from(format!("theme-duplicate-tip-{id}")))
            .tooltip(tooltip(t!("Duplicate theme")))
            .child(
                Button::new(SharedString::from(format!("theme-duplicate-{id}")))
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconXs)
                    .icon(IconName::Copy)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        duplicate_theme(&duplicate_id, cx);
                    }),
            );
        let _ = action;
        div()
            .id(SharedString::from(format!("theme-card-{id}")))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(theme.radii.xl)
            .border_1()
            .border_color(with_alpha(c.border, 0.7))
            .bg(with_alpha(c.card(), 0.6))
            .hover(|s| s.bg(with_alpha(c.accent_surface, 0.1)))
            .cursor_pointer()
            .tooltip(tooltip(tooltip_text))
            .on_click(use_card)
            .test_support()
            .child(div().flex().min_h(theme.sp(16.)).items_center().justify_center().gap(theme.sp(2.5)).px(theme.sp(3.)).pt(theme.sp(3.)).children(circles))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .px(theme.sp(3.))
                    .pb(theme.sp(3.))
                    .pt(theme.sp(2.))
                    .child(div().flex_1().min_w_0().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(SharedString::from(card.document.name.clone())))
                    .child(div().flex().flex_none().items_center().gap(theme.sp(1.)).child(duplicate).when(removable, |d| {
                        d.child(
                            div().id(SharedString::from(format!("theme-remove-tip-{id}"))).tooltip(tooltip(t!("Remove theme"))).child(
                                Button::new(SharedString::from(format!("theme-remove-{id}")))
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::IconXs)
                                    .icon(IconName::Trash2)
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        remove_handle.update(cx, |this, cx| {
                                            this.remove = Some(remove_card.clone());
                                            cx.notify();
                                        });
                                    }),
                            ),
                        )
                    })),
            )
            .into_any_element()
    }

    fn render_library(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let catalog = ThemeStore::catalog(cx);
        let cards: Vec<AnyElement> = catalog.into_iter().map(|card| self.render_card(card, cx)).collect();
        let tiles = self.render_mode_tiles(cx);
        let open = cx.listener(|this, _, window, cx| this.open_import(window, cx));
        let theme = cx.app_theme();
        let c = &theme.colors;
        let heading = |text: SharedString| div().flex().items_center().min_h(theme.sp(7.)).ui_text(theme, TextSize::Base, weight::SEMIBOLD).text_color(c.foreground()).child(text);
        // `repeat(auto-fit, minmax(min(100%, 16rem), 1fr))` with an 8px gap,
        // over the page's width (see `settings_content_width`).
        let gap = theme.sp(2.);
        let width = settings_group_width(window, cx);
        let columns = (((width + gap) / (theme.rem(16.) + gap)).floor() as usize).max(1);
        let mut rows = Vec::new();
        let mut iter = cards.into_iter().peekable();
        while iter.peek().is_some() {
            let row: Vec<AnyElement> = iter.by_ref().take(columns).collect();
            let fill = columns - row.len();
            rows.push(
                div()
                    .flex()
                    .gap(gap)
                    .children(row.into_iter().map(|card| div().flex_1().min_w_0().child(card)))
                    .children((0..fill).map(|_| div().flex_1()))
                    .into_any_element(),
            );
        }
        let create = Button::new("theme-create").variant(ButtonVariant::Outline).size(ButtonSize::Xs).icon(IconName::Paintbrush).label(t!("Create theme")).on_click(|_, _, cx| {
            let seed = owner_id(cx.app_theme().appearance, cx).unwrap_or_else(|| standard_theme().id().to_owned());
            duplicate_theme(&seed, cx);
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(3.))
            .child(div().px(theme.sp(4.)).child(heading(setting_title("color-scheme"))))
            .child(tiles)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .min_h(theme.sp(8.))
                    .items_center()
                    .justify_between()
                    .gap(theme.sp(3.))
                    .px(theme.sp(4.))
                    .pt(theme.sp(5.))
                    .child(div().ui_text(theme, TextSize::Base, weight::SEMIBOLD).text_color(c.foreground()).child(setting_title("theme")))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_end()
                            .gap(theme.sp(2.))
                            .child(create)
                            .child(Button::new("theme-add").variant(ButtonVariant::Outline).size(ButtonSize::Xs).icon(IconName::Plus).label(t!("Add theme")).on_click(open)),
                    ),
            )
            .child(div().flex().flex_col().gap(gap).children(rows))
            .into_any_element()
    }

    fn render_interface(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = ClientSettingsStore::get(cx).clone();
        let theme = cx.app_theme();
        let enabled = thread_details_enabled(&settings);
        let total = THREAD_DETAILS_ITEMS.len();
        let details_menu = dropdown_menu(
            "thread-details-menu",
            Button::new("thread-details-trigger")
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .label(format!("{enabled} of {total} on"))
                .trailing_icon(IconName::ChevronDown),
            |_, cx| {
                let settings = ClientSettingsStore::get(cx).clone();
                THREAD_DETAILS_ITEMS.iter().fold(MenuPopup::new(), |menu, (id, _, default)| {
                    let id = *id;
                    let default = *default;
                    let visible = thread_detail_visible(&settings, id, default);
                    menu.item(MenuItem::new(thread_detail_label(id)).kind(MenuItemKind::Checkbox(visible)).on_select(move |_, cx| {
                        ClientSettingsStore::update(cx, |s| {
                            let default = thread_detail_default(s, id, default);
                            if !visible == default {
                                s.thread_details_items.remove(id);
                            } else {
                                s.thread_details_items.insert(id.to_owned(), !visible);
                            }
                        })
                    }))
                })
            },
        );
        let mut details_row = SettingsRow::new(setting_title("thread-details-items"))
            .description(t!("Choose what the thread details card shows. You can also right-click the card."))
            .control(div().min_w(theme.rem(10.)).child(details_menu));
        if !THREAD_DETAILS_ITEMS.iter().all(|(id, _, default)| thread_detail_visible(&settings, id, *default) == *default) {
            details_row = details_row.reset(reset_button("thread-details-items", |_, cx| {
                ClientSettingsStore::update(cx, |s| {
                    s.thread_details_items.clear();
                    s.thread_details_usage_enabled = false;
                })
            }));
        }
        // `flex w-full items-center gap-3 sm:w-52`: the value chip, then the slider.
        let slider_row = |id: &'static str, value: u64, text: String, chip_rem: f32, range: (u64, u64, u64), set: fn(&mut ClientSettings, u64)| {
            div()
                .flex()
                .items_center()
                .gap(theme.sp(3.))
                .w(theme.rem(13.))
                .child(slider_value(text, chip_rem, cx))
                .child(SettingsSlider::new(SharedString::from(format!("slider-{id}")), value, range, move |v, _, cx| {
                    ClientSettingsStore::update(cx, |s| set(s, v))
                }))
        };
        let contrast = client_row(
            "setting-appearance-contrast",
            t!("Adjust the contrast of colors and borders across the interface."),
            |s| &s.appearance_contrast,
            |s, v| s.appearance_contrast = v,
            slider_row("contrast", settings.appearance_contrast, format!("{}%", settings.appearance_contrast), 3., (50, 150, 5), |s, v| s.appearance_contrast = v),
            cx,
        );
        let glass = client_row(
            "setting-glass-opacity",
            t!("Higher values make menus, dialogs, and the composer more solid."),
            |s| &s.glass_opacity,
            |s, v| s.glass_opacity = v,
            slider_row("glass", settings.glass_opacity, format!("{}%", settings.glass_opacity), 3., (0, 100, 5), |s, v| s.glass_opacity = v),
            cx,
        );
        // `grid sm:grid-cols-[7rem_13rem] sm:gap-4`: the preview, then the chip and slider.
        let animation = div()
            .flex()
            .items_center()
            .gap(theme.sp(4.))
            .child(div().w(theme.rem(7.)).child(panel_animations_preview(cx)))
            .child(
                div()
                    .w(theme.rem(13.))
                    .flex()
                    .items_center()
                    .gap(theme.sp(3.))
                    .child(slider_value(format!("{} ms", settings.panel_animation_duration_ms), 4., cx))
                    .child(SettingsSlider::new("slider-panel-animations", settings.panel_animation_duration_ms, (0, 400, 25), |v, _, cx| {
                        ClientSettingsStore::update(cx, |s| s.panel_animation_duration_ms = v)
                    })),
            );
        let toggle_advanced: Rc<dyn Fn(&mut Window, &mut App)> = {
            let this = cx.entity();
            Rc::new(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.advanced_open = !this.advanced_open;
                    cx.notify();
                })
            })
        };
        let advanced = advanced_settings(
            "appearance-advanced",
            self.advanced_open,
            toggle_advanced,
            vec![
                client_row(
                    "panel-animations",
                    t!("Set how fast panels open and close."),
                    |s| &s.panel_animation_duration_ms,
                    |s, v| s.panel_animation_duration_ms = v,
                    animation,
                    cx,
                )
                .into_any_element(),
                // This app has no Dev or Nightly stage label, so the web's
                // "Latest" choices apply: the default art reads None.
                client_select(
                    "environment-identification",
                    t!("Show Dev or Nightly artwork in the sidebar header."),
                    SelectWidth::Rem(10.),
                    vec![
                        (EnvironmentIdentificationMode::Artwork, t!("None")),
                        (EnvironmentIdentificationMode::NightlyArtwork, t!("Nightly artwork")),
                        (EnvironmentIdentificationMode::DevArtwork, t!("Dev artwork")),
                    ],
                    |s| &s.environment_identification_mode,
                    |s, v| s.environment_identification_mode = v,
                    cx,
                )
                .into_any_element(),
                client_switch(
                    "composer-context",
                    t!("Keep branch and worktree controls below the composer after a thread starts."),
                    |s| &s.persist_composer_context_strip,
                    |s, v| s.persist_composer_context_strip = v,
                    cx,
                )
                .into_any_element(),
            ],
        );
        let chat_width = crate::components::ui::select::Select::new(
            "select-chat-width",
            Rc::from([t!("Comfortable (default)"), t!("Wide"), t!("Full", context = "chat width")]),
            match settings.chat_width {
                ChatWidth::Wide => 1,
                ChatWidth::Full => 2,
                _ => 0,
            },
            |index, _, cx| {
                let value = match index {
                    1 => ChatWidth::Wide,
                    2 => ChatWidth::Full,
                    _ => ChatWidth::Comfortable,
                };
                ClientSettingsStore::update(cx, |s| s.chat_width = value)
            },
        )
        .size(crate::components::ui::select::SelectSize::Sm)
        .value_label(match settings.chat_width {
            ChatWidth::Wide => t!("Wide"),
            ChatWidth::Full => t!("Full", context = "chat width"),
            _ => t!("Comfortable"),
        });
        let effort = crate::components::ui::select::Select::new(
            "select-effort-display",
            Rc::from([t!("Word (default)"), t!("Bars", context = "show effort as bars")]),
            usize::from(settings.effort_display == EffortDisplay::Bars),
            |index, _, cx| {
                let value = if index == 1 { EffortDisplay::Bars } else { EffortDisplay::Label };
                ClientSettingsStore::update(cx, |s| s.effort_display = value)
            },
        )
        .size(crate::components::ui::select::SelectSize::Sm)
        .value_label(if settings.effort_display == EffortDisplay::Bars {
            t!("Bars", context = "show effort as bars")
        } else {
            t!("Word", context = "show effort as a word")
        });
        SettingsSection::new(t!("Interface"))
            .child(contrast)
            .child(glass)
            .child(client_row(
                "chat-width",
                t!("Set how wide messages and the composer can grow on large screens."),
                |s| &s.chat_width,
                |s, v| s.chat_width = v,
                sized_select(SelectWidth::Rem(10.), chat_width),
                cx,
            ))
            .child(details_row)
            .child(client_row(
                "effort-display",
                t!("Show reasoning effort in the composer as its name, such as High, or as bars."),
                |s| &s.effort_display,
                |s, v| s.effort_display = v,
                sized_select(SelectWidth::Rem(10.), effort),
                cx,
            ))
            .child(client_switch(
                "math-rendering",
                t!("Typeset LaTeX math, such as $x^2$ or \\[x^2\\], in messages and Markdown previews. The composer keeps TeX source editable."),
                |s| &s.math_rendering_enabled,
                |s, v| s.math_rendering_enabled = v,
                cx,
            ))
            .child(advanced)
            .into_any_element()
    }

    /// `FontFamilySettingsRow`: the family field (`sm:w-44`) and the size
    /// select (`w-22`, raised to the row's 10rem select minimum).
    fn font_row(&self, role: FontRole, id: &'static str, title: Option<SharedString>, description: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let settings = ClientSettingsStore::get(cx).clone();
        let defaults = ClientSettings::defaults();
        let theme = cx.app_theme();
        let input = self.fonts[FontRole::ALL.iter().position(|r| *r == role).expect("role")].clone();
        let size = role.size(&settings);
        let (min, max) = role.range();
        let sizes: Rc<[SharedString]> = (min..=max).map(|px| SharedString::from(format!("{px} px"))).collect();
        let select = crate::components::ui::select::Select::new(
            SharedString::from(format!("select-{id}-size")),
            sizes,
            size.saturating_sub(min) as usize,
            move |index, _, cx| ClientSettingsStore::update(cx, |s| role.set_size(s, min + index as u64)),
        )
        .size(crate::components::ui::select::SelectSize::Sm);
        let control = div()
            .flex()
            .items_center()
            .gap(theme.sp(2.))
            .child(div().w(theme.rem(11.)).child(TextInput::new(&input).size(crate::components::ui::input::InputSize::Sm)))
            .child(sized_select(SelectWidth::Rem(10.), select));
        let changed = role.size(&settings) != role.size(defaults) || role.family(&settings) != role.family(defaults);
        let mut row = SettingsRow::new(title.unwrap_or_else(|| setting_title(id))).description(description).control(control);
        if changed {
            row = row.reset(reset_button(id, move |_, cx| {
                let defaults = ClientSettings::defaults();
                ClientSettingsStore::update(cx, |s| {
                    role.set_size(s, role.size(defaults));
                    role.set_family(s, role.family(defaults).to_owned());
                })
            }));
        }
        row.into_any_element()
    }

    fn render_typography(&self, cx: &mut Context<Self>) -> AnyElement {
        let advanced = separate_fonts(cx);
        let mut section = SettingsSection::new(t!("Typography"))
            .child(self.font_row(FontRole::Interface, "interface-font", None, t!("Everything outside code blocks and the terminal."), cx));
        if advanced {
            section = section
                .child(self.font_row(FontRole::Prompt, "prompt-font", None, t!("Only the box you write prompts in. Mono works well here."), cx))
                .child(self.font_row(FontRole::Code, "code-font", None, t!("Code blocks, diffs, and file previews."), cx))
                .child(self.font_row(FontRole::Terminal, "terminal-font", None, t!("Terminal output, independent from code blocks and diffs."), cx));
        } else {
            section = section.child(self.font_row(
                FontRole::Code,
                "code-font",
                Some(t!("Monospace font")),
                t!("Code blocks, diffs, file previews, and the terminal."),
                cx,
            ));
        }
        let separate = SettingsRow::new(setting_title("separate-fonts"))
            .description(t!("Set the prompt, code, and terminal fonts separately instead of one monospace font."))
            .control(crate::components::ui::switch::Switch::new("switch-separate-fonts", advanced).on_change(|on, _, cx| set_separate_fonts(on, cx)))
            .into_any_element();
        let mut advanced_rows = vec![separate];
        // `FontSmoothingRow` renders on macOS only.
        if cfg!(target_os = "macos") {
            advanced_rows.push(
                client_switch(
                    "font-smoothing",
                    t!("Use thinner grayscale text smoothing instead of the macOS default."),
                    |s| &s.font_smoothing,
                    |s, v| s.font_smoothing = v,
                    cx,
                )
                .into_any_element(),
            );
        }
        let toggle: Rc<dyn Fn(&mut Window, &mut App)> = {
            let this = cx.entity();
            Rc::new(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.typography_advanced_open = !this.typography_advanced_open;
                    cx.notify();
                })
            })
        };
        section
            .child(client_switch(
                "word-wrap",
                t!("Wrap long lines in code blocks, diffs, and file previews by default."),
                |s| &s.word_wrap,
                |s, v| s.word_wrap = v,
                cx,
            ))
            .child(advanced_settings("typography-advanced", self.typography_advanced_open, toggle, advanced_rows))
            .into_any_element()
    }

    fn render_import_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.import.as_ref()?;
        let theme = cx.app_theme();
        let c = &theme.colors;
        let close = cx.listener(|this, _: &(), _, cx| {
            this.import = None;
            cx.notify();
        });
        let close = Rc::new(close);
        let cancel = close.clone();
        let paste_submit = cx.listener(|this, _: &gpui_kit::ClickEvent, _, cx| this.submit_paste(cx));
        let search = cx.listener(|this, _: &gpui_kit::ClickEvent, _, cx| this.search(cx));
        let results = dialog.results.iter().map(|extension| {
            let picked = extension.clone();
            let importing = dialog.importing.as_deref() == Some(extension.id.as_str());
            let import = cx.listener(move |this, _: &gpui_kit::ClickEvent, _, cx| this.import_extension(picked.clone(), cx));
            div()
                .flex()
                .items_center()
                .gap(theme.sp(3.))
                .py(theme.sp(2.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(extension.name.clone()))
                        .child(div().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(format!("{} · {}", extension.publisher, extension.license))),
                )
                .child(Button::new(SharedString::from(format!("ovx-import-{}", extension.id))).variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(if importing { t!("Installing...") } else { t!("Install") }).disabled(dialog.importing.is_some()).on_click(import))
        });
        let body = div()
            .flex()
            .flex_col()
            .gap(theme.sp(3.))
            .w(px(480.))
            .child(TextArea::new(&dialog.paste))
            .child(div().flex().justify_end().child(Button::new("theme-import-paste").size(ButtonSize::Sm).label(t!("Add theme")).on_click(paste_submit)))
            .child(div().ui_text(theme, TextSize::Sm, weight::SEMIBOLD).child(t!("Search community themes")))
            .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("Find open-source themes from Open VSX.")))
            .child(
                div()
                    .flex()
                    .gap(theme.sp(2.))
                    .child(div().flex_1().child(TextInput::new(&dialog.query)))
                    .child(Button::new("theme-import-search").variant(ButtonVariant::Outline).size(ButtonSize::Sm).icon(IconName::Search).label(t!("Search")).disabled(dialog.searching).on_click(search)),
            )
            .children(results)
            .children(dialog.error.clone().map(|error| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(error)));
        let surface = DialogSurface::new(t!("Add a theme"))
            .footer(div().flex().justify_end().child(Button::new("theme-import-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(move |_, w, cx| cancel(&(), w, cx))))
            .child(body);
        Some(Modal::dialog("theme-import-dialog", surface, move |w, cx| close(&(), w, cx)).into_any_element())
    }

    fn render_remove_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = self.remove.clone()?;
        let theme = cx.app_theme();
        let close = Rc::new(cx.listener(|this, _: &(), _, cx| {
            this.remove = None;
            cx.notify();
        }));
        let cancel = close.clone();
        let id = card.id().to_owned();
        let confirm = cx.listener(move |this, _: &gpui_kit::ClickEvent, _, cx| {
            ThemeStore::delete_custom(&id, cx);
            this.remove = None;
            cx.notify();
        });
        let surface = DialogSurface::new(t!("Remove “{label}”?", label = card.document.name.clone()))
            .description(t!("You can bring it back anytime by importing its JSON file."))
            .footer(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.sp(2.))
                    .child(Button::new("theme-remove-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(move |_, w, cx| cancel(&(), w, cx)))
                    .child(Button::new("theme-remove-confirm").variant(ButtonVariant::Destructive).label(t!("Remove theme")).on_click(confirm)),
            );
        Some(Modal::dialog("theme-remove-dialog", surface, move |w, cx| close(&(), w, cx)).into_any_element())
    }
}

impl Render for AppearanceSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = self.render_library(window, cx);
        let interface = self.render_interface(cx);
        let typography = self.render_typography(cx);
        let import = self.render_import_dialog(cx);
        let remove = self.render_remove_dialog(cx);
        // `SettingsSection hideTitle variant="plain"` titled "Colors & themes".
        let library = SettingsSection::new(t!("Colors & themes")).plain().hide_title().child(library);
        page_container(cx).child(library).child(interface).child(typography).children(import).children(remove)
    }
}

/// Saves an import as the active theme, as the import dialog does.
fn use_imported(id: &str, label: &str, cx: &mut App) {
    let Some(theme) = ThemeStore::find(id, cx) else { return };
    let modes = theme.modes();
    let description = if modes.len() == 1 {
        ThemeStore::set_half(modes[0], Some(id), cx);
        if modes[0].is_dark() { t!("It’s now your dark theme.") } else { t!("It’s now your light theme.") }
    } else {
        ThemeStore::set_theme(id, cx);
        t!("It’s now active.")
    };
    Toaster::push(ToastData::new(ToastKind::Success, t!("{label} added", label = label.to_owned())).description(description), cx);
}

fn mode_slug(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

/// The id of the theme drawing `appearance` (`lightOwner`/`darkOwner`);
/// `None` is the standard theme.
fn owner_id(appearance: Appearance, cx: &App) -> Option<String> {
    let store = ThemeStore::global(cx);
    if let Some(half) = store.halves().get(appearance) {
        return Some(half.to_owned());
    }
    let base = ThemeStore::find(store.preference(), cx)?;
    (base.document.id != standard_theme().document.id && base.document.palette_for(appearance).is_some())
        .then(|| base.document.id.clone())
}

fn owner_palette(appearance: Appearance, cx: &App) -> ThemePalette {
    owner_id(appearance, cx)
        .and_then(|id| ThemeStore::find(&id, cx))
        .unwrap_or_else(|| standard_theme().clone())
        .document
        .palette(appearance)
}

/// `assignHalf`.
fn assign_half(appearance: Appearance, card_id: Option<String>, cx: &mut App) {
    let base = ThemeStore::find(ThemeStore::global(cx).preference(), cx)
        .filter(|base| base.document.id != standard_theme().document.id)
        .map(|base| base.document.id.clone());
    if card_id.is_none()
        && let Some(base) = base
    {
        let other = if appearance.is_dark() { Appearance::Light } else { Appearance::Dark };
        let other_owner = ThemeStore::global(cx).halves().get(other).map(str::to_owned).unwrap_or(base);
        let mode = mode_slug(ThemeStore::global(cx).appearance_mode());
        ThemeStore::set_theme(mode, cx);
        ThemeStore::set_half(other, Some(&other_owner), cx);
        return;
    }
    ThemeStore::set_half(appearance, card_id.as_deref(), cx);
}

/// `box-shadow: inset 0 0 0 <width> <color>`, a ring inside the border
/// box. GPUI insets from the border box, so a bordered element adds its
/// border width.
fn inset_ring(color: Hsla, width: Pixels) -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow::new(px(0.), px(0.), color).spread_radius(width).inset()
}

/// Which half of a System tile a wireframe pane fills.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Half {
    Left,
    Right,
}

/// `ThemeCardPreviewColors`: the roles the wireframe draws.
#[derive(Clone, Copy)]
struct WireframeColors {
    sidebar: Hsla,
    canvas: Hsla,
    surface: Hsla,
    accent_surface: Hsla,
    message_surface: Hsla,
    message_action: Hsla,
}

/// The colours of the theme that owns `appearance`, or the standard
/// theme's fixed preview colours.
fn wireframe_colors(appearance: Appearance, cx: &App) -> WireframeColors {
    let hex = |v: &str| crate::syntax::hex(v).unwrap_or_default();
    match owner_id(appearance, cx) {
        None if appearance.is_dark() => WireframeColors {
            sidebar: hex("#0f0f10"),
            canvas: hex("#0a0a0a"),
            surface: hex("#121212"),
            accent_surface: hex("#27272a"),
            message_surface: hex("#27272a"),
            message_action: hex("#8b9cff"),
        },
        None => WireframeColors {
            sidebar: hex("#fafafa"),
            canvas: hex("#fcfcfc"),
            surface: hex("#ffffff"),
            accent_surface: hex("#f4f4f5"),
            message_surface: hex("#e4e4e7"),
            message_action: hex("#4f46e5"),
        },
        Some(_) => {
            let p = owner_palette(appearance, cx);
            WireframeColors {
                sidebar: p.sidebar,
                canvas: p.canvas,
                surface: p.surface,
                accent_surface: p.accent_surface,
                message_surface: p.message_surface,
                message_action: p.message_action,
            }
        }
    }
}

/// `ThemeWireframe`: a 140px sketch of the app in a theme's colours, its
/// shapes placed in percentages of the tile. A System tile clips the light
/// pane to the left half and the dark pane to the right, 2px apart.
fn wireframe(panes: &[(WireframeColors, Option<Half>)], cx: &App) -> AnyElement {
    let theme = cx.app_theme();
    let line = gpui_kit::hsla(0., 0., 127. / 255., 0.25);
    let pane = |colors: WireframeColors| {
        let rect = |left: f32, top: f32, w: f32, h: f32| {
            div().absolute().left(gpui_kit::relative(left)).top(gpui_kit::relative(top)).w(gpui_kit::relative(w)).h(gpui_kit::relative(h))
        };
        let outline = |d: gpui_kit::Div| d.shadow(vec![inset_ring(line, px(1.))]);
        div()
            .absolute()
            .inset_0()
            .bg(colors.canvas)
            .child(div().absolute().top_0().bottom_0().left_0().w(gpui_kit::relative(0.22)).bg(colors.sidebar).border_r_1().border_color(line))
            .child(outline(rect(0.03, 0.08, 0.16, 0.08).rounded(theme.radii.md).bg(colors.surface)))
            .child(rect(0.03, 0.22, 0.16, 0.07).rounded(theme.radii.md).bg(colors.accent_surface))
            .child(rect(0.03, 0.32, 0.16, 0.07).rounded(theme.radii.md).bg(with_alpha(colors.message_surface, 0.7)))
            .child(rect(0.03, 0.42, 0.16, 0.07).rounded(theme.radii.md).bg(with_alpha(colors.message_surface, 0.5)))
            .child(rect(0.48, 0.11, 0.24, 0.09).rounded(theme.radii.lg).bg(colors.message_surface))
            .child(rect(0.27, 0.28, 0.34, 0.05).rounded(theme.radii.sm).bg(line))
            .child(rect(0.27, 0.38, 0.26, 0.05).rounded(theme.radii.sm).bg(line))
            .child(
                outline(rect(0.26, 0.77, 0.68, 0.15).rounded(theme.radii.md).bg(colors.surface))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(theme.sp(1.))
                    .child(div().h(gpui_kit::relative(0.26)).w(gpui_kit::relative(0.34)).rounded_full().bg(with_alpha(line, 0.7 * 0.25)))
                    .child(div().size(px(11.)).rounded_full().bg(colors.message_action)),
            )
            .child(
                outline(rect(0.75, 0.08, 0.20, 0.46).rounded(theme.radii.lg).bg(colors.surface))
                    .shadow(vec![inset_ring(line, px(1.)), gpui_kit::BoxShadow {
                        color: gpui_kit::hsla(0., 0., 0., 0.14),
                        offset: gpui_kit::point(px(0.), px(2.)),
                        blur_radius: px(5.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .children([gpui_kit::rgb(0x34d399).into(), colors.message_action, gpui_kit::rgb(0xfbbf24).into()].into_iter().enumerate().map(|(row, dot): (usize, Hsla)| {
                        div()
                            .absolute()
                            .left(gpui_kit::relative(0.11))
                            .right(gpui_kit::relative(0.11))
                            .top(gpui_kit::relative(0.10 + row as f32 * 0.30))
                            .h(gpui_kit::relative(0.20))
                            .flex()
                            .items_center()
                            .gap(theme.sp(1.))
                            .child(div().size(px(4.)).flex_none().rounded_full().bg(with_alpha(dot, 0.55)))
                            .child(div().h(gpui_kit::relative(0.30)).w(gpui_kit::relative(0.52)).rounded_sm().bg(line))
                    })),
            )
    };
    div()
        .relative()
        .w_full()
        .h(theme.rem(8.75))
        .overflow_hidden()
        .rounded(theme.radii.lg)
        .border_1()
        .border_color(with_alpha(theme.colors.border, 0.6))
        .children(panes.iter().map(|(colors, half)| match half {
            None => pane(*colors).into_any_element(),
            Some(Half::Left) => div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right(gpui_kit::relative(0.5))
                .mr(px(1.))
                .overflow_hidden()
                .child(div().absolute().top_0().bottom_0().left_0().w(gpui_kit::relative(2.)).child(pane(*colors)))
                .into_any_element(),
            Some(Half::Right) => div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .left(gpui_kit::relative(0.5))
                .ml(px(1.))
                .overflow_hidden()
                .child(div().absolute().top_0().bottom_0().right_0().w(gpui_kit::relative(2.)).child(pane(*colors)))
                .into_any_element(),
        }))
        .into_any_element()
}

/// `PanelAnimationsPreview` at rest with the panels open: a 40px frame
/// with a sidebar, three lines, a composer strip and a side panel.
fn panel_animations_preview(cx: &App) -> AnyElement {
    let theme = cx.app_theme();
    let c = &theme.colors;
    let line = |w: f32, alpha: f32| div().h(px(2.)).w(gpui_kit::relative(w)).rounded_full().bg(with_alpha(c.muted_foreground, alpha));
    div()
        .flex()
        .h(theme.rem(2.5))
        .w_full()
        .overflow_hidden()
        .rounded(theme.radii.lg)
        .border_1()
        .border_color(c.border)
        .bg(c.background())
        .p(theme.sp(1.))
        .shadow(theme.shadows.xs5.clone())
        .child(div().h_full().w(theme.rem(1.)).flex_none().rounded(theme.radii.md).bg(c.sidebar))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .px(theme.sp(1.))
                .child(div().flex().flex_col().flex_1().gap(theme.sp(1.)).pt(theme.sp(1.)).child(line(1., 0.25)).child(line(0.8, 0.2)).child(line(0.6, 0.15)))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(theme.sp(2.))
                        .px(theme.sp(2.))
                        .border_t_1()
                        .border_color(with_alpha(c.border, 0.7))
                        .bg(with_alpha(c.foreground(), 0.05))
                        .child(div().h(px(1.)).w(gpui_kit::relative(0.66)).rounded_full().bg(with_alpha(c.muted_foreground, 0.25))),
                ),
        )
        .child(div().h_full().w(theme.rem(1.25)).flex_none().rounded(theme.radii.md).bg(c.muted))
        .into_any_element()
}

/// Copies a theme as a custom theme and uses it. The web opens the theme
/// editor seeded with it instead; this app has no editor.
fn duplicate_theme(id: &str, cx: &mut App) {
    match ThemeStore::duplicate(id, cx) {
        Ok(new_id) => {
            if let Some(theme) = ThemeStore::find(&new_id, cx) {
                use_imported(&new_id, &theme.document.name.clone(), cx);
            }
        }
        Err(error) => {
            Toaster::push(ToastData::new(ToastKind::Error, t!("Could not create the theme.")).description(error.to_string()), cx);
        }
    }
}

/// The width of a settings section's card on the current page: the modal's
/// content column less the scrollbar gutters, capped at `max-w-4xl`, less
/// the page padding. Mirrors the sizes `routes/settings.rs` lays out.
pub fn settings_group_width(window: &Window, cx: &App) -> Pixels {
    let theme = cx.app_theme();
    let viewport = window.viewport_size();
    let inset = if viewport.width >= px(1024.) { px(40.) } else { px(24.) };
    let modal = (viewport.width - inset * 2.).min(theme.rem(80.));
    let content = modal - px(2.) - theme.rem(15.);
    (content - px(2. * super::settings_layout::SCROLLBAR_GUTTER)).min(theme.rem(56.)) - theme.sp(12.)
}

/// `THREAD_DETAILS_ITEMS`: id, label source, default visibility.
const THREAD_DETAILS_ITEMS: [(&str, &str, bool); 12] = [
    ("environment", "Environment and worktree", true),
    ("openIn", "Open in editor", true),
    ("scripts", "Project scripts", true),
    ("branch", "Branch and pull request", true),
    ("gitActions", "Git actions", true),
    ("changes", "Changes", true),
    ("servers", "Servers", true),
    ("automations", "Automations", true),
    ("lineage", "Lineage", true),
    ("sideChats", "Side chats", true),
    ("usage", "Usage limits", false),
    ("context", "Context window", false),
];

fn thread_detail_label(id: &str) -> SharedString {
    match id {
        "environment" => t!("Environment and worktree"),
        "openIn" => t!("Open in editor"),
        "scripts" => t!("Project scripts"),
        "branch" => t!("Branch and pull request"),
        "gitActions" => t!("Git actions"),
        "changes" => t!("Changes", context = "thread details item: version control changes"),
        "servers" => t!("Servers", context = "thread details item: running dev servers"),
        "automations" => t!("Automations"),
        "lineage" => t!("Lineage", context = "thread details item: parent and child agents"),
        "sideChats" => t!("Side chats"),
        "usage" => t!("Usage limits"),
        _ => t!("Context window"),
    }
}

/// The retired Labs toggle still turns Usage and Context on by default.
fn thread_detail_default(settings: &ClientSettings, id: &str, default: bool) -> bool {
    if id == "usage" || id == "context" { settings.thread_details_usage_enabled } else { default }
}

fn thread_detail_visible(settings: &ClientSettings, id: &str, default: bool) -> bool {
    settings.thread_details_items.get(id).copied().unwrap_or_else(|| thread_detail_default(settings, id, default))
}

fn thread_details_enabled(settings: &ClientSettings) -> usize {
    THREAD_DETAILS_ITEMS.iter().filter(|(id, _, default)| thread_detail_visible(settings, id, *default)).count()
}
