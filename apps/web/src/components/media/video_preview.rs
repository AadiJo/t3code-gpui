//! A video or another file this client cannot draw. GPUI here has no video
//! decoder (no GStreamer or FFmpeg in the dependency tree), so in place of
//! `MediaVideoPlayer.tsx` this shows the file and opens it with the OS
//! default app, the native client's route for every kind spec 07 leaves to
//! the OS. The file is fetched through its asset URL first when it is remote.

use gpui_kit::{App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div, prelude::*, px};

use crate::{
    ActiveAppTheme as _,
    components::{
        icons::{Icon, IconName, IconSize},
        ui::button::{Button, ButtonSize, ButtonVariant},
    },
    file_preview::{FilePreviewKind, format_file_size},
    file_source::SharedWorkspaceFiles,
    i18n::t,
};

/// Fetches a workspace file to this machine and opens it with the default app.
pub fn open_with_default_app(files: &SharedWorkspaceFiles, path: &str, cx: &mut App) {
    let fetch = files.local_copy(path.to_owned());
    cx.spawn(async move |cx| {
        if let Ok(local) = cx.background_spawn(fetch).await {
            cx.update(|cx| cx.open_with_system(&local));
        }
    })
    .detach();
}

/// The card for a file opened outside the app.
#[derive(IntoElement)]
pub struct ExternalFileCard {
    files: SharedWorkspaceFiles,
    path: SharedString,
    kind: FilePreviewKind,
    size: Option<u64>,
}

impl ExternalFileCard {
    pub fn new(files: SharedWorkspaceFiles, path: SharedString, kind: FilePreviewKind, size: Option<u64>) -> Self {
        Self { files, path, kind, size }
    }
}

impl RenderOnce for ExternalFileCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let colors = theme.colors;
        let name: SharedString = self.path.rsplit('/').next().unwrap_or(&self.path).to_owned().into();
        let (icon, title) = match self.kind {
            FilePreviewKind::Video => (IconName::Film, t!("Video preview")),
                        _ => (IconName::File, name.clone()),
        };
        let detail = match self.size {
            Some(size) => format!("{name} · {}", format_file_size(size)),
            None => name.to_string(),
        };
        let files = self.files.clone();
        let path = self.path.clone();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.))
            .child(
                div()
                    .max_w(px(360.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.))
                    .text_center()
                    .child(
                        div()
                            .size(px(48.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme.radii.xl)
                            .bg(colors.muted)
                            .child(Icon::new(icon).size(IconSize::Xl).color(colors.muted_foreground)),
                    )
                    .child(div().text_size(px(13.)).text_color(colors.text).child(title))
                    .child(div().text_size(px(12.)).text_color(colors.muted_foreground).child(detail))
                    .child(
                        Button::new("open-default-app")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .icon(IconName::SquareArrowOutUpRight)
                            .label(t!("Open", context = "verb, opens a file"))
                            .on_click(move |_, _, cx| open_with_default_app(&files, &path, cx)),
                    ),
            )
    }
}
