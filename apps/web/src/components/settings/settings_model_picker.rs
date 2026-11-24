//! `ProviderModelPicker` as settings rows use it (`ProjectDefaultsSettings.tsx`
//! with `SETTINGS_PICKER_TRIGGER_CLASSNAME`): an outlined button with the
//! provider's mark and the model name that opens the model picker below it,
//! without the composer's effort column and footer.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    TestSupportExt as _,
    App, AppContext as _, ElementId, Entity, Focusable as _, IntoElement, Pixels, Point, RenderOnce, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use t3code_contracts::ModelSelection as SavedSelection;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::chat::composer_icons::provider_icon;
use crate::components::chat::composer_permissions::RuntimeMode;
use crate::components::chat::composer_theme::ComposerColors;
use crate::components::chat::model_picker::{ModelPicker, ModelPickerEvent, input_handler_canvas};
use crate::components::ui::{ControlHighlightExt as _, TextStyleExt as _};
use crate::model_selection::{ModelCatalog, ModelSelection, ProviderIcon, ProviderInstanceId, display_model_name};

/// The open picker for one trigger, kept with `window.use_keyed_state`.
#[derive(Default)]
struct PickerSlot {
    open: Option<(Entity<ModelPicker>, Subscription)>,
    /// Where the outside press that last closed it happened, so the same
    /// press landing on the trigger does not reopen it.
    closed_at: Option<Point<Pixels>>,
}

/// A settings model picker. `selection` is the saved value; `label`
/// replaces the trigger's mark and name ("None" for an unset fallback).
#[derive(IntoElement)]
pub struct SettingsModelPicker {
    id: SharedString,
    catalog: Arc<ModelCatalog>,
    selection: Option<SavedSelection>,
    label: Option<SharedString>,
    disabled: bool,
    on_pick: Rc<dyn Fn(SavedSelection, &mut App)>,
}

impl SettingsModelPicker {
    /// `id` names the control; its trigger is `model-trigger-<id>`.
    pub fn new(
        id: impl Into<SharedString>,
        catalog: Arc<ModelCatalog>,
        selection: Option<SavedSelection>,
        on_pick: impl Fn(SavedSelection, &mut App) + 'static,
    ) -> Self {
        Self { id: id.into(), catalog, selection, label: None, disabled: false, on_pick: Rc::new(on_pick) }
    }

