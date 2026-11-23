//! Project scripts ("actions"): building one from the editor's input, ids,
//! keybinding commands, and which script the row runs. Ports `projectScripts.ts`
//! and `projectScriptMenuLabel` from `packages/shared/src/projectScriptsCopy.ts`.

use gpui_kit::SharedString;
use t3code_contracts::{ProjectScript, ProjectScriptIcon};

use crate::i18n::t;

/// `MAX_SCRIPT_ID_LENGTH` from `contracts/src/keybindings.ts`.
pub const MAX_SCRIPT_ID_LENGTH: usize = 24;

/// What the script editor dialog collects (`ProjectScriptInput`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectScriptInput {
    pub name: String,
    pub command: String,
    pub icon: ProjectScriptIcon,
    pub run_on_worktree_create: bool,
    pub wait_for_setup: bool,
    pub run_on_settle: bool,
    pub stop_on_settle: bool,
    pub preview_url: Option<String>,
    pub auto_open_preview: bool,
}

impl Default for ProjectScriptInput {
    /// `EMPTY_PROJECT_SCRIPT_INPUT`.
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            icon: ProjectScriptIcon::Play,
            run_on_worktree_create: false,
            wait_for_setup: false,
            run_on_settle: false,
            stop_on_settle: false,
            preview_url: None,
            auto_open_preview: false,
        }
    }
}

impl ProjectScriptInput {
    /// The editor's fields for an existing script (`editorRequestForScript`).
    pub fn from_script(script: &ProjectScript) -> Self {
        Self {
            name: script.name.clone(),
            command: script.command.clone(),
            icon: script.icon,
            run_on_worktree_create: script.run_on_worktree_create,
            wait_for_setup: script.run_on_worktree_create && script.r#async == Some(false),
            run_on_settle: script.run_on_settle == Some(true),
            stop_on_settle: script.stop_on_settle == Some(true),
            preview_url: script.preview_url.clone(),
            auto_open_preview: script.auto_open_preview == Some(true),
        }
    }
}

/// `buildProjectScript`: optional flags are only written when set.
pub fn build_project_script(id: String, input: &ProjectScriptInput) -> ProjectScript {
    ProjectScript {
        id,
        name: input.name.clone(),
        command: input.command.clone(),
        icon: input.icon,
        run_on_worktree_create: input.run_on_worktree_create,
        r#async: (input.run_on_worktree_create && input.wait_for_setup).then_some(false),
        run_on_settle: input.run_on_settle.then_some(true),
        stop_on_settle: input.stop_on_settle.then_some(true),
        auto_open_preview: input.preview_url.as_ref().map(|_| input.auto_open_preview),
        preview_url: input.preview_url.clone(),
    }
}

/// `releaseClaimedRoles`: one script per project may be the setup or the
/// on-settle script, so saving another with that role takes it from this one.
pub fn release_claimed_roles(script: &ProjectScript, saved: &ProjectScriptInput) -> ProjectScript {
    let release_setup = saved.run_on_worktree_create && script.run_on_worktree_create;
    let release_settle = saved.run_on_settle && script.run_on_settle == Some(true);
    let mut next = script.clone();
    if release_setup {
        next.run_on_worktree_create = false;
    }
    if release_settle {
        next.run_on_settle = Some(false);
    }
    next
}

fn normalize_script_id(value: &str) -> String {
    let mut cleaned = String::new();
    for ch in value.trim().to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            cleaned.push(ch);
        } else if !cleaned.ends_with('-') {
            cleaned.push('-');
        }
    }
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        return "script".to_owned();
    }
    if cleaned.len() <= MAX_SCRIPT_ID_LENGTH {
        return cleaned.to_owned();
    }
    let cut = cleaned[..MAX_SCRIPT_ID_LENGTH].trim_end_matches('-');
    if cut.is_empty() { "script".to_owned() } else { cut.to_owned() }
}

fn is_script_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SCRIPT_ID_LENGTH
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `commandForProjectScript`: the keybinding command that runs a script.
pub fn command_for_project_script(script_id: &str) -> Option<String> {
    is_script_id(script_id).then(|| format!("script.{script_id}.run"))
}

/// `projectScriptIdFromCommand`.
pub fn project_script_id_from_command(command: &str) -> Option<&str> {
    let id = command.trim().strip_prefix("script.")?.strip_suffix(".run")?;
    is_script_id(id).then_some(id)
}

/// `nextProjectScriptId`: the name as an id, suffixed `-2`, `-3`… when taken.
pub fn next_project_script_id<'a>(name: &str, existing_ids: impl IntoIterator<Item = &'a str>) -> String {
    let taken: std::collections::HashSet<&str> = existing_ids.into_iter().collect();
    let base = normalize_script_id(name);
    if !taken.contains(base.as_str()) {
        return base;
    }
    for suffix in 2..10_000u32 {
        let candidate = format!("{base}-{suffix}");
        let safe = if candidate.len() <= MAX_SCRIPT_ID_LENGTH {
            candidate
        } else {
            let keep = MAX_SCRIPT_ID_LENGTH.saturating_sub(suffix.to_string().len() + 1).max(1);
            format!("{}-{suffix}", &base[..keep.min(base.len())])
        };
        if !taken.contains(safe.as_str()) {
            return safe;
        }
    }
    base
}

/// `primaryProjectScript`: the script the row's main button runs. Setup and
/// on-settle scripts are only picked when nothing else exists.
pub fn primary_project_script(scripts: &[ProjectScript]) -> Option<&ProjectScript> {
    let on_settle = |s: &ProjectScript| s.run_on_settle == Some(true);
    scripts
        .iter()
        .find(|s| !s.run_on_worktree_create && !on_settle(s))
        .or_else(|| scripts.iter().find(|s| !on_settle(s)))
}

/// `projectScriptMenuLabel`: marks the setup and on-settle scripts.
pub fn project_script_menu_label(script: &ProjectScript) -> SharedString {
    let name = script.name.as_str();
    match (script.run_on_worktree_create, script.run_on_settle == Some(true)) {
        (true, true) => t!("{name} (setup, on settle)", name = name),
        (true, false) => t!("{name} (setup)", name = name),
        (false, true) => t!("{name} (on settle)", name = name),
        (false, false) => SharedString::from(name.to_owned()),
    }
}
