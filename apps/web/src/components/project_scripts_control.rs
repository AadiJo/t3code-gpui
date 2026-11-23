//! The project scripts row of the thread details card. Ports
//! `ProjectScriptsControl.tsx` (panel layout) and `projectScriptEditor.tsx`:
//! a split row that runs the primary script, a menu of every script with its
//! setup / on-settle marks and an edit button, "Add project script", and the
//! add/edit dialog. Saving sends `projects.mutate` with the new script list;
//! running a script is the owner's (it opens a terminal).

use std::sync::Arc;

use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, TestSupportExt as _, Window, component::input::InputState, div, prelude::*, px,
};
use t3code_contracts::{
    CommandId, ProjectId, ProjectMutation, ProjectMutationProjectUpdate, ProjectScript, ProjectScriptIcon,
};

use super::chat::thread_details_panel_styles::{DismissPress, below_row, observed, row, row_base, row_icon, split_row, split_secondary};
use super::git_actions_control_source::{GitActionsSource, now_ms};
use super::icons::{Icon, IconName, IconSize};
use super::ui::button::{Button, ButtonSize, ButtonVariant};
use super::ui::dialog::{DialogSurface, Modal};
use super::ui::input::TextInput;
use super::ui::popover::glass_surface;
use super::ui::switch::Switch;
use super::ui::toast::{ToastData, ToastKind, Toaster};
use super::ui::tooltip::tooltip;
use super::ui::TextStyleExt as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::i18n::t;
use crate::project_scripts::{
    ProjectScriptInput, build_project_script, next_project_script_id, primary_project_script, project_script_menu_label,
    release_claimed_roles,
};
use crate::{ActiveAppTheme as _, AppTheme};

/// `ScriptIcon`.
pub fn script_icon(icon: ProjectScriptIcon) -> IconName {
    match icon {
        ProjectScriptIcon::Play => IconName::Play,
        ProjectScriptIcon::Test => IconName::FlaskConical,
        ProjectScriptIcon::Lint => IconName::ListChecks,
        ProjectScriptIcon::Configure => IconName::Wrench,
        ProjectScriptIcon::Build => IconName::Hammer,
        ProjectScriptIcon::Debug => IconName::Bug,
    }
}

/// What the row asks its owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectScriptsEvent {
    /// Run this script in a terminal.
    Run(ProjectScript),
}

/// The add/edit dialog while open.
struct EditorDialog {
    script_id: Option<String>,
    name: Entity<InputState>,
    command: Entity<InputState>,
    preview_url: Entity<InputState>,
    icon: ProjectScriptIcon,
    run_on_worktree_create: bool,
    wait_for_setup: bool,
    run_on_settle: bool,
    stop_on_settle: bool,
    auto_open_preview: bool,
    error: Option<SharedString>,
}

/// The project's scripts row.
pub struct ProjectScriptsControl {
    source: Arc<dyn GitActionsSource>,
    project_id: ProjectId,
    scripts: Vec<ProjectScript>,
    preferred_script_id: Option<String>,
    /// The session may run terminal commands.
    can_run: bool,
    menu_open: bool,
    dismissed: DismissPress,
    editor: Option<EditorDialog>,
}

impl EventEmitter<ProjectScriptsEvent> for ProjectScriptsControl {}

impl ProjectScriptsControl {
    pub fn new(
        source: Arc<dyn GitActionsSource>,
        project_id: ProjectId,
        scripts: Vec<ProjectScript>,
        can_run: bool,
    ) -> Self {
        Self { source, project_id, scripts, preferred_script_id: None, can_run, menu_open: false,
            dismissed: DismissPress::default(), editor: None }
    }

    pub fn set_scripts(&mut self, scripts: Vec<ProjectScript>, cx: &mut Context<Self>) {
        self.scripts = scripts;
        cx.notify();
    }

