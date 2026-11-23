//! Keeps every thread's terminal drawer alive while the user moves between
//! threads, so sessions keep their output and scrollback, and shows only the
//! active thread's drawer when it is open. Ports `PersistentThreadTerminals.tsx`.

use std::collections::{HashMap, HashSet};

use gpui_kit::*;

use super::thread_terminal_drawer::{BackendFactory, DrawerMode, ThreadTerminalDrawer};
use crate::terminal_ui_state_store::{TerminalUiStateStore, ThreadKey};

/// The drawers of all threads that have opened a terminal.
pub struct PersistentThreadTerminals {
    factory: BackendFactory,
    drawers: HashMap<ThreadKey, Entity<ThreadTerminalDrawer>>,
    active: Option<ThreadKey>,
    _observe: Subscription,
}

impl PersistentThreadTerminals {
    pub fn new(factory: BackendFactory, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe_global::<TerminalUiStateStore>(|this, cx| this.sync_visibility(cx));
        Self { factory, drawers: HashMap::new(), active: None, _observe: observe }
    }

    /// Shows `thread`'s drawer (creating it on first open); the others stay mounted but hidden.
    pub fn set_active_thread(&mut self, thread: Option<(ThreadKey, SharedString)>, cx: &mut Context<Self>) {
        if let Some((key, cwd)) = &thread {
            let open = cx.global::<TerminalUiStateStore>().state(key).terminal_open;
            if open && !self.drawers.contains_key(key) {
                let factory = self.factory.clone();
                let key_for_drawer = key.clone();
                let cwd = cwd.clone();
                let drawer = cx.new(|cx| ThreadTerminalDrawer::new(key_for_drawer, cwd, DrawerMode::Drawer, factory, cx));
                self.drawers.insert(key.clone(), drawer);
            }
        }
        self.active = thread.map(|(key, _)| key);
        self.sync_visibility(cx);
    }

    /// The drawer for a thread, if it has been opened.
    pub fn drawer(&self, thread: &ThreadKey) -> Option<&Entity<ThreadTerminalDrawer>> {
        self.drawers.get(thread)
    }

    /// Drops drawers (and their sessions' views) for threads that no longer exist.
    pub fn remove_orphaned(&mut self, live: &HashSet<ThreadKey>, cx: &mut Context<Self>) {
        self.drawers.retain(|key, _| live.contains(key));
        cx.update_global::<TerminalUiStateStore, _>(|store, _| store.remove_orphaned(live));
        cx.notify();
    }

    fn sync_visibility(&mut self, cx: &mut Context<Self>) {
        let store = cx.global::<TerminalUiStateStore>();
        let updates: Vec<_> = self
            .drawers
            .iter()
            .map(|(key, drawer)| (drawer.clone(), self.active.as_ref() == Some(key) && store.state(key).terminal_open))
            .collect();
        for (drawer, visible) in updates {
            drawer.update(cx, |drawer, cx| drawer.set_visible(visible, cx));
        }
        cx.notify();
    }
}

impl Render for PersistentThreadTerminals {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.active.as_ref().and_then(|key| {
            let open = cx.global::<TerminalUiStateStore>().state(key).terminal_open;
            open.then(|| self.drawers.get(key).cloned()).flatten()
        });
        div().flex_none().w_full().children(visible)
    }
}
