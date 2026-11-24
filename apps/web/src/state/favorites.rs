//! Model favourites on live data (`useToggleEnvironmentModelFavorite`): an environment shows its
//! server's favourites, or the client settings' legacy list while the server has none, and a
//! toggle sends the desired state (`setModelFavorites`) with the one-time migration of the
//! legacy list (`modelPreferencesMigrationPatch`).

use gpui_kit::{App, SharedString};
use t3code_contracts::{
    ClientServerSettingsPatch, ClientServerSettingsPatchMigrateModelPreferences, ClientServerSettingsPatchSetModelFavoritesItem,
    EnvironmentId, ModelFavorites, ProviderInstanceId as WireInstanceId, ProviderModelPreferences, ProviderModelPreferencesInnerValue, ServerConfig, ServerUpdateSettingsPayload,
};

use super::StateBridge;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::t;
use crate::model_selection::ProviderInstanceId;

/// The favourites an environment shows (`resolveModelPreferences`).
pub fn effective(config: Option<&ServerConfig>, cx: &App) -> ModelFavorites {
    config
        .and_then(|c| c.settings.favorites.clone())
        .unwrap_or_else(|| ClientSettingsStore::get(cx).favorites.clone())
}

/// Flips one favourite on `environment_id`'s server.
pub fn toggle(environment_id: &EnvironmentId, instance: &ProviderInstanceId, model: &SharedString, cx: &mut App) {
    let Some(environment) = StateBridge::environment(environment_id, cx) else { return };
    let config = environment.read(cx).config.clone();
    let Some(link) = crate::live::rpc::RpcLink::new(environment_id, cx) else { return };
    let current = effective(config.as_deref(), cx);
    let favorite = !current.iter().any(|f| f.provider.as_str() == instance.0.as_ref() && f.model == model.as_ref());
    let client = ClientSettingsStore::get(cx);
    let server = config.as_ref().map(|c| &c.settings);
    let migrate_favorites = server.is_some_and(|s| s.favorites.is_none()).then(|| client.favorites.clone()).filter(|f| !f.is_empty());
    let migrate_preferences: Option<ProviderModelPreferences> = server
        .is_some_and(|s| s.provider_model_preferences.is_none())
        .then(|| {
            client
                .provider_model_preferences
                .iter()
                .map(|(instance, prefs)| {
                    let value = ProviderModelPreferencesInnerValue { hidden_models: prefs.hidden_models.clone(), model_order: prefs.model_order.clone() };
                    (instance.clone(), value)
                })
                .collect::<ProviderModelPreferences>()
        })
        .filter(|p| !p.is_empty());
    let migrate = (migrate_favorites.is_some() || migrate_preferences.is_some()).then(|| ClientServerSettingsPatchMigrateModelPreferences {
        favorites: migrate_favorites,
        provider_model_preferences: migrate_preferences,
    });
    let patch = ClientServerSettingsPatch {
        migrate_model_preferences: migrate,
        set_model_favorites: Some(vec![ClientServerSettingsPatchSetModelFavoritesItem {
            provider: WireInstanceId::new(instance.0.as_ref()),
            model: model.to_string(),
            favorite,
        }]),
        ..Default::default()
    };
    let task = link.call::<t3code_contracts::rpc::methods::ServerUpdateSettings>(ServerUpdateSettingsPayload { patch, provider_instance_mutation: None });
    cx.spawn(async move |cx| {
        if let Err(error) = task.await {
            cx.update(|cx| {
                crate::components::ui::toast::Toaster::push(
                    crate::components::ui::toast::ToastData::new(crate::components::ui::toast::ToastKind::Error, t!("Model preferences not saved"))
                        .description(error),
                    cx,
                )
            });
        }
    })
    .detach();
}
