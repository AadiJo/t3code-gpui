//! Settings → Worktrees. Ports `WorktreesSettings.tsx`, `BranchNamingSettings.tsx`
//! and the worktree cleanup section of `StorageSettings.tsx`.

use gpui_kit::{Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*};
use t3code_contracts::{
    BranchNamingMode, ClientServerSettingsPatch, ClientServerSettingsPatchStorageCleanup, WorktreeSubmodules,
};

use super::server_settings_controls::{
    Fields, bool_setting, field_input, interval_seconds_field, lock_reason, save_patch, select_row, switch_row,
};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::components::managed_policy::managed_by;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;

/// The Worktrees page.
pub struct WorktreesSettingsPanel {
    model: Entity<ServerPanelsModel>,
    fields: Fields,
    _observe: Subscription,
}

impl WorktreesSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, fields: Fields::default(), _observe: observe }
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

        let dir = self.fields.get("worktreesDirectory", s.worktrees_directory.clone().unwrap_or_default(), "~/worktrees", false, window, cx);
        let dir_read = dir.clone();
        let dir_model = model.clone();
        let submodules = [WorktreeSubmodules::Recursive, WorktreeSubmodules::TopLevel, WorktreeSubmodules::NoneType];
        let sub_ix = submodules.iter().position(|m| Some(*m) == s.worktree_submodules).unwrap_or(0);
        let sub_model = model.clone();
        let creation = SettingsSection::new(t!("Creation", context = "settings section about creating worktrees"))
            .child(SettingsRow::new(t!("Worktree location")).description(t!("Folder where new worktrees are created, on any drive, such as D:\\worktrees or ~/worktrees. Existing worktrees stay where they are. Leave empty to use the T3 home folder.")).control(
                div().flex().items_center().gap(theme.sp(2.)).child(div().w(theme.sp(56.)).child(field_input(&dir, true, false, ro || lock(&["worktreesDirectory"]).is_some()))).child(
                    Button::new("worktrees-dir-save").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Save")).disabled(ro).on_click(move |_, _, cx| {
                        let value = dir_read.read(cx).value().trim().to_owned();
                        save_patch(&dir_model, "worktreesDirectory", ClientServerSettingsPatch { worktrees_directory: Some(value), ..Default::default() }, cx);
                    }),
                ),
            ))
            .child(bool_setting(&model, "newWorktreesStartFromOrigin", t!("Start from remote"), Some(t!("Uses the latest commit of the base branch’s tracked remote branch, even if its name differs. Otherwise uses the same-named branch on origin. Uses your local branch if that remote branch is missing.")), s.new_worktrees_start_from_origin.unwrap_or(true), lock(&["newWorktreesStartFromOrigin"]), |v| ClientServerSettingsPatch { new_worktrees_start_from_origin: Some(v), ..Default::default() }))
            .child(select_row("worktreeSubmodules", t!("Submodules"), Some(t!("How new worktrees populate git submodules.")), vec![t!("Recursive"), t!("Top level only"), t!("Skip")], sub_ix, lock(&["worktreeSubmodules"]), move |ix, _, cx| {
                save_patch(&sub_model, "worktreeSubmodules", ClientServerSettingsPatch { worktree_submodules: Some(Some(submodules[ix])), ..Default::default() }, cx);
            }));

        let modes = [BranchNamingMode::Static, BranchNamingMode::Semantic, BranchNamingMode::Custom];
        let mode_ix = modes.iter().position(|m| Some(*m) == s.branch_naming_mode).unwrap_or(1);
        let mode_model = model.clone();
        let prefix = self.fields.get("branchNamePrefix", s.branch_name_prefix.clone().unwrap_or_default(), t!("No prefix"), false, window, cx);
        let prefix_read = prefix.clone();
        let prefix_model = model.clone();
        let mut branches = SettingsSection::new(t!("Branches"))
            .child(select_row("branchNamingMode", t!("Worktree branch naming"), Some(t!("Choose how new worktree branches are named from your first message.")), vec![t!("Static"), t!("Semantic"), t!("Custom")], mode_ix, lock(&["branchNamingMode"]), move |ix, _, cx| {
                save_patch(&mode_model, "branchNamingMode", ClientServerSettingsPatch { branch_naming_mode: Some(modes[ix]), ..Default::default() }, cx);
            }))
            .child(SettingsRow::new(t!("Branch prefix")).control(
                div().flex().items_center().gap(theme.sp(2.)).child(div().w(theme.sp(40.)).child(field_input(&prefix, true, false, ro))).child(
                    Button::new("branch-prefix-save").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Save")).disabled(ro).on_click(move |_, _, cx| {
                        let value = prefix_read.read(cx).value().trim().to_owned();
                        save_patch(&prefix_model, "branchNamePrefix", ClientServerSettingsPatch { branch_name_prefix: Some(value), ..Default::default() }, cx);
                    }),
                ),
            ));
        if s.branch_naming_mode == Some(BranchNamingMode::Custom) {
            let instructions = self.fields.get("branchNameInstructions", s.branch_name_instructions.clone().unwrap_or_default(), t!("Use julius/ followed by the issue ID and a short description."), false, window, cx);
            let read = instructions.clone();
            let instr_model = model.clone();
            branches = branches.child(SettingsRow::new(t!("Branch naming instructions")).control(
                div().flex().items_center().gap(theme.sp(2.)).child(div().w(theme.sp(64.)).child(field_input(&instructions, false, false, ro))).child(
                    Button::new("branch-instructions-save").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Save")).disabled(ro).on_click(move |_, _, cx| {
                        let value = read.read(cx).value().trim().to_owned();
                        save_patch(&instr_model, "branchNameInstructions", ClientServerSettingsPatch { branch_name_instructions: Some(value), ..Default::default() }, cx);
                    }),
                ),
            ));
        }

        let sc = s.storage_cleanup.clone().unwrap_or_default();
        let after_days = sc.worktree_after_days;
        let settled_days = sc.worktree_settled_after_days;
        let (inactive_model, inactive_days_model, settled_model, settled_days_model) = (model.clone(), model.clone(), model.clone(), model.clone());
        let mut cleanup_section = SettingsSection::new(t!("Worktree cleanup"))
            .child(bool_setting(&model, "storageCleanup.worktreeOnDelete", t!("Delete worktrees with deleted threads"), Some(t!("Remove unused worktrees when active or archived threads are deleted. Worktrees with local changes are kept.")), sc.worktree_on_delete.unwrap_or(true), lock(&["storageCleanup.worktreeOnDelete"]), |v| cleanup(|c| c.worktree_on_delete = Some(v))))
            .child(bool_setting(&model, "storageCleanup.worktreeOnMerge", t!("Delete merged worktrees"), Some(t!("Remove worktrees whose pull request is merged and whose commits are included in the default branch.")), sc.worktree_on_merge.unwrap_or(false), lock(&["storageCleanup.worktreeOnMerge"]), |v| cleanup(|c| c.worktree_on_merge = Some(v))))
            .child(switch_row("storageCleanup.worktreeAfterDays", t!("Delete inactive worktrees"), Some(t!("Remove worktrees after their threads have been inactive for this many days. Branches and thread history are kept.")), after_days.is_some(), lock(&["storageCleanup.worktreeAfterDays"]), move |v, _, cx| {
                save_patch(&inactive_model, "storageCleanup.worktreeAfterDays", cleanup(|c| c.worktree_after_days = v.then_some(30)), cx);
            }));
        if let Some(days) = after_days {
            cleanup_section = cleanup_section.child(SettingsRow::new(t!("Days")).control(interval_seconds_field("worktree-after-days", days, 1, 1, ro, move |d, _, cx| {
                save_patch(&inactive_days_model, "storageCleanup.worktreeAfterDays", cleanup(|c| c.worktree_after_days = Some(d)), cx);
            }, cx)));
        }
        cleanup_section = cleanup_section.child(switch_row("storageCleanup.worktreeSettledAfterDays", t!("Delete settled worktrees"), Some(t!("Remove worktrees this many days after their thread settles. Set 0 days to remove them at the next cleanup. Branches and thread history are kept.")), settled_days.is_some(), lock(&["storageCleanup.worktreeSettledAfterDays"]), move |v, _, cx| {
            save_patch(&settled_model, "storageCleanup.worktreeSettledAfterDays", cleanup(|c| c.worktree_settled_after_days = v.then_some(7)), cx);
        }));
        if let Some(days) = settled_days {
            cleanup_section = cleanup_section.child(SettingsRow::new(t!("Days")).control(interval_seconds_field("worktree-settled-days", days, 0, 1, ro, move |d, _, cx| {
                save_patch(&settled_days_model, "storageCleanup.worktreeSettledAfterDays", cleanup(|c| c.worktree_settled_after_days = Some(d)), cx);
            }, cx)));
        }
        cleanup_section = cleanup_section.child(bool_setting(&model, "storageCleanup.worktreeUnchanged", t!("Delete unchanged worktrees"), Some(t!("Remove worktrees with no commits beyond the default branch.")), sc.worktree_unchanged.unwrap_or(false), lock(&["storageCleanup.worktreeUnchanged"]), |v| cleanup(|c| c.worktree_unchanged = Some(v))));

        page_container(cx).child(creation).child(branches).child(cleanup_section)
    }
}
