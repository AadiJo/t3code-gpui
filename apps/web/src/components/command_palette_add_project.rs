//! The Add project pages. Ports the entry of `CommandPaletteAddProject.tsx`
//! (`resolveAddProjectEntry`): connect an environment, pick one, or pick a
//! source. The folder browser, clone and new-project flows then run in the palette
//! (`command_palette_project_flow`).

use std::rc::Rc;

use gpui_kit::SharedString;

use super::command_palette_logic::{AddProjectStep, PaletteEvent, PaletteGroup, PaletteItem, PaletteRun, PaletteTitle, PaletteView};
use super::command_palette_lookups::PaletteEnvironment;
use super::icons::IconName;
use super::sidebar::model::EnvironmentId;
use crate::i18n::t;

/// An environment's Sources page (`buildAddProjectSourceGroups`). `can_create_new` says whether
/// its server reports a `newProjectsRoot`.
pub fn sources_view(environment: &EnvironmentId, can_create_new: bool) -> PaletteView {
    let source = |value: String, title, description, icon, step, terms: &[&str]| {
        let mut item = PaletteItem::action(value, PaletteTitle::Text(title), icon, PaletteRun::Event(PaletteEvent::AddProject(step)), &[]);
        item.set_terms(terms.iter().map(|t| SharedString::from(*t)));
        Rc::new(item.description(description).keep_open())
    };
    let id = &environment.0;
    let mut items = Vec::new();
    if can_create_new {
        items.push(source(
            format!("action:add-project:{id}:new"),
            t!("New project"),
            t!("Start a new Git repository from a name"),
            IconName::FolderGit2,
            AddProjectStep::NewProject(environment.clone()),
            &["new project", "create", "empty", "repository", "git init"],
        ));
    }
    items.push(source(
        format!("action:add-project:{id}:local"),
        t!("Local folder"),
        t!("Browse a folder on disk"),
        IconName::FolderPlus,
        AddProjectStep::BrowseFolders(environment.clone()),
        &["local", "folder", "directory", "browse"],
    ));
    items.push(source(
        format!("action:add-project:{id}:url"),
        t!("Git URL"),
        t!("Clone from a remote URL"),
        IconName::Link,
        AddProjectStep::CloneRepository(environment.clone()),
        &["clone", "remote", "repository", "repo", "git", "Git URL"],
    ));
    PaletteView {
        addon_icon: IconName::FolderPlus,
        groups: vec![PaletteGroup { value: format!("sources:{id}").into(), label: t!("Sources"), items }],
        initial_query: None,
    }
}

/// The Add project row: its page depends on how many environments can take a project.
pub fn add_project_item(environments: &[PaletteEnvironment], can_create_new: &dyn Fn(&EnvironmentId) -> bool) -> PaletteItem {
    let connected: Vec<&PaletteEnvironment> = environments.iter().filter(|e| e.connected).collect();
    let view = match connected.as_slice() {
        [] => {
            let mut item = PaletteItem::action(
                "action:add-project:connect-environment",
                PaletteTitle::Text(t!("Connect an environment")),
                IconName::Server,
                PaletteRun::Event(PaletteEvent::AddProject(AddProjectStep::ConnectEnvironment)),
                &[],
            );
            item.set_terms([item.search_name()]);
            let item = item.description(t!("No connected environment can add a project"));
            PaletteView {
                addon_icon: IconName::FolderPlus,
                groups: vec![PaletteGroup { value: "add-project".into(), label: Default::default(), items: vec![Rc::new(item)] }],
                initial_query: None,
            }
        }
        [only] => sources_view(&only.id, can_create_new(&only.id)),
        many => PaletteView {
            addon_icon: IconName::FolderPlus,
            groups: vec![PaletteGroup {
                value: "environments".into(),
                label: t!("Environments"),
                items: many
                    .iter()
                    .map(|environment| {
                        let mut item = PaletteItem::submenu(
                            format!("sources:{}", environment.id.0),
                            environment.label.clone(),
                            IconName::Server,
                            sources_view(&environment.id, can_create_new(&environment.id)),
                            &[],
                        );
                        item.set_terms([environment.label.clone()]);
                        Rc::new(item)
                    })
                    .collect(),
            }],
            initial_query: None,
        },
    };
    let mut item = PaletteItem::submenu("action:add-project", t!("Add project"), IconName::FolderPlus, view, &[]);
    item.set_terms(
        ["add project", "folder", "directory", "browse", "clone", "remote", "repository", "repo", "git", "url", "environment"]
            .map(Into::into),
    );
    item
}
