//! Settings → Scheduled tasks. Ports `ScheduledTasksSettings.tsx`: the task
//! list (schedule, next run, enable switch, run now, delete) and the editor
//! with its four schedule kinds (fixed time with weekdays, once, interval,
//! webhook with signature, age limit, dedupe keys and filters), the
//! workspace, model and permissions. Validation and the saved schedule come
//! from `scheduled_tasks_settings_logic`.

use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::{AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{
    ModelSelection, OrchestrationV2ThreadLaunchWorkspaceStrategy as Strategy, OrchestrationV2ThreadLaunchWorkspaceStrategyRoot,
    OrchestrationV2ThreadLaunchWorkspaceStrategyWorktree, ProjectId, RuntimeMode, ScheduledTaskDeleteInput, ScheduledTaskId,
    ScheduledTaskRunNowInput, ScheduledTaskSetEnabledInput, ScheduledTaskThreadMode, ScheduledTaskUpsertInput,
    ScheduledTaskUpsertScheduleWebhookSignatureEncoding as Encoding,
};

use super::provider_settings_panel::toast_error;
use super::scheduled_tasks_settings_logic::{
    DraftState, FilterOp, ScheduleMode, WorkspaceMode, empty_webhook_filter_draft, remove_webhook_filter_draft,
    schedule_draft_errors, schedule_from_draft, schedule_label, task_to_draft,
};
use super::server_settings_controls::{Fields, field_input, hint, tinted};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::threads_settings::model_options;
use crate::components::worktree_base_branch_picker::{WorktreeBaseBranchEvent, WorktreeBaseBranchPicker};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::managed_policy::{ManagedCapability, managed_capability_denial};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{Modal, dialog_glass};
use crate::components::ui::select::{Select, SelectSize};
use crate::components::ui::input::{InputSize, TextArea};
use crate::components::ui::switch::Switch;
use crate::components::ui::toggle::{Toggle, ToggleSize, ToggleVariant};
use crate::i18n::t;
use crate::timestamp_format::{format_relative_time_label, now_ms};

/// The editor closes.
#[derive(Clone, Debug)]
pub enum ScheduledTaskEditorEvent {
    Close,
}

/// The task editor dialog.
pub struct ScheduledTaskEditor {
    model: Entity<ServerPanelsModel>,
    pub draft: DraftState,
    fields: Fields,
    /// The Prompt textarea, created on first render.
    prompt: Option<Entity<TextareaState>>,
    scroll: gpui_kit::ScrollHandle,
    /// Where the Schedule row sits among the form's rows, for [`Self::reveal_schedule`].
    schedule_row: usize,
    errors: Vec<SharedString>,
    saving: bool,
    /// The Base branch picker for the project it was made for.
    base_branch: Option<(String, Entity<WorktreeBaseBranchPicker>, Subscription)>,
}

impl EventEmitter<ScheduledTaskEditorEvent> for ScheduledTaskEditor {}

impl ScheduledTaskEditor {
    pub fn new(model: Entity<ServerPanelsModel>, draft: DraftState) -> Self {
        Self {
            model,
            draft,
            fields: Fields::default(),
            prompt: None,
            scroll: gpui_kit::ScrollHandle::new(),
            schedule_row: 0,
            errors: Vec::new(),
            saving: false,
            base_branch: None,
        }
    }

    /// `WorktreeBaseBranchPicker` over the picked project's checkout, made again when the
    /// project changes. A blank base ref shows the default branch (`scheduledTaskWorktreeBaseRef`).
    fn base_branch_picker(&mut self, cwd: Option<String>, cx: &mut Context<Self>) -> Entity<WorktreeBaseBranchPicker> {
        let key = self.draft.project_id.clone();
        if let Some((made_for, picker, _)) = &self.base_branch
            && *made_for == key
        {
            return picker.clone();
        }
        let source = self.model.read(cx).source().git_source();
        let value = super::scheduled_tasks_settings_logic::scheduled_task_worktree_base_ref(&self.draft.base_ref, &[]);
        let start_from_origin = self.draft.start_from_origin;
        let picker = cx.new(|cx| WorktreeBaseBranchPicker::new(source, cwd, value, start_from_origin, cx));
        let subscription = cx.subscribe(&picker, |this, _, event: &WorktreeBaseBranchEvent, cx| {
            match event {
                WorktreeBaseBranchEvent::Changed(branch) => this.draft.base_ref = branch.clone(),
                WorktreeBaseBranchEvent::StartFromOriginChanged(on) => this.draft.start_from_origin = *on,
            }
            cx.notify();
        });
        self.base_branch = Some((key, picker.clone(), subscription));
        picker
    }

    /// Scrolls the form so the Schedule row is in view.
    pub fn reveal_schedule(&self) {
        self.scroll.scroll_to_item(self.schedule_row);
    }

    /// Scrolls the form to its last row (the Enabled switch).
    pub fn reveal_end(&self) {
        self.scroll.scroll_to_item(self.schedule_row + 1);
    }

    /// Switches the schedule kind.
    pub fn set_mode(&mut self, mode: ScheduleMode, cx: &mut Context<Self>) {
        self.draft.schedule_mode = mode;
        self.errors.clear();
        cx.notify();
    }

    /// Reads the text fields back into the draft.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let text = |key: &str, cx: &Context<Self>, f: &Fields| f.value(key, cx).map(|v| v.to_string());
        let d = &mut self.draft;
        let f = &self.fields;
        if let Some(v) = text("title", cx, f) { d.title = v; }
        if let Some(v) = self.prompt.as_ref().map(|p| p.read(cx).value().to_string()) { d.prompt = v; }
        if let Some(v) = text("checkoutPath", cx, f) { d.existing_worktree_path = v; }
        // The field shows the time as a native time input does ("09:00 AM" on a 12-hour clock).
        if let Some((hour, minute)) = text("time", cx, f).as_deref().and_then(super::threads_settings_snooze::parse_time) {
            d.time_of_day = format!("{hour:02}:{minute:02}");
        }
        if let Some(v) = text("runAt", cx, f) { d.run_at = v; }
        if let Some(v) = text("interval", cx, f) { d.interval_minutes = v; }
        if let Some(v) = text("sigHeader", cx, f) { d.signature_header = v; }
        if let Some(v) = text("sigPrefix", cx, f) { d.signature_prefix = v; }
        if let Some(v) = text("sigSecret", cx, f) { d.signature_secret = v; }
        if let Some(v) = text("maxAge", cx, f) { d.max_delivery_age_minutes = v; }
        if let Some(v) = text("replayKey", cx, f) { d.replay_key = v; }
        if let Some(v) = text("coalesceKey", cx, f) { d.coalesce_key = v; }
        if let Some(v) = text("coalesceWindow", cx, f) { d.coalesce_window_seconds = v; }
        for filter in &mut d.filters {
            if let Some(v) = f.value(&format!("filter:{}:path", filter.id), cx) { filter.path = v.to_string(); }
            if let Some(v) = f.value(&format!("filter:{}:value", filter.id), cx) { filter.value = v.to_string(); }
        }
    }

    /// Validates and saves (`scheduledTasks.upsert`).
    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.sync(cx);
        let errors = schedule_draft_errors(&self.draft);
        let mut messages: Vec<SharedString> = errors.iter().map(|e| SharedString::from(format!("{} {}", e.title, e.description))).collect();
        if self.draft.title.trim().is_empty() {
            messages.push(t!("Add a title."));
        }
        if self.draft.prompt.trim().is_empty() {
            messages.push(t!("Add a prompt."));
        }
        if let Err(e) = super::scheduled_tasks_settings_logic::parse_webhook_filter_drafts(&self.draft.filters)
            && self.draft.schedule_mode == ScheduleMode::Webhook
        {
            messages.push(e);
        }
        let model_selection = self.draft.base_model_selection.clone().or_else(|| {
            let (instance, model) = self.draft.model_key.split_once(':')?;
            Some(ModelSelection { instance_id: instance.into(), model: model.into(), options: None })
        });
        if model_selection.is_none() {
            messages.push(t!("Pick a model."));
        }
        self.errors = messages;
        let schedule = schedule_from_draft(&self.draft);
        let (Some(schedule), Some(model_selection), true) = (schedule, model_selection, self.errors.is_empty()) else {
            cx.notify();
            return;
        };
        let strategy = match self.draft.workspace_mode {
            WorkspaceMode::Root => Strategy::Root(OrchestrationV2ThreadLaunchWorkspaceStrategyRoot { branch: None }),
            _ => Strategy::Worktree(OrchestrationV2ThreadLaunchWorkspaceStrategyWorktree {
                base_ref: self.draft.base_ref.trim().to_owned(),
                branch: None,
                start_from_origin: Some(self.draft.start_from_origin),
            }),
        };
        let input = ScheduledTaskUpsertInput {
            id: self.draft.editing_id.clone().map(ScheduledTaskId::new),
            require_existing: self.draft.editing_id.as_ref().map(|_| true),
            command_id: None,
            title: self.draft.title.trim().to_owned(),
            prompt: self.draft.prompt.trim().to_owned(),
            enabled: self.draft.enabled,
            schedule,
            project_id: ProjectId::new(self.draft.project_id.as_str()),
            thread_id: None,
            thread_mode: Some(self.draft.thread_mode),
            workspace_strategy: strategy,
            model_selection,
            runtime_mode: self.draft.runtime_mode,
            interaction_mode: self.draft.interaction_mode,
            created_by: None,
            creation_source: Some(t3code_contracts::OrchestrationV2CreationSource::Web),
        };
        self.saving = true;
        let this = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.run("scheduled-task-save", move |s| s.upsert_scheduled_task(input), move |result, cx| {
                this.update(cx, |this, cx| {
                    this.saving = false;
                    match result {
                        Ok(_) => cx.emit(ScheduledTaskEditorEvent::Close),
                        Err(e) => {
                            toast_error(t!("Could not save scheduled task"), e.clone(), cx);
                            this.errors = vec![e.message];
                        }
                    }
                    cx.notify();
                })
                .ok();
            }, cx)
        });
        cx.notify();
    }

    /// `Field`: its `Label` over the control, `space-y-1.5`.
    fn labeled(&self, label: SharedString, control: impl IntoElement, cx: &Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        div().flex().flex_col().gap(theme.sp(1.5)).child(field_label(label, cx)).child(control).into_any_element()
    }

    /// A `Field` around a `Select` or `Combobox`. Their hidden form input is the field's last
    /// child, so Tailwind's `space-y-1.5` (a bottom margin on every child but the last) leaves
    /// the same gap under the trigger.
    fn labeled_select(&self, label: SharedString, control: impl IntoElement, cx: &Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        div().pb(theme.sp(1.5)).child(self.labeled(label, control, cx)).into_any_element()
    }

    fn input(&mut self, key: &str, value: String, placeholder: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        self.fields.get(key.to_owned(), value, placeholder, key == "sigSecret", window, cx)
    }

    fn render_schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let d = self.draft.clone();
        match d.schedule_mode {
            ScheduleMode::Fixed => {
                let hour12 = crate::timestamp_format::resolve_hour12(crate::hooks::settings_bridge::timestamp_format(cx));
                let time = self.input("time", super::threads_settings_snooze::time_field_text(&d.time_of_day, hour12), "", window, cx);
                let zone = self.model.read(cx).snapshot().scheduled_task_time_zone.clone();
                let zone = super::scheduled_tasks_settings_logic::schedule_time_zone_label(zone.as_deref(), None).unwrap_or_default();
                let days = div().flex().children([1u64, 2, 3, 4, 5, 6, 0].into_iter().map(|day| {
                    let on = d.weekdays.contains(&day);
                    Toggle::new(SharedString::from(format!("weekday-{day}")))
                        .label(two_letter_weekday(day))
                        .variant(ToggleVariant::Outline)
                        .size(ToggleSize::Sm)
                        .pressed(on)
                        .on_toggle({
                            let this = cx.entity().downgrade();
                            move |pressed, _, cx| {
                                this.update(cx, |this, cx| {
                                    if pressed { this.draft.weekdays.insert(day); } else { this.draft.weekdays.remove(&day); }
                                    cx.notify();
                                }).ok();
                            }
                        })
                }));
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme.sp(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .child(field_label(t!("Run at"), cx))
                            .child(div().w(theme.sp(32.)).child(field_input(&time, false, false, false).size(InputSize::Default).trailing_icon(IconName::Clock)))
                            .child(hint(zone, cx))
                            .child(hint(t!("on"), cx)),
                    )
                    .child(days)
                    .into_any_element()
            }
            ScheduleMode::Once => {
                let run_at = self.input("runAt", d.run_at.clone(), "2026-11-24T09:00", window, cx);
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(field_label(t!("Run on"), cx))
                    .child(div().w(theme.sp(56.)).child(field_input(&run_at, false, false, false).size(InputSize::Default)))
                    .into_any_element()
            }
            ScheduleMode::Interval => {
                let interval = self.input("interval", d.interval_minutes.clone(), "15", window, cx);
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(field_label(t!("Run every"), cx))
                    .child(div().w(theme.sp(24.)).child(field_input(&interval, false, false, false).size(InputSize::Default)))
                    .child(hint(t!("minutes"), cx))
                    .into_any_element()
            }
            ScheduleMode::Webhook => {
                let header = self.input("sigHeader", d.signature_header.clone(), "x-hub-signature-256", window, cx);
                let prefix = self.input("sigPrefix", d.signature_prefix.clone(), "sha256=", window, cx);
                let secret = self.input("sigSecret", String::new(), t!("Leave empty to keep the saved secret"), window, cx);
                let max_age = self.input("maxAge", d.max_delivery_age_minutes.clone(), t!("No limit"), window, cx);
                let replay = self.input("replayKey", d.replay_key.clone(), "{{headers.x-github-delivery}}", window, cx);
                let coalesce = self.input("coalesceKey", d.coalesce_key.clone(), "{{body.pull_request.number}}", window, cx);
                let window_secs = self.input("coalesceWindow", d.coalesce_window_seconds.clone(), "120", window, cx);
                let mut filters = div().flex().flex_col().gap(theme.sp(2.));
                for filter in d.filters.clone() {
                    let path = self.input(&format!("filter:{}:path", filter.id), filter.path.clone(), "body.action", window, cx);
                    let value = self.input(&format!("filter:{}:value", filter.id), filter.value.clone(), t!("value"), window, cx);
                    let id = filter.id;
                    let op_ix = FilterOp::ALL.iter().position(|o| *o == filter.op).unwrap_or(0);
                    let this = cx.entity().downgrade();
                    filters = filters.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .child(div().w(theme.sp(36.)).child(field_input(&path, true, false, false)))
                            .child(div().flex_none().w(theme.sp(40.)).child(Select::new(SharedString::from(format!("filter-op-{id}")), FilterOp::ALL.iter().map(|o| o.label()).collect::<Vec<_>>(), op_ix, move |ix, _, cx| {
                                this.update(cx, |this, cx| {
                                    if let Some(f) = this.draft.filters.iter_mut().find(|f| f.id == id) { f.op = FilterOp::ALL[ix]; }
                                    cx.notify();
                                }).ok();
                            }).size(SelectSize::Sm)))
                            .when(filter.op != FilterOp::Exists, |r| r.child(div().flex_1().child(field_input(&value, true, false, false))))
                            .child(Button::new(SharedString::from(format!("filter-remove-{id}"))).variant(ButtonVariant::GhostMuted).size(ButtonSize::IconXs).icon(IconName::X).on_click(cx.listener(move |this, _, _, cx| {
                                this.draft.filters = remove_webhook_filter_draft(&this.draft.filters, id);
                                cx.notify();
                            }))),
                    );
                }
                let encodings = [Encoding::Hex, Encoding::Base64];
                let enc_ix = encodings.iter().position(|e| *e == d.signature_encoding).unwrap_or(0);
                let this = cx.entity().downgrade();
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(3.))
                    .child(hint(t!("Runs once per request to the task's webhook URL."), cx))
                    .child(div().flex().items_center().gap(theme.sp(2.)).child(Switch::new("webhook-signature", d.signature_enabled).on_change({
                        let this = cx.entity().downgrade();
                        move |v, _, cx| {
                            this.update(cx, |this, cx| {
                                this.draft.signature_enabled = v;
                                cx.notify();
                            }).ok();
                        }
                    })).child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).child(t!("Verify a signature header"))))
                    .when(d.signature_enabled, |b| {
                        b.child(
                            div()
                                .flex()
                                .gap(theme.sp(2.))
                                .child(div().flex_1().child(field_input(&header, true, false, false)))
                                .child(div().flex_none().w(theme.sp(32.)).child(Select::new("sig-encoding", vec!["hex".into(), "base64".into()], enc_ix, move |ix, _, cx| {
                                    this.update(cx, |this, cx| {
                                        this.draft.signature_encoding = encodings[ix];
                                        cx.notify();
                                    }).ok();
                                }).size(SelectSize::Sm)))
                                .child(div().flex_none().w(theme.sp(28.)).child(field_input(&prefix, true, false, false))),
                        )
                        .child(field_input(&secret, true, false, false))
                    })
                    .child(self.labeled(t!("Skip requests older than (minutes)"), div().w(theme.sp(28.)).child(field_input(&max_age, true, false, false)), cx))
                    .child(div().flex().gap(theme.sp(2.)).child(div().flex_1().child(self.labeled(t!("Duplicate request key"), field_input(&replay, true, false, false), cx))).child(div().flex_1().child(self.labeled(t!("Coalesce key"), field_input(&coalesce, true, false, false), cx))).child(div().w(theme.sp(24.)).child(self.labeled(t!("Window (s)"), field_input(&window_secs, true, false, false), cx))))
                    .child(self.labeled(t!("Filters"), filters, cx))
                    .child(div().flex().child(Button::new("filter-add").variant(ButtonVariant::Outline).size(ButtonSize::Xs).icon(IconName::Plus).label(t!("Add filter")).on_click(cx.listener(|this, _, _, cx| {
                        this.sync(cx);
                        this.draft.filters.push(empty_webhook_filter_draft());
                        cx.notify();
                    }))))
                    .into_any_element()
            }
        }
    }
}

