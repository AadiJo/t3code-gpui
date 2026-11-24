//! Composer attachments on live data (`useComposerAttachmentIntake.ts`,
//! `composerAttachmentFiles.ts`, `attachmentUploadQueue.ts`, `composerContextRecords.ts`):
//! which files the composer accepts, the signed upload each one starts the moment it lands, and
//! the turn's `attachments` and context records once every upload is ready.
//!
//! Uploads outlive the composer: [`AttachmentUploads`] keeps each one's state by context id, so a
//! thread switched away from and back shows where its uploads got to.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui_kit::{App, SharedString, WeakEntity};
use t3code_client_runtime::operations::{EnvironmentCommands, UploadKind};
use t3code_contracts::{ChatAttachment, ChatFileAttachment, ChatImageAttachment, EnvironmentId, OrchestrationMessageContext, PastedTextAttachmentSource};

use super::StateBridge;
use crate::components::chat::composer::ChatComposer;
use crate::composer_draft_store::{ComposerAttachment, ComposerAttachmentKind, UploadState};
use crate::composer_editor_mentions::sanitize_context_label;
use crate::i18n::t;

/// `PROVIDER_SEND_TURN_MAX_ATTACHMENTS`.
pub const MAX_ATTACHMENTS: usize = 100;
/// `PROVIDER_SEND_TURN_MAX_IMAGE_BYTES`.
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
/// `PROVIDER_SEND_TURN_MAX_FILE_BYTES`: the cap on any advertised upload limit.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// `IMAGE_ATTACHMENT_MAX_EDGE`: larger PNG and JPEG images are scaled down before upload.
const IMAGE_MAX_EDGE: u32 = 2000;
/// `ATTACHMENT_ONLY_BOOTSTRAP_PROMPT`: the text of a message that is only attachments.
pub const ATTACHMENT_ONLY_PROMPT: &str =
    "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";

/// What a dropped, picked or pasted file becomes (`classifyComposerAttachmentFile`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileClass {
    /// Sent to the provider as an image, with this MIME type.
    Image(&'static str),
    /// Staged as a file, with this MIME type.
    File(String),
    /// An image type providers do not take.
    UnsupportedImage,
    /// HEIC or HEIF: converted to JPEG before it is attached (`convertHeicImage`).
    Heic,
}

/// The MIME type a browser reports for `name`, from its extension.
pub fn mime_from_name(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "dng" => "image/x-adobe-dng",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        _ => return None,
    })
}

/// Sorts a file into image or file, with the web's MIME fixes for files staged as files.
pub fn classify(name: &str, mime: &str) -> FileClass {
    let lower = mime.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    let lower_name = name.to_ascii_lowercase();
    if lower == "image/svg+xml" || lower_name.ends_with(".svg") {
        return FileClass::File("image/svg+xml".into());
    }
    if lower_name.ends_with(".dng") || matches!(lower.as_str(), "image/dng" | "image/x-dng" | "image/x-adobe-dng") {
        return FileClass::File("image/x-adobe-dng".into());
    }
    let generic = lower.is_empty() || lower == "application/octet-stream";
    // `isHeicImageFile`: Finder and some browsers omit the MIME type when dragging HEIC photos.
    if matches!(lower.as_str(), "image/heic" | "image/heif") || (generic && (lower_name.ends_with(".heic") || lower_name.ends_with(".heif"))) {
        return FileClass::Heic;
    }
    if generic && let Some(image) = mime_from_name(name).and_then(supported_image) {
        return FileClass::Image(image);
    }
    if !lower.starts_with("image/") {
        let mime = if mime.is_empty() { "application/octet-stream".to_owned() } else { mime.to_owned() };
        return FileClass::File(mime);
    }
    match supported_image(&lower) {
        Some(image) => FileClass::Image(image),
        // Previewable workspace image types stage as files; the rest are refused.
        None if [".bmp", ".ico", ".avif", ".tif", ".tiff"].iter().any(|ext| lower_name.ends_with(ext)) => FileClass::File(lower),
        None => FileClass::UnsupportedImage,
    }
}

