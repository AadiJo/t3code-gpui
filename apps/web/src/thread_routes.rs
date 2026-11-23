//! Whether a thread route can render yet. Ports `threadRoutes.ts`
//! (`resolveThreadRouteRenderState`); route parsing itself lives in
//! [`crate::router`].

/// `ThreadRouteRenderState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadRouteRenderState {
    Loading,
    Ready,
    Missing,
}

/// What the client knows about the route's thread.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadRouteFacts {
    pub bootstrap_complete: bool,
    pub server_thread_exists: bool,
    pub server_thread_deleted: bool,
    pub draft_thread_exists: bool,
}

/// `resolveThreadRouteRenderState`, for a thread whose detail is not loaded separately.
pub fn resolve_thread_route_render_state(facts: ThreadRouteFacts) -> ThreadRouteRenderState {
    if facts.draft_thread_exists || (facts.server_thread_exists && !facts.server_thread_deleted) {
        ThreadRouteRenderState::Ready
    } else if !facts.bootstrap_complete {
        ThreadRouteRenderState::Loading
    } else {
        ThreadRouteRenderState::Missing
    }
}
