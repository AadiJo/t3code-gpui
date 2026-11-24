//! The device preference pages of `SettingsPanels.tsx`: General, Chat,
//! Diffs & pull requests, Labs and About. Each renders from the stores, so
//! it is a plain function the settings route calls in render; the route
//! re-renders when the stores change.

use std::rc::Rc;

use gpui_kit::{AnyElement, App, IntoElement, SharedString, Window, div, prelude::*};
use t3code_contracts::{
    ClientServerSettingsPatch, ClientSettingsSchemaConfirmQuit, ClientSettingsSchemaFollowUpBehavior,
    ClientSettingsSchemaSendShortcut, DiffColorScheme, DiffLayout, NotificationMode, QuitConfirmationMode,
    ScreenReaderAnnouncements, SidebarProjectGroupingMode, SidebarProjectSortOrder, TimestampFormat, ToolCallVisibility,
};

use super::settings_controls::{
    SelectWidth, SettingsScope, client_row, client_select, client_switch, reset_button, select_control, server_switch, sized_select,
};
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::settings_search::setting_title;
use crate::client_persistence_storage::LocalStorage;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::switch::Switch;
use crate::hooks::use_settings::{ClientSettingsStore, DesktopSettingsStore, UpdateChannel};
use crate::i18n::locales::{Locale, SYSTEM_LANGUAGE};
use crate::i18n::{t, t_rich};

/// A `<T>` block's text with its links drawn as plain text.
pub fn rich_plain(text: crate::i18n::RichText) -> SharedString {
    text.plain().into()
}

/// The modifier key's label, `⌘` on macOS and `Ctrl` elsewhere.
pub fn modifier_label() -> &'static str {
    if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" }
}

/// `NOTIFICATION_MODE_LABELS`.
fn notification_mode_options() -> Vec<(NotificationMode, SharedString)> {
    vec![
        (NotificationMode::Off, t!("Off", context = "notification setting, no notifications or sound")),
        (NotificationMode::Notifications, t!("Notifications only")),
        (NotificationMode::Sound, t!("Sound only")),
        (NotificationMode::NotificationsAndSound, t!("Notifications with sound")),
    ]
}

const LAST_ENABLED_PROJECT_GROUPING_MODE_KEY: &str = "t3code:last-enabled-project-grouping-mode";

