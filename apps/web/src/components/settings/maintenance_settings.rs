//! Settings → Maintenance. Ports `MaintenanceSettingsPanel` (`SettingsPanels.tsx`),
//! the cleanup and artifact rows of `StorageSettings.tsx`,
//! `BackgroundActivitySettings.tsx` (profile) and the link to Diagnostics,
//! which opens [`DiagnosticsSettingsPanel`] in place.

use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use t3code_contracts::{BackgroundActivityProfile, ClientServerSettingsPatch, ClientServerSettingsPatchStorageCleanup};

use super::diagnostics_settings::DiagnosticsSettingsPanel;
use super::provider_settings_panel::toast_error;
use super::server_settings_controls::{bool_setting, hint, interval_seconds_field, lock_reason, save_patch, select_row};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::components::managed_policy::managed_by;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::timestamp_format::{format_relative_time_label, now_ms};

/// The Maintenance page.
pub struct MaintenanceSettingsPanel {
    model: Entity<ServerPanelsModel>,
    diagnostics: Option<Entity<DiagnosticsSettingsPanel>>,
    _observe: Subscription,
}

impl MaintenanceSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, diagnostics: None, _observe: observe }
    }

    /// Opens Diagnostics in place of the page.
    pub fn open_diagnostics(&mut self, cx: &mut Context<Self>) {
        let model = self.model.clone();
        self.diagnostics = Some(cx.new(|cx| DiagnosticsSettingsPanel::new(model, cx)));
        cx.notify();
    }
}

fn storage(f: impl FnOnce(&mut ClientServerSettingsPatchStorageCleanup)) -> ClientServerSettingsPatch {
    let mut c = ClientServerSettingsPatchStorageCleanup::default();
    f(&mut c);
    ClientServerSettingsPatch { storage_cleanup: Some(c), ..Default::default() }
}

