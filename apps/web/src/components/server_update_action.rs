//! The server Update controls and their progress. Ports `ServerUpdateAction.tsx`
//! (`ServerUpdateAction`, `ServerUpdatesAction`, `OutdatedServerUpdateAction`,
//! `ServerUpdateProgress`, `useServerUpdate`) and `ComposerServerUpdateStatus.tsx`.
//!
//! [`ServerUpdateStore`] is the GPUI global over the runtime's [`ServerUpdates`]: it starts
//! updates on tokio, mirrors each environment's [`ServerUpdateState`] into an entity views
//! observe, and caches whether each session may maintain its environment. The controls are
//! plain elements; a desktop-managed host needs a confirmation, which the caller shows and
//! then passes to [`ServerUpdateStore::update`].

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt as _;
use gpui_kit::{TestSupportExt as _, AnyElement, App, AppContext as _, ClipboardItem, Entity, Global, IntoElement, ParentElement as _, SharedString, Styled as _, Task, Window, div, prelude::*};
use t3code_client_runtime::connection::server_update::{
    ServerUpdateFailure, ServerUpdateOutcome, ServerUpdateStage, ServerUpdateState, ServerUpdates, update_server,
};
use t3code_client_runtime::connection::{ConnectionPhase, outdated_host_update};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::{
    AuthEnvironmentScope, ExecutionEnvironmentCapabilitiesServerInstallation as Installation, ServerSelfUpdateCapability,
    ServerSelfUpdateInput, ServerSelfUpdateOutcomeStatus,
};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::{TooltipAnchor, TooltipSide};
use crate::connection::ConnectionRuntime;
use crate::i18n::t;
use crate::version_skew::{manual_command_installs, manual_server_update_command, updates_remotely};

/// One server an Update control acts on (`ServerUpdateTarget`).
#[derive(Clone, Debug)]
pub struct ServerUpdateTarget {
    pub environment_id: EnvironmentId,
    /// Names the server inside sentences, e.g. "Studio server".
    pub server_label: SharedString,
    pub self_update: Option<ServerSelfUpdateCapability>,
    pub installation: Option<Installation>,
    pub desktop_app_update: bool,
    pub target_version: SharedString,
    /// The host's version now, to refuse a Stable/Nightly crossing.
    pub server_version: Option<SharedString>,
    /// Why the host's managed policy refuses updates.
    pub updates_denial: Option<SharedString>,
}

impl ServerUpdateTarget {
    /// The target for an environment's config, updating it to this client's version.
    pub fn for_config(
        environment_id: EnvironmentId,
        server_label: SharedString,
        config: Option<&t3code_contracts::ServerConfig>,
        target_version: SharedString,
    ) -> Self {
        Self {
            environment_id,
            server_label,
            self_update: crate::version_skew::self_update_capability(config),
            installation: config.and_then(|c| c.environment.capabilities.server_installation.clone()),
            desktop_app_update: crate::version_skew::supports_desktop_app_update(config),
            target_version,
            server_version: config.map(|c| c.environment.server_version.clone().into()),
            updates_denial: crate::version_skew::managed_updates_denial(config),
        }
    }

    /// Whether the client can start this update (`ServerUpdatesAction`'s `eligible`).
    pub fn updates_remotely(&self) -> bool {
        updates_remotely(self.self_update.as_ref(), self.desktop_app_update) && self.updates_denial.is_none() && !self.refused_crossing()
    }

    /// Hosts refuse a Stable/Nightly crossing outside a track switch (`refusedCrossing`).
    pub fn refused_crossing(&self) -> bool {
        self.self_update.is_some()
            && self.server_version.as_ref().is_some_and(|from| crate::version_skew::crosses_release_track(from, &self.target_version))
    }

    /// Whether starting it asks first: it relaunches the host's desktop app.
    pub fn needs_confirmation(&self) -> bool {
        self.self_update == Some(ServerSelfUpdateCapability::DesktopManaged)
    }

    /// The manual command when the host cannot update itself.
    fn manual_command(&self) -> Option<String> {
        self.self_update.is_none().then(|| manual_server_update_command(&self.target_version, self.installation.as_ref()))
    }
}

