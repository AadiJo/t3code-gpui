//! Settings → Browser and Devices: `BrowserSettingsPanel` and
//! `DevicesSettingsPanel` of `IntegrationsSettings.tsx`, with the device
//! sources of `DeviceSourcesSettings.tsx`. Browser rows other than agent
//! access are this device's client settings.

use gpui_kit::{AnyElement, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use t3code_contracts::{
    BrowserLinkTarget, BrowserProfile, BrowserProfileKind, BrowserSearchEngine, ClientServerSettingsPatch, PreviewAppearancePreference,
};

use super::device_host_editor::{DeviceHostEditor, DeviceHostEditorEvent};
use super::server_settings_controls::{Fields, advanced_fold, bool_setting, field_input, lock_reason};
use super::settings_controls::{client_select, client_switch, SelectWidth};
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::badge::Badge;
use crate::components::ui::menu::{MenuItem, MenuPopup, dropdown_menu};
use crate::hooks::use_settings::ClientSettingsStore;
use crate::theme_color::with_alpha;
use crate::timestamp_format::now_ms;
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::components::managed_policy::managed_by;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::i18n::t;

/// The Browser page: agent access (server), then the device's browsing
/// and recording settings.
pub struct BrowserSettingsPanel {
    model: Entity<ServerPanelsModel>,
    fields: Fields,
    advanced_open: bool,
    _observe: Subscription,
}

impl BrowserSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, fields: Fields::default(), advanced_open: false, _observe: observe }
    }

    /// `BrowserProfilesSetting`: the profile list with "Add profile" and a menu per profile.
    fn profiles_row(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingsRow {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let settings = ClientSettingsStore::get(cx).clone();
        let mut profiles = vec![BrowserProfile { id: "default".into(), name: "Default".into(), kind: BrowserProfileKind::Persistent }];
        profiles.extend(settings.browser_profiles.iter().filter(|p| p.id != "default" && p.id != "incognito").cloned());
        let user_count = profiles.len() - 1;
        let default_id = if profiles.iter().any(|p| p.id == settings.browser_default_profile_id) { settings.browser_default_profile_id.clone() } else { "default".into() };
        let add = dropdown_menu(
            "browser-profile-add",
            Button::new("browser-profile-add-trigger").variant(ButtonVariant::Outline).size(ButtonSize::Sm).icon(IconName::Plus).label(t!("Add profile")),
            move |_, _| {
                let mut menu = MenuPopup::new().item(MenuItem::new(t!("Blank profile")).disabled(user_count >= 24).on_select(|_, cx| {
                    ClientSettingsStore::update(cx, |s| {
                        let n = s.browser_profiles.len();
                        s.browser_profiles.push(BrowserProfile { id: format!("profile-{:x}", now_ms() as u64 + n as u64), name: t!("New profile").to_string(), kind: BrowserProfileKind::Persistent });
                    })
                }));
                if user_count >= 24 {
                    menu = menu.item(MenuItem::new(t!("You’ve reached the profile limit")).disabled(true));
                }
                menu
            },
        );
        let mut list = div().mt(theme.sp(2.)).mb(theme.sp(2.)).overflow_hidden().rounded(theme.radii.lg).border_1().border_color(with_alpha(c.border, 0.6));
        for (ix, profile) in profiles.iter().enumerate() {
            let built_in = profile.id == "default";
            let is_default = profile.id == default_id;
            let id = profile.id.clone();
            let name: AnyElement = if built_in {
                div().truncate().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.foreground()).child(profile.name.clone()).into_any_element()
            } else {
                let rename_id = id.clone();
                let field = self.fields.draft(format!("profile:{id}"), profile.name.clone(), "", false, window, cx, move |text, _, cx| {
                    let name = text.trim().to_owned();
                    if name.is_empty() {
                        return;
                    }
                    let rename_id = rename_id.clone();
                    ClientSettingsStore::update(cx, move |s| {
                        if let Some(p) = s.browser_profiles.iter_mut().find(|p| p.id == rename_id) {
                            p.name = name;
                        }
                    });
                });
                div().w_full().max_w(theme.sp(56.)).child(field_input(&field, false, false, false)).into_any_element()
            };
            let (default_target, remove_target) = (id.clone(), id.clone());
            let menu = dropdown_menu(
                SharedString::from(format!("browser-profile-menu-{id}")),
                Button::new(SharedString::from(format!("browser-profile-menu-trigger-{id}"))).variant(ButtonVariant::GhostMuted).size(ButtonSize::IconXs).icon(IconName::EllipsisVertical),
                move |_, _| {
                    let (default_target, remove_target) = (default_target.clone(), remove_target.clone());
                    MenuPopup::new()
                        .item(MenuItem::new(t!("Set as default")).disabled(is_default).on_select(move |_, cx| {
                            let target = default_target.clone();
                            ClientSettingsStore::update(cx, move |s| s.browser_default_profile_id = target);
                        }))
                        .item(MenuItem::new(t!("Remove profile and data")).disabled(built_in).on_select(move |_, cx| {
                            let target = remove_target.clone();
                            ClientSettingsStore::update(cx, move |s| {
                                s.browser_profiles.retain(|p| p.id != target);
                                if s.browser_default_profile_id == target {
                                    s.browser_default_profile_id = "default".into();
                                }
                            });
                        }))
                },
            );
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(3.))
                    .px(theme.sp(3.))
                    .py(theme.sp(2.))
                    .when(ix > 0, |d| d.border_t_1().border_color(with_alpha(c.border, 0.6)))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(theme.sp(2.))
                            .child(name)
                            .when(is_default, |d| d.child(Badge::new(t!("Default", context = "default browser profile badge")))),
                    )
                    .child(menu),
            );
        }
        SettingsRow::new(setting_title("browser-profiles"))
            .description(t!("Profiles separate cookies and logins. Incognito data is cleared when the app closes."))
            .control(add)
            .child(list)
    }
}