    /// `triggerLabel`: text shown instead of the model (no mark).
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// `pickModelSelection`: re-picking the saved model keeps its options.
fn picked(current: Option<&SavedSelection>, next: &ModelSelection) -> SavedSelection {
    match current {
        Some(current) if current.instance_id == next.instance_id.0.as_ref() && current.model == next.model.as_ref() => current.clone(),
        _ => SavedSelection { instance_id: next.instance_id.0.to_string(), model: next.model.to_string(), options: None },
    }
}

impl RenderOnce for SettingsModelPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let slot = window.use_keyed_state(ElementId::from(self.id.clone()), cx, |_, _| PickerSlot::default());
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let entry = self
            .selection
            .as_ref()
            .and_then(|s| self.catalog.instances.iter().find(|e| e.instance_id.0.as_ref() == s.instance_id))
            .cloned();
        let model_name = self.selection.as_ref().map(|s| {
            self.catalog
                .models
                .get(&ProviderInstanceId(SharedString::from(s.instance_id.clone())))
                .and_then(|models| models.iter().find(|m| m.slug.as_ref() == s.model))
                .map_or_else(|| SharedString::from(s.model.clone()), |m| display_model_name(m, false))
        });
        let colors = ComposerColors::new(cx);
        let mark = match (&self.label, &entry, &self.selection) {
            (None, Some(entry), Some(selection)) => Some(provider_icon(
                ProviderIcon::for_model(&entry.driver_kind, Some(&selection.model)),
                sp(4.),
                &entry.display_name,
                &colors,
                cx,
            )),
            _ => None,
        };
        let text = self.label.clone().or(model_name).unwrap_or_default();
        let open = slot.read(cx).open.as_ref().map(|(picker, _)| picker.clone());
        let disabled = self.disabled;
        let trigger = div()
            .id(SharedString::from(format!("model-trigger-{}", self.id)))
            .test_support()
            .flex()
            .flex_none()
            .items_center()
            .gap(sp(1.5))
            .h(sp(6.5) + px(2.))
            .px(sp(2.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(c.input)
            .bg(theme.derived.input_fill)
            .shadow(theme.shadows.xs5.clone())
            .control_highlight(theme)
            .ui_text(theme, TextSize::Sm, weight::NORMAL)
            .text_color(c.foreground())
            .whitespace_nowrap()
            .children(mark)
            .child(div().min_w_0().truncate().child(text))
            .when(disabled, |d| d.opacity(crate::components::ui::DISABLED_OPACITY))
            .when(!disabled, |d| {
                let (slot, catalog, saved, on_pick) = (slot.clone(), self.catalog.clone(), self.selection.clone(), self.on_pick.clone());
                d.cursor_pointer().hover(|d| d.bg(c.accent.opacity(0.5))).on_click(move |event, window, cx| {
                    let press = match event {
                        gpui_kit::ClickEvent::Mouse(event) => event.down.position,
                        other => other.position(),
                    };
                    let state = slot.read(cx);
                    if state.closed_at == Some(press) {
                        slot.update(cx, |s, _| s.closed_at = None);
                        return;
                    }
                    if state.open.is_some() {
                        slot.update(cx, |s, cx| {
                            s.open = None;
                            cx.notify();
                        });
                        return;
                    }
                    let selection = saved.as_ref().map_or_else(
                        || ModelSelection {
                            // Nothing saved: open on the first ready instance with a current model.
                            instance_id: catalog
                                .instances
                                .iter()
                                .find(|e| e.ready && catalog.models.get(&e.instance_id).is_some_and(|m| m.iter().any(|m| !m.is_legacy && !m.is_unavailable)))
                                .or(catalog.instances.first())
                                .map(|e| e.instance_id.clone())
                                .unwrap_or(ProviderInstanceId(SharedString::default())),
                            model: SharedString::default(),
                            effort: None,
                            speed: None,
                            ultracode: false,
                        },
                        |s| ModelSelection {
                            instance_id: ProviderInstanceId(SharedString::from(s.instance_id.clone())),
                            model: SharedString::from(s.model.clone()),
                            effort: None,
                            speed: None,
                            ultracode: false,
                        },
                    );
                    let picker = cx.new(|cx| ModelPicker::new(catalog.clone(), selection, RuntimeMode::default(), cx).plain());
                    let (slot_events, saved, on_pick) = (slot.clone(), saved.clone(), on_pick.clone());
                    let subscription = window.subscribe(&picker, cx, move |_, event: &ModelPickerEvent, _window, cx| {
                        let close = |cx: &mut App| {
                            slot_events.update(cx, |s, cx| {
                                s.open = None;
                                cx.notify();
                            })
                        };
                        match event {
                            ModelPickerEvent::Select(next) => {
                                on_pick(picked(saved.as_ref(), next), cx);
                                close(cx);
                            }
                            ModelPickerEvent::Close => close(cx),
                            ModelPickerEvent::ToggleFavorite { .. } | ModelPickerEvent::RuntimeModeChanged(_) => {}
                        }
                    });
                    window.focus(&picker.focus_handle(cx), cx);
                    slot.update(cx, |s, cx| {
                        s.open = Some((picker, subscription));
                        cx.notify();
                    });
                })
            });
        let close_slot = slot.clone();
        div().id(self.id).flex().flex_col().child(trigger).when_some(open, |d, picker| {
            d.child(crate::components::ui::popover::below(
                div()
                    .on_mouse_down_out(move |event, _, cx| {
                        close_slot.update(cx, |s, cx| {
                            s.open = None;
                            s.closed_at = Some(event.position);
                            cx.notify();
                        })
                    })
                    .child(picker.clone())
                    .child(input_handler_canvas(picker)),
            ))
        })
    }
}
