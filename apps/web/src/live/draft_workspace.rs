//! Where a draft's first send runs: the project root, a new worktree from a base branch, or an
//! existing worktree. Ports the draft side of `useComposerSelection.onEnvModeChange`,
//! `BranchToolbarBranchSelector.setThreadBranch`, `resolveNewThreadEnvMode` and the launch
//! strategy `startThreadTurn` builds from `planComposerTurn`.

use gpui_kit::App;
use t3code_contracts::{
    EnvironmentId, OrchestrationV2ThreadLaunchWorkspaceStrategy as Strategy, OrchestrationV2ThreadLaunchWorkspaceStrategyExistingWorktree,
    OrchestrationV2ThreadLaunchWorkspaceStrategyRoot, OrchestrationV2ThreadLaunchWorkspaceStrategyWorktree, ProjectId, ServerSettings,
    ThreadEnvMode,
};

use crate::components::branch_toolbar::BranchToolbarEvent;
use crate::components::branch_toolbar_logic::{EnvMode, resolve_draft_env_mode_after_branch_change};
use crate::state::StateBridge;

/// A draft's workspace choice, as the web's draft thread keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DraftWorkspace {
    pub env_mode: EnvMode,
    /// The checkout's branch, or a new worktree's base.
    pub branch: Option<String>,
    /// An existing worktree the draft runs in.
    pub worktree_path: Option<String>,
    pub start_from_origin: bool,
}

/// The first send in New worktree mode has no base branch to start from.
#[derive(Debug)]
pub struct MissingBaseBranch;

/// The project's workspace settings: server settings under the project's overrides
/// (`resolveProjectOverrides`). The `t3.json` fallback is read by [`project_file_env_mode`].
struct ProjectWorkspaceSettings {
    env_mode: Option<ThreadEnvMode>,
    /// Whether the env mode came from the environment's own setting (not a project override).
    env_mode_from_environment: bool,
    start_from_origin: bool,
}

fn project_settings(settings: &ServerSettings, project: &ProjectId, project_mode: Option<ThreadEnvMode>) -> ProjectWorkspaceSettings {
    let stored = settings.project_settings_overrides.as_ref().and_then(|o| o.get(project.as_str()));
    let folded = settings.project_settings_folded == Some(true);
    let override_mode = stored.and_then(|o| o.default_thread_env_mode.clone()).or(if folded { None } else { project_mode });
    ProjectWorkspaceSettings {
        env_mode_from_environment: override_mode.is_none(),
        env_mode: override_mode.or_else(|| settings.default_thread_env_mode.clone()),
        start_from_origin: stored
            .and_then(|o| o.new_worktrees_start_from_origin)
            .or(settings.new_worktrees_start_from_origin)
            .unwrap_or(true),
    }
}

/// Whether `workspace_root` is a folder directly inside `new_projects_root` (`resolveNewThreadEnvMode`).
fn is_new_project_folder(new_projects_root: &str, workspace_root: &str) -> bool {
    let separator = if new_projects_root.contains('\\') { '\\' } else { '/' };
    let prefix = if new_projects_root.ends_with(separator) { new_projects_root.to_owned() } else { format!("{new_projects_root}{separator}") };
    workspace_root.len() > prefix.len() && workspace_root.starts_with(&prefix) && !workspace_root[prefix.len()..].contains(separator)
}

/// `T3_PROJECT_FILE_NAME`: the repository's shared T3 Code settings at the workspace root.
pub const PROJECT_FILE: &str = "t3.json";

/// The workspace root whose `t3.json` decides a new draft's mode: only when neither the project
/// nor the environment sets one, and the environment is connected (`consultProjectFile`).
pub fn project_file_root(environment_id: &EnvironmentId, project: &ProjectId, cx: &App) -> Option<String> {
    let environment = StateBridge::environment(environment_id, cx)?;
    let environment = environment.read(cx);
    let config = environment.config.as_ref()?;
    environment.store.session()?;
    let shell_project = environment.shell.snapshot.as_ref()?.projects.iter().find(|p| &p.id == project)?;
    let settings = project_settings(&config.settings, project, shell_project.default_thread_env_mode.clone());
    settings.env_mode.is_none().then(|| shell_project.workspace_root.clone())
}

/// The `defaultThreadEnvMode` of a `t3.json` (`parseT3ProjectFile`); `None` for a file that is
/// not a JSON object or holds another value there.
pub fn project_file_env_mode(contents: &str) -> Option<ThreadEnvMode> {
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    match value.as_object()?.get("defaultThreadEnvMode")?.as_str()? {
        "worktree" => Some(ThreadEnvMode::Worktree),
        "local" => Some(ThreadEnvMode::Local),
        _ => None,
    }
}