fn supported_image(mime: &str) -> Option<&'static str> {
    ["image/gif", "image/jpeg", "image/png", "image/webp"].into_iter().find(|m| *m == mime)
}

/// A file offered to the composer, before the intake rules run.
pub struct Candidate {
    pub name: String,
    pub mime: String,
    pub size_bytes: u64,
    pub kind: ComposerAttachmentKind,
}

/// The accepted attachments, the HEIC photos still to convert, and the last refusal's message.
pub struct Intake {
    pub accepted: Vec<ComposerAttachment>,
    pub converting: Vec<HeicJob>,
    pub error: Option<SharedString>,
}

/// Where a HEIC photo's bytes are.
pub enum HeicSource {
    Bytes(Arc<[u8]>),
    Path(PathBuf),
}

/// A HEIC or HEIF photo accepted into the draft that still has to become a JPEG.
pub struct HeicJob {
    pub name: String,
    pub source: HeicSource,
}

impl HeicJob {
    /// Reads and converts the photo (blocking: run it on a background thread). The attachment is
    /// `<name>.jpg`, as `fileNameForMimeType` names it; the error is the intake's refusal text.
    pub fn convert(self) -> Result<ComposerAttachment, SharedString> {
        let bytes = match self.source {
            HeicSource::Bytes(bytes) => Some(bytes.to_vec()),
            HeicSource::Path(path) => std::fs::read(path).ok(),
        };
        let Some(jpeg) = bytes.and_then(|bytes| convert_heic(&bytes)) else {
            return Err(t!("''{name}'' could not be read as an image.", name = self.name));
        };
        let base = self.name.rsplit_once('.').filter(|(base, _)| !base.is_empty()).map_or(self.name.as_str(), |(base, _)| base);
        let name = format!("{}.jpg", if base.is_empty() { "image" } else { base });
        Ok(ComposerAttachment {
            context_id: format!("image_{}", uuid::Uuid::new_v4()).into(),
            name: name.into(),
            size_bytes: jpeg.len() as u64,
            kind: ComposerAttachmentKind::Image { bytes: Arc::from(jpeg), mime: "image/jpeg".into() },
            upload: UploadState::Uploading,
        })
    }
}

/// Applies the web's intake rules: at most [`MAX_ATTACHMENTS`] per message, supported image types
/// only, files within the server's staging limit (`None`: the server takes no files). `held`
/// counts the draft's attachments and the photos still converting.
pub fn intake(candidates: Vec<Candidate>, held: usize, file_limit: Option<u64>) -> Intake {
    let mut reserved = held;
    let mut accepted = Vec::new();
    let mut converting = Vec::new();
    let mut error = None;
    for candidate in candidates {
        if reserved >= MAX_ATTACHMENTS {
            error = Some(t!("You can attach up to {max} files per message.", max = MAX_ATTACHMENTS));
            continue;
        }
        let class = match &candidate.kind {
            ComposerAttachmentKind::PastedText { .. } => FileClass::File(candidate.mime.clone()),
            _ => classify(&candidate.name, &candidate.mime),
        };
        let (prefix, kind) = match class {
            FileClass::UnsupportedImage => {
                error = Some(t!(
                    "''{name}'' is not a supported image type. Attach GIF, HEIC, HEIF, JPEG, PNG, or WebP images.",
                    name = candidate.name
                ));
                continue;
            }
            FileClass::Heic => {
                // Decoding takes seconds; the composer runs it off the UI thread and holds the slot.
                let source = match candidate.kind {
                    ComposerAttachmentKind::Image { bytes, .. } => HeicSource::Bytes(bytes),
                    ComposerAttachmentKind::File { path } => HeicSource::Path(PathBuf::from(path.as_ref())),
                    ComposerAttachmentKind::PastedText { .. } => unreachable!("pasted text stages as a file"),
                };
                converting.push(HeicJob { name: candidate.name, source });
                reserved += 1;
                continue;
            }
            FileClass::Image(mime) => {
                let bytes = match candidate.kind {
                    ComposerAttachmentKind::Image { bytes, .. } => bytes,
                    ComposerAttachmentKind::File { path } => match std::fs::read(path.as_ref()) {
                        Ok(bytes) => Arc::from(bytes),
                        Err(_) => {
                            error = Some(t!("''{name}'' could not be read as an image.", name = candidate.name));
                            continue;
                        }
                    },
                    ComposerAttachmentKind::PastedText { .. } => unreachable!("pasted text stages as a file"),
                };
                ("image", ComposerAttachmentKind::Image { bytes, mime: mime.into() })
            }
            FileClass::File(_) => {
                let Some(limit) = file_limit else {
                    error = Some(t!("This server does not support file attachments."));
                    continue;
                };
                if candidate.size_bytes == 0 {
                    error = Some(t!("''{name}'' is empty or could not be read.", name = candidate.name));
                    continue;
                }
                if candidate.size_bytes > limit {
                    error = Some(too_large_message(&candidate.name, limit));
                    continue;
                }
                ("file", candidate.kind)
            }
        };
        let name = if candidate.name.is_empty() { prefix.to_owned() } else { candidate.name };
        accepted.push(ComposerAttachment {
            context_id: format!("{prefix}_{}", uuid::Uuid::new_v4()).into(),
            name: name.into(),
            size_bytes: candidate.size_bytes,
            kind,
            upload: UploadState::Uploading,
        });
        reserved += 1;
    }
    Intake { accepted, converting, error }
}

