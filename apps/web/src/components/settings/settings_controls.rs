//! The pieces every settings panel shares: the per-setting reset button
//! (`SettingResetButton`), switch and select rows bound to a client
//! setting, the scope picker for settings saved on a server
//! (`SettingsScopeSentence`), and "Restore device defaults"
//! (`useSettingsRestore`).

use std::rc::Rc;

use gpui_kit::{AnyElement, App, Global, IntoElement, SharedString, Window, div, prelude::*};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::{ClientServerSettingsPatch, ServerSettings};

use super::settings_layout::SettingsRow;
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{IconName};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::components::ui::select::Select;
use crate::components::ui::switch::Switch;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::{ClientSettings, ClientSettingsStore, ServerSettingsStore};
use crate::hooks::use_theme::{ThemeStore, separate_fonts, set_separate_fonts};
use crate::i18n::t;
use crate::theme_palette::ThemeMode;

/// `SettingResetButton`: a micro ghost button with a "Reset to default" tooltip.
pub fn reset_button(id: impl Into<SharedString>, on_click: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let id: SharedString = id.into();
    div()
        .id(SharedString::from(format!("reset-tip-{id}")))
        .tooltip(tooltip(t!("Reset to default")))
        .child(
            Button::new(SharedString::from(format!("reset-{id}")))
                .variant(ButtonVariant::GhostMuted)
                .size(ButtonSize::IconMicro)
                .icon(IconName::Undo2)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on_click(window, cx);
                }),
        )
        .into_any_element()
}

/// A row for a client setting: the catalog title, the description, a reset
/// button while the value differs from its default, and the control.
pub fn client_row<T: PartialEq + Clone + 'static>(
    id: &'static str,
    description: impl Into<SharedString>,
    get: fn(&ClientSettings) -> &T,
    set: fn(&mut ClientSettings, T),
    control: impl IntoElement,
    cx: &App,
) -> SettingsRow {
    let current = get(ClientSettingsStore::get(cx));
    let default = get(ClientSettings::defaults());
    let mut row = SettingsRow::new(setting_title(id)).description(description).control(control);
    if current != default {
        let default = default.clone();
        row = row.reset(reset_button(id, move |_, cx| {
            let default = default.clone();
            ClientSettingsStore::update(cx, |settings| set(settings, default));
        }));
    }
    row
}

/// A switch row bound to a boolean client setting.
pub fn client_switch(
    id: &'static str,
    description: impl Into<SharedString>,
    get: fn(&ClientSettings) -> &bool,
    set: fn(&mut ClientSettings, bool),
    cx: &App,
) -> SettingsRow {
    let checked = *get(ClientSettingsStore::get(cx));
    let switch = Switch::new(SharedString::from(format!("switch-{id}")), checked)
        .on_change(move |checked, _, cx| ClientSettingsStore::update(cx, |settings| set(settings, checked)));
    client_row(id, description, get, set, switch, cx)
}

/// How wide a settings select is, from its trigger's `sm:w-*` class.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelectWidth {
    /// A fixed width in rem: `sm:w-40` is `Rem(10.)`.
    Rem(f32),
    /// `w-auto min-w-0`: as wide as the chosen value, but never under the
    /// 10rem every select in a settings row gets (`[&_select-trigger]:min-w-40`).
    Fit,
}

/// A select row bound to a client setting with fixed options.
pub fn client_select<T: PartialEq + Clone + 'static>(
    id: &'static str,
    description: impl Into<SharedString>,
    width: SelectWidth,
    options: Vec<(T, SharedString)>,
    get: fn(&ClientSettings) -> &T,
    set: fn(&mut ClientSettings, T),
    cx: &App,
) -> SettingsRow {
    let current = get(ClientSettingsStore::get(cx));
    let selected = options.iter().position(|(value, _)| value == current).unwrap_or(0);
    let labels: Rc<[SharedString]> = options.iter().map(|(_, label)| label.clone()).collect();
    let values: Rc<[T]> = options.into_iter().map(|(value, _)| value).collect();
    let select = select_control(SharedString::from(format!("select-{id}")), width, labels, selected, move |index, _, cx| {
        let value = values[index].clone();
        ClientSettingsStore::update(cx, |settings| set(settings, value));
    });
    client_row(id, description, get, set, select, cx)
}

/// A small select (`size="sm"`) at its row's width.
pub fn select_control(
    id: SharedString,
    width: SelectWidth,
    labels: Rc<[SharedString]>,
    selected: usize,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    sized_select(width, Select::new(id, labels, selected, on_change).size(crate::components::ui::select::SelectSize::Sm))
}

