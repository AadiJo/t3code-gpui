//! Connection, sync, and domain state for a T3 Code client. Mirrors
//! `packages/client-runtime`. Nothing here depends on GPUI: it runs on tokio
//! and hands state to the UI through channels.

pub mod authorization;
pub mod connection;
pub mod environment;
pub mod operations;
pub mod platform;
pub mod relay;
pub mod rpc;
pub mod state;
