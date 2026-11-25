//! Consent to send audio, and separately to clean transcripts up (`voiceConsent.ts`). Both live
//! in their own localStorage keys, outside client settings, so saving ordinary preferences can
//! never restore a revoked grant. Each new choice is a distinct grant, even for the same
//! destination, and a recording binds to the grant it started under.

use gpui_kit::App;
use t3code_client_runtime::voice_input::speech::{AudioChoice, AudioGrant, OffLiteral, VoiceTranscriptionDestination};

use crate::client_persistence_storage::LocalStorage;

const AUDIO_KEY: &str = "t3code:voice-audio-grant:v1";
const CLEANUP_KEY: &str = "t3code:voice-cleanup-grant:v1";

fn saved_key(id: &str) -> String {
    format!("{AUDIO_KEY}:saved:{id}")
}

fn read_choice(cx: &App) -> AudioChoice {
    LocalStorage::get(cx, AUDIO_KEY).unwrap_or(AudioChoice::None(()))
}

/// The grant in force (`useAudioChoice`), or `None` when audio may not leave this device.
pub fn audio_grant(cx: &App) -> Option<AudioGrant> {
    read_choice(cx).active(|id| LocalStorage::get::<bool>(cx, &saved_key(id))).cloned()
}

/// `chooseAudioDestination`: a new grant for `destination`, or Off.
pub fn choose_audio_destination(destination: Option<VoiceTranscriptionDestination>, cx: &mut App) {
    let previous = read_choice(cx);
    let choice = match destination {
        Some(destination) => AudioChoice::Grant(AudioGrant { id: uuid::Uuid::new_v4().to_string(), destination }),
        None => AudioChoice::Off(OffLiteral::Off),
    };
    LocalStorage::set(cx, AUDIO_KEY, Some(&choice));
    if let AudioChoice::Pending { pending, .. } = previous {
        LocalStorage::set::<bool>(cx, &saved_key(&pending.id), None);
    }
    changed(cx);
}

/// A recording bound to a grant that is no longer in force ends at once.
fn changed(cx: &mut App) {
    if let Some(voice) = super::voice_input_session::VoiceInput::try_global(cx) {
        voice.update(cx, |voice, cx| voice.destination_changed(cx));
    }
}

/// `pendingCloudAudioChoice`: a cloud grant that applies once its key saves. Returns the
/// grant id to pass to [`confirm_pending`].
pub fn pending_cloud_choice(destination: VoiceTranscriptionDestination, cx: &mut App) -> String {
    let previous = read_choice(cx);
    let active = previous.active(|id| LocalStorage::get::<bool>(cx, &saved_key(id))).cloned();
    let previous_choice = match active {
        Some(grant) => AudioChoice::Grant(grant),
        None if matches!(previous, AudioChoice::Off(_)) => AudioChoice::Off(OffLiteral::Off),
        None => AudioChoice::None(()),
    };
    let pending = AudioGrant { id: uuid::Uuid::new_v4().to_string(), destination };
    let id = pending.id.clone();
    LocalStorage::set(cx, AUDIO_KEY, Some(&AudioChoice::Pending { pending, previous: Box::new(previous_choice) }));
    if let AudioChoice::Pending { pending, .. } = previous {
        LocalStorage::set::<bool>(cx, &saved_key(&pending.id), None);
    }
    changed(cx);
    id
}

/// Marks the pending grant saved, if it is still the pending one.
pub fn confirm_pending(id: &str, cx: &mut App) {
    if matches!(read_choice(cx), AudioChoice::Pending { pending, .. } if pending.id == id) {
        LocalStorage::set(cx, &saved_key(id), Some(&true));
        changed(cx);
    }
}

/// `bindAudioGrant`: whether the grant a recording started under is still the one in force.
pub fn grant_still_holds(id: &str, cx: &App) -> bool {
    audio_grant(cx).is_some_and(|grant| grant.id == id)
}

/// `readCleanupGrant`: the cleanup grant's id, or `None` while cleanup is off.
pub fn cleanup_grant(cx: &App) -> Option<String> {
    LocalStorage::get::<Option<String>>(cx, CLEANUP_KEY).flatten()
}

/// `chooseTranscriptCleanup`.
pub fn choose_transcript_cleanup(enabled: bool, cx: &mut App) {
    let grant = enabled.then(|| uuid::Uuid::new_v4().to_string());
    LocalStorage::set(cx, CLEANUP_KEY, grant.as_ref());
    changed(cx);
}
