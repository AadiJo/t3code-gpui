//! Speech wire values the generated contracts do not carry (`contracts/src/speech.ts`,
//! `settings.ts`) and the pure helpers from `shared/src/speech.ts`.

use serde::{Deserialize, Serialize};
use t3code_contracts::{CloudSpeechProvider, EnvironmentId, SpeechCustomWords, SpeechCustomWordsInnerItem};
use unicode_normalization::UnicodeNormalization as _;

/// `SPEECH_STREAM_PATH`.
pub const SPEECH_STREAM_PATH: &str = "/ws/voice";
/// The sample rate every recording is sent at: mono 32-bit float little-endian PCM.
pub const SPEECH_SAMPLE_RATE: u32 = 16_000;
/// `SPEECH_STREAM_MAX_CHUNK_BYTES`: one second of audio per socket frame.
pub const SPEECH_STREAM_MAX_CHUNK_BYTES: usize = SPEECH_SAMPLE_RATE as usize * 4;
/// `SPEECH_STREAM_MAX_QUEUED_BYTES`: five minutes of audio.
pub const SPEECH_STREAM_MAX_QUEUED_BYTES: usize = SPEECH_SAMPLE_RATE as usize * 4 * 5 * 60;
/// `SPEECH_MAX_OPTIONS_BYTES`.
pub const SPEECH_MAX_OPTIONS_BYTES: usize = 128_000;

/// `SpeechTranscriptionOptions`: recognition preferences that travel with every recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechTranscriptionOptions {
    pub speech_language: String,
    pub speech_translate_to_english: bool,
    pub speech_custom_words: SpeechCustomWords,
    pub speech_remove_filler_words: bool,
    /// Preserved for separately consented transcript cleanup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_correction_word: Option<String>,
    #[serde(default = "default_true")]
    pub speech_silence_filtering: bool,
}

fn default_true() -> bool {
    true
}

/// `SpeechStreamText`: what the live transcriber has settled and what it may still revise.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SpeechStreamText {
    pub committed: String,
    pub tentative: String,
}

/// `SpeechStreamCommand`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SpeechStreamCommand<'a> {
    Start { options: &'a SpeechTranscriptionOptions },
    Finish,
}

/// `SpeechStreamEvent`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SpeechStreamEvent {
    Ready,
    Update { revision: i64, text: Option<SpeechStreamText> },
    Finished { text: String },
    Error { message: String },
}

/// `VoiceTranscriptionDestination`: where microphone audio may go.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VoiceTranscriptionDestination {
    #[serde(rename_all = "camelCase")]
    Environment { environment_id: EnvironmentId },
    #[serde(rename_all = "camelCase")]
    Cloud {
        environment_id: EnvironmentId,
        provider: CloudSpeechProvider,
        /// The exact OpenRouter model; only that provider names one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

impl VoiceTranscriptionDestination {
    /// The environment the audio goes to (or is forwarded through).
    pub fn environment_id(&self) -> &EnvironmentId {
        match self {
            Self::Environment { environment_id } | Self::Cloud { environment_id, .. } => environment_id,
        }
    }

    pub fn cloud_provider(&self) -> Option<CloudSpeechProvider> {
        match self {
            Self::Cloud { provider, .. } => Some(*provider),
            Self::Environment { .. } => None,
        }
    }
}

/// One act of consent naming a destination (`voiceConsent.ts` `Grant`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioGrant {
    pub id: String,
    pub destination: VoiceTranscriptionDestination,
}

/// The stored audio choice (`AudioChoice`): none yet, explicitly off, a grant, or a cloud grant
/// waiting for its key to save (`pending`, with the choice that applies until then).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AudioChoice {
    Pending { pending: AudioGrant, previous: Box<AudioChoice> },
    Grant(AudioGrant),
    Off(OffLiteral),
    None(()),
}

/// The literal `"off"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OffLiteral {
    #[serde(rename = "off")]
    Off,
}

