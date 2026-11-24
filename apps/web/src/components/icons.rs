//! Icons. Lucide 0.564.0 glyphs at the app's global stroke width of 1.75
//! (`index.css:1761-1765`), and the provider and brand marks from
//! `Icons.tsx`, all embedded SVGs named by enum rather than by path.

use std::time::Duration;

use gpui_kit::{
    App, Hsla, ImageSource, IntoElement, Pixels, RenderOnce, SharedString, Styled, TimedTransformation,
    Transformation, Window, img, prelude::*, px, svg,
};

use crate::theme_palette::Appearance;

macro_rules! lucide_icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        /// A Lucide icon the app uses. Drawn as a mask tinted with one colour.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum IconName {
            $($variant,)*
        }

        impl IconName {
            /// Every icon, for the gallery.
            pub const ALL: &[IconName] = &[$(IconName::$variant,)*];

            /// The Lucide file name, without extension.
            pub const fn file(self) -> &'static str {
                match self { $(IconName::$variant => $file,)* }
            }

            /// The embedded asset path.
            pub const fn path(self) -> &'static str {
                match self { $(IconName::$variant => concat!("icons/lucide/", $file, ".svg"),)* }
            }
        }
    };
}

macro_rules! brand_icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        /// A provider, editor or platform mark from `Icons.tsx`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum BrandIcon {
            $($variant,)*
        }

        impl BrandIcon {
            /// Every mark, for the gallery.
            pub const ALL: &[BrandIcon] = &[$(BrandIcon::$variant,)*];

            /// The embedded asset path.
            pub const fn path(self) -> &'static str {
                match self { $(BrandIcon::$variant => concat!("icons/brand/", $file, ".svg"),)* }
            }
        }
    };
}

