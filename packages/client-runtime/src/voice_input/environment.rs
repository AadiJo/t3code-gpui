//! The voice HTTP routes of one environment (`voice-input/environment.ts`): speech status,
//! batch and cloud transcription, the media ticket for the live stream, the model catalog and
//! transcript cleanup. Every request carries the session's credential and refuses redirects,
//! so audio and credentials never follow one to another host.

use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use t3code_contracts::{
    CloudSpeechError, CloudSpeechKeys, CloudSpeechModels, CloudSpeechProvider, EnvironmentSpeechModelDownload, EnvironmentSpeechModels,
    EnvironmentSpeechPostProcessingRequest, EnvironmentSpeechPostProcessingResult, EnvironmentSpeechStatus,
    EnvironmentSpeechTranscriptionResult, SpeechAcceleration, SpeechModelUnloadTimeout,
};
use url::Url;

use super::speech::{SPEECH_STREAM_PATH, SpeechTranscriptionOptions, encode_speech_pcm_request};
use crate::authorization::remote::WebSocketTicket;
use crate::environment::endpoint_url;
use crate::preview::environment_link::EnvironmentLink;
use crate::rpc::http::{EnvironmentHttpError, tls_config};

/// Long enough for a model download or a slow transcription.
const INFERENCE_OR_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a voice request failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VoiceRequestError {
    #[error("This environment is not connected.")]
    NotConnected,
    #[error("The request timed out.")]
    Timeout,
    #[error("{0}")]
    Network(String),
    #[error("{}", .0.message())]
    Cloud(CloudSpeechError),
    #[error("The environment refused the request: {0:?}")]
    Environment(EnvironmentHttpError),
    #[error("The environment answered HTTP {status}.")]
    Status { status: u16 },
    #[error("The environment's answer did not decode: {0}")]
    Decode(String),
}

/// The copy `CloudSpeechError.message` shows.
pub trait CloudSpeechErrorMessage {
    fn message(&self) -> String;
}

impl CloudSpeechErrorMessage for CloudSpeechError {
    fn message(&self) -> String {
        use t3code_contracts::CloudSpeechErrorReason as R;
        let name = cloud_speech_provider_name(self.provider);
        match self.reason {
            R::MissingKey => format!("Add your {name} API key in Settings → Voice."),
            R::KeyChanged => format!("The {name} key changed before your recording was sent. Try again."),
            R::UnreadableKey => format!(
                "Couldn’t read the {name} API key. Unlock the vault on the selected environment or update the key in Settings → Voice. No audio was sent to {name}."
            ),
            R::RejectedKey => format!("{name} rejected this API key. Check its permissions and billing in Settings → Voice."),
            R::RateLimit => format!("{name} reached a usage limit. Check the account's quota or try again later."),
            R::ModelUnavailable => format!("The chosen model isn’t available on {name}. Choose another model in Settings → Voice."),
            R::Unavailable => format!("{name} couldn’t transcribe this recording. Try again later."),
        }
    }
}

/// `CLOUD_SPEECH_PROVIDER_NAMES`.
pub fn cloud_speech_provider_name(provider: CloudSpeechProvider) -> &'static str {
    match provider {
        CloudSpeechProvider::Openai => "OpenAI",
        CloudSpeechProvider::Xai => "xAI",
        CloudSpeechProvider::Openrouter => "OpenRouter",
    }
}

/// `CLOUD_SPEECH_LABELS`.
pub fn cloud_speech_label(provider: CloudSpeechProvider) -> &'static str {
    match provider {
        CloudSpeechProvider::Openai => "OpenAI",
        CloudSpeechProvider::Xai => "Grok Voice Transcribe",
        CloudSpeechProvider::Openrouter => "OpenRouter",
    }
}

/// A client that never follows a redirect.
fn voice_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .use_preconfigured_tls((*tls_config()).clone())
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("voice HTTP client builds")
        })
        .clone()
}

/// The body a request sends.
enum Body {
    None,
    Json(Vec<u8>),
    Octets(Vec<u8>),
}