/// The longest edge an attached image keeps (`IMAGE_ATTACHMENT_MAX_EDGE`).
const IMAGE_ATTACHMENT_MAX_EDGE: u32 = 2000;
/// The web's first JPEG quality step (`QUALITY_STEPS[0]`).
const JPEG_QUALITY: u8 = 92;

/// Converts HEIC or HEIF bytes to a JPEG of at most [`IMAGE_ATTACHMENT_MAX_EDGE`] on its longest
/// edge (`convertHeicImage`). The platform decodes: `sips` on macOS, `heif-convert` (libheif)
/// elsewhere when installed. `None` when nothing could read it.
pub fn convert_heic(bytes: &[u8]) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("t3code-heic-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).ok()?;
    let (input, decoded) = (dir.join("in.heic"), dir.join("out.jpg"));
    let result = (|| {
        std::fs::write(&input, bytes).ok()?;
        let ran = if cfg!(target_os = "macos") {
            std::process::Command::new("/usr/bin/sips").args(["-s", "format", "jpeg"]).arg(&input).arg("--out").arg(&decoded).output()
        } else {
            std::process::Command::new("heif-convert").args(["-q", "100"]).arg(&input).arg(&decoded).output()
        };
        if !ran.ok()?.status.success() {
            return None;
        }
        let image = image::load_from_memory(&std::fs::read(&decoded).ok()?).ok()?;
        let longest = image.width().max(image.height());
        let image = if longest > IMAGE_ATTACHMENT_MAX_EDGE {
            let scale = IMAGE_ATTACHMENT_MAX_EDGE as f64 / longest as f64;
            let width = ((image.width() as f64 * scale).round() as u32).clamp(1, IMAGE_ATTACHMENT_MAX_EDGE);
            let height = ((image.height() as f64 * scale).round() as u32).clamp(1, IMAGE_ATTACHMENT_MAX_EDGE);
            image.resize_exact(width, height, image::imageops::FilterType::Triangle)
        } else {
            image
        };
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&image.to_rgb8()).ok()?;
        Some(out)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// `fileAttachmentTooLargeMessage`.
fn too_large_message(name: &str, limit: u64) -> SharedString {
    const MB: u64 = 1024 * 1024;
    let size: SharedString = if limit >= MB && limit % MB == 0 {
        t!("{count} MB", count = limit / MB)
    } else if limit >= 1024 && limit % 1024 == 0 {
        t!("{count} KB", count = limit / 1024)
    } else {
        t!("{count, plural, one {{count} byte} other {{count} bytes}}", count = limit)
    };
    t!("''{name}'' exceeds the {size} attachment limit.", name = name, size = size)
}

/// The staging limit a server's config allows (`fileAttachmentStagingLimit`): unknown
/// capabilities admit the contract's cap, a server without uploads admits no files.
pub fn file_limit(config: Option<&t3code_contracts::ServerConfig>) -> Option<u64> {
    let Some(config) = config else { return Some(MAX_FILE_BYTES) };
    config.environment.capabilities.file_attachments.as_ref().map(|f| f.max_upload_bytes.min(MAX_FILE_BYTES))
}

/// Where an upload got to.
#[derive(Clone, Debug)]
pub enum UploadPhase {
    Uploading,
    Ready(ReadyUpload),
    Failed(SharedString),
}

/// A finished upload: what the turn references.
#[derive(Clone, Debug)]
pub struct ReadyUpload {
    pub attachment_id: String,
    pub mime: String,
    pub size_bytes: u64,
}

/// Every upload this window started, by the attachment's context id. Lives on the UI thread.
pub struct AttachmentUploads;

thread_local! {
    static UPLOADS: std::cell::RefCell<HashMap<SharedString, (EnvironmentId, UploadPhase)>> = Default::default();
}

impl AttachmentUploads {
    /// The upload state of `context_id`, as the composer shows it.
    pub fn state(context_id: &str) -> Option<UploadState> {
        UPLOADS.with_borrow(|uploads| {
            Some(match uploads.get(context_id)?.1 {
                UploadPhase::Uploading => UploadState::Uploading,
                UploadPhase::Ready(_) => UploadState::Uploaded,
                UploadPhase::Failed(_) => UploadState::Failed,
            })
        })
    }

    fn ready(context_id: &str, environment: &EnvironmentId) -> Option<ReadyUpload> {
        UPLOADS.with_borrow(|uploads| match uploads.get(context_id)? {
            (env, UploadPhase::Ready(ready)) if env == environment => Some(ready.clone()),
            _ => None,
        })
    }

    fn set(context_id: SharedString, environment: EnvironmentId, phase: UploadPhase) {
        UPLOADS.with_borrow_mut(|uploads| uploads.insert(context_id, (environment, phase)));
    }

    /// Forgets an attachment the composer dropped, deleting its pending upload.
    pub fn release(context_id: &str, cx: &mut App) {
        let Some((environment, phase)) = UPLOADS.with_borrow_mut(|uploads| uploads.remove(context_id)) else { return };
        if let UploadPhase::Ready(ready) = phase
            && let Some(env) = StateBridge::environment(&environment, cx)
        {
            let commands = env.read(cx).commands.clone();
            StateBridge::spawn(async move { commands.delete_attachment(ready.attachment_id).await }, cx).detach();
        }
    }

    /// Registers a persisted draft attachment's upload again after a restart and returns the
    /// state its tile shows: uploaded when the draft kept where it went, else waiting for the
    /// composer to upload it (`ComposerEvent::AttachmentsRestored`).
    pub fn restore(context_id: &str, upload: Option<&t3code_client_runtime::state::drafts::DraftUpload>, mime: &str, size_bytes: u64) -> UploadState {
        if let Some(state) = Self::state(context_id) {
            return state;
        }
        match upload {
            Some(upload) => {
                let ready = ReadyUpload { attachment_id: upload.attachment_id.clone(), mime: mime.to_owned(), size_bytes };
                Self::set(context_id.into(), upload.environment_id.clone(), UploadPhase::Ready(ready));
                UploadState::Uploaded
            }
            None => UploadState::Uploading,
        }
    }

    /// Where `context_id` finished uploading, as a draft keeps it.
    pub fn persisted(context_id: &str) -> Option<t3code_client_runtime::state::drafts::DraftUpload> {
        UPLOADS.with_borrow(|uploads| match uploads.get(context_id)? {
            (environment, UploadPhase::Ready(ready)) => {
                Some(t3code_client_runtime::state::drafts::DraftUpload { attachment_id: ready.attachment_id.clone(), environment_id: environment.clone() })
            }
            _ => None,
        })
    }

    /// Whether this window has started (or finished) uploading `context_id`.
    pub fn known(context_id: &str) -> bool {
        UPLOADS.with_borrow(|uploads| uploads.contains_key(context_id))
    }

    /// Forgets uploads a sent message now owns (the server keeps them).
    pub fn forget_sent(context_ids: impl IntoIterator<Item = SharedString>) {
        UPLOADS.with_borrow_mut(|uploads| {
            for id in context_ids {
                uploads.remove(&id);
            }
        });
    }
}

/// Where sent images are cached by attachment id, so timeline rows can show them.
static IMAGE_CACHE: OnceLock<PathBuf> = OnceLock::new();

/// Where files restored into a draft ("Edit from here") are kept while the draft holds them.
static RESTORED_FILES: OnceLock<PathBuf> = OnceLock::new();

/// Sets the image cache and the restored-files folder under the app's data directory. Called
/// once at bridge start.
pub fn init_image_cache(data_dir: &std::path::Path) {
    let dir = data_dir.join("attachment-images");
    _ = std::fs::create_dir_all(&dir);
    _ = IMAGE_CACHE.set(dir);
    _ = RESTORED_FILES.set(data_dir.join("draft-files"));
}

/// Fetches a sent message's attachments back into composer attachments ("Edit from here",
/// `prepareRevertedMessageAttachments`): each through `assets.createUrl` and the signed URL. They
/// keep the context ids of the message's records, so the chips in its text still bind to them,
/// and upload again when added. Fails with the web's message when any cannot be restored.
pub async fn fetch_message_attachments(
    link: crate::live::rpc::RpcLink,
    attachments: Vec<ChatAttachment>,
    context: Option<OrchestrationMessageContext>,
) -> Result<Vec<ComposerAttachment>, SharedString> {
    use t3code_contracts::rpc::methods::AssetsCreateUrl;
    use t3code_contracts::{AssetCreateUrlInput, AssetResource, AssetResourceAttachment};
    let records = context.map(|c| c.records).unwrap_or_default();
    let context_id_of = |attachment_id: &str, kind: &str| {
        records
            .iter()
            .find(|r| r.get("attachmentId").and_then(|v| v.as_str()) == Some(attachment_id))
            .and_then(|r| r.get("contextId").and_then(|v| v.as_str()))
            .map(SharedString::from)
            .unwrap_or_else(|| SharedString::from(format!("{kind}_{}", uuid::Uuid::new_v4())))
    };
    let session = link.session().await?;
    let mut restored = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let (id, name, mime, size, image, pasted) = match &attachment {
            ChatAttachment::Image(a) => (a.id.clone(), a.name.clone(), a.mime_type.clone(), a.size_bytes, true, false),
            ChatAttachment::File(a) => (a.id.clone(), a.name.clone(), a.mime_type.clone(), a.size_bytes, false, a.source.is_some()),
            ChatAttachment::Unknown(_) => return Err(t!("This message has an attachment that cannot be restored.")),
        };
        let resource = AssetResource::Attachment(AssetResourceAttachment {
            attachment_id: id.clone(),
            file_name: Some(name.clone()),
            mime_type: Some(mime.clone()),
            disposition: None,
            render_intent: None,
        });
        let url = session
            .client
            .call::<AssetsCreateUrl>(AssetCreateUrlInput { resource })
            .await
            .map_err(|error| crate::live::rpc::error_message(&error))?;
        let bytes = link
            .link
            .fetch_relative(reqwest::Method::GET, &url.relative_url, None)
            .await
            .map_err(|_| t!("Could not restore attachment: {name}", name = name.clone()))?;
        let kind = if image {
            ComposerAttachmentKind::Image { bytes: Arc::from(bytes.as_ref()), mime: mime.clone().into() }
        } else if pasted {
            ComposerAttachmentKind::PastedText { text: Arc::from(String::from_utf8_lossy(&bytes).as_ref()) }
        } else {
            let dir = RESTORED_FILES.get().cloned().unwrap_or_else(std::env::temp_dir).join(uuid::Uuid::new_v4().simple().to_string());
            let path = dir.join(name.replace(['/', '\\'], "_"));
            std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &bytes)).map_err(|_| t!("Could not restore attachment: {name}", name = name.clone()))?;
            ComposerAttachmentKind::File { path: path.to_string_lossy().into_owned().into() }
        };
        restored.push(ComposerAttachment {
            context_id: context_id_of(&id, if image { "image" } else { "file" }),
            name: name.into(),
            size_bytes: size,
            kind,
            upload: UploadState::Uploading,
        });
    }
    Ok(restored)
}