    pub fn set_menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.menu_open = open;
        cx.notify();
    }

    /// Opens the add dialog, or the edit dialog for `script_id`.
    pub fn open_editor(&mut self, script_id: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let script = script_id.and_then(|id| self.scripts.iter().find(|s| s.id == id));
        let input = script.map(ProjectScriptInput::from_script).unwrap_or_default();
        let field = |value: &str, placeholder: SharedString, window: &mut Window, cx: &mut Context<Self>| {
            let value = value.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
        };
        let name = field(&input.name, t!("Test", context = "example action name"), window, cx);
        let command = field(&input.command, "bun test".into(), window, cx);
        let preview_url =
            field(input.preview_url.as_deref().unwrap_or(""), "http://localhost:5173".into(), window, cx);
        self.editor = Some(EditorDialog {
            script_id: script.map(|s| s.id.clone()),
            name,
            command,
            preview_url,
            icon: input.icon,
            run_on_worktree_create: input.run_on_worktree_create,
            wait_for_setup: input.wait_for_setup,
            run_on_settle: input.run_on_settle,
            stop_on_settle: input.stop_on_settle,
            auto_open_preview: input.auto_open_preview,
            error: None,
        });
        self.menu_open = false;
        cx.notify();
    }

    fn primary(&self) -> Option<&ProjectScript> {
        self.preferred_script_id
            .as_deref()
            .and_then(|id| self.scripts.iter().find(|s| s.id == id))
            .or_else(|| primary_project_script(&self.scripts))
    }

    /// Runs `script` (the row's primary button); checks call it directly.
    pub fn run(&mut self, script: ProjectScript, cx: &mut Context<Self>) {
        if self.can_run {
            self.preferred_script_id = Some(script.id.clone());
            cx.emit(ProjectScriptsEvent::Run(script));
        }
    }

    /// Saves the dialog: validates, gives other scripts up their claimed roles,
    /// and sends the whole list as a `project.update`.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else { return };
        let name = editor.name.read(cx).value().trim().to_owned();
        let command = editor.command.read(cx).value().trim().to_owned();
        let preview = editor.preview_url.read(cx).value().trim().to_owned();
        if name.is_empty() {
            editor.error = Some(t!("Name is required."));
            cx.notify();
            return;
        }
        if command.is_empty() {
            editor.error = Some(t!("Command is required."));
            cx.notify();
            return;
        }
        let input = ProjectScriptInput {
            name,
            command,
            icon: editor.icon,
            run_on_worktree_create: editor.run_on_worktree_create,
            wait_for_setup: editor.wait_for_setup,
            run_on_settle: editor.run_on_settle,
            stop_on_settle: editor.stop_on_settle,
            preview_url: (!preview.is_empty()).then_some(preview),
            auto_open_preview: editor.auto_open_preview,
        };
        let id = editor
            .script_id
            .clone()
            .unwrap_or_else(|| next_project_script_id(&input.name, self.scripts.iter().map(|s| s.id.as_str())));
        let saved = build_project_script(id.clone(), &input);
        let mut scripts: Vec<ProjectScript> = self
            .scripts
            .iter()
            .filter(|s| s.id != id)
            .map(|s| release_claimed_roles(s, &input))
            .collect();
        match self.scripts.iter().position(|s| s.id == id) {
            Some(index) => scripts.insert(index.min(scripts.len()), saved),
            None => scripts.push(saved),
        }
        self.submit(scripts, cx);
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editor.as_ref().and_then(|e| e.script_id.clone()) else { return };
        let scripts = self.scripts.iter().filter(|s| s.id != id).cloned().collect();
        self.submit(scripts, cx);
    }

    fn submit(&mut self, scripts: Vec<ProjectScript>, cx: &mut Context<Self>) {
        let request = self.source.mutate_project(ProjectMutation::ProjectUpdate(ProjectMutationProjectUpdate {
            command_id: CommandId::new(format!("project-scripts-{}", now_ms())),
            project_id: self.project_id.clone(),
            title: None,
            workspace_root: None,
            default_model_selection: None,
            auto_pull: None,
            project_icon: None,
            favicon_path: None,
            default_thread_env_mode: None,
            scripts: Some(scripts.clone()),
            pinned: None,
            archived: None,
        }));
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.scripts = scripts;
                        this.editor = None;
                    }
                    Err(error) => {
                        let message = if error.0.is_empty() { t!("Failed to save action.") } else { error.0 };
                        match &mut this.editor {
                            Some(editor) => editor.error = Some(message),
                            None => {
                                Toaster::push(ToastData::new(ToastKind::Error, t!("Failed to save action.")), cx);
                            }
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_menu(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let highlight = c.highlight();
        let mut rows: Vec<AnyElement> = self
            .scripts
            .iter()
            .enumerate()
            .map(|(index, script)| {
                let run = script.clone();
                let edit = script.id.clone();
                let group: SharedString = format!("script-item-{index}").into();
                div()
                    .id(("script-item", index))
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .min_h(theme.sp(7.))
                    .px(theme.sp(2.))
                    .rounded(theme.radii.sm)
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .cursor_pointer()
                    .hover(move |s| s.bg(highlight))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        this.run(run.clone(), cx);
                    }))
                    .child(Icon::new(script_icon(script.icon)).size(IconSize::Md).color(c.muted_foreground).opacity(0.8))
                    .child(div().flex_1().min_w_0().truncate().child(project_script_menu_label(script)))
                    .child(
                        div()
                            .id(("script-edit", index))
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme.radii.md)
                            .opacity(0.)
                            .group_hover(group, |s| s.opacity(1.))
                            .hover(|s| s.bg(theme.colors.sidebar_row_hover))
                            .tooltip(tooltip(t!("Edit {name}", name = script.name.as_str())))
                            .child(Icon::new(IconName::Settings).size(IconSize::Sm).color(c.muted_foreground))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_editor(Some(&edit), window, cx);
                            }))
                            .test_support(),
                    )
                    .test_support()
                    .into_any_element()
            })
            .collect();
        rows.push(
            div()
                .id("script-add")
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .min_h(theme.sp(7.))
                .px(theme.sp(2.))
                .rounded(theme.radii.sm)
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .cursor_pointer()
                .hover(move |s| s.bg(highlight))
                .child(Icon::new(IconName::Plus).size(IconSize::Md).color(c.muted_foreground).opacity(0.8))
                .child(t!("Add project script"))
                .on_click(cx.listener(|this, _, window, cx| this.open_editor(None, window, cx)))
                .into_any_element(),
        );
        below_row(
            glass_surface(theme)
                .id("scripts-menu")
                .occlude()
                .w(theme.spacing.thread_details_panel_width - px(16.))
                .p(theme.sp(1.))
                .on_mouse_down_out(cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    this.dismissed.record(event.position);
                    this.set_menu_open(false, cx);
                }))
                .children(rows),
        )
    }

    fn render_editor(&self, editor: &EditorDialog, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let label = |text: SharedString| div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(text);
        let hint = |text: SharedString| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(text);
        let icons = [
            ProjectScriptIcon::Play,
            ProjectScriptIcon::Test,
            ProjectScriptIcon::Lint,
            ProjectScriptIcon::Configure,
            ProjectScriptIcon::Build,
            ProjectScriptIcon::Debug,
        ];
        let icon_picker = div().flex().gap(px(4.)).children(icons.into_iter().map(|icon| {
            let selected = editor.icon == icon;
            div()
                .id(SharedString::from(format!("script-icon-{}", icon.as_str())))
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(if selected { c.ring() } else { c.border })
                .cursor_pointer()
                .hover(|s| s.bg(c.sidebar_row_hover))
                .child(Icon::new(script_icon(icon)).size(IconSize::Sm))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor.icon = icon;
                    }
                    cx.notify();
                }))
        }));
        let switch_row = |id: &'static str, text: SharedString, on: bool, set: fn(&mut EditorDialog, bool)| {
            let this = cx.entity();
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).child(text))
                .child(Switch::new(id, on).on_change(move |value, _, cx| {
                    this.update(cx, |this, cx| {
                        if let Some(editor) = &mut this.editor {
                            set(editor, value);
                        }
                        cx.notify();
                    })
                }))
        };
        let editing = editor.script_id.is_some();
        let mut surface = DialogSurface::new(if editing { t!("Edit Action") } else { t!("Add Action") })
            .description(t!("Actions are project-scoped commands you can run from the top bar or keybindings."))
            .child(div().flex().flex_col().gap(px(6.)).child(label(t!("Name"))).child(icon_picker).child(TextInput::new(&editor.name)))
            .child(div().flex().flex_col().gap(px(6.)).child(label(t!("Command"))).child(TextInput::new(&editor.command).mono()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(label(t!("Preview URL (optional)")))
                    .child(TextInput::new(&editor.preview_url))
                    .child(hint(t!("Open this URL in the in-app preview when this action runs."))),
            )
            .child(switch_row("script-setup", t!("Run automatically on worktree creation"), editor.run_on_worktree_create, |e, v| {
                e.run_on_worktree_create = v
            }));
        if editor.run_on_worktree_create {
            surface = surface.child(switch_row(
                "script-wait",
                t!("Wait for it to finish before the agent starts"),
                editor.wait_for_setup,
                |e, v| e.wait_for_setup = v,
            ));
        }
        surface = surface
            .child(switch_row("script-settle", t!("Run in the thread's worktree when the thread settles"), editor.run_on_settle, |e, v| {
                e.run_on_settle = v
            }))
            .child(switch_row("script-stop", t!("Stop this action when the thread settles"), editor.stop_on_settle, |e, v| {
                e.stop_on_settle = v
            }))
            .children(editor.error.clone().map(|e| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(e)));
        if editing {
            surface = surface.footer(
                div().mr_auto().child(
                    Button::new("script-delete")
                        .variant(ButtonVariant::DestructiveOutline)
                        .label(t!("Delete", context = "verb, delete the action"))
                        .on_click(cx.listener(|this, _, _, cx| this.delete(cx))),
                ),
            );
        }
        let surface = surface
            .footer(observed(
                "script-cancel",
                Button::new("script-cancel-button")
                    .variant(ButtonVariant::Outline)
                    .label(t!("Cancel"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.editor = None;
                        cx.notify();
                    })),
            ))
            .footer(
                Button::new("script-save")
                    .size(ButtonSize::Default)
                    .label(if editing { t!("Save changes") } else { t!("Save action") })
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );
        let close = std::rc::Rc::new(cx.listener(|this, _: &(), _, cx| {
            this.editor = None;
            cx.notify();
        }));
        Modal::dialog("script-editor", div().id("dialog-content").occlude().w_full().max_w(cx.app_theme().rem(32.)).child(surface), move |window, cx| close(&(), window, cx)).into_any_element()
    }
}