/// Every environment's update state as views read it, plus each session's maintain permission.
#[derive(Default)]
pub struct ServerUpdateStates {
    states: HashMap<EnvironmentId, ServerUpdateState>,
    /// The connection generation each answer belongs to; `None` while the check runs.
    can_maintain: HashMap<EnvironmentId, (u64, Option<bool>)>,
}

/// The GPUI side of server updates. See the module docs.
pub struct ServerUpdateStore {
    updates: Arc<ServerUpdates>,
    states: Entity<ServerUpdateStates>,
}

impl Global for ServerUpdateStore {}

impl ServerUpdateStore {
    fn global(cx: &mut App) -> &ServerUpdateStore {
        if !cx.has_global::<Self>() {
            let states = cx.new(|_| ServerUpdateStates::default());
            cx.set_global(Self { updates: ServerUpdates::new(), states });
        }
        cx.global::<Self>()
    }

    /// The entity to observe for state changes.
    pub fn entity(cx: &mut App) -> Entity<ServerUpdateStates> {
        Self::global(cx).states.clone()
    }

    /// The environment's update state.
    pub fn state(environment_id: &EnvironmentId, cx: &App) -> ServerUpdateState {
        cx.try_global::<Self>()
            .and_then(|store| store.states.read(cx).states.get(environment_id).cloned())
            .unwrap_or_default()
    }

    /// The state as views show it for a host on `server_version` (`updateStateAtom`).
    pub fn shown_state(environment_id: &EnvironmentId, server_version: Option<&str>, cx: &App) -> ServerUpdateState {
        Self::state(environment_id, cx).for_server_version(server_version)
    }

    /// Whether this session may maintain the environment (`canUpdateServer`). Starts the check
    /// for a new connection and answers `false` until it returns.
    pub fn can_maintain(environment_id: &EnvironmentId, cx: &mut App) -> bool {
        let connection = ConnectionRuntime::is_running(cx)
            .then(|| ConnectionRuntime::state(cx).read(cx).environment(environment_id).and_then(|env| env.status.clone()))
            .flatten();
        let Some(status) = connection.filter(|status| status.phase == ConnectionPhase::Connected) else { return false };
        let states = Self::entity(cx);
        if let Some((generation, answer)) = states.read(cx).can_maintain.get(environment_id)
            && *generation == status.generation
        {
            return answer.unwrap_or(false);
        }
        let Some(link) = ConnectionRuntime::global(cx).environment_link(environment_id) else { return false };
        states.update(cx, |states, _| states.can_maintain.insert(environment_id.clone(), (status.generation, None)));
        let check = ConnectionRuntime::global(cx).spawn(
            async move { link.session_state().await.map_err(|e| crate::connection::CommandError(e.to_string().into())) },
            cx,
        );
        let id = environment_id.clone();
        cx.spawn(async move |cx| {
            let can = check.await.is_ok_and(|state| state.grants_scope(AuthEnvironmentScope::EnvironmentMaintain));
            cx.update(|cx| {
                states.update(cx, |states, cx| {
                    states.can_maintain.insert(id, (status.generation, Some(can)));
                    cx.notify();
                })
            });
        })
        .detach();
        false
    }

    /// Whether an update of the environment is already in flight (`pendingUpdateEnvironmentIds`).
    pub fn is_pending(environment_id: &EnvironmentId, cx: &mut App) -> bool {
        Self::global(cx).updates.is_pending(environment_id)
    }

    /// Starts `target`'s update (`useServerUpdate`). Toasts its success or failure unless `quiet`.
    pub fn update(target: ServerUpdateTarget, quiet: bool, cx: &mut App) -> Task<ServerUpdateOutcome> {
        if !ConnectionRuntime::is_running(cx) || Self::is_pending(&target.environment_id, cx) || !Self::can_maintain(&target.environment_id, cx) {
            return Task::ready(ServerUpdateOutcome::Skipped);
        }
        let Some(supervisor) = ConnectionRuntime::global(cx).supervisor(&target.environment_id) else {
            return Task::ready(ServerUpdateOutcome::Skipped);
        };
        let updates = Self::global(cx).updates.clone();
        let input = ServerSelfUpdateInput {
            target_version: target.target_version.to_string(),
            channel: None,
            target_database_migration: None,
            stage_only: None,
        };
        let id = target.environment_id.clone();
        let run = async move { update_server(&updates, &id, supervisor, input).await };
        let task = Self::follow(target.environment_id.clone(), run, cx);
        cx.spawn(async move |cx| {
            let outcome = task.await;
            if !quiet {
                cx.update(|cx| toast_outcome(&target.server_label, &target.self_update, &outcome, cx));
            }
            outcome
        })
    }

