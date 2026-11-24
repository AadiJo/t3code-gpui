//! Keybindings. Ports `KeybindingsSettings.tsx` and its `.logic.ts`: one
//! row per rule of the environment's keymap, grouped like the web page,
//! with the binding, conflicts, recording a new shortcut from the keyboard,
//! unassign, reset and remove, and search. Changes go to every connected
//! environment through `server.upsertKeybinding` / `server.removeKeybinding`.
//!
//! The `when` control is a menu of "Always" and the conditions the keymap
//! already uses; the web opens a visual condition builder there. "Add
//! keybinding" adds a rule for any command with a recorded shortcut.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, Render, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use t3code_contracts::{ServerRemoveKeybindingInput, ServerUpsertKeybindingInput};
use t3code_shared::keybindings::{
    KeybindingCommand, ResolvedKeybindingRule, ResolvedKeybindingsConfig, ShortcutPlatform, default_resolved_keybindings,
    parse_keybinding_shortcut, shortcut_to_keybinding_input,
};

use super::settings_layout::{SettingsSection, page_container};
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::kbd::Kbd;
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::t;
use crate::shortcuts::commands::command_title;
use crate::shortcuts::keymap::ShortcutState;
use crate::theme_color::with_alpha;

/// Where a rule comes from (`KeybindingSource`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeybindingSource {
    Default,
    Custom,
    Project,
}

/// One row (`KeybindingRow`).
#[derive(Clone, Debug)]
pub struct KeybindingRow {
    pub id: String,
    pub command: KeybindingCommand,
    pub key: String,
    pub when: String,
    pub source: KeybindingSource,
    pub is_unassigned: bool,
    pub default_key: Option<String>,
    pub default_when: String,
    pub conflicts: Vec<SharedString>,
    pub rule: ResolvedKeybindingRule,
}

fn when_of(rule: &ResolvedKeybindingRule) -> String {
    rule.when_ast.as_ref().map(|w| w.to_expression()).unwrap_or_default()
}

/// `commandLabel`: the catalog's title, so the row and its search hit agree.
pub fn command_label(command: &KeybindingCommand) -> SharedString {
    let id = format!("keybinding-{command}");
    let title = setting_title(&id);
    if title.as_ref() == id { command_title(command) } else { title }
}

fn default_for(rule: &ResolvedKeybindingRule) -> Option<&'static ResolvedKeybindingRule> {
    let defaults = default_resolved_keybindings().rules();
    let key = shortcut_to_keybinding_input(&rule.shortcut);
    let when = when_of(rule);
    let same_command = |entry: &&ResolvedKeybindingRule| entry.command == rule.command;
    defaults
        .iter()
        .filter(same_command)
        .find(|entry| shortcut_to_keybinding_input(&entry.shortcut) == key && when_of(entry) == when)
        .or_else(|| (when == "false").then(|| defaults.iter().filter(same_command).find(|e| shortcut_to_keybinding_input(&e.shortcut) == key)).flatten())
        .or_else(|| defaults.iter().filter(same_command).find(|e| when_of(e) == when))
        .or_else(|| defaults.iter().find(|e| e.command == rule.command))
}

fn source_of(rule: &ResolvedKeybindingRule) -> KeybindingSource {
    if matches!(rule.command, KeybindingCommand::Script(_)) {
        return KeybindingSource::Project;
    }
    let key = shortcut_to_keybinding_input(&rule.shortcut);
    let when = when_of(rule);
    let is_default = default_resolved_keybindings()
        .rules()
        .iter()
        .any(|e| e.command == rule.command && shortcut_to_keybinding_input(&e.shortcut) == key && when_of(e) == when);
    if is_default { KeybindingSource::Default } else { KeybindingSource::Custom }
}

/// The chord a shortcut resolves to on a platform, for conflicts (`mod+k` = `ctrl+k` off macOS).
fn conflict_key(rule: &ResolvedKeybindingRule, platform: ShortcutPlatform) -> String {
    let chord = rule.shortcut.chord(platform);
    format!("{}{}{}{}+{}", chord.meta as u8, chord.ctrl as u8, chord.alt as u8, chord.shift as u8, rule.shortcut.key)
}

