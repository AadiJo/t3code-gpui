//! Settings → Projects. Ports `ProjectsSettings.tsx`: with no project picked,
//! the `SettingsScopeNotice` list of projects; picking one opens its
//! `ProjectSettingsPanel` page (Name, Project icon, Pin, Archive, actions,
//! Danger), each change sent with `projects.mutate`.

use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{CommandId, ProjectMutation, ProjectMutationProjectUpdate};

use super::provider_settings_panel::toast_error;
use super::server_settings_controls::{Fields, field_input};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::add_project_to_machine_dialog::{AddProjectToMachineDialog, AddProjectToMachineEvent};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::project_favicon::ProjectFavicon;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::components::ui::switch::Switch;
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// The Projects page.
pub struct ProjectsSettingsPanel {
    model: Entity<ServerPanelsModel>,
    selected: Option<String>,
    fields: Fields,
    dialog: Option<Entity<AddProjectToMachineDialog>>,
    _subscriptions: Vec<Subscription>,
}

fn command_id() -> CommandId {
    CommandId::new(format!("settings-{}", uuid_like()))
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
}

impl ProjectsSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, selected: None, fields: Fields::default(), dialog: None, _subscriptions: vec![observe] }
    }

    fn update_project(&mut self, update: ProjectMutationProjectUpdate, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| {
            m.run("project-mutate", move |s| s.mutate_project(ProjectMutation::ProjectUpdate(update)), |result, cx| {
                if let Err(e) = result {
                    toast_error(t!("Could not update project"), e, cx);
                }
            }, cx)
        });
    }

    /// Opens "Add to another machine" for the selected project.
    pub fn open_add_to_machine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.selected.clone() else { return };
        let model = self.model.clone();
        let dialog = cx.new(|cx| AddProjectToMachineDialog::new(model, project, window, cx));
        let sub = cx.subscribe(&dialog, |this, _, _: &AddProjectToMachineEvent, cx| {
            this.dialog = None;
            cx.notify();
        });
        self._subscriptions.push(sub);
        self.dialog = Some(dialog);
        cx.notify();
    }

    pub fn dialog(&self) -> Option<&Entity<AddProjectToMachineDialog>> {
        self.dialog.as_ref()
    }
}

