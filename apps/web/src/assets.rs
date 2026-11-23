use std::borrow::Cow;

use anyhow::anyhow;
use gpui_kit::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
struct Embedded;

/// Serves this crate's embedded assets first, then gpui-kit's bundled icons.
pub struct Assets;

impl Assets {
    /// Returns an embedded file by its path under `apps/web/assets`.
    pub fn get(path: &str) -> Option<Cow<'static, [u8]>> {
        Embedded::get(path).map(|file| file.data)
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // `icons/lucide/x.svg?stroke=2.25` is the icon with an explicit stroke width.
        if let Some((base, width)) = path.split_once("?stroke=") {
            let width: f32 = width.parse()?;
            let svg = Self::get(base).ok_or_else(|| anyhow!("missing asset {base}"))?;
            let svg = std::str::from_utf8(&svg)?
                .replacen(r#"stroke-width="1.75""#, &format!(r#"stroke-width="{width}""#), 1);
            return Ok(Some(Cow::Owned(svg.into_bytes())));
        }
        if let Some(data) = Self::get(path) {
            return Ok(Some(data));
        }
        gpui_kit::assets::Assets
            .load(path)
            .map_err(|_| anyhow!("missing asset {path}"))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = Embedded::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| p.to_string().into())
            .collect();
        out.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(out)
    }
}
