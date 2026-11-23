//! The server-side settings pages of one environment, created on first
//! visit. The settings route renders [`ServerSettingsPages::page`] for the
//! paths [`ServerSettingsPages::handles`] returns true for.

use std::collections::HashMap;

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*};

use super::connections_settings_access::AccessSettingsPanel;
use super::integrations_settings::{BrowserSettingsPanel, DevicesSettingsPanel};
use super::maintenance_settings::MaintenanceSettingsPanel;
use super::projects_settings::ProjectsSettingsPanel;
use super::provider_settings_panel::ProviderSettingsPanel;
use super::scheduled_tasks_settings::ScheduledTasksSettingsPanel;
use super::server_settings_data::ServerPanelsModel;
use super::settings_search::SettingsPath;
use super::source_control_settings::SourceControlSettingsPanel;
use super::threads_settings::ThreadsSettingsPanel;
use super::worktrees_settings::WorktreesSettingsPanel;

/// The page views, by path, over one model.
pub struct ServerSettingsPages {
    model: Entity<ServerPanelsModel>,
    views: HashMap<SettingsPath, AnyView>,
}

impl ServerSettingsPages {
    pub fn new(model: Entity<ServerPanelsModel>) -> Self {
        Self { model, views: HashMap::new() }
    }

    pub fn model(&self) -> &Entity<ServerPanelsModel> {
        &self.model
    }

    /// The pages this module draws.
    pub fn handles(path: SettingsPath) -> bool {
        matches!(
            path,
            SettingsPath::Providers
                | SettingsPath::Threads
                | SettingsPath::SourceControl
                | SettingsPath::Worktrees
                | SettingsPath::Projects
                | SettingsPath::ScheduledTasks
                | SettingsPath::Browser
                | SettingsPath::Devices
                | SettingsPath::Maintenance
                | SettingsPath::Access
        )
    }

    /// The view for `path`, created the first time.
    pub fn page(&mut self, path: SettingsPath, window: &mut Window, cx: &mut App) -> Option<AnyView> {
        if !Self::handles(path) {
            return None;
        }
        if let Some(view) = self.views.get(&path) {
            return Some(view.clone());
        }
        let model = self.model.clone();
        let view: AnyView = match path {
            SettingsPath::Providers => cx.new(|cx| ProviderSettingsPanel::new(model, None, cx)).into(),
            SettingsPath::Threads => cx.new(|cx| ThreadsSettingsPanel::new(model, cx)).into(),
            SettingsPath::SourceControl => cx.new(|cx| SourceControlSettingsPanel::new(model, window, cx)).into(),
            SettingsPath::Worktrees => cx.new(|cx| WorktreesSettingsPanel::new(model, cx)).into(),
            SettingsPath::Projects => cx.new(|cx| ProjectsSettingsPanel::new(model, cx)).into(),
            SettingsPath::ScheduledTasks => cx.new(|cx| ScheduledTasksSettingsPanel::new(model, cx)).into(),
            SettingsPath::Browser => cx.new(|cx| BrowserSettingsPanel::new(model, cx)).into(),
            SettingsPath::Devices => cx.new(|cx| DevicesSettingsPanel::new(model, cx)).into(),
            SettingsPath::Maintenance => cx.new(|cx| MaintenanceSettingsPanel::new(model, cx)).into(),
            SettingsPath::Access => cx.new(|cx| AccessSettingsPanel::new(model, cx)).into(),
            _ => return None,
        };
        self.views.insert(path, view.clone());
        Some(view)
    }

    /// A page as a typed entity, for scenes and scripted checks.
    pub fn typed<T: 'static>(&self, path: SettingsPath) -> Option<Entity<T>> {
        self.views.get(&path).and_then(|v| v.clone().downcast::<T>().ok())
    }
}

/// Renders one page; the settings route places this in its content pane.
pub struct ServerSettingsPage {
    pub pages: Entity<ServerSettingsPages>,
    pub path: SettingsPath,
}

impl Render for ServerSettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self.path;
        let view = self.pages.update(cx, |pages, cx| pages.page(path, window, cx));
        div().size_full().children(view)
    }
}
