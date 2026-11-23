//! Appearance. Ports `AppearanceSettingsPanel.tsx`, the theme library of
//! `ThemeSettings.tsx` with `ThemePreviewCircles` and `ThemeWireframe`,
//! `ThemeImportDialog` (paste a T3 or VS Code theme, or search Open VSX)
//! and `TypographySettings.tsx`.
//!
//! Left out because this app cannot honour them: Contrast (a CSS filter
//! over every colour), Glass opacity (no backdrop blur), custom theme CSS,
//! background images, the theme editor, and font smoothing (GPUI picks the
//! rasteriser's smoothing).

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::{
    TestSupportExt as _, Focusable as _, AnyElement, App, AppContext as _, Context, Entity, Hsla, IntoElement, Render, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use t3code_contracts::{ChatWidth, EffortDisplay, EnvironmentIdentificationMode};

use super::settings_controls::{client_row, client_select, client_switch, reset_button};
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
    animation: Entity<SliderState>,
    fonts: Vec<Entity<InputState>>,
    advanced_open: bool,
    import: Option<ImportDialog>,
    /// A custom theme waiting for "Remove theme" to be confirmed.
    remove: Option<CatalogTheme>,
    _subscriptions: Vec<Subscription>,
}

impl AppearanceSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = ClientSettingsStore::get(cx).clone();
        let animation = cx.new(|_| {
            SliderState::new().min(0.).max(400.).step(25.).default_value(settings.panel_animation_duration_ms as f32)
        });
        let mut subscriptions = vec![cx.subscribe(&animation, |_, _, event: &SliderEvent, cx| {
            let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
            let ms = value.start().round() as u64;
            ClientSettingsStore::update(cx, |s| s.panel_animation_duration_ms = ms);
        })];
        let fonts: Vec<Entity<InputState>> = FontRole::ALL
            .iter()
            .map(|role| {
                let value = role.family(&settings).to_owned();
                cx.new(|cx| InputState::new(window, cx).placeholder(t!("System default")).default_value(value))
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
        Self { animation, fonts, advanced_open: false, import: None, remove: None, _subscriptions: subscriptions }
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
        let ms = settings.panel_animation_duration_ms as f32;
        if (self.animation.read(cx).value().start() - ms).abs() > 0.5 {
            self.animation.update(cx, |state, cx| state.set_value(ms, window, cx));
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
        let light = owner_palette(Appearance::Light, cx);
        let dark = owner_palette(Appearance::Dark, cx);
        let tile = |m: ThemeMode, label: SharedString, panes: AnyElement| {
            let active = m == mode;
            div()
                .id(SharedString::from(format!("appearance-mode-{}", mode_slug(m))))
                .flex_1()
                .flex()
                .flex_col()
                .gap(theme.sp(1.5))
                .p(theme.sp(2.))
                .rounded(theme.radii.xl)
                .border_1()
                .cursor_pointer()
                .when(active, |d| d.border_2().border_color(c.ring()).bg(with_alpha(c.accent_surface, 0.3)))
                .when(!active, |d| d.border_color(with_alpha(c.border, 0.7)).bg(with_alpha(c.card(), 0.6)))
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
            .child(tile(
                ThemeMode::System,
                t!("System"),
                div().flex().child(div().w_1_2().overflow_hidden().child(wireframe(&light, true))).child(div().w_1_2().overflow_hidden().child(wireframe(&dark, false))).into_any_element(),
            ))
            .child(tile(ThemeMode::Light, t!("Light"), wireframe(&light, true)))
            .child(tile(ThemeMode::Dark, t!("Dark"), wireframe(&dark, true)))
            .into_any_element()
    }

    fn render_card(&self, card: CatalogTheme, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let id = card.id().to_owned();
        let card_id: Option<String> = (card.document.id != standard_theme().document.id).then(|| id.clone());
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
        let circles = modes.iter().map(|mode| {
            let mode = *mode;
            let palette = card.document.palette(mode);
            let is_picked = picked.contains(&mode);
            let card_id = card_id.clone();
            div()
                .id(SharedString::from(format!("theme-{id}-{}", if mode.is_dark() { "dark" } else { "light" })))
                .relative()
                .size(px(68.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(is_picked, |d| d.border_2().border_color(c.ring()))
                .tooltip(tooltip(if mode.is_dark() { t!("Use for dark mode only") } else { t!("Use for light mode only") }))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    assign_half(mode, card_id.clone(), cx);
                })
                .child(preview_circle(&palette, mode, c.background()))
                .when(is_picked, |d| {
                    d.child(
                        div()
                            .absolute()
                            .bottom(px(2.))
                            .right(px(2.))
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border_1()
                            .border_color(with_alpha(c.border, 0.7))
                            .bg(c.background())
                            .child(Icon::new(if mode.is_dark() { IconName::Moon } else { IconName::Sun }).size(IconSize::Xs).color(c.foreground())),
                    )
                })
        });
        let removable = card.source == ThemeSource::Custom;
        let remove_card = card.clone();
        let remove_handle = cx.entity();
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
            .child(div().flex().min_h(px(64.)).items_center().justify_center().gap(theme.sp(2.5)).px(theme.sp(3.)).pt(theme.sp(3.)).children(circles))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .px(theme.sp(3.))
                    .pb(theme.sp(3.))
                    .pt(theme.sp(2.))
                    .child(div().flex_1().min_w_0().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(SharedString::from(card.document.name.clone())))
                    .when(removable, |d| {
                        d.child(
                            div().id(SharedString::from(format!("theme-remove-tip-{id}"))).tooltip(tooltip(t!("Remove theme"))).child(
                                Button::new(SharedString::from(format!("theme-remove-{id}")))
                                    .variant(ButtonVariant::GhostMuted)
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
                    }),
            )
            .into_any_element()
    }

    fn render_library(&self, cx: &mut Context<Self>) -> AnyElement {
        let catalog = ThemeStore::catalog(cx);
        let cards: Vec<AnyElement> = catalog.into_iter().map(|card| self.render_card(card, cx)).collect();
        let tiles = self.render_mode_tiles(cx);
        let open = cx.listener(|this, _, window, cx| this.open_import(window, cx));
        let theme = cx.app_theme();
        let c = &theme.colors;
        let heading = |text: SharedString| div().flex().items_center().min_h(theme.sp(7.)).ui_text(theme, TextSize::Base, weight::SEMIBOLD).text_color(c.foreground()).child(text);
        let mut rows = Vec::new();
        let mut iter = cards.into_iter().peekable();
        while iter.peek().is_some() {
            let row: Vec<AnyElement> = iter.by_ref().take(3).collect();
            let fill = 3 - row.len();
            rows.push(
                div()
                    .flex()
                    .gap(theme.sp(2.))
                    .children(row.into_iter().map(|card| div().flex_1().min_w_0().child(card)))
                    .children((0..fill).map(|_| div().flex_1()))
                    .into_any_element(),
            );
        }
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
                    .items_center()
                    .justify_between()
                    .gap(theme.sp(3.))
                    .px(theme.sp(4.))
                    .pt(theme.sp(5.))
                    .child(heading(setting_title("theme")))
                    .child(Button::new("theme-add").variant(ButtonVariant::Outline).size(ButtonSize::Xs).icon(IconName::Plus).label(t!("Add theme")).on_click(open)),
            )
            .child(div().flex().flex_col().gap(theme.sp(2.)).children(rows))
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
            .control(details_menu);
        if !THREAD_DETAILS_ITEMS.iter().all(|(id, _, default)| thread_detail_visible(&settings, id, *default) == *default) {
            details_row = details_row.reset(reset_button("thread-details-items", |_, cx| {
                ClientSettingsStore::update(cx, |s| {
                    s.thread_details_items.clear();
                    s.thread_details_usage_enabled = false;
                })
            }));
        }
        let animation = div()
            .flex()
            .items_center()
            .gap(theme.sp(3.))
            .child(div().w(px(160.)).child(Slider::new(&self.animation)))
            .child(div().w(px(48.)).ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.muted_foreground).child(format!("{} ms", settings.panel_animation_duration_ms)));
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
                client_select(
                    "environment-identification",
                    t!("Choose how Dev and Nightly environments are identified."),
                    vec![
                        (EnvironmentIdentificationMode::Artwork, t!("Artwork")),
                        (EnvironmentIdentificationMode::NightlyArtwork, t!("Nightly artwork")),
                        (EnvironmentIdentificationMode::DevArtwork, t!("Dev artwork")),
                        (EnvironmentIdentificationMode::Pill, t!("Version pill")),
                        (EnvironmentIdentificationMode::NoneType, t!("None")),
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
        SettingsSection::new(t!("Interface"))
            .child(client_select(
                "chat-width",
                t!("Set how wide messages and the composer can grow on large screens."),
                vec![
                    (ChatWidth::Comfortable, t!("Comfortable (default)")),
                    (ChatWidth::Wide, t!("Wide")),
                    (ChatWidth::Full, t!("Full", context = "chat width")),
                ],
                |s| &s.chat_width,
                |s, v| s.chat_width = v,
                cx,
            ))
            .child(details_row)
            .child(client_select(
                "effort-display",
                t!("Show reasoning effort in the composer as its name, such as High, or as bars."),
                vec![
                    (EffortDisplay::Label, t!("Word (default)")),
                    (EffortDisplay::Bars, t!("Bars", context = "show effort as bars")),
                ],
                |s| &s.effort_display,
                |s, v| s.effort_display = v,
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

    fn font_row(&self, role: FontRole, id: &'static str, title: Option<SharedString>, description: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let settings = ClientSettingsStore::get(cx).clone();
        let defaults = ClientSettings::defaults();
        let theme = cx.app_theme();
        let input = self.fonts[FontRole::ALL.iter().position(|r| *r == role).expect("role")].clone();
        let size = role.size(&settings);
        let (min, max) = role.range();
        let stepper = |delta: i64, icon: IconName, enabled: bool| {
            Button::new(SharedString::from(format!("{id}-size-{}", if delta < 0 { "down" } else { "up" })))
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::IconSm)
                .icon(icon)
                .disabled(!enabled)
                .on_click(move |_, _, cx| {
                    ClientSettingsStore::update(cx, |s| {
                        let next = (role.size(s) as i64 + delta).clamp(min as i64, max as i64) as u64;
                        role.set_size(s, next);
                    })
                })
        };
        let control = div()
            .flex()
            .items_center()
            .gap(theme.sp(2.))
            .child(div().w(px(176.)).child(TextInput::new(&input)))
            .child(stepper(-1, IconName::Minus, size > min))
            .child(div().w(px(40.)).flex().justify_center().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(format!("{size}px")))
            .child(stepper(1, IconName::Plus, size < max));
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
        let separate = gpui_kit::SharedString::from("switch-separate-fonts");
        section
            .child(client_switch(
                "word-wrap",
                t!("Wrap long lines in code blocks, diffs, and file previews by default."),
                |s| &s.word_wrap,
                |s, v| s.word_wrap = v,
                cx,
            ))
            .child(
                SettingsRow::new(setting_title("separate-fonts"))
                    .description(t!("Set the prompt, code, and terminal fonts separately instead of one monospace font."))
                    .control(crate::components::ui::switch::Switch::new(separate, advanced).on_change(|on, _, cx| set_separate_fonts(on, cx))),
            )
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = self.render_library(cx);
        let interface = self.render_interface(cx);
        let typography = self.render_typography(cx);
        let import = self.render_import_dialog(cx);
        let remove = self.render_remove_dialog(cx);
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

/// `ThemePreviewCircle`: the canvas mixed toward black or white with the
/// accent and action as soft spots. The web blurs radial gradients; GPUI
/// draws the spots as plain circles.
fn preview_circle(palette: &ThemePalette, mode: Appearance, ring: Hsla) -> AnyElement {
    let base = mix_oklab(palette.canvas, if mode.is_dark() { 0.85 } else { 0.9 }, if mode.is_dark() { gpui_kit::black() } else { gpui_kit::white() });
    div()
        .relative()
        .size(px(56.))
        .overflow_hidden()
        .rounded_full()
        .border_2()
        .border_color(ring)
        .bg(base)
        .child(div().absolute().left(px(14.)).top(px(8.)).size(px(30.)).rounded_full().bg(with_alpha(palette.accent, 0.85)))
        .child(div().absolute().left(px(30.)).top(px(30.)).size(px(16.)).rounded_full().bg(with_alpha(palette.message_action, 0.7)))
        .into_any_element()
}

/// `ThemeWireframe`: a sidebar, a message bubble and a composer, drawn
/// with a palette.
fn wireframe(palette: &ThemePalette, round_left: bool) -> AnyElement {
    let bar = |w: f32, color: Hsla| div().h(px(4.)).w(px(w)).rounded_full().bg(color);
    div()
        .h(px(140.))
        .flex()
        .overflow_hidden()
        .when(round_left, |d| d.rounded(px(8.)))
        .border_1()
        .border_color(palette.border)
        .bg(palette.canvas)
        .child(
            div()
                .w(px(44.))
                .h_full()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(8.))
                .bg(palette.sidebar)
                .border_r_1()
                .border_color(palette.sidebar_border)
                .child(bar(24., palette.sidebar_foreground))
                .child(bar(28., palette.sidebar_row_selected))
                .child(bar(20., palette.sidebar_muted_foreground))
                .child(bar(26., palette.sidebar_muted_foreground)),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .justify_between()
                .p(px(10.))
                .child(div().flex().justify_end().child(div().w(px(64.)).h(px(18.)).rounded(px(6.)).bg(palette.message_surface)))
                .child(div().flex().flex_col().gap(px(5.)).child(bar(70., palette.text)).child(bar(52., palette.text_muted)))
                .child(
                    div()
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .px(px(5.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(palette.border)
                        .bg(palette.surface)
                        .child(div().size(px(14.)).rounded_full().bg(palette.message_action)),
                ),
        )
        .into_any_element()
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