/// `GeneralSettingsPanel`.
pub fn general_panel(cx: &App) -> AnyElement {
    let settings = ClientSettingsStore::get(cx).clone();
    let notifications = SettingsSection::new(t!("Notifications"))
        .child(client_select(
            "thread-notifications",
            t!("System alerts when a thread finishes, fails, or needs input or approval. Applies to this device while T3 Code is open."),
            SelectWidth::Rem(14.),
            notification_mode_options(),
            |s| &s.notification_mode,
            |s, v| s.notification_mode = v,
            cx,
        ))
        .child(client_switch(
            "in-app-notifications",
            t!("Show a toast when another thread finishes, fails, or needs input or approval while this app has focus."),
            |s| &s.in_app_notifications_enabled,
            |s, v| s.in_app_notifications_enabled = v,
            cx,
        ))
        .child(client_switch(
            "notification-inbox",
            t!("Show the notification bell in the sidebar footer. Notifications are still recorded and dismissed without it."),
            |s| &s.notification_inbox_enabled,
            |s, v| s.notification_inbox_enabled = v,
            cx,
        ))
        .child(client_switch(
            "app-icon-badge",
            t!("Show the number of unread notifications on the T3 Code icon in the Dock or taskbar."),
            |s| &s.app_icon_badge_enabled,
            |s, v| s.app_icon_badge_enabled = v,
            cx,
        ));

    let grouping_enabled = settings.sidebar_project_grouping_mode != SidebarProjectGroupingMode::Separate;
    let grouping_switch = Switch::new("switch-project-grouping", grouping_enabled).on_change(|checked, _, cx| {
        let current = ClientSettingsStore::get(cx).sidebar_project_grouping_mode.clone();
        if !checked && current != SidebarProjectGroupingMode::Separate {
            let remembered = if current == SidebarProjectGroupingMode::RepositoryPath { "repository_path" } else { "repository" };
            LocalStorage::set(cx, LAST_ENABLED_PROJECT_GROUPING_MODE_KEY, Some(&remembered.to_owned()));
        }
        let last = LocalStorage::get::<String>(cx, LAST_ENABLED_PROJECT_GROUPING_MODE_KEY);
        let next = match (checked, last.as_deref()) {
            (false, _) => SidebarProjectGroupingMode::Separate,
            (true, Some("repository_path")) => SidebarProjectGroupingMode::RepositoryPath,
            (true, _) => SidebarProjectGroupingMode::Repository,
        };
        ClientSettingsStore::update(cx, |s| s.sidebar_project_grouping_mode = next);
    });
    let sidebar = SettingsSection::new(t!("Sidebar"))
        .child(client_row(
            "project-grouping",
            t!("Combine matching repositories across environments."),
            |s| &s.sidebar_project_grouping_mode,
            |s, v| s.sidebar_project_grouping_mode = v,
            grouping_switch,
            cx,
        ))
        .child(client_select(
            "project-order",
            t!("Order of projects in the sidebar project picker and command palette."),
            SelectWidth::Rem(11.),
            vec![
                (SidebarProjectSortOrder::UpdatedAt, t!("Last user message")),
                (SidebarProjectSortOrder::CreatedAt, t!("Created at")),
                (SidebarProjectSortOrder::Manual, t!("Manual")),
            ],
            |s| &s.sidebar_project_sort_order,
            |s, v| s.sidebar_project_sort_order = v,
            cx,
        ))
        .child(client_switch(
            "working-shelf",
            t!("Fold working and monitoring threads into a Working section. They return to the top of the inbox when they need you. The boring sidebar in Labs has no Working section."),
            |s| &s.sidebar_working_shelf_enabled,
            |s, v| s.sidebar_working_shelf_enabled = v,
            cx,
        ));

    let quit_options = vec![
        (ClientSettingsSchemaConfirmQuit::QuitConfirmationMode(QuitConfirmationMode::Direct), t!("Direct")),
        (ClientSettingsSchemaConfirmQuit::QuitConfirmationMode(QuitConfirmationMode::Hold), t!("Hold")),
        (ClientSettingsSchemaConfirmQuit::QuitConfirmationMode(QuitConfirmationMode::DoubleClick), t!("Double press")),
    ];
    let confirmations = SettingsSection::new(t!("Confirmations"))
        .child(client_switch(
            "unpin-confirmation",
            t!("Ask before unpinning a thread from the pinned section."),
            |s| &s.confirm_thread_unpin,
            |s, v| s.confirm_thread_unpin = v,
            cx,
        ))
        .child(client_switch(
            "archive-confirmation",
            t!("Require a second click on the inline archive action before a thread is archived."),
            |s| &s.confirm_thread_archive,
            |s, v| s.confirm_thread_archive = v,
            cx,
        ))
        .child(client_switch(
            "delete-confirmation",
            t!("Ask before deleting a thread and its chat history."),
            |s| &s.confirm_thread_delete,
            |s, v| s.confirm_thread_delete = v,
            cx,
        ))
        .child(client_switch(
            "terminal-close-confirmation",
            t!("Ask before closing terminals and stopping their running processes."),
            |s| &s.confirm_terminal_close,
            |s, v| s.confirm_terminal_close = v,
            cx,
        ))
        .child(client_select(
            "quit-confirmation",
            t!("Hold mode also quits on two quick presses."),
            SelectWidth::Rem(10.),
            quit_options,
            |s| &s.confirm_quit,
            |s, v| s.confirm_quit = v,
            cx,
        ));

    let mut languages: Vec<(String, SharedString)> = vec![(SYSTEM_LANGUAGE.to_owned(), t!("System default"))];
    languages.extend(Locale::ALL.iter().map(|locale| (locale.tag().to_owned(), SharedString::new_static(locale.native_name()))));
    let display = SettingsSection::new(t!("Display"))
        .child(client_select(
            "language",
            t!("System default follows your browser or OS language."),
            SelectWidth::Rem(10.),
            languages,
            |s| &s.language,
            |s, v| s.language = v,
            cx,
        ))
        .child(client_select(
            "time-format",
            t!("System default follows your browser or OS clock preference."),
            SelectWidth::Rem(10.),
            vec![
                (TimestampFormat::Locale, t!("System default")),
                (TimestampFormat::N12Hour, t!("12-hour")),
                (TimestampFormat::N24Hour, t!("24-hour")),
            ],
            |s| &s.timestamp_format,
            |s, v| s.timestamp_format = v,
            cx,
        ));

    page_container(cx).child(notifications).child(sidebar).child(confirmations).child(display).into_any_element()
}

