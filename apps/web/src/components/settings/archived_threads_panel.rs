//! Archive. Ports `ArchivedThreadsPanel.tsx`: archived threads from every
//! connected environment (`orchestration.getArchivedShellSnapshot`), grouped
//! by project, newest first, with Unarchive and Delete
//! (`orchestration.dispatchCommand`). Delete asks first while "Delete
//! confirmation" is on.

use std::rc::Rc;

use gpui_kit::{TestSupportExt as _, AnyElement, App, Context, IntoElement, Render, SharedString, Window, div, prelude::*};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::rpc::methods::{OrchestrationDispatchCommand, OrchestrationGetArchivedShellSnapshot};
use t3code_contracts::{
    CommandId, OrchestrationV2ArchivedShellSnapshot, OrchestrationV2Command, OrchestrationV2CommandThreadDelete,
    OrchestrationV2CommandThreadUnarchive, ThreadId,
};

use super::settings_controls::environment_label;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::settings_search::setting_title;
use crate::components::icons::IconName;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::menu::{ContextMenu, MenuItem, MenuPopup};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::{ClientSettingsStore, ServerSettingsStore};
use crate::i18n::t;
use crate::ActiveAppTheme as _;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArchiveAction {
    Unarchive,
    Delete,
}

/// One archived thread to show.
#[derive(Clone)]
struct ArchivedThread {
    environment: EnvironmentId,
    id: ThreadId,
    title: String,
    archived_at: String,
    created_at: String,
}

/// The Archive page and its last load.
pub struct ArchivedThreadsPanel {
    loading: bool,
    error: Option<SharedString>,
    snapshots: Vec<(EnvironmentId, OrchestrationV2ArchivedShellSnapshot)>,
    pending: Vec<ThreadId>,
    confirm_delete: Option<ArchivedThread>,
    /// How many threads the live shell showed at the last load: an archive or unarchive from
    /// anywhere changes it, and the page reloads (the fork reloads on each visit).
    shell_rows: usize,
    _shell: Option<gpui_kit::Subscription>,
}

