//! Where media bytes come from and how an image becomes something GPUI draws.
//! Ports the parts of `client-runtime/src/mediaSource.ts` and
//! `AttachmentImage.tsx` a native client needs: bytes arrive through
//! `assets.createUrl` (see [`WorkspaceFiles::read_asset`](crate::file_source::WorkspaceFiles::read_asset))
//! and decode off the main thread.

use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Image, ImageFormat, RenderImage, SharedString, Task};

use crate::file_source::{FileError, FileFuture, SharedWorkspaceFiles};

/// The bytes behind one media item.
#[derive(Clone)]
pub enum MediaBytes {
    /// Already in memory (a pasted image, a fixture).
    Ready(Arc<[u8]>),
    /// A workspace file, read through a signed asset URL.
    Workspace { files: SharedWorkspaceFiles, path: SharedString },
}

impl MediaBytes {
    /// Fetches the bytes; run the future on the background executor.
    pub fn fetch(&self) -> FileFuture<Arc<[u8]>> {
        match self {
            Self::Ready(bytes) => {
                let bytes = bytes.clone();
                Box::pin(async move { Ok(bytes) })
            }
            Self::Workspace { files, path } => {
                let read = files.read_asset(path.to_string());
                Box::pin(async move { read.await.map(|asset| asset.bytes) })
            }
        }
    }
}

/// A decoded image and the bytes it came from (kept for copy and save).
#[derive(Clone)]
pub struct DecodedImage {
    pub render: Arc<RenderImage>,
    pub bytes: Arc<[u8]>,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
}

impl DecodedImage {
    /// The image as a GPUI clipboard image.
    pub fn clipboard_image(&self) -> Image {
        Image::from_bytes(self.format, self.bytes.to_vec())
    }
}

/// The GPUI decoder for a file name, sniffing the bytes when the extension is unknown.
pub fn image_format(name: &str, bytes: &[u8]) -> Option<ImageFormat> {
    let lower = name.to_ascii_lowercase();
    let by_name = match lower.rsplit('.').next().unwrap_or("") {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::Webp),
        "svg" => Some(ImageFormat::Svg),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "ico" => Some(ImageFormat::Ico),
        _ => None,
    };
    by_name.or_else(|| match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xff, 0xd8, 0xff, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', b'8', ..] => Some(ImageFormat::Gif),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some(ImageFormat::Webp),
        [b'B', b'M', ..] => Some(ImageFormat::Bmp),
        _ => None,
    })
}

/// Loads and decodes an image on the background executor. AVIF and HEIC
/// have no decoder here and fail with the web's "Could not decode this image."
pub fn load_image(source: &MediaBytes, name: SharedString, cx: &App) -> Task<Result<DecodedImage, FileError>> {
    let svg = cx.svg_renderer();
    let fetch = source.fetch();
    cx.background_spawn(async move {
        let bytes = fetch.await?;
        let format = image_format(&name, &bytes).ok_or_else(|| FileError::new("Could not decode this image."))?;
        let image = Image::from_bytes(format, bytes.to_vec());
        let render = image.to_image_data(svg).map_err(|_| FileError::new("Could not decode this image."))?;
        let size = render.size(0);
        Ok(DecodedImage { render, bytes, format, width: size.width.0 as u32, height: size.height.0 as u32 })
    })
}
