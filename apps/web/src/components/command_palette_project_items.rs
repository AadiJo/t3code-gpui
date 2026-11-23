//! Project rows: the New thread in… page and project search results. Ports
//! `CommandPaletteProjectItems.tsx` and `buildProjectActionItems`.

use std::rc::Rc;

use super::command_palette_logic::{PaletteEvent, PaletteItem, PaletteRun, PaletteTitle, enumerate_items};
use super::command_palette_lookups::PaletteProject;
use super::icons::IconName;
use super::sidebar::model::EnvironmentId;
use crate::i18n::t;
use t3code_shared::keybindings::StaticCommand;

fn project_item(prefix: &str, project: &PaletteProject) -> PaletteItem {
    let value = format!("{prefix}:{}:{}", project.project_ref.environment_id.0, project.project_ref.project_id.0);
    let mut item = PaletteItem::action(
        value,
        PaletteTitle::Text(project.title.clone()),
        IconName::Folder,
        PaletteRun::Event(PaletteEvent::NewThreadIn(project.project_ref.clone())),
        &[],
    );
    item.set_terms([project.title.clone(), project.workspace_root.clone()]);
    item.description(project.workspace_root.clone())
}

/// The New thread in… page: pinned projects first, then the rest, with the
/// first nine numbered for the jump shortcuts. "No project" sits after the
/// pinned ones and the first unpinned one (`newThreadNoProjectIndex`).
pub fn new_thread_in_items(projects: &[PaletteProject], scratch_environment: Option<&EnvironmentId>) -> Vec<Rc<PaletteItem>> {
    let mut ordered: Vec<&PaletteProject> = projects.iter().collect();
    ordered.sort_by_key(|p| !p.pinned);
    let numbered = enumerate_items(ordered.iter().map(|p| project_item("new-thread-in", p)).collect());
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
pub fn project_search_items(projects: &[PaletteProject]) -> Vec<Rc<PaletteItem>> {
    projects.iter().map(|p| Rc::new(project_item("project", p))).collect()
}