/// `ChatSettingsPanel`.
pub fn chat_panel(advanced_open: bool, toggle_advanced: Rc<dyn Fn(&mut Window, &mut App)>, cx: &App) -> AnyElement {
    let settings = ClientSettingsStore::get(cx).clone();
    let modifier = modifier_label();
    let composer = SettingsSection::new(t!("Composer"))
        .child(client_switch(
            "composer-rich-text",
            t!("Show formatted Markdown as you type."),
            |s| &s.composer_rich_text_enabled,
            |s, v| s.composer_rich_text_enabled = v,
            cx,
        ))
        .child(client_select(
            "send-shortcut",
            t!("Choose when Enter sends a prompt or inserts a new line"),
            SelectWidth::Fit,
            vec![
                (ClientSettingsSchemaSendShortcut::Enter, SharedString::new_static("Enter")),
                (
                    ClientSettingsSchemaSendShortcut::ModEnterMultiline,
                    t!("{modifierLabel} + Enter for multiline prompts", modifierLabel = modifier),
                ),
                (ClientSettingsSchemaSendShortcut::ModEnter, t!("{modifierLabel} + Enter always", modifierLabel = modifier)),
            ],
            |s| &s.send_shortcut,
            |s, v| s.send_shortcut = v,
            cx,
        ))
        .child(advanced_settings("chat-advanced", advanced_open, toggle_advanced, vec![client_switch(
            "skills-in-slash-menu",
            t!("Also include skills in the / command menu. Skills always appear when you type $."),
            |s| &s.show_skills_in_slash_menu,
            |s, v| s.show_skills_in_slash_menu = v,
            cx,
        )
        .into_any_element()]));
    let follow_up_description = match settings.send_shortcut {
        ClientSettingsSchemaSendShortcut::ModEnterMultiline => t!(
            "Queue follow-ups while the agent runs or steer the current run. Press {modifier} + Enter for single-line prompts or {modifier} + Shift + Enter for multiline prompts to do the opposite for one message.",
            modifier = modifier
        ),
        ClientSettingsSchemaSendShortcut::ModEnter => t!(
            "Queue follow-ups while the agent runs or steer the current run. Press {modifier} + Shift + Enter to do the opposite for one message.",
            modifier = modifier
        ),
        _ => t!(
            "Queue follow-ups while the agent runs or steer the current run. Press {modifier} + Enter to do the opposite for one message.",
            modifier = modifier
        ),
    };
    let follow_ups = SettingsSection::new(t!("Follow-ups")).child(client_select(
        "follow-up-behavior",
        follow_up_description,
        SelectWidth::Fit,
        vec![
            (ClientSettingsSchemaFollowUpBehavior::Queue, t!("Queue")),
            (ClientSettingsSchemaFollowUpBehavior::Steer, t!("Steer")),
        ],
        |s| &s.follow_up_behavior,
        |s, v| s.follow_up_behavior = v,
        cx,
    ));
    let activity = SettingsSection::new(t!("Activity")).child(client_select(
        "tool-call-visibility",
        t!("Hide the commands, edits, and searches agents run. Hide successful keeps failures. Zen shows only final answers. Approvals, questions, and errors always show."),
        SelectWidth::Rem(10.),
        vec![
            (ToolCallVisibility::ShowAll, t!("Show all")),
            (ToolCallVisibility::HideSuccessful, t!("Hide successful")),
            (ToolCallVisibility::HideAll, t!("Hide all")),
            (ToolCallVisibility::Zen, t!("Zen", context = "tool call visibility")),
        ],
        |s| &s.tool_call_visibility,
        |s, v| s.tool_call_visibility = v,
        cx,
    ));
    let screen_reader = SettingsSection::new(t!("Screen reader"))
        .child(client_select(
            "screen-reader-announcements",
            t!("What a screen reader hears from the open thread: a turn starting, needing you, and ending. Status and messages adds what the agent writes before its last message. Everything adds each step."),
            SelectWidth::Rem(12.),
            vec![
                (ScreenReaderAnnouncements::Status, t!("Status only")),
                (ScreenReaderAnnouncements::Messages, t!("Status and messages")),
                (ScreenReaderAnnouncements::Everything, t!("Everything")),
            ],
            |s| &s.screen_reader_announcements,
            |s, v| s.screen_reader_announcements = v,
            cx,
        ))
        .child(client_switch(
            "screen-reader-window-title",
            t!("Start the window title with the thread's status, like Needs approval, and its name, so screen readers read them first when you switch apps. Taskbars, shared screens, and time trackers see it too."),
            |s| &s.screen_reader_window_title,
            |s, v| s.screen_reader_window_title = v,
            cx,
        ));
    page_container(cx).child(composer).child(follow_ups).child(activity).child(screen_reader).into_any_element()
}

