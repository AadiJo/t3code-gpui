//! The native terminal: an `alacritty_terminal` model, a GPUI element that
//! paints its grid, and a view that turns input into bytes. It reaches the
//! outside only through [`backend::TerminalBackend`].

pub mod backend;
pub mod core;
pub mod key_codes;
#[cfg(unix)]
pub mod local_pty;
pub mod mouse;
pub mod palette;
pub mod renderer;
pub mod surface;

pub use backend::{GridSize, TerminalBackend, TerminalInbound, TerminalOutbound};
pub use surface::{TerminalSurface, TerminalSurfaceEvent};
