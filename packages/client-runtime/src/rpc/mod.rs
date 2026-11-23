//! Effect RPC over WebSocket: framing, the client, the per-environment
//! session, and shared HTTP plumbing.

pub mod client;
pub mod e2e_methods;
pub mod http;
pub mod protocol;
pub mod session;

pub use client::{CloseReason, FrameDirection, FrameTap, RpcClient, RpcError, RpcSubscription};
pub use session::{RpcSession, RpcSessionOptions};