/// The cache file of an image: its attachment id with the extension of its type, which GPUI's
/// image loader reads the format from.
fn cache_path(attachment_id: &str, mime: &str) -> Option<PathBuf> {
    if attachment_id.contains(['/', '\\']) {
        return None;
    }
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    };
    Some(IMAGE_CACHE.get()?.join(format!("{attachment_id}.{ext}")))
}

/// The cached file of an attachment's image, when it is on disk.
pub fn cached_image(attachment_id: &str, mime: &str) -> Option<PathBuf> {
    cache_path(attachment_id, mime).filter(|path| path.is_file())
}

/// Writes an image's bytes to the cache.
pub fn cache_image(attachment_id: &str, mime: &str, bytes: &[u8]) {
    if let Some(path) = cache_path(attachment_id, mime) {
        _ = std::fs::write(path, bytes);
    }
}

/// Scales a PNG or JPEG down to [`IMAGE_MAX_EDGE`] (`fitImageToMaxEdge`); other images and images
/// that already fit come back unchanged.
fn fit_image(bytes: Vec<u8>, mime: &str) -> Vec<u8> {
    let format = match mime {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        _ => return bytes,
    };
    let Ok(decoded) = image::load_from_memory_with_format(&bytes, format) else { return bytes };
    if decoded.width().max(decoded.height()) <= IMAGE_MAX_EDGE {
        return bytes;
    }
    let scaled = decoded.resize(IMAGE_MAX_EDGE, IMAGE_MAX_EDGE, image::imageops::FilterType::Triangle);
    let mut out = Cursor::new(Vec::new());
    let written = match format {
        image::ImageFormat::Jpeg => scaled.to_rgb8().write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92)),
        _ => scaled.write_to(&mut out, format),
    };
    if written.is_ok() { out.into_inner() } else { bytes }
}