impl AudioChoice {
    /// The grant in force (`activeChoice`). `saved` reports the pending grant's save marker:
    /// `None` when absent, `Some(true)` once the key saved.
    pub fn active(&self, saved: impl Fn(&str) -> Option<bool>) -> Option<&AudioGrant> {
        match self {
            Self::Grant(grant) => Some(grant),
            Self::Off(_) | Self::None(()) => None,
            Self::Pending { pending, previous } => match saved(&pending.id) {
                None => previous.active(|_| None),
                Some(true) => Some(pending),
                Some(false) => None,
            },
        }
    }
}

/// `cloudSpeechLanguages`: explicit hints every cloud API understands.
pub const CLOUD_SPEECH_LANGUAGES: &[&str] = &[
    "ar", "bg", "ca", "cs", "da", "de", "el", "en", "es", "fa", "fi", "fr", "hi", "hr", "hu", "id", "it", "ja", "ko", "mk", "ms",
    "nl", "pl", "pt", "ro", "ru", "sk", "sv", "th", "tr", "uk", "ur", "vi", "zh",
];

/// `effectiveSpeechLanguage`: the language a model can produce for the requested one.
pub fn effective_speech_language(languages: &[String], supports_detection: bool, intent: &str) -> String {
    if languages.len() == 1 {
        return languages[0].clone();
    }
    if intent != "auto" && languages.iter().any(|language| language == intent) {
        return intent.to_owned();
    }
    if supports_detection {
        return "auto".to_owned();
    }
    languages.iter().find(|language| *language == "en").or(languages.first()).cloned().unwrap_or_else(|| "auto".to_owned())
}

fn vocabulary_key(value: &str) -> String {
    value.trim().nfc().collect::<String>().to_lowercase()
}

/// `resolveSpeechVocabulary`: hint terms first, then project spellings, then personal ones,
/// each spelling once, at most 100 entries.
pub fn resolve_speech_vocabulary(personal: &SpeechCustomWords, project: &SpeechCustomWords, hints: &[&str]) -> SpeechCustomWords {
    let known: std::collections::HashSet<String> = project
        .iter()
        .chain(personal)
        .flat_map(|word| std::iter::once(&word.term).chain(&word.aliases))
        .map(|spelling| vocabulary_key(spelling))
        .collect();
    let hints = hints.iter().filter_map(|hint| {
        let term = hint.split_whitespace().collect::<Vec<_>>().join(" ");
        (!term.is_empty() && term.chars().count() <= 50 && !known.contains(&vocabulary_key(&term)))
            .then(|| SpeechCustomWordsInnerItem { term, aliases: Vec::new() })
    });
    let mut seen = std::collections::HashSet::new();
    hints
        .collect::<Vec<_>>()
        .into_iter()
        .chain(project.iter().cloned())
        .chain(personal.iter().cloned())
        .filter_map(|word| {
            if !seen.insert(vocabulary_key(&word.term)) {
                return None;
            }
            let aliases = word.aliases.into_iter().filter(|alias| seen.insert(vocabulary_key(alias))).collect();
            Some(SpeechCustomWordsInnerItem { term: word.term, aliases })
        })
        .take(100)
        .collect()
}

/// `encodeSpeechPcmRequest`: a four-byte little-endian JSON length, the UTF-8 options, then
/// the raw PCM.
pub fn encode_speech_pcm_request(pcm: &[u8], options: &SpeechTranscriptionOptions) -> Result<Vec<u8>, String> {
    let metadata = serde_json::to_vec(options).map_err(|error| error.to_string())?;
    if metadata.len() > SPEECH_MAX_OPTIONS_BYTES {
        return Err("Speech options are too large.".into());
    }
    let mut body = Vec::with_capacity(4 + metadata.len() + pcm.len());
    body.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    body.extend_from_slice(&metadata);
    body.extend_from_slice(pcm);
    Ok(body)
}

/// Mono `f32` samples as the little-endian bytes the server reads.
pub fn pcm_bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}