impl Render for BrowserSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.model.read(cx).snapshot().clone();
        let lock = lock_reason(managed_by(snapshot.managed_policy.as_deref(), &["enableAgentBrowserAccess"]), snapshot.read_only);
        let agent = SettingsSection::new(t!("Agent access")).child(bool_setting(
            &self.model,
            "enableAgentBrowserAccess",
            t!("Agent browser access"),
            Some(t!("Allow agents to use the shared browser. Projects can override it.")),
            snapshot.settings.enable_agent_browser_access.unwrap_or(true),
            lock,
            |v| ClientServerSettingsPatch { enable_agent_browser_access: Some(v), ..Default::default() },
        ));
        let engines = [
            (BrowserSearchEngine::Duckduckgo, SharedString::from("DuckDuckGo")),
            (BrowserSearchEngine::Google, "Google".into()),
            (BrowserSearchEngine::Bing, "Bing".into()),
            (BrowserSearchEngine::Brave, "Brave Search".into()),
            (BrowserSearchEngine::Kagi, "Kagi".into()),
            (BrowserSearchEngine::Ecosia, "Ecosia".into()),
            (BrowserSearchEngine::Custom, t!("Custom", context = "custom search engine")),
        ];
        let zoom_levels = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];
        let this = cx.entity().downgrade();
        let advanced = advanced_fold(
            "browser-advanced",
            self.advanced_open,
            move |_, cx| {
                this.update(cx, |this, cx| {
                    this.advanced_open = !this.advanced_open;
                    cx.notify();
                })
                .ok();
            },
            vec![
                client_select(
                    "browser-default-zoom",
                    t!("Page zoom applied to new browser tabs."), SelectWidth::Rem(10.),
                    zoom_levels.iter().map(|z| (*z, SharedString::from(format!("{}%", (z * 100.0f64).round())))).collect(),
                    |s| &s.browser_default_zoom_factor,
                    |s, v| s.browser_default_zoom_factor = v,
                    cx,
                )
                .into_any_element(),
                client_select(
                    "browser-default-appearance",
                    t!("The color scheme pages are told to prefer. System follows your OS setting."), SelectWidth::Rem(10.),
                    vec![
                        (PreviewAppearancePreference::System, t!("System", context = "appearance follows the system")),
                        (PreviewAppearancePreference::Light, t!("Light", context = "appearance")),
                        (PreviewAppearancePreference::Dark, t!("Dark", context = "appearance")),
                    ],
                    |s| &s.browser_default_appearance,
                    |s, v| s.browser_default_appearance = v,
                    cx,
                )
                .into_any_element(),
            ],
            cx,
        );
        let browsing = SettingsSection::new(t!("Browsing"))
            .child(self.profiles_row(window, cx))
            .child(client_select(
                "browser-link-target",
                t!("Where links in the chat and terminal open. Hold ⌘ or Ctrl while clicking a link to open it in your default browser either way."), SelectWidth::Rem(10.),
                vec![(BrowserLinkTarget::System, t!("Your default browser")), (BrowserLinkTarget::App, "T3 Code".into())],
                |s| &s.browser_link_target,
                |s, v| s.browser_link_target = v,
                cx,
            ))
            .child(client_switch(
                "browser-auto-show-floating-preview",
                t!("Show the floating preview when an agent opens a browser or device unless the agent says otherwise."),
                |s| &s.browser_auto_show_floating_preview,
                |s, v| s.browser_auto_show_floating_preview = v,
                cx,
            ))
            .child(client_select("browser-search-engine", t!("What the address bar searches when you type something that isn't a URL."), SelectWidth::Rem(10.), engines.to_vec(), |s| &s.browser_search_engine, |s, v| s.browser_search_engine = v, cx))
            .child(advanced);
        let recordings = SettingsSection::new(t!("Recordings"))
            .child(client_select(
                "browser-recording-frame-rate",
                t!("Maximum recording rate. 30 fps saves CPU and storage; 60 fps is smoother."), SelectWidth::Rem(10.),
                vec![(30., t!("30 fps")), (60., t!("60 fps"))],
                |s| &s.browser_recording_frame_rate,
                |s, v| s.browser_recording_frame_rate = v,
                cx,
            ))
            .child(client_switch(
                "browser-recording-key-presses",
                t!("Show pressed keys and shortcuts in new recordings. Password fields are excluded."),
                |s| &s.browser_recording_show_key_presses,
                |s, v| s.browser_recording_show_key_presses = v,
                cx,
            ))
            .child(client_switch(
                "browser-recording-mouse-presses",
                t!("Highlight mouse presses and held buttons in new recordings."),
                |s| &s.browser_recording_show_mouse_presses,
                |s, v| s.browser_recording_show_mouse_presses = v,
                cx,
            ));
        page_container(cx).child(agent).child(browsing).child(recordings)
    }
}

