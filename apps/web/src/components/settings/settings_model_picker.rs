//! `ProviderModelPicker` as settings rows use it (`ProjectDefaultsSettings.tsx`
//! with `SETTINGS_PICKER_TRIGGER_CLASSNAME`): an outlined button with the
//! provider's mark and the model name that opens the model picker below it,
//! without the composer's effort column and footer. With [`SettingsModelPicker::traits`] the
//! model's options sit beside it as `TraitsPicker` draws them in settings: a chip naming the
//! current values whose menu lists each option.

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
use crate::components::icons::{BrandIcon, Icon, IconSize};
use crate::components::ui::{ControlHighlightExt as _, TextStyleExt as _};
use crate::provider_instances::provider_instance_initials;
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
    traits: bool,
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
        Self { id: id.into(), catalog, selection, label: None, disabled: false, traits: false, on_pick: Rc::new(on_pick) }
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

    /// Shows the model's options chip beside the trigger, as settings rows with a model do.
    pub fn traits(mut self, traits: bool) -> Self {
        self.traits = traits;
        self
    }
}

/// `SETTINGS_PICKER_TRIGGER_CLASSNAME` on a `ComposerControl`: the outlined settings trigger.
fn settings_trigger(id: SharedString, cx: &App) -> gpui_kit::Stateful<gpui_kit::Div> {
    let theme = cx.app_theme();
    let c = &theme.colors;
    let sp = |n: f32| theme.sp(n);
    div()
        .id(id)
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
}

/// The open traits menu for one chip, kept with `window.use_keyed_state`.
#[derive(Default)]
struct TraitsSlot {
    open: bool,
    closed_at: Option<Point<Pixels>>,
}