impl Render for ScheduledTaskEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let d = self.draft.clone();
        let snapshot = self.model.read(cx).snapshot().clone();
        let title = self.input("title", d.title.clone(), t!("e.g. Check for Sentry issues"), window, cx);
        let prompt = self
            .prompt
            .get_or_insert_with(|| {
                let initial = d.prompt.clone();
                cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 12).placeholder(t!("What should the agent do each time this runs?")).default_value(initial))
            })
            .clone();
        let webhook_denied = managed_capability_denial(snapshot.managed_policy.as_deref(), ManagedCapability::WebhookTasks);
        let modes = [(ScheduleMode::Fixed, t!("At a time")), (ScheduleMode::Once, t!("Once")), (ScheduleMode::Interval, t!("Every interval")), (ScheduleMode::Webhook, t!("On webhook"))];
        // `ToggleGroup` (segmented): `gap-0.5 rounded-lg bg-input/40 p-0.5`.
        let kinds = div().flex().gap(theme.sp(0.5)).p(theme.sp(0.5)).rounded(theme.radii.lg).bg(crate::theme_color::with_alpha(c.input, 0.4)).children(modes.iter().map(|(mode, label)| {
            let mode = *mode;
            Toggle::new(SharedString::from(format!("schedule-kind-{mode:?}")))
                .label(label.clone())
                .variant(ToggleVariant::Segmented)
                .size(ToggleSize::Segmented)
                .pressed(d.schedule_mode == mode)
                .disabled(mode == ScheduleMode::Webhook && webhook_denied.is_some() && d.schedule_mode != ScheduleMode::Webhook)
                .on_toggle({
                    let this = cx.entity().downgrade();
                    move |_, _, cx| {
                        this.update(cx, |this, cx| this.set_mode(mode, cx)).ok();
                    }
                })
        }));
        let environment_id = t3code_client_runtime::environment::EnvironmentId::new(snapshot.environment_id.to_string());
        let runs_on = Select::new("task-environment", vec![SharedString::from(snapshot.environment_label.clone())], 0, |_, _, _| {})
            .size(SelectSize::Sm)
            .leading(super::settings_controls::environment_machine_icon(
                super::settings_controls::environment_machine_kind(&environment_id, cx),
                theme.sp(4.),
                c.foreground(),
            ));
        let thread_modes = [
            (ScheduledTaskThreadMode::NewThread, t!("New thread each run"), t!("Each run starts a new thread with the workspace and model below.")),
            (ScheduledTaskThreadMode::Continue, t!("One thread, continuing the conversation"), t!("Each run continues this conversation, in its workspace and with its current model.")),
            (ScheduledTaskThreadMode::FreshSession, t!("One thread, fresh session each run"), t!("Each run starts a fresh session in this thread, without earlier runs' context.")),
        ];
        let thread_ix = thread_modes.iter().position(|(m, _, _)| *m == d.thread_mode).unwrap_or(0);
        let this = cx.entity().downgrade();
        let destination = Select::new("task-destination", thread_modes.iter().map(|(_, l, _)| l.clone()).collect::<Vec<_>>(), thread_ix, move |ix, _, cx| {
            this.update(cx, |this, cx| {
                this.draft.thread_mode = [ScheduledTaskThreadMode::NewThread, ScheduledTaskThreadMode::Continue, ScheduledTaskThreadMode::FreshSession][ix];
                cx.notify();
            }).ok();
        }).size(SelectSize::Sm);
        let projects: Vec<_> = snapshot.projects.iter().filter(|p| p.archived_at.is_none()).cloned().collect();
        let project_ix = projects.iter().position(|p| p.id.to_string() == d.project_id).unwrap_or(0);
        let project_ids: Vec<String> = projects.iter().map(|p| p.id.to_string()).collect();
        let this = cx.entity().downgrade();
        let project_select = Select::new("task-project", projects.iter().map(|p| SharedString::from(p.title.clone())).collect::<Vec<_>>(), project_ix, move |ix, _, cx| {
            let id = project_ids[ix].clone();
            this.update(cx, |this, cx| {
                this.draft = super::scheduled_tasks_settings_logic::change_draft_project(&this.draft, &id);
                cx.notify();
            }).ok();
        }).size(SelectSize::Sm);
        let workspaces = [WorkspaceMode::Worktree, WorkspaceMode::Root, WorkspaceMode::ExistingWorktree];
        let workspace_ix = workspaces.iter().position(|w| *w == d.workspace_mode).unwrap_or(0);
        let this = cx.entity().downgrade();
        let workspace = Select::new("task-workspace", vec![t!("Create a new worktree"), t!("Use the project checkout"), t!("Use a specific checkout")], workspace_ix, move |ix, _, cx| {
            this.update(cx, |this, cx| {
                this.draft.workspace_mode = workspaces[ix];
                cx.notify();
            }).ok();
        }).size(SelectSize::Sm);
        // `ProviderModelPicker` with the settings trigger: the saved model, else the first one offered.
        let options = model_options(self.model.read(cx));
        let shown = options.iter().find(|(s, _)| format!("{}:{}", s.instance_id, s.model) == d.model_key).or(options.first()).map(|(s, _)| s.clone());
        let catalog = {
            let panels = self.model.read(cx).snapshot();
            std::sync::Arc::new(crate::provider_models_live::model_catalog_for(panels.providers.iter().map(|p| p.as_ref()), &panels.settings))
        };
        let this = cx.entity().downgrade();
        let model_select = div().flex().child(super::settings_model_picker::SettingsModelPicker::new("task-model", catalog, shown, move |selection, cx| {
            this.update(cx, |this, cx| {
                this.draft.model_key = format!("{}:{}", selection.instance_id, selection.model);
                this.draft.base_model_selection = Some(selection);
                cx.notify();
            })
            .ok();
        }));
        let schedule = self.render_schedule(window, cx);
        let new_thread = d.thread_mode == ScheduledTaskThreadMode::NewThread;
        let mut body: Vec<AnyElement> = vec![
            self.labeled_select(t!("Runs on"), runs_on, cx),
            self.labeled(t!("Name"), field_input(&title, false, false, false).size(InputSize::Default), cx),
            self.labeled(t!("Destination"), div().flex().flex_col().gap(theme.sp(1.5)).child(destination).child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(thread_modes[thread_ix].2.clone())), cx),
        ];
        if new_thread {
            body.push(
                div()
                    .flex()
                    .gap(theme.sp(3.))
                    .child(div().flex_1().min_w_0().child(self.labeled_select(t!("Project"), project_select, cx)))
                    .child(div().flex_1().min_w_0().child(self.labeled_select(t!("Workspace"), workspace, cx)))
                    .into_any_element(),
            );
            match d.workspace_mode {
                WorkspaceMode::Worktree => {
                    let cwd = projects.get(project_ix).map(|p| p.workspace_root.clone());
                    let base = self.base_branch_picker(cwd, cx);
                    body.push(self.labeled_select(t!("Base branch"), base, cx));
                }
                WorkspaceMode::ExistingWorktree => {
                    let path = self.input("checkoutPath", d.existing_worktree_path.clone(), "", window, cx);
                    body.push(self.labeled(t!("Checkout path"), field_input(&path, false, false, false).size(InputSize::Default), cx));
                }
                WorkspaceMode::Root => {}
            }
        }
        body.push(self.labeled(t!("Prompt"), TextArea::new(&prompt), cx));
        if new_thread {
            body.push(self.labeled(t!("Model"), model_select, cx));
        }
        let enabled = d.enabled;
        self.schedule_row = body.len();
        body.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(3.))
                    .child(div().flex().flex_wrap().items_center().justify_between().gap(theme.sp(2.)).child(field_label(t!("Schedule"), cx)).child(kinds))
                    .children(webhook_denied.map(|r| hint(r, cx)))
                    .child(schedule)
                    .into_any_element(),
        );
        body.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.sp(4.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.sp(1.))
                            // An inline-flex `Label` in a block: it sits on the dialog text's baseline
                            // (15px on a 1.5rem line), about 1.25 spacing units below the block top.
                            .child(field_label(t!("Enabled"), cx).mt(theme.sp(1.25)))
                            .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(t!("Disabled tasks stay saved but do not run."))),
                    )
                    .child(Switch::new("task-enabled", enabled).on_change({
                        let this = cx.entity().downgrade();
                        move |v, _, cx| {
                            this.update(cx, |this, cx| {
                                this.draft.enabled = v;
                                cx.notify();
                            }).ok();
                        }
                    }))
                    .into_any_element(),
        );
        body.extend(self.errors.iter().map(|e| tinted(e.clone(), c.destructive(), cx)));
        let editing = d.editing_id.is_some();
        // `DialogPopup max-h-full`: the dialog stops at the viewport's padding and the panel scrolls.
        let max_height = window.viewport_size().height - theme.sp(6.) * 2.;
        dialog_glass(theme)
            .id("scheduled-task-editor")
            .relative()
            .flex()
            .flex_col()
            .max_h(max_height)
            .w(theme.rem(36.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(2.))
                    .p(theme.sp(6.))
                    .pb(theme.sp(4.))
                    .child(
                        div()
                            .ui_text(theme, TextSize::Xl, weight::SEMIBOLD)
                            .line_height(px(TextSize::Xl.size(theme.fonts.interface) * 1.375))
                            .child(if editing { t!("Edit task") } else { t!("New task") }),
                    )
                    .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(t!("Run a prompt automatically on an interval, at a fixed time, once at a set date, or when a webhook is called."))),
            )
            .child(
                div()
                    .absolute()
                    .top(theme.sp(2.))
                    .right(theme.sp(2.))
                    .child(Button::new("task-close").variant(ButtonVariant::Ghost).size(ButtonSize::IconSm).icon(IconName::X).on_click(cx.listener(|_, _, _, cx| cx.emit(ScheduledTaskEditorEvent::Close)))),
            )
            .child(
                div()
                    .id("scheduled-task-editor-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .gap(theme.sp(5.))
                    .px(theme.sp(6.))
                    .pb(theme.sp(6.))
                    .children(body),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.sp(2.))
                    .px(theme.sp(6.))
                    .py(theme.sp(4.))
                    .border_t_1()
                    .border_color(c.border)
                    .bg(theme.derived.dialog_footer)
                    .rounded_b(theme.radii.xxl - px(1.))
                    .child(Button::new("task-cancel").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Cancel")).on_click(cx.listener(|_, _, _, cx| cx.emit(ScheduledTaskEditorEvent::Close))))
                    .child(Button::new("task-save").size(ButtonSize::Sm).label(if editing { t!("Save task") } else { t!("Create task") }).disabled(self.saving).on_click(cx.listener(|this, _, _, cx| this.save(cx)))),
            )
    }
}

