//! The command palette's data over the [`FixtureStore`](super::FixtureStore),
//! until the live state bridge provides a source over the environment
//! stores. The app root refreshes it when the store's snapshot changes and
//! when the shown thread changes; the palette reads it when it opens.

use std::cell::RefCell;
use std::sync::Arc;

use crate::components::command_palette_lookups::{
    CommandPaletteSource, FixtureSource, PaletteEnvironment, PaletteFeature, PalettePort, PaletteProject,
    PaletteSetting, PaletteThread,
};
use crate::components::sidebar::model::{ProjectRef, SidebarSnapshot, ThreadKey};

/// Palette data from the fixture store's sidebar snapshot.
pub struct FixturePaletteSource {
    /// The snapshot the lists come from; built into `lists` on first read.
    snapshot: RefCell<Option<SidebarSnapshot>>,
    lists: RefCell<Option<Arc<FixtureSource>>>,
    active: RefCell<Option<ThreadKey>>,
    features: &'static [PaletteFeature],
}

/// What the showcase and audit worlds offer.
const FIXTURE_FEATURES: &[PaletteFeature] = &[PaletteFeature::ProjectSettings, PaletteFeature::PullRequests, PaletteFeature::Issues];

/// What the Mac reference's palette offered (its root list in `palette-root`).
const REFERENCE_FEATURES: &[PaletteFeature] = &[
    PaletteFeature::Triage,
    PaletteFeature::LinkPullRequest,
    PaletteFeature::RestartAgentSession,
    PaletteFeature::NewProject,
    PaletteFeature::PullRequests,
    PaletteFeature::Issues,
    PaletteFeature::UseInAnotherApp,
    PaletteFeature::ProjectSettings,
];

impl FixturePaletteSource {
    /// `reference` offers what the Mac reference's palette offered.
    pub fn new(snapshot: SidebarSnapshot, reference: bool) -> Self {
        let features = if reference { REFERENCE_FEATURES } else { FIXTURE_FEATURES };
        Self { snapshot: RefCell::new(Some(snapshot)), lists: RefCell::new(None), active: RefCell::new(None), features }
    }

    /// The store's threads or projects changed.
    pub fn set_snapshot(&self, snapshot: SidebarSnapshot) {
        *self.snapshot.borrow_mut() = Some(snapshot);
        *self.lists.borrow_mut() = None;
    }

    /// The main pane shows another thread (or none).
    pub fn set_active_thread(&self, key: Option<ThreadKey>) {
        *self.active.borrow_mut() = key;
    }

    /// Builds the lists lazily, so snapshot changes cost nothing until the palette opens.
    fn lists(&self) -> Arc<FixtureSource> {
        if let Some(lists) = self.lists.borrow().as_ref() {
            return lists.clone();
        }
        let snapshot = self.snapshot.borrow_mut().take().unwrap_or_default();
        let lists = Arc::new(FixtureSource::new(&snapshot));
        *self.lists.borrow_mut() = Some(lists.clone());
        lists
    }
}

impl CommandPaletteSource for FixturePaletteSource {
    fn threads(&self) -> Arc<[Arc<PaletteThread>]> {
        self.lists().threads()
    }

    fn projects(&self) -> Arc<[PaletteProject]> {
        self.lists().projects()
    }

    fn active_thread(&self) -> Option<ThreadKey> {
        self.active.borrow().clone()
    }

    fn current_project(&self) -> Option<ProjectRef> {
        let active = self.active.borrow().clone();
        match active {
            Some(active) => self.lists().threads().iter().find(|t| t.key == active).and_then(|t| t.project.clone()),
            None => self.lists().projects().first().map(|p| p.project_ref.clone()),
        }
    }

    fn environments(&self) -> Vec<PaletteEnvironment> {
        self.lists().environments()
    }

    fn ports(&self) -> Vec<PalettePort> {
        Vec::new()
    }

    fn settings(&self) -> Arc<[PaletteSetting]> {
        self.lists().settings()
    }

    fn supports(&self, feature: PaletteFeature) -> bool {
        self.features.contains(&feature)
    }

    fn active_thread_has_linked_items(&self) -> bool {
        self.lists().has_linked_items(self.active.borrow().as_ref())
    }

    fn now_ms(&self) -> i64 {
        self.lists().now_ms()
    }

    fn project_flows(&self) -> Option<std::rc::Rc<dyn crate::components::command_palette_project_flow::ProjectFlowBackend>> {
        self.lists().project_flows()
    }
}
