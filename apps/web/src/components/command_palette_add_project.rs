//! The Add project pages. Ports the entry of `CommandPaletteAddProject.tsx`
//! (`resolveAddProjectEntry`): connect an environment, pick one, or pick a
//! source. The folder browser and clone flows run outside the palette.

use std::rc::Rc;

use super::command_palette_logic::{AddProjectStep, PaletteEvent, PaletteGroup, PaletteItem, PaletteRun, PaletteTitle, PaletteView};
use super::command_palette_lookups::PaletteEnvironment;
use super::icons::IconName;
use super::sidebar::model::EnvironmentId;
use crate::i18n::t;

fn sources_view(environment: &EnvironmentId) -> PaletteView {
    let source = |value: &str, title, description, icon, step| {
        let mut item = PaletteItem::action(value, PaletteTitle::Text(title), icon, PaletteRun::Event(PaletteEvent::AddProject(step)), &[]);
        item.set_terms([item.search_name()]);
        Rc::new(item.description(description))
    };
    PaletteView {
        addon_icon: IconName::FolderPlus,
        groups: vec![PaletteGroup {
            value: format!("sources:{}", environment.0).into(),
            label: t!("Sources"),
            items: vec![
                source(
                    "add-project:folder",
                    t!("Local folder"),
                    t!("Browse a folder on disk"),
                    IconName::Folder,
                    AddProjectStep::BrowseFolders(environment.clone()),
                ),
                source(
                    "add-project:url",
                    t!("Git URL"),
                    t!("Clone from a remote URL"),
                    IconName::FolderGit2,
                    AddProjectStep::CloneRepository(environment.clone()),
                ),
            ],
        }],
        initial_query: None,
    }
}

/// The Add project row: its page depends on how many environments can take a project.
pub fn add_project_item(environments: &[PaletteEnvironment]) -> PaletteItem {
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
        [only] => sources_view(&only.id),
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
                            sources_view(&environment.id),
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
