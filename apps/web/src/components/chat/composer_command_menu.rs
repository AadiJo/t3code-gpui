//! The slash, `@`, `$` and `#` menu attached above the composer. Ports
//! `ComposerCommandMenu.tsx` and the item building of `useComposerMenuItems.ts`.
//! Items come from a [`ComposerMenuSource`]; the chat view supplies one backed
//! by workspace and provider searches, scenes supply fixtures.

use crate::components::icons::{Icon, IconName, IconSize};
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, Hsla, prelude::FluentBuilder as _, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::components::chat::composer_icons::{composer_icon_stroke, provider_icon};
use crate::components::chat::composer_theme::{COMMAND_MENU_MAX_HEIGHT, ComposerColors};
use crate::composer_logic::{ComposerTrigger, ComposerTriggerKind};
use crate::model_selection::{ProviderIcon, ProviderInstanceId};

/// A built-in slash command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlashCommand {
    Model,
    Side,
    Btw,
    Clear,
}

/// What picking an item does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerCommandAction {
    /// Replace the trigger with this text (a file link, a `$skill `, a `#123 `, a thread chip).
    Insert(SharedString),
    /// Run a built-in command.
    Slash(SlashCommand),
    /// Replace the trigger with a model mention and select that model.
    Model { instance_id: ProviderInstanceId, model: SharedString, insert: SharedString },
    /// Listed but refused: picking leaves the menu and the text as they are (`/side` while
    /// side chats are unavailable).
    Refused,
}

/// The leading icon of an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerCommandIcon {
    None,
    Icon(IconName),
    Provider(ProviderIcon),
    /// A pull request's state glyph in its tone (`resolvePullRequestState`).
    PullRequest { state: t3code_contracts::PullRequestState, is_draft: bool },
}

/// One menu row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerCommandItem {
    pub id: SharedString,
    pub icon: ComposerCommandIcon,
    pub label: SharedString,
    /// Drawn before the label in `secondaryLabel`, as `/skill:` for slash skills.
    pub label_prefix: Option<SharedString>,
    pub description: SharedString,
    /// A badge at the end, such as a skill's source.
    pub badge: Option<SharedString>,
    pub action: ComposerCommandAction,
}

/// What the slash list reads from the composer itself (`compactSlashCommandAvailable`).
pub struct SlashMenuContext<'a> {
    pub prompt: &'a str,
    /// The instance the composer sends with; its commands and skills are listed.
    pub instance_id: &'a ProviderInstanceId,
    pub has_attachments: bool,
}

/// Supplies menu items for a trigger. Returns the items and whether a search is still running.
pub trait ComposerMenuSource {
    fn items(&self, trigger: &ComposerTrigger) -> (Vec<ComposerCommandItem>, bool);

    /// The whole `/` list, built-ins included; `None` shows [`builtin_slash_items`] before `items`.
    fn slash_items(&self, _trigger: &ComposerTrigger, _context: &SlashMenuContext) -> Option<(Vec<ComposerCommandItem>, bool)> {
        None
    }

    /// The empty text when it depends on the source (`#` without a repository); `None` uses
    /// [`empty_state_text`].
    fn empty_text(&self, _trigger: &ComposerTrigger) -> Option<SharedString> {
        None
    }
}

/// The built-in slash commands, filtered by the query.
pub fn builtin_slash_items(query: &str) -> Vec<ComposerCommandItem> {
    let all = [
        (SlashCommand::Model, "/model", "Switch response model for this thread"),
        (SlashCommand::Side, "/side", "Ask a side question in a new side chat"),
        (SlashCommand::Btw, "/btw", "Same as /side"),
        (SlashCommand::Clear, "/clear", "Start a fresh session; earlier messages stay visible"),
    ];
    let query = query.to_lowercase();
    all.into_iter()
        .filter(|(_, label, _)| label[1..].starts_with(&query))
        .map(|(command, label, description)| ComposerCommandItem {
            id: format!("slash:{label}").into(),
            icon: ComposerCommandIcon::None,
            label: label.into(),
            label_prefix: None,
            description: description.into(),
            badge: None,
            action: ComposerCommandAction::Slash(command),
        })
        .collect()
}