/// `AdvancedSettings`: a collapsed "Advanced" disclosure at the foot of a section.
pub fn advanced_settings(
    id: &'static str,
    open: bool,
    toggle: Rc<dyn Fn(&mut Window, &mut App)>,
    rows: Vec<AnyElement>,
) -> AnyElement {
    AdvancedDisclosure { id, open, toggle, rows }.into_any_element()
}

#[derive(IntoElement)]
struct AdvancedDisclosure {
    id: &'static str,
    open: bool,
    toggle: Rc<dyn Fn(&mut Window, &mut App)>,
    rows: Vec<AnyElement>,
}

impl gpui_kit::RenderOnce for AdvancedDisclosure {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        use crate::ActiveAppTheme as _;
        use crate::appearance_fonts::{TextSize, weight};
        use crate::components::icons::{Icon, IconName, IconSize};
        use crate::components::ui::TextStyleExt as _;
        let theme = cx.app_theme();
        let c = &theme.colors;
        let toggle = self.toggle.clone();
        let divider = crate::theme_color::with_alpha(c.border, 0.5);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(self.id)
                    .flex()
                    .items_center()
                    .gap(theme.sp(1.5))
                    .min_h(theme.sp(10.))
                    .px(theme.sp(4.))
                    .cursor_pointer()
                    .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                    .text_color(c.muted_foreground)
                    .hover(|s| s.text_color(c.foreground()))
                    .on_click(move |_, window, cx| toggle(window, cx))
                    .child(
                        Icon::new(if self.open { IconName::ChevronDown } else { IconName::ChevronRight })
                            .size(IconSize::Sm)
                            .color(c.muted_foreground),
                    )
                    .child(t!("Advanced")),
            )
            .when(self.open, |d| {
                d.child(div().border_t_1().border_color(divider).flex().flex_col().children(
                    self.rows.into_iter().enumerate().map(|(ix, row)| {
                        div().when(ix > 0, |d| d.border_t_1().border_color(divider)).child(row)
                    }),
                ))
            })
    }
}

/// `DiffsSettingsPanel`.
pub fn diffs_panel(cx: &App) -> AnyElement {
    let settings = ClientSettingsStore::get(cx).clone();
    let collapsed_select = select_control(
        "select-default-diff-file-state".into(),
        SelectWidth::Rem(10.),
        Rc::from([t!("Expanded"), t!("Collapsed")]),
        usize::from(settings.diff_files_collapsed),
        |index, _, cx| ClientSettingsStore::update(cx, |s| s.diff_files_collapsed = index == 1),
    );
    let diffs = SettingsSection::new(t!("Diffs"))
        .child(client_select(
            "diff-layout",
            t!("Show diffs stacked or side by side. The toggle in the diff toolbar changes this too."),
            SelectWidth::Rem(10.),
            vec![
                (DiffLayout::Stacked, t!("Stacked", context = "diff layout")),
                (DiffLayout::Split, t!("Split", context = "diff layout")),
            ],
            |s| &s.diff_layout,
            |s, v| s.diff_layout = v,
            cx,
        ))
        .child(client_switch(
            "hide-whitespace-changes",
            t!("Set whether the diff panel ignores whitespace-only edits by default."),
            |s| &s.diff_ignore_whitespace,
            |s, v| s.diff_ignore_whitespace = v,
            cx,
        ))
        .child(client_row(
            "default-diff-file-state",
            t!("Start with files expanded or collapsed when opening diffs or a pull request's Code tab."),
            |s| &s.diff_files_collapsed,
            |s, v| s.diff_files_collapsed = v,
            collapsed_select,
            cx,
        ))
        .child(diff_colors_row(&settings.diff_color_scheme, cx));
    let pull_requests = SettingsSection::new(t!("Pull requests"))
        .child(client_switch(
            "proactive-panels",
            t!("Open linked pull requests first. Otherwise, open Changes for edits to at least 3 files or 50 lines."),
            |s| &s.proactive_panels_enabled,
            |s, v| s.proactive_panels_enabled = v,
            cx,
        ))
        .child(client_switch(
            "pull-request-links-open-in-app",
            t!("Clicking a pull request link opens it in T3 Code. Turn off to open pull request links like any other link, on your git host."),
            |s| &s.pull_request_links_open_in_app,
            |s, v| s.pull_request_links_open_in_app = v,
            cx,
        ));
    page_container(cx).child(diffs).child(pull_requests).into_any_element()
}

