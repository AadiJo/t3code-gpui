//! Settings → Browser and Devices, the server-side rows of
//! `IntegrationsSettings.tsx` and `DeviceSourcesSettings.tsx`. The Browser
//! page's client rows (profiles, viewport, recordings, links) belong to the
//! client settings panels; this page draws the agent access row.

use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, prelude::*};
use t3code_contracts::ClientServerSettingsPatch;

use super::device_host_editor::{DeviceHostEditor, DeviceHostEditorEvent};
use super::server_settings_controls::{bool_setting, lock_reason};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::components::managed_policy::managed_by;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::i18n::t;

/// The Browser page's server rows.
pub struct BrowserSettingsPanel {
    model: Entity<ServerPanelsModel>,
    _observe: Subscription,
}

impl BrowserSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, _observe: observe }
    }
}

impl Render for BrowserSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.model.read(cx).snapshot().clone();
        let lock = lock_reason(managed_by(snapshot.managed_policy.as_deref(), &["enableAgentBrowserAccess"]), snapshot.read_only);
        page_container(cx).child(SettingsSection::new(t!("Agent access")).child(bool_setting(
            &self.model,
            "enableAgentBrowserAccess",
            t!("Agent browser access"),
            Some(t!("Let agents open, read, and control browser tabs on this environment.")),
            snapshot.settings.enable_agent_browser_access.unwrap_or(true),
            lock,
            |v| ClientServerSettingsPatch { enable_agent_browser_access: Some(v), ..Default::default() },
        )))
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
        let mut sources = SettingsSection::new(t!("Device sources")).header_action(
            Button::new("device-host-add")
                .variant(ButtonVariant::GhostMuted)
                .size(ButtonSize::Xs)
                .label(t!("Add device host"))
                .disabled(ro)
                .on_click(cx.listener(|this, _, window, cx| this.open_editor(None, window, cx))),
        );
        if hosts.is_empty() {
            sources.extend([SettingsRow::new(t!("No device hosts")).description(t!("Simulators on this environment are always available.")).into_any_element()]);
        }
        for (ix, host) in hosts.iter().enumerate() {
            let target = match host.port {
                Some(port) => format!("{} · {port}", host.target),
                None => host.target.clone(),
            };
            sources.extend([SettingsRow::new(SharedString::from(host.label.clone()))
                .description(SharedString::from(target))
                .control(
                    Button::new(SharedString::from(format!("device-host-edit-{ix}")))
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Xs)
                        .label(t!("Edit"))
                        .disabled(ro)
                        .on_click(cx.listener(move |this, _, window, cx| this.open_editor(Some(ix), window, cx))),
                )
                .into_any_element()]);
        }
        let usage = SettingsSection::new(t!("Devices"))
            .child(bool_setting(&self.model, "enableDeviceSupport", t!("Use devices"), Some(t!("Show iOS and Android devices in the right panel.")), s.enable_device_support.unwrap_or(false), lock(&["enableDeviceSupport"]), |v| ClientServerSettingsPatch { enable_device_support: Some(v), ..Default::default() }))
            .child(bool_setting(&self.model, "enableAgentDeviceAccess", t!("Agents can use devices"), Some(t!("Let agents open and control devices with MCP tools.")), s.enable_agent_device_access.unwrap_or(false), lock(&["enableAgentDeviceAccess"]), |v| ClientServerSettingsPatch { enable_agent_device_access: Some(v), ..Default::default() }));
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