lucide_icons! {
    Activity => "activity",
    AlarmClock => "alarm-clock",
    AlarmClockOff => "alarm-clock-off",
    Archive => "archive",
    ArchiveX => "archive-x",
    ArrowDown => "arrow-down",
    ArrowDownUp => "arrow-down-up",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    ArrowRightLeft => "arrow-right-left",
    ArrowUp => "arrow-up",
    ArrowUpLeft => "arrow-up-left",
    ArrowUpRight => "arrow-up-right",
    AtSign => "at-sign",
    AudioLines => "audio-lines",
    Battery => "battery",
    Bell => "bell",
    Blocks => "blocks",
    BookOpen => "book-open",
    Bookmark => "bookmark",
    Bot => "bot",
    Box => "box",
    Brain => "brain",
    Bug => "bug",
    Building => "building",
    Cable => "cable",
    Calendar => "calendar",
    CalendarArrowDown => "calendar-arrow-down",
    CalendarArrowUp => "calendar-arrow-up",
    CalendarClock => "calendar-clock",
    Camera => "camera",
    ChartNoAxesColumn => "chart-no-axes-column",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ChevronUp => "chevron-up",
    ChevronsDownUp => "chevrons-down-up",
    ChevronsLeftRightEllipsis => "chevrons-left-right-ellipsis",
    ChevronsUpDown => "chevrons-up-down",
    Circle => "circle",
    CircleAlert => "circle-alert",
    CircleArrowUp => "circle-arrow-up",
    CircleCheck => "circle-check",
    CircleDashed => "circle-dashed",
    CircleDot => "circle-dot",
    CircleDotDashed => "circle-dot-dashed",
    CircleQuestionMark => "circle-question-mark",
    CircleSlash => "circle-slash",
    CircleStop => "circle-stop",
    CircleX => "circle-x",
    ClipboardCopy => "clipboard-copy",
    Clock => "clock",
    Clock3 => "clock-3",
    Cloud => "cloud",
    CloudDownload => "cloud-download",
    CloudUpload => "cloud-upload",
    Code => "code",
    CodeXml => "code-xml",
    Columns2 => "columns-2",
    Copy => "copy",
    CornerDownLeft => "corner-down-left",
    CornerLeftUp => "corner-left-up",
    CornerUpRight => "corner-up-right",
    Cpu => "cpu",
    Database => "database",
    Download => "download",
    Earth => "earth",
    Ellipsis => "ellipsis",
    EllipsisVertical => "ellipsis-vertical",
    Eraser => "eraser",
    ExternalLink => "external-link",
    Eye => "eye",
    EyeOff => "eye-off",
    File => "file",
    FileBraces => "file-braces",
    FileCodeCorner => "file-code-corner",
    FileDiff => "file-diff",
    FileSearch => "file-search",
    FileSpreadsheet => "file-spreadsheet",
    FileText => "file-text",
    Files => "files",
    Film => "film",
    FlaskConical => "flask-conical",
    FlaskConicalOff => "flask-conical-off",
    Folder => "folder",
    FolderClosed => "folder-closed",
    FolderCode => "folder-code",
    FolderGit => "folder-git",
    FolderGit2 => "folder-git-2",
    FolderOpen => "folder-open",
    FolderPlus => "folder-plus",
    FolderTree => "folder-tree",
    Gauge => "gauge",
    GitBranch => "git-branch",
    GitBranchPlus => "git-branch-plus",
    GitCommitHorizontal => "git-commit-horizontal",
    GitFork => "git-fork",
    GitMerge => "git-merge",
    GitPullRequest => "git-pull-request",
    GitPullRequestArrow => "git-pull-request-arrow",
    GitPullRequestClosed => "git-pull-request-closed",
    GitPullRequestDraft => "git-pull-request-draft",
    Globe => "globe",
    GripVertical => "grip-vertical",
    Hammer => "hammer",
    HardDrive => "hard-drive",
    Hash => "hash",
    History => "history",
    House => "house",
    Image => "image",
    Inbox => "inbox",
    Info => "info",
    KeyRound => "key-round",
    Keyboard => "keyboard",
    Languages => "languages",
    Laptop => "laptop",
    Layers => "layers",
    Lightbulb => "lightbulb",
    Link => "link",
    Link2 => "link-2",
    Link2Off => "link-2-off",
    ListChecks => "list-checks",
    ListFilter => "list-filter",
    ListPlus => "list-plus",
    ListTodo => "list-todo",
    ListTree => "list-tree",
    LoaderCircle => "loader-circle",
    Lock => "lock",
    LockOpen => "lock-open",
    LogIn => "log-in",
    Mail => "mail",
    MailOpen => "mail-open",
    Maximize => "maximize",
    Maximize2 => "maximize-2",
    MemoryStick => "memory-stick",
    MessageCircle => "message-circle",
    MessageCircleQuestionMark => "message-circle-question-mark",
    MessageSquare => "message-square",
    MessageSquareDashed => "message-square-dashed",
    MessageSquareOff => "message-square-off",
    MessageSquarePlus => "message-square-plus",
    MessageSquareText => "message-square-text",
    MessageSquareWarning => "message-square-warning",
    MessagesSquare => "messages-square",
    Mic => "mic",
    MicOff => "mic-off",
    Minimize2 => "minimize-2",
    Minus => "minus",
    Monitor => "monitor",
    Moon => "moon",
    MousePointer2 => "mouse-pointer-2",
    MousePointerClick => "mouse-pointer-click",
    OctagonAlert => "octagon-alert",
    OctagonX => "octagon-x",
    Package => "package",
    PackagePlus => "package-plus",
    Paintbrush => "paintbrush",
    Palette => "palette",
    PanelBottom => "panel-bottom",
    PanelBottomClose => "panel-bottom-close",
    PanelLeft => "panel-left",
    PanelLeftClose => "panel-left-close",
    PanelRight => "panel-right",
    PanelRightClose => "panel-right-close",
    PanelsTopLeft => "panels-top-left",
    Paperclip => "paperclip",
    Pause => "pause",
    PenLine => "pen-line",
    Pencil => "pencil",
    PictureInPicture2 => "picture-in-picture-2",
    Pilcrow => "pilcrow",
    Pin => "pin",
    PinOff => "pin-off",
    Play => "play",
    PlayFilled => "play-filled",
    Plug => "plug",
    Plug2 => "plug-2",
    Plus => "plus",
    Power => "power",
    Presentation => "presentation",
    QrCode => "qr-code",
    Quote => "quote",
    RadioTower => "radio-tower",
    Redo2 => "redo-2",
    RefreshCw => "refresh-cw",
    Reply => "reply",
    RotateCcw => "rotate-ccw",
    RotateCw => "rotate-cw",
    Route => "route",
    Rows3 => "rows-3",
    Scale => "scale",
    Scan => "scan",
    ScrollText => "scroll-text",
    Search => "search",
    Send => "send",
    Server => "server",
    Settings => "settings",
    Settings2 => "settings-2",
    Shield => "shield",
    ShieldAlert => "shield-alert",
    ShieldCheck => "shield-check",
    ShieldOff => "shield-off",
    ShieldQuestionMark => "shield-question-mark",
    Slash => "slash",
    SlidersHorizontal => "sliders-horizontal",
    Smartphone => "smartphone",
    SmilePlus => "smile-plus",
    Sparkles => "sparkles",
    Square => "square",
    SquareArrowOutUpRight => "square-arrow-out-up-right",
    SquareCheck => "square-check",
    SquareMenu => "square-menu",
    SquarePen => "square-pen",
    SquareSplitHorizontal => "square-split-horizontal",
    SquareSplitVertical => "square-split-vertical",
    SquareTerminal => "square-terminal",
    Star => "star",
    StarFilled => "star-filled",
    Sun => "sun",
    Table2 => "table-2",
    Tag => "tag",
    Target => "target",
    Terminal => "terminal",
    TextAlignStart => "text-align-start",
    TextSearch => "text-search",
    TextWrap => "text-wrap",
    Ticket => "ticket",
    Timer => "timer",
    Trash => "trash",
    Trash2 => "trash-2",
    TrendingDown => "trending-down",
    TrendingUp => "trending-up",
    TriangleAlert => "triangle-alert",
    Type => "type",
    Undo2 => "undo-2",
    Unlink => "unlink",
    Unlink2 => "unlink-2",
    Unplug => "unplug",
    Upload => "upload",
    User => "user",
    UserCheck => "user-check",
    UserLock => "user-lock",
    UserPlus => "user-plus",
    UserRound => "user-round",
    UserRoundX => "user-round-x",
    Users => "users",
    Volume2 => "volume-2",
    VolumeOff => "volume-off",
    WifiOff => "wifi-off",
    Workflow => "workflow",
    Wrench => "wrench",
    X => "x",
    Zap => "zap",
    ZapFilled => "zap-filled",
}

