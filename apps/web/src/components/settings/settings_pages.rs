//! The device preference pages the settings route shows: it picks the page
//! for the current path, re-renders when a store changes, adds the scope
//! sentence on pages saved on a server, and owns the "Restore device
//! defaults" confirmation.

use std::rc::Rc;

use gpui_kit::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};

use super::appearance_settings::AppearanceSettings;
use super::archived_threads_panel::ArchivedThreadsPanel;
use super::keybindings_settings::KeybindingsSettings;
use super::settings_controls::{SettingsScope, restore_device_defaults, restore_defaults_button, scope_sentence};
use super::settings_panels::{about_panel, chat_panel, diffs_panel, general_panel, labs_panel};
use super::settings_search::SettingsPath;
use super::snap_shot_settings::snap_shot_panel;
use super::voice_settings_panel::VoiceSettings;
use crate::ActiveAppTheme as _;
use crate::client_persistence_storage::LocalStorage;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::hooks::use_settings::{ClientSettingsStore, DesktopSettingsStore};
use crate::hooks::use_theme::ThemeStore;
use crate::i18n::t;

/// Which pages this entity draws. Environments and the server-heavy pages
/// belong to other views.
pub fn is_device_page(path: SettingsPath) -> bool {
    matches!(
        path,
        SettingsPath::General
            | SettingsPath::Appearance
            | SettingsPath::Chat
            | SettingsPath::Diffs
            | SettingsPath::Keybindings
            | SettingsPath::Voice
            | SettingsPath::SnapShot
            | SettingsPath::Labs
            | SettingsPath::Archived
            | SettingsPath::About
    )
}

/// The device pages and their view state.
pub struct SettingsPages {
    path: SettingsPath,
    chat_advanced: bool,
    appearance: Entity<AppearanceSettings>,
    keybindings: Entity<KeybindingsSettings>,
    archived: Entity<ArchivedThreadsPanel>,
    voice: Entity<VoiceSettings>,
    /// Labels of what "Restore device defaults" will reset, while it asks.
    confirm_restore: Option<Vec<SharedString>>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsPages {
    pub fn new(path: SettingsPath, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global::<ClientSettingsStore>(|_, cx| cx.notify()),
            cx.observe_global::<DesktopSettingsStore>(|_, cx| cx.notify()),
            cx.observe_global::<LocalStorage>(|_, cx| cx.notify()),
            cx.observe_global::<ThemeStore>(|_, cx| cx.notify()),
            cx.observe_global::<SettingsScope>(|_, cx| cx.notify()),
        ];
        Self {
            path,
            chat_advanced: false,
            appearance: cx.new(|cx| AppearanceSettings::new(window, cx)),
            keybindings: cx.new(|cx| KeybindingsSettings::new(window, cx)),
            archived: cx.new(|cx| ArchivedThreadsPanel::new(window, cx)),
            voice: cx.new(|cx| VoiceSettings::new(window, cx)),
            confirm_restore: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_path(&mut self, path: SettingsPath, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            cx.notify();
        }
    }

    /// The Keybindings page, for scripted checks.
    pub fn keybindings(&self) -> &Entity<KeybindingsSettings> {
        &self.keybindings
    }

    /// The Appearance page, for scripted checks.
    pub fn appearance(&self) -> &Entity<AppearanceSettings> {
        &self.appearance
    }

    /// The header's "Restore device defaults" button, for device-only pages.
    pub fn restore_button(this: &Entity<Self>, cx: &App) -> Option<AnyElement> {
        if !this.read(cx).path.is_device_only() {
            return None;
        }
        let pages = this.clone();
        Some(restore_defaults_button(
            move |changed, _, cx| {
                pages.update(cx, |this, cx| {
                    this.confirm_restore = Some(changed);
                    cx.notify();
                })
            },
            cx,
        ))
    }

    fn render_page(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        match self.path {
            SettingsPath::General => general_panel(cx),
            SettingsPath::Appearance => self.appearance.clone().into_any_element(),
            SettingsPath::Chat => {
                let toggle: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |_, cx| {
                    this.update(cx, |this, cx| {
                        this.chat_advanced = !this.chat_advanced;
                        cx.notify();
                    })
                });
                chat_panel(self.chat_advanced, toggle, cx)
            }
            SettingsPath::Diffs => diffs_panel(cx),
            SettingsPath::Keybindings => self.keybindings.clone().into_any_element(),
            SettingsPath::Voice => self.voice.clone().into_any_element(),
            SettingsPath::SnapShot => snap_shot_panel(cx),
            SettingsPath::Labs => labs_panel(cx),
            SettingsPath::Archived => self.archived.clone().into_any_element(),
            SettingsPath::About => about_panel(Rc::new(|_, cx| cx.open_url(LICENSES_URL)), cx),
            _ => div().into_any_element(),
        }
    }
}

/// The open source notices the web shows at `/settings/open-source-licenses`.
const LICENSES_URL: &str = "https://github.com/pingdotgg/t3code/blob/main/LICENSE";

impl Render for SettingsPages {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.render_page(window, cx);
        let theme = cx.app_theme();
        let scoped = !self.path.is_device_only() && self.path != SettingsPath::Keybindings;
        let confirm = self.confirm_restore.clone().map(|changed| {
            let close = cx.listener(|this, _: &(), _, cx| {
                this.confirm_restore = None;
                cx.notify();
            });
            let close = Rc::new(close);
            let cancel = close.clone();
            let restore = cx.listener(|this, _: &(), _, cx| {
                this.confirm_restore = None;
                restore_device_defaults(cx);
                cx.notify();
            });
            let surface = DialogSurface::new(t!("Restore default settings?"))
                .description(t!("This will reset: {settings}.", settings = changed.join(", ")))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap(theme.sp(2.))
                        .child(Button::new("restore-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(move |_, w, cx| cancel(&(), w, cx)))
                        .child(Button::new("restore-confirm").variant(ButtonVariant::Destructive).label(t!("Restore device defaults")).on_click(move |_, w, cx| restore(&(), w, cx))),
                );
            Modal::dialog("restore-defaults-dialog", surface, move |w, cx| close(&(), w, cx))
        });
        div()
            .flex()
            .flex_col()
            .when(scoped, |d| d.child(div().pt(theme.sp(6.)).max_w(theme.rem(56.)).mx_auto().w_full().px(theme.sp(6.)).child(scope_sentence(cx))))
            .child(page)
            .children(confirm)
    }
}
