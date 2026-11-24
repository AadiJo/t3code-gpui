//! The sidebar's events as runtime commands (`Sidebar.tsx` handlers, `threadCommands.ts`).
//! Row actions show at once through the shell's optimistic layer; failures surface as a toast.
//! Events that need more UI (rename, new thread, settings) come back as a [`SidebarOutcome`].

use gpui_kit::{App, ClipboardItem, SharedString};
use jiff::ZonedDateTime as Zoned;
use t3code_client_runtime::operations::{CommandError, EnvironmentCommands};
use t3code_client_runtime::state::models::{format_ms, now_ms};
use t3code_contracts::ThreadId;

use super::StateBridge;
use super::shell::runtime_ids;
use crate::components::sidebar::SidebarEvent;
use crate::components::sidebar::context_menu::MenuCommand;
use crate::components::sidebar::model::{ProjectRef, ThreadKey};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;

/// What the app must do after a sidebar event the bridge cannot finish alone.
#[derive(Clone, Debug)]
pub enum SidebarOutcome {
    /// Nothing more.
    Done,
    /// Show this thread in the main pane (the bridge already opened it).
    Opened(ThreadKey),
    /// Ask for a new title.
    Rename(ThreadKey),
    /// Start a new thread, in a project when one is given.
    NewThread(Option<ProjectRef>),
    /// Ask for a snooze time.
    SnoozeCustom(Vec<ThreadKey>),
    /// An app-level destination (settings, pull requests, project filter…).
    Navigate(SidebarEvent),
}

fn commands_for(key: &ThreadKey, cx: &App) -> Option<(EnvironmentCommands, ThreadId)> {
    let (environment_id, thread_id) = runtime_ids(key);
    let environment = StateBridge::environment(&environment_id, cx)?;
    Some((environment.read(cx).commands.clone(), thread_id))
}

/// Runs a command on each thread and toasts the first failure.
fn run_each<F, Fut>(keys: &[ThreadKey], cx: &mut App, command: F)
where
    F: Fn(EnvironmentCommands, ThreadId) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<t3code_client_runtime::operations::Accepted, CommandError>> + Send,
{
    run_each_titled(keys, t!("Something went wrong."), cx, command)
}

/// [`run_each`] with the failure toast's title.
fn run_each_titled<F, Fut>(keys: &[ThreadKey], failure: SharedString, cx: &mut App, command: F)
where
    F: Fn(EnvironmentCommands, ThreadId) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<t3code_client_runtime::operations::Accepted, CommandError>> + Send,
{
    let targets: Vec<_> = keys.iter().filter_map(|key| commands_for(key, cx)).collect();
    let task = StateBridge::spawn(
        async move {
            let mut first_error = None;
            for (commands, thread_id) in targets {
                if let Err(error) = command(commands, thread_id).await {
                    first_error.get_or_insert(error);
                }
            }
            first_error
        },
        cx,
    );
    cx.spawn(async move |cx| {
        if let Some(Some(error)) = task.await {
            cx.update(|cx| {
                Toaster::push(ToastData::new(ToastKind::Error, failure).description(SharedString::from(error.to_string())), cx);
            });
        }
    })
    .detach();
}

/// Renames a thread.
pub fn rename(key: &ThreadKey, title: String, cx: &mut App) {
    run_each(std::slice::from_ref(key), cx, move |commands, thread_id| {
        let title = title.clone();
        async move { commands.rename(&thread_id, title).await }
    });
}