/// The Scheduled tasks page.
pub struct ScheduledTasksSettingsPanel {
    model: Entity<ServerPanelsModel>,
    editor: Option<Entity<ScheduledTaskEditor>>,
    _subscriptions: Vec<Subscription>,
}

impl ScheduledTasksSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, editor: None, _subscriptions: vec![observe] }
    }

    /// Opens the editor for a new task, or for the task with `id`.
    pub fn open_editor(&mut self, id: Option<&str>, cx: &mut Context<Self>) {
        let snapshot = self.model.read(cx).snapshot().clone();
        let draft = match id.and_then(|id| snapshot.scheduled_tasks.iter().find(|t| t.id.as_str() == id)) {
            Some(task) => task_to_draft(task),
            None => {
                let project = snapshot.projects.iter().find(|p| p.archived_at.is_none() && p.pinned_at.is_some()).or_else(|| snapshot.projects.iter().find(|p| p.archived_at.is_none()));
                let mut draft = DraftState::new_task(project.map(|p| p.id.to_string()).unwrap_or_default());
                if let Some((selection, _)) = model_options(self.model.read(cx)).into_iter().next() {
                    draft.model_key = format!("{}:{}", selection.instance_id, selection.model);
                    draft.base_model_selection = Some(selection);
                }
                draft
            }
        };
        let model = self.model.clone();
        let editor = cx.new(|_| ScheduledTaskEditor::new(model, draft));
        let sub = cx.subscribe(&editor, |this, _, _: &ScheduledTaskEditorEvent, cx| {
            this.editor = None;
            cx.notify();
        });
        self._subscriptions.push(sub);
        self.editor = Some(editor);
        cx.notify();
    }

    pub fn editor(&self) -> Option<&Entity<ScheduledTaskEditor>> {
        self.editor.as_ref()
    }
}

