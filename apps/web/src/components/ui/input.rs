//! Text input and textarea. Ports `input.tsx` and `textarea.tsx`: the
//! control frame is ours; text editing is gpui-component's editor drawn
//! without its own chrome.

use gpui_kit::{
    AnyElement, App, Entity, Focusable as _, Hsla, IntoElement, Pixels, RenderOnce, Styled, Window,
    component::input::{Input, InputState, Textarea, TextareaState},
    div, prelude::*, px,
};

use super::{ControlState, DISABLED_OPACITY, TextStyleExt as _, ControlHighlightExt as _, input_focus_ring};
use crate::appearance_fonts::{TextSize, weight};
use crate::theme_color::with_alpha;
use crate::{ActiveAppTheme as _, AppTheme};

/// `InputProps.size`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputSize {
    Sm,
    #[default]
    Default,
    Lg,
    Compact,
}

impl InputSize {
    pub const ALL: [InputSize; 4] = [Self::Sm, Self::Default, Self::Lg, Self::Compact];

    pub fn label(self) -> &'static str {
        match self {
            Self::Sm => "sm",
            Self::Default => "default",
            Self::Lg => "lg",
            Self::Compact => "compact",
        }
    }
}

/// How the frame draws: idle, focused, invalid, disabled.
struct FramePaint {
    border: Hsla,
    shadows: Vec<gpui_kit::BoxShadow>,
    highlight: bool,
}

fn frame_paint(theme: &AppTheme, focused: bool, invalid: bool, disabled: bool) -> FramePaint {
    let c = &theme.colors;
    let destructive_ring = with_alpha(c.destructive(), if theme.is_dark() { 0.24 } else { 0.16 });
    match (focused, invalid) {
        (true, true) => FramePaint {
            border: with_alpha(c.destructive(), 0.64),
            shadows: vec![input_focus_ring(destructive_ring)],
            highlight: false,
        },
        (true, false) => FramePaint {
            border: c.ring(),
            shadows: vec![input_focus_ring(theme.derived.input_focus_ring)],
            highlight: false,
        },
        (false, true) => FramePaint { border: with_alpha(c.destructive(), 0.36), shadows: vec![], highlight: false },
        (false, false) => FramePaint {
            border: c.input,
            shadows: if disabled { vec![] } else { theme.shadows.xs5.clone() },
            highlight: !disabled,
        },
    }
}

/// The shared control frame: radius, 1px `border-input`, `input/32` fill.
fn frame(theme: &AppTheme, radius: Pixels, paint: FramePaint, disabled: bool, child: AnyElement) -> gpui_kit::Div {
    div()
        .relative()
        .flex()
        .w_full()
        .rounded(radius)
        .border_1()
        .border_color(paint.border)
        .bg(theme.derived.input_fill)
        .shadow(paint.shadows)
        .text_color(theme.colors.foreground())
        .when(paint.highlight, |f| f.control_highlight(theme))
        .child(child)
        .when(disabled, |f| f.opacity(DISABLED_OPACITY))
}

fn is_focused(focused: bool, state: Option<ControlState>) -> bool {
    match state {
        Some(state) => state == ControlState::FocusVisible,
        None => focused,
    }
}

/// A single-line text field. Create the state with `InputState::new(window, cx)`.
#[derive(IntoElement)]
pub struct TextInput {
    state: Entity<InputState>,
    size: InputSize,
    mono: bool,
    invalid: bool,
    disabled: bool,
    forced: Option<ControlState>,
}

impl TextInput {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self { state: state.clone(), size: InputSize::Default, mono: false, invalid: false, disabled: false, forced: None }
    }

    pub fn size(mut self, size: InputSize) -> Self {
        self.size = size;
        self
    }

    /// `font="mono"`: mono with tabular figures.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// `aria-invalid`.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Draws a state without focus or pointer (gallery).
    pub fn state(mut self, state: ControlState) -> Self {
        self.forced = Some(state);
        self
    }
}

impl RenderOnce for TextInput {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focused = self.state.read(cx).focus_handle(cx).is_focused(window);
        let theme = cx.app_theme();
        let sp = |n: f32| theme.sp(n);
        let disabled = self.disabled || self.forced == Some(ControlState::Disabled);
        let (height, padding, radius, text) = match self.size {
            InputSize::Sm => (sp(6.5), sp(2.5) - px(1.), theme.radii.lg, TextSize::Sm),
            InputSize::Default => (sp(7.5), sp(3.) - px(1.), theme.radii.lg, TextSize::Sm),
            InputSize::Lg => (sp(8.5), sp(3.) - px(1.), theme.radii.lg, TextSize::Sm),
            InputSize::Compact => (sp(7.), sp(2.5) - px(1.), theme.radii.md, TextSize::Xs),
        };
        let paint = frame_paint(theme, is_focused(focused, self.forced), self.invalid, disabled);
        let field = Input::new(&self.state).appearance(false).disabled(disabled);
        // `Input::h` is an inherent method that only sizes multi-line inputs.
        let field = Styled::h(field, height).px(padding).w_full();
        let field = if self.mono {
            field.mono_text(px(TextSize::Sm.size(theme.fonts.interface)), weight::NORMAL)
        } else {
            field.ui_text(theme, text, weight::NORMAL)
        };
        frame(theme, radius, paint, disabled, field.into_any_element())
    }
}

/// A multi-line field that grows with its content from 70px to 256px.
/// Create the state with `TextareaState::new(window, cx).auto_grow(1, 12)`;
/// the 70px minimum comes from this frame, not the row count.
#[derive(IntoElement)]
pub struct TextArea {
    state: Entity<TextareaState>,
    invalid: bool,
    disabled: bool,
    forced: Option<ControlState>,
}

impl TextArea {
    pub fn new(state: &Entity<TextareaState>) -> Self {
        Self { state: state.clone(), invalid: false, disabled: false, forced: None }
    }

    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn state(mut self, state: ControlState) -> Self {
        self.forced = Some(state);
        self
    }
}

impl RenderOnce for TextArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focused = self.state.read(cx).focus_handle(cx).is_focused(window);
        let theme = cx.app_theme();
        let disabled = self.disabled || self.forced == Some(ControlState::Disabled);
        let paint = frame_paint(theme, is_focused(focused, self.forced), self.invalid, disabled);
        let field = Textarea::new(&self.state)
            .appearance(false)
            .disabled(disabled)
            .w_full()
            .min_h(theme.sp(17.5))
            .max_h(theme.sp(64.))
            .px(theme.sp(3.) - px(1.))
            .py(theme.sp(1.5) - px(1.))
            .ui_text(theme, TextSize::Sm, weight::NORMAL);
        frame(theme, theme.radii.lg, paint, disabled, field.into_any_element())
    }
}
