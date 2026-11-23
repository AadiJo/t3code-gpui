//! The Lucide 0.564.0 icons the shell bundles under `assets/icons/shell`
//! (stroke 1.75; context-menu icons under `menu/` keep stroke 2).

use gpui_kit::{Hsla, Pixels, Styled as _, Svg, svg};

/// A bundled shell icon. Draw it with [`icon`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellIcon {
    PanelLeft,
    PanelLeftClose,
    SquareMenu,
    PanelBottom,
    PanelBottomClose,
    PanelRight,
    PanelRightClose,
    Maximize2,
    Minimize2,
    Search,
    X,
    ListFilter,
    FolderPlus,
    SquarePen,
    Folder,
    FolderCode,
    Settings,
    AlarmClock,
    AlarmClockOff,
    Check,
    CircleAlert,
    CircleCheck,
    CircleDashed,
    CircleDotDashed,
    Clock,
    GitBranch,
    GitFork,
    FolderGit2,
    MessageCircleQuestion,
    Pin,
    PinOff,
    ShieldQuestion,
    ShieldAlert,
    CircleHelp,
    Terminal,
    Undo2,
    UserRound,
    ChevronDown,
    ChevronRight,
    Plus,
    ChartNoAxesColumn,
    ArrowLeft,
    Bell,
    GitPullRequest,
    CircleDot,
    Monitor,
    Laptop,
}

impl ShellIcon {
    /// Asset path of the SVG.
    pub fn path(self) -> &'static str {
        use ShellIcon::*;
        match self {
            PanelLeft => "icons/shell/panel-left.svg",
            PanelLeftClose => "icons/shell/panel-left-close.svg",
            SquareMenu => "icons/shell/square-menu.svg",
            PanelBottom => "icons/shell/panel-bottom.svg",
            PanelBottomClose => "icons/shell/panel-bottom-close.svg",
            PanelRight => "icons/shell/panel-right.svg",
            PanelRightClose => "icons/shell/panel-right-close.svg",
            Maximize2 => "icons/shell/maximize-2.svg",
            Minimize2 => "icons/shell/minimize-2.svg",
            Search => "icons/shell/search.svg",
            X => "icons/shell/x.svg",
            ListFilter => "icons/shell/list-filter.svg",
            FolderPlus => "icons/shell/folder-plus.svg",
            SquarePen => "icons/shell/square-pen.svg",
            Folder => "icons/shell/folder.svg",
            FolderCode => "icons/shell/folder-code.svg",
            Settings => "icons/shell/settings.svg",
            AlarmClock => "icons/shell/alarm-clock.svg",
            AlarmClockOff => "icons/shell/alarm-clock-off.svg",
            Check => "icons/shell/check.svg",
            CircleAlert => "icons/shell/circle-alert.svg",
            CircleCheck => "icons/shell/circle-check.svg",
            CircleDashed => "icons/shell/circle-dashed.svg",
            CircleDotDashed => "icons/shell/circle-dot-dashed.svg",
            Clock => "icons/shell/clock.svg",
            GitBranch => "icons/shell/git-branch.svg",
            GitFork => "icons/shell/git-fork.svg",
            FolderGit2 => "icons/shell/folder-git-2.svg",
            MessageCircleQuestion => "icons/shell/message-circle-question-mark.svg",
            Pin => "icons/shell/pin.svg",
            PinOff => "icons/shell/pin-off.svg",
            ShieldQuestion => "icons/shell/shield-question-mark.svg",
            ShieldAlert => "icons/shell/shield-alert.svg",
            CircleHelp => "icons/shell/circle-question-mark.svg",
            Terminal => "icons/shell/terminal.svg",
            Undo2 => "icons/shell/undo-2.svg",
            UserRound => "icons/shell/user-round.svg",
            ChevronDown => "icons/shell/chevron-down.svg",
            ChevronRight => "icons/shell/chevron-right.svg",
            Plus => "icons/shell/plus.svg",
            ChartNoAxesColumn => "icons/shell/chart-no-axes-column.svg",
            ArrowLeft => "icons/shell/arrow-left.svg",
            Bell => "icons/shell/bell.svg",
            GitPullRequest => "icons/shell/git-pull-request.svg",
            CircleDot => "icons/shell/circle-dot.svg",
            Monitor => "icons/shell/monitor.svg",
            Laptop => "icons/shell/laptop.svg",
        }
    }
}

/// Context-menu icons, drawn at stroke 2 like `contextMenuFallback.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuIcon {
    Archive,
    Check,
    Timer,
    ChevronRight,
    CircleCheck,
    Clock,
    Pencil,
    Copy,
    Folder,
    GitBranch,
    Hash,
    MailOpen,
    MessageSquare,
    MessageSquarePlus,
    Pin,
    PinOff,
    RefreshCw,
    Settings,
    UserPlus,
    FolderPlus,
    FolderTree,
    TextSearch,
    Trash,
}

impl MenuIcon {
    pub fn path(self) -> &'static str {
        use MenuIcon::*;
        match self {
            Archive => "icons/shell/menu/archive.svg",
            Check => "icons/shell/menu/check.svg",
            Timer => "icons/shell/menu/timer.svg",
            ChevronRight => "icons/shell/menu/chevron-right.svg",
            CircleCheck => "icons/shell/menu/circle-check.svg",
            Clock => "icons/shell/menu/clock.svg",
            Pencil => "icons/shell/menu/pencil.svg",
            Copy => "icons/shell/menu/copy.svg",
            Folder => "icons/shell/menu/folder.svg",
            GitBranch => "icons/shell/menu/git-branch.svg",
            Hash => "icons/shell/menu/hash.svg",
            MailOpen => "icons/shell/menu/mail-open.svg",
            MessageSquare => "icons/shell/menu/message-square.svg",
            MessageSquarePlus => "icons/shell/menu/message-square-plus.svg",
            Pin => "icons/shell/menu/pin.svg",
            PinOff => "icons/shell/menu/pin-off.svg",
            RefreshCw => "icons/shell/menu/refresh-cw.svg",
            Settings => "icons/shell/menu/settings.svg",
            UserPlus => "icons/shell/menu/user-plus.svg",
            FolderPlus => "icons/shell/menu/folder-plus.svg",
            FolderTree => "icons/shell/menu/folder-tree.svg",
            TextSearch => "icons/shell/menu/text-search.svg",
            Trash => "icons/shell/menu/trash.svg",
        }
    }
}

/// An icon at `size` px, tinted `color`.
pub fn icon(icon: ShellIcon, size: Pixels, color: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .size(size)
        .flex_none()
        .text_color(color)
}

/// A context-menu icon at 16px.
pub fn menu_icon(icon: MenuIcon, color: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .size(gpui_kit::px(16.))
        .flex_none()
        .text_color(color)
}
