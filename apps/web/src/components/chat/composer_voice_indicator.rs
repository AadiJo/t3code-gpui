//! `GlobalVoiceInputIndicator`: while the draft a recording writes into is off screen, a
//! capsule at the top of the window names it (its button returns to it) and carries the same
//! recording controls as the composer.

use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px,
};

use super::composer_speech::{PillActions, recording_pill};
use super::composer_theme::ComposerColors;
use crate::ActiveAppTheme as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;
use crate::speech::voice_input_session::VoiceInput;

/// The indicator view; one per app.
pub struct GlobalVoiceInputIndicator {
    _observe: gpui_kit::Subscription,
}

struct IndicatorGlobal(Entity<GlobalVoiceInputIndicator>);
impl Global for IndicatorGlobal {}

impl GlobalVoiceInputIndicator {
    /// The layer the window root draws over everything.
    pub fn layer(cx: &mut App) -> AnyElement {
        if let Some(global) = cx.try_global::<IndicatorGlobal>() {
            return global.0.clone().into_any_element();
        }
        let voice = VoiceInput::global(cx);
        let entity = cx.new(|cx| Self { _observe: cx.observe(&voice, |_, _, cx| cx.notify()) });
        cx.set_global(IndicatorGlobal(entity.clone()));
        entity.into_any_element()
    }
}

impl Render for GlobalVoiceInputIndicator {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(voice) = VoiceInput::try_global(cx) else { return div().into_any_element() };
        let snapshot = voice.read(cx).off_screen().map(|(target, state)| (target.clone(), state.clone()));
        let Some((target, state)) = snapshot else { return div().into_any_element() };
        let owner = target.owner_key.clone();
        let started = voice.read(cx).recording_started_at(&owner);
        let levels = voice.read(cx).activity_for(&owner).map(|a| a.levels).unwrap_or_default();
        let c = ComposerColors::new(cx);
        let theme = cx.app_theme();
        let actions = {
            let (cancel, finish, skip, retry) = (owner.clone(), owner.clone(), owner.clone(), target.clone());
            PillActions {
                cancel: Rc::new(move |_, cx| VoiceInput::global(cx).update(cx, |voice, cx| voice.cancel(&cancel, cx))),
                finish: Rc::new(move |_, cx| VoiceInput::global(cx).update(cx, |voice, cx| voice.stop(&finish, cx))),
                skip: Rc::new(move |_, cx| VoiceInput::global(cx).update(cx, |voice, _| voice.skip_post_processing(&skip))),
                retry: Rc::new(move |_, cx| {
                    let target = retry.clone();
                    VoiceInput::global(cx).update(cx, |voice, cx| voice.start(target, cx))
                }),
            }
        };
        let open = target.open.clone();
        let label = Button::new("voice-indicator-draft")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::SmPill)
            .label(target.label.clone().unwrap_or_else(|| t!("Untitled thread")))
            .disabled(open.is_none())
            .on_click(move |_, window, cx| {
                if let Some(open) = &open {
                    open(window, cx);
                }
            });
        // `fixed inset-x-0 top-3 flex justify-center px-4`, a `bg-popover` capsule with `p-0.5`.
        div()
            .absolute()
            .top(c.r(px(12.)))
            .left_0()
            .right_0()
            .px(c.r(px(16.)))
            .flex()
            .justify_center()
            .child(
                div()
                    .id("voice-indicator")
                    .occlude()
                    .flex()
                    .max_w_full()
                    .items_center()
                    .gap(c.r(px(4.)))
                    .rounded_full()
                    .bg(theme.colors.popover())
                    .p(c.r(px(2.)))
                    .shadow(theme.shadows.lg.clone())
                    .child(div().min_w_0().flex_shrink(1.).child(label))
                    .child(recording_pill(&state, started, &levels, None, false, actions, &c))
                    .test_support(),
            )
            .into_any_element()
    }
}