brand_icons! {
    AcpRegistry => "acp-registry",
    Android => "android",
    Anthropic => "anthropic",
    Antigravity => "antigravity",
    Apple => "apple",
    ClaudeAi => "claude-ai",
    Cursor => "cursor",
    FileExplorer => "file-explorer",
    Finder => "finder",
    Gemini => "gemini",
    Git => "git",
    Grok => "grok",
    Jujutsu => "jujutsu",
    Kiro => "kiro",
    Linear => "linear",
    Linux => "linux",
    MacAccessibility => "mac-accessibility",
    MacScreenRecording => "mac-screen-recording",
    Meta => "meta",
    Ollama => "ollama",
    OnePassword => "one-password",
    OpenAi => "open-ai",
    OpenRouter => "open-router",
    Trae => "trae",
    Ultrafast => "ultrafast",
    VisualStudioCode => "visual-studio-code",
    VisualStudioCodeInsiders => "visual-studio-code-insiders",
    VsCodium => "vs-codium",
    Zed => "zed",
}

/// How a brand mark is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BrandPaint {
    /// One colour: the caller's colour (`currentColor` in the source).
    Current,
    /// One fixed colour per appearance (`fill-[#..] dark:fill-[#..]`).
    Fixed { light: u32, dark: u32 },
    /// The SVG's own colours.
    FullColor,
}

impl BrandIcon {
    /// How `Icons.tsx` colours this mark.
    pub const fn paint(self) -> BrandPaint {
        use BrandIcon::*;
        match self {
            Android | Anthropic | Apple | Linear | Linux | Ollama | OnePassword | OpenAi
            | OpenRouter | Trae | Ultrafast | Zed => BrandPaint::Current,
            AcpRegistry => BrandPaint::Fixed { light: 0x000000, dark: 0xffffff },
            Cursor => BrandPaint::Fixed { light: 0x26251e, dark: 0xedecec },
            Grok => BrandPaint::Fixed { light: 0x0f0f0f, dark: 0xf5f5f5 },
            _ => BrandPaint::FullColor,
        }
    }
}

/// What an [`Icon`] draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconSource {
    Lucide(IconName),
    Brand(BrandIcon),
}

impl From<IconName> for IconSource {
    fn from(name: IconName) -> Self {
        Self::Lucide(name)
    }
}

impl From<BrandIcon> for IconSource {
    fn from(icon: BrandIcon) -> Self {
        Self::Brand(icon)
    }
}

/// Icon sizes the app uses (`size-3` to `size-5`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconSize {
    /// 12px.
    Xs,
    /// 14px.
    Sm,
    /// 16px.
    Md,
    /// 18px.
    Lg,
    /// 20px.
    Xl,
    Custom(Pixels),
}