fn conflicts_with_when(left: &str, right: &str) -> bool {
    if left == "false" || right == "false" {
        return false;
    }
    left.is_empty() || right.is_empty() || left == right
}

/// `buildKeybindingRows`.
pub fn build_rows(config: &ResolvedKeybindingsConfig, platform: ShortcutPlatform) -> Vec<KeybindingRow> {
    let mut rows: Vec<KeybindingRow> = Vec::new();
    for rule in config.rules() {
        let key = shortcut_to_keybinding_input(&rule.shortcut);
        let when = when_of(rule);
        let id = format!("{}\0{key}\0{when}", rule.command);
        let default = default_for(rule);
        let row = KeybindingRow {
            id: id.clone(),
            command: rule.command.clone(),
            is_unassigned: when == "false" && default.is_some_and(|d| shortcut_to_keybinding_input(&d.shortcut) == key),
            default_key: default.map(|d| shortcut_to_keybinding_input(&d.shortcut)),
            default_when: default.map(when_of).unwrap_or_default(),
            source: source_of(rule),
            key,
            when,
            conflicts: Vec::new(),
            rule: rule.clone(),
        };
        match rows.iter().position(|r| r.id == id) {
            Some(index) => rows[index] = row,
            None => rows.push(row),
        }
    }
    let snapshot: Vec<(String, String, String, KeybindingCommand)> =
        rows.iter().map(|r| (r.id.clone(), conflict_key(&r.rule, platform), r.when.clone(), r.command.clone())).collect();
    for row in &mut rows {
        let mine = conflict_key(&row.rule, platform);
        let mut labels: Vec<SharedString> = snapshot
            .iter()
            .filter(|(id, key, when, _)| *id != row.id && *key == mine && conflicts_with_when(when, &row.when))
            .map(|(_, _, _, command)| command_label(command))
            .collect();
        labels.sort();
        labels.dedup();
        row.conflicts = labels;
    }
    rows.sort_by(|a, b| command_label(&a.command).cmp(&command_label(&b.command)).then(a.key.cmp(&b.key)));
    rows
}

/// `KEYBINDING_GROUPS`, by the command's first segment.
fn group_of(command: &KeybindingCommand) -> (&'static str, SharedString) {
    let text = command.to_string();
    let prefix = text.split('.').next().unwrap_or_default();
    match prefix {
        "sidebar" | "rightPanel" | "commandPalette" | "filePicker" | "projectSearch" | "editor" => ("navigation", t!("Navigation")),
        "thread" | "chat" | "pullRequest" => ("threads", t!("Threads")),
        "composer" | "modelPicker" => ("composer", t!("Composer")),
        "terminal" => ("terminal", t!("Terminal")),
        "preview" | "diff" => ("preview", t!("Preview & diff")),
        "theme" | "appearance" | "themeEditor" => ("appearance", t!("Appearance")),
        "script" => ("scripts", t!("Project scripts")),
        _ => ("other", t!("Other", context = "keybinding group")),
    }
}

const GROUP_ORDER: [&str; 8] = ["navigation", "threads", "composer", "terminal", "preview", "appearance", "scripts", "other"];

