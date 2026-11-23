//! Images and videos from the workspace and chat. Ports `components/media`:
//! decoding, the media actions menu and the video surface. GPUI in this build
//! has no video decoder, so a video shows a card that opens it in the OS player.

pub mod media_actions;
pub mod media_source;
pub mod video_preview;