impl Render for MaintenanceSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(diagnostics) = self.diagnostics.clone() {
            return div()
                .child(div().px(cx.app_theme().sp(6.)).pt(cx.app_theme().sp(4.)).child(
                    Button::new("diagnostics-back").variant(ButtonVariant::GhostMuted).size(ButtonSize::Xs).label(t!("Maintenance")).on_click(cx.listener(|this, _, _, cx| {
                        this.diagnostics = None;
                        cx.notify();
                    })),
                ))
                .child(diagnostics)
                .into_any_element();
        }
        let snapshot = self.model.read(cx).snapshot().clone();
        let s = &snapshot.settings;
        let policy = snapshot.managed_policy.as_deref();
        let ro = snapshot.read_only;
        let lock = |paths: &[&str]| lock_reason(managed_by(policy, paths), ro);
        let model = self.model.clone();
        let pending = self.model.read(cx).is_pending("storage-cleanup");

        let last_run: SharedString = match &snapshot.storage_report {
            Some(report) => {
                let counts = serde_json::to_value(&report.counts).unwrap_or_default();
                let n = |k: &str| counts.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                format!(
                    "{} · {}",
                    t!("Last run {when}.", when = format_relative_time_label(&report.finished_at, now_ms())),
                    t!("{removed} removed, {kept} kept", removed = n("removed"), kept = n("kept"))
                )
                .into()
            }
            None => t!("Cleanup hasn't run yet."),
        };
        let run_model = model.clone();
        let cleanup = SettingsSection::new(t!("Cleanup")).child(
            SettingsRow::new(t!("Delete now")).description(last_run).control(
                Button::new("storage-delete-now")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(if pending { t!("Deleting…") } else { t!("Delete now") })
                    .disabled(ro || pending)
                    .on_click(move |_, _, cx| {
                        run_model.update(cx, |m, cx| {
                            m.run("storage-cleanup", |s| s.run_storage_cleanup(), |result, cx| match result {
                                Ok(_) => {
                                    Toaster::push(ToastData::new(ToastKind::Success, t!("Cleanup finished")), cx);
                                }
                                Err(e) => toast_error(t!("Could not run cleanup"), e, cx),
                            }, cx)
                        });
                    }),
            ),
        );
        let sc = s.storage_cleanup.clone().unwrap_or_default();
        let (artifacts_model, logs_model) = (model.clone(), model.clone());
        let artifacts = SettingsSection::new(t!("Artifacts and logs"))
            .child(SettingsRow::new(t!("Delete old browser artifacts")).description(t!("Delete saved browser captures after this many days. Older capture links will no longer open.")).control(
                interval_seconds_field("browser-artifacts-days", sc.browser_artifacts_after_days.unwrap_or(30), 1, 1, ro || lock(&["storageCleanup.browserArtifactsAfterDays"]).is_some(), move |d, _, cx| {
                    save_patch(&artifacts_model, "storageCleanup.browserArtifactsAfterDays", storage(|c| c.browser_artifacts_after_days = Some(d)), cx);
                }, cx),
            ))
            .child(SettingsRow::new(t!("Delete old rotated logs")).description(t!("Delete inactive rotated log files after this many days. Current logs are kept.")).control(
                interval_seconds_field("logs-days", sc.logs_after_days.unwrap_or(14), 1, 1, ro || lock(&["storageCleanup.logsAfterDays"]).is_some(), move |d, _, cx| {
                    save_patch(&logs_model, "storageCleanup.logsAfterDays", storage(|c| c.logs_after_days = Some(d)), cx);
                }, cx),
            ));

        let profiles = [BackgroundActivityProfile::Balanced, BackgroundActivityProfile::Performance, BackgroundActivityProfile::BatterySaver];
        let profile_ix = profiles.iter().position(|p| Some(*p) == s.background_activity_profile).unwrap_or(0);
        let profile_descriptions = [
            t!("Pauses probes for idle clients, locked hosts, or low power mode."),
            t!("Allows scoped background probes while any subscribed client remains connected."),
            t!("Also pauses background probes when the host or client is on battery."),
        ];
        let profile_model = model.clone();
        let environment = SettingsSection::new(t!("Environment"))
            .child(bool_setting(&model, "enableProviderUpdateChecks", t!("Provider update checks"), Some(t!("Check installed provider CLIs for newer available versions and fetch model catalog updates. When off, new models may not appear.")), s.enable_provider_update_checks.unwrap_or(true), lock(&["enableProviderUpdateChecks"]), |v| ClientServerSettingsPatch { enable_provider_update_checks: Some(v), ..Default::default() }))
            .child(bool_setting(&model, "continueThreadsAfterServerUpdate", t!("Continue threads after restarts"), Some(t!("Automatically resume interrupted threads after an update, crash, or machine restart on the selected environments.")), s.continue_threads_after_server_update.unwrap_or(true), lock(&["continueThreadsAfterServerUpdate"]), |v| ClientServerSettingsPatch { continue_threads_after_server_update: Some(v), ..Default::default() }))
            .child(bool_setting(&model, "keepAwakeWhileAgentsWork", t!("Keep awake while agents work"), Some(t!("Stop the computer from going to sleep while agents are working, and let it sleep once they finish. Closing a laptop lid still puts it to sleep.")), s.keep_awake_while_agents_work.unwrap_or(false), lock(&["keepAwakeWhileAgentsWork"]), |v| ClientServerSettingsPatch { keep_awake_while_agents_work: Some(v), ..Default::default() }))
            .child(select_row("backgroundActivityProfile", t!("Background activity"), Some(profile_descriptions[profile_ix].clone()), vec![t!("Balanced"), t!("Performance"), t!("Battery saver")], profile_ix, lock(&["backgroundActivityProfile"]), move |ix, _, cx| {
                save_patch(&profile_model, "backgroundActivityProfile", ClientServerSettingsPatch { background_activity_profile: Some(profiles[ix]), ..Default::default() }, cx);
            }))
            .child(SettingsRow::new(t!("Diagnostics")).description(t!("Traces, processes, and resource history for this environment.")).control(
                Button::new("open-diagnostics").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Open")).on_click(cx.listener(|this, _, _, cx| this.open_diagnostics(cx))),
            ));
        page_container(cx).child(cleanup).child(artifacts).child(environment).into_any_element()
    }
}
