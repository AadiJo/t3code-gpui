//! Settings → Worktrees. Ports `WorktreesSettings.tsx`, `BranchNamingSettings.tsx`
//! and `WorktreeCleanupSection` of `StorageSettings.tsx`.

use gpui_kit::{Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*};
use t3code_contracts::{
    BranchNamingMode, ClientServerSettingsPatch, ClientServerSettingsPatchStorageCleanup, WorktreeKeepWhen, WorktreeSubmodules,
};

use crate::components::ui::input::InputSize;
use super::server_settings_controls::{
    Fields, advanced_fold, bool_setting, field_input, lock_reason, retention_control, save_patch, select_row,
};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::components::managed_policy::managed_by;
use crate::i18n::t;

/// An unsized `Input` (`h-8`, the browser's 20-character text field width at the interface size).
const INPUT_WIDTH: gpui_kit::Rems = gpui_kit::rems(11.2);

/// The Worktrees page.
pub struct WorktreesSettingsPanel {
    model: Entity<ServerPanelsModel>,
    fields: Fields,
    creation_advanced: bool,
    cleanup_advanced: bool,
    _observe: Subscription,
}

impl WorktreesSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, fields: Fields::default(), creation_advanced: false, cleanup_advanced: false, _observe: observe }
    }
}

fn cleanup(f: impl FnOnce(&mut ClientServerSettingsPatchStorageCleanup)) -> ClientServerSettingsPatch {
    let mut c = ClientServerSettingsPatchStorageCleanup::default();
    f(&mut c);
    ClientServerSettingsPatch { storage_cleanup: Some(c), ..Default::default() }
}