impl Render for ScheduledTasksSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let snapshot = self.model.read(cx).snapshot().clone();
        let ro = snapshot.read_only;
        let zone = snapshot.scheduled_task_time_zone.as_ref().map(|z| z.to_string());
        let mut list = SettingsSection::new(t!("Scheduled tasks")).header_action(
            Button::new("task-new").variant(ButtonVariant::GhostMuted).size(ButtonSize::Xs).icon(IconName::Plus).label(t!("New task")).disabled(ro).on_click(cx.listener(|this, _, _, cx| this.open_editor(None, cx))),
        );
        if snapshot.scheduled_tasks.is_empty() {
            list.extend([SettingsRow::new(t!("No scheduled tasks")).description(t!("No tasks match this environment and project selection.")).into_any_element()]);
        }
        for task in snapshot.scheduled_tasks.iter() {
            let id = task.id.to_string();
            let label = schedule_label(&task.schedule, zone.as_deref(), task.next_run_at.as_deref());
            let next: SharedString = match (&task.blocked_by_policy, &task.next_run_at) {
                (Some(reason), _) => reason.clone().into(),
                (None, Some(next)) => format!("{label} · {}", t!("Next {when}", when = format_relative_time_label(next, now_ms()))).into(),
                (None, None) => label,
            };
            let (toggle_model, run_model, delete_model) = (self.model.clone(), self.model.clone(), self.model.clone());
            let (toggle_id, run_id, delete_id, edit_id) = (id.clone(), id.clone(), id.clone(), id.clone());
            list.extend([SettingsRow::new(SharedString::from(task.title.clone()))
                .description(next)
                .control(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.sp(1.))
                        .child(Button::new(SharedString::from(format!("task-run-{id}"))).variant(ButtonVariant::GhostMuted).size(ButtonSize::IconXs).icon(IconName::Play).disabled(ro).on_click(move |_, _, cx| {
                            let input = ScheduledTaskRunNowInput { id: ScheduledTaskId::new(run_id.as_str()) };
                            run_model.update(cx, |m, cx| m.run("task-run", move |s| s.run_scheduled_task_now(input), |r, cx| if let Err(e) = r { toast_error(t!("Could not run scheduled task"), e, cx) }, cx));
                        }))
                        .child(Button::new(SharedString::from(format!("task-edit-{id}"))).variant(ButtonVariant::GhostMuted).size(ButtonSize::IconXs).icon(IconName::Pencil).disabled(ro).on_click(cx.listener(move |this, _, _, cx| this.open_editor(Some(&edit_id), cx))))
                        .child(Button::new(SharedString::from(format!("task-delete-{id}"))).variant(ButtonVariant::GhostDestructive).size(ButtonSize::IconXs).icon(IconName::Trash2).disabled(ro).on_click(move |_, _, cx| {
                            let input = ScheduledTaskDeleteInput { id: ScheduledTaskId::new(delete_id.as_str()) };
                            delete_model.update(cx, |m, cx| m.run("task-delete", move |s| s.delete_scheduled_task(input), |r, cx| if let Err(e) = r { toast_error(t!("Could not delete scheduled task"), e, cx) }, cx));
                        }))
                        .child(Switch::new(SharedString::from(format!("task-enabled-{id}")), task.enabled).disabled(ro).on_change(move |enabled, _, cx| {
                            let input = ScheduledTaskSetEnabledInput { id: ScheduledTaskId::new(toggle_id.as_str()), enabled };
                            toggle_model.update(cx, |m, cx| m.run("task-enabled", move |s| s.set_scheduled_task_enabled(input), |r, cx| if let Err(e) = r { toast_error(t!("Could not update scheduled task"), e, cx) }, cx));
                        })),
                )
                .into_any_element()]);
        }
        let editor = self.editor.clone().map(|editor| {
            let this = cx.entity().downgrade();
            Modal::dialog("scheduled-task-editor-modal", editor, move |_, cx| {
                this.update(cx, |this, cx| {
                    this.editor = None;
                    cx.notify();
                }).ok();
            })
        });
        div().relative().size_full().child(page_container(cx).child(list)).children(editor)
    }
}

/// `WEEKDAY_SHORT`: the weekday buttons' two-letter names, Sunday 0.
fn two_letter_weekday(day: u64) -> SharedString {
    match day {
        0 => t!("Su", context = "two-letter Sunday"),
        1 => t!("Mo", context = "two-letter Monday"),
        2 => t!("Tu", context = "two-letter Tuesday"),
        3 => t!("We", context = "two-letter Wednesday"),
        4 => t!("Th", context = "two-letter Thursday"),
        5 => t!("Fr", context = "two-letter Friday"),
        _ => t!("Sa", context = "two-letter Saturday"),
    }
}

/// `Label`: `sm:text-sm/4 font-medium`, a 1rem line.
fn field_label(label: SharedString, cx: &App) -> Div {
    let theme = cx.app_theme();
    div().ui_text(theme, TextSize::Sm, weight::MEDIUM).line_height(theme.rem(1.)).child(label)
}