/// `DiffColorsSettingsRow`: the trigger leads with the deletion and
/// addition dots (addition first for blue & orange) and names the scheme
/// without "(default)".
fn diff_colors_row(scheme: &DiffColorScheme, cx: &App) -> SettingsRow {
    use crate::ActiveAppTheme as _;
    let theme = cx.app_theme();
    let blue_orange = *scheme == DiffColorScheme::BlueOrange;
    let dark = theme.is_dark();
    let pick = |d: &str, l: &str| crate::syntax::hex(if dark { d } else { l }).unwrap_or_default();
    let (deletion, addition) = if blue_orange {
        (pick("#ff8904", "#f54900"), pick("#51a2ff", "#155dfc"))
    } else {
        (theme.colors.destructive(), theme.fixed.success)
    };
    let dot = |color| div().size(gpui_kit::px(8.)).rounded_full().bg(color);
    let dots = div()
        .flex()
        .flex_none()
        .gap(theme.sp(1.))
        .when(blue_orange, |d| d.flex_row_reverse())
        .child(dot(deletion))
        .child(dot(addition));
    let select = crate::components::ui::select::Select::new(
        "select-diff-color-scheme",
        Rc::from([t!("Red & green (default)"), t!("Blue & orange")]),
        usize::from(blue_orange),
        |index, _, cx| {
            let value = if index == 1 { DiffColorScheme::BlueOrange } else { DiffColorScheme::RedGreen };
            ClientSettingsStore::update(cx, |s| s.diff_color_scheme = value)
        },
    )
    .size(crate::components::ui::select::SelectSize::Sm)
    .value_label(if blue_orange { t!("Blue & orange") } else { t!("Red & green") })
    .leading(dots);
    client_row(
        "diff-color-scheme",
        t!("Choose colors for additions and deletions, including change counts."),
        |s| &s.diff_color_scheme,
        |s, v| s.diff_color_scheme = v,
        sized_select(SelectWidth::Rem(10.), select),
        cx,
    )
}

