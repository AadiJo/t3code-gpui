//! The project scripts row of the thread details card. Ports
//! `ProjectScriptsControl.tsx` (panel layout) and `projectScriptEditor.tsx`:
//! a split row that runs the primary script, a menu of every script with its
//! setup / on-settle marks and an edit button, "Add project script", and the
//! add/edit dialog. Saving sends `projects.mutate` with the new script list;
//! running a script is the owner's (it opens a terminal).

use std::sync::Arc;

use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    FocusHandle, SharedString, Styled as _, TestSupportExt as _, Window,
    component::input::{InputState, TextareaState},
    div, prelude::*, px,
};
use t3code_contracts::{
    CommandId, ProjectId, ProjectMutation, ProjectMutationProjectUpdate, ProjectScript, ProjectScriptIcon,
};

use super::chat::thread_details_panel_styles::{DismissPress, below_row, observed, row_width, row, row_base, row_icon, split_row, split_secondary};
use super::git_actions_control_source::{GitActionsSource, now_ms};
use super::icons::{Icon, IconName, IconSize};
use super::ui::button::{Button, ButtonSize, ButtonVariant};
use super::ui::dialog::{DialogSurface, Modal};
use super::ui::input::{TextArea, TextInput};
use super::ui::popover::{PopoverSurface, below};
use super::ui::popover::glass_surface;
use super::ui::switch::Switch;
use super::ui::toast::{ToastData, ToastKind, Toaster};
use super::ui::tooltip::tooltip;
use super::ui::TextStyleExt as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::{t, t_rich};
use crate::theme_color::with_alpha;
use t3code_shared::keybindings::ShortcutPlatform;
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

/// The script's current shortcut (`keybindingValueForCommand`): the last rule
/// for its run command in the installed keymap.
fn script_keybinding(script_id: &str, cx: &gpui_kit::App) -> Option<String> {
    let command = crate::project_scripts::command_for_project_script(script_id)?;
    let state = cx.try_global::<crate::shortcuts::keymap::ShortcutState>()?;
    state.config.rules().iter().rev().find_map(|rule| {
        let rule_command = serde_json::to_value(&rule.command).ok()?;
        (rule_command.as_str() == Some(command.as_str()))
            .then(|| t3code_shared::keybindings::shortcut_to_keybinding_input(&rule.shortcut))
    })
}