/// Starts uploading each attachment to `environment_id`, marking the composer's tiles as they
/// finish. A failed upload shows on its tile and blocks the send until it is removed.
pub fn start_uploads(environment_id: &EnvironmentId, attachments: &[ComposerAttachment], composer: &WeakEntity<ChatComposer>, cx: &mut App) {
    let Some(environment) = StateBridge::environment(environment_id, cx) else {
        for attachment in attachments {
            let reason = t!("Upload failed");
            AttachmentUploads::set(attachment.context_id.clone(), environment_id.clone(), UploadPhase::Failed(reason));
            _ = composer.update(cx, |c, cx| c.set_upload_state(&attachment.context_id, UploadState::Failed, cx));
        }
        return;
    };
    let commands = environment.read(cx).commands.clone();
    for attachment in attachments {
        AttachmentUploads::set(attachment.context_id.clone(), environment_id.clone(), UploadPhase::Uploading);
        let task = StateBridge::spawn(upload(commands.clone(), attachment.clone()), cx);
        let (context_id, environment_id, composer) = (attachment.context_id.clone(), environment_id.clone(), composer.clone());
        cx.spawn(async move |cx| {
            let result = task.await.unwrap_or_else(|| Err(t!("Upload failed")));
            cx.update(|cx| {
                let (phase, state) = match result {
                    Ok(ready) => (UploadPhase::Ready(ready), UploadState::Uploaded),
                    Err(reason) => (UploadPhase::Failed(reason), UploadState::Failed),
                };
                AttachmentUploads::set(context_id.clone(), environment_id, phase);
                _ = composer.update(cx, |c, cx| c.set_upload_state(&context_id, state, cx));
            });
        })
        .detach();
    }
}

