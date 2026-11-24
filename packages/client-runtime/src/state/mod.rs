//! Server state per environment, from the connected session to the view models the UI reads:
//! the shell (sidebar rows and projects), mounted thread details, their caches, and the pure
//! derivations (status, sections, sort, unread, notifications). Mirrors `client-runtime/src/state`.
//!
//! Views read stores through `tokio::sync::watch` receivers of `Arc` values. Every row and record
//! is behind its own `Arc`, so one changed thread does not copy the list, and a view can skip rows
//! whose `Arc` did not change.

pub mod cache_persistence;
pub mod cache_store;
pub mod client_store;
pub mod drafts;
pub mod environment;
pub mod environment_http;
pub mod models;
pub mod notifications;
pub mod older_settled_threads;
pub mod optimistic_messages;
pub mod orchestration;
pub mod orchestration_v2_projection;
pub mod project_grouping;
pub mod raw_stream;
pub mod runtime;
pub mod shell;
pub mod shell_reducer;
pub mod sidebar;
pub mod thread_detail;
pub mod thread_families;
pub mod thread_history_merge;
pub mod thread_inbox;
pub mod thread_lifecycle;
pub mod thread_runs;
pub mod thread_search;
pub mod thread_settled;
pub mod thread_shell;
pub mod thread_sort;
pub mod thread_tag_filter;
pub mod thread_state;
pub mod thread_visits;
pub mod threads;
pub mod ui_state;

pub use environment::{EnvironmentContext, EnvironmentStore, SyncStatus, ThreadHandle};
pub use runtime::StateRuntime;
pub use shell::{ShellState, ShellStore};
pub use thread_state::{ThreadState, ThreadStatus};
