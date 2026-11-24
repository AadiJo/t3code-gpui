//! Ports `ThemeEditorPanel.tsx`, `ThemeEditorHost.tsx` and
//! `themeEditorStore.ts`: the floating "Create theme" / "Edit theme" panel.
//! It previews the edited palette on the whole app while open, saves a
//! custom theme (or merges a palette into one with the same name), and puts
//! the stored theme back when it closes.
//!
//! The panel can be dragged by its header, resized from its corner and
//! minimized. Simple mode edits Background and Accent and derives the rest;
//! Advanced edits each colour family.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, Global, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Render, SharedString, Size, Subscription, Window, anchored, canvas, deferred, div, point, prelude::*, px, size,
};
use gpui_kit::TestSupportExt as _;
use serde_json::{Map, Value};

use super::theme_color_picker::{ThemeColorPickerPanel, swatch_color, theme_role_label};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{InputSize, TextInput};
use crate::components::ui::popover::{Popover, PopoverPadding, PopoverSurface};
use crate::components::ui::switch::Switch;
use crate::components::ui::toggle::{Toggle, ToggleSize, ToggleVariant};
use crate::components::ui::tooltip::tooltip;
use crate::components::ui::{GlassExt as _, TextStyleExt as _};
use crate::hooks::use_theme::ThemeStore;
use crate::i18n::t;
use crate::theme_color::with_alpha;
use crate::theme_palette::{Appearance, ThemeDocument, ThemeSettings};
use crate::vscode_theme_import::{
    ThemeAppearance, ThemeColors, create_vivid_theme_colors, parse_theme_file, standard_theme_colors, theme_id_from_name,
    update_theme_color_family,
};

/// The roles Simple mode edits.
const SIMPLE_ROLES: [&str; 2] = ["canvas", "accent"];

/// One colour family of Advanced mode: its label, the role its swatch edits, and every role it covers.
struct Family {
    label: &'static str,
    role: &'static str,
    roles: &'static [&'static str],
}

/// `THEME_EDITOR_ROLE_GROUPS`.
const GROUPS: [(&str, &[Family]); 5] = [
    ("Foundation", &[
        Family { label: "Background", role: "canvas", roles: &["canvas", "chrome", "toolbar"] },
        Family { label: "Surface", role: "surface", roles: &["surface"] },
        Family { label: "Raised surface", role: "surfaceRaised", roles: &["surfaceRaised"] },
        Family { label: "Overlay", role: "surfaceOverlay", roles: &["surfaceOverlay"] },
        Family { label: "Text", role: "text", roles: &["text", "toolbarForeground", "toolbarControlForeground"] },
        Family {
            label: "Muted text",
            role: "mutedForeground",
            roles: &["textMuted", "mutedForeground", "placeholder", "secondaryLabel", "iconMuted", "sidebarMutedForeground"],
        },
        Family { label: "Border", role: "border", roles: &["border", "toolbarBorder", "sidebarBorder"] },
        Family { label: "Input", role: "input", roles: &["input"] },
    ]),
    ("Brand & content", &[
        Family { label: "Subtle surface", role: "secondary", roles: &["secondary", "secondaryForeground", "muted", "toolbarControl"] },
        Family { label: "Highlight surface", role: "accentSurface", roles: &["accentSurface", "accentSurfaceForeground", "toolbarControlHover"] },
        Family {
            label: "Accent",
            role: "accent",
            roles: &["accent", "accentForeground", "focus", "update", "updateForeground", "updateSurface", "terminalCursor"],
        },
        Family { label: "Action", role: "messageAction", roles: &["messageAction", "messageActionForeground", "messageActionHover"] },
        Family { label: "Message surface", role: "messageSurface", roles: &["messageSurface", "messageForeground"] },
        Family { label: "Code surface", role: "codeBackground", roles: &["codeBackground", "codeForeground"] },
    ]),
    ("Search", &[
        Family { label: "Search matches", role: "searchMatchBackground", roles: &["searchMatchBackground", "searchMatchForeground"] },
        Family { label: "Current match", role: "searchMatchActiveBackground", roles: &["searchMatchActiveBackground", "searchMatchActiveForeground"] },
    ]),
    ("Context", &[
        Family { label: "Sidebar background", role: "sidebar", roles: &["sidebar", "sidebarForeground"] },
        Family { label: "Sidebar controls", role: "sidebarControlSurface", roles: &["sidebarControlSurface"] },
        Family { label: "Sidebar selection", role: "sidebarRowSelected", roles: &["sidebarRowHover", "sidebarRowActive", "sidebarRowSelected"] },
        Family {
            label: "Terminal background",
            role: "terminalBackground",
            roles: &["terminalBackground", "terminalForeground", "terminalSelection", "terminalScrollbar", "terminalScrollbarHover"],
        },
    ]),
    ("Status", &[
        Family { label: "Error", role: "error", roles: &["error", "errorForeground", "errorSurface"] },
        Family { label: "Warning", role: "warning", roles: &["warning", "warningForeground", "warningSurface"] },
    ]),
];

