//! Controls the server settings panels share: a switch or select row that
//! shows a policy lock, a reset button, the seconds stepper, small text, and
//! a keyed store of text fields so a panel can create its inputs lazily.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::InputState;
use gpui_kit::{AnyElement, App, AppContext as _, Context, Entity, Hsla, IntoElement, SharedString, Window, div, prelude::*};

use super::settings_layout::SettingsRow;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::managed_policy::{ManagedControl, managed_by_reason};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{InputSize, TextInput};
use crate::components::ui::select::{Select, SelectSize};
use crate::components::ui::switch::Switch;
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::i18n::t;

/// Why a row's control is inert: an administrator's lock or missing permission.
pub fn lock_reason(managed_by: Option<SharedString>, read_only: bool) -> Option<SharedString> {
    managed_by.map(|org| managed_by_reason(&org)).or_else(|| {
        read_only.then(|| t!("This connection lacks permission to change settings on the selected environment."))
    })
}

/// A settings row with a switch. A lock draws the switch inert with the reason on hover.
pub fn switch_row(
    id: impl Into<SharedString>,
    title: SharedString,
    description: Option<SharedString>,
    checked: bool,
    lock: Option<SharedString>,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> SettingsRow {
    let id = id.into();
    let switch = Switch::new(SharedString::from(format!("{id}-switch")), checked)
        .disabled(lock.is_some())
        .on_change(on_change);
    let row = SettingsRow::new(title).control(ManagedControl::new(format!("{id}-lock"), lock, switch));
    match description {
        Some(d) => row.description(d),
        None => row,
    }
}

/// A settings row with a select over `options`.
pub fn select_row(
    id: impl Into<SharedString>,
    title: SharedString,
    description: Option<SharedString>,
    options: Vec<SharedString>,
    selected: usize,
    lock: Option<SharedString>,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> SettingsRow {
    let id = id.into();
    let select = Select::new(SharedString::from(format!("{id}-select")), options, selected, on_change).size(SelectSize::Sm);
    let row = SettingsRow::new(title).control(ManagedControl::new(format!("{id}-lock"), lock, div().min_w(gpui_kit::px(160.)).child(select)));
    match description {
        Some(d) => row.description(d),
        None => row,
    }
}

/// `SettingResetButton`: a small undo arrow beside a changed value.
pub fn reset_button(id: impl Into<SharedString>, label: SharedString, on_click: impl Fn(&mut Window, &mut App) + 'static) -> impl IntoElement {
    let id: SharedString = id.into();
    div().id(SharedString::from(format!("{id}-tip"))).tooltip(tooltip(TooltipSurface::new(label))).child(
        Button::new(id)
            .variant(ButtonVariant::GhostMuted)
            .size(ButtonSize::IconMicro)
            .icon(IconName::RotateCcw)
            .on_click(move |_, window, cx| on_click(window, cx)),
    )
}

/// Muted extra-small text, for hints under a control.
pub fn hint(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let theme = cx.app_theme();
    div()
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .text_color(theme.colors.muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// Extra-small text in a colour, for errors and statuses.
pub fn tinted(text: impl Into<SharedString>, color: Hsla, cx: &App) -> AnyElement {
    let theme = cx.app_theme();
    div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(color).child(text.into()).into_any_element()
}

/// `IntervalSecondsField`: minus, a number, plus. Commits whole multiples of `step` at or above `min`.
pub fn interval_seconds_field(
    id: impl Into<SharedString>,
    value: u64,
    min: u64,
    step: u64,
    disabled: bool,
    on_commit: impl Fn(u64, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.app_theme();
    let id: SharedString = id.into();
    let commit = Rc::new(on_commit);
    let (down, up) = (commit.clone(), commit);
    div()
        .flex()
        .items_center()
        .gap(theme.sp(1.))
        .child(
            Button::new(SharedString::from(format!("{id}-dec")))
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::IconSm)
                .icon(IconName::Minus)
                .disabled(disabled || value <= min)
                .on_click(move |_, window, cx| down(value.saturating_sub(step).max(min), window, cx)),
        )
        .child(
            div()
                .min_w(theme.sp(14.))
                .h(theme.sp(7.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(theme.colors.input)
                .mono_text(gpui_kit::px(TextSize::Sm.size(theme.fonts.interface)), weight::NORMAL)
                .child(value.to_string()),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-inc")))
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::IconSm)
                .icon(IconName::Plus)
                .disabled(disabled)
                .on_click(move |_, window, cx| up(value + step, window, cx)),
        )
}

/// Text fields a panel creates on first render, keyed by a stable name.
#[derive(Default)]
pub struct Fields {
    fields: HashMap<SharedString, Entity<InputState>>,
}

impl Fields {
    /// The field for `key`, created with `initial` and `placeholder` the first time.
    pub fn get<V: 'static>(
        &mut self,
        key: impl Into<SharedString>,
        initial: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        masked: bool,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<InputState> {
        let key = key.into();
        if let Some(field) = self.fields.get(&key) {
            return field.clone();
        }
        let (initial, placeholder) = (initial.into(), placeholder.into());
        let field = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).masked(masked).default_value(initial));
        self.fields.insert(key, field.clone());
        field
    }

    /// The current text of a field, if it exists.
    pub fn value(&self, key: &str, cx: &App) -> Option<SharedString> {
        self.fields.get(key).map(|f| f.read(cx).value())
    }

    /// Replaces a field's text.
    pub fn set(&self, key: &str, value: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        if let Some(field) = self.fields.get(key) {
            let value = value.into();
            field.update(cx, |state, cx| state.set_value(value, window, cx));
        }
    }

    /// Forgets a field so the next `get` recreates it from a new initial value.
    pub fn reset(&mut self, key: &str) {
        self.fields.remove(key);
    }

    /// Forgets every field whose key starts with `prefix`.
    pub fn reset_prefix(&mut self, prefix: &str) {
        self.fields.retain(|key, _| !key.starts_with(prefix));
    }
}

/// A small text input bound to a field from [`Fields`].
pub fn field_input(field: &Entity<InputState>, mono: bool, invalid: bool, disabled: bool) -> TextInput {
    let input = TextInput::new(field).size(InputSize::Sm).invalid(invalid).disabled(disabled);
    if mono { input.mono() } else { input }
}

/// Sends a settings patch (`server.updateSettings`) and toasts a failure.
pub fn save_patch(
    model: &Entity<super::server_settings_data::ServerPanelsModel>,
    key: impl Into<SharedString>,
    patch: t3code_contracts::ClientServerSettingsPatch,
    cx: &mut App,
) {
    let input = t3code_contracts::ServerUpdateSettingsPayload { patch, provider_instance_mutation: None };
    model.update(cx, |model, cx| {
        model.run(key, move |s| s.update_settings(input), |result, cx| {
            if let Err(error) = result {
                super::provider_settings_panel::toast_error(t!("Could not save settings"), error, cx);
            }
        }, cx)
    });
}

/// A switch row bound to one boolean server setting.
pub fn bool_setting(
    model: &Entity<super::server_settings_data::ServerPanelsModel>,
    id: &'static str,
    title: SharedString,
    description: Option<SharedString>,
    value: bool,
    lock: Option<SharedString>,
    patch: fn(bool) -> t3code_contracts::ClientServerSettingsPatch,
) -> SettingsRow {
    let model = model.clone();
    switch_row(id, title, description, value, lock, move |checked, _, cx| save_patch(&model, id, patch(checked), cx))
}