/// `keybindingFromKeyboardEvent` for a GPUI keystroke: `None` for a bare modifier.
pub fn keybinding_from_keystroke(keystroke: &gpui_kit::Keystroke, platform: ShortcutPlatform) -> Option<String> {
    let key = match keystroke.key.as_str() {
        "up" => "arrowup".to_owned(),
        "down" => "arrowdown".to_owned(),
        "left" => "arrowleft".to_owned(),
        "right" => "arrowright".to_owned(),
        "escape" => "esc".to_owned(),
        "space" => "space".to_owned(),
        "enter" | "tab" | "backspace" | "delete" | "home" | "end" | "pageup" | "pagedown" => keystroke.key.clone(),
        key if key.chars().count() == 1 => key.to_lowercase(),
        key if key.starts_with('f') && key[1..].parse::<u8>().is_ok() => key.to_owned(),
        _ => return None,
    };
    let m = &keystroke.modifiers;
    let mut parts = Vec::new();
    match platform {
        ShortcutPlatform::Mac => {
            if m.platform {
                parts.push("mod");
            }
            if m.control {
                parts.push("ctrl");
            }
        }
        ShortcutPlatform::Other => {
            if m.control {
                parts.push("mod");
            }
            if m.platform {
                parts.push("meta");
            }
        }
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift {
        parts.push("shift");
    }
    parts.push(&key);
    Some(parts.join("+"))
}

fn remove_target(row: &KeybindingRow) -> ServerRemoveKeybindingInput {
    ServerRemoveKeybindingInput {
        key: row.key.clone(),
        command: contract_command(&row.command),
        when: (!row.when.trim().is_empty()).then(|| row.when.clone()),
    }
}

fn contract_command(command: &KeybindingCommand) -> t3code_contracts::KeybindingCommand {
    serde_json::from_value(serde_json::Value::String(command.to_string())).expect("commands round trip")
}

/// The recording id of the "New keybinding" row.
const NEW_ROW: &str = "\0new";

/// The rule "Add keybinding" is drafting.
#[derive(Clone, Debug, Default)]
struct NewBinding {
    command: usize,
    key: Option<String>,
}

/// Every command a rule can bind, in label order.
fn bindable_commands() -> Vec<KeybindingCommand> {
    let mut commands: Vec<KeybindingCommand> = Vec::new();
    for rule in default_resolved_keybindings().rules() {
        if !commands.contains(&rule.command) {
            commands.push(rule.command.clone());
        }
    }
    commands.sort_by_key(command_label);
    commands
}

/// The Keybindings page.
pub struct KeybindingsSettings {
    search: Entity<InputState>,
    query: SharedString,
    /// The row recording a shortcut, by id; [`NEW_ROW`] for the new rule.
    recording: Option<String>,
    /// The "New keybinding" row while it is open.
    adding: Option<NewBinding>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl KeybindingsSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search keybindings")));
        let subscription = cx.subscribe(&search, |this, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = state.read(cx).value();
                cx.notify();
            }
        });
        Self {
            search,
            query: SharedString::default(),
            recording: None,
            adding: None,
            focus: cx.focus_handle(),
            _subscriptions: vec![subscription, cx.observe_global::<ShortcutState>(|_, cx| cx.notify())],
        }
    }

    fn platform(cx: &App) -> ShortcutPlatform {
        cx.try_global::<ShortcutState>().map_or(ShortcutPlatform::current(), |state| state.platform)
    }

    /// The rules shown: the keymap environment's, or the installed keymap.
    fn config(cx: &App) -> (ResolvedKeybindingsConfig, bool) {
        let from_env = ServerSettingsStore::keymap_environment(cx).and_then(|id| ServerSettingsStore::config(cx, &id)).and_then(|config| {
            serde_json::to_value(&config.keybindings).ok().and_then(|v| serde_json::from_value::<ResolvedKeybindingsConfig>(v).ok())
        });
        match from_env {
            Some(config) => (config, true),
            None => (cx.try_global::<ShortcutState>().map(|s| (*s.config).clone()).unwrap_or_default(), false),
        }
    }

    fn rows(&self, cx: &App) -> Vec<KeybindingRow> {
        let (config, _) = Self::config(cx);
        let query = self.query.trim().to_lowercase();
        build_rows(&config, Self::platform(cx))
            .into_iter()
            .filter(|row| {
                query.is_empty()
                    || row.command.to_string().to_lowercase().contains(&query)
                    || command_label(&row.command).to_lowercase().contains(&query)
                    || row.key.to_lowercase().contains(&query)
                    || row.when.to_lowercase().contains(&query)
                    || format!("{:?}", row.source).to_lowercase().contains(&query)
            })
            .collect()
    }

    /// Starts recording on the row for `command`, as a click on its shortcut does.
    pub fn start_recording_for_scene(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (config, _) = Self::config(cx);
        if let Some(row) = build_rows(&config, Self::platform(cx)).into_iter().find(|row| row.command.to_string() == command) {
            self.start_recording(row.id, window, cx);
        }
    }

    fn start_recording(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = Some(id);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Saves a recorded shortcut on the recording row (`save`), replacing its old rule.
    pub fn record(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(id) = self.recording.take() else { return };
        if id == NEW_ROW {
            if let Some(adding) = &mut self.adding {
                adding.key = Some(key);
            }
            cx.notify();
            return;
        }
        let (config, _) = Self::config(cx);
        let Some(row) = build_rows(&config, Self::platform(cx)).into_iter().find(|row| row.id == id) else { return };
        // Giving an unassigned row a shortcut brings back its default condition.
        let when = if row.is_unassigned { row.default_when.clone() } else { row.when.clone() };
        save(
            ServerUpsertKeybindingInput {
                key,
                command: contract_command(&row.command),
                when: (!when.trim().is_empty()).then_some(when),
                replace: Some(remove_target(&row)),
            },
            cx,
        );
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_none() {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
        if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
            self.recording = None;
            cx.notify();
            return;
        }
        if let Some(key) = keybinding_from_keystroke(&event.keystroke, Self::platform(cx))
            && parse_keybinding_shortcut(&key).is_some()
        {
            self.record(key, cx);
        }
    }

    fn render_row(&self, row: KeybindingRow, can_edit: bool, conditions: &[String], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let platform = Self::platform(cx);
        let recording = self.recording.as_deref() == Some(row.id.as_str());
        let id = row.id.clone();
        let key_control = if recording {
            recording_field(cx).into_any_element()
        } else {
            div()
                .id(SharedString::from(format!("kb-pill-{}", row.id)))
                .flex()
                .items_center()
                .h(theme.sp(7.))
                .px(theme.sp(1.5))
                .mr(-theme.sp(1.5))
                .rounded(theme.radii.md)
                .border_1()
                .border_color(gpui_kit::transparent_black())
                .cursor_pointer()
                .hover(|s| s.border_color(with_alpha(c.border, 0.7)).bg(c.accent_surface))
                .when(can_edit, |d| d.on_click(cx.listener(move |this, _, window, cx| this.start_recording(id.clone(), window, cx))))
                .child(if row.is_unassigned {
                    div()
                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(t!("Unassigned", context = "keyboard shortcut with no key assigned"))
                        .into_any_element()
                } else {
                    keycaps(&row.key, platform).into_any_element()
                })
                .into_any_element()
        };
        let menu_row = row.clone();
        let actions = dropdown_menu(
            SharedString::from(format!("kb-actions-{}", row.id)),
            Button::new(SharedString::from(format!("kb-actions-trigger-{}", row.id))).variant(ButtonVariant::GhostMuted).size(ButtonSize::IconSm).icon(IconName::Ellipsis),
            move |_, _| {
                let row = menu_row.clone();
                let mut menu = MenuPopup::new();
                // `canReset`, `canUnassign`, `canRemove`.
                if row.source == KeybindingSource::Custom && row.default_key.is_some() {
                    let reset = row.clone();
                    menu = menu.item(MenuItem::new(t!("Reset to default")).on_select(move |_, cx| reset_row(&reset, cx)));
                }
                if row.source == KeybindingSource::Default {
                    let unassign = row.clone();
                    menu = menu.item(
                        MenuItem::new(t!("Unassign", context = "verb, removes the key from a keyboard shortcut"))
                            .destructive()
                            .on_select(move |_, cx| unassign_row(&unassign, cx)),
                    );
                }
                if row.source != KeybindingSource::Default && !row.is_unassigned {
                    let remove = row.clone();
                    menu = menu.item(
                        MenuItem::new(t!("Remove", context = "verb, removes a keybinding")).destructive().on_select(move |_, cx| remove_row(&remove, cx)),
                    );
                }
                menu
            },
        );
        let conflicts = match row.conflicts.len() {
            0 => None,
            1 => Some(t!("Conflicts with {command}.", command = row.conflicts[0].to_string())),
            n => {
                let shown: Vec<String> = row.conflicts.iter().take(3).map(ToString::to_string).collect();
                Some(if n > 3 {
                    t!("Conflicts with {commands}, and more.", commands = shown.join(", "))
                } else {
                    t!("Conflicts with {commands}.", commands = shown.join(", "))
                })
            }
        };
        let warning = conflicts.map(|text| {
            div()
                .id(SharedString::from(format!("kb-conflict-{}", row.id)))
                .tooltip(tooltip(text))
                .child(crate::components::icons::Icon::new(IconName::TriangleAlert).size(crate::components::icons::IconSize::Custom(px(18.))).color(c.warning))
        });
        let description = if row.is_unassigned {
            div()
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(t!("No keyboard shortcut assigned. Click Unassigned to set one."))
                .into_any_element()
        } else {
            when_control(&row, can_edit, conditions, cx).into_any_element()
        };
        let mut title = div().flex().items_center().gap(theme.sp(2.)).child(command_label(&row.command));
        if row.source != KeybindingSource::Default {
            title = title.child(crate::components::ui::badge::Badge::new(if row.source == KeybindingSource::Custom {
                t!("Custom", context = "keybinding source badge")
            } else {
                t!("Project", context = "keybinding source badge")
            }).variant(crate::components::ui::badge::BadgeVariant::Outline).size(crate::components::ui::badge::BadgeSize::Sm));
        }
        // `SettingsRow` with the condition as its description and the
        // conflict warning, the hover-only actions and the shortcut as its control.
        div()
            .id(SharedString::from(format!("kb-row-{}", row.id)))
            .group("kb-row")
            .px(theme.sp(4.))
            .py(theme.sp(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(8.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(theme.sp(1.))
                            .child(
                                div()
                                    .id(SharedString::from(format!("kb-title-{}", row.id)))
                                    .flex()
                                    .min_h(theme.sp(5.))
                                    .items_center()
                                    .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                                    .text_color(c.foreground())
                                    .tooltip(tooltip(SharedString::from(row.command.to_string())))
                                    .child(title),
                            )
                            .child(description),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_end()
                            .gap(theme.sp(1.5))
                            .children(warning)
                            .when(can_edit, |d| d.child(div().opacity(0.).group_hover("kb-row", |s| s.opacity(1.)).child(actions)))
                            .child(key_control),
                    ),
            )
            .into_any_element()
    }
}

/// `KeybindingPill`: one 20px keycap per part of the shortcut, 4px apart.
fn keycaps(key: &str, platform: ShortcutPlatform) -> impl IntoElement {
    let mac = platform == ShortcutPlatform::Mac;
    div().flex().items_center().gap(px(4.)).children(key.split('+').map(|part| {
        let label: SharedString = match part {
            "mod" if mac => "⌘".into(),
            "mod" => "Ctrl".into(),
            "shift" => "⇧".into(),
            "alt" if mac => "⌥".into(),
            "alt" => "Alt".into(),
            "ctrl" => "⌃".into(),
            part if part.chars().count() == 1 => part.to_uppercase().into(),
            part => SharedString::from(part.to_owned()),
        };
        Kbd::new(label)
    }))
}

/// The recording field: `Input size="sm" font="mono"` 11rem wide, focused,
/// showing its "Press shortcut" placeholder.
fn recording_field(cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    let c = &theme.colors;
    div()
        .flex()
        .items_center()
        .w(theme.rem(11.))
        .h(theme.sp(7.))
        .px(theme.sp(2.5) - px(1.))
        .rounded(theme.radii.lg)
        .border_1()
        .border_color(c.ring())
        .bg(theme.derived.input_fill)
        .shadow(vec![crate::components::ui::input_focus_ring(theme.derived.input_focus_ring)])
        .mono_text(px(TextSize::Sm.size(theme.fonts.interface)), weight::NORMAL)
        .text_color(c.placeholder)
        .child(t!("Press shortcut"))
}

/// `KeybindingRowWhen`: "When" and the condition button (mono, foreground
/// for a condition, muted "Always" for none). Its menu picks "Always" or a
/// condition the keymap already uses.
fn when_control(row: &KeybindingRow, can_edit: bool, conditions: &[String], cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    let c = &theme.colors;
    let expression = row.when.clone();
    let label: SharedString = if expression.is_empty() { t!("Always") } else { expression.clone().into() };
    let color = if expression.is_empty() { c.muted_foreground } else { c.foreground() };
    let trigger = div()
        .id(SharedString::from(format!("kb-when-trigger-{}", row.id)))
        .flex()
        .min_w_0()
        .items_center()
        .gap(theme.sp(1.))
        .h(theme.sp(5.))
        .px(px(5.))
        .rounded(theme.radii.sm)
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .hover(|s| s.bg(c.accent_surface))
        .cursor_pointer()
        .text_color(color)
        .child(div().truncate().mono_text(px(11.), weight::MEDIUM).line_height(px(16.5)).child(label))
        .child(crate::components::icons::Icon::new(IconName::ChevronDown).size(crate::components::icons::IconSize::Sm).color(with_alpha(color, 0.6)));
    let menu_row = row.clone();
    let conditions: Vec<String> = conditions.to_vec();
    let control = if can_edit {
        dropdown_menu(SharedString::from(format!("kb-when-{}", row.id)), trigger, move |_, _| {
            let options = std::iter::once(String::new()).chain(conditions.iter().cloned());
            options.fold(MenuPopup::new(), |menu, option| {
                let row = menu_row.clone();
                let label: SharedString = if option.is_empty() { t!("Always") } else { option.clone().into() };
                menu.item(MenuItem::new(label).kind(MenuItemKind::Radio(option == row.when)).on_select(move |_, cx| {
                    if option != row.when {
                        set_when(&row, &option, cx);
                    }
                }))
            })
        })
        .into_any_element()
    } else {
        trigger.into_any_element()
    };
    div()
        .flex()
        .h(theme.sp(6.))
        .items_center()
        .gap(theme.sp(1.5))
        .child(
            div()
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .line_height(px(TextSize::Xs.size(theme.fonts.interface)))
                .text_color(with_alpha(c.muted_foreground, 0.7))
                .child(t!("When", context = "heading for keybinding conditions")),
        )
        .child(control)
}

/// Saves a row's shortcut under a new condition, replacing its rule.
fn set_when(row: &KeybindingRow, when: &str, cx: &mut App) {
    save(
        ServerUpsertKeybindingInput {
            key: row.key.clone(),
            command: contract_command(&row.command),
            when: (!when.trim().is_empty()).then(|| when.to_owned()),
            replace: Some(remove_target(row)),
        },
        cx,
    );
}

/// Writes to every environment with a config, as the web writes to every connected one.
fn save(input: ServerUpsertKeybindingInput, cx: &mut App) {
    let targets = ServerSettingsStore::environment_ids(cx);
    let task = ServerSettingsStore::upsert_keybinding(targets, input, cx);
    report(task, t!("Unable to save keybinding"), t!("The keybinding was not saved."), cx);
}

fn reset_row(row: &KeybindingRow, cx: &mut App) {
    let Some(default_key) = row.default_key.clone() else { return };
    save(
        ServerUpsertKeybindingInput {
            key: default_key,
            command: contract_command(&row.command),
            when: (!row.default_when.trim().is_empty()).then(|| row.default_when.clone()),
            replace: Some(remove_target(row)),
        },
        cx,
    );
}

fn unassign_row(row: &KeybindingRow, cx: &mut App) {
    save(
        ServerUpsertKeybindingInput {
            key: row.key.clone(),
            command: contract_command(&row.command),
            when: Some("false".into()),
            replace: Some(remove_target(row)),
        },
        cx,
    );
}

fn remove_row(row: &KeybindingRow, cx: &mut App) {
    let targets = ServerSettingsStore::environment_ids(cx);
    let task = ServerSettingsStore::remove_keybinding(targets, remove_target(row), cx);
    report(task, t!("Unable to remove keybinding"), t!("The keybinding was not removed."), cx);
}

fn report(task: gpui_kit::Task<Result<(), crate::connection::CommandError>>, title: SharedString, fallback: SharedString, cx: &mut App) {
    cx.spawn(async move |cx| {
        if let Err(error) = task.await {
            cx.update(|cx| {
                let description = if error.0.is_empty() { fallback } else { error.0 };
                Toaster::push(ToastData::new(ToastKind::Error, title).description(description), cx);
            });
        }
    })
    .detach();
}

impl KeybindingsSettings {
    /// `NewKeybindingSettingsRow`: the command, the recorded shortcut, Save
    /// and a cancel button, on a `bg-muted/15` row.
    fn render_new_row(&self, adding: &NewBinding, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let platform = Self::platform(cx);
        let commands = bindable_commands();
        let labels: std::rc::Rc<[SharedString]> = commands.iter().map(command_label).collect();
        let this = cx.entity();
        let pick = this.clone();
        let select = crate::components::ui::select::Select::new("kb-new-command", labels, adding.command, move |index, _, cx| {
            pick.update(cx, |this, cx| {
                if let Some(adding) = &mut this.adding {
                    adding.command = index;
                }
                cx.notify();
            })
        })
        .size(crate::components::ui::select::SelectSize::Sm);
        let recording = self.recording.as_deref() == Some(NEW_ROW);
        let key_field = if recording {
            recording_field(cx).into_any_element()
        } else {
            div()
                .id("kb-new-key")
                .flex()
                .items_center()
                .w(theme.rem(11.))
                .h(theme.sp(7.))
                .px(theme.sp(2.5) - px(1.))
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(c.input)
                .bg(theme.derived.input_fill)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.start_recording(NEW_ROW.to_owned(), window, cx)))
                .child(match &adding.key {
                    Some(key) => keycaps(key, platform).into_any_element(),
                    None => div().mono_text(px(TextSize::Sm.size(theme.fonts.interface)), weight::NORMAL).text_color(c.placeholder).child(t!("Unassigned")).into_any_element(),
                })
                .into_any_element()
        };
        let save_binding = adding.clone();
        let command = commands.get(save_binding.command).cloned();
        let can_save = save_binding.key.is_some() && command.is_some();
        div()
            .bg(with_alpha(c.muted, 0.15))
            .px(theme.sp(4.))
            .py(theme.sp(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(8.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(theme.sp(1.))
                            .child(div().min_h(theme.sp(5.)).ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(t!("New keybinding")))
                            .child(
                                div()
                                    .flex()
                                    .h(theme.sp(6.))
                                    .items_center()
                                    .gap(theme.sp(1.5))
                                    .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(with_alpha(c.muted_foreground, 0.7)).child(t!("When", context = "heading for keybinding conditions")))
                                    .child(div().px(px(5.)).mono_text(px(11.), weight::MEDIUM).text_color(c.muted_foreground).child(t!("Always"))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(theme.sp(2.))
                            .child(div().w(theme.rem(14.)).child(select))
                            .child(key_field)
                            .child(Button::new("kb-new-save").size(ButtonSize::Sm).label(t!("Save", context = "verb")).disabled(!can_save).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let (Some(key), Some(command)) = (save_binding.key.clone(), command.clone()) else { return };
                                    save(ServerUpsertKeybindingInput { key, command: contract_command(&command), when: None, replace: None }, cx);
                                    this.adding = None;
                                    cx.notify();
                                },
                            )))
                            .child(
                                div().id("kb-new-cancel-tip").tooltip(tooltip(t!("Cancel"))).child(
                                    Button::new("kb-new-cancel").variant(ButtonVariant::GhostMuted).size(ButtonSize::IconSm).icon(IconName::X).on_click(cx.listener(|this, _, _, cx| {
                                        this.adding = None;
                                        if this.recording.as_deref() == Some(NEW_ROW) {
                                            this.recording = None;
                                        }
                                        cx.notify();
                                    })),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// Opens the keymap file of the environment the keymap comes from in its
/// preferred editor (`openInPreferredEditor`).
fn open_keybindings_file(cx: &mut App) {
    let Some(id) = ServerSettingsStore::keymap_environment(cx) else { return };
    let Some(config) = ServerSettingsStore::config(cx, &id) else { return };
    let Some(editor) = crate::editor_preferences::effective_editor(crate::editor_preferences::LastEditor::get(cx), &config.available_editors) else {
        return;
    };
    let input = t3code_contracts::LaunchEditorInput {
        cwd: config.keybindings_config_path.clone(),
        editor,
        position: None,
        project_root: None,
        reveal: None,
    };
    let task = ServerSettingsStore::call::<t3code_contracts::rpc::methods::ShellOpenInEditor>(&id, input, cx);
    cx.spawn(async move |cx| {
        if let Err(error) = task.await {
            cx.update(|cx| {
                let description = if error.0.is_empty() { t!("The keybindings file was not opened.") } else { error.0 };
                Toaster::push(ToastData::new(ToastKind::Error, t!("Unable to open keybindings file")).description(description), cx);
            });
        }
    })
    .detach();
}

impl Render for KeybindingsSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (config, from_env) = Self::config(cx);
        let mut conditions: Vec<String> = config.rules().iter().map(when_of).filter(|w| !w.is_empty() && w != "false").collect();
        conditions.sort();
        conditions.dedup();
        let rows = self.rows(cx);
        let mut groups: Vec<(&'static str, SharedString, Vec<KeybindingRow>)> = Vec::new();
        for row in rows {
            let (id, title) = group_of(&row.command);
            match groups.iter_mut().find(|g| g.0 == id) {
                Some(group) => group.2.push(row),
                None => groups.push((id, title, vec![row])),
            }
        }
        groups.sort_by_key(|g| GROUP_ORDER.iter().position(|id| *id == g.0));
        let empty = groups.is_empty();
        let sections: Vec<AnyElement> = groups
            .into_iter()
            .map(|(_, title, rows)| {
                let rows: Vec<AnyElement> = rows.into_iter().map(|row| self.render_row(row, from_env, &conditions, cx)).collect();
                SettingsSection::new(title).children(rows).into_any_element()
            })
            .collect();
        let new_row = self.adding.clone().map(|adding| self.render_new_row(&adding, cx));
        let theme = cx.app_theme();
        let c = &theme.colors;
        let can_open_file = from_env
            && ServerSettingsStore::keymap_environment(cx)
                .and_then(|id| ServerSettingsStore::config(cx, &id))
                .is_some_and(|config| !config.keybindings_config_path.is_empty());
        // `InputGroup` with the search icon as its addon, `sm:flex-1 h-8`.
        let search = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .h(theme.sp(8.) + px(2.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(c.input)
            .bg(theme.derived.input_fill)
            .shadow(theme.shadows.xs5.clone())
            .child(div().flex_none().pl(px(9.)).child(crate::components::icons::Icon::new(IconName::Search).size(crate::components::icons::IconSize::Sm).color(c.muted_foreground)))
            .child(div().flex_1().min_w_0().pl(px(4.)).pr(theme.sp(2.5)).child(gpui_kit::Styled::h(gpui_kit::component::input::Input::new(&self.search).appearance(false), theme.sp(8.)).ui_text(theme, TextSize::Sm, weight::NORMAL).w_full()));
        let toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.sp(2.))
            .px(theme.sp(4.))
            .py(theme.sp(3.))
            .child(search)
            .child(
                Button::new("kb-add")
                    .variant(ButtonVariant::Outline)
                    .icon(IconName::Plus)
                    .label(t!("Add keybinding"))
                    .disabled(self.adding.is_some() || !from_env)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.adding = Some(NewBinding::default());
                        cx.notify();
                    })),
            )
            .child(
                div().id("kb-open-file-tip").tooltip(tooltip(t!("Open keybindings.json"))).child(
                    Button::new("kb-open-file")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Icon)
                        .icon(IconName::FileBraces)
                        .disabled(!can_open_file)
                        .on_click(|_, _, cx| open_keybindings_file(cx)),
                ),
            );
        let new_group = new_row.map(|row| {
            div()
                .rounded(theme.radii.xl)
                .border_1()
                .border_color(with_alpha(c.border, 0.6))
                .bg(with_alpha(c.card(), 0.4))
                .shadow(theme.shadows.xs5.clone())
                .overflow_hidden()
                .child(row)
        });
        div()
            .track_focus(&self.focus)
            .key_context("KeybindingRecorder")
            .capture_key_down(cx.listener(Self::on_key_down))
            .child(
                page_container(cx)
                    .child(SettingsSection::new(setting_title("keybindings")).child(toolbar))
                    .when(!from_env, |d| d.child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("Connect an environment to change keybindings."))))
                    .children(new_group)
                    .children(sections)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .rounded(theme.radii.xl)
                                .border_1()
                                .border_color(with_alpha(c.border, 0.6))
                                .bg(with_alpha(c.card(), 0.4))
                                .px(theme.sp(4.))
                                .py(theme.sp(12.))
                                .flex()
                                .justify_center()
                                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                                .text_color(c.muted_foreground)
                                .child(t!("No keybindings match your search.")),
                        )
                    }),
            )
    }
}