/// Pins or unpins every checkout of a project group, one machine at a time, and toasts the
/// first failure (`useProjectPin` → `updateProjectGroup`).
pub fn set_projects_pinned(projects: &[ProjectRef], pinned: bool, cx: &mut App) {
    let targets: Vec<_> = projects
        .iter()
        .filter_map(|project| {
            let environment_id = t3code_contracts::EnvironmentId::new(project.environment_id.0.as_ref());
            let commands = StateBridge::environment(&environment_id, cx)?.read(cx).commands.clone();
            Some((commands, t3code_contracts::ProjectId::new(project.project_id.0.as_ref())))
        })
        .collect();
    let task = StateBridge::spawn(
        async move {
            for (commands, project_id) in targets {
                let update = t3code_contracts::ProjectMutationProjectUpdate {
                    command_id: t3code_client_runtime::operations::commands::new_command_id(),
                    project_id,
                    title: None,
                    workspace_root: None,
                    default_model_selection: None,
                    auto_pull: None,
                    project_icon: None,
                    favicon_path: None,
                    default_thread_env_mode: None,
                    scripts: None,
                    pinned: Some(pinned),
                    archived: None,
                };
                if let Err(error) = commands.mutate_project(t3code_contracts::ProjectMutation::ProjectUpdate(update)).await {
                    return Some(error);
                }
            }
            None
        },
        cx,
    );
    cx.spawn(async move |cx| {
        if let Some(Some(error)) = task.await {
            cx.update(|cx| {
                let title = if pinned { t!("Failed to pin project") } else { t!("Failed to unpin project") };
                Toaster::push(ToastData::new(ToastKind::Error, title).description(SharedString::from(error.to_string())), cx);
            });
        }
    })
    .detach();
}

/// Snoozes threads until a time.
pub fn snooze(keys: &[ThreadKey], until: &Zoned, cx: &mut App) {
    let until = format_ms(until.timestamp().as_millisecond());
    run_each(keys, cx, move |commands, thread_id| {
        let until = until.clone();
        async move { commands.snooze(&thread_id, until).await }
    });
}

/// Offers the undo notice for an action about to run on `keys`, planned from each thread's row
/// before it changes (`planThreadUndo`, `runUndoPlan`).
fn offer_undo(keys: &[ThreadKey], command: MenuCommand, cx: &mut App) {
    use crate::thread_undo::{ThreadUndo, UndoAction};
    let action = match command {
        MenuCommand::Settle => UndoAction::Settled,
        MenuCommand::Snooze(_) => UndoAction::Snoozed,
        MenuCommand::Unpin => UndoAction::Unpinned,
        MenuCommand::Archive => UndoAction::Archived,
        _ => return,
    };
    let shell = StateBridge::shell(cx);
    let rows: Vec<_> = keys.iter().filter_map(|key| shell.read(cx).snapshot().threads.iter().find(|t| &t.key == key).cloned()).collect();
    for row in rows {
        let key = row.key.clone();
        let iso = |ms: Option<crate::components::sidebar::model::TimestampMs>| ms.map(|ms| format_ms(ms.0));
        let pin_order = row.pin_order_key.as_ref().map(|k| k.to_string());
        let pinned = row.pinned_at.is_some();
        let active_order = row.active_order_key.as_ref().map(|k| k.to_string());
        let unsettled_at = iso(row.unsettled_at);
        let snoozed_until = row.snoozed_until;
        let undo: crate::thread_undo::UndoFn = std::rc::Rc::new(move |cx| {
            let (pin_order, active_order, unsettled_at) = (pin_order.clone(), active_order.clone(), unsettled_at.clone());
            // A snooze can expire before Undo runs, and the server rejects an expired restore.
            let snooze = snoozed_until.filter(|until| until.0 > now_ms()).map(|until| format_ms(until.0));
            let failure = match action {
                UndoAction::Settled => t!("Failed to undo settle"),
                UndoAction::Snoozed => t!("Failed to undo snooze"),
                UndoAction::Unpinned => t!("Failed to undo unpin"),
                UndoAction::Archived => t!("Failed to undo archive"),
            };
            run_each_titled(std::slice::from_ref(&key), failure, cx, move |c, id| {
                let (pin_order, active_order, unsettled_at, snooze) = (pin_order.clone(), active_order.clone(), unsettled_at.clone(), snooze.clone());
                async move {
                    let mut result = match action {
                        UndoAction::Snoozed => return c.unsnooze(&id).await,
                        UndoAction::Archived => return c.unarchive(&id).await,
                        UndoAction::Unpinned => c.pin(&id, pin_order).await?,
                        UndoAction::Settled => {
                            let accepted = c.unsettle(&id, Some(unsettled_at)).await?;
                            if pinned {
                                c.pin(&id, pin_order).await?
                            } else if let Some(order) = active_order {
                                c.reorder_active(&id, order).await?
                            } else {
                                accepted
                            }
                        }
                    };
                    if let Some(until) = snooze {
                        result = c.snooze(&id, until).await?;
                    }
                    Ok(result)
                }
            });
        });
        ThreadUndo::show(action, row.key.clone(), undo, cx);
    }
}

