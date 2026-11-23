//! Which thread find is open in, and its query. Ports `threadFindStore.ts`:
//! find is open in at most one thread, the one the chat view shows, and a
//! route that shows anything else closes it (settings and usage draw over
//! the thread, so they keep it).

use gpui_kit::{App, Global, SharedString};

/// The find state of the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadFindStore {
    /// The scoped thread key find is open in; `None` when closed.
    pub thread_key: Option<SharedString>,
    pub query: SharedString,
    /// Bumped by each open, so an open bar refocuses and selects its query.
    pub focus_request: u64,
}

impl Global for ThreadFindStore {}

impl ThreadFindStore {
    pub fn get(cx: &App) -> &Self {
        cx.try_global::<Self>().unwrap_or(&CLOSED)
    }

    pub fn is_open_in(cx: &App, thread_key: &str) -> bool {
        Self::get(cx).thread_key.as_deref() == Some(thread_key)
    }

    /// Opens find in `thread_key`, or refocuses it there (`openThreadFind`).
    /// Opening in another thread starts with an empty query.
    pub fn open(cx: &mut App, thread_key: SharedString) {
        let store = cx.default_global::<Self>();
        if store.thread_key.as_ref() != Some(&thread_key) {
            store.query = SharedString::default();
        }
        store.thread_key = Some(thread_key);
        store.focus_request += 1;
    }

    pub fn set_query(cx: &mut App, query: SharedString) {
        cx.default_global::<Self>().query = query;
    }

    pub fn close(cx: &mut App) {
        let store = cx.default_global::<Self>();
        store.thread_key = None;
        store.query = SharedString::default();
    }

    /// Closes find unless the route still shows its thread (`retainThreadFindForRoute`).
    pub fn retain_for_route(cx: &mut App, modal: bool, shown: Option<&str>) {
        if modal {
            return;
        }
        let open = Self::get(cx).thread_key.clone();
        if open.is_some() && open.as_deref() != shown {
            Self::close(cx);
        }
    }
}

static CLOSED: ThreadFindStore = ThreadFindStore { thread_key: None, query: SharedString::new_static(""), focus_request: 0 };