impl ProjectsSettingsPanel {
    /// `SettingsScopeNotice target="project"`: a line of copy and one row per
    /// project with its icon, name, path and a chevron.
    fn render_list(&mut self, projects: &[t3code_contracts::Project], cx: &mut Context<Self>) -> gpui_kit::Div {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let rows = projects.iter().enumerate().map(|(ix, project)| {
            let id = project.id.to_string();
            let secondary: SharedString = if project.archived_at.is_some() {
                format!("{} · {}", t!("Archived", context = "project state"), project.workspace_root).into()
            } else {
                project.workspace_root.clone().into()
            };
            div()
                .id(SharedString::from(format!("project-row-{id}")))
                .flex()
                .items_center()
                .gap(theme.sp(3.))
                .w_full()
                .min_w_0()
                .px(theme.sp(4.))
                .py(theme.sp(3.))
                .when(ix > 0, |d| d.border_t_1().border_color(with_alpha(c.border, 0.5)))
                .cursor_pointer()
                .hover(|d| d.bg(with_alpha(c.accent, 0.5)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                }))
                .child(ProjectFavicon::new(project.title.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(project.title.clone()))
                        .child(div().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(secondary)),
                )
                .child(Icon::new(IconName::ChevronRight).size(IconSize::Md).color(c.muted_foreground))
                .into_any_element()
        });
        page_container(cx).child(
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(3.))
                .child(div().px(theme.sp(4.)).ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(t!("Choose a project to manage its name, icon, checkouts and actions.")))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(theme.radii.xl)
                        .border_1()
                        .border_color(with_alpha(c.border, 0.6))
                        .bg(with_alpha(c.card(), 0.4))
                        .shadow(theme.shadows.xs5.clone())
                        .children(rows),
                ),
        )
    }

    /// `ProjectSettingsPanel` for one project.
    fn render_project(&mut self, project: &t3code_contracts::Project, ro: bool, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::Div {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let pid = project.id.clone();
        let this = cx.entity().downgrade();
        let rename_id = pid.clone();
        let name = self.fields.draft(format!("{}:name", project.id), project.title.clone(), "", false, window, cx, move |text, _, cx| {
            let title = text.trim().to_owned();
            if title.is_empty() {
                Toaster::push(ToastData::new(ToastKind::Warning, t!("Project title cannot be empty")), cx);
                return;
            }
            let id = rename_id.clone();
            this.update(cx, |this, cx| this.update_project(update(id, |u| u.title = Some(title)), cx)).ok();
        });
        let (pin_id, archive_id, delete_id) = (pid.clone(), pid.clone(), pid.clone());
        let pinned = project.pinned_at.is_some();
        let archived = project.archived_at.is_some();
        let this_pin = cx.entity().downgrade();
        let alert = div()
            .flex()
            .items_start()
            .gap(theme.sp(2.))
            .rounded(theme.radii.xl)
            .border_1()
            .border_color(with_alpha(theme.fixed.info, 0.32))
            .bg(with_alpha(theme.fixed.info, 0.04))
            .px(theme.sp(3.5))
            .py(theme.sp(3.))
            .ui_text(theme, TextSize::Sm, weight::NORMAL)
            .child(div().pt(px(2.)).child(Icon::new(IconName::Info).size(IconSize::Md).color(theme.fixed.info)))
            .child(div().flex_1().text_color(c.muted_foreground).child(t!("Can't find a setting? Keep this project picked above and hop to any other settings page.")));
        let overview = div()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(theme.radii.xl)
            .border_1()
            .border_color(with_alpha(c.border, 0.6))
            .bg(with_alpha(c.card(), 0.4))
            .children(
                [
                    SettingsRow::new(t!("Name"))
                        .description(t!("The shared name for this project group in the sidebar and thread lists."))
                        .control(div().w(theme.sp(56.)).child(field_input(&name, false, false, ro)))
                        .into_any_element(),
                    SettingsRow::new(t!("Project icon"))
                        .description(t!("Automatic"))
                        .control(ProjectFavicon::new(project.title.clone()).size(px(20.)))
                        .into_any_element(),
                    SettingsRow::new(t!("Pin"))
                        .description(t!("Pinned projects come first in project pickers."))
                        .control(Switch::new("project-pin", pinned).disabled(ro).on_change(move |v, _, cx| {
                            let id = pin_id.clone();
                            this_pin.update(cx, |this, cx| this.update_project(update(id, |u| u.pinned = Some(v)), cx)).ok();
                        }))
                        .into_any_element(),
                    SettingsRow::new(t!("Archive"))
                        .description(if archived {
                            t!("Hidden from project pickers. Threads and files are kept.")
                        } else {
                            t!("Hides the project from project pickers. Threads and files are kept.")
                        })
                        .control(
                            Button::new("project-archive")
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Sm)
                                .label(if archived { t!("Unarchive") } else { t!("Archive", context = "verb, archives a project") })
                                .disabled(ro)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_project(update(archive_id.clone(), |u| u.archived = Some(!archived)), cx);
                                })),
                        )
                        .into_any_element(),
                ]
                .into_iter()
                .enumerate()
                .map(|(ix, row)| div().when(ix > 0, |d| d.border_t_1().border_color(with_alpha(c.border, 0.5))).child(row)),
            );
        let mut actions = SettingsSection::new(t!("Actions"));
        for script in &project.scripts {
            actions.extend([SettingsRow::new(SharedString::from(script.name.clone())).description(SharedString::from(script.command.clone())).into_any_element()]);
        }
        if project.scripts.is_empty() {
            actions.extend([SettingsRow::new(t!("No actions")).into_any_element()]);
        }
        let danger = SettingsSection::new(t!("Danger")).child(
            SettingsRow::new(t!("Remove project"))
                .description(t!("Deletes the project entry and its threads. Files on disk are not touched."))
                .control(
                    Button::new("project-remove")
                        .variant(ButtonVariant::DestructiveOutline)
                        .size(ButtonSize::Sm)
                        .icon(IconName::Trash2)
                        .label(t!("Remove project"))
                        .disabled(ro)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let input = ProjectMutation::ProjectDelete(t3code_contracts::ProjectMutationProjectDelete { command_id: command_id(), project_id: delete_id.clone(), force: None });
                            this.selected = None;
                            this.model.update(cx, |m, cx| {
                                m.run("project-mutate", move |s| s.mutate_project(input), |result, cx| {
                                    if let Err(e) = result {
                                        toast_error(t!("Failed to remove project"), e, cx);
                                    }
                                }, cx)
                            });
                        })),
                ),
        );
        page_container(cx).gap(theme.sp(6.)).child(alert).child(overview).child(actions).child(danger)
    }
}

impl Render for ProjectsSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.model.read(cx).snapshot().clone();
        let ro = snapshot.read_only;
        let projects: Vec<_> = snapshot.projects.iter().filter(|p| p.deleted_at.is_none()).cloned().collect();
        let page = match projects.iter().find(|p| Some(p.id.to_string()) == self.selected) {
            Some(project) => self.render_project(project, ro, window, cx),
            None => {
                self.selected = None;
                self.render_list(&projects, cx)
            }
        };
        let dialog = self.dialog.clone().map(|dialog| {
            let this = cx.entity().downgrade();
            Modal::dialog("add-project-to-machine", dialog, move |_, cx| {
                this.update(cx, |this, cx| {
                    this.dialog = None;
                    cx.notify();
                })
                .ok();
            })
        });
        div().relative().size_full().child(page).children(dialog)
    }
}

/// A `project.update` mutation for one project.
fn update(project_id: t3code_contracts::ProjectId, f: impl FnOnce(&mut ProjectMutationProjectUpdate)) -> ProjectMutationProjectUpdate {
    let mut u: ProjectMutationProjectUpdate = serde_json::from_value(serde_json::json!({
        "commandId": command_id(),
        "projectId": project_id,
    }))
    .expect("minimal project update");
    f(&mut u);
    u
}
