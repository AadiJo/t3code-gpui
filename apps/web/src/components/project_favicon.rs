//! `ProjectFavicon.tsx`: the icon drawn for a project in sidebar rows, folder
//! headers and the chat breadcrumb. A project's icon override wins; without
//! one the project gets its fetched favicon, else the monogram derived from its
//! name.

use gpui_kit::{App, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _, StyledImage as _, Window, div, px};
use t3code_contracts::OrchestrationProjectShellProjectIcon as ProjectIcon;

use crate::ActiveAppTheme as _;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::project_monogram::ProjectMonogram;
use crate::project_icon_colors::{ProjectIdentity, project_icon_ink};

/// What a project draws besides its name: the icon override and the fetched favicon.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectIconSource {
    pub icon: Option<ProjectIcon>,
    pub favicon: Option<std::sync::Arc<std::path::Path>>,
}

/// A project's icon at 16px (`size-4`), or another size for the breadcrumb.
#[derive(IntoElement)]
pub struct ProjectFavicon {
    name: SharedString,
    icon: Option<ProjectIcon>,
    favicon: Option<std::sync::Arc<std::path::Path>>,
    size: Pixels,
}

impl ProjectFavicon {
    /// The icon for a project with this display name.
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self { name: name.into(), icon: None, favicon: None, size: px(16.) }
    }

    /// The project's override and favicon (`ProjectFavicon`'s `project`).
    pub fn source(mut self, source: Option<&ProjectIconSource>) -> Self {
        if let Some(source) = source {
            self.icon = source.icon.clone();
            self.favicon = source.favicon.clone();
        }
        self
    }

    /// The project's icon override (`project.projectIcon`).
    pub fn icon(mut self, icon: Option<ProjectIcon>) -> Self {
        self.icon = icon;
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for ProjectFavicon {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = self.size;
        // `AutomaticProjectFavicon`: without an override, a fetched favicon shows at 14px
        // (`size-3.5 rounded-[25%] object-contain`).
        if self.icon.is_none()
            && let Some(path) = self.favicon
        {
            let side = size * 0.875;
            return gpui_kit::img(path.to_path_buf()).size(side).flex_none().rounded(side * 0.25).object_fit(gpui_kit::ObjectFit::Contain).into_any_element();
        }
        match self.icon {
            Some(ProjectIcon::Monogram(m)) => ProjectMonogram::new(m.text, m.color).size(size).into_any_element(),
            Some(ProjectIcon::Lucide(lucide)) => {
                let name = IconName::ALL.iter().copied().find(|i| i.file() == lucide.name).unwrap_or(IconName::FolderCode);
                let ink = project_icon_ink(lucide.color, cx.app_theme().appearance);
                Icon::new(name).size(IconSize::Custom(size * 0.875)).color(ink).into_any_element()
            }
            Some(ProjectIcon::Emoji(emoji)) => div()
                .flex()
                .flex_none()
                .size(size * 0.875)
                .items_center()
                .justify_center()
                .text_size(size * 0.7)
                .line_height(size * 0.875)
                .child(emoji.emoji)
                .into_any_element(),
            None | Some(ProjectIcon::Unknown(_)) if self.name.trim().is_empty() => {
                Icon::new(IconName::FolderCode).size(IconSize::Custom(size * 0.875)).color(cx.app_theme().colors.icon_muted).into_any_element()
            }
            None | Some(ProjectIcon::Unknown(_)) => {
                let identity = ProjectIdentity::from_name(&self.name);
                ProjectMonogram::new(identity.monogram, identity.color).size(size).into_any_element()
            }
        }
    }
}
