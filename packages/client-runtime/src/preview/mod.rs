//! Preview features that reach the environment outside the RPC socket: the
//! streamed browser (`preview/serverBrowserStream.ts`) and port forwarding
//! over the preview tunnel (the desktop app's `LoopbackTunnel.ts`).

pub mod environment_link;
pub mod loopback_tunnel;
pub mod server_browser_stream;