fn thread_command(keys: Vec<ThreadKey>, command: MenuCommand, cx: &mut App) -> SidebarOutcome {
    offer_undo(&keys, command, cx);
    match command {
        MenuCommand::Pin => run_each(&keys, cx, |c, id| async move { c.pin(&id, None).await }),
        MenuCommand::Unpin => run_each(&keys, cx, |c, id| async move { c.unpin(&id).await }),
        MenuCommand::Settle => run_each(&keys, cx, |c, id| async move { c.settle(&id).await }),
        MenuCommand::Unsettle => run_each(&keys, cx, |c, id| async move { c.unsettle(&id, None).await }),
        MenuCommand::Snooze(until) => {
            if let Ok(until) = jiff::Timestamp::from_millisecond(until.0).map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::UTC)) {
                snooze(&keys, &until, cx)
            }
        }
        MenuCommand::SnoozeCustom => return SidebarOutcome::SnoozeCustom(keys),
        MenuCommand::Wake => run_each(&keys, cx, |c, id| async move { c.unsnooze(&id).await }),
        MenuCommand::Rename => return keys.into_iter().next().map_or(SidebarOutcome::Done, SidebarOutcome::Rename),
        MenuCommand::RegenerateTitle => run_each(&keys, cx, |c, id| async move { c.regenerate_title(&id).await.map(|(_, accepted)| accepted) }),
        MenuCommand::MarkUnread => run_each(&keys, cx, |c, id| async move { c.mark_unread(&id).await }),
        MenuCommand::AutoSettle(enabled) => run_each(&keys, cx, move |c, id| async move { c.set_auto_settle(&id, enabled).await }),
        // The chat header opens find itself; the sidebar never offers it.
        MenuCommand::FindInThread => {}
        MenuCommand::Archive => run_each(&keys, cx, |c, id| async move { c.archive(&id).await }),
        MenuCommand::Delete => run_each(&keys, cx, |c, id| async move { c.delete(&id, false).await }),
        MenuCommand::CopyThreadId => {
            let ids = keys.iter().map(|key| key.thread_id.0.to_string()).collect::<Vec<_>>().join("\n");
            cx.write_to_clipboard(ClipboardItem::new_string(ids));
        }
        MenuCommand::CopyBranch | MenuCommand::CopyPath | MenuCommand::CopyThread => {
            let shell = StateBridge::shell(cx);
            let shell = shell.read(cx);
            let text = keys
                .iter()
                .filter_map(|key| shell.row(key))
                .filter_map(|row| match command {
                    MenuCommand::CopyBranch => row.source.branch.clone(),
                    MenuCommand::CopyPath => row.source.worktree_path.clone(),
                    _ => Some(row.title().to_owned()),
                })
                .collect::<Vec<_>>()
                .join("\n");
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        MenuCommand::NewThreadOnBranch | MenuCommand::NewThread => {
            let shell = StateBridge::shell(cx);
            let project = keys.first().and_then(|key| shell.read(cx).snapshot().threads.iter().find(|t| &t.key == key).map(|t| t.project_ref()));
            return SidebarOutcome::NewThread(project);
        }
        MenuCommand::FilterByProject | MenuCommand::ProjectSettings | MenuCommand::PinProject | MenuCommand::UnpinProject => {
            return SidebarOutcome::Navigate(SidebarEvent::ThreadCommand(keys, command));
        }
    }
    SidebarOutcome::Done
}