/// The Devices page.
pub struct DevicesSettingsPanel {
    model: Entity<ServerPanelsModel>,
    editor: Option<Entity<DeviceHostEditor>>,
    _subscriptions: Vec<Subscription>,
}

impl DevicesSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, editor: None, _subscriptions: vec![observe] }
    }

    /// Opens the host editor, for a new host or the host at `index`.
    pub fn open_editor(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let editor = cx.new(|cx| DeviceHostEditor::new(model, index, window, cx));
        let sub = cx.subscribe(&editor, |this, _, _: &DeviceHostEditorEvent, cx| {
            this.editor = None;
            cx.notify();
        });
        self._subscriptions.push(sub);
        self.editor = Some(editor);
        cx.notify();
    }

    pub fn editor(&self) -> Option<&Entity<DeviceHostEditor>> {
        self.editor.as_ref()
    }
}

impl Render for DevicesSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.model.read(cx).snapshot().clone();
        let s = &snapshot.settings;
        let policy = snapshot.managed_policy.as_deref();
        let ro = snapshot.read_only;
        let lock = |paths: &[&str]| lock_reason(managed_by(policy, paths), ro);
        let hosts = s.device_hosts.clone().unwrap_or_default();
        let this = cx.entity().downgrade();
        let add = dropdown_menu(
            "device-source-add",
            Button::new("device-source-add-trigger").variant(ButtonVariant::Outline).size(ButtonSize::Sm).icon(IconName::Plus).label(t!("Add source")).trailing_icon(IconName::ChevronDown).disabled(ro || self.editor.is_some()),
            move |_, _| {
                let this = this.clone();
                MenuPopup::new().item(MenuItem::new(t!("Machine over SSH")).on_select(move |window, cx| {
                    this.update(cx, |this, cx| this.open_editor(None, window, cx)).ok();
                }))
            },
        );
        let mut sources = SettingsSection::new(t!("Device sources")).header_action(add).child(
            SettingsRow::new(t!("Local simulators")).description(t!("Simulators and emulators installed on this environment's machine.")),
        );
        for (ix, host) in hosts.iter().enumerate() {
            let target = match host.port {
                Some(port) => format!("{} · {port}", host.target),
                None => host.target.clone(),
            };
            let remove_model = self.model.clone();
            let remaining: Vec<_> = hosts.iter().enumerate().filter(|(i, _)| *i != ix).map(|(_, h)| h.clone()).collect();
            let more = dropdown_menu(
                SharedString::from(format!("device-host-more-{ix}")),
                Button::new(SharedString::from(format!("device-host-more-trigger-{ix}"))).variant(ButtonVariant::GhostMuted).size(ButtonSize::IconXs).icon(IconName::Ellipsis),
                move |_, _| {
                    let (model, remaining) = (remove_model.clone(), remaining.clone());
                    MenuPopup::new().item(MenuItem::new(t!("Remove", context = "verb, removes a device source")).destructive().on_select(move |_, cx| {
                        let patch = ClientServerSettingsPatch { device_hosts: Some(remaining.clone()), ..Default::default() };
                        super::server_settings_controls::save_patch(&model, "deviceHosts", patch, cx);
                    }))
                },
            );
            sources.extend([SettingsRow::new(SharedString::from(host.label.clone()))
                .description(SharedString::from(target))
                .control(
                    div()
                        .flex()
                        .items_center()
                        .gap(gpui_kit::px(4.))
                        .child(
                            Button::new(SharedString::from(format!("device-host-edit-{ix}")))
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Xs)
                                .label(t!("Edit"))
                                .disabled(ro)
                                .on_click(cx.listener(move |this, _, window, cx| this.open_editor(Some(ix), window, cx))),
                        )
                        .child(more),
                )
                .into_any_element()]);
        }
        let usage = SettingsSection::new(t!("Devices"))
            .child(bool_setting(&self.model, "enableDeviceSupport", t!("Use devices"), Some(t!("Open simulators and emulators in the Device panel.")), s.enable_device_support.unwrap_or(false), lock(&["enableDeviceSupport"]), |v| ClientServerSettingsPatch { enable_device_support: Some(v), ..Default::default() }))
            .child(bool_setting(&self.model, "enableAgentDeviceAccess", t!("Agents can use devices"), Some(t!("New agent sessions can open and drive these devices.")), s.enable_agent_device_access.unwrap_or(false), lock(&["enableAgentDeviceAccess"]), |v| ClientServerSettingsPatch { enable_agent_device_access: Some(v), ..Default::default() }));
        let editor = self.editor.clone().map(|editor| {
            let this = cx.entity().downgrade();
            Modal::dialog("device-host-editor", editor, move |_, cx| {
                this.update(cx, |this, cx| {
                    this.editor = None;
                    cx.notify();
                })
                .ok();
            })
        });
        gpui_kit::div().relative().size_full().child(page_container(cx).child(usage).child(sources)).children(editor)
    }
}