/// Places a settings select at its width.
pub fn sized_select(width: SelectWidth, select: Select) -> impl IntoElement {
    match width {
        SelectWidth::Rem(rem) => div().w(gpui_kit::rems(rem)).child(select),
        SelectWidth::Fit => div().min_w(gpui_kit::rems(10.)).child(select),
    }
}

/// The environments a server-saved setting applies to: one, or every
/// connected one (`All environments`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsScope {
    pub environment: Option<EnvironmentId>,
}

impl Global for SettingsScope {}

impl SettingsScope {
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }

    pub fn set(scope: Self, cx: &mut App) {
        cx.set_global(scope);
        cx.refresh_windows();
    }

    /// The environments the scope writes to, with a config.
    pub fn targets(cx: &App) -> Vec<EnvironmentId> {
        let known = ServerSettingsStore::environment_ids(cx);
        match Self::get(cx).environment {
            Some(id) => known.into_iter().filter(|known| *known == id).collect(),
            None => {
                let mut ids = known;
                ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                ids
            }
        }
    }

    /// The value the scope shows: the selected environment's, or the first
    /// target's under All environments.
    pub fn settings(cx: &App) -> Option<ServerSettings> {
        Self::targets(cx).first().and_then(|id| ServerSettingsStore::resolved(cx, id))
    }

    /// Writes a patch to every target. Failures show a toast.
    pub fn update(patch: ClientServerSettingsPatch, cx: &mut App) {
        let targets = Self::targets(cx);
        if targets.is_empty() {
            Toaster::push(ToastData::new(ToastKind::Warning, t!("Setting not saved")).description(t!("Update older servers to save this setting.")), cx);
            return;
        }
        for id in targets {
            let save = ServerSettingsStore::update(id.clone(), patch.clone(), cx);
            cx.spawn(async move |cx| {
                if let Err(error) = save.await {
                    cx.update(|cx| {
                        Toaster::push(
                            ToastData::new(ToastKind::Error, t!("Setting not saved")).description(t!(
                                "Could not save on {label}: {error}",
                                label = environment_label(&id, cx),
                                error = error.0.to_string()
                            )),
                            cx,
                        );
                    });
                }
            })
            .detach();
        }
    }
}

/// An environment's display label.
pub fn environment_label(id: &EnvironmentId, cx: &App) -> String {
    if ConnectionRuntime::is_running(cx)
        && let Some(env) = ConnectionRuntime::state(cx).read(cx).environment(id)
    {
        return env.entry.label.clone();
    }
    ServerSettingsStore::config(cx, id)
        .map(|config| config.environment.label.clone())
        .unwrap_or_else(|| id.as_str().to_owned())
}

/// A switch row bound to a boolean server setting in the current scope.
/// Disabled with an explanation when no environment is connected.
pub fn server_switch(
    id: &'static str,
    description: impl Into<SharedString>,
    get: fn(&ServerSettings) -> Option<bool>,
    patch: fn(bool) -> ClientServerSettingsPatch,
    cx: &App,
) -> SettingsRow {
    let settings = SettingsScope::settings(cx);
    let checked = settings.as_ref().and_then(get).unwrap_or(false);
    let available = settings.is_some();
    let switch = Switch::new(SharedString::from(format!("switch-{id}")), checked)
        .disabled(!available)
        .on_change(move |checked, _, cx| SettingsScope::update(patch(checked), cx));
    let row = SettingsRow::new(setting_title(id)).description(description).control(switch);
    if !available {
        return row.status(t!("Reconnect the selected environment to change this setting."));
    }
    let values: Vec<Option<bool>> =
        SettingsScope::targets(cx).iter().map(|target| ServerSettingsStore::resolved(cx, target).as_ref().and_then(get)).collect();
    let default = get(ServerSettingsStore::defaults());
    let (summary, quiet) = if values.windows(2).any(|pair| pair[0] != pair[1]) {
        (t!("Mixed across selected environments"), false)
    } else if values.first().copied().flatten() != default {
        (t!("Set on the environment"), true)
    } else {
        (t!("Built-in default"), true)
    };
    row.inheritance(inheritance_marker(id, summary), quiet)
}

/// `SettingInheritance`'s trigger: a micro layers button whose tooltip says
/// where the value comes from.
pub fn inheritance_marker(id: &'static str, summary: SharedString) -> AnyElement {
    div()
        .id(SharedString::from(format!("inheritance-tip-{id}")))
        .tooltip(tooltip(summary))
        .child(
            Button::new(SharedString::from(format!("inheritance-{id}")))
                .variant(ButtonVariant::GhostMuted)
                .size(ButtonSize::IconMicro)
                .icon(IconName::Layers),
        )
        .into_any_element()
}