async fn send<T: DeserializeOwned>(
    link: &EnvironmentLink,
    method: reqwest::Method,
    path: &str,
    query: &[(&str, &str)],
    body: Body,
    timeout: Duration,
) -> Result<T, VoiceRequestError> {
    let session = link.session(Duration::from_secs(10)).await.map_err(|_| VoiceRequestError::NotConnected)?;
    let mut url: Url = endpoint_url(&session.prepared.http_base_url, path);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    let authorization = link.supervisor.http_authorization.borrow().clone().unwrap_or_else(|| session.prepared.http_authorization.clone());
    let mut request = authorization.apply(voice_client().request(method.clone(), url.clone()), method.as_str(), &url, link.auth.key());
    request = match body {
        Body::None => request,
        Body::Json(bytes) => request.header(reqwest::header::CONTENT_TYPE, "application/json").body(bytes),
        Body::Octets(bytes) => request.header(reqwest::header::CONTENT_TYPE, "application/octet-stream").body(bytes),
    };
    let response = tokio::time::timeout(timeout, request.send())
        .await
        .map_err(|_| VoiceRequestError::Timeout)?
        .map_err(|error| VoiceRequestError::Network(error.to_string()))?;
    let status = response.status();
    let bytes = tokio::time::timeout(timeout, response.bytes())
        .await
        .map_err(|_| VoiceRequestError::Timeout)?
        .map_err(|error| VoiceRequestError::Network(error.to_string()))?;
    if status.is_success() {
        let bytes: &[u8] = if bytes.is_empty() { b"null" } else { &bytes };
        return serde_json::from_slice(bytes).map_err(|error| VoiceRequestError::Decode(error.to_string()));
    }
    if let Ok(cloud) = serde_json::from_slice::<CloudSpeechError>(&bytes) {
        return Err(VoiceRequestError::Cloud(cloud));
    }
    Err(match serde_json::from_slice::<EnvironmentHttpError>(&bytes) {
        Ok(error) => VoiceRequestError::Environment(error),
        Err(_) => VoiceRequestError::Status { status: status.as_u16() },
    })
}

fn json(value: &impl Serialize) -> Body {
    Body::Json(serde_json::to_vec(value).unwrap_or_default())
}

/// `GET /api/voice/status`.
pub async fn speech_status(link: &EnvironmentLink) -> Result<EnvironmentSpeechStatus, VoiceRequestError> {
    send(link, reqwest::Method::GET, "/api/voice/status", &[], Body::None, DEFAULT_TIMEOUT).await
}

/// `POST /api/voice/transcribe`: a finished recording, transcribed on the environment.
pub async fn transcribe_pcm(link: &EnvironmentLink, pcm: &[u8], options: &SpeechTranscriptionOptions) -> Result<String, VoiceRequestError> {
    let body = encode_speech_pcm_request(pcm, options).map_err(VoiceRequestError::Network)?;
    let result: EnvironmentSpeechTranscriptionResult =
        send(link, reqwest::Method::POST, "/api/voice/transcribe", &[], Body::Octets(body), INFERENCE_OR_DOWNLOAD_TIMEOUT).await?;
    Ok(result.text)
}

/// `POST /api/voice/cloud/:provider/transcribe`: forwarded to the named provider. OpenRouter
/// names its exact model.
pub async fn transcribe_cloud_pcm(
    link: &EnvironmentLink,
    provider: CloudSpeechProvider,
    model: Option<&str>,
    pcm: &[u8],
    options: &SpeechTranscriptionOptions,
) -> Result<String, VoiceRequestError> {
    let body = encode_speech_pcm_request(pcm, options).map_err(VoiceRequestError::Network)?;
    let path = format!("/api/voice/cloud/{}/transcribe", provider.as_str());
    let query: Vec<(&str, &str)> = model.map(|model| ("model", model)).into_iter().collect();
    let result: EnvironmentSpeechTranscriptionResult =
        send(link, reqwest::Method::POST, &path, &query, Body::Octets(body), Duration::from_secs(130)).await?;
    Ok(result.text)
}

/// `GET /api/voice/cloud/keys`.
pub async fn cloud_speech_keys(link: &EnvironmentLink) -> Result<CloudSpeechKeys, VoiceRequestError> {
    send(link, reqwest::Method::GET, "/api/voice/cloud/keys", &[], Body::None, DEFAULT_TIMEOUT).await
}

/// `GET /api/voice/cloud/openrouter/models`.
pub async fn cloud_speech_models(link: &EnvironmentLink) -> Result<CloudSpeechModels, VoiceRequestError> {
    send(link, reqwest::Method::GET, "/api/voice/cloud/openrouter/models", &[], Body::None, DEFAULT_TIMEOUT).await
}

/// `POST /api/voice/cloud/:provider/key`; `None` removes the key.
pub async fn set_cloud_speech_key(link: &EnvironmentLink, provider: CloudSpeechProvider, key: Option<&str>) -> Result<(), VoiceRequestError> {
    let path = format!("/api/voice/cloud/{}/key", provider.as_str());
    send::<serde_json::Value>(link, reqwest::Method::POST, &path, &[], json(&serde_json::json!({ "key": key })), DEFAULT_TIMEOUT).await.map(|_| ())
}

/// `requireCloudSpeechKey`: fails with the provider's missing-key copy when no key is saved.
pub async fn require_cloud_speech_key(link: &EnvironmentLink, provider: CloudSpeechProvider) -> Result<(), VoiceRequestError> {
    let keys = cloud_speech_keys(link).await?;
    let present = match provider {
        CloudSpeechProvider::Openai => keys.openai,
        CloudSpeechProvider::Xai => keys.xai,
        CloudSpeechProvider::Openrouter => keys.openrouter.unwrap_or(false),
    };
    if present {
        Ok(())
    } else {
        Err(VoiceRequestError::Cloud(CloudSpeechError { provider, reason: t3code_contracts::CloudSpeechErrorReason::MissingKey }))
    }
}