impl Render for ProjectScriptsControl {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let editor = self.editor.as_ref().map(|e| self.render_editor(e, &theme, cx));
        let menu = self.menu_open.then(|| self.render_menu(&theme, cx));
        let content = match self.primary().cloned() {
            Some(primary) => {
                let tip = if self.can_run {
                    t!("Run {name}", name = primary.name.as_str())
                } else {
                    t!("Pair this client again with permission to run terminal commands.")
                };
                let run = primary.clone();
                let main = row_base("script-run", &theme, false, false)
                    .child(row_icon(&theme, script_icon(primary.icon)))
                    .child(div().flex_1().min_w_0().truncate().child(primary.name.clone()))
                    .tooltip(tooltip(tip))
                    .on_click(cx.listener(move |this, _, _, cx| this.run(run.clone(), cx)));
                let chevron = split_secondary("script-menu", &theme, false, self.menu_open)
                    .tooltip(tooltip(t!("Script actions")))
                    .on_click(cx.listener(|this, event, _, cx| {
                        if this.dismissed.consumes(event) {
                            return;
                        }
                        let open = !this.menu_open;
                        this.set_menu_open(open, cx);
                    }));
                split_row("scripts-split", &theme, main, chevron, self.menu_open, false).into_any_element()
            }
            None => row("script-add-row", &theme, Some(row_icon(&theme, IconName::Plus)), t!("Add project script"), false)
                .on_click(cx.listener(|this, _, window, cx| this.open_editor(None, window, cx)))
                .into_any_element(),
        };
        div().flex().flex_col().w_full().child(content).children(menu).children(editor)
    }
}