async fn upload(commands: EnvironmentCommands, attachment: ComposerAttachment) -> Result<ReadyUpload, SharedString> {
    let name = attachment.name.to_string();
    let (kind, mime, bytes) = match attachment.kind {
        ComposerAttachmentKind::Image { bytes, mime } => {
            let mime = mime.to_string();
            let source = bytes.to_vec();
            let fitted = tokio::task::spawn_blocking({
                let mime = mime.clone();
                move || fit_image(source, &mime)
            })
            .await
            .map_err(|_| t!("Upload failed"))?;
            (UploadKind::Image, mime, fitted)
        }
        ComposerAttachmentKind::File { path } => {
            let mime = match classify(&name, mime_from_name(&name).unwrap_or_default()) {
                FileClass::File(mime) => mime,
                _ => mime_from_name(&name).unwrap_or("application/octet-stream").to_owned(),
            };
            let bytes = tokio::fs::read(path.as_ref()).await.map_err(|_| t!("Original file is no longer available"))?;
            (UploadKind::File, mime, bytes)
        }
        ComposerAttachmentKind::PastedText { text } => (UploadKind::File, PASTED_TEXT_MIME.to_owned(), text.as_bytes().to_vec()),
    };
    let size_bytes = bytes.len() as u64;
    let cached = (kind == UploadKind::Image).then(|| bytes.clone());
    let attachment_id = commands
        .upload_attachment(kind, &name, &mime, bytes)
        .await
        .map_err(|error| SharedString::from(error.to_string()))?;
    if let Some(bytes) = cached {
        cache_image(&attachment_id, &mime, &bytes);
    }
    Ok(ReadyUpload { attachment_id, mime, size_bytes })
}