    /// Updates a host on an older protocol over a bare socket (`OutdatedServerUpdateAction`).
    pub fn update_outdated(
        environment_id: EnvironmentId,
        server_label: SharedString,
        from_version: Option<String>,
        target_version: SharedString,
        cx: &mut App,
    ) -> Task<ServerUpdateOutcome> {
        if !ConnectionRuntime::is_running(cx) || Self::is_pending(&environment_id, cx) {
            return Task::ready(ServerUpdateOutcome::Skipped);
        }
        let runtime = ConnectionRuntime::global(cx);
        let entry = runtime.registry().entries().borrow().iter().find(|e| e.environment_id == environment_id).cloned();
        let Some(entry) = entry else { return Task::ready(ServerUpdateOutcome::Skipped) };
        let (auth, supervisor) = (runtime.auth().clone(), runtime.supervisor(&environment_id));
        let updates = Self::global(cx).updates.clone();
        let input = ServerSelfUpdateInput { target_version: target_version.to_string(), channel: None, target_database_migration: None, stage_only: None };
        let run = async move { outdated_host_update::update_outdated_host(&updates, &auth, &entry, supervisor, input, from_version).await };
        let task = Self::follow(environment_id, run, cx);
        cx.spawn(async move |cx| {
            let outcome = task.await;
            cx.update(|cx| toast_outcome(&server_label, &None, &outcome, cx));
            outcome
        })
    }

    /// Runs `run` on tokio and mirrors the environment's state into the entity until it ends.
    fn follow(
        environment_id: EnvironmentId,
        run: impl Future<Output = ServerUpdateOutcome> + Send + 'static,
        cx: &mut App,
    ) -> Task<ServerUpdateOutcome> {
        let updates = Self::global(cx).updates.clone();
        let states = Self::entity(cx);
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let (done_tx, done_rx) = futures::channel::oneshot::channel();
        let watched = environment_id.clone();
        ConnectionRuntime::global(cx).tokio().spawn(async move {
            let mut watch = updates.watch(&watched);
            let forward = sender.clone();
            let mirror = async move {
                loop {
                    _ = forward.unbounded_send(watch.borrow_and_update().clone());
                    if watch.changed().await.is_err() {
                        return;
                    }
                }
            };
            let outcome = tokio::select! {
                outcome = run => outcome,
                () = mirror => ServerUpdateOutcome::Skipped,
            };
            _ = sender.unbounded_send(updates.state(&watched));
            _ = done_tx.send(outcome);
        });
        cx.spawn(async move |cx| {
            while let Some(state) = receiver.next().await {
                let id = environment_id.clone();
                cx.update(|cx| {
                    states.update(cx, |states, cx| {
                        states.states.insert(id, state);
                        cx.notify();
                    })
                });
            }
            done_rx.await.unwrap_or(ServerUpdateOutcome::Skipped)
        })
    }
}

fn toast_outcome(server_label: &SharedString, self_update: &Option<ServerSelfUpdateCapability>, outcome: &ServerUpdateOutcome, cx: &mut App) {
    let toast = match outcome {
        ServerUpdateOutcome::Skipped => return,
        ServerUpdateOutcome::Updated(result) => {
            let description = if *self_update == Some(ServerSelfUpdateCapability::DesktopManaged) {
                t!("Desktop app relaunched on {version}.", version = result.target_version.as_str())
            } else {
                t!("Reconnected on t3@{version}.", version = result.target_version.as_str())
            };
            ToastData::new(ToastKind::Success, t!("{serverLabel} updated", serverLabel = server_label.as_ref())).description(description)
        }
        ServerUpdateOutcome::Failed(failure) => {
            ToastData::new(ToastKind::Error, t!("{serverLabel} update failed", serverLabel = server_label.as_ref())).description(failure_text(failure))
        }
    };
    Toaster::push(toast, cx);
}

