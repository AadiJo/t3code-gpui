//! Project rows: the New thread in… page and project search results. Ports
//! `CommandPaletteProjectItems.tsx` and `buildProjectActionItems`.

use std::rc::Rc;

use super::command_palette_logic::{PaletteEvent, PaletteItem, PaletteProjectRow, PaletteRun, PaletteTitle, enumerate_items};
use super::command_palette_lookups::{PaletteEnvironment, PaletteProject};
use super::icons::IconName;
use super::sidebar::model::{EnvironmentId, ProjectRef};
use crate::i18n::t;
use t3code_shared::keybindings::StaticCommand;

fn project_item(prefix: &str, project: &PaletteProject, context: &ProjectContext) -> PaletteItem {
    let value = format!("{prefix}:{}:{}", project.project_ref.environment_id.0, project.project_ref.project_id.0);
    let mut item = PaletteItem::action(
        value,
        PaletteTitle::Text(project.title.clone()),
        IconName::Folder,
        PaletteRun::Event(PaletteEvent::NewThreadIn(project.project_ref.clone())),
        &[],
    );
    let location = context.environments.iter().find(|e| e.id == project.project_ref.environment_id).map(|e| e.label.clone());
    let mut terms = vec![project.title.clone(), project.workspace_root.clone()];
    terms.extend(location.clone());
    item.set_terms(terms);
    item.project = Some(PaletteProjectRow {
        project_ref: project.project_ref.clone(),
        name: project.title.clone(),
        location,
        workspace_root: project.workspace_root.clone(),
        current: context.current.as_ref() == Some(&project.project_ref),
        pinned: project.pinned,
    });
    item.description(project.workspace_root.clone())
}

/// Where the rows are shown from: the environments (for location labels)
/// and the current project.
pub struct ProjectContext<'a> {
    pub environments: &'a [PaletteEnvironment],
    pub current: Option<ProjectRef>,
}

/// The New thread in… page: pinned projects first, then the rest, with the
/// first nine numbered for the jump shortcuts. "No project" sits after the
/// pinned ones and the first unpinned one (`newThreadNoProjectIndex`).
pub fn new_thread_in_items(
    projects: &[PaletteProject],
    scratch_environment: Option<&EnvironmentId>,
    context: &ProjectContext,
) -> Vec<Rc<PaletteItem>> {
    let mut ordered: Vec<&PaletteProject> = projects.iter().collect();
    ordered.sort_by_key(|p| !p.pinned);
    let numbered = enumerate_items(ordered.iter().map(|p| project_item("new-thread-in", p, context)).collect());
    let mut items: Vec<Rc<PaletteItem>> = numbered.into_iter().map(Rc::new).collect();
    if let Some(environment) = scratch_environment {
        let pinned = ordered.iter().filter(|p| p.pinned).count();
        let at = (pinned + 1).min(items.len());
        let mut no_project = PaletteItem::action(
            "new-thread-in:no-project",
            PaletteTitle::Text(t!("No project")),
            IconName::MessageSquareDashed,
            PaletteRun::Event(PaletteEvent::NewThreadWithoutProject(environment.clone())),
            &["no project", "without project", "none"],
        )
        .shortcut(StaticCommand::ChatNewWithoutProject);
        no_project.set_terms(["no project", "without project", "none"].map(Into::into));
        items.insert(at, Rc::new(no_project));
    }
    items
}

/// Projects as root search results.
/// Search results carry no "Current".
pub fn project_search_items(projects: &[PaletteProject], environments: &[PaletteEnvironment]) -> Vec<Rc<PaletteItem>> {
    let context = ProjectContext { environments, current: None };
    projects.iter().map(|p| Rc::new(project_item("project", p, &context))).collect()
}
