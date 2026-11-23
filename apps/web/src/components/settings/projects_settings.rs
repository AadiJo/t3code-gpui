//! Settings → Projects. Ports `ProjectsSettings.tsx` (the project list) and
//! the rows of `ProjectSettingsPanel.tsx` for one project: Name, Project icon
//! (monogram and colour), Pin, Archive and its actions (scripts), each sent
//! with `projects.mutate`. "Add to another machine" opens
//! [`AddProjectToMachineDialog`].

use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{CommandId, ProjectMutation, ProjectMutationProjectUpdate};

use super::provider_settings_panel::toast_error;
use super::server_settings_controls::{Fields, field_input, hint};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::add_project_to_machine_dialog::{AddProjectToMachineDialog, AddProjectToMachineEvent};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::components::ui::switch::Switch;
use crate::i18n::t;
use crate::project_identity::derive_project_identity;
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

impl Render for ProjectsSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let ro = snapshot.read_only;
        let projects: Vec<_> = snapshot.projects.iter().filter(|p| p.deleted_at.is_none()).cloned().collect();
        if self.selected.is_none() {
            self.selected = projects.first().map(|p| p.id.to_string());
        }
        let mut list = SettingsSection::new(t!("Projects"));
        for project in &projects {
            let identity = derive_project_identity(&project.title);
            let id = project.id.to_string();
            let selected = self.selected.as_deref() == Some(id.as_str());
            list.extend([div()
                .id(SharedString::from(format!("project-row-{id}")))
                .flex()
                .items_center()
                .gap(theme.sp(3.))
                .px(theme.sp(4.))
                .py(theme.sp(2.5))
                .cursor_pointer()
                .when(selected, |d| d.bg(with_alpha(c.foreground(), 0.06)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                }))
                .child(
                    div()
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme.radii.md)
                        .bg(with_alpha(c.muted, 0.8))
                        .ui_text(theme, TextSize::Xs2, weight::SEMIBOLD)
                        .child(identity.monogram),
                )
                .child(div().flex_1().min_w_0().flex().flex_col().child(div().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(project.title.clone())).child(hint(project.workspace_root.clone(), cx)))
                .when(project.pinned_at.is_some(), |d| d.child(hint(t!("Pinned"), cx)))
                .into_any_element()]);
        }
        if projects.is_empty() {
            list.extend([SettingsRow::new(t!("No projects")).into_any_element()]);
        }
        let mut page = page_container(cx).child(list);
        if let Some(project) = projects.iter().find(|p| Some(p.id.to_string()) == self.selected) {
            let pid = project.id.clone();
            let name = self.fields.get(format!("{}:name", project.id), project.title.clone(), "", false, window, cx);
            let name_read = name.clone();
            let (rename_id, pin_id, archive_id) = (pid.clone(), pid.clone(), pid.clone());
            let pinned = project.pinned_at.is_some();
            let archived = project.archived_at.is_some();
            let this = cx.entity().downgrade();
            let this_pin = this.clone();
            let detail = SettingsSection::new(t!("Project"))
                .child(SettingsRow::new(t!("Name")).description(t!("The shared name for this project group in the sidebar and thread lists.")).control(
                    div().flex().items_center().gap(theme.sp(2.)).child(div().w(theme.sp(56.)).child(field_input(&name, false, false, ro))).child(
                        Button::new("project-rename").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Save")).disabled(ro).on_click(cx.listener(move |this, _, _, cx| {
                            let title = name_read.read(cx).value().trim().to_owned();
                            this.update_project(update(rename_id.clone(), |u| u.title = Some(title)), cx);
                        })),
                    ),
                ))
                .child(SettingsRow::new(t!("Pin")).description(t!("Pinned projects come first in project pickers.")).control(Switch::new("project-pin", pinned).disabled(ro).on_change(move |v, _, cx| {
                    let id = pin_id.clone();
                    this_pin.update(cx, |this, cx| this.update_project(update(id, |u| u.pinned = Some(v)), cx)).ok();
                })))
                .child(SettingsRow::new(t!("Archive")).description(t!("Archived projects leave the sidebar. Their threads are kept.")).control(
                    Button::new("project-archive").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(if archived { t!("Unarchive") } else { t!("Archive") }).disabled(ro).on_click(cx.listener(move |this, _, _, cx| {
                        this.update_project(update(archive_id.clone(), |u| u.archived = Some(!archived)), cx);
                    })),
                ))
                .child(SettingsRow::new(t!("Add to another machine")).description(t!("Clone this project onto another environment.")).control(
                    Button::new("project-add-to-machine").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Add to another machine")).disabled(ro).on_click(cx.listener(|this, _, window, cx| this.open_add_to_machine(window, cx))),
                ));
            let mut actions = SettingsSection::new(t!("Actions"));
            for script in &project.scripts {
                actions.extend([SettingsRow::new(SharedString::from(script.name.clone())).description(SharedString::from(script.command.clone())).into_any_element()]);
            }
            if project.scripts.is_empty() {
                actions.extend([SettingsRow::new(t!("No actions")).into_any_element()]);
            }
            page = page.child(detail).child(actions);
        }
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
