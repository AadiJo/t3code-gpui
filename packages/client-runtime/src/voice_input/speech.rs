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
    pub fn active(&self, saved: &dyn Fn(&str) -> Option<bool>) -> Option<&AudioGrant> {
        match self {
            Self::Grant(grant) => Some(grant),
            Self::Off(_) | Self::None(()) => None,
            Self::Pending { pending, previous } => match saved(&pending.id) {
                None => previous.active(&|_| None),
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

/// `SpeechRecommendationRole`: the badge a benchmark pick earns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechRecommendationRole {
    Recommended,
    Faster,
    Accurate,
}

/// `speechRecommendationPolicy.json`.
mod policy {
    pub const FREE_WAIT_SECONDS: f64 = 3.;
    pub const WAIT_WEIGHT: f64 = 0.5;
    pub const OVERALL_MAX_WAIT_SECONDS: f64 = 5.;
    pub const MAX_LANGUAGE_ERROR_PERCENT: f64 = 20.;
    pub const ACCURATE_MAX_WAIT_SECONDS: f64 = 10.;
    pub const FAST_MAX_WAIT_SECONDS: f64 = 4.;
}

/// The reference seconds per 30 s of audio on `tier`, as the fork decodes the wire's two forms:
/// `None` when the server sent no figure for the tier (an older server), `Some(None)` when it
/// was not measured. The generated contract cannot tell a missing CPU figure from a null one;
/// both read as missing.
pub fn benchmark_seconds(model: &t3code_contracts::EnvironmentSpeechModel, tier: t3code_contracts::SpeechBenchmarkTier) -> Option<Option<f64>> {
    use t3code_contracts::{EnvironmentSpeechModelBenchmarkSecondsPer30s as Times, SpeechBenchmarkTier as Tier};
    let benchmark = model.benchmark.as_ref()?;
    match (&benchmark.seconds_per30s, tier) {
        (Some(Times::Object(times)), Tier::Cpu) => times.cpu.map(Some),
        (Some(Times::Object(times)), _) => Some(times.accelerated),
        (_, Tier::Cpu) => benchmark.cpu_seconds_per30s.map(Some),
        (Some(Times::Number(seconds)), _) => Some(Some(*seconds)),
        (None, _) => Some(None),
    }
}

/// `recommendSpeechModels`: reference accuracy and speed picks for `languages` on `tier`, in
/// pick order (recommended, faster, more accurate). No languages, no tier or no complete
/// measurements give no badge.
pub fn recommend_speech_models(
    models: &[t3code_contracts::EnvironmentSpeechModel],
    languages: &[String],
    tier: Option<t3code_contracts::SpeechBenchmarkTier>,
) -> Vec<(t3code_contracts::SpeechModelId, SpeechRecommendationRole)> {
    let mut wanted: Vec<&String> = Vec::new();
    for language in languages {
        if !wanted.contains(&language) {
            wanted.push(language);
        }
    }
    let Some(tier) = tier.filter(|_| !wanted.is_empty()) else { return Vec::new() };
    struct Scored<'a> {
        id: &'a t3code_contracts::SpeechModelId,
        error: f64,
        worst: f64,
        wait: f64,
        cost: f64,
    }
    let scored: Vec<Scored> = models
        .iter()
        .filter_map(|model| {
            if !wanted.iter().all(|language| model.languages.contains(language)) {
                return None;
            }
            let wait = benchmark_seconds(model, tier).flatten().filter(|wait| wait.is_finite() && *wait >= 0.)?;
            let errors: Option<Vec<f64>> = wanted
                .iter()
                .map(|language| {
                    model.benchmark.as_ref()?.accuracy.get(language.as_str()).map(|score| score.error_percent).filter(|e| e.is_finite() && *e >= 0.)
                })
                .collect();
            let errors = errors?;
            let error = (errors.iter().map(|value| value.ln()).sum::<f64>() / errors.len() as f64).exp();
            let worst = errors.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            Some(Scored { id: &model.id, error, worst, wait, cost: error + policy::WAIT_WEIGHT * (wait - policy::FREE_WAIT_SECONDS).max(0.) })
        })
        .collect();
    if scored.is_empty() {
        return Vec::new();
    }
    let useful: Vec<&Scored> = scored.iter().filter(|model| model.worst < policy::MAX_LANGUAGE_ERROR_PERCENT).collect();
    let pool: Vec<&Scored> = if useful.is_empty() { scored.iter().collect() } else { useful.clone() };
    // The first lowest, as `reduce` with a strict `<` keeps it.
    let lowest = |items: &[&'_ Scored<'_>], score: &dyn Fn(&Scored) -> f64| -> Option<usize> {
        let mut best: Option<usize> = None;
        for (ix, item) in items.iter().enumerate() {
            if best.is_none_or(|b| score(item) < score(items[b])) {
                best = Some(ix);
            }
        }
        best
    };
    let within: Vec<&Scored> = pool.iter().copied().filter(|model| model.wait <= policy::OVERALL_MAX_WAIT_SECONDS).collect();
    let best: &Scored = if !useful.is_empty() {
        let items = if within.is_empty() { &pool } else { &within };
        items[lowest(items, &|m| m.cost).expect("non-empty pool")]
    } else {
        pool[lowest(&pool, &|m| m.worst + m.error / 1000.).expect("non-empty pool")]
    };
    let quicker: Vec<&Scored> = pool.iter().copied().filter(|model| model.wait < best.wait).collect();
    let fast = (!useful.is_empty())
        .then(|| {
            let fastest: Vec<&Scored> = quicker.iter().copied().filter(|m| m.wait <= policy::FAST_MAX_WAIT_SECONDS).collect();
            lowest(&fastest, &|m| m.error).map(|ix| fastest[ix]).or_else(|| lowest(&quicker, &|m| m.wait + m.error / 1000.).map(|ix| quicker[ix]))
        })
        .flatten();
    let accurate = (!useful.is_empty())
        .then(|| {
            let better: Vec<&Scored> =
                pool.iter().copied().filter(|m| m.error < best.error && m.wait <= policy::ACCURATE_MAX_WAIT_SECONDS).collect();
            lowest(&better, &|m| m.error).map(|ix| better[ix])
        })
        .flatten();
    let mut roles: Vec<(t3code_contracts::SpeechModelId, SpeechRecommendationRole)> = Vec::new();
    for (model, role) in [
        (Some(best), SpeechRecommendationRole::Recommended),
        (fast, SpeechRecommendationRole::Faster),
        (accurate, SpeechRecommendationRole::Accurate),
    ] {
        if let Some(model) = model
            && !roles.iter().any(|(id, _)| id == model.id)
        {
            roles.push((model.id.clone(), role));
        }
    }
    roles
}

/// What first-run setup offers (`chooseSetupSpeechModel`).
#[derive(Debug, Clone)]
pub struct SetupSpeechModel {
    pub model: t3code_contracts::EnvironmentSpeechModel,
    /// The language it was chosen for; `None` without a language hint.
    pub language: Option<String>,
    pub recommended: bool,
}

/// `chooseSetupSpeechModel`: the selected model, else the benchmark pick, else any model the
/// server would transcribe the person's language with. With neither a language setting nor a
/// preferred language, only the selected model, without a recommendation.
pub fn choose_setup_speech_model(
    models: &[t3code_contracts::EnvironmentSpeechModel],
    selected_model_id: &str,
    intent: &str,
    preferred: &[String],
    tier: Option<t3code_contracts::SpeechBenchmarkTier>,
) -> Option<SetupSpeechModel> {
    if intent == "auto" && preferred.is_empty() {
        let model = models.iter().find(|model| model.id.as_str() == selected_model_id)?;
        return Some(SetupSpeechModel { model: model.clone(), language: None, recommended: false });
    }
    let intents = [intent.to_owned()];
    for language in if intent == "auto" { preferred } else { &intents[..] } {
        let capable: Vec<t3code_contracts::EnvironmentSpeechModel> = models
            .iter()
            .filter(|model| {
                let effective = effective_speech_language(&model.languages, model.supports_language_detection, intent);
                effective == *language || (effective == "auto" && model.languages.contains(language))
            })
            .cloned()
            .collect();
        let recommended_id = recommend_speech_models(&capable, std::slice::from_ref(language), tier).into_iter().next().map(|(id, _)| id);
        let model = capable
            .iter()
            .find(|model| model.id.as_str() == selected_model_id)
            .or_else(|| capable.iter().find(|model| Some(&model.id) == recommended_id.as_ref()))
            .or(capable.first());
        if let Some(model) = model {
            return Some(SetupSpeechModel {
                recommended: Some(&model.id) == recommended_id.as_ref(),
                model: model.clone(),
                language: Some(language.clone()),
            });
        }
    }
    None
}
