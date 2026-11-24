//! Attachment uploads through the signed upload URL (`attachmentUploadQueue.ts`,
//! `state/attachments.ts`): mint a slot with `attachments.createUploadUrl`, POST the raw bytes to
//! the returned path (the token in the path is the credential), then reference the minted
//! `attachmentId` in the turn. A failed transfer deletes its mint.

use std::time::Duration;

use t3code_contracts::rpc::methods::{AttachmentsCreateUploadUrl, AttachmentsDelete};
use t3code_contracts::{
    AttachmentCreateUploadUrlInput, AttachmentCreateUploadUrlInputFile, AttachmentCreateUploadUrlInputImage,
    AttachmentCreateUploadUrlInputImageMimeType as ImageMime, AttachmentDeleteInput,
};

use super::commands::{CommandError, EnvironmentCommands};
use crate::environment::endpoint_url;
use crate::rpc::http::http_client;

/// The web's transfer timeout (`UPLOAD_TIMEOUT_MS`).
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What an upload sends: images go to the provider as images, files as paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadKind {
    Image,
    File,
}

/// Why an upload failed, with the web's copy.
#[derive(Debug, Clone, thiserror::Error)]
pub enum UploadError {
    #[error("Unsupported image type")]
    UnsupportedImage,
    #[error("{0}")]
    Mint(CommandError),
    #[error("Upload rejected ({0})")]
    Rejected(u16),
    #[error("Upload timed out")]
    TimedOut,
    #[error("Upload failed")]
    Failed,
}

fn image_mime(mime: &str) -> Option<ImageMime> {
    Some(match mime.to_ascii_lowercase().as_str() {
        "image/gif" => ImageMime::ImageGif,
        "image/jpeg" => ImageMime::ImageJpeg,
        "image/png" => ImageMime::ImagePng,
        "image/webp" => ImageMime::ImageWebp,
        _ => return None,
    })
}

impl EnvironmentCommands {
    /// Uploads `bytes` and returns the server's `attachmentId`.
    pub async fn upload_attachment(&self, kind: UploadKind, name: &str, mime: &str, bytes: Vec<u8>) -> Result<String, UploadError> {
        let size_bytes = bytes.len() as u64;
        let (input, content_type) = match kind {
            UploadKind::Image => {
                let mime_type = image_mime(mime).ok_or(UploadError::UnsupportedImage)?;
                let input = AttachmentCreateUploadUrlInputImage { r#type: None, name: name.to_owned(), mime_type, size_bytes };
                (AttachmentCreateUploadUrlInput::Image(input), mime.to_ascii_lowercase())
            }
            UploadKind::File => {
                let mime_type = mime.to_ascii_lowercase();
                let input = AttachmentCreateUploadUrlInputFile { name: name.to_owned(), mime_type: mime_type.clone(), size_bytes };
                (AttachmentCreateUploadUrlInput::File(input), mime_type)
            }
        };
        let session = self.session().await.map_err(UploadError::Mint)?;
        let minted = session
            .client
            .call::<AttachmentsCreateUploadUrl>(input)
            .await
            .map_err(|error| UploadError::Mint(CommandError::Refused(error.to_string())))?;
        let url = endpoint_url(&session.prepared.http_base_url, &minted.relative_url);
        let sent = http_client()
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .timeout(UPLOAD_TIMEOUT)
            .body(bytes)
            .send()
            .await;
        let failure = match sent {
            Ok(response) if response.status().is_success() => return Ok(minted.attachment_id),
            Ok(response) => UploadError::Rejected(response.status().as_u16()),
            Err(error) if error.is_timeout() => UploadError::TimedOut,
            Err(_) => UploadError::Failed,
        };
        self.delete_attachment(minted.attachment_id).await;
        Err(failure)
    }

    /// Deletes a pending upload the draft no longer holds. Best effort, like the web's.
    pub async fn delete_attachment(&self, attachment_id: String) {
        if let Ok(session) = self.session().await {
            _ = session.client.call::<AttachmentsDelete>(AttachmentDeleteInput { attachment_id }).await;
        }
    }
}
