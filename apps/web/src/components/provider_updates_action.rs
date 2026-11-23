//! "Update all" in the Providers header. Ports `ProviderUpdatesAction.tsx`
//! and the candidate rules in `ProviderUpdateLaunchNotification.logic.ts`,
//! for the one environment the panel shows.

use gpui_kit::{App, Entity, IntoElement, RenderOnce, SharedString, Window, div, prelude::*};
use t3code_contracts::{
    ProviderDriverKind, ProviderInstanceId, ServerProviderCompatibilityStatus, ServerProviderUpdateInput,
    ServerProviderUpdateStatus, ServerProviderVersionAdvisoryStatus,
};

use crate::components::settings::server_settings_data::ServerPanelsModel;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::i18n::t;
use crate::provider_instances::{ServerProvider, provider_display_name};

/// `isProviderUpdateActive`.
pub fn is_provider_update_active(provider: &ServerProvider) -> bool {
    provider
        .update_state
        .as_ref()
        .is_some_and(|u| matches!(u.status, ServerProviderUpdateStatus::Queued | ServerProviderUpdateStatus::Running))
}

/// `isProviderSettingsUpdateCandidate`: enabled, behind latest, and updatable with a command.
pub fn is_provider_settings_update_candidate(provider: &ServerProvider) -> bool {
    let latest_bad = provider.compatibility_advisory.as_ref().is_some_and(|c| {
        matches!(c.latest_version_status, Some(ServerProviderCompatibilityStatus::Broken | ServerProviderCompatibilityStatus::Unsupported))
    });
    provider.enabled
        && !latest_bad
        && provider.version_advisory.as_ref().is_some_and(|a| {
            a.status == ServerProviderVersionAdvisoryStatus::BehindLatest && a.can_update == Some(true) && a.update_command.is_some()
        })
}

/// One update per driver that can run now (`collectProviderUpdateCandidates` + `canOneClickUpdateProviderCandidate`).
pub fn one_click_update_candidates(providers: &[std::sync::Arc<ServerProvider>]) -> Vec<std::sync::Arc<ServerProvider>> {
    let mut seen = Vec::<String>::new();
    providers
        .iter()
        .filter(|p| is_provider_settings_update_candidate(p) && !is_provider_update_active(p))
        .filter(|p| {
            let fresh = !seen.contains(&p.driver);
            seen.push(p.driver.clone());
            fresh
        })
        .cloned()
        .collect()
}

/// The header button. Renders nothing when there is nothing to update.
#[derive(IntoElement)]
pub struct ProviderUpdatesAction {
    model: Entity<ServerPanelsModel>,
}

impl ProviderUpdatesAction {
    pub fn new(model: Entity<ServerPanelsModel>) -> Self {
        Self { model }
    }
}

impl RenderOnce for ProviderUpdatesAction {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let model = self.model.read(cx);
        let snapshot = model.snapshot();
        let pending = model.is_pending("update-all");
        let candidates = if snapshot.read_only { Vec::new() } else { one_click_update_candidates(&snapshot.providers) };
        if candidates.is_empty() && !pending {
            return div().into_any_element();
        }
        let names: Vec<&str> = candidates.iter().map(|p| provider_display_name(&p.driver).unwrap_or(&p.driver)).collect();
        let summary: SharedString = format!("{}: {}", snapshot.environment_label, names.join(", ")).into();
        let model = self.model.clone();
        div()
            .id("provider-updates-action")
            .tooltip(tooltip(TooltipSurface::new(summary)))
            .child(
                Button::new("provider-update-all")
                    .variant(ButtonVariant::GhostMuted)
                    .size(ButtonSize::Xs)
                    .label(if pending { t!("Updating…") } else { t!("Update all") })
                    .disabled(pending)
                    .on_click(move |_, _, cx| {
                        let inputs: Vec<ServerProviderUpdateInput> = candidates
                            .iter()
                            .map(|p| ServerProviderUpdateInput {
                                provider: ProviderDriverKind::new(p.driver.as_str()),
                                target_version: None,
                                instance_id: Some(ProviderInstanceId::new(p.instance_id.as_str())),
                            })
                            .collect();
                        model.update(cx, |model, cx| {
                            model.run(
                                "update-all",
                                move |source| {
                                    let calls: Vec<_> = inputs.into_iter().map(|i| source.update_provider(i)).collect();
                                    Box::pin(async move {
                                        let mut last = Ok(Vec::new());
                                        for call in calls {
                                            last = call.await;
                                        }
                                        last
                                    })
                                },
                                |result, cx| {
                                    if let Err(error) = result {
                                        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not update providers")).description(error.message), cx);
                                    }
                                },
                                cx,
                            )
                        });
                    }),
            )
            .into_any_element()
    }
}