/// `LabsSettingsPanel`. The client rows are this device's; the server rows
/// follow the scope picker. Fast worktree checkout shows only when every
/// environment in scope reports the capability (macOS servers).
pub fn labs_panel(cx: &App) -> AnyElement {
    use crate::ActiveAppTheme as _;
    use super::settings_controls::{LinkTarget, linked_description};
    use super::settings_search::SettingsPath;
    let theme = cx.app_theme();
    let settings = ClientSettingsStore::get(cx).clone();
    let mut boring = client_switch(
        "boring-sidebar",
        t!("Group sidebar threads into project folders. Pinned threads stay on top, and Snoozed and Settled stay below the folders."),
        |s| &s.boring_sidebar_enabled,
        |s, v| s.boring_sidebar_enabled = v,
        cx,
    );
    if settings.boring_sidebar_enabled {
        // `mt-2 mb-2 border-l-2 border-border/60 ps-3` around a `px-0 py-2` row.
        boring = boring.child(
            div()
                .mt(theme.sp(2.))
                .mb(theme.sp(2.))
                .border_l_2()
                .border_color(crate::theme_color::with_alpha(theme.colors.border, 0.6))
                .pl(theme.sp(3.))
                .child(
                    div().mx(-theme.sp(4.)).my(-theme.sp(1.)).child(client_switch(
                        "boring-sidebar-multiline",
                        t!("Show folder threads on two lines: the title, then the branch, pull request, and agent."),
                        |s| &s.boring_sidebar_multiline_rows,
                        |s, v| s.boring_sidebar_multiline_rows = v,
                        cx,
                    )),
                ),
        );
    }
    let targets = SettingsScope::targets(cx);
    let fast_checkout_supported = !targets.is_empty()
        && targets.iter().all(|id| {
            crate::hooks::use_settings::ServerSettingsStore::config(cx, id)
                .is_some_and(|config| config.environment.capabilities.fast_worktree_checkout == Some(true))
        });
    let mut beta = SettingsSection::new(t!("Beta"))
        .child(boring)
        .child(
            server_switch(
                "guest-links",
                SharedString::default(),
                |s| s.guest_sharing_enabled,
                |v| ClientServerSettingsPatch { guest_sharing_enabled: Some(v), ..Default::default() },
                cx,
            )
            .description_element(linked_description(
                "guest-links-description",
                t_rich!(r#"Share a thread with a guest who watches it live in a browser and sends messages you approve. Needs T3 Connect: sign in and link this machine in{" "}<Link>Sharing & access</Link>. Turning it off stops every guest link."#),
                vec![LinkTarget::Page(SettingsPath::Access)],
            )),
        )
        .child(
            server_switch(
                "linked-environments-beta",
                SharedString::default(),
                |s| s.linked_environments_enabled,
                |v| ClientServerSettingsPatch { linked_environments_enabled: Some(v), ..Default::default() },
                cx,
            )
            .description_element(linked_description(
                "linked-environments-description",
                t_rich!(r#"Let agents here work on your other machines, and theirs here. Turn it on for each machine, then link them in{" "}<Link>Connections</Link>. While off, nothing new starts over a link; running tasks finish."#),
                vec![LinkTarget::Page(SettingsPath::Connections)],
            )),
        )
        .child(server_switch(
            "prompt-suggestions-beta",
            t!("Show a predicted next message in the empty composer after a Claude or Codex turn. Tab uses it. Each is an extra cached request on your plan. Needs Codex 0.165+. Claude threads update on restart."),
            |s| s.prompt_suggestions_enabled,
            |v| ClientServerSettingsPatch { prompt_suggestions_enabled: Some(v), ..Default::default() },
            cx,
        ))
        .child(
            server_switch(
                "simlock-hosts",
                SharedString::default(),
                |s| s.simlock_device_hosts_enabled,
                |v| ClientServerSettingsPatch { simlock_device_hosts_enabled: Some(v), ..Default::default() },
                cx,
            )
            .description_element(linked_description(
                "simlock-hosts-description",
                t_rich!(r#"Let the Device panel lease simulators and emulators from your own{" "}<a>Simlock</a>{" "}pool, added in{" "}<Link>Devices</Link>. An alpha that may change or break. T3 Code never installs Simlock."#),
                vec![LinkTarget::Url(SIMLOCK_URL), LinkTarget::Page(SettingsPath::Devices)],
            )),
        );
    if fast_checkout_supported {
        beta = beta.child(server_switch(
            "worktree-fast-checkout",
            t!("On a Mac, clone new worktrees from a checkout cached in the T3 home folder instead of writing every file. Uses Git’s normal checkout when it can’t. Turning it off deletes the cache."),
            |s| s.fast_worktree_checkout,
            |v| ClientServerSettingsPatch { fast_worktree_checkout: Some(v), ..Default::default() },
            cx,
        ));
    }
    page_container(cx).child(beta).into_any_element()
}

const SIMLOCK_URL: &str = "https://incubator.callstack.com/simlock/";
/// `AboutVersionTitle`'s copy button: the version in mono `text-2xs`,
/// muted until hovered, copying itself with a "Copy version" tooltip.
fn version_copy_button(cx: &App) -> AnyElement {
    use crate::ActiveAppTheme as _;
    use crate::components::ui::TextStyleExt as _;
    let theme = cx.app_theme();
    let c = &theme.colors;
    let version = env!("CARGO_PKG_VERSION");
    div()
        .id("about-version-copy")
        // `items-baseline` with the 14px title puts the 11px code 2.5px lower.
        .mt(gpui_kit::px(2.5))
        .cursor_pointer()
        .mono_text(gpui_kit::px(crate::appearance_fonts::TextSize::Xs2.size(theme.fonts.interface)), crate::appearance_fonts::weight::MEDIUM)
        .text_color(c.muted_foreground)
        .hover(|s| s.text_color(c.foreground()))
        .tooltip(crate::components::ui::tooltip::tooltip(t!("Copy version")))
        .on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(version.to_owned())))
        .child(version)
        .into_any_element()
}

const PRIVACY_POLICY_URL: &str = "https://t3.codes/privacy-policy";
const APP_STORE_URL: &str = "https://apps.apple.com/us/app/t3-code-remote-claude-more/id6787819824";
const PLAY_STORE_URL: &str = "https://play.google.com/store/apps/details?id=com.t3tools.t3code";

/// `AboutSettingsPanel`. The desktop self-updater is not built, so the
/// version row has no "Check for Updates" button; the update track is
/// saved for it. The Welcome page row needs a primary environment, which
/// this remote-only app does not have, so it is hidden as on the web.
pub fn about_panel(on_open_licenses: Rc<dyn Fn(&mut Window, &mut App)>, cx: &App) -> AnyElement {
    let channel = DesktopSettingsStore::get(cx).update_channel;
    let track = select_control(
        "select-update-track".into(),
        SelectWidth::Rem(10.),
        Rc::from([t!("Stable"), t!("Nightly")]),
        usize::from(channel == UpdateChannel::Nightly),
        |index, _, cx| {
            DesktopSettingsStore::update(cx, |s| {
                s.update_channel = if index == 1 { UpdateChannel::Nightly } else { UpdateChannel::Latest };
                s.update_channel_configured_by_user = true;
            })
        },
    );
    let mut track_row = SettingsRow::new(setting_title("update-track"))
        .description(t!("Switches this app and every connected machine whose database the target release supports."))
        .control(track);
    if channel != UpdateChannel::Latest {
        track_row = track_row.reset(reset_button("update-track", |_, cx| {
            DesktopSettingsStore::update(cx, |s| s.update_channel = UpdateChannel::Latest)
        }));
    }
    let link = |id: &'static str, label: SharedString, url: &'static str| {
        Button::new(id).variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(label).on_click(move |_, _, cx| cx.open_url(url))
    };
    let store = |id: &'static str, icon: crate::components::icons::BrandIcon, label: SharedString, url: &'static str| link(id, label, url).icon(icon);
    let app = SettingsSection::new(t!("App"))
        .child(
            SettingsRow::new(t!("Version"))
                .title_suffix(version_copy_button(cx))
                .description(t!("Current version of the application.")),
        )
        .child(track_row)
        .child(
            SettingsRow::new(setting_title("mobile-app"))
                .description(t!("Connect to your server from your phone or tablet."))
                .control(
                    div()
                        .flex()
                        .gap(gpui_kit::rems(0.5))
                        .child(store("mobile-app-ios", crate::components::icons::BrandIcon::Apple, t!("iPhone and iPad"), APP_STORE_URL))
                        .child(store("mobile-app-android", crate::components::icons::BrandIcon::Android, SharedString::new_static("Android"), PLAY_STORE_URL)),
                ),
        );
    let legal = SettingsSection::new(t!("Legal"))
        .child(
            SettingsRow::new(setting_title("privacy-policy"))
                .description(t!("How we handle your data, including the anonymous usage data T3 Code collects."))
                .control(link("privacy-policy", t!("View policy"), PRIVACY_POLICY_URL)),
        )
        .child(
            SettingsRow::new(setting_title("open-source-licenses"))
                .description(t!("Notices for dependencies, assets, and optional tools used by T3 Code."))
                .control(
                    Button::new("open-source-licenses")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .label(t!("View licenses"))
                        .on_click(move |_, window, cx| on_open_licenses(window, cx)),
                ),
        );
    page_container(cx).child(app).child(legal).into_any_element()
}
