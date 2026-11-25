//! `LocalEnvironmentSetting.tsx` in its "This app" section, as the fork draws it with the
//! local environment turned off. This client bundles no server, so the switch stays off and
//! disabled, and its tooltip says why.

use gpui_kit::{IntoElement, ParentElement as _, StatefulInteractiveElement as _, div, prelude::*};

use super::settings_layout::{SettingsRow, SettingsSection};
use super::settings_search::setting_title;
use crate::components::ui::switch::Switch;
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::i18n::t;

/// The "This app" section with its one "Local environment" row, off.
pub fn local_environment_section() -> impl IntoElement {
    let switch = div()
        .id("local-environment-tip")
        .tooltip(tooltip(TooltipSurface::new(t!("This client has no bundled server."))))
        .child(Switch::new("local-environment-switch", false).disabled(true));
    SettingsSection::new(t!("This app")).child(
        SettingsRow::new(setting_title("local-environment"))
            .description(t!("Turned off. Agents only run in remote environments."))
            .control(switch),
    )
}