/// A family or group label through the catalog (`msg(...)` in the fork).
fn label_text(label: &str) -> SharedString {
    match label {
        "Foundation" => t!("Foundation"),
        "Brand & content" => t!("Brand & content"),
        "Search" => t!("Search"),
        "Context" => t!("Context"),
        "Status" => t!("Status"),
        "Background" => t!("Background"),
        "Surface" => t!("Surface"),
        "Raised surface" => t!("Raised surface"),
        "Overlay" => t!("Overlay"),
        "Text" => t!("Text"),
        "Muted text" => t!("Muted text"),
        "Border" => t!("Border"),
        "Input" => t!("Input"),
        "Subtle surface" => t!("Subtle surface"),
        "Highlight surface" => t!("Highlight surface"),
        "Accent" => t!("Accent"),
        "Action" => t!("Action"),
        "Message surface" => t!("Message surface"),
        "Code surface" => t!("Code surface"),
        "Search matches" => t!("Search matches"),
        "Current match" => t!("Current match"),
        "Sidebar background" => t!("Sidebar background"),
        "Sidebar controls" => t!("Sidebar controls"),
        "Sidebar selection" => t!("Sidebar selection"),
        "Terminal background" => t!("Terminal background"),
        "Error" => t!("Error"),
        "Warning" => t!("Warning"),
        other => other.to_owned().into(),
    }
}

/// The family a role belongs to.
fn family_of(role: &str) -> Option<&'static Family> {
    GROUPS.iter().flat_map(|(_, families)| families.iter()).find(|f| f.roles.contains(&role))
}

fn to_import(appearance: Appearance) -> ThemeAppearance {
    if appearance.is_dark() { ThemeAppearance::Dark } else { ThemeAppearance::Light }
}

fn other(appearance: Appearance) -> Appearance {
    if appearance.is_dark() { Appearance::Light } else { Appearance::Dark }
}

fn is_color(value: &str) -> bool {
    crate::vscode_theme_import::canonical_color(value.trim()).is_some()
}

/// `getManagedEditorColors`: the palette Simple mode derives from Background and Accent.
fn managed_colors(appearance: Appearance, colors: &ThemeColors) -> ThemeColors {
    let defaults = standard_theme_colors(to_import(appearance));
    let pick = |role: &str| colors.get(role).filter(|v| is_color(v)).or_else(|| defaults.get(role)).cloned().unwrap_or_default();
    create_vivid_theme_colors(to_import(appearance), &pick("canvas"), &pick("accent"))
}

/// What a custom theme in the store holds, as the editor needs it.
#[derive(Clone)]
struct StoredTheme {
    id: String,
    label: String,
    appearance: Appearance,
    /// Each half's colours.
    colors: BTreeMap<Appearance, ThemeColors>,
    managed: bool,
    /// The theme file as stored, for the keys the editor does not edit (`css`, `collection`).
    source: Value,
}

impl StoredTheme {
    fn modes(&self) -> Vec<Appearance> {
        [Appearance::Light, Appearance::Dark].into_iter().filter(|m| self.colors.contains_key(m)).collect()
    }

    /// Reads a theme from its stored file.
    fn read(source: Value) -> Option<Self> {
        let parsed = parse_theme_file(&source).ok()?;
        let appearance = if parsed.appearance == ThemeAppearance::Dark { Appearance::Dark } else { Appearance::Light };
        let mut colors = BTreeMap::from([(appearance, parsed.colors.clone())]);
        for (name, variant) in &parsed.variants {
            colors.insert(if *name == "dark" { Appearance::Dark } else { Appearance::Light }, variant.clone());
        }
        let managed = source.get("managed").and_then(Value::as_bool) == Some(true);
        Some(Self { id: parsed.id, label: parsed.label, appearance, colors, managed, source })
    }
}

/// Opens, closes and holds the one theme editor.
#[derive(Default)]
pub struct ThemeEditorStore {
    panel: Option<Entity<ThemeEditorPanel>>,
}

impl Global for ThemeEditorStore {}

/// What to open the editor on (`ThemeEditorSessionInput`).
pub struct ThemeEditorSession {
    /// The custom theme being edited, or `None` to create one.
    pub editing_id: Option<String>,
    /// The theme a new theme starts from.
    pub seed_id: Option<String>,
    pub seed_name: Option<String>,
    pub initial_appearance: Appearance,
}

impl ThemeEditorStore {
    /// Opens the editor, replacing an open one.
    pub fn open(session: ThemeEditorSession, window: &mut Window, cx: &mut App) {
        let panel = cx.new(|cx| ThemeEditorPanel::new(session, window, cx));
        cx.default_global::<Self>().panel = Some(panel);
        cx.refresh_windows();
    }

    /// Closes the editor and puts the stored theme back.
    pub fn close(cx: &mut App) {
        if cx.default_global::<Self>().panel.take().is_some() {
            ThemeStore::refresh(cx);
            cx.refresh_windows();
        }
    }

    /// `toggleThemeEditorForTheme`: closes an open editor, or opens one seeded with the theme in use.
    pub fn toggle(window: &mut Window, cx: &mut App) {
        if cx.default_global::<Self>().panel.is_some() {
            Self::close(cx);
            return;
        }
        let appearance = cx.app_theme().appearance;
        let store = ThemeStore::global(cx);
        let seed = store.halves().get(appearance).map(str::to_owned).or_else(|| ThemeStore::find(store.preference(), cx).map(|t| t.id().to_owned()));
        Self::open(ThemeEditorSession { editing_id: None, seed_id: seed, seed_name: None, initial_appearance: appearance }, window, cx);
    }

    /// The open panel, for the window root to draw over everything.
    pub fn layer(cx: &App) -> Option<AnyView> {
        cx.try_global::<Self>().and_then(|store| store.panel.clone()).map(Into::into)
    }
}

const PANEL_MARGIN: f32 = 8.;
const PANEL_MIN_WIDTH: f32 = 280.;
const PANEL_MIN_HEIGHT: f32 = 220.;

/// The colour row a swatch popover edits, and its picker.
struct OpenPicker {
    role: &'static str,
    panel: Entity<ThemeColorPickerPanel>,
}

