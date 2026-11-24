//! Logical project groups. Ports `sidebarProjectGrouping.ts` and the
//! `buildProjectGroups` half of `client-runtime/state/projectGrouping.ts`:
//! projects on different servers (or checkouts) of one repository share a group.

use std::collections::{BTreeMap, HashMap};

use gpui_kit::SharedString;

use super::model::{ProjectGroupingMode, ProjectRef, SidebarProject, TimestampMs};

/// Per-project grouping overrides keyed by [`physical_project_key`]
/// (`sidebarProjectGroupingOverrides`).
pub type GroupingOverrides = BTreeMap<String, ProjectGroupingMode>;

/// One sidebar project group (`SidebarProjectSnapshot`).
#[derive(Clone, Debug)]
pub struct ProjectGroup {
    /// The logical project key.
    pub key: SharedString,
    pub label: SharedString,
    pub workspace_root: SharedString,
    pub member_refs: Vec<ProjectRef>,
    pub pinned: bool,
    pub archived: bool,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
    /// The representative project's icon override and favicon.
    pub icon: Option<t3code_contracts::OrchestrationProjectShellProjectIcon>,
    pub favicon: Option<std::sync::Arc<std::path::Path>>,
}

/// `${environmentId}:${normalized workspaceRoot}`.
pub fn physical_project_key(project: &SidebarProject) -> String {
    let root = project.workspace_root.trim_end_matches(['/', '\\']);
    format!("{}:{}", project.project_ref.environment_id.0, root)
}

fn logical_project_key(project: &SidebarProject, mode: ProjectGroupingMode) -> String {
    if mode == ProjectGroupingMode::Separate {
        return physical_project_key(project);
    }
    let Some(repository) = project
        .repository_key
        .as_ref()
        .filter(|key| !key.contains('\0'))
    else {
        return physical_project_key(project);
    };
    match (mode, project.repository_relative_path.as_ref()) {
        (ProjectGroupingMode::RepositoryPath, Some(path)) if !path.is_empty() => {
            format!("{repository}::{path}")
        }
        _ => repository.to_string(),
    }
}

/// The fresher of two copies of one physical project wins (`updatedAt`, then id).
fn fresher(candidate: &SidebarProject, existing: &SidebarProject) -> bool {
    let delta = candidate.updated_at.0 - existing.updated_at.0;
    delta > 0 || (delta == 0 && candidate.project_ref.project_id > existing.project_ref.project_id)
}

/// `deriveProjectGroupLabel`: a title every member shares wins when it was
/// customised or is not just the repository's name; otherwise the shared
/// repository display name, the shared repository name, and for a
/// whole-repository group the representative's repository name.
fn group_label(key: &str, representative: &SidebarProject, members: &[&SidebarProject]) -> SharedString {
    fn unique<'a>(values: impl Iterator<Item = Option<&'a SharedString>>) -> Vec<&'a SharedString> {
        let mut out: Vec<&SharedString> = Vec::new();
        for value in values.flatten().filter(|v| !v.trim().is_empty()) {
            if !out.contains(&value) {
                out.push(value);
            }
        }
        out
    }
    let titles = unique(members.iter().map(|m| Some(&m.title)));
    let display_names = unique(members.iter().map(|m| m.repository_display_name.as_ref()));
    let repository_names = unique(members.iter().map(|m| m.repository_name.as_ref()));
    if let [title] = titles[..]
        && (members.iter().any(|m| m.has_custom_title)
            || (!display_names.contains(&title) && !repository_names.contains(&title)))
    {
        return title.clone();
    }
    if let [name] = display_names[..] {
        return name.clone();
    }
    if let [name] = repository_names[..] {
        return name.clone();
    }
    if representative.repository_key.as_deref() == Some(key) {
        if let Some(name) = representative.repository_display_name.as_ref().or(representative.repository_name.as_ref()) {
            return name.clone();
        }
    }
    representative.title.clone()
}

/// Groups projects by logical key, in first-seen order. A project's own
/// entry in `overrides` replaces `mode` for it.
pub fn build_project_groups(
    projects: &[SidebarProject],
    mode: ProjectGroupingMode,
    overrides: &GroupingOverrides,
) -> Vec<ProjectGroup> {
    let mut winners: Vec<(String, &SidebarProject)> = Vec::new();
    let mut winner_ix: HashMap<String, usize> = HashMap::new();
    for project in projects {
        let physical = physical_project_key(project);
        match winner_ix.get(&physical) {
            Some(&ix) if fresher(project, winners[ix].1) => winners[ix].1 = project,
            Some(_) => {}
            None => {
                winner_ix.insert(physical.clone(), winners.len());
                winners.push((physical, project));
            }
        }
    }

    let mut groups: Vec<ProjectGroup> = Vec::new();
    let mut group_ix: HashMap<String, usize> = HashMap::new();
    let mut logical_by_physical: HashMap<String, String> = HashMap::new();
    for (physical, winner) in &winners {
        // A copy without an identity borrows one from a sibling checkout.
        let identity = if winner.repository_key.is_some() {
            *winner
        } else {
            projects
                .iter()
                .filter(|p| p.repository_key.is_some() && &physical_project_key(p) == physical)
                .max_by_key(|p| p.updated_at)
                .unwrap_or(winner)
        };
        let key = logical_project_key(identity, overrides.get(physical).copied().unwrap_or(mode));
        logical_by_physical.insert(physical.clone(), key.clone());
        match group_ix.get(&key) {
            Some(&ix) => {
                let group = &mut groups[ix];
                group.pinned |= winner.pinned;
                group.archived &= winner.archived;
                group.created_at = group.created_at.min(winner.created_at);
                group.updated_at = group.updated_at.max(winner.updated_at);
            }
            None => {
                group_ix.insert(key.clone(), groups.len());
                groups.push(ProjectGroup {
                    key: key.into(),
                    label: SharedString::default(),
                    workspace_root: winner.workspace_root.clone(),
                    member_refs: Vec::new(),
                    pinned: winner.pinned,
                    archived: winner.archived,
                    created_at: winner.created_at,
                    updated_at: winner.updated_at,
                    icon: winner.icon.clone(),
                    favicon: winner.favicon.clone(),
                });
            }
        }
    }
    let mut members: Vec<Vec<&SidebarProject>> = vec![Vec::new(); groups.len()];
    let mut representatives: Vec<Option<&SidebarProject>> = vec![None; groups.len()];
    for (physical, winner) in &winners {
        let ix = group_ix[&logical_by_physical[physical]];
        members[ix].push(winner);
        representatives[ix].get_or_insert(winner);
    }
    for project in projects {
        let key = &logical_by_physical[&physical_project_key(project)];
        let group = &mut groups[group_ix[key]];
        if !group.member_refs.contains(&project.project_ref) {
            group.member_refs.push(project.project_ref.clone());
        }
    }
    for (ix, group) in groups.iter_mut().enumerate() {
        if let Some(representative) = representatives[ix] {
            // The live layer names groups itself (`group_label`); fixtures derive it here.
            group.label = representative
                .group_label
                .clone()
                .unwrap_or_else(|| group_label(&group.key, representative, &members[ix]));
        }
    }
    groups
}