/// The empty or loading text for a trigger.
pub fn empty_state_text(kind: ComposerTriggerKind, loading: bool) -> &'static str {
    match (kind, loading) {
        (ComposerTriggerKind::Skill | ComposerTriggerKind::SlashCommand, true) => "Searching workspace skills...",
        (ComposerTriggerKind::PullRequest, true) => "Finding pull request...",
        (_, true) => "Searching workspace files...",
        (ComposerTriggerKind::Skill, false) => "No skills found. Try / to browse provider commands.",
        (ComposerTriggerKind::Path, false) => "No matching files or folders.",
        _ => "No matching command.",
    }
}

/// The menu's current content.
#[derive(Clone, Debug)]
pub struct ComposerMenuState {
    pub trigger: ComposerTrigger,
    pub items: Arc<Vec<ComposerCommandItem>>,
    pub loading: bool,
    pub active: usize,
    /// The source's empty text, when it has its own.
    pub empty: Option<SharedString>,
}

impl ComposerMenuState {
    /// Moves the highlight, wrapping at the ends.
    pub fn step(&mut self, down: bool) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        self.active = if down { (self.active + 1) % n } else { (self.active + n - 1) % n };
    }

    pub fn active_item(&self) -> Option<&ComposerCommandItem> {
        self.items.get(self.active)
    }
}

/// Item row height: `min-h-7` with `py-1.5` text-xs content.
pub const MENU_ITEM_HEIGHT: gpui_kit::Pixels = px(28.);

/// Renders the menu list. `on_pick` and `on_hover` receive the item index.
pub fn render_menu(
    state: &ComposerMenuState,
    colors: &ComposerColors,
    on_pick: impl Fn(usize, &mut gpui_kit::Window, &mut App) + 'static,
    on_hover: impl Fn(usize, &mut gpui_kit::Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let on_pick = std::rc::Rc::new(on_pick);
    let on_hover = std::rc::Rc::new(on_hover);
    let c = *colors;
    if state.items.is_empty() {
        return div().px(px(20.)).pt(px(14.)).pb(px(28.)).child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(c.secondary_label)
                .child(match (&state.empty, state.loading) {
                    (Some(text), false) => text.clone(),
                    _ => SharedString::from(empty_state_text(state.trigger.kind, state.loading)),
                }),
        ).into_any_element();
    }
    let rows: Vec<AnyElement> = state
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let active = index == state.active;
            let pick = on_pick.clone();
            let hover = on_hover.clone();
            let leading: Option<AnyElement> = match &item.icon {
                ComposerCommandIcon::None => None,
                ComposerCommandIcon::Icon(i) => Some(Icon::new(*i).size(IconSize::Custom(px(16.))).color(c.secondary_label).stroke(composer_icon_stroke(*i)).into_any_element()),
                ComposerCommandIcon::Provider(p) => Some(provider_icon(*p, px(16.), &item.label, &c, cx)),
                ComposerCommandIcon::PullRequest { state, is_draft } => {
                    let (icon, tone, _) = crate::components::pull_request::pull_request_presentation::pull_request_state_presentation(crate::theme_palette::ActiveAppTheme::app_theme(cx), *state, *is_draft);
                    Some(Icon::new(icon).size(IconSize::Custom(px(16.))).color(tone).into_any_element())
                }
            };
            let text_color: Hsla = if active { c.accent_foreground } else { c.foreground };
            div()
                .id(SharedString::from(format!("composer-menu-item-{index}")))
                .flex()
                .items_center()
                .gap(px(8.))
                .min_h(MENU_ITEM_HEIGHT)
                .px(px(8.))
                .py(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(active, |el| el.bg(c.accent))
                .text_color(text_color)
                .on_mouse_move(move |_, window, cx| hover(index, window, cx))
                .on_mouse_down(gpui_kit::MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(move |_, window, cx| pick(index, window, cx))
                .children(leading)
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .max_w(gpui_kit::relative(0.45))
                                .overflow_hidden()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .children(item.label_prefix.clone().map(|p| div().text_color(c.secondary_label).child(p)))
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(c.secondary_label)
                                .child(item.description.clone()),
                        )
                        .children(item.badge.clone().map(|badge| {
                            div()
                                .flex_none()
                                .px(px(6.))
                                .h(px(18.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .bg(c.kbd_background)
                                .text_size(px(11.))
                                .text_color(c.secondary_label)
                                .child(badge)
                        })),
                )
                .into_any_element()
        })
        .collect();
    div()
        .id("composer-command-menu-list")
        .max_h(COMMAND_MENU_MAX_HEIGHT)
        .overflow_y_scroll()
        .p(px(8.))
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