/// The editor panel.
pub struct ThemeEditorPanel {
    editing: Option<StoredTheme>,
    name: Entity<InputState>,
    filter: Entity<InputState>,
    appearance: Appearance,
    advanced: bool,
    colors: BTreeMap<Appearance, ThemeColors>,
    simple_dirty: BTreeMap<Appearance, bool>,
    regenerate_guided: bool,
    /// Kept from the source theme and saved with it; this app does not edit custom CSS.
    css: String,
    error: Option<SharedString>,
    minimized: bool,
    selected_role: Option<&'static str>,
    /// The colours before a picker opened, restored when Escape cancels it.
    snapshot: Option<(BTreeMap<Appearance, ThemeColors>, BTreeMap<Appearance, bool>, bool)>,
    picker: Option<OpenPicker>,
    /// Hex fields of the visible rows, by role, with the text each last showed.
    hex_inputs: BTreeMap<&'static str, (Entity<InputState>, Subscription, String)>,
    /// Top-left and width once the user drags or resizes; `None` keeps it bottom-right.
    position: Option<(Point<Pixels>, Pixels)>,
    size: Option<Size<Pixels>>,
    drag: Option<Drag>,
    bounds: gpui_kit::Bounds<Pixels>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
enum Drag {
    Move { offset: Point<Pixels>, width: Pixels },
    Resize { pointer: Point<Pixels>, start: gpui_kit::Bounds<Pixels> },
}

impl ThemeEditorPanel {
    fn new(session: ThemeEditorSession, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let find = |id: &Option<String>, cx: &App| id.as_deref().and_then(|id| ThemeStore::theme_file(id, cx)).and_then(StoredTheme::read);
        let editing = find(&session.editing_id, cx);
        let seed = find(&session.seed_id, cx);
        let source = editing.clone().or(seed);
        let mut colors: BTreeMap<Appearance, ThemeColors> = [Appearance::Light, Appearance::Dark]
            .into_iter()
            .map(|a| (a, standard_theme_colors(to_import(a))))
            .collect();
        let mut appearance = session.initial_appearance;
        if let Some(theme) = &source {
            if !theme.colors.contains_key(&appearance) {
                appearance = theme.appearance;
            }
            colors.extend(theme.colors.clone());
        }
        let name_text = editing.as_ref().map(|t| t.label.clone()).or(session.seed_name).unwrap_or_default();
        let placeholder = if editing.is_some() { t!("Theme name") } else { t!("e.g. Aurora") };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(name_text));
        name.update(cx, |input, cx| input.focus(window, cx));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter colors")));
        let on_name = cx.subscribe(&name, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.error = None;
                this.follow_merge_target(cx);
                cx.notify();
            }
        });
        let on_filter = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
        let unmanaged = source.as_ref().is_some_and(|t| !t.managed);
        let css = source.as_ref().and_then(|t| t.source.get("css")).and_then(Value::as_str).unwrap_or_default().to_owned();
        let mut this = Self {
            editing,
            name,
            filter,
            appearance,
            // Unmanaged themes open in Advanced mode to keep imported or hand-tuned colours.
            advanced: unmanaged,
            colors,
            simple_dirty: BTreeMap::new(),
            regenerate_guided: unmanaged,
            css,
            error: None,
            minimized: false,
            selected_role: None,
            snapshot: None,
            picker: None,
            hex_inputs: BTreeMap::new(),
            position: None,
            size: None,
            drag: None,
            bounds: gpui_kit::Bounds::default(),
            _subscriptions: vec![on_name, on_filter],
        };
        this.preview(cx);
        this
    }

    fn name_text(&self, cx: &App) -> String {
        self.name.read(cx).value().to_string()
    }

    /// Another custom theme this name targets, which saving merges a palette into.
    fn merge_target(&self, cx: &App) -> Option<StoredTheme> {
        let name = self.name_text(cx);
        let normalized = name.trim().to_lowercase();
        if normalized.is_empty() {
            return None;
        }
        let id = theme_id_from_name(&name);
        ThemeStore::custom_theme_files(cx)
            .into_iter()
            .filter_map(StoredTheme::read)
            .find(|t| Some(&t.id) != self.editing.as_ref().map(|e| &e.id) && (t.id == id || t.label.trim().to_lowercase() == normalized))
    }

    /// A new theme named like one with a single half moves to the other half.
    fn follow_merge_target(&mut self, cx: &mut App) {
        if self.editing.is_some() {
            return;
        }
        if let Some(target) = self.merge_target(cx) {
            let taken = target.modes();
            if taken.len() == 1 && taken[0] == self.appearance {
                self.appearance = other(self.appearance);
                self.preview(cx);
            }
        }
    }

    /// `appearanceLockReason`.
    fn lock_reason(&self, appearance: Appearance, cx: &App) -> Option<SharedString> {
        if let Some(editing) = &self.editing {
            if !editing.colors.contains_key(&appearance) {
                let label = editing.label.clone();
                return Some(if appearance.is_dark() {
                    t!("“{label}” has no dark palette. Create a theme with the same name to add one.", label = label)
                } else {
                    t!("“{label}” has no light palette. Create a theme with the same name to add one.", label = label)
                });
            }
            return None;
        }
        let target = self.merge_target(cx)?;
        target.colors.contains_key(&appearance).then(|| {
            if appearance.is_dark() {
                t!("“{label}” already has a dark palette.", label = target.label.clone())
            } else {
                t!("“{label}” already has a light palette.", label = target.label.clone())
            }
        })
    }

    /// `applyThemeColorPreview`: draws the edited half on the whole app.
    fn preview(&self, cx: &mut App) {
        let colors = self.colors.get(&self.appearance).cloned().unwrap_or_default();
        let file = serde_json::json!({ "version": 1, "id": "__preview", "name": "Preview", "appearance": self.appearance_name(self.appearance), "colors": colors });
        if let Ok(document) = ThemeDocument::parse(file.to_string().as_bytes()) {
            let appearance = self.appearance;
            let document = std::sync::Arc::new(document);
            cx.defer(move |cx| {
                ThemeSettings::update(cx, |settings| {
                    settings.document = document;
                    settings.mode = if appearance.is_dark() { crate::theme_palette::ThemeMode::Dark } else { crate::theme_palette::ThemeMode::Light };
                })
            });
        }
    }

    fn appearance_name(&self, appearance: Appearance) -> &'static str {
        to_import(appearance).name()
    }

    fn update_color(&mut self, role: &'static str, value: String, cx: &mut Context<Self>) {
        let appearance = self.appearance;
        let current = self.colors.get(&appearance).cloned().unwrap_or_default();
        let simple = !self.advanced && SIMPLE_ROLES.contains(&role) && is_color(&value);
        let next = if self.advanced {
            update_theme_color_family(to_import(appearance), &current, role, &value)
        } else {
            let mut next = current;
            next.insert(role.to_owned(), value);
            if simple { managed_colors(appearance, &next) } else { next }
        };
        self.colors.insert(appearance, next);
        if simple {
            self.simple_dirty.insert(appearance, true);
        }
        if self.advanced {
            self.regenerate_guided = true;
        }
        self.preview(cx);
        cx.notify();
    }

    fn set_advanced(&mut self, advanced: bool, cx: &mut Context<Self>) {
        self.advanced = advanced;
        if advanced {
            cx.notify();
            return;
        }
        if self.selected_role.is_some_and(|r| !SIMPLE_ROLES.contains(&r)) {
            self.selected_role = None;
        }
        if self.regenerate_guided {
            let both = self.editing.as_ref().is_some_and(|e| e.modes().len() > 1);
            let appearances = if both { vec![Appearance::Light, Appearance::Dark] } else { vec![self.appearance] };
            for appearance in appearances {
                self.simple_dirty.insert(appearance, true);
                let colors = managed_colors(appearance, &self.colors[&appearance]);
                self.colors.insert(appearance, colors);
            }
            self.regenerate_guided = false;
            self.preview(cx);
        }
        cx.notify();
    }

    /// `handleSubmit`.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let name = self.name_text(cx);
        if name.trim().is_empty() {
            self.error = Some(t!("Name your theme first."));
            cx.notify();
            return;
        }
        let target = self.merge_target(cx);
        let target_css = target.as_ref().and_then(|t| t.source.get("css")).and_then(Value::as_str).unwrap_or_default().to_owned();
        if let Some(target) = &target
            && !target_css.is_empty()
            && !self.css.trim().is_empty()
            && target_css != self.css
        {
            self.error = Some(t!("“{label}” has different custom CSS. Use the same CSS or pick another name.", label = target.label.clone()));
            cx.notify();
            return;
        }
        let css = if self.css.trim().is_empty() { target_css } else { self.css.clone() };
        let colors_for_save: BTreeMap<Appearance, ThemeColors> = if self.advanced {
            self.colors.clone()
        } else {
            self.colors
                .iter()
                .map(|(a, c)| (*a, if self.simple_dirty.get(a) == Some(&true) { managed_colors(*a, c) } else { c.clone() }))
                .collect()
        };
        let file = |id: Option<&str>, name: &str, appearance: Appearance, variants: Vec<(Appearance, ThemeColors)>, managed: bool, base: Option<&Value>| {
            let mut map = Map::new();
            map.insert("version".into(), 1.into());
            if let Some(id) = id {
                map.insert("id".into(), id.into());
            }
            map.insert("name".into(), name.into());
            map.insert("appearance".into(), self.appearance_name(appearance).into());
            map.insert("colors".into(), serde_json::to_value(&colors_for_save[&appearance]).unwrap_or_default());
            let variants: Map<String, Value> =
                variants.into_iter().filter(|(a, _)| *a != appearance).map(|(a, c)| (self.appearance_name(a).to_owned(), serde_json::to_value(c).unwrap_or_default())).collect();
            if !variants.is_empty() {
                map.insert("variants".into(), Value::Object(variants));
            }
            if managed {
                map.insert("managed".into(), true.into());
            }
            if !css.is_empty() {
                map.insert("css".into(), css.clone().into());
            }
            if let Some(collection) = base.and_then(|b| b.get("collection")) {
                map.insert("collection".into(), collection.clone());
            }
            Value::Object(map)
        };
        let (value, merged) = match (&self.editing, &target) {
            (Some(editing), Some(target)) => {
                let modes = editing.modes();
                if let Some(collision) = modes.iter().find(|m| target.colors.contains_key(m)) {
                    self.error = Some(if collision.is_dark() {
                        t!("“{label}” already has a dark palette. Pick another name.", label = target.label.clone())
                    } else {
                        t!("“{label}” already has a light palette. Pick another name.", label = target.label.clone())
                    });
                    cx.notify();
                    return;
                }
                let mut halves: Vec<(Appearance, ThemeColors)> = target.colors.clone().into_iter().collect();
                halves.extend(modes.iter().map(|m| (*m, colors_for_save[m].clone())));
                let mut value = file(Some(&target.id), &target.label, target.appearance, halves, target.managed && !self.advanced, Some(&target.source));
                value["colors"] = serde_json::to_value(&target.colors[&target.appearance]).unwrap_or_default();
                (value, Some((modes[0], Some(editing.id.clone()))))
            }
            (Some(editing), None) => {
                let halves: Vec<(Appearance, ThemeColors)> = editing.modes().into_iter().map(|m| (m, colors_for_save[&m].clone())).collect();
                (file(Some(&editing.id), &name, editing.appearance, halves, !self.advanced, Some(&editing.source)), None)
            }
            (None, Some(target)) => {
                if target.colors.contains_key(&self.appearance) {
                    self.error = Some(t!("“{label}” already has light and dark palettes. Pick another name.", label = target.label.clone()));
                    cx.notify();
                    return;
                }
                let mut halves: Vec<(Appearance, ThemeColors)> = target.colors.clone().into_iter().collect();
                halves.push((self.appearance, colors_for_save[&self.appearance].clone()));
                let mut value = file(Some(&target.id), &target.label, target.appearance, halves, target.managed && !self.advanced, Some(&target.source));
                value["colors"] = serde_json::to_value(&target.colors[&target.appearance]).unwrap_or_default();
                (value, Some((self.appearance, None)))
            }
            (None, None) => {
                if ThemeStore::find(&theme_id_from_name(&name), cx).is_some() {
                    self.error = Some(t!("A theme named \"{label}\" is already installed.", label = name.trim().to_owned()));
                    cx.notify();
                    return;
                }
                (file(None, &name, self.appearance, Vec::new(), !self.advanced, None), None)
            }
        };
        let parsed = match parse_theme_file(&value) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.error = Some(error.0.into());
                cx.notify();
                return;
            }
        };
        let mut saved = parsed.to_file();
        for key in ["managed", "css", "collection"] {
            if let Some(extra) = value.get(key) {
                saved[key] = extra.clone();
            }
        }
        let id = parsed.id.clone();
        let created = self.editing.is_none() && merged.is_none();
        let was_active = !created && {
            let store = ThemeStore::global(cx);
            store.preference().as_ref() == id || store.halves().light.as_deref() == Some(&id) || store.halves().dark.as_deref() == Some(&id)
        };
        if let Err(error) = ThemeStore::add_custom(saved.to_string().as_bytes(), cx) {
            self.error = Some(if self.editing.is_some() { error.to_string().into() } else { t!("Could not create the theme.") });
            cx.notify();
            return;
        }
        if let Some((_, Some(retired))) = &merged {
            ThemeStore::delete_custom(retired, cx);
        }
        // A new theme or a merged palette is put to use; an edited theme in use is redrawn.
        if created || merged.is_some() {
            ThemeStore::set_theme(&id, cx);
        } else if was_active {
            ThemeStore::refresh(cx);
        }
        ThemeEditorStore::close(cx);
    }

    /// The hex field of a colour row, kept in step with the palette.
    fn hex_input(&mut self, role: &'static str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some((input, _, shown)) = self.hex_inputs.get_mut(role) {
            if shown != value {
                *shown = value.to_owned();
                let (input, value) = (input.clone(), value.to_owned());
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
            return self.hex_inputs[role].0.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value.to_owned()));
        let subscription = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let text = input.read(cx).value().to_string();
                if let Some(entry) = this.hex_inputs.get_mut(role) {
                    entry.2 = text.clone();
                }
                this.update_color(role, text, cx);
            }
            InputEvent::Focus => {
                this.select_role(role, cx);
            }
            _ => {}
        });
        self.hex_inputs.insert(role, (input.clone(), subscription, value.to_owned()));
        input
    }

    /// `selectThemeRole`: selects the row; a role outside Simple mode switches to Advanced.
    fn select_role(&mut self, role: &'static str, cx: &mut Context<Self>) {
        let visible = family_of(role).map_or(role, |f| f.role);
        self.selected_role = Some(visible);
        if !SIMPLE_ROLES.contains(&visible) {
            self.advanced = true;
            self.filter.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    fn render_field(&mut self, role: &'static str, label: SharedString, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let value = self.colors.get(&self.appearance).and_then(|m| m.get(role)).cloned().unwrap_or_default();
        let valid = is_color(&value);
        let shown = if value.trim().to_lowercase().starts_with("oklch(") {
            crate::vscode_theme_import::theme_color_to_hex(&value).unwrap_or_else(|| value.clone())
        } else {
            value.clone()
        };
        let selected = self.selected_role == Some(role);
        let entity = cx.entity();
        let hex = self.hex_input(role, &shown, window, cx);
        let toggle_entity = entity.clone();
        let usage_hint = if selected { t!("Hide where {label} is used", label = label.clone()) } else { t!("Show where {label} is used", label = label.clone()) };
        let name_button = div()
            .id(SharedString::from(format!("theme-role-{role}")))
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .rounded(theme.radii.md)
            .cursor_pointer()
            .ui_text(&theme, TextSize::Sm, weight::NORMAL)
            .text_color(c.muted_foreground)
            .hover(|s| s.text_color(theme.colors.foreground()))
            .tooltip(tooltip(usage_hint))
            .on_click(move |_, _, cx| {
                toggle_entity.update(cx, |this, cx| {
                    this.selected_role = if this.selected_role == Some(role) { None } else { Some(role) };
                    cx.notify();
                })
            })
            .child(div().flex_1().min_w_0().truncate().child(label.clone()));
        let swatch_value = if valid { value.clone() } else { "#000000".to_owned() };
        let popover_entity = entity.clone();
        let picker_label = label.clone();
        let swatch = div()
            .id(SharedString::from(format!("theme-swatch-tip-{role}")))
            .tooltip(tooltip(t!("Choose {label} color", label = label.clone())))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(sp(6.))
                    .rounded_full()
                    .border_1()
                    .border_color(with_alpha(c.foreground(), 0.3))
                    .cursor_pointer()
                    .child(div().absolute().inset_0().rounded_full().bg(swatch_color(&swatch_value)).shadow(theme.shadows.sm5.clone())),
            );
        let picker = Popover::new(SharedString::from(format!("theme-picker-{role}")), swatch, move |handle, window, cx| {
            let panel = popover_entity.update(cx, |this, cx| {
                if this.picker.as_ref().is_none_or(|p| p.role != role) {
                    this.snapshot = Some((this.colors.clone(), this.simple_dirty.clone(), this.regenerate_guided));
                    this.select_role(role, cx);
                    let weak = cx.entity().downgrade();
                    let on_change: Rc<dyn Fn(String, &mut Window, &mut App)> =
                        Rc::new(move |color, _, cx| _ = weak.update(cx, |this, cx| this.update_color(role, color, cx)));
                    let value = this.colors.get(&this.appearance).and_then(|m| m.get(role)).cloned().unwrap_or_default();
                    let label = picker_label.clone();
                    let panel = cx.new(|cx| ThemeColorPickerPanel::new(label, &value, on_change, window, cx));
                    this.picker = Some(OpenPicker { role, panel });
                }
                this.picker.as_ref().map(|p| p.panel.clone())
            });
            let cancel = popover_entity.clone();
            PopoverSurface::new()
                .padding(PopoverPadding::None)
                .child(
                    div()
                        .on_key_down(move |event, _, cx| {
                            if event.keystroke.key == "escape" {
                                cancel.update(cx, |this, cx| {
                                    if let Some((colors, dirty, regenerate)) = this.snapshot.take() {
                                        this.colors = colors;
                                        this.simple_dirty = dirty;
                                        this.regenerate_guided = regenerate;
                                        this.preview(cx);
                                    }
                                    this.picker = None;
                                    cx.notify();
                                });
                                handle.close(cx);
                            }
                        })
                        .children(panel),
                )
                .into_any_element()
        });
        let hex_field = div().id(SharedString::from(format!("theme-hex-{role}"))).w(theme.rem(7.)).flex_none().child(TextInput::new(&hex).size(InputSize::Compact).mono().invalid(!valid)).test_support();
        div()
            .id(SharedString::from(format!("theme-field-{role}")))
            .flex()
            .min_h(sp(11.))
            .min_w_0()
            .items_center()
            .gap(sp(2.))
            .rounded(theme.radii.lg)
            .px(sp(2.))
            .py(sp(1.5))
            .when(selected, |d| d.bg(with_alpha(c.accent_surface, 0.6)).border_1().border_color(c.ring()))
            .child(name_button)
            .child(div().ml_auto().flex().flex_none().items_center().gap(sp(2.)).child(picker).child(hex_field))
            .into_any_element()
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let entity = cx.entity();
        let row = |label: SharedString, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(sp(3.))
                .child(div().w(gpui_kit::relative(1. / 3.)).flex_none().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(label))
                .child(div().flex_1().min_w_0().child(control))
                .into_any_element()
        };
        let name_row = row(t!("Theme name"), TextInput::new(&self.name).size(InputSize::Sm).into_any_element());
        let segment = |appearance: Appearance, cx: &App| {
            let reason = self.lock_reason(appearance, cx);
            let entity = entity.clone();
            let toggle = Toggle::new(SharedString::from(format!("theme-editor-{}", self.appearance_name(appearance))))
                .label(if appearance.is_dark() { t!("Dark") } else { t!("Light") })
                .variant(ToggleVariant::Segmented)
                .size(ToggleSize::Segmented)
                .pressed(self.appearance == appearance)
                .on_toggle(move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        if this.lock_reason(appearance, cx).is_none() {
                            this.appearance = appearance;
                            this.preview(cx);
                            cx.notify();
                        }
                    })
                });
            match reason {
                Some(reason) => div().id(SharedString::from(format!("theme-editor-lock-{}", self.appearance_name(appearance)))).tooltip(tooltip(reason)).child(toggle).into_any_element(),
                None => toggle.into_any_element(),
            }
        };
        let appearance_row = row(
            t!("Appearance"),
            div()
                .flex()
                .w_auto()
                .gap(sp(0.5))
                .rounded(theme.radii.lg)
                .bg(with_alpha(c.input, 0.4))
                .p(sp(0.5))
                .child(segment(Appearance::Light, cx))
                .child(segment(Appearance::Dark, cx))
                .into_any_element(),
        );
        let advanced_entity = entity.clone();
        let colors_header = div()
            .flex()
            .items_start()
            .gap(sp(3.))
            .child(
                div()
                    .w(gpui_kit::relative(1. / 3.))
                    .flex_none()
                    .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(t!("Colors")))
                    .when(!self.advanced, |d| d.child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("Two colors, rest derived")))),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_start()
                    .gap(sp(3.))
                    .when(self.advanced, |d| d.child(div().flex_1().min_w_0().child(TextInput::new(&self.filter).size(InputSize::Sm))))
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(sp(2.))
                            .pt(sp(0.5))
                            .ui_text(&theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(c.foreground())
                            .child(t!("Advanced"))
                            .child(Switch::new("theme-editor-advanced", self.advanced).on_change(move |checked, _, cx| {
                                advanced_entity.update(cx, |this, cx| this.set_advanced(checked, cx))
                            })),
                    ),
            );
        let mut fields: Vec<AnyElement> = Vec::new();
        if self.advanced {
            let query = self.filter.read(cx).value().trim().to_lowercase();
            let mut any = false;
            for (title, families) in GROUPS {
                let shown: Vec<&Family> = families
                    .iter()
                    .filter(|f| {
                        query.is_empty() || std::iter::once(label_text(f.label).to_string()).chain(f.roles.iter().map(|r| theme_role_label(r).to_string())).collect::<Vec<_>>().join(" ").to_lowercase().contains(&query)
                    })
                    .collect();
                if shown.is_empty() {
                    continue;
                }
                any = true;
                let rows: Vec<AnyElement> = shown.into_iter().map(|f| self.render_field(f.role, label_text(f.label), window, cx)).collect();
                fields.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(2.))
                        .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(label_text(title)))
                        .child(div().flex().flex_col().gap(sp(1.)).children(rows))
                        .into_any_element(),
                );
            }
            if !any {
                fields.push(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("No matches.")).into_any_element());
            }
        } else {
            let rows = [("canvas", t!("Background")), ("accent", t!("Accent"))].map(|(role, label)| self.render_field(role, label, window, cx));
            fields.push(div().flex().flex_col().gap(sp(1.)).children(rows).into_any_element());
        }
        // Rows hidden by the filter or mode keep no hex field.
        let visible: Vec<&'static str> = if self.advanced {
            GROUPS.iter().flat_map(|(_, f)| f.iter().map(|f| f.role)).collect()
        } else {
            SIMPLE_ROLES.to_vec()
        };
        self.hex_inputs.retain(|role, _| visible.contains(role));
        div()
            .id("theme-editor-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(sp(3.))
            .py(sp(3.))
            .pl(sp(3.))
            .pr(sp(1.5))
            .child(name_row)
            .children(self.error.clone().map(|e| div().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(c.destructive()).child(e)))
            .child(appearance_row)
            .child(div().flex().flex_col().gap(sp(5.).min(sp(3.))).child(colors_header).children(fields))
            .test_support()
            .into_any_element()
    }

    fn start_drag(&mut self, drag: Drag, cx: &mut Context<Self>) {
        self.drag = Some(drag);
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>, viewport: Size<Pixels>, cx: &mut Context<Self>) {
        let margin = px(PANEL_MARGIN);
        match self.drag {
            Some(Drag::Move { offset, width }) => {
                self.position = Some((clamp_position(viewport, position - offset, width), width));
            }
            Some(Drag::Resize { pointer, start }) => {
                let max_w = px(PANEL_MIN_WIDTH).max(viewport.width - margin - start.origin.x);
                let max_h = px(PANEL_MIN_HEIGHT).max(viewport.height - margin - start.origin.y);
                self.size = Some(size(
                    (start.size.width + position.x - pointer.x).max(px(PANEL_MIN_WIDTH)).min(max_w),
                    (start.size.height + position.y - pointer.y).max(px(PANEL_MIN_HEIGHT)).min(max_h),
                ));
            }
            None => return,
        }
        cx.notify();
    }
}