impl Render for WorktreesSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let snapshot = self.model.read(cx).snapshot().clone();
        let s = &snapshot.settings;
        let policy = snapshot.managed_policy.as_deref();
        let ro = snapshot.read_only;
        let lock = |paths: &[&str]| lock_reason(managed_by(policy, paths), ro);
        let model = self.model.clone();

        let dir_model = model.clone();
        let dir = self.fields.draft("worktreesDirectory", s.worktrees_directory.clone().unwrap_or_default(), t!("Default"), false, window, cx, move |text, _, cx| {
            save_patch(&dir_model, "worktreesDirectory", ClientServerSettingsPatch { worktrees_directory: Some(text.trim().to_owned()), ..Default::default() }, cx);
        });
        let submodules = [WorktreeSubmodules::Recursive, WorktreeSubmodules::TopLevel, WorktreeSubmodules::NoneType];
        let sub_ix = submodules.iter().position(|m| Some(*m) == s.worktree_submodules).unwrap_or(0);
        let sub_model = model.clone();
        let this = cx.entity().downgrade();
        let creation_advanced = advanced_fold(
            "worktrees-creation-advanced",
            self.creation_advanced,
            move |_, cx| {
                this.update(cx, |this, cx| {
                    this.creation_advanced = !this.creation_advanced;
                    cx.notify();
                })
                .ok();
            },
            vec![select_row("worktreeSubmodules", t!("Submodules"), Some(t!("How new worktrees populate git submodules.")), vec![t!("Recursive"), t!("Top level only"), t!("Skip")], sub_ix, lock(&["worktreeSubmodules"]), move |ix, _, cx| {
                save_patch(&sub_model, "worktreeSubmodules", ClientServerSettingsPatch { worktree_submodules: Some(Some(submodules[ix])), ..Default::default() }, cx);
            })
            .into_any_element()],
            cx,
        );
        let creation = SettingsSection::new(t!("Creation", context = "settings section about creating worktrees"))
            .child(SettingsRow::new(t!("Worktree location")).description(t!("Folder where new worktrees are created, on any drive, such as D:\\worktrees or ~/worktrees. Existing worktrees stay where they are. Leave empty to use the T3 home folder.")).control(
                div().w(INPUT_WIDTH).child(field_input(&dir, false, false, ro || lock(&["worktreesDirectory"]).is_some()).size(InputSize::Default)),
            ))
            .child(bool_setting(&model, "newWorktreesStartFromOrigin", t!("Start from remote"), Some(t!("Uses the latest commit of the base branch’s tracked remote branch, even if its name differs. Otherwise uses the same-named branch on origin. Uses your local branch if that remote branch is missing.")), s.new_worktrees_start_from_origin.unwrap_or(true), lock(&["newWorktreesStartFromOrigin"]), |v| ClientServerSettingsPatch { new_worktrees_start_from_origin: Some(v), ..Default::default() }))
            .child(creation_advanced);

        let modes = [BranchNamingMode::Static, BranchNamingMode::Semantic, BranchNamingMode::Custom];
        let mode = s.branch_naming_mode.unwrap_or(BranchNamingMode::Static);
        let mode_ix = modes.iter().position(|m| *m == mode).unwrap_or(0);
        let mode_model = model.clone();
        let prefix_model = model.clone();
        let prefix = self.fields.draft("branchNamePrefix", s.branch_name_prefix.clone().unwrap_or_default(), t!("No prefix"), false, window, cx, move |text, _, cx| {
            save_patch(&prefix_model, "branchNamePrefix", ClientServerSettingsPatch { branch_name_prefix: Some(text.trim().to_owned()), ..Default::default() }, cx);
        });
        let mut naming = select_row("branchNamingMode", t!("Worktree branch naming"), Some(t!("Choose how new worktree branches are named from your first message.")), vec![t!("Static prefix"), t!("Semantic prefix"), t!("Custom instructions")], mode_ix, lock(&["branchNamingMode"]), move |ix, _, cx| {
            save_patch(&mode_model, "branchNamingMode", ClientServerSettingsPatch { branch_naming_mode: Some(modes[ix]), ..Default::default() }, cx);
        });
        if mode == BranchNamingMode::Semantic {
            naming = naming.status(t!("The model chooses a prefix that describes the work, such as feat/add-search, fix/login-timeout, or refactor/auth."));
        }
        let mut branches = SettingsSection::new(t!("Branches")).child(naming);
        match mode {
            BranchNamingMode::Custom => {
                let instr_model = model.clone();
                let instructions = self.fields.draft("branchNameInstructions", s.branch_name_instructions.clone().unwrap_or_default(), t!("Use julius/ followed by the issue ID and a short description."), false, window, cx, move |text, _, cx| {
                    save_patch(&instr_model, "branchNameInstructions", ClientServerSettingsPatch { branch_name_instructions: Some(text.trim().to_owned()), ..Default::default() }, cx);
                });
                branches = branches.child(SettingsRow::new(t!("Branch naming instructions")).description(t!("Appended to the naming prompt. The model returns the complete branch name; no prefix or suffix is added.")).control(div().w(theme.sp(64.)).child(field_input(&instructions, false, false, ro))));
            }
            BranchNamingMode::Static => {
                branches = branches.child(SettingsRow::new(t!("Branch prefix")).description(t!("For example, t3 or t3/ produces t3/add-search. Leave empty for no prefix.")).control(div().w(INPUT_WIDTH).child(field_input(&prefix, false, false, ro).size(InputSize::Default))));
            }
            _ => {}
        }

        let sc = s.storage_cleanup.clone().unwrap_or_default();
        let (inactive_model, settled_model, keep_model) = (model.clone(), model.clone(), model.clone());
        let keep_values = [WorktreeKeepWhen::AnyLocalFiles, WorktreeKeepWhen::UncommittedChanges, WorktreeKeepWhen::TrackedChanges];
        let keep = sc.worktree_keep_when.unwrap_or(WorktreeKeepWhen::UncommittedChanges);
        let keep_ix = keep_values.iter().position(|k| *k == keep).unwrap_or(1);
        let keep_description = match keep {
            WorktreeKeepWhen::AnyLocalFiles => t!("Includes ignored files, except node_modules."),
            WorktreeKeepWhen::TrackedChanges => t!("Deletes untracked and ignored files, including .env."),
            _ => t!("Keeps edits and untracked files. Deletes ignored files, including .env."),
        };
        let this = cx.entity().downgrade();
        let cleanup_advanced = advanced_fold(
            "worktrees-cleanup-advanced",
            self.cleanup_advanced,
            move |_, cx| {
                this.update(cx, |this, cx| {
                    this.cleanup_advanced = !this.cleanup_advanced;
                    cx.notify();
                })
                .ok();
            },
            vec![
                SettingsRow::new(t!("Delete settled worktrees"))
                    .description(t!("Remove worktrees this many days after their thread settles. Set 0 days to remove them at the next cleanup. Branches and thread history are kept."))
                    .control(retention_control("worktree-settled", sc.worktree_settled_after_days, 0, ro || lock(&["storageCleanup.worktreeSettledAfterDays"]).is_some(), move |v, _, cx| {
                        save_patch(&settled_model, "storageCleanup.worktreeSettledAfterDays", cleanup(|c| c.worktree_settled_after_days = Some(v)), cx);
                    }, cx))
                    .into_any_element(),
                bool_setting(&model, "storageCleanup.worktreeUnchanged", t!("Delete unchanged worktrees"), Some(t!("Remove worktrees with no commits beyond the default branch.")), sc.worktree_unchanged.unwrap_or(false), lock(&["storageCleanup.worktreeUnchanged"]), |v| cleanup(|c| c.worktree_unchanged = Some(v))).into_any_element(),
                select_row("storageCleanup.worktreeKeepWhen", t!("Keep worktrees with local changes"), Some(keep_description), vec![t!("Any local files"), t!("Uncommitted changes"), t!("Edited tracked files")], keep_ix, lock(&["storageCleanup.worktreeKeepWhen"]), move |ix, _, cx| {
                    save_patch(&keep_model, "storageCleanup.worktreeKeepWhen", cleanup(|c| c.worktree_keep_when = Some(keep_values[ix])), cx);
                })
                .into_any_element(),
            ],
            cx,
        );
        let cleanup_section = SettingsSection::new(t!("Cleanup"))
            .child(bool_setting(&model, "storageCleanup.worktreeOnDelete", t!("Delete worktrees with deleted threads"), Some(t!("Remove unused worktrees when active or archived threads are deleted. Worktrees with local changes are kept.")), sc.worktree_on_delete.unwrap_or(true), lock(&["storageCleanup.worktreeOnDelete"]), |v| cleanup(|c| c.worktree_on_delete = Some(v))))
            .child(bool_setting(&model, "storageCleanup.worktreeOnMerge", t!("Delete merged worktrees"), Some(t!("Remove worktrees whose pull request is merged and whose commits are included in the default branch.")), sc.worktree_on_merge.unwrap_or(false), lock(&["storageCleanup.worktreeOnMerge"]), |v| cleanup(|c| c.worktree_on_merge = Some(v))))
            .child(SettingsRow::new(t!("Delete inactive worktrees")).description(t!("Remove worktrees after their threads have been inactive for this many days. Branches and thread history are kept.")).control(retention_control("worktree-inactive", sc.worktree_after_days, 1, ro || lock(&["storageCleanup.worktreeAfterDays"]).is_some(), move |v, _, cx| {
                save_patch(&inactive_model, "storageCleanup.worktreeAfterDays", cleanup(|c| c.worktree_after_days = Some(v)), cx);
            }, cx)))
            .child(cleanup_advanced);

        page_container(cx).child(creation).child(branches).child(cleanup_section)
    }
}
