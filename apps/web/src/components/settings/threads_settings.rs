//! Settings → Threads. Ports `ThreadsSettingsPanel` (`SettingsPanels.tsx`),
//! the environment rows of `ProjectDefaultsSettings.tsx` (category general),
//! `SnoozePresetsSettings.tsx` (shown, edited elsewhere) and
//! `ThreadTitleInstructionsSettings.tsx`, at environment scope.

use gpui_kit::{App, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{
    ClientServerSettingsPatch, ClientServerSettingsPatchTextGenerationModelSelection, ModelSelection, ResponseStreamingMode,
    RuntimeMode, SidebarAutoSettleAfterDays, ThreadEnvMode,
};

use super::server_settings_controls::{
    advanced_fold,
    Fields, bool_setting, field_input, hint, interval_seconds_field, lock_reason, reset_button, save_patch, select_row,
};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::components::managed_policy::managed_by;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;
use crate::provider_instances::provider_instance_entries_for;

/// `AUTO_SETTLE_DEFAULT_DAYS`.
const AUTO_SETTLE_DEFAULT_DAYS: f64 = 3.;

/// Every selectable model on the environment, as "Provider · Model" labels.
pub fn model_options(model: &ServerPanelsModel) -> Vec<(ModelSelection, SharedString)> {
    let snapshot = model.snapshot();
    provider_instance_entries_for(&snapshot.providers, &snapshot.settings, None)
        .into_iter()
        .filter(|entry| entry.enabled && entry.is_available)
        .flat_map(|entry| {
            entry
                .models()
                .iter()
                .filter(|m| m.is_legacy != Some(true))
                .map(|m| {
                    (
                        ModelSelection { instance_id: entry.instance_id.clone(), model: m.slug.clone(), options: None },
                        SharedString::from(format!("{} · {}", entry.display_name, m.name)),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A select row over the environment's models; `none_label` adds a first "none" choice.
pub fn model_select_row(
    model: &Entity<ServerPanelsModel>,
    id: &'static str,
    title: SharedString,
    description: Option<SharedString>,
    current: Option<&ModelSelection>,
    none_label: Option<SharedString>,
    lock: Option<SharedString>,
    patch: fn(Option<ModelSelection>) -> ClientServerSettingsPatch,
    cx: &App,
) -> SettingsRow {
    let options = model_options(model.read(cx));
    if options.is_empty() && none_label.is_none() {
        return SettingsRow::new(title).control(hint(t!("No providers available"), cx));
    }
    let offset = usize::from(none_label.is_some());
    let mut labels: Vec<SharedString> = none_label.into_iter().collect();
    labels.extend(options.iter().map(|(_, l)| l.clone()));
    let selected = current
        .and_then(|c| options.iter().position(|(s, _)| s.instance_id == c.instance_id && s.model == c.model))
        .map_or(0, |ix| ix + offset);
    let model = model.clone();
    select_row(id, title, description, labels, selected, lock, move |ix, _, cx| {
        let value = if ix < offset { None } else { options.get(ix - offset).map(|(s, _)| s.clone()) };
        save_patch(&model, id, patch(value), cx);
    })
}

/// The Threads page.
pub struct ThreadsSettingsPanel {
    model: Entity<ServerPanelsModel>,
    fields: Fields,
    advanced_open: bool,
    _observe: Subscription,
}

impl ThreadsSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, fields: Fields::default(), advanced_open: false, _observe: observe }
    }
}

fn runtime_mode_label(mode: RuntimeMode) -> SharedString {
    match mode {
        RuntimeMode::ApprovalRequired => t!("Supervised"),
        RuntimeMode::AutoAcceptEdits => t!("Auto-accept edits"),
        RuntimeMode::Auto => t!("Auto"),
        RuntimeMode::FullAccess => t!("Full access"),
    }
}

impl Render for ThreadsSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let snapshot = self.model.read(cx).snapshot().clone();
        let s = &snapshot.settings;
        let policy = snapshot.managed_policy.as_deref();
        let ro = snapshot.read_only;
        let lock = |paths: &[&str]| lock_reason(managed_by(policy, paths), ro);
        let model = &self.model;

        // New projects.
        let save_base = model.clone();
        let base = self.fields.draft("addProjectBaseDirectory", s.add_project_base_directory.clone().unwrap_or_default(), "~/", false, window, cx, move |text, _, cx| {
            let patch = ClientServerSettingsPatch { add_project_base_directory: Some(text.trim().to_owned()), ..Default::default() };
            save_patch(&save_base, "addProjectBaseDirectory", patch, cx);
        });
        let new_projects = SettingsSection::new(t!("New projects"))
            .child(
                SettingsRow::new(t!("Add project starts in"))
                    .description(t!("Leave empty to use \"~/\" when the Add Project browser opens."))
                    .control(div().w(px(288.)).child(field_input(&base, false, false, ro))),
            )
            .child(bool_setting(model, "ghqCloneLayout", t!("Clone into host folders"), Some(t!("Clones go to github.com/owner/repo under the folder above, like ghq.")), s.ghq_clone_layout.unwrap_or(false), lock(&["ghqCloneLayout"]), |v| ClientServerSettingsPatch { ghq_clone_layout: Some(v), ..Default::default() }));

        // Project defaults (environment scope).
        let env_modes = [(ThreadEnvMode::Local, t!("Current checkout")), (ThreadEnvMode::Worktree, t!("New worktree"))];
        let env_ix = env_modes.iter().position(|(m, _)| Some(*m) == s.default_thread_env_mode).unwrap_or(0);
        let env_model = model.clone();
        let runtime_modes = RuntimeMode::ALL;
        let runtime_ix = runtime_modes.iter().position(|m| Some(*m) == s.default_runtime_mode).unwrap_or(3);
        let runtime_model = model.clone();
        let defaults = SettingsSection::new(t!("New threads"))
            .child(model_select_row(model, "defaultModelSelection", t!("Model"), Some(t!("Default model for new threads. Projects can override it.")), s.default_model_selection.as_ref(), None, lock(&["defaultModelSelection"]), |v| ClientServerSettingsPatch { default_model_selection: Some(v), ..Default::default() }, cx))
            .child(model_select_row(model, "fallbackModelSelection", t!("Fallback model"), Some(t!("Used for new threads when the default model's provider is out of usage.")), s.fallback_model_selection.as_ref(), Some(t!("None", context = "no fallback model")), lock(&["fallbackModelSelection"]), |v| ClientServerSettingsPatch { fallback_model_selection: Some(v), ..Default::default() }, cx))
            .child(select_row("defaultRuntimeMode", t!("Permissions"), Some(t!("Default permissions for new threads. Projects can override them.")), runtime_modes.iter().map(|m| runtime_mode_label(*m)).collect(), runtime_ix, lock(&["defaultRuntimeMode"]), move |ix, _, cx| {
                let patch = ClientServerSettingsPatch { default_runtime_mode: Some(RuntimeMode::ALL[ix]), ..Default::default() };
                save_patch(&runtime_model, "defaultRuntimeMode", patch, cx);
            }))
            .child(select_row("defaultThreadEnvMode", t!("Workspace"), Some(t!("Where new threads start. Projects and their t3.json can override it.")), env_modes.iter().map(|(_, l)| l.clone()).collect(), env_ix, lock(&["defaultThreadEnvMode"]), move |ix, _, cx| {
                let patch = ClientServerSettingsPatch { default_thread_env_mode: Some(Some([ThreadEnvMode::Local, ThreadEnvMode::Worktree][ix])), ..Default::default() };
                save_patch(&env_model, "defaultThreadEnvMode", patch, cx);
            }));

        // Snooze options: the presets, listed.
        let presets = s.snooze_presets.as_ref().and_then(|p| serde_json::to_value(p).ok()).and_then(|v| v.get("presets").cloned()).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let preset_labels: Vec<String> = presets.iter().filter_map(|p| p.get("label").and_then(|l| l.as_str()).map(str::to_owned).or_else(|| p.get("id").and_then(|i| i.as_str()).map(str::to_owned))).collect();
        let snooze = SettingsSection::new(t!("Snooze")).child(
            SettingsRow::new(t!("Snooze options"))
                .description(t!("Choices in every snooze menu, listed soonest first. Weekdays mean the next one after today."))
                .status(SharedString::from(if preset_labels.is_empty() { "Default".to_owned() } else { preset_labels.join(" · ") })),
        );

        let usage = SettingsSection::new(t!("Usage limits"))
            .child(bool_setting(model, "autoResumeLimitedThreads", t!("Resume after usage limits"), Some(t!("Automatically resume threads when their usage limits reset. You can cancel auto-resume in each thread.")), s.auto_resume_limited_threads.unwrap_or(false), lock(&["autoResumeLimitedThreads"]), |v| ClientServerSettingsPatch { auto_resume_limited_threads: Some(v), ..Default::default() }))
            .child(bool_setting(model, "snoozeLimitedThreads", t!("Snooze until limits reset"), Some(t!("Snooze threads until their usage limits reset. Enable auto-resume to continue the agent after the reset.")), s.snooze_limited_threads.unwrap_or(false), lock(&["snoozeLimitedThreads"]), |v| ClientServerSettingsPatch { snooze_limited_threads: Some(v), ..Default::default() }));

        // Auto-settle.
        let days = match &s.sidebar_auto_settle_after_days {
            Some(SidebarAutoSettleAfterDays::Number(n)) => Some(*n),
            Some(SidebarAutoSettleAfterDays::Text(_)) => None,
            None => Some(AUTO_SETTLE_DEFAULT_DAYS),
        };
        let inactive_model = model.clone();
        let days_model = model.clone();
        let mut auto_settle = SettingsSection::new(t!("Auto-settle"))
            .child(bool_setting(model, "sidebarAutoSettleOnMerge", t!("Auto-settle merged threads"), Some(t!("Settle a thread when its pull request merges. Closed pull requests still settle automatically.")), s.sidebar_auto_settle_on_merge.unwrap_or(true), lock(&["sidebarAutoSettleOnMerge"]), |v| ClientServerSettingsPatch { sidebar_auto_settle_on_merge: Some(v), ..Default::default() }))
            .child(super::server_settings_controls::switch_row("sidebarAutoSettleAfterDays", t!("Auto-settle inactive threads"), Some(t!("Sidebar threads with no activity for this long settle automatically.")), days.is_some(), lock(&["sidebarAutoSettleAfterDays"]), move |checked, _, cx| {
                let value = checked.then_some(SidebarAutoSettleAfterDays::Number(AUTO_SETTLE_DEFAULT_DAYS));
                let patch = ClientServerSettingsPatch { sidebar_auto_settle_after_days: Some(value), ..Default::default() };
                save_patch(&inactive_model, "sidebarAutoSettleAfterDays", patch, cx);
            }));
        if let Some(days) = days {
            let days_field = self.fields.draft("autoSettleDays", format!("{}", days as u64), "", false, window, cx, move |text, _, cx| {
                if let Ok(d) = text.trim().parse::<u64>() {
                    let patch = ClientServerSettingsPatch { sidebar_auto_settle_after_days: Some(Some(SidebarAutoSettleAfterDays::Number(d.clamp(1, 3650) as f64))), ..Default::default() };
                    save_patch(&days_model, "sidebarAutoSettleAfterDays", patch, cx);
                }
            });
            auto_settle = auto_settle
                .child(SettingsRow::new(t!("Days of inactivity before auto-settle")).description(t!("Any new activity un-settles a thread automatically.")).control(div().w(px(96.)).child(field_input(&days_field, false, false, ro))))
                .child(bool_setting(model, "sidebarAutoSettleKeepPullRequests", t!("Keep threads with a PR"), Some(t!("Inactivity does not settle threads with a pull request. Merge and close still do.")), s.sidebar_auto_settle_keep_pull_requests.unwrap_or(false), lock(&["sidebarAutoSettleKeepPullRequests"]), |v| ClientServerSettingsPatch { sidebar_auto_settle_keep_pull_requests: Some(v), ..Default::default() }));
        }

        // Text generation.
        let instructions_model = model.clone();
        let instructions = self.fields.draft("threadTitleInstructions", s.thread_title_instructions.clone().unwrap_or_default(), t!("Keep titles to a few words. Lead with the main subject."), false, window, cx, move |text, _, cx| {
            save_patch(&instructions_model, "threadTitleInstructions", ClientServerSettingsPatch { thread_title_instructions: Some(text.trim().to_owned()), ..Default::default() }, cx);
        });
        let streaming = [ResponseStreamingMode::Turn, ResponseStreamingMode::Paragraph];
        let streaming_ix = streaming.iter().position(|m| Some(*m) == s.response_streaming_mode).unwrap_or(0);
        let streaming_model = model.clone();
        let this = cx.entity().downgrade();
        let advanced = advanced_fold(
            "text-generation-advanced",
            self.advanced_open,
            move |_, cx| {
                this.update(cx, |this, cx| {
                    this.advanced_open = !this.advanced_open;
                    cx.notify();
                })
                .ok();
            },
            vec![
                SettingsRow::new(t!("Thread title instructions"))
                    .description(t!("Appended to the prompt for new and regenerated thread titles."))
                    .control(div().w(theme.sp(64.)).child(field_input(&instructions, false, false, ro)))
                    .into_any_element(),
                select_row("responseStreamingMode", t!("Response streaming"), Some(if streaming_ix == 0 { t!("Text appears once the agent finishes its turn.") } else { t!("Each paragraph or code block appears as soon as it is complete.") }), vec![t!("Wait for the full response"), t!("Show finished paragraphs")], streaming_ix, lock(&["responseStreamingMode"]), move |ix, _, cx| {
                    save_patch(&streaming_model, "responseStreamingMode", ClientServerSettingsPatch { response_streaming_mode: Some(streaming[ix]), ..Default::default() }, cx);
                })
                .into_any_element(),
            ],
            cx,
        );
        let text_generation = SettingsSection::new(t!("Text generation"))
            .child(model_select_row(model, "textGenerationModelSelection", t!("Text generation model"), Some(t!("For thread titles and other generated text. Source control has its own setting. Text generators from Providers also appear here.")), s.text_generation_model_selection.as_ref(), None, lock(&["textGenerationModelSelection"]), |v| ClientServerSettingsPatch {
                text_generation_model_selection: v.and_then(|v| serde_json::from_value::<ClientServerSettingsPatchTextGenerationModelSelection>(serde_json::to_value(v).ok()?).ok()),
                ..Default::default()
            }, cx))
            .child(advanced);

        page_container(cx).child(new_projects).child(defaults).child(snooze).child(usage).child(auto_settle).child(text_generation)
    }
}
