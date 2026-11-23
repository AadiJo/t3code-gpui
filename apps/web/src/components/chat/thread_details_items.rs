//! Which items the thread details card shows. Ports `threadDetailsItems.ts`:
//! every item has a default, and only choices that differ are stored, so a
//! later default change reaches everyone who never touched it.

use std::collections::BTreeMap;

use gpui_kit::{App, Global, SharedString};

use crate::i18n::t;

/// One item of the card. The ids are stored in client settings; never rename one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThreadDetailsItem {
    Environment,
    OpenIn,
    Scripts,
    Branch,
    GitActions,
    Changes,
    Servers,
    Automations,
    Lineage,
    SideChats,
    Usage,
    Context,
}

impl ThreadDetailsItem {
    /// `THREAD_DETAILS_ITEMS`, in menu order.
    pub const ALL: [Self; 12] = [
        Self::Environment,
        Self::OpenIn,
        Self::Scripts,
        Self::Branch,
        Self::GitActions,
        Self::Changes,
        Self::Servers,
        Self::Automations,
        Self::Lineage,
        Self::SideChats,
        Self::Usage,
        Self::Context,
    ];

    /// The stored id.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::OpenIn => "openIn",
            Self::Scripts => "scripts",
            Self::Branch => "branch",
            Self::GitActions => "gitActions",
            Self::Changes => "changes",
            Self::Servers => "servers",
            Self::Automations => "automations",
            Self::Lineage => "lineage",
            Self::SideChats => "sideChats",
            Self::Usage => "usage",
            Self::Context => "context",
        }
    }

    pub fn label(self) -> SharedString {
        match self {
            Self::Environment => t!("Environment and worktree"),
            Self::OpenIn => t!("Open in editor"),
            Self::Scripts => t!("Project scripts"),
            Self::Branch => t!("Branch and pull request"),
            Self::GitActions => t!("Git actions"),
            Self::Changes => t!("Changes", context = "thread details item: version control changes"),
            Self::Servers => t!("Servers", context = "thread details item: running dev servers"),
            Self::Automations => t!("Automations"),
            Self::Lineage => t!("Lineage", context = "thread details item: parent and child agents"),
            Self::SideChats => t!("Side chats"),
            Self::Usage => t!("Usage limits"),
            Self::Context => t!("Context window"),
        }
    }

    pub const fn default_visible(self) -> bool {
        !matches!(self, Self::Usage | Self::Context)
    }
}

/// The client settings the card reads (`threadDetailsItems`, `threadDetailsUsageEnabled`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadDetailsItemSettings {
    /// Only the choices that differ from the default.
    pub items: BTreeMap<ThreadDetailsItem, bool>,
    /// The retired Labs toggle; people who turned it on keep Usage and Context.
    pub usage_enabled: bool,
}

impl Global for ThreadDetailsItemSettings {}

impl ThreadDetailsItemSettings {
    fn default_for(&self, item: ThreadDetailsItem) -> bool {
        match item {
            ThreadDetailsItem::Usage | ThreadDetailsItem::Context => self.usage_enabled,
            _ => item.default_visible(),
        }
    }

    /// `resolveThreadDetailsVisibility` for one item.
    pub fn visible(&self, item: ThreadDetailsItem) -> bool {
        self.items.get(&item).copied().unwrap_or_else(|| self.default_for(item))
    }

    /// `setThreadDetailsItem`.
    pub fn set(&mut self, item: ThreadDetailsItem, visible: bool) {
        if visible == self.default_for(item) {
            self.items.remove(&item);
        } else {
            self.items.insert(item, visible);
        }
    }

    /// `isDefaultThreadDetailsVisibility`.
    pub fn is_default(&self) -> bool {
        ThreadDetailsItem::ALL.iter().all(|item| self.visible(*item) == item.default_visible())
    }

    /// `RESET_THREAD_DETAILS_ITEMS`: also clears the retired Labs flag.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The app-wide settings, defaults when none were installed.
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }
}