/// `getEnvironmentSpeechStreamUrl`: a `media` ticket and the `ws(s)` URL of the live stream.
pub async fn speech_stream_url(link: &EnvironmentLink) -> Result<Url, VoiceRequestError> {
    let session = link.session(Duration::from_secs(10)).await.map_err(|_| VoiceRequestError::NotConnected)?;
    let ticket: WebSocketTicket =
        send(link, reqwest::Method::POST, "/api/auth/websocket-ticket/media", &[], Body::None, DEFAULT_TIMEOUT).await?;
    let mut url = endpoint_url(&session.prepared.http_base_url, SPEECH_STREAM_PATH);
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).expect("ws schemes are valid");
    url.query_pairs_mut().append_pair("wsTicket", &ticket.ticket);
    Ok(url)
}

/// `GET /api/voice/models`, with benchmarks when asked.
pub async fn speech_models(link: &EnvironmentLink, benchmarks: bool) -> Result<EnvironmentSpeechModels, VoiceRequestError> {
    let query: &[(&str, &str)] = if benchmarks { &[("benchmarks", "1")] } else { &[] };
    send(link, reqwest::Method::GET, "/api/voice/models", query, Body::None, DEFAULT_TIMEOUT).await
}

/// `POST /api/voice/models/download`.
pub async fn download_speech_model(link: &EnvironmentLink, model_id: &str) -> Result<EnvironmentSpeechModelDownload, VoiceRequestError> {
    let body = json(&serde_json::json!({ "modelId": model_id }));
    send(link, reqwest::Method::POST, "/api/voice/models/download", &[], body, INFERENCE_OR_DOWNLOAD_TIMEOUT).await
}

/// `POST /api/voice/models/select`.
pub async fn select_speech_model(link: &EnvironmentLink, model_id: &str) -> Result<EnvironmentSpeechModels, VoiceRequestError> {
    send(link, reqwest::Method::POST, "/api/voice/models/select", &[], json(&serde_json::json!({ "modelId": model_id })), DEFAULT_TIMEOUT).await
}

/// `POST /api/voice/models/cancel`.
pub async fn cancel_speech_model_download(link: &EnvironmentLink, model_id: &str) -> Result<EnvironmentSpeechModels, VoiceRequestError> {
    send(link, reqwest::Method::POST, "/api/voice/models/cancel", &[], json(&serde_json::json!({ "modelId": model_id })), DEFAULT_TIMEOUT).await
}

/// `POST /api/voice/models/remove`.
pub async fn remove_speech_model(link: &EnvironmentLink, model_id: &str) -> Result<EnvironmentSpeechModels, VoiceRequestError> {
    send(link, reqwest::Method::POST, "/api/voice/models/remove", &[], json(&serde_json::json!({ "modelId": model_id })), DEFAULT_TIMEOUT).await
}

/// `getSpeechModelDownloadOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechModelDownloadOutcome {
    Incomplete,
    AlreadySelected,
    NeedsSelection,
}

/// What a finished download means for selection.
pub fn speech_model_download_outcome(download: &EnvironmentSpeechModelDownload, model_id: &str) -> SpeechModelDownloadOutcome {
    if download.cancelled == Some(true) {
        return SpeechModelDownloadOutcome::Incomplete;
    }
    match download.models.iter().find(|model| model.id.as_str() == model_id) {
        Some(model) if model.state == t3code_contracts::EnvironmentSpeechModelState::Installed => {
            if model.active {
                SpeechModelDownloadOutcome::AlreadySelected
            } else {
                SpeechModelDownloadOutcome::NeedsSelection
            }
        }
        _ => SpeechModelDownloadOutcome::Incomplete,
    }
}

/// `POST /api/voice/acceleration`.
pub async fn update_speech_acceleration(link: &EnvironmentLink, acceleration: &SpeechAcceleration) -> Result<EnvironmentSpeechStatus, VoiceRequestError> {
    send(link, reqwest::Method::POST, "/api/voice/acceleration", &[], json(&serde_json::json!({ "acceleration": acceleration })), DEFAULT_TIMEOUT).await
}

/// `POST /api/voice/model-unload-timeout`.
pub async fn update_speech_model_unload_timeout(
    link: &EnvironmentLink,
    timeout: SpeechModelUnloadTimeout,
) -> Result<EnvironmentSpeechStatus, VoiceRequestError> {
    send(link, reqwest::Method::POST, "/api/voice/model-unload-timeout", &[], json(&serde_json::json!({ "timeout": timeout })), DEFAULT_TIMEOUT).await
}

/// `POST /api/voice/post-process`: text and nearby draft text, never audio, to the draft
/// environment's cleanup model.
pub async fn post_process_transcript(
    link: &EnvironmentLink,
    request: &EnvironmentSpeechPostProcessingRequest,
) -> Result<String, VoiceRequestError> {
    let result: EnvironmentSpeechPostProcessingResult =
        send(link, reqwest::Method::POST, "/api/voice/post-process", &[], json(request), INFERENCE_OR_DOWNLOAD_TIMEOUT).await?;
    Ok(result.text)
}