/// The add/edit dialog while open.
struct EditorDialog {
    script_id: Option<String>,
    name: Entity<InputState>,
    command: Entity<TextareaState>,
    /// The shortcut as typed into the capture field (`mod+shift+t`), empty for none.
    keybinding: String,
    /// The shortcut the script had when the dialog opened.
    initial_keybinding: Option<String>,
    keybinding_focus: FocusHandle,
    icon_picker_open: bool,
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
    /// A shortcut change waiting on the scripts save: script id, new key, old key.
    pending_keybinding: Option<(String, Option<String>, Option<String>)>,
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
            dismissed: DismissPress::default(), editor: None, pending_keybinding: None }
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
        let command_value = input.command.clone();
        let command = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 12).placeholder("bun test").default_value(command_value));
        let initial_keybinding = script.and_then(|s| script_keybinding(&s.id, cx));
        let preview_url =
            field(input.preview_url.as_deref().unwrap_or(""), "http://localhost:5173".into(), window, cx);
        self.editor = Some(EditorDialog {
            script_id: script.map(|s| s.id.clone()),
            name,
            command,
            keybinding: initial_keybinding.clone().unwrap_or_default(),
            initial_keybinding,
            keybinding_focus: cx.focus_handle(),
            icon_picker_open: false,
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
        let keybinding = Some(editor.keybinding.trim().to_owned()).filter(|k| !k.is_empty());
        let keybinding_change = (keybinding != editor.initial_keybinding).then(|| (keybinding, editor.initial_keybinding.clone()));
        if let Some((Some(key), _)) = &keybinding_change
            && t3code_shared::keybindings::parse_keybinding_shortcut(key).is_none()
        {
            editor.error = Some(t!("Invalid keybinding."));
            cx.notify();
            return;
        }
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
        self.pending_keybinding = keybinding_change.map(|(key, previous)| (id, key, previous));
        self.submit(scripts, cx);
    }

    /// Saves a changed shortcut once the scripts are saved
    /// (`persistProjectScripts`): upsert the new key, or remove the old one.
    fn persist_keybinding(&mut self, cx: &mut Context<Self>) {
        let Some((id, key, previous)) = self.pending_keybinding.take() else { return };
        let Some(command) = crate::project_scripts::command_for_project_script(&id) else { return };
        let Some(environment) = ServerSettingsStore::keymap_environment(cx) else { return };
        let command: t3code_contracts::KeybindingCommand = serde_json::from_value(serde_json::Value::String(command)).expect("script command");
        let task = match key {
            Some(key) => ServerSettingsStore::upsert_keybinding(
                vec![environment],
                t3code_contracts::ServerUpsertKeybindingInput {
                    key,
                    command: command.clone(),
                    when: None,
                    replace: previous.map(|key| t3code_contracts::ServerRemoveKeybindingInput { key, command, when: None }),
                },
                cx,
            ),
            None => match previous {
                Some(key) => ServerSettingsStore::remove_keybinding(
                    vec![environment],
                    t3code_contracts::ServerRemoveKeybindingInput { key, command, when: None },
                    cx,
                ),
                None => return,
            },
        };
        cx.spawn(async move |_, cx| {
            if task.await.is_err() {
                cx.update(|cx| {
                    Toaster::push(
                        ToastData::new(ToastKind::Error, t!("The script was saved, but this connection can no longer change keyboard shortcuts.")),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    /// The keybinding field's key handler (`captureKeybinding`): Tab moves on,
    /// Backspace or Delete clears, any other chord replaces the shortcut.
    fn capture_keybinding(&mut self, event: &gpui_kit::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else { return };
        let key = event.keystroke.key.as_str();
        if key == "tab" {
            return;
        }
        cx.stop_propagation();
        if key == "backspace" || key == "delete" {
            editor.keybinding.clear();
        } else if let Some(next) =
            crate::components::settings::keybindings_settings::keybinding_from_keystroke(&event.keystroke, ShortcutPlatform::current())
        {
            editor.keybinding = next;
        }
        let _ = window;
        cx.notify();
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
                        this.persist_keybinding(cx);
                    }
                    Err(error) => {
                        this.pending_keybinding = None;
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
                    .child(Icon::new(script_icon(script.icon)).size(IconSize::Custom(theme.rem(1.))).color(c.muted_foreground).opacity(0.8))
                    .child(div().flex_1().min_w_0().truncate().child(project_script_menu_label(script)))
                    .child(
                        div()
                            .id(("script-edit", index))
                            .size(theme.rem(1.5))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme.radii.md)
                            .opacity(0.)
                            .group_hover(group, |s| s.opacity(1.))
                            .hover(|s| s.bg(theme.colors.sidebar_row_hover))
                            .tooltip(tooltip(t!("Edit {name}", name = script.name.as_str())))
                            .child(Icon::new(IconName::Settings).size(IconSize::Custom(theme.rem(0.875))).color(c.muted_foreground))
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
                .child(Icon::new(IconName::Plus).size(IconSize::Custom(theme.rem(1.))).color(c.muted_foreground).opacity(0.8))
                .child(t!("Add project script"))
                .on_click(cx.listener(|this, _, window, cx| this.open_editor(None, window, cx)))
                .into_any_element(),
        );
        below_row(
            glass_surface(theme)
                .id("scripts-menu")
                .occlude()
                .w(row_width(&theme))
                .p(theme.sp(1.))
                .on_mouse_down_out(cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    this.dismissed.record(event.position);
                    this.set_menu_open(false, cx);
                }))
                .children(rows),
        )
    }

    fn render_editor(&self, editor: &EditorDialog, theme: &AppTheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let label = |text: SharedString| div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(text);
        let hint = |text: SharedString| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(text);
        let icons = [
            (ProjectScriptIcon::Play, t!("Play")),
            (ProjectScriptIcon::Test, t!("Test")),
            (ProjectScriptIcon::Lint, t!("Lint")),
            (ProjectScriptIcon::Configure, t!("Configure")),
            (ProjectScriptIcon::Build, t!("Build")),
            (ProjectScriptIcon::Debug, t!("Debug")),
        ];
        let dark = theme.is_dark();
        // The icon button opens a 3-column grid of icons with their names.
        let icon_grid = editor.icon_picker_open.then(|| {
            let cards = icons.into_iter().map(|(icon, name)| {
                let selected = editor.icon == icon;
                div()
                    .id(SharedString::from(format!("script-icon-{}", icon.as_str())))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(theme.rem(0.5))
                    .px(theme.rem(0.5))
                    .py(theme.rem(0.5))
                    .rounded(theme.radii.md)
                    .border_1()
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .map(|card| {
                        if selected {
                            card.border_color(with_alpha(c.primary(), if dark { 0.3 } else { 0.7 })).bg(with_alpha(c.primary(), 0.1))
                        } else {
                            card.border_color(if dark { gpui_kit::hsla(0., 0., 0., 0.) } else { with_alpha(c.border, 0.7) })
                                .when(dark, |card| card.bg(gpui_kit::hsla(0., 0., 1., 0.035)))
                                .cursor_pointer()
                                .hover(|s| s.bg(with_alpha(c.accent, 0.6)))
                        }
                    })
                    .child(Icon::new(script_icon(icon)).size(IconSize::Custom(theme.rem(1.))))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(editor) = &mut this.editor {
                            editor.icon = icon;
                            editor.icon_picker_open = false;
                        }
                        cx.notify();
                    }))
            });
            let popup = PopoverSurface::new().child(div().grid().grid_cols(3).gap(theme.rem(0.5)).children(cards));
            div()
                .absolute()
                .left_0()
                .bottom_0()
                .child(below(div().occlude().on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor.icon_picker_open = false;
                    }
                    cx.notify();
                })).child(popup)))
        });
        let icon_button = div().relative().flex_none().child(
            Button::new("script-icon-button")
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Icon)
                .icon(script_icon(editor.icon))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor.icon_picker_open = !editor.icon_picker_open;
                    }
                    cx.notify();
                })),
        ).children(icon_grid);
        // A read-only field that records the next chord pressed in it.
        let focused = editor.keybinding_focus.is_focused(window);
        let keybinding_field = div()
            .id("script-keybinding")
            .track_focus(&editor.keybinding_focus)
            .on_key_down(cx.listener(Self::capture_keybinding))
            .flex()
            .items_center()
            .h(theme.sp(9.))
            .px(theme.sp(3.) - px(1.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(if focused { c.ring() } else { c.input })
            .bg(c.background())
            .cursor_text()
            .ui_text(theme, TextSize::Sm, weight::NORMAL)
            .text_color(if editor.keybinding.is_empty() { c.placeholder } else { c.foreground() })
            .child(if editor.keybinding.is_empty() { t!("Press shortcut") } else { SharedString::from(editor.keybinding.clone()) });
        let switch_row = |id: &'static str, text: SharedString, on: bool, disabled: bool, set: fn(&mut EditorDialog, bool)| {
            let this = cx.entity();
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.rem(0.75))
                .px(theme.rem(0.75))
                .py(theme.rem(0.5))
                .rounded(theme.radii.md)
                .border_1()
                .map(|row| {
                    if dark {
                        row.border_color(gpui_kit::hsla(0., 0., 0., 0.)).bg(gpui_kit::hsla(0., 0., 1., 0.035))
                    } else {
                        row.border_color(with_alpha(c.border, 0.7))
                    }
                })
                .when(disabled, |row| row.opacity(0.6))
                .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).child(text))
                .child(Switch::new(id, on).disabled(disabled).on_change(move |value, _, cx| {
                    this.update(cx, |this, cx| {
                        if let Some(editor) = &mut this.editor {
                            set(editor, value);
                        }
                        cx.notify();
                    })
                }))
        };
        let has_preview_url = !editor.preview_url.read(cx).value().trim().is_empty();
        let editing = editor.script_id.is_some();
        let mut surface = DialogSurface::new(if editing { t!("Edit Action") } else { t!("Add Action") })
            .bare_footer()
            .description(t!("Actions are project-scoped commands you can run from the top bar or keybindings."))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem(0.375))
                    .child(label(t!("Name")))
                    .child(div().flex().items_center().gap(theme.rem(0.5)).child(icon_button).child(div().flex_1().child(TextInput::new(&editor.name)))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem(0.375))
                    .child(label(t!("Keybinding")))
                    .child(keybinding_field)
                    .child(
                        div().child(
                            t_rich!("Press a shortcut. Use <code>Backspace</code> to clear. Shortcuts are environment-wide. Projects using the same action share its shortcut.")
                                .plain(),
                        )
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground),
                    ),
            )
            .child(div().flex().flex_col().gap(theme.rem(0.375)).child(label(t!("Command"))).child(TextArea::new(&editor.command)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem(0.375))
                    .child(label(t!("Preview URL (optional)")))
                    .child(TextInput::new(&editor.preview_url))
                    .child(hint(t!("Open this URL in the in-app preview when this action runs."))),
            )
            .child(switch_row("script-setup", t!("Run automatically on worktree creation"), editor.run_on_worktree_create, false, |e, v| {
                e.run_on_worktree_create = v
            }))
            .child(switch_row(
                "script-wait",
                t!("Wait for it to finish before the agent starts"),
                editor.wait_for_setup,
                !editor.run_on_worktree_create,
                |e, v| e.wait_for_setup = v,
            ));
        surface = surface
            .child(switch_row("script-settle", t!("Run in the thread's worktree when the thread settles"), editor.run_on_settle, false, |e, v| {
                e.run_on_settle = v
            }))
            .child(switch_row("script-stop", t!("Stop this action when the thread settles"), editor.stop_on_settle, false, |e, v| {
                e.stop_on_settle = v
            }))
            .child(switch_row(
                "script-auto-preview",
                t!("Open preview automatically when this action runs"),
                editor.auto_open_preview,
                !has_preview_url,
                |e, v| e.auto_open_preview = v,
            ))
            .children(editor.error.clone().map(|e| div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.destructive()).child(e)));
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let editor = self.editor.as_ref().map(|e| self.render_editor(e, &theme, window, cx));
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
