//! The composer's bundled icons: Lucide 0.564.0 at stroke 1.75 (send and
//! attach at 1.8, stop at 2.2, as the fork draws them) and the provider marks
//! the model picker needs.

use gpui_kit::{App, Hsla, IntoElement, ParentElement as _, Pixels, Styled as _, div, img, px, rgb, svg};

use crate::components::chat::composer_theme::ComposerColors;
use crate::model_selection::ProviderIcon;

/// A Lucide icon in `apps/web/assets/icons/composer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Paperclip,
    CornerDownLeft,
    CornerUpRight,
    ListPlus,
    Check,
    Play,
    Square,
    ArrowLeft,
    Mic,
    X,
    Star,
    StarFilled,
    Search,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    Zap,
    ZapFilled,
    Workflow,
    Lock,
    PenLine,
    ShieldCheck,
    ShieldOff,
    CircleAlert,
    File,
    Folder,
    MessagesSquare,
    GitBranch,
    Monitor,
    LoaderCircle,
    Minimize2,
    Ellipsis,
    GitPullRequest,
    Image,
    FileText,
    Terminal,
    FolderGit,
    Sparkles,
    Cpu,
    Slash,
}

impl Icon {
    /// The asset path.
    pub fn path(self) -> &'static str {
        match self {
            Self::Paperclip => "icons/composer/paperclip.svg",
            Self::CornerDownLeft => "icons/composer/corner-down-left.svg",
            Self::CornerUpRight => "icons/composer/corner-up-right.svg",
            Self::ListPlus => "icons/composer/list-plus.svg",
            Self::Check => "icons/composer/check.svg",
            Self::Play => "icons/composer/play-filled.svg",
            Self::Square => "icons/composer/square.svg",
            Self::ArrowLeft => "icons/composer/arrow-left.svg",
            Self::Mic => "icons/composer/mic.svg",
            Self::X => "icons/composer/x.svg",
            Self::Star => "icons/composer/star.svg",
            Self::StarFilled => "icons/composer/star-filled.svg",
            Self::Search => "icons/composer/search.svg",
            Self::ChevronRight => "icons/composer/chevron-right.svg",
            Self::ChevronUp => "icons/composer/chevron-up.svg",
            Self::ChevronDown => "icons/composer/chevron-down.svg",
            Self::Zap => "icons/composer/zap.svg",
            Self::ZapFilled => "icons/composer/zap-filled.svg",
            Self::Workflow => "icons/composer/workflow.svg",
            Self::Lock => "icons/composer/lock.svg",
            Self::PenLine => "icons/composer/pen-line.svg",
            Self::ShieldCheck => "icons/composer/shield-check.svg",
            Self::ShieldOff => "icons/composer/shield-off.svg",
            Self::CircleAlert => "icons/composer/circle-alert.svg",
            Self::File => "icons/composer/file.svg",
            Self::Folder => "icons/composer/folder.svg",
            Self::MessagesSquare => "icons/composer/messages-square.svg",
            Self::GitBranch => "icons/composer/git-branch.svg",
            Self::Monitor => "icons/composer/monitor.svg",
            Self::LoaderCircle => "icons/composer/loader-circle.svg",
            Self::Minimize2 => "icons/composer/minimize-2.svg",
            Self::Ellipsis => "icons/composer/ellipsis.svg",
            Self::GitPullRequest => "icons/composer/git-pull-request.svg",
            Self::Image => "icons/composer/image.svg",
            Self::FileText => "icons/composer/file-text.svg",
            Self::Terminal => "icons/composer/terminal.svg",
            Self::FolderGit => "icons/composer/folder-git-2.svg",
            Self::Sparkles => "icons/composer/sparkles.svg",
            Self::Cpu => "icons/composer/cpu.svg",
            Self::Slash => "icons/composer/slash.svg",
        }
    }
}

/// An icon at a size and colour.
pub fn icon(icon: Icon, size: Pixels, color: Hsla) -> impl IntoElement {
    svg().path(icon.path()).size(size).flex_none().text_color(color)
}

/// A provider or model mark. Monochrome marks take the theme's foreground;
/// Claude keeps its orange, Gemini and Meta their own colours.
pub fn provider_icon(mark: ProviderIcon, size: Pixels, initials: &str, colors: &ComposerColors, _cx: &App) -> gpui_kit::AnyElement {
    let dark = colors.is_dark();
    let mono = |path: &'static str, color: Hsla| svg().path(path).size(size).flex_none().text_color(color).into_any_element();
    match mark {
        ProviderIcon::OpenAi => mono("icons/composer/brand/openai.svg", if dark { rgb(0xffffff).into() } else { rgb(0x000000).into() }),
        ProviderIcon::Claude => mono("icons/composer/brand/claude.svg", rgb(0xd97757).into()),
        ProviderIcon::Cursor => mono("icons/composer/brand/cursor.svg", if dark { rgb(0xedecec).into() } else { rgb(0x26251e).into() }),
        ProviderIcon::Grok => mono("icons/composer/brand/grok.svg", if dark { rgb(0xf5f5f5).into() } else { rgb(0x0f0f0f).into() }),
        ProviderIcon::OpenCode => img(if dark { "icons/composer/brand/opencode-dark.svg" } else { "icons/composer/brand/opencode-light.svg" })
            .size(size)
            .flex_none()
            .into_any_element(),
        ProviderIcon::Gemini => img("icons/composer/brand/gemini.svg").size(size).flex_none().into_any_element(),
        ProviderIcon::Meta => img("icons/composer/brand/meta.svg").size(size).flex_none().into_any_element(),
        ProviderIcon::Initials => div()
            .size(size)
            .flex_none()
            .rounded(px(4.))
            .bg(colors.input)
            .flex()
            .items_center()
            .justify_center()
            .text_size(size * 0.5)
            .text_color(colors.foreground)
            .child(initials.chars().take(2).collect::<String>())
            .into_any_element(),
    }
}