impl IconSize {
    /// The size at a root font size: named sizes are Tailwind's rem sizes
    /// (`size-4` is 1rem), so they follow the Interface size setting.
    pub fn at_rem(self, rem: Pixels) -> Pixels {
        match self {
            Self::Custom(size) => size,
            named => named.pixels() * (rem / px(16.)),
        }
    }

    /// The size at the default 16px root.
    pub fn pixels(self) -> Pixels {
        match self {
            Self::Xs => px(12.),
            Self::Sm => px(14.),
            Self::Md => px(16.),
            Self::Lg => px(18.),
            Self::Xl => px(20.),
            Self::Custom(size) => size,
        }
    }
}

/// An icon element. Colour defaults to the surrounding text colour, like `currentColor`.
#[derive(IntoElement)]
pub struct Icon {
    source: IconSource,
    size: IconSize,
    color: Option<Hsla>,
    opacity: f32,
    stroke: Option<f32>,
    spin: Option<(Duration, f32)>,
    rotate_degrees: f32,
    group_color: Option<(SharedString, Hsla)>,
}

impl Icon {
    pub fn new(source: impl Into<IconSource>) -> Self {
        Self {
            source: source.into(),
            size: IconSize::Md,
            color: None,
            opacity: 1.,
            stroke: None,
            spin: None,
            rotate_degrees: 0.,
            group_color: None,
        }
    }

    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Multiplies the colour's alpha, as `opacity-80` on an svg does.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// An explicit Lucide `strokeWidth`, overriding the global 1.75.
    pub fn stroke(mut self, width: f32) -> Self {
        self.stroke = Some(width);
        self
    }

    /// A static rotation, as `-rotate-90` on a collapsed chevron.
    pub fn rotate(mut self, degrees: f32) -> Self {
        self.rotate_degrees = degrees;
        self
    }

    /// The colour while the named group is hovered, as `currentColor`
    /// follows a parent's `:hover` text colour.
    pub fn group_color(mut self, group: impl Into<SharedString>, color: Hsla) -> Self {
        self.group_color = Some((group.into(), color));
        self
    }

    /// Turns once per `period`, presented at most `steps_per_second` times a
    /// second. The scene animates the sprite; the view does not re-render.
    pub fn spin(mut self, period: Duration, steps_per_second: f32) -> Self {
        self.spin = Some((period, steps_per_second));
        self
    }
}

/// The asset path for a Lucide icon at a stroke width other than 1.75.
/// [`crate::Assets`] rewrites the stroke when it loads such a path.
fn stroked_path(name: IconName, width: f32) -> SharedString {
    format!("{}?stroke={width}", name.path()).into()
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let current = window.text_style().color;
        let size = self.size.at_rem(window.rem_size());
        let appearance = crate::ActiveAppTheme::app_theme(cx).appearance;
        let tint = |color: Hsla| Hsla { a: color.a * self.opacity, ..color };
        let (path, color): (SharedString, Option<Hsla>) = match self.source {
            IconSource::Lucide(name) => (
                match self.stroke {
                    Some(width) => stroked_path(name, width),
                    None => name.path().into(),
                },
                Some(self.color.unwrap_or(current)),
            ),
            IconSource::Brand(icon) => match icon.paint() {
                BrandPaint::Current => (icon.path().into(), Some(self.color.unwrap_or(current))),
                BrandPaint::Fixed { light, dark } => {
                    let rgb = if appearance == Appearance::Dark { dark } else { light };
                    (icon.path().into(), Some(gpui_kit::rgb(rgb).into()))
                }
                BrandPaint::FullColor => (icon.path().into(), None),
            },
        };
        let Some(color) = color else {
            return img(ImageSource::from(path))
                .size(size)
                .flex_none()
                .opacity(self.opacity)
                .into_any_element();
        };
        let pose = Transformation::rotate(gpui_kit::radians(self.rotate_degrees.to_radians()));
        let transformation = match self.spin {
            Some((period, steps)) => TimedTransformation::rotation(period, pose).with_max_fps(steps),
            None => pose.into(),
        };
        let mut icon = svg()
            .path(path)
            .size(size)
            .flex_none()
            .text_color(tint(color))
            .with_timed_transformation(transformation);
        if let Some((group, hover)) = self.group_color {
            let hover = tint(hover);
            icon = icon.group_hover(group, move |s| s.text_color(hover));
        }
        icon.into_any_element()
    }
}