/// The failed state's text in the UI language (`serverUpdateFailureText`).
pub fn failure_text(failure: &ServerUpdateFailure) -> SharedString {
    match failure {
        ServerUpdateFailure::Reason(reason) => reason.clone().into(),
        ServerUpdateFailure::ResumeTimeout { version } => t!(
            "Could not confirm that the server resumed on t3@{version}. Reconnect to check the update outcome.",
            version = version.as_str()
        ),
        ServerUpdateFailure::ProgressIncomplete { version } => {
            t!("The t3@{version} update ended before the server accepted the restart.", version = version.as_str())
        }
        ServerUpdateFailure::Terminal { version, status } => match status {
            ServerSelfUpdateOutcomeStatus::Committed => t!("The t3@{version} update committed.", version = version.as_str()),
            ServerSelfUpdateOutcomeStatus::RolledBack => t!("The t3@{version} update rolled-back.", version = version.as_str()),
            _ => t!("The t3@{version} update failed.", version = version.as_str()),
        },
        ServerUpdateFailure::DesktopResumedWithoutUpdate => t!("The desktop app resumed without installing the prepared update."),
        ServerUpdateFailure::CannotSelfUpdate { host } => {
            t!("Update T3 Code on {host} manually; it cannot update itself.", host = host.as_str())
        }
        ServerUpdateFailure::DidNotComeBack { host } => {
            t!("{host} did not come back on a compatible T3 Code version.", host = host.as_str())
        }
        ServerUpdateFailure::Unknown => t!("Server update failed."),
    }
}

/// A running stage's label (`serverUpdateStageLabel`).
pub fn stage_label(stage: ServerUpdateStage) -> SharedString {
    match stage {
        ServerUpdateStage::Downloading | ServerUpdateStage::Installing => t!("Downloading…"),
        ServerUpdateStage::Resuming => t!("Restarting…"),
    }
}

/// The composer banner icon for a state (`ComposerServerUpdateIcon`): static on purpose.
pub fn banner_icon(state: &ServerUpdateState) -> IconName {
    if matches!(state, ServerUpdateState::Failed { .. }) { IconName::CircleAlert } else { IconName::Download }
}

/// The line under a row while an update runs or after it failed (`ServerUpdateProgress`):
/// a 6px dot and the stage, or the failure in the destructive colour with the full text on hover.
pub fn server_update_progress(id: impl Into<SharedString>, state: &ServerUpdateState, cx: &App) -> Option<AnyElement> {
    let theme = cx.app_theme();
    let c = &theme.colors;
    let (color, text, failed) = match state {
        ServerUpdateState::Idle => return None,
        ServerUpdateState::Running { stage, .. } => (c.foreground(), stage_label(*stage), false),
        ServerUpdateState::Failed { failure, .. } => (c.destructive(), failure_text(failure), true),
    };
    let id = id.into();
    let dot = div().flex_none().size(theme.rem(0.375)).rounded_full().bg(color);
    // The failure's full text shows above its label (`TooltipPopup side="top"`).
    let anchor = TooltipAnchor::new();
    let label = div()
        .id(SharedString::from(format!("{id}-text")))
        .relative()
        .min_w_0()
        .truncate()
        .child(text.clone())
        .when(failed, |label| label.child(anchor.measure()).tooltip(anchor.tooltip(text.clone(), TooltipSide::Top)));
    Some(
        div()
            .id(id)
            .mt(theme.sp(1.))
            .flex()
            .min_w_0()
            .items_center()
            .gap(theme.sp(2.))
            .ui_text(theme, TextSize::Xs, if failed { weight::NORMAL } else { weight::MEDIUM })
            .text_color(color)
            .child(dot)
            .child(label)
            .test_support()
            .into_any_element(),
    )
}

/// How an Update control draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAppearance {
    Button,
    /// A 24px ghost icon button with `CircleArrowUp` and the action as its tooltip.
    Icon,
}

