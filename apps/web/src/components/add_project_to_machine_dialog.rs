//! "Add to another machine". Ports `AddProjectToMachineDialog.tsx` for the
//! remote-only app: pick a folder on the target machine with a folder
//! browser (`filesystem.browse`), then start a clone (`projectClone.start`).
//! The target is the environment the panels act on; choosing among several
//! environments and copying ignored files are not built.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AppContext as _, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{FilesystemBrowseInput, FilesystemBrowseResult};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::settings::provider_settings_panel::toast_error;
use crate::components::settings::server_settings_controls::{field_input, hint, tinted};
use crate::components::settings::server_settings_data::ServerPanelsModel;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::DialogSurface;
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// The dialog closes.
#[derive(Clone, Debug)]
pub enum AddProjectToMachineEvent {
    Close,
}

/// The dialog.
pub struct AddProjectToMachineDialog {
    model: Entity<ServerPanelsModel>,
    project_id: String,
    folder: Entity<InputState>,
    listing: Option<FilesystemBrowseResult>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AddProjectToMachineEvent> for AddProjectToMachineDialog {}

impl AddProjectToMachineDialog {
    pub fn new(model: Entity<ServerPanelsModel>, project_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let base = model.read(cx).snapshot().settings.add_project_base_directory.clone().filter(|b| !b.is_empty()).unwrap_or_else(|| "~/".into());
        let folder = cx.new(|cx| InputState::new(window, cx).default_value(base));
        let sub = cx.subscribe(&folder, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.browse(cx);
            }
        });
        let mut this = Self { model, project_id, folder, listing: None, error: None, _subscriptions: vec![sub] };
        this.browse(cx);
        this
    }

    /// Lists the folder typed so far on the target machine.
    pub fn browse(&mut self, cx: &mut Context<Self>) {
        let partial_path = self.folder.read(cx).value().to_string();
        let this = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.run("filesystem-browse", move |s| s.browse_filesystem(FilesystemBrowseInput { partial_path, cwd: None }), move |result, cx| {
                this.update(cx, |this, cx| {
                    match result {
                        Ok(listing) => this.listing = Some(listing),
                        Err(e) => this.error = Some(e.message),
                    }
                    cx.notify();
                })
                .ok();
            }, cx)
        });
    }

    fn pick(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.folder.update(cx, |state, cx| state.set_value(format!("{path}/"), window, cx));
        self.browse(cx);
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let folder = self.folder.read(cx).value().trim().trim_end_matches('/').to_owned();
        if folder.is_empty() {
            self.error = Some(t!("Choose a folder on the other machine."));
            cx.notify();
            return;
        }
        let input: Result<t3code_contracts::ProjectCloneStartInput, _> = serde_json::from_value(serde_json::json!({
            "sourceProjectId": self.project_id,
            "targetPath": folder,
        }));
        let Ok(input) = input else {
            self.error = Some(t!("Could not start the clone."));
            cx.notify();
            return;
        };
        let this = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.run("project-clone", move |s| s.start_project_clone(input), move |result, cx| {
                match result {
                    Ok(_) => {
                        this.update(cx, |_, cx| cx.emit(AddProjectToMachineEvent::Close)).ok();
                    }
                    Err(e) => {
                        toast_error(t!("Could not start the clone."), e.clone(), cx);
                        this.update(cx, |this, cx| {
                            this.error = Some(e.message);
                            cx.notify();
                        })
                        .ok();
                    }
                }
            }, cx)
        });
    }
}

impl Render for AddProjectToMachineDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let machine = self.model.read(cx).snapshot().environment_label.clone();
        let entries = self.listing.as_ref().map(|l| l.entries.clone()).unwrap_or_default();
        let list = div()
            .id("folder-browser")
            .flex()
            .flex_col()
            .h(px(180.))
            .overflow_y_scroll()
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(c.border)
            .children(entries.into_iter().map(|entry| {
                let path = entry.full_path.clone();
                div()
                    .id(SharedString::from(format!("folder-{}", entry.full_path)))
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .px(theme.sp(3.))
                    .py(theme.sp(1.5))
                    .cursor_pointer()
                    .hover(|d| d.bg(with_alpha(c.muted, 0.4)))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(path.clone(), window, cx)))
                    .child(Icon::new(IconName::Folder).size(IconSize::Sm).color(c.muted_foreground))
                    .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).child(entry.name.clone()))
            }));
        DialogSurface::new(t!("Add to another machine"))
            .on_close({
                let this = cx.entity().downgrade();
                move |_, cx| {
                    this.update(cx, |_, cx| cx.emit(AddProjectToMachineEvent::Close)).ok();
                }
            })
            .child(div().flex().flex_col().gap(theme.sp(1.5)).child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).child(t!("Folder on {machine}", machine = machine.clone()))).child(field_input(&self.folder, true, self.error.is_some(), false)))
            .child(list)
            .children(self.error.clone().map(|e| tinted(e, c.destructive(), cx)))
            .child(hint(self.listing.as_ref().map(|l| l.parent_path.clone()).unwrap_or_default(), cx))
            .footer(Button::new("clone-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(cx.listener(|_, _, _, cx| cx.emit(AddProjectToMachineEvent::Close))))
            .footer(Button::new("clone-submit").size(ButtonSize::Default).label(t!("Clone to {machine}", machine = machine)).on_click(cx.listener(|this, _, _, cx| this.submit(cx))))
    }
}
