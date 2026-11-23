//! "Add device host" / "Edit device host". Ports `DeviceHostEditor.tsx` for
//! one environment: name, SSH target, identity file and port, a connection
//! test (`device.testHost`) and save (`server.updateSettings` with `deviceHosts`).

use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext as _, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Window, div, prelude::*};
use t3code_contracts::{ClientServerSettingsPatch, SshDeviceHostConfig, SshDeviceHostConfigKind};

use super::provider_settings_panel::toast_error;
use super::server_settings_controls::{field_input, hint, save_patch, tinted};
use super::server_settings_data::ServerPanelsModel;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::DialogSurface;
use crate::i18n::t;

/// The editor closes.
#[derive(Clone, Debug)]
pub enum DeviceHostEditorEvent {
    Close,
}

/// What the last connection test said.
#[derive(Clone, Debug)]
enum TestState {
    Idle,
    Checking,
    Passed,
    Failed(SharedString),
}

/// The dialog.
pub struct DeviceHostEditor {
    model: Entity<ServerPanelsModel>,
    index: Option<usize>,
    name: Entity<InputState>,
    target: Entity<InputState>,
    identity: Entity<InputState>,
    port: Entity<InputState>,
    test: TestState,
}

impl EventEmitter<DeviceHostEditorEvent> for DeviceHostEditor {}

impl DeviceHostEditor {
    pub fn new(model: Entity<ServerPanelsModel>, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let existing = index.and_then(|ix| model.read(cx).snapshot().settings.device_hosts.clone().unwrap_or_default().get(ix).cloned());
        let field = |value: String, placeholder: SharedString, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
        };
        let name = field(existing.as_ref().map(|h| h.label.clone()).unwrap_or_default(), t!("Studio Mac"), window, cx);
        let target = field(existing.as_ref().map(|h| h.target.clone()).unwrap_or_default(), t!("user@host or SSH alias"), window, cx);
        let identity = field(existing.as_ref().and_then(|h| h.identity_file.clone()).unwrap_or_default(), t!("SSH config default"), window, cx);
        let port = field(existing.as_ref().and_then(|h| h.port).map(|p| p.to_string()).unwrap_or_default(), t!("Default", context = "uses the default SSH port"), window, cx);
        Self { model, index, name, target, identity, port, test: TestState::Idle }
    }

    fn config(&self, cx: &gpui_kit::App) -> Option<SshDeviceHostConfig> {
        let label = self.name.read(cx).value().trim().to_owned();
        let target = self.target.read(cx).value().trim().to_owned();
        if label.is_empty() || target.is_empty() {
            return None;
        }
        let identity = self.identity.read(cx).value().trim().to_owned();
        let port = self.port.read(cx).value().trim().parse::<u64>().ok().filter(|p| (1..=65535).contains(p));
        let id = format!("ssh-{}", label.to_lowercase().replace(|c: char| !c.is_ascii_alphanumeric(), "-"));
        Some(SshDeviceHostConfig {
            kind: Some(SshDeviceHostConfigKind::Ssh),
            id,
            label,
            target,
            identity_file: (!identity.is_empty()).then_some(identity),
            port,
        })
    }

    /// `device.testHost`.
    pub fn test_connection(&mut self, cx: &mut Context<Self>) {
        let Some(config) = self.config(cx) else { return };
        self.test = TestState::Checking;
        let this = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.run("device-test-host", move |s| s.test_device_host(config), move |result, cx| {
                this.update(cx, |this, cx| {
                    this.test = match result {
                        Ok(_) => TestState::Passed,
                        Err(e) => TestState::Failed(e.message),
                    };
                    cx.notify();
                })
                .ok();
            }, cx)
        });
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(config) = self.config(cx) else {
            toast_error(t!("Could not save device host"), super::server_settings_data::RpcFailure::new(t!("Enter a name and an SSH target.")), cx);
            return;
        };
        let mut hosts = self.model.read(cx).snapshot().settings.device_hosts.clone().unwrap_or_default();
        match self.index {
            Some(ix) if ix < hosts.len() => hosts[ix] = config,
            _ => hosts.push(config),
        }
        save_patch(&self.model, "deviceHosts", ClientServerSettingsPatch { device_hosts: Some(hosts), ..Default::default() }, cx);
        cx.emit(DeviceHostEditorEvent::Close);
    }
}

impl Render for DeviceHostEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let environment = self.model.read(cx).snapshot().environment_label.clone();
        let label = |text: SharedString| div().ui_text(theme, TextSize::Xs, weight::MEDIUM).child(text);
        let status = match &self.test {
            TestState::Idle => hint(t!("Check access before saving"), cx),
            TestState::Checking => hint(t!("Checking environments…"), cx),
            TestState::Passed => tinted(t!("Connection checks passed"), theme.fixed.success, cx),
            TestState::Failed(e) => tinted(e.clone(), c.destructive(), cx),
        };
        DialogSurface::new(if self.index.is_none() { t!("Add device host") } else { t!("Edit device host") })
            .description(t!("Connect from {environment}.", environment = environment))
            .on_close({
                let this = cx.entity().downgrade();
                move |_, cx| {
                    this.update(cx, |_, cx| cx.emit(DeviceHostEditorEvent::Close)).ok();
                }
            })
            .child(div().flex().flex_col().gap(theme.sp(1.5)).child(label(t!("Name"))).child(field_input(&self.name, false, false, false)))
            .child(div().flex().flex_col().gap(theme.sp(1.5)).child(label(t!("SSH target"))).child(field_input(&self.target, true, false, false)))
            .child(
                div()
                    .flex()
                    .gap(theme.sp(3.))
                    .child(div().flex_1().flex().flex_col().gap(theme.sp(1.5)).child(label(t!("Identity file"))).child(field_input(&self.identity, true, false, false)))
                    .child(div().w(theme.sp(24.)).flex().flex_col().gap(theme.sp(1.5)).child(label(t!("Port"))).child(field_input(&self.port, true, false, false))),
            )
            .child(hint(t!("Optional. Resolved separately on each environment."), cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(Button::new("device-test").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Test connection")).on_click(cx.listener(|this, _, _, cx| this.test_connection(cx))))
                    .child(status),
            )
            .footer(Button::new("device-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(cx.listener(|_, _, _, cx| cx.emit(DeviceHostEditorEvent::Close))))
            .footer(Button::new("device-save").label(t!("Save")).on_click(cx.listener(|this, _, _, cx| this.save(cx))))
    }
}