/// `clampPanelPosition`: inside the window horizontally; vertically only the header must stay visible.
fn clamp_position(viewport: Size<Pixels>, at: Point<Pixels>, width: Pixels) -> Point<Pixels> {
    let margin = px(PANEL_MARGIN);
    let max_x = margin.max(viewport.width - width - margin);
    let max_y = margin.max(viewport.height - px(48.));
    point(at.x.max(margin).min(max_x), at.y.max(margin).min(max_y))
}

impl Render for ThemeEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let viewport = window.viewport_size();
        let editing = self.editing.is_some();
        let title = if editing { t!("Edit theme") } else { t!("Create theme") };
        let subtitle = match self.selected_role {
            Some(role) => if self.advanced { family_of(role).map_or_else(|| theme_role_label(role), |f| label_text(f.label)) } else { theme_role_label(role) },
            None => t!("Select a color below"),
        };
        let entity = cx.entity();
        let margin = px(PANEL_MARGIN);
        let width = self.size.map(|s| s.width.max(px(PANEL_MIN_WIDTH)).min(viewport.width - margin * 2.)).unwrap_or_else(|| theme.rem(26.).min(viewport.width - theme.rem(2.)));
        let height = self.size.filter(|_| !self.minimized).map(|s| s.height.max(px(PANEL_MIN_HEIGHT)).min(viewport.height - margin * 2.));
        let max_height = if self.minimized { viewport.height } else { theme.rem(42.).min(viewport.height - theme.rem(6.)) };
        let origin = self.position.map(|(at, w)| clamp_position(viewport, at, self.size.map_or(w, |_| width)));

        let move_entity = entity.clone();
        let header = div()
            .id("theme-editor-header")
            .flex()
            .items_center()
            .gap(sp(1.))
            .border_b_1()
            .border_color(with_alpha(c.border, 0.7))
            .px(sp(3.))
            .py(sp(2.))
            .cursor_grab()
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                move_entity.update(cx, |this, cx| {
                    let bounds = this.bounds;
                    this.start_drag(Drag::Move { offset: event.position - bounds.origin, width: bounds.size.width }, cx);
                })
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap(sp(2.))
                    .child(div().flex_none().truncate().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(title))
                    .when(!self.minimized, |d| d.child(div().truncate().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(subtitle))),
            )
            .child({
                let entity = entity.clone();
                Button::new("theme-editor-minimize")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconXs)
                    .icon(if self.minimized { IconName::ChevronUp } else { IconName::ChevronDown })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        entity.update(cx, |this, cx| {
                            this.minimized = !this.minimized;
                            cx.notify();
                        })
                    })
            })
            .child(Button::new("theme-editor-close").variant(ButtonVariant::Ghost).size(ButtonSize::IconXs).icon(IconName::X).on_click(|_, _, cx| {
                cx.stop_propagation();
                ThemeEditorStore::close(cx);
            }));

        let mut panel = div()
            .id("theme-editor-panel")
            .occlude()
            .relative()
            .flex()
            .flex_col()
            .w(width)
            .max_h(max_height)
            .when_some(height, |d, h| d.h(h))
            .overflow_hidden()
            .rounded(theme.radii.xl)
            .border_1()
            .border_color(theme.derived.dialog_glass_border)
            .glass(theme.derived.glass_surface)
            .shadow(theme.shadows.dialog.clone())
            .text_color(theme.foregrounds.popover)
            .on_key_down(|event, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    ThemeEditorStore::close(cx);
                }
            })
            .child({
                let entity = entity.clone();
                canvas(move |bounds, _, cx| entity.update(cx, |this, _| this.bounds = bounds), |_, _, _, _| {}).absolute().size_full()
            })
            .child(header);
        if !self.minimized {
            let body = self.render_body(window, cx);
            let submit_label = match (editing, self.merge_target(cx)) {
                (true, Some(target)) => t!("Merge into “{label}”", label = target.label.clone()),
                (true, None) => t!("Save changes"),
                (false, Some(_)) => if self.appearance.is_dark() { t!("Add dark palette") } else { t!("Add light palette") },
                (false, None) => t!("Create theme"),
            };
            let submit_icon = match (editing, self.merge_target(cx).is_some()) {
                (true, _) => None,
                (false, true) => Some(IconName::Plus),
                (false, false) => Some(IconName::Paintbrush),
            };
            let submit_entity = entity.clone();
            let mut submit = Button::new("theme-editor-submit")
                .size(ButtonSize::Sm)
                .label(submit_label)
                .disabled(self.name_text(cx).trim().is_empty())
                .on_click(move |_, _, cx| submit_entity.update(cx, |this, cx| this.submit(cx)));
            if let Some(icon) = submit_icon {
                submit = submit.icon(icon);
            }
            let resize_entity = entity.clone();
            panel = panel
                .child(body)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(sp(2.))
                        .border_t_1()
                        .border_color(with_alpha(c.border, 0.7))
                        .px(sp(3.))
                        .py(sp(2.))
                        .child(Button::new("theme-editor-cancel").variant(ButtonVariant::Ghost).size(ButtonSize::Sm).label(t!("Cancel")).on_click(|_, _, cx| ThemeEditorStore::close(cx)))
                        .child(submit),
                )
                .child(
                    div()
                        .id("theme-editor-resize")
                        .absolute()
                        .bottom_0()
                        .right_0()
                        .size(sp(5.))
                        .flex()
                        .items_end()
                        .justify_end()
                        .p(sp(1.))
                        .cursor(gpui_kit::CursorStyle::ResizeUpLeftDownRight)
                        .text_color(with_alpha(c.muted_foreground, 0.7))
                        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                            cx.stop_propagation();
                            resize_entity.update(cx, |this, cx| {
                                let bounds = this.bounds;
                                this.position = Some((clamp_position(viewport, bounds.origin, bounds.size.width), bounds.size.width));
                                this.start_drag(Drag::Resize { pointer: event.position, start: bounds }, cx);
                            })
                        })
                        .child(resize_grip(sp(2.5), with_alpha(c.muted_foreground, 0.7))),
                );
        }
        // While dragging or resizing, follow the pointer anywhere in the window.
        if self.drag.is_some() {
            let entity = entity.clone();
            panel = panel.child(
                canvas(|_, _, _| {}, move |_, _, window, _| {
                    let moved = entity.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, _, window, cx| {
                        let viewport = window.viewport_size();
                        moved.update(cx, |this, cx| this.drag_to(e.position, viewport, cx));
                    });
                    let up = entity.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                        up.update(cx, |this, cx| {
                            this.drag = None;
                            cx.notify();
                        })
                    });
                })
                .absolute()
                .size_0(),
            );
        }
        let placed = match origin {
            Some(at) => div().absolute().left(at.x).top(at.y).child(panel),
            None => div().absolute().right(sp(4.)).bottom(sp(4.)).child(panel),
        };
        deferred(anchored().position(point(px(0.), px(0.))).child(div().w(viewport.width).h(viewport.height).relative().child(placed))).with_priority(5)
    }
}