/// `TraitsPicker` with `allowPromptInjectedEffort={false}`: the chip and its menu. Nothing when
/// the model has no options.
fn traits_chip(id: &SharedString, catalog: &ModelCatalog, saved: &SavedSelection, on_pick: Rc<dyn Fn(SavedSelection, &mut App)>, window: &mut Window, cx: &mut App) -> Option<gpui_kit::AnyElement> {
    let selection = crate::provider_models_live::from_wire(saved);
    let traits = selection.traits(catalog).filter(|t| t.any())?;
    let slot = window.use_keyed_state(ElementId::from(SharedString::from(format!("{id}-traits"))), cx, |_, _| TraitsSlot::default());
    let theme = cx.app_theme();
    let c = &theme.colors;
    let open = slot.read(cx).open;
    let label = traits.trigger_label();
    let toggle = slot.clone();
    let trigger = settings_trigger(SharedString::from(format!("traits-trigger-{id}")), cx)
        .min_w_0()
        .cursor_pointer()
        .hover(|d| d.bg(theme.colors.accent.opacity(0.5)))
        .child(div().child(label))
        .on_click(move |event, _, cx| {
            let press = crate::components::ui::popover::press_position(event);
            toggle.update(cx, |s, cx| {
                if s.closed_at == Some(press) {
                    s.closed_at = None;
                } else {
                    s.open = !s.open;
                }
                cx.notify();
            });
        })
        .test_support();
    // Writes one option, as `onModelOptionsChange` with `createModelSelection` does.
    let write = {
        let (slot, traits, saved) = (slot.clone(), traits.clone(), saved.clone());
        Rc::new(move |id: SharedString, value: crate::model_selection::OptionValue, cx: &mut App| {
            let mut next = selection.clone();
            next.options = traits.with_value(&id, value);
            let mut wire = crate::provider_models_live::to_wire(&next, &ModelCatalog::default(), None);
            wire.instance_id = saved.instance_id.clone();
            on_pick(wire, cx);
            slot.update(cx, |s, cx| {
                s.open = false;
                cx.notify();
            });
        })
    };
    let menu = open.then(|| {
        let mut sections: Vec<gpui_kit::AnyElement> = Vec::new();
        let header = |text: SharedString| {
            div().px(theme.sp(2.)).pt(theme.sp(1.5)).pb(theme.sp(1.)).ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(c.muted_foreground).child(text)
        };
        let separator = || div().mx(theme.sp(2.)).my(theme.sp(1.)).h(px(1.)).bg(c.border);
        let item = |key: String, checked: bool, content: gpui_kit::AnyElement, on: Rc<dyn Fn(&mut App)>| {
            div()
                .id(SharedString::from(key))
                .flex()
                .items_center()
                .min_h(theme.sp(7.))
                .px(theme.sp(2.))
                .py(theme.sp(1.))
                .rounded(theme.radii.sm)
                .cursor_pointer()
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.foreground())
                .when(checked, |d| d.bg(c.foreground().opacity(0.08)))
                .hover(|d| d.bg(theme.colors.accent).text_color(theme.colors.accent_foreground))
                .child(content)
                .on_click(move |_, _, cx| on(cx))
                .test_support()
                .into_any_element()
        };
        let selects = traits.descriptors.iter().filter_map(|d| match d {
            crate::model_selection::OptionDescriptor::Select(s) => Some(s),
            _ => None,
        });
        for (n, d) in selects.enumerate() {
            if n > 0 {
                sections.push(separator().into_any_element());
            }
            sections.push(header(d.label.clone()).into_any_element());
            let current = d.value().cloned();
            for (i, option) in d.selectable().into_iter().enumerate() {
                let content = div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .min_w_0()
                    .child(
                        div().flex().min_w_0().items_center().gap(theme.sp(1.)).child(div().min_w_0().truncate().child(option.label.clone())).when(option.is_default, |d| {
                            d.child(
                                crate::components::ui::badge::Badge::new(crate::i18n::t!("Default", context = "the default option in a list"))
                                    .variant(crate::components::ui::badge::BadgeVariant::Outline)
                                    .size(crate::components::ui::badge::BadgeSize::Sm),
                            )
                        }),
                    )
                    .children(option.description.clone().map(|text| div().max_w(theme.rem(14.)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground.opacity(0.8)).child(text)))
                    .into_any_element();
                let (write, id, value) = (write.clone(), d.id.clone(), option.id.clone());
                sections.push(item(format!("traits-{}-{i}", d.id), current.as_ref() == Some(&option.id), content, Rc::new(move |cx| write(id.clone(), crate::model_selection::OptionValue::Text(value.clone()), cx))));
            }
        }
        let booleans = traits.descriptors.iter().filter_map(|d| match d {
            crate::model_selection::OptionDescriptor::Boolean(b) => Some(b),
            _ => None,
        });
        let any_select = traits.descriptors.iter().any(|d| matches!(d, crate::model_selection::OptionDescriptor::Select(_)));
        for (n, b) in booleans.enumerate() {
            if n > 0 || any_select {
                sections.push(separator().into_any_element());
            }
            sections.push(div().px(theme.sp(2.)).py(theme.sp(1.5)).ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(c.muted_foreground).child(b.label.clone()).into_any_element());
            for on in [true, false] {
                let label = if on { crate::i18n::t!("On", context = "toggle state") } else { crate::i18n::t!("Off", context = "toggle state") };
                let (write, id) = (write.clone(), b.id.clone());
                sections.push(item(format!("traits-{}-{}", b.id, if on { "on" } else { "off" }), (b.current == Some(true)) == on, div().child(label).into_any_element(), Rc::new(move |cx| write(id.clone(), crate::model_selection::OptionValue::Bool(on), cx))));
            }
        }
        let close = slot.clone();
        crate::components::ui::popover::below(
            crate::components::ui::popover::glass_surface(theme)
                .id(SharedString::from(format!("traits-menu-{id}")))
                .min_w(theme.rem(10.))
                .p(theme.sp(1.))
                .on_mouse_down_out(move |event, _, cx| {
                    close.update(cx, |s, cx| {
                        s.open = false;
                        s.closed_at = Some(event.position);
                        cx.notify();
                    })
                })
                .children(sections),
        )
    });
    Some(div().flex().flex_col().child(trigger).children(menu).into_any_element())
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
            (None, Some(entry), Some(selection)) => {
                // `ProviderInstanceIcon size-4`: ACP registry agents draw the package mark.
                let icon = if entry.driver_kind.as_str() == "acpRegistry" {
                    Icon::new(BrandIcon::AcpRegistry).size(IconSize::Custom(sp(4.))).color(c.foreground()).into_any_element()
                } else {
                    provider_icon(ProviderIcon::for_model(&entry.driver_kind, Some(&selection.model)), sp(4.), &entry.display_name, &colors, cx)
                };
                // The trigger's badge (`right/bottom -0.125rem h-3 min-w-3 px-0.5 text-5xs`, ringed in
                // the input colour) tells apart instances that share a driver. The catalog carries no
                // accent colours, so an accent-coloured badge draws in the card colour here.
                let shared = self.catalog.instances.iter().filter(|other| other.driver_kind == entry.driver_kind).count() > 1;
                let badge = shared.then(|| {
                    let (bg, fg) = (c.card(), c.muted_foreground);
                    div()
                        .absolute()
                        .right(-theme.rem(0.125))
                        .bottom(-theme.rem(0.125))
                        .flex()
                        .h(sp(3.))
                        .min_w(sp(3.))
                        .px(sp(0.5))
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_1()
                        .border_color(c.input)
                        .bg(bg)
                        .shadow(theme.shadows.sm5.clone())
                        .ui_text(theme, TextSize::Xs5, weight::SEMIBOLD)
                        .line_height(px(TextSize::Xs5.size(theme.fonts.interface)))
                        .text_color(fg)
                        .child(provider_instance_initials(&entry.display_name))
                });
                Some(div().relative().flex().flex_none().size(sp(4.)).items_center().justify_center().child(icon).children(badge).into_any_element())
            }
            _ => None,
        };
        let text = self.label.clone().or(model_name).unwrap_or_default();
        let open = slot.read(cx).open.as_ref().map(|(picker, _)| picker.clone());
        let disabled = self.disabled;
        let trigger = settings_trigger(SharedString::from(format!("model-trigger-{}", self.id)), cx)
            .test_support()
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
                            options: Vec::new(),
                        },
                        |s| ModelSelection {
                            instance_id: ProviderInstanceId(SharedString::from(s.instance_id.clone())),
                            model: SharedString::from(s.model.clone()),
                            options: Vec::new(),
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
        let chip = self
            .traits
            .then(|| self.selection.as_ref().and_then(|saved| traits_chip(&self.id, &self.catalog, saved, self.on_pick.clone(), window, cx)))
            .flatten();
        let picker = div().id(self.id.clone()).flex().flex_col().child(trigger).when_some(open, |d, picker| {
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
        });
        // `flex min-w-0 flex-wrap items-center justify-end gap-1.5` around the picker and its chip.
        match chip {
            Some(chip) => div().flex().min_w_0().flex_wrap().items_center().justify_end().gap(cx.app_theme().sp(1.5)).child(picker).child(chip).into_any_element(),
            None => picker.into_any_element(),
        }
    }
}