/// The per-server Update control (`ServerUpdateAction`). `on_update` runs on click when the host
/// updates itself (the caller confirms a desktop-managed host first); a host that cannot update
/// itself offers its manual command to copy instead.
pub fn server_update_action(
    id: impl Into<SharedString>,
    target: &ServerUpdateTarget,
    label: Option<SharedString>,
    appearance: UpdateAppearance,
    on_update: impl Fn(&mut Window, &mut App) + 'static,
    cx: &mut App,
) -> AnyElement {
    let id = id.into();
    let icon_size = IconSize::Custom(cx.app_theme().rem(0.875));
    if let Some(denial) = &target.updates_denial {
        return match appearance {
            UpdateAppearance::Icon => {
                let anchor = TooltipAnchor::new();
                div()
                .id(SharedString::from(format!("{id}-tip")))
                .relative()
                .tooltip(anchor.tooltip(denial.clone(), TooltipSide::Top))
                .child(anchor.measure())
                .child(
                    Button::new(id)
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::IconXs)
                        .icon(IconName::CircleArrowUp)
                        .icon_size(icon_size)
                        .disabled(true),
                )
                .into_any_element()
            }
            UpdateAppearance::Button => muted_note(denial.clone(), cx),
        };
    }
    if target.self_update == Some(ServerSelfUpdateCapability::DesktopManaged) && !target.desktop_app_update {
        return muted_note(t!("Update the desktop app on that machine to update this server."), cx);
    }
    if target.refused_crossing() {
        return muted_note(t!("Switch tracks in About → Update track"), cx);
    }
    let can_update = ServerUpdateStore::can_maintain(&target.environment_id, cx);
    let manual = target.manual_command();
    let installs = manual_command_installs(target.installation.as_ref());
    let action_label = match &manual {
        Some(_) if installs => t!("Copy update command"),
        Some(_) => t!("Copy relaunch command"),
        None => label.unwrap_or_else(|| t!("Update", context = "verb, updates the server")),
    };
    let disabled = manual.is_none() && !can_update;
    let server_label = target.server_label.clone();
    let click = move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| match &manual {
        Some(command) => copy_manual_command(command, installs, &server_label, cx),
        None => on_update(window, cx),
    };
    match appearance {
        UpdateAppearance::Icon => {
            let anchor = TooltipAnchor::new();
            div()
            .id(SharedString::from(format!("{id}-tip")))
            .relative()
            .tooltip(anchor.tooltip(action_label.clone(), TooltipSide::Top))
            .child(anchor.measure())
            .child(
                Button::new(id)
                    .variant(ButtonVariant::GhostMuted)
                    .size(ButtonSize::IconXs)
                    .icon(IconName::CircleArrowUp)
                    .icon_size(icon_size)
                    .disabled(disabled)
                    .on_click(click),
            )
            .into_any_element()
        }
        UpdateAppearance::Button => Button::new(id)
            .variant(ButtonVariant::Outline)
            .size(ButtonSize::Xs)
            .label(action_label)
            .disabled(disabled)
            .on_click(click)
            .into_any_element(),
    }
}

/// "Update all" over several servers (`ServerUpdatesAction`). `on_update` gets the eligible
/// targets; the caller confirms desktop-managed ones and starts the rest.
pub fn server_updates_action(
    id: impl Into<SharedString>,
    targets: &[ServerUpdateTarget],
    label: Option<SharedString>,
    variant: ButtonVariant,
    on_update: impl Fn(Vec<ServerUpdateTarget>, &mut Window, &mut App) + 'static,
    cx: &mut App,
) -> AnyElement {
    let eligible: Vec<ServerUpdateTarget> = targets.iter().filter(|t| t.updates_remotely()).cloned().collect();
    let pending = eligible.iter().all(|t| ServerUpdateStore::is_pending(&t.environment_id, cx));
    Button::new(id.into())
        .variant(variant)
        .size(ButtonSize::Xs)
        .label(label.unwrap_or_else(|| t!("Update all")))
        .disabled(eligible.is_empty() || pending)
        .on_click(move |_, window, cx| {
            let available: Vec<_> = eligible.iter().filter(|t| !ServerUpdateStore::is_pending(&t.environment_id, cx)).cloned().collect();
            on_update(available, window, cx);
        })
        .into_any_element()
}

