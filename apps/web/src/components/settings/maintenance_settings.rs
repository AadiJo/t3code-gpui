//! Settings → Maintenance. Ports `MaintenanceSettingsPanel` (`SettingsPanels.tsx`):
//! Updates, Background activity (`BackgroundActivitySettings.tsx`), Artifacts
//! and logs and Cleanup (`StorageCleanupSections`), and the link to
//! Diagnostics, which opens [`DiagnosticsSettingsPanel`] in place.

use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use t3code_contracts::{BackgroundActivityProfile, ClientServerSettingsPatch, ClientServerSettingsPatchStorageCleanup};

use super::diagnostics_settings::DiagnosticsSettingsPanel;
use super::provider_settings_panel::toast_error;
use super::server_settings_controls::{bool_setting, lock_reason, retention_control, save_patch, select_row};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::theme_color::with_alpha;
use crate::components::managed_policy::managed_by;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;
use crate::timestamp_format::{format_relative_time_label, now_ms};

/// The Maintenance page.
pub struct MaintenanceSettingsPanel {
    model: Entity<ServerPanelsModel>,
    diagnostics: Option<Entity<DiagnosticsSettingsPanel>>,
    last_run_open: bool,
    _observe: Subscription,
}

impl MaintenanceSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, diagnostics: None, last_run_open: false, _observe: observe }
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
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let s = &snapshot.settings;
        let policy = snapshot.managed_policy.as_deref();
        let ro = snapshot.read_only;
        let lock = |paths: &[&str]| lock_reason(managed_by(policy, paths), ro);
        let model = self.model.clone();
        let pending = self.model.read(cx).is_pending("storage-cleanup");

        let updates = SettingsSection::new(t!("Updates"))
            .child(bool_setting(&model, "enableProviderUpdateChecks", t!("Provider update checks"), Some(t!("Check installed provider CLIs for newer available versions and fetch model catalog updates. When off, new models may not appear.")), s.enable_provider_update_checks.unwrap_or(true), lock(&["enableProviderUpdateChecks"]), |v| ClientServerSettingsPatch { enable_provider_update_checks: Some(v), ..Default::default() }))
            .child(bool_setting(&model, "continueThreadsAfterServerUpdate", t!("Continue threads after restarts"), Some(t!("Automatically resume interrupted threads after an update, crash, or machine restart on the selected environments.")), s.continue_threads_after_server_update.unwrap_or(true), lock(&["continueThreadsAfterServerUpdate"]), |v| ClientServerSettingsPatch { continue_threads_after_server_update: Some(v), ..Default::default() }));

        let profiles = [BackgroundActivityProfile::Balanced, BackgroundActivityProfile::Performance, BackgroundActivityProfile::BatterySaver];
        let profile_ix = profiles.iter().position(|p| Some(*p) == s.background_activity_profile).unwrap_or(0);
        let profile_descriptions = [
            t!("Pauses probes for idle clients, locked hosts, or low power mode."),
            t!("Allows scoped background probes while any subscribed client remains connected."),
            t!("Also pauses background probes when the host or client is on battery."),
        ];
        let profile_model = model.clone();
        // `PolicyTooltip`: a 20px info button beside the title.
        let policy_info = div()
            .id("background-policy-info")
            .size(theme.sp(5.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.radii.md)
            .tooltip(tooltip(TooltipSurface::new(t!("This shared policy gates background work such as Git refreshes and provider health probes after their individual intervals elapse."))))
            .child(Icon::new(IconName::Info).size(IconSize::Sm).color(c.muted_foreground));
        let background = SettingsSection::new(t!("Background activity"))
            .child(select_row("backgroundActivityProfile", t!("Background activity"), Some(profile_descriptions[profile_ix].clone()), vec![t!("Balanced"), t!("Performance"), t!("Battery saver")], profile_ix, lock(&["backgroundActivityProfile"]), move |ix, _, cx| {
                save_patch(&profile_model, "backgroundActivityProfile", ClientServerSettingsPatch { background_activity_profile: Some(profiles[ix]), ..Default::default() }, cx);
            }).reset(policy_info))
            .child(bool_setting(&model, "keepAwakeWhileAgentsWork", t!("Keep awake while agents work"), Some(t!("Stop the computer from going to sleep while agents are working, and let it sleep once they finish. Closing a laptop lid still puts it to sleep.")), s.keep_awake_while_agents_work.unwrap_or(false), lock(&["keepAwakeWhileAgentsWork"]), |v| ClientServerSettingsPatch { keep_awake_while_agents_work: Some(v), ..Default::default() }));

        let sc = s.storage_cleanup.clone().unwrap_or_default();
        let (artifacts_model, logs_model) = (model.clone(), model.clone());
        let artifacts = SettingsSection::new(t!("Artifacts and logs"))
            .child(SettingsRow::new(t!("Delete old browser artifacts")).description(t!("Delete saved browser captures after this many days. Older capture links will no longer open.")).control(
                retention_control("browser-artifacts", sc.browser_artifacts_after_days, 1, ro || lock(&["storageCleanup.browserArtifactsAfterDays"]).is_some(), move |v, _, cx| {
                    save_patch(&artifacts_model, "storageCleanup.browserArtifactsAfterDays", storage(|c| c.browser_artifacts_after_days = Some(v)), cx);
                }, cx),
            ))
            .child(SettingsRow::new(t!("Delete old rotated logs")).description(t!("Delete inactive rotated log files after this many days. Current logs are kept.")).control(
                retention_control("rotated-logs", sc.logs_after_days, 1, ro || lock(&["storageCleanup.logsAfterDays"]).is_some(), move |v, _, cx| {
                    save_patch(&logs_model, "storageCleanup.logsAfterDays", storage(|c| c.logs_after_days = Some(v)), cx);
                }, cx),
            ));

        let has_rules = sc.browser_artifacts_after_days.is_some()
            || sc.logs_after_days.is_some()
            || sc.worktree_after_days.is_some()
            || sc.worktree_settled_after_days.is_some()
            || sc.worktree_on_merge.unwrap_or(false)
            || sc.worktree_on_delete.unwrap_or(true)
            || sc.worktree_unchanged.unwrap_or(false);
        let report = snapshot.storage_report.clone();
        let counts = report.as_ref().map(|r| serde_json::to_value(&r.counts).unwrap_or_default());
        let n = |k: &str| counts.as_ref().and_then(|c| c.get(k)).and_then(|v| v.as_u64()).unwrap_or(0);
        let description: SharedString = if !has_rules {
            t!("Turn on a cleanup rule to use this.")
        } else {
            match &report {
                Some(report) => {
                    let bytes = report.bytes_freed.as_ref().and_then(|b| serde_json::to_value(b).ok()).and_then(|v| v.as_f64()).unwrap_or(0.);
                    let when = format_relative_time_label(&report.finished_at, now_ms());
                    let bytes = format_bytes(bytes);
                    match n("failed") {
                        0 => t!("Last run {when}. {bytes} recovered.", when = when, bytes = bytes),
                        1 => t!("Last run {when}. {bytes} recovered. {count} item couldn't be removed.", when = when, bytes = bytes, count = 1),
                        f => t!("Last run {when}. {bytes} recovered. {count} items couldn't be removed.", when = when, bytes = bytes, count = f),
                    }
                }
                None => t!("Cleanup hasn't run yet."),
            }
        };
        let run_model = model.clone();
        // `SettingsSection variant="plain"`: the Delete now group, then the Last run fold, 4px apart.
        let mut cleanup = SettingsSection::new(t!("Cleanup")).plain().child(
            SettingsSection::new(t!("Cleanup")).hide_title().child(
                SettingsRow::new(t!("Delete now")).description(description).control(
                    Button::new("storage-delete-now")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .label(if pending { t!("Deleting…") } else { t!("Delete now") })
                        .disabled(ro || pending || !has_rules)
                        .on_click(move |_, _, cx| {
                            run_model.update(cx, |m, cx| {
                                m.run("storage-cleanup", |s| s.run_storage_cleanup(), |result, cx| {
                                    if let Err(e) = result {
                                        toast_error(t!("Could not run cleanup"), e, cx);
                                    }
                                }, cx)
                            });
                        }),
                ),
            ),
        );
        if let Some(report) = report {
            let summary: SharedString = if n("failed") > 0 {
                t!("{removed} removed, {kept} kept, {failed} failed", removed = n("removed"), kept = n("kept"), failed = n("failed"))
            } else {
                t!("{removed} removed, {kept} kept", removed = n("removed"), kept = n("kept"))
            };
            let open = self.last_run_open;
            let entries = serde_json::to_value(&report.entries).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            cleanup = cleanup.child(
                div()
                    .rounded(theme.radii.xl)
                    .border_1()
                    .border_color(with_alpha(c.border, 0.6))
                    .bg(with_alpha(c.card(), 0.4))
                    .child(
                        div()
                            .id("cleanup-last-run")
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .min_h(theme.sp(11.))
                            .px(theme.sp(4.))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.last_run_open = !this.last_run_open;
                                cx.notify();
                            }))
                            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Md).color(c.muted_foreground))
                            .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(t!("Last run")))
                            .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(summary)),
                    )
                    .when(open, |d| {
                        d.children(entries.iter().map(|entry| {
                            let name = entry.get("path").and_then(|p| p.as_str()).and_then(|p| p.rsplit(['/', '\\']).find(|s| !s.is_empty())).map(str::to_owned).unwrap_or_else(|| t!("Worktrees").to_string());
                            let outcome = entry.get("outcome").and_then(|o| o.as_str()).unwrap_or_default().to_owned();
                            let reason = entry.get("reason").and_then(|o| o.as_str()).unwrap_or_default().to_owned();
                            div().border_t_1().border_color(with_alpha(c.border, 0.5)).child(
                                SettingsRow::new(SharedString::from(name)).description(SharedString::from(if reason.is_empty() { outcome } else { format!("{outcome} · {reason}") })),
                            )
                        }))
                    }),
            );
        }

        let diagnostics = SettingsSection::new(t!("Diagnostics")).child(
            SettingsRow::new(t!("Diagnostics")).description(t!("Inspect processes, resource use, and logs on this environment.")).control(
                Button::new("open-diagnostics").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("View diagnostics")).on_click(cx.listener(|this, _, _, cx| this.open_diagnostics(cx))),
            ),
        );
        page_container(cx).child(updates).child(background).child(artifacts).child(cleanup).child(diagnostics).into_any_element()
    }
}

/// `formatBytes`: "512 B", "1.50 KB", "12.3 MB", "640 GB".
fn format_bytes(value: f64) -> String {
    let scaled = |v: f64| format!("{:.*}", if v >= 100. { 0 } else if v >= 10. { 1 } else { 2 }, v);
    if value.round() < 1024. {
        return format!("{} B", value.round());
    }
    let units = ["KB", "MB", "GB", "TB"];
    let (mut next, mut unit) = (value / 1024., 0);
    while scaled(next).parse::<f64>().unwrap_or(0.) >= 1024. && unit < units.len() - 1 {
        next /= 1024.;
        unit += 1;
    }
    format!("{} {}", scaled(next), units[unit])
}
