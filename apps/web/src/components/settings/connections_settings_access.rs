//! Settings → Sharing & access, the server rows of `ConnectionsSettings`
//! `view="access"`: Environment icon, Require approval for new devices and
//! Preview tunnel. The rows that only manage a local server (network access,
//! Tailscale HTTPS, WSL, this machine) are dropped in this remote-only app.
//! Authorized clients and pairing links use HTTP endpoints and are not built.

use gpui_kit::{Context, Entity, IntoElement, Render, SharedString, Subscription, Window, prelude::*};
use t3code_contracts::{ClientServerSettingsPatch, EnvironmentMachineKind};

use super::server_settings_controls::{bool_setting, lock_reason, save_patch, select_row};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsSection, page_container};
use crate::components::managed_policy::{ManagedCapability, managed_by, managed_capability_denial};
use crate::i18n::t;

/// The access page.
pub struct AccessSettingsPanel {
    model: Entity<ServerPanelsModel>,
    _observe: Subscription,
}

impl AccessSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, _observe: observe }
    }
}

fn machine_label(kind: EnvironmentMachineKind) -> SharedString {
    let label = kind.as_str().split('-').map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
    });
    SharedString::from(label.collect::<Vec<_>>().join(" "))
}

impl Render for AccessSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.model.read(cx).snapshot().clone();
        let s = &snapshot.settings;
        let policy = snapshot.managed_policy.as_deref();
        let ro = snapshot.read_only;
        let kinds = EnvironmentMachineKind::ALL;
        let kind_ix = kinds.iter().position(|k| Some(*k) == s.environment_icon).unwrap_or(0);
        let icon_model = self.model.clone();
        let tunnel_lock = managed_capability_denial(policy, ManagedCapability::PreviewTunnel)
            .map(SharedString::from)
            .or_else(|| lock_reason(managed_by(policy, &["enablePreviewTunnel"]), ro));
        let environment = SettingsSection::new(t!("This environment"))
            .child(select_row("environmentIcon", t!("Environment icon"), Some(t!("Shown next to this environment's name in the sidebar and pickers.")), kinds.iter().map(|k| machine_label(*k)).collect(), kind_ix, lock_reason(managed_by(policy, &["environmentIcon"]), ro), move |ix, _, cx| {
                save_patch(&icon_model, "environmentIcon", ClientServerSettingsPatch { environment_icon: Some(EnvironmentMachineKind::ALL[ix]), ..Default::default() }, cx);
            }))
            .child(bool_setting(&self.model, "requireConnectDeviceApproval", t!("Require approval for new devices"), Some(t!("New T3 Connect devices wait until you approve them from a device that already has access.")), s.require_connect_device_approval.unwrap_or(true), lock_reason(managed_by(policy, &["requireConnectDeviceApproval"]), ro), |v| ClientServerSettingsPatch { require_connect_device_approval: Some(v), ..Default::default() }))
            .child(bool_setting(&self.model, "enablePreviewTunnel", t!("Preview tunnel"), Some(t!("Open this machine's local servers in browser tabs on other devices.")), s.enable_preview_tunnel.unwrap_or(true), tunnel_lock, |v| ClientServerSettingsPatch { enable_preview_tunnel: Some(v), ..Default::default() }));
        page_container(cx).child(environment)
    }
}
