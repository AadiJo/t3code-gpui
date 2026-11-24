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
                Toaster::push(ToastData::new(ToastKind::Error, t!("Something went wrong.")).description(SharedString::from(error.to_string())), cx);
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

/// Snoozes threads until a time.
pub fn snooze(keys: &[ThreadKey], until: &Zoned, cx: &mut App) {
    let until = format_ms(until.timestamp().as_millisecond());
    run_each(keys, cx, move |commands, thread_id| {
        let until = until.clone();
        async move { commands.snooze(&thread_id, until).await }
    });
}

fn thread_command(keys: Vec<ThreadKey>, command: MenuCommand, cx: &mut App) -> SidebarOutcome {
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