/// `SettingsScopeSentence` for this app: "Applying settings for All
/// projects across All environments" (or "on" one environment). Projects
/// come from the shell, which these pages do not read, so the project
/// menu offers All projects only.
pub fn scope_sentence(cx: &App) -> AnyElement {
    let theme = cx.app_theme();
    let scope = SettingsScope::get(cx);
    let mut ids = ServerSettingsStore::environment_ids(cx);
    ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let environment_name = match &scope.environment {
        Some(id) => SharedString::from(environment_label(id, cx)),
        None if ids.is_empty() => t!("No environments"),
        None => t!("All environments"),
    };
    // Triggers carry an icon only for a picked project or environment.
    let menu_button = |id: &'static str, icon: Option<IconName>, label: SharedString| {
        let button = Button::new(id).variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(label).trailing_icon(IconName::ChevronDown);
        match icon {
            Some(icon) => button.icon(icon),
            None => button,
        }
    };
    let project = dropdown_menu("scope-project", menu_button("scope-project-trigger", None, t!("All projects")), |_, _| {
        MenuPopup::new().item(MenuItem::new(t!("All projects")).kind(MenuItemKind::Radio(true)).on_select(|_, _| {}))
    });
    let environment_icon = scope.environment.as_ref().map(|_| IconName::Server);
    let environments = ids.clone();
    let selected = scope.environment.clone();
    let environment = dropdown_menu("scope-environment", menu_button("scope-environment-trigger", environment_icon, environment_name), move |_, cx| {
        let mut menu = MenuPopup::new().item(
            MenuItem::new(t!("All environments"))
                .kind(MenuItemKind::Radio(selected.is_none()))
                .on_select(|_, cx| SettingsScope::set(SettingsScope { environment: None }, cx)),
        );
        if !environments.is_empty() {
            menu = menu.separator();
        }
        for id in &environments {
            let picked = id.clone();
            menu = menu.item(
                MenuItem::new(environment_label(id, cx))
                    .kind(MenuItemKind::Radio(selected.as_ref() == Some(id)))
                    .on_select(move |_, cx| SettingsScope::set(SettingsScope { environment: Some(picked.clone()) }, cx)),
            );
        }
        menu
    });
    // The web translates this sentence as one rich `<T>` block whose menus
    // are variables; its words stay English here.
    let word = if scope.environment.is_some() { "on" } else { "across" };
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(theme.sp(1.5))
        .gap_y(theme.sp(1.))
        .px(theme.sp(4.))
        .ui_text(theme, TextSize::Sm, weight::NORMAL)
        .text_color(theme.colors.muted_foreground)
        .child("Applying settings for")
        .child(project)
        .child(word)
        .child(environment)
        .into_any_element()
}

/// `useSettingsRestore` for this device: the labels of client and theme
/// settings that differ from their defaults.
pub fn changed_device_settings(cx: &App) -> Vec<SharedString> {
    let settings = ClientSettingsStore::get(cx);
    let defaults = ClientSettings::defaults();
    let theme = ThemeStore::global(cx);
    let mut labels = Vec::new();
    let mut check = |changed: bool, label: SharedString| {
        if changed {
            labels.push(label);
        }
    };
    check(theme.preference().as_ref() != crate::hooks::use_theme::DEFAULT_THEME_ID, t!("Theme"));
    check(theme.appearance_mode() != ThemeMode::System, t!("Follow system"));
    check(!theme.halves().is_empty(), t!("Theme mix"));
    macro_rules! field {
        ($($field:ident => $label:literal),* $(,)?) => {
            $(check(settings.$field != defaults.$field, t!($label));)*
        };
    }
    field! {
        appearance_contrast => "Contrast",
        glass_opacity => "Glass opacity",
        diff_color_scheme => "Diff colors",
        chat_width => "Chat width",
        effort_display => "Effort display",
        math_rendering_enabled => "Render math",
        panel_animation_duration_ms => "Panel animations",
        environment_identification_mode => "Environment identification",
        language => "Language",
        timestamp_format => "Time format",
        notification_mode => "Thread notifications",
        in_app_notifications_enabled => "In-app notifications",
        notification_inbox_enabled => "Notification inbox",
        app_icon_badge_enabled => "App icon badge",
        sidebar_project_grouping_mode => "Project Grouping",
        sidebar_project_sort_order => "Project order",
        sidebar_working_shelf_enabled => "Working section",
        confirm_thread_unpin => "Unpin confirmation",
        confirm_thread_archive => "Archive confirmation",
        confirm_terminal_close => "Terminal close confirmation",
        confirm_thread_delete => "Delete confirmation",
        confirm_quit => "Quit shortcut",
    }
    labels
}

