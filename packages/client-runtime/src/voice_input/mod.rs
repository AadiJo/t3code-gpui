//! Voice input (`client-runtime/src/voice-input`): the dictation controller shared by every
//! editor, the environment's voice HTTP routes, and the live transcription socket. Capture
//! and the UI belong to the client.

pub mod controller;
pub mod environment;
pub mod speech;
pub mod stream;

pub use controller::{
    CancelToken, PreparedVoiceTranscription, RecorderError, Recording, VoiceDraftSnapshot, VoiceErrorAction, VoiceInputController,
    VoiceInputControllerDependencies, VoiceInputErrorCode, VoiceInputPhase, VoiceInputState, VoiceMessage, VoiceRecorder, VoiceText,
    VoiceTranscriber, VoiceTranscriptionError, VoiceTranscriptionErrorCode, VOICE_RECORDING_LIMIT_SECONDS,
};
