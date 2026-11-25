//! Voice input (`apps/web/src/speech`): microphone capture with cpal, the microphone order,
//! consent, and the app's one dictation session that every composer shares.

pub mod mac_permission;
pub mod microphones;
pub mod pcm_capture;
pub mod pcm_resampler;
pub mod voice_consent;
pub mod voice_input_session;
