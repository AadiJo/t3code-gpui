//! Picks the UI language and switches it at runtime. Ports `apps/web/src/i18n/runtime.ts`:
//! the saved `language` setting ("system" or a locale tag) is the only source of truth,
//! the catalog inflates off the main thread, and English stays on screen until it is ready.

use std::sync::Arc;

use gpui_kit::{App, Global};

use super::{
    catalog::Catalog,
    locales::{Locale, SYSTEM_LANGUAGE, resolve_locale_selection},
    messages::{active_locale, set_active},
};
use crate::timestamp_format::set_timestamp_locale;

/// The UI language state. Observe it with `cx.observe_global::<I18n>` to react to a
/// switch; every window is also refreshed when the language changes.
#[derive(Clone, Debug)]
pub struct I18n {
    /// The saved setting this state was resolved from.
    pub language: String,
    /// The locale whose catalog is active.
    pub locale: Locale,
    /// The locale being loaded, when a switch is in flight.
    requested: Locale,
}

impl Global for I18n {}

/// Follows the system language and sets the regional format locale. Call once at startup.
pub fn init(cx: &mut App) {
    set_timestamp_locale(sys_locale::get_locale().as_deref());
    cx.set_global(I18n {
        language: SYSTEM_LANGUAGE.to_owned(),
        locale: Locale::En,
        requested: Locale::En,
    });
    apply_language_setting(SYSTEM_LANGUAGE, cx);
}

/// The host's preferred UI languages, most preferred first.
pub fn system_languages() -> Vec<String> {
    sys_locale::get_locales().collect()
}

/// Applies the `language` client setting. Call it whenever the setting changes.
pub fn apply_language_setting(language: &str, cx: &mut App) {
    let locale = resolve_locale_selection(language, &system_languages()).catalog;
    if !cx.has_global::<I18n>() {
        cx.set_global(I18n {
            language: language.to_owned(),
            locale: active_locale(),
            requested: active_locale(),
        });
    }
    let state = cx.global_mut::<I18n>();
    state.language = language.to_owned();
    state.requested = locale;
    if locale == active_locale() {
        return;
    }
    let load = cx.background_executor().spawn(async move { Catalog::load(locale).map(Arc::new) });
    cx.spawn(async move |cx| {
        let catalog = load.await;
        cx.update(|cx| {
            // A later call may have asked for another language while this one loaded.
            if cx.global::<I18n>().requested != locale {
                return;
            }
            set_active(locale, catalog);
            cx.global_mut::<I18n>().locale = locale;
            cx.refresh_windows();
        })
    })
    .detach();
}

/// Switches synchronously, for tools and tests with no running app.
pub fn activate_locale_blocking(locale: Locale) {
    set_active(locale, Catalog::load(locale).map(Arc::new));
}