/// Handles one sidebar event. Activation opens the thread in [`super::threads::ThreadViews`]
/// synchronously, so its timeline can paint in the same frame.
pub fn handle(event: &SidebarEvent, cx: &mut App) -> SidebarOutcome {
    match event {
        SidebarEvent::ActivateThread(key) => {
            let threads = StateBridge::threads(cx);
            match threads.update(cx, |threads, cx| threads.open(key.clone(), cx)) {
                Some(_) => SidebarOutcome::Opened(key.clone()),
                None => SidebarOutcome::Done,
            }
        }
        SidebarEvent::NewThread(project) => SidebarOutcome::NewThread(project.clone()),
        SidebarEvent::ThreadCommand(keys, command) => thread_command(keys.clone(), *command, cx),
        SidebarEvent::DiscardDraft(key) => {
            let (environment_id, thread_id) = runtime_ids(key);
            let scoped = t3code_client_runtime::state::drafts::scoped_thread_key(&environment_id, &thread_id);
            StateBridge::drafts(cx).update(|drafts| {
                drafts.drafts_by_thread_key.remove(&scoped);
            });
            SidebarOutcome::Done
        }
        SidebarEvent::RenameThread(key, title) => {
            rename(key, title.to_string(), cx);
            SidebarOutcome::Done
        }
        other => SidebarOutcome::Navigate(other.clone()),
    }
}

/// The current time as the runtime formats it.
pub fn now_iso() -> String {
    format_ms(now_ms())
}

/// One snooze menu choice: `id` is `snoozePresetId`, `when` the wake time as shown.
#[derive(Clone, Debug)]
pub struct ResolvedSnoozePreset {
    pub id: SharedString,
    pub label: SharedString,
    pub when: SharedString,
    pub wake: Zoned,
}

/// `snoozePresetId`.
pub fn snooze_preset_id(setting: &t3code_contracts::SnoozePresetSetting) -> SharedString {
    match setting {
        t3code_contracts::SnoozePresetSetting::After(a) => format!("after:{}", a.minutes).into(),
        t3code_contracts::SnoozePresetSetting::At(at) => {
            let day = serde_json::to_value(&at.day).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            format!("at:{day}:{}", at.time).into()
        }
        t3code_contracts::SnoozePresetSetting::Unknown(_) => SharedString::default(),
    }
}

/// `resolveSnoozePresets`: an environment's snooze menu at `now` (its settings, else the
/// defaults), soonest first, one choice per wake minute.
pub fn resolve_snooze_presets(settings: Option<&[t3code_contracts::SnoozePresetSetting]>, now: &Zoned) -> Vec<ResolvedSnoozePreset> {
    use crate::components::settings::threads_settings_snooze::{default_presets, resolve};
    let defaults;
    let settings = match settings {
        Some(settings) => settings,
        None => {
            defaults = default_presets();
            &defaults
        }
    };
    let mut presets: Vec<ResolvedSnoozePreset> = settings
        .iter()
        .filter_map(|setting| {
            let resolved = resolve(setting, now)?;
            Some(ResolvedSnoozePreset { id: snooze_preset_id(setting), label: resolved.label, when: resolved.when.into(), wake: resolved.wake })
        })
        .collect();
    let minute = |p: &ResolvedSnoozePreset| p.wake.timestamp().as_second().div_euclid(60);
    presets.sort_by_key(minute);
    let mut seen = std::collections::HashSet::new();
    presets.retain(|p| seen.insert(minute(p)));
    presets
}