impl ArchivedThreadsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell = crate::state::StateBridge::is_running(cx).then(|| {
            cx.observe(&crate::state::StateBridge::shell(cx), |this: &mut Self, shell, cx| {
                let rows = shell.read(cx).presented().len();
                if rows != this.shell_rows {
                    this.shell_rows = rows;
                    this.refresh(cx);
                }
            })
        });
        let shell_rows = if crate::state::StateBridge::is_running(cx) { crate::state::StateBridge::shell(cx).read(cx).presented().len() } else { 0 };
        let mut this = Self { loading: false, error: None, snapshots: Vec::new(), pending: Vec::new(), confirm_delete: None, shell_rows, _shell: shell };
        this.refresh(cx);
        this
    }

    /// Reloads every environment's archived threads.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !ConnectionRuntime::is_running(cx) {
            return;
        }
        let ids = ServerSettingsStore::environment_ids(cx);
        let calls: Vec<_> = ids
            .iter()
            .map(|id| (id.clone(), ServerSettingsStore::call::<OrchestrationGetArchivedShellSnapshot>(id, Default::default(), cx)))
            .collect();
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let mut snapshots = Vec::new();
            let mut error = None;
            for (id, call) in calls {
                match call.await {
                    Ok(snapshot) => snapshots.push((id, snapshot)),
                    Err(e) => error = Some(e.0),
                }
            }
            this.update(cx, |this, cx| {
                this.loading = false;
                this.snapshots = snapshots;
                this.error = error;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run(&mut self, action: ArchiveAction, thread: ArchivedThread, cx: &mut Context<Self>) {
        if self.pending.contains(&thread.id) {
            return;
        }
        if action == ArchiveAction::Delete && ClientSettingsStore::get(cx).confirm_thread_delete && self.confirm_delete.is_none() {
            self.confirm_delete = Some(thread);
            cx.notify();
            return;
        }
        self.confirm_delete = None;
        self.pending.push(thread.id.clone());
        let command_id = CommandId::from(uuid::Uuid::new_v4().to_string().as_str());
        let command = match action {
            ArchiveAction::Unarchive => OrchestrationV2Command::ThreadUnarchive(OrchestrationV2CommandThreadUnarchive { command_id, thread_id: thread.id.clone() }),
            ArchiveAction::Delete => OrchestrationV2Command::ThreadDelete(OrchestrationV2CommandThreadDelete { command_id, thread_id: thread.id.clone(), only_if_unused: None }),
        };
        let call = ServerSettingsStore::call::<OrchestrationDispatchCommand>(&thread.environment, command, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                this.pending.retain(|id| *id != thread.id);
                match result {
                    Ok(_) => this.refresh(cx),
                    Err(error) => {
                        let title = if action == ArchiveAction::Unarchive { t!("Failed to unarchive thread") } else { t!("Failed to delete thread") };
                        Toaster::push(ToastData::new(ToastKind::Error, title).description(error.0), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for ArchivedThreadsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut groups: Vec<(String, Vec<ArchivedThread>)> = Vec::new();
        let multiple = self.snapshots.iter().filter(|(_, s)| !s.threads.is_empty()).count() > 1;
        for (environment, snapshot) in &self.snapshots {
            for project in &snapshot.projects {
                let mut threads: Vec<ArchivedThread> = snapshot
                    .threads
                    .iter()
                    .filter(|thread| thread.project_id == project.id)
                    .map(|thread| ArchivedThread {
                        environment: environment.clone(),
                        id: thread.id.clone(),
                        title: thread.title.clone(),
                        archived_at: thread.archived_at.clone().unwrap_or_else(|| thread.created_at.clone()),
                        created_at: thread.created_at.clone(),
                    })
                    .collect();
                if threads.is_empty() {
                    continue;
                }
                threads.sort_by(|a, b| b.archived_at.cmp(&a.archived_at));
                groups.push((project.title.clone(), threads));
            }
        }
        let this = cx.entity();
        let sections: Vec<AnyElement> = if groups.is_empty() {
            let (title, description, empty) = if self.loading {
                (t!("Loading archived threads"), t!("Checking connected environments."), false)
            } else if let Some(error) = &self.error {
                (t!("Could not load archived threads"), error.clone(), false)
            } else {
                (t!("No archived threads"), t!("Archived threads will appear here."), true)
            };
            let mut row = SettingsRow::new(title).description(description);
            if empty {
                row = row.title_icon(crate::components::icons::IconName::Archive);
            }
            vec![SettingsSection::new(setting_title("archive")).child(row).into_any_element()]
        } else {
            groups
                .into_iter()
                .map(|(title, threads)| {
                    let gap = cx.app_theme().sp(2.);
                    SettingsSection::new(title)
                        .children(threads.into_iter().map(|thread| {
                            let now = crate::timestamp_format::now_ms();
                            let relative = |iso: &str| crate::timestamp_format::format_relative_time(iso, now).map(|p| p.label).unwrap_or_default();
                            let when = format!("Archived {} · Created {}", relative(&thread.archived_at), relative(&thread.created_at));
                            let description = if multiple {
                                format!("{} · {}", environment_label(&thread.environment, cx), when)
                            } else {
                                when
                            };
                            let busy = self.pending.contains(&thread.id);
                            let id = thread.id.as_str().to_owned();
                            // `ARCHIVE_ACTIONS`: an outline icon button per action, with its label as tooltip.
                            let button = |action: ArchiveAction| {
                                let (handle, thread) = (this.clone(), thread.clone());
                                let (key, icon, variant, label) = match action {
                                    ArchiveAction::Unarchive => ("unarchive", IconName::ArchiveX, ButtonVariant::Outline, t!("Unarchive")),
                                    ArchiveAction::Delete => ("delete", IconName::Trash2, ButtonVariant::DestructiveOutline, t!("Delete", context = "verb, deletes a thread")),
                                };
                                let button = Button::new(SharedString::from(format!("archived-{key}-{id}")))
                                    .variant(variant)
                                    .size(ButtonSize::IconSm)
                                    .icon(icon)
                                    .disabled(busy)
                                    .on_click(move |_, _, cx| handle.update(cx, |this, cx| this.run(action, thread.clone(), cx)));
                                div().id(SharedString::from(format!("archived-tip-{key}-{id}"))).tooltip(crate::components::ui::tooltip::tooltip(label)).child(button).test_support()
                            };
                            let controls = div().flex().flex_none().items_center().gap(gap).child(button(ArchiveAction::Unarchive)).child(button(ArchiveAction::Delete));
                            let row = SettingsRow::new(thread.title.clone()).description(description).control(controls);
                            let (handle, menu_thread) = (this.clone(), thread.clone());
                            ContextMenu::new(SharedString::from(format!("archived-menu-{id}")), row, move |_, cx| {
                                if handle.read(cx).pending.contains(&menu_thread.id) {
                                    return MenuPopup::new().context();
                                }
                                let item = |action: ArchiveAction, label: SharedString| {
                                    let (handle, thread) = (handle.clone(), menu_thread.clone());
                                    MenuItem::new(label).on_select(move |_, cx| handle.update(cx, |this, cx| this.run(action, thread.clone(), cx)))
                                };
                                MenuPopup::new()
                                    .context()
                                    .item(item(ArchiveAction::Unarchive, t!("Unarchive")))
                                    .item(item(ArchiveAction::Delete, t!("Delete", context = "verb, deletes a thread")).destructive())
                            })
                        }))
                        .into_any_element()
                })
                .collect()
        };
        let confirm = self.confirm_delete.clone().map(|thread| {
            let close = Rc::new(cx.listener(|this, _: &(), _, cx| {
                this.confirm_delete = None;
                cx.notify();
            }));
            let cancel = close.clone();
            let delete = cx.listener(move |this, _: &gpui_kit::ClickEvent, _, cx| this.run(ArchiveAction::Delete, thread.clone(), cx));
            Modal::dialog(
                "archived-delete-dialog",
                DialogSurface::new(t!("Delete {title}", title = self.confirm_delete.as_ref().map(|t| t.title.clone()).unwrap_or_default())).footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap(gpui_kit::rems(0.5))
                        .child(Button::new("archived-delete-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(move |_, w, cx| cancel(&(), w, cx)))
                        .child(Button::new("archived-delete-confirm").variant(ButtonVariant::Destructive).label(t!("Delete", context = "verb, deletes a thread")).on_click(delete)),
                ),
                move |w, cx: &mut App| close(&(), w, cx),
            )
        });
        page_container(cx).children(sections).children(confirm)
    }
}