/// Starts each target, as `ServerUpdatesAction` does once confirmed.
pub fn update_all(targets: Vec<ServerUpdateTarget>, cx: &mut App) {
    for target in targets {
        ServerUpdateStore::update(target, false, cx).detach();
    }
}

/// The text `requestConfirmDialog` shows before relaunching desktop apps.
pub fn desktop_confirmation(targets: &[ServerUpdateTarget]) -> SharedString {
    match targets {
        [one] => t!(
            "Update the T3 Code desktop app that runs the {serverLabel}? It will close and relaunch on that machine.",
            serverLabel = one.server_label.as_ref()
        ),
        many => t!(
            "Update the T3 Code desktop apps on {servers}? They will close and relaunch on those machines.",
            servers = many.iter().map(|t| t.server_label.as_ref()).collect::<Vec<_>>().join(", ").as_str()
        ),
    }
}

/// Splits a confirm message into the dialog's title and description (`resolveConfirmDialogCopy`):
/// the line ending in a question mark, else the text through the first question mark.
pub fn confirm_copy(message: &str) -> (SharedString, Option<SharedString>) {
    let is_question = |c: char| c == '?' || c == '？';
    let message = message.trim();
    let lines: Vec<&str> = message.split('\n').collect();
    if let Some(ix) = lines.iter().position(|line| line.trim().ends_with(is_question)) {
        let rest = lines.iter().enumerate().filter(|(i, _)| *i != ix).map(|(_, l)| *l).collect::<Vec<_>>().join("\n");
        let rest = rest.trim();
        return (lines[ix].trim().to_owned().into(), (!rest.is_empty()).then(|| rest.to_owned().into()));
    }
    if let Some(at) = message.find(is_question) {
        let end = at + message[at..].chars().next().map_or(1, char::len_utf8);
        let rest = message[end..].trim();
        return (message[..end].trim().to_owned().into(), (!rest.is_empty()).then(|| rest.to_owned().into()));
    }
    let description = if message.is_empty() { t!("This action requires your confirmation.") } else { message.to_owned().into() };
    (t!("Confirm action"), Some(description))
}

/// The Update button for a host on an older protocol (`OutdatedServerUpdateAction`).
pub fn outdated_server_update_action(
    id: impl Into<SharedString>,
    environment_id: EnvironmentId,
    server_label: SharedString,
    from_version: Option<String>,
    target_version: SharedString,
    label: SharedString,
    cx: &App,
) -> AnyElement {
    if from_version.as_deref().is_some_and(|from| crate::version_skew::crosses_release_track(from, &target_version)) {
        return muted_note(t!("Switch tracks in About → Update track"), cx);
    }
    Button::new(id.into())
        .variant(ButtonVariant::Outline)
        .size(ButtonSize::Xs)
        .label(label)
        .on_click(move |_, _, cx| {
            ServerUpdateStore::update_outdated(environment_id.clone(), server_label.clone(), from_version.clone(), target_version.clone(), cx)
                .detach();
        })
        .into_any_element()
}

fn muted_note(text: SharedString, cx: &App) -> AnyElement {
    let theme = cx.app_theme();
    div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(text).into_any_element()
}

/// Copies the manual command and says what to do with it (`useCopyToClipboard`'s `onCopy`).
fn copy_manual_command(command: &str, installs: bool, server_label: &SharedString, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(command.to_owned()));
    let toast = if installs {
        ToastData::new(ToastKind::Success, t!("Update command copied")).description(t!(
            "Run `{command}` on {serverLabel}, then restart t3 with your usual options.",
            command = command,
            serverLabel = server_label.as_ref()
        ))
    } else {
        ToastData::new(ToastKind::Success, t!("Relaunch command copied")).description(t!(
            "Stop t3 on {serverLabel}, then relaunch with `{command}` using the same subcommand and options. This does not update an installed t3 command.",
            command = command,
            serverLabel = server_label.as_ref()
        ))
    };
    Toaster::push(toast, cx);
}