/// The MIME type of a large paste staged as a file.
pub const PASTED_TEXT_MIME: &str = "text/plain;charset=utf-8";

/// What a send carries for its attachments: the `attachments` list and the context records
/// that bind each chip to its upload (`attachmentContextRecord`).
pub struct TurnAttachments {
    pub attachments: Vec<ChatAttachment>,
    pub context: Option<OrchestrationMessageContext>,
}

/// The turn's attachments, or `None` while any upload is not ready for `environment_id`.
pub fn turn_attachments(environment_id: &EnvironmentId, attachments: &[ComposerAttachment]) -> Option<TurnAttachments> {
    let mut list = Vec::with_capacity(attachments.len());
    let mut records = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let ready = AttachmentUploads::ready(&attachment.context_id, environment_id)?;
        let (kind, chat) = match &attachment.kind {
            ComposerAttachmentKind::Image { .. } => (
                "image",
                ChatAttachment::Image(ChatImageAttachment {
                    id: ready.attachment_id.clone(),
                    name: attachment.name.to_string(),
                    mime_type: ready.mime.clone(),
                    size_bytes: ready.size_bytes,
                    source: None,
                }),
            ),
            other => (
                "file",
                ChatAttachment::File(ChatFileAttachment {
                    id: ready.attachment_id.clone(),
                    name: attachment.name.to_string(),
                    mime_type: ready.mime.clone(),
                    size_bytes: ready.size_bytes,
                    source: matches!(other, ComposerAttachmentKind::PastedText { .. }).then_some(PastedTextAttachmentSource {}),
                }),
            ),
        };
        records.push(serde_json::json!({
            "version": 1,
            "contextId": attachment.context_id.as_ref(),
            "label": sanitize_context_label(&attachment.name, kind),
            "attachmentId": ready.attachment_id,
            "name": attachment.name.as_ref(),
            "mimeType": ready.mime,
            "sizeBytes": ready.size_bytes,
            "kind": kind,
        }));
        list.push(chat);
    }
    let context = (!records.is_empty()).then(|| OrchestrationMessageContext { version: 1.0, guest: None, records });
    Some(TurnAttachments { attachments: list, context })
}