/// The corner grip: two strokes, `M7 1 1 7M7 4.5 4.5 7` in an 8x8 box at 1.2 wide.
fn resize_grip(side: Pixels, color: gpui_kit::Hsla) -> impl IntoElement {
    canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let scale = bounds.size.width / 8.;
        let at = |x: f32, y: f32| bounds.origin + point(scale * x, scale * y);
        let mut path = gpui_kit::PathBuilder::stroke(scale * 1.2);
        path.move_to(at(7., 1.));
        path.line_to(at(1., 7.));
        path.move_to(at(7., 4.5));
        path.line_to(at(4.5, 7.));
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    })
    .size(side)
}

/// Opens the editor on a custom theme (its card's Edit button).
pub fn edit_theme(id: &str, window: &mut Window, cx: &mut App) {
    let appearance = cx.app_theme().appearance;
    ThemeEditorStore::open(ThemeEditorSession { editing_id: Some(id.to_owned()), seed_id: None, seed_name: None, initial_appearance: appearance }, window, cx);
}

/// Opens the editor on a copy of `seed` named "<label> copy" (a card's Duplicate button).
pub fn duplicate_theme(seed: Option<&str>, label: &str, window: &mut Window, cx: &mut App) {
    let appearance = cx.app_theme().appearance;
    ThemeEditorStore::open(
        ThemeEditorSession {
            editing_id: None,
            seed_id: seed.map(str::to_owned),
            seed_name: Some(format!("{label} copy")),
            initial_appearance: appearance,
        },
        window,
        cx,
    );
}