impl DraftWorkspace {
    /// The workspace a `t3.json` mode gives a draft whose project and environment set none. A
    /// mode from the file is not overridden by the new-project-folder rule.
    pub fn from_project_file(mode: ThreadEnvMode, environment_id: &EnvironmentId, project: &ProjectId, cx: &App) -> Self {
        let start_from_origin = StateBridge::environment(environment_id, cx)
            .and_then(|env| env.read(cx).config.as_ref().map(|c| project_settings(&c.settings, project, None).start_from_origin))
            .unwrap_or(true);
        let env_mode = if mode == ThreadEnvMode::Worktree { EnvMode::Worktree } else { EnvMode::Local };
        Self { env_mode, start_from_origin: env_mode == EnvMode::Worktree && start_from_origin, ..Self::default() }
    }

    /// A new draft's workspace in `project` (`useHandleNewThread.resolveDefaultEnvMode`).
    pub fn new(environment_id: &EnvironmentId, project: &ProjectId, cx: &App) -> Self {
        let Some(environment) = StateBridge::environment(environment_id, cx) else { return Self::default() };
        let environment = environment.read(cx);
        let Some(config) = environment.config.as_ref() else { return Self::default() };
        let snapshot = environment.shell.snapshot.as_ref();
        let shell_project = snapshot.and_then(|s| s.projects.iter().find(|p| &p.id == project).cloned());
        let settings = project_settings(&config.settings, project, shell_project.as_ref().and_then(|p| p.default_thread_env_mode.clone()));
        let has_threads = snapshot.is_some_and(|s| s.threads.iter().any(|t| &t.project_id == project));
        let configured = settings.env_mode.clone().unwrap_or(ThreadEnvMode::Local);
        let new_folder = settings.env_mode_from_environment
            && !has_threads
            && configured != ThreadEnvMode::Local
            && shell_project
                .as_ref()
                .zip(config.new_projects_root.as_deref())
                .is_some_and(|(p, root)| is_new_project_folder(root, &p.workspace_root));
        let env_mode = if configured == ThreadEnvMode::Worktree && !new_folder { EnvMode::Worktree } else { EnvMode::Local };
        Self { env_mode, start_from_origin: env_mode == EnvMode::Worktree && settings.start_from_origin, ..Self::default() }
    }

    /// Applies a branch toolbar event the way the web writes it to its draft thread.
    /// `previous_seed` is the "Previous worktree" the toolbar offered, for `UsePreviousWorktree`.
    pub fn apply(
        &mut self,
        event: &BranchToolbarEvent,
        environment_id: &EnvironmentId,
        project: &ProjectId,
        previous_seed: Option<crate::components::branch_toolbar_logic::PreviousWorktreeSeed>,
        cx: &App,
    ) {
        match event {
            BranchToolbarEvent::EnvModeChanged(mode) => {
                let start_from_origin = StateBridge::environment(environment_id, cx)
                    .and_then(|env| env.read(cx).config.as_ref().map(|c| project_settings(&c.settings, project, None).start_from_origin))
                    .unwrap_or(true);
                self.env_mode = *mode;
                self.start_from_origin = *mode == EnvMode::Worktree && start_from_origin;
                if *mode == EnvMode::Worktree {
                    self.worktree_path = None;
                }
            }
            BranchToolbarEvent::StartFromOriginChanged(value) => self.start_from_origin = *value,
            BranchToolbarEvent::UsePreviousWorktree => {
                // The toolbar offers the seed it was given; read it again from the same shells.
                if let Some(seed) = previous_seed {
                    self.branch = seed.branch;
                    self.worktree_path = Some(seed.worktree_path);
                    self.env_mode = EnvMode::Worktree;
                }
            }
            BranchToolbarEvent::ThreadBranchChanged { branch, worktree_path } => {
                self.env_mode =
                    resolve_draft_env_mode_after_branch_change(worktree_path.as_deref(), self.worktree_path.as_deref(), self.env_mode);
                self.branch = Some(branch.clone());
                self.worktree_path = worktree_path.clone();
            }
            _ => {}
        }
    }

    /// The launch's workspace strategy (`startThreadTurn` over `planComposerTurn`).
    pub fn strategy(&self) -> Result<Strategy, MissingBaseBranch> {
        if let Some(worktree_path) = &self.worktree_path {
            return Ok(Strategy::ExistingWorktree(OrchestrationV2ThreadLaunchWorkspaceStrategyExistingWorktree {
                worktree_path: worktree_path.clone(),
                branch: self.branch.clone(),
            }));
        }
        if self.env_mode == EnvMode::Worktree {
            let base_ref = self.branch.clone().ok_or(MissingBaseBranch)?;
            return Ok(Strategy::Worktree(OrchestrationV2ThreadLaunchWorkspaceStrategyWorktree {
                base_ref,
                branch: None,
                start_from_origin: self.start_from_origin.then_some(true),
            }));
        }
        Ok(Strategy::Root(OrchestrationV2ThreadLaunchWorkspaceStrategyRoot { branch: self.branch.clone() }))
    }
}