/// Resets every device setting `useSettingsRestore` resets, and the theme.
pub fn restore_device_defaults(cx: &mut App) {
    if ThemeStore::global(cx).preference().as_ref() != crate::hooks::use_theme::DEFAULT_THEME_ID {
        ThemeStore::set_theme(crate::hooks::use_theme::DEFAULT_THEME_ID, cx);
    }
    for appearance in [crate::theme_palette::Appearance::Light, crate::theme_palette::Appearance::Dark] {
        if ThemeStore::global(cx).halves().get(appearance).is_some() {
            ThemeStore::set_half(appearance, None, cx);
        }
    }
    if ThemeStore::global(cx).appearance_mode() != ThemeMode::System {
        ThemeStore::set_appearance_mode(ThemeMode::System, cx);
    }
    if separate_fonts(cx) {
        set_separate_fonts(false, cx);
    }
    let defaults = ClientSettings::defaults();
    ClientSettingsStore::update(cx, |s| {
        macro_rules! reset {
            ($($field:ident),* $(,)?) => { $(s.$field = defaults.$field.clone();)* };
        }
        reset!(
            appearance_contrast, diff_color_scheme, chat_width, effort_display, math_rendering_enabled, language,
            timestamp_format, notification_mode, in_app_notifications_enabled, notification_inbox_enabled,
            app_icon_badge_enabled, word_wrap, persist_composer_context_strip, diff_files_collapsed,
            diff_ignore_whitespace, diff_layout, proactive_panels_enabled, show_skills_in_slash_menu,
            composer_rich_text_enabled, send_shortcut, follow_up_behavior, tool_call_visibility,
            screen_reader_announcements, screen_reader_window_title, environment_identification_mode, glass_opacity,
            panel_animation_duration_ms, sidebar_project_grouping_mode, sidebar_project_sort_order,
            sidebar_working_shelf_enabled, confirm_thread_archive, confirm_terminal_close, confirm_thread_delete,
            confirm_thread_unpin, confirm_quit, font_family_sans, font_family_composer, font_family_code,
            font_family_terminal, font_size_interface, font_size_prompt, font_size_code, font_size_terminal,
            browser_default_viewport, browser_default_zoom_factor, browser_default_appearance,
            browser_recording_frame_rate, browser_recording_show_key_presses, browser_recording_show_mouse_presses,
            browser_link_target, browser_search_engine, browser_custom_search_url, browser_auto_show_floating_preview,
            pull_request_links_open_in_app,
        );
    });
}

/// "Restore device defaults" in the settings header, disabled when
/// nothing differs. Asks first, naming what will reset.
pub fn restore_defaults_button(on_confirm: impl Fn(Vec<SharedString>, &mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
    let changed = changed_device_settings(cx);
    Button::new("settings-restore-defaults")
        .variant(ButtonVariant::Ghost)
        .size(ButtonSize::Xs)
        .icon(IconName::RotateCcw)
        .label(t!("Restore device defaults"))
        .disabled(changed.is_empty())
        .on_click(move |_, window, cx| on_confirm(changed.clone(), window, cx))
        .into_any_element()
}

/// Where a link inside a settings description goes.
#[derive(Clone, Copy, Debug)]
pub enum LinkTarget {
    Page(super::settings_search::SettingsPath),
    Url(&'static str),
}

/// A description from a `<T>` block whose `<Link>` and `<a>` elements are
/// underlined links (`underline underline-offset-2`), in order of
/// `targets`.
pub fn linked_description(id: &'static str, text: crate::i18n::RichText, targets: Vec<LinkTarget>) -> AnyElement {
    let mut plain = String::new();
    let mut ranges = Vec::new();
    for segment in &text.segments {
        let start = plain.len();
        plain.push_str(&segment.text);
        if matches!(segment.tag.as_deref(), Some("Link" | "a")) && start < plain.len() {
            ranges.push(start..plain.len());
        }
    }
    let underline = gpui_kit::HighlightStyle {
        underline: Some(gpui_kit::UnderlineStyle { thickness: gpui_kit::px(1.), color: None, wavy: false }),
        ..Default::default()
    };
    let styled = gpui_kit::StyledText::new(plain).with_highlights(ranges.iter().map(|range| (range.clone(), underline)));
    gpui_kit::InteractiveText::new(id, styled)
        .on_click(ranges, move |index, _, cx| match targets.get(index) {
            Some(LinkTarget::Page(path)) => super::settings_layout::SettingsNavigator::open(*path, cx),
            Some(LinkTarget::Url(url)) => cx.open_url(url),
            None => {}
        })
        .into_any_element()
}
