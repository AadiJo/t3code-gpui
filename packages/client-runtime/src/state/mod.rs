//! Server state per environment, from the connected session to the view models the UI reads:
//! the shell (sidebar rows and projects), mounted thread details, their caches, and the pure
//! derivations (status, sections, sort, unread, notifications). Mirrors `client-runtime/src/state`.
//!
//! Views read stores through `tokio::sync::watch` receivers of `Arc` values. Every row and record
//! is behind its own `Arc`, so one changed thread does not copy the list, and a view can skip rows
//! whose `Arc` did not change.

pub mod cache_persistence;
pub mod cache_store;
pub mod environment;
pub mod environment_http;
pub mod models;
pub mod orchestration_v2_projection;
pub mod raw_stream;
pub mod shell;
pub mod shell_reducer;
pub mod thread_history_merge;
pub mod thread_lifecycle;
pub mod thread_state;
pub mod threads;

pub use environment::{EnvironmentContext, EnvironmentStore, SyncStatus, ThreadHandle};
pub use shell::{ShellState, ShellStore};
pub use thread_state::{ThreadState, ThreadStatus};
