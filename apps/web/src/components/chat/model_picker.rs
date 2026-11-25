//! The composer's model picker popover. Ports `ModelPickerContent.tsx`,
//! `ModelPickerSidebar.tsx`, `ModelListRow.tsx`, the effort column and footer
//! of `ComposerModelEffort.tsx`, and the popup layout of `ProviderModelPicker.tsx`
//! with the fork's changes: a fixed 320px body, Claude before OpenAI, no "New"
//! badge, and model icon overrides.

use crate::components::ui::GlassExt as _;
use crate::components::icons::{Icon, IconName, IconSize};
use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, Context, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Pixels, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, UTF16Selection, UniformListScrollHandle, Window, actions, div,
    hsla, point, prelude::FluentBuilder as _, px, uniform_list,
};

use crate::components::chat::composer_icons::{composer_icon_stroke, provider_icon};
use crate::components::chat::composer_permissions::{RuntimeMode, runtime_mode_icon};
use crate::components::chat::composer_theme::{
    ComposerColors, MODEL_PICKER_EFFORT_WIDTH, MODEL_PICKER_HEIGHT, MODEL_PICKER_LIST_WIDTH, MODEL_PICKER_RAIL_WIDTH,
    POPUP_RADIUS, ui_font_family,
};
use crate::shortcuts::commands;
use crate::model_ordering::{ProviderModelItem, order_claude_before_openai, sort_provider_model_items};
use crate::model_search::{SearchableModel, rank_model_search};
use crate::model_selection::{
    ModelCatalog, ModelOption, ModelSelection, ModelTraits, OptionChoice, OptionDescriptor, OptionValue, ProviderIcon,
    ProviderInstanceEntry, ProviderInstanceId, SelectDescriptor, display_model_name,
};

/// One option's own height (`py-1` around the 24px icon-and-label content), at a 16px rem.
const MODEL_OPTION_HEIGHT: Pixels = px(32.);
const PICKER_CONTEXT: &str = "ModelPicker";
/// Footer: `py-2` around a 28px control, plus its 1px top border.
const FOOTER_HEIGHT: Pixels = px(45.);
/// The popup's outer size: list, effort column, footer and the 1px border, in rem
/// (`h-80`, `w-48`, ...) at the interface size; `plain` drops the effort column and
/// footer (`ModelPickerContent` alone, as settings open it). Fixed per interface size, so
/// the composer can cache the picker's subtree with a known layout.
pub fn model_picker_size(plain: bool, c: &ComposerColors) -> gpui_kit::Size<Pixels> {
    let border = px(2.);
    if plain {
        gpui_kit::Size { width: c.r(MODEL_PICKER_LIST_WIDTH) + border, height: c.r(MODEL_PICKER_HEIGHT) + border }
    } else {
        gpui_kit::Size {
            width: c.r(MODEL_PICKER_LIST_WIDTH) + c.r(MODEL_PICKER_EFFORT_WIDTH) + border,
            height: c.r(MODEL_PICKER_HEIGHT) + c.r(FOOTER_HEIGHT) + border,
        }
    }
}

actions!(model_picker, [PickerUp, PickerDown, PickerConfirm, PickerClose, PickerBackspace, PreviousProvider, NextProvider, PickerLeft, PickerRight]);

/// Binds the picker's keys.
pub fn bind_keys(cx: &mut App) {
    let c = Some(PICKER_CONTEXT);
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    let (prev, next): (&'static str, &'static str) =
        if modifier == "cmd" { ("cmd-shift-up", "cmd-shift-down") } else { ("ctrl-shift-up", "ctrl-shift-down") };
    cx.bind_keys([
        KeyBinding::new("up", PickerUp, c),
        KeyBinding::new("down", PickerDown, c),
        KeyBinding::new("enter", PickerConfirm, c),
        KeyBinding::new("escape", PickerClose, c),
        KeyBinding::new("backspace", PickerBackspace, c),
        KeyBinding::new("left", PickerLeft, c),
        KeyBinding::new("right", PickerRight, c),
        KeyBinding::new(prev, PreviousProvider, c),
        KeyBinding::new(next, NextProvider, c),
    ]);
}

/// Which list the rail shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerTab {
    Favorites,
    Instance(ProviderInstanceId),
}

/// What the picker asks its owner to do.
#[derive(Clone, Debug)]
pub enum ModelPickerEvent {
    Select(ModelSelection),
    ToggleFavorite { instance_id: ProviderInstanceId, model: SharedString },
    RuntimeModeChanged(RuntimeMode),
    Close,
}

/// A row of the list.
#[derive(Clone, Debug)]
enum Row {
    Model(PickerItem),
    Legacy { expanded: bool },
}

/// A row of the effort column (`EffortRows`, `OptionMenuRow`, `OptionSwitchRow`).
#[derive(Clone, Debug)]
enum ColumnRow {
    /// A reasoning level.
    Level(SelectDescriptor, OptionChoice),
    /// On or Off of a two-state reasoning control (`thinking`).
    State(SharedString, bool),
    /// Another select: its name over its value, with its choices in a side menu.
    Menu(SelectDescriptor),
    /// Another two-state option, toggled in place.
    Switch(SharedString, SharedString, bool),
}

#[derive(Clone, Debug)]
struct PickerItem {
    instance_id: ProviderInstanceId,
    entry: ProviderInstanceEntry,
    model: ModelOption,
}

impl ProviderModelItem for PickerItem {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.instance_id
    }
    fn slug(&self) -> &str {
        &self.model.slug
    }
}

/// The model picker entity. Open it with [`ModelPicker::new`]; it emits
/// [`ModelPickerEvent`]s and owns only view state (tab, query, highlight).
pub struct ModelPicker {
    focus_handle: FocusHandle,
    catalog: Arc<ModelCatalog>,
    selection: ModelSelection,
    runtime_mode: RuntimeMode,
    tab: PickerTab,
    query: String,
    highlighted: usize,
    expanded_legacy: HashSet<ProviderInstanceId>,
    rows: Arc<Vec<Row>>,
    scroll: UniformListScrollHandle,
    hovered_row: Option<usize>,
    /// The footer's permissions menu is open, and the mode a hover previews.
    permissions_open: bool,
    permissions_preview: Option<RuntimeMode>,
    /// Without the effort column and footer, as pickers outside the composer show it.
    plain: bool,
    /// The effort column row the keyboard is on (`composer.effort` moves focus to the column).
    effort_focus: Option<usize>,
    /// The option whose side menu is open, and the choice the keyboard is on in it.
    option_menu: Option<(SharedString, Option<usize>)>,
    /// Where the side menu was last painted: it floats outside the picker, so a press there is
    /// not a press outside.
    option_menu_bounds: std::rc::Rc<std::cell::Cell<Option<gpui_kit::Bounds<Pixels>>>>,
    /// Where the effort column was last painted, for the side menu's flip.
    column_bounds: std::rc::Rc<std::cell::Cell<Option<gpui_kit::Bounds<Pixels>>>>,
    /// Where an input method's uncommitted text starts in `query`, while it composes.
    marked_from: Option<usize>,
}

impl EventEmitter<ModelPickerEvent> for ModelPicker {}

impl Focusable for ModelPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModelPicker {
    pub fn new(catalog: Arc<ModelCatalog>, selection: ModelSelection, runtime_mode: RuntimeMode, cx: &mut Context<Self>) -> Self {
        // Opens on Favorites when the active model is a favourite, else on its instance.
        let tab = if catalog.is_favorite(&selection.instance_id, &selection.model) {
            PickerTab::Favorites
        } else {
            PickerTab::Instance(selection.instance_id.clone())
        };
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            catalog,
            selection,
            runtime_mode,
            tab,
            query: String::new(),
            highlighted: 0,
            expanded_legacy: HashSet::new(),
            rows: Arc::default(),
            scroll: UniformListScrollHandle::new(),
            hovered_row: None,
            permissions_open: false,
            permissions_preview: None,
            plain: false,
            effort_focus: None,
            option_menu: None,
            option_menu_bounds: Default::default(),
            column_bounds: Default::default(),
            marked_from: None,
        };
        this.rebuild_rows();
        this.highlight_selected();
        this
    }

    /// Drops the effort column and footer (`ProviderModelPicker` without `effortColumn`).
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }

    /// Updates the catalog and selection from the composer.
    pub fn update_state(&mut self, catalog: Arc<ModelCatalog>, selection: ModelSelection, runtime_mode: RuntimeMode, cx: &mut Context<Self>) {
        self.catalog = catalog;
        self.selection = selection;
        self.runtime_mode = runtime_mode;
        self.rebuild_rows();
        cx.notify();
    }

    /// Moves the keyboard to the selected effort level (`focusEffort`); arrows then move between
    /// the column's rows and Enter picks one. Without levels the first option row takes it.
    pub fn focus_effort(&mut self, cx: &mut Context<Self>) {
        let rows = self.column_rows();
        if rows.is_empty() {
            return;
        }
        let index = rows
            .iter()
            .position(|row| match row {
                ColumnRow::Level(d, o) => d.value() == Some(&o.id),
                ColumnRow::State(_, on) => self.traits().and_then(|t| t.reasoning().cloned()).is_some_and(|r| matches!(r, OptionDescriptor::Boolean(b) if b.current == Some(true)) == *on),
                _ => false,
            })
            .unwrap_or(0);
        self.effort_focus = Some(index);
        self.option_menu = None;
        self.permissions_open = false;
        cx.notify();
    }

    /// Opens the footer's permissions menu (`composer.mode` when the picker owns it).
    pub fn open_permissions(&mut self, cx: &mut Context<Self>) {
        if !self.plain {
            self.permissions_open = true;
            self.permissions_preview = None;
            self.effort_focus = None;
            cx.notify();
        }
    }

    /// The effort level under keyboard focus, if the column has it.
    pub fn effort_focus(&self) -> Option<usize> {
        self.effort_focus
    }

    /// Whether the footer's permissions menu is open.
    pub fn permissions_open(&self) -> bool {
        self.permissions_open
    }

    /// The selected model's options under the selection.
    fn traits(&self) -> Option<ModelTraits> {
        self.selection.traits(&self.catalog)
    }

    /// The effort column's rows: the reasoning control's levels, then every other option.
    fn column_rows(&self) -> Vec<ColumnRow> {
        let Some(traits) = self.traits().filter(|_| !self.plain) else { return Vec::new() };
        let mut rows = Vec::new();
        match traits.reasoning() {
            Some(OptionDescriptor::Select(d)) => rows.extend(d.selectable().into_iter().map(|o| ColumnRow::Level(d.clone(), o))),
            Some(OptionDescriptor::Boolean(b)) => rows.extend([true, false].map(|on| ColumnRow::State(b.id.clone(), on))),
            None => {}
        }
        rows.extend(traits.others().into_iter().map(|d| match d {
            OptionDescriptor::Select(s) => ColumnRow::Menu(s.clone()),
            OptionDescriptor::Boolean(b) => ColumnRow::Switch(b.id.clone(), b.label.clone(), b.current == Some(true)),
        }));
        rows
    }

    /// Writes one option into the selection and tells the owner (`changeSelect`, `changeBoolean`).
    fn set_option(&mut self, id: &str, value: OptionValue, cx: &mut Context<Self>) {
        let Some(traits) = self.traits() else { return };
        if let (Some(OptionDescriptor::Select(d)), OptionValue::Text(v)) = (traits.descriptors.iter().find(|d| d.id().as_ref() == id), &value) {
            if d.prompt_injected.contains(v) {
                return;
            }
        }
        self.selection.options = traits.with_value(id, value);
        cx.emit(ModelPickerEvent::Select(self.selection.clone()));
        cx.notify();
    }

    /// Runs a column row: picks a level or state, opens a side menu, or flips a switch.
    fn activate_row(&mut self, row: ColumnRow, cx: &mut Context<Self>) {
        match row {
            ColumnRow::Level(d, o) => self.set_option(&d.id, OptionValue::Text(o.id), cx),
            ColumnRow::State(id, on) => self.set_option(&id, OptionValue::Bool(on), cx),
            ColumnRow::Switch(id, _, on) => self.set_option(&id, OptionValue::Bool(!on), cx),
            ColumnRow::Menu(d) => self.open_option_menu(&d, cx),
        }
    }

    /// Opens a select's side menu with the keyboard on its current choice.
    fn open_option_menu(&mut self, descriptor: &SelectDescriptor, cx: &mut Context<Self>) {
        let value = descriptor.value().cloned();
        let at = descriptor.selectable().iter().position(|o| Some(&o.id) == value.as_ref());
        self.option_menu = Some((descriptor.id.clone(), at.or(Some(0))));
        cx.notify();
    }

    /// The open side menu's select under the selection.
    fn open_menu_descriptor(&self) -> Option<SelectDescriptor> {
        let (id, _) = self.option_menu.as_ref()?;
        self.traits()?.descriptors.into_iter().find_map(|d| match d {
            OptionDescriptor::Select(s) if &s.id == id => Some(s),
            _ => None,
        })
    }

    /// The option whose side menu is open, by id (for checks).
    pub fn open_option_menu_id(&self) -> Option<SharedString> {
        self.option_menu.as_ref().map(|(id, _)| id.clone())
    }

    /// The rail's instances: visible ones, Claude before OpenAI.
    fn rail_entries(&self) -> Vec<ProviderInstanceEntry> {
        order_claude_before_openai(&self.catalog.instances, |e| &e.driver_kind)
    }

    fn items(&self) -> Vec<PickerItem> {
        let selected = Some(self.selection.model.as_ref());
        self.rail_entries()
            .into_iter()
            .flat_map(|entry| {
                let instance = entry.instance_id.clone();
                let sel = (instance == self.selection.instance_id).then_some(selected).flatten();
                self.catalog
                    .visible_models(&instance, sel)
                    .into_iter()
                    .map(|model| PickerItem { instance_id: instance.clone(), entry: entry.clone(), model: model.clone() })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn rebuild_rows(&mut self) {
        let items = self.items();
        let favorites = &self.catalog.favorites;
        let order: Vec<ProviderInstanceId> = self.rail_entries().into_iter().map(|e| e.instance_id).collect();
        let query = self.query.trim().to_owned();
        let rows = if !query.is_empty() {
            rank_model_search(items, &query, |item| SearchableModel {
                driver_kind: item.entry.driver_kind.as_str(),
                provider_display_name: &item.entry.display_name,
                name: &item.model.name,
                short_name: item.model.short_name.as_deref(),
                sub_provider: item.model.sub_provider.as_deref(),
                is_favorite: self.catalog.is_favorite(&item.instance_id, &item.model.slug),
            })
            .into_iter()
            .map(Row::Model)
            .collect()
        } else {
            match &self.tab {
                PickerTab::Favorites => {
                    let favs: Vec<_> = items.into_iter().filter(|i| self.catalog.is_favorite(&i.instance_id, &i.model.slug)).collect();
                    sort_provider_model_items(&favs, favorites, false, &order).into_iter().map(Row::Model).collect()
                }
                PickerTab::Instance(id) => {
                    let mine: Vec<_> = items.into_iter().filter(|i| &i.instance_id == id).collect();
                    let sorted = sort_provider_model_items(&mine, favorites, true, &[]);
                    let (legacy, current): (Vec<_>, Vec<_>) = sorted
                        .into_iter()
                        .partition(|i| i.model.is_legacy && !self.catalog.is_favorite(&i.instance_id, &i.model.slug));
                    let mut rows: Vec<Row> = current.into_iter().map(Row::Model).collect();
                    if !legacy.is_empty() {
                        let expanded = self.expanded_legacy.contains(id);
                        if expanded {
                            rows.extend(legacy.into_iter().map(Row::Model));
                        }
                        rows.push(Row::Legacy { expanded });
                    }
                    rows
                }
            }
        };
        self.rows = Arc::new(rows);
        self.highlighted = self.highlighted.min(self.rows.len().saturating_sub(1));
    }

    fn highlight_selected(&mut self) {
        if let Some(index) = self.rows.iter().position(|row| match row {
            Row::Model(item) => item.instance_id == self.selection.instance_id && item.model.slug == self.selection.model,
            Row::Legacy { .. } => false,
        }) {
            self.highlighted = index;
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    fn select_tab(&mut self, tab: PickerTab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.query.clear();
        self.highlighted = 0;
        self.rebuild_rows();
        cx.notify();
    }

    fn pick_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index).cloned() else { return };
        match row {
            Row::Legacy { .. } => {
                if let PickerTab::Instance(id) = &self.tab {
                    let id = id.clone();
                    if !self.expanded_legacy.remove(&id) {
                        self.expanded_legacy.insert(id);
                    }
                    self.rebuild_rows();
                    cx.notify();
                }
            }
            Row::Model(item) => {
                if item.model.is_unavailable || !item.entry.ready {
                    return;
                }
                // `pickModelSelection`: the same model keeps its options, another starts without any.
                let same = item.instance_id == self.selection.instance_id && item.model.slug == self.selection.model;
                let selection = ModelSelection {
                    instance_id: item.instance_id,
                    model: item.model.slug,
                    options: if same { self.selection.options.clone() } else { Vec::new() },
                };
                self.selection = selection.clone();
                cx.emit(ModelPickerEvent::Select(selection));
                // A docked picker stays open and moves to the new model's levels (`focusEffort("levels")`).
                self.focus_effort(cx);
                cx.notify();
            }
        }
    }

    /// `⌘1`–`⌘9`: picks the row whose hint shows that number.
    fn jump(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.rows.len() {
            self.highlighted = index;
            self.pick_row(index, cx);
        }
    }

    fn adjacent_provider(&mut self, step: isize, cx: &mut Context<Self>) {
        let mut tabs = vec![PickerTab::Favorites];
        tabs.extend(self.rail_entries().into_iter().filter(|e| e.ready).map(|e| PickerTab::Instance(e.instance_id)));
        let current = tabs.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(tabs.len() as isize) as usize;
        self.select_tab(tabs[next].clone(), cx);
    }

    fn on_up(&mut self, _: &PickerUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, at)) = self.option_menu.clone() {
            let count = self.open_menu_descriptor().map_or(0, |d| d.selectable().len());
            self.option_menu = Some((id, Some(at.map_or(0, |at| at.saturating_sub(1)).min(count.saturating_sub(1)))));
            return cx.notify();
        }
        if let Some(at) = self.effort_focus {
            // `moveBetweenRows`: Up and Down stop at the column's ends.
            self.effort_focus = Some(at.saturating_sub(1));
            return cx.notify();
        }
        if !self.rows.is_empty() {
            self.highlighted = (self.highlighted + self.rows.len() - 1) % self.rows.len();
            self.scroll.scroll_to_item(self.highlighted, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn on_down(&mut self, _: &PickerDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, at)) = self.option_menu.clone() {
            let count = self.open_menu_descriptor().map_or(0, |d| d.selectable().len());
            self.option_menu = Some((id, Some(at.map_or(0, |at| at + 1).min(count.saturating_sub(1)))));
            return cx.notify();
        }
        if let Some(at) = self.effort_focus {
            self.effort_focus = Some((at + 1).min(self.column_rows().len().saturating_sub(1)));
            return cx.notify();
        }
        if !self.rows.is_empty() {
            self.highlighted = (self.highlighted + 1) % self.rows.len();
            self.scroll.scroll_to_item(self.highlighted, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn on_confirm(&mut self, _: &PickerConfirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, at)) = self.option_menu.clone() {
            let picked = self.open_menu_descriptor().and_then(|d| Some((d.id.clone(), d.selectable().get(at?)?.id.clone())));
            self.option_menu = None;
            if let Some((id, value)) = picked {
                self.set_option(&id, OptionValue::Text(value), cx);
            }
            return cx.notify();
        }
        if let Some(row) = self.effort_focus.and_then(|at| self.column_rows().get(at).cloned()) {
            return self.activate_row(row, cx);
        }
        self.pick_row(self.highlighted, cx);
    }

    /// ArrowRight on an option row opens its side menu.
    fn on_right(&mut self, _: &PickerRight, _: &mut Window, cx: &mut Context<Self>) {
        match self.effort_focus.and_then(|at| self.column_rows().get(at).cloned()) {
            Some(ColumnRow::Menu(d)) if self.option_menu.is_none() => self.open_option_menu(&d, cx),
            _ => cx.propagate(),
        }
    }

    /// ArrowLeft in a side menu closes it.
    fn on_left(&mut self, _: &PickerLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.option_menu.take().is_some() {
            return cx.notify();
        }
        cx.propagate();
    }

    fn on_close(&mut self, _: &PickerClose, _: &mut Window, cx: &mut Context<Self>) {
        // Escape closes an open side menu first, as its own menu would.
        if self.option_menu.take().is_some() {
            return cx.notify();
        }
        cx.emit(ModelPickerEvent::Close);
    }

    fn on_backspace(&mut self, _: &PickerBackspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.query.pop().is_some() {
            if self.marked_from.is_some_and(|from| from >= self.query.len()) {
                self.marked_from = None;
            }
            self.highlighted = 0;
            self.rebuild_rows();
            cx.notify();
        }
    }

    fn on_previous_provider(&mut self, _: &PreviousProvider, _: &mut Window, cx: &mut Context<Self>) {
        self.adjacent_provider(-1, cx);
    }

    fn on_next_provider(&mut self, _: &NextProvider, _: &mut Window, cx: &mut Context<Self>) {
        self.adjacent_provider(1, cx);
    }

    /// The search field's text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Types into the search field (also used by scripted tests).
    pub fn type_query(&mut self, text: &str, cx: &mut Context<Self>) {
        self.effort_focus = None;
        self.option_menu = None;
        self.query.push_str(text);
        self.highlighted = 0;
        self.rebuild_rows();
        cx.notify();
    }

    fn render_rail(&self, colors: &ComposerColors, cx: &mut Context<Self>) -> impl IntoElement {
        let c = *colors;
        let entries = self.rail_entries();
        let selected_model = self.selection.model.clone();
        let button = |id: SharedString, selected: bool, disabled: bool, content: AnyElement, tab: PickerTab, cx: &mut Context<Self>| {
            div()
                .id(id)
                .relative()
                .w_full()
                .h(c.r(px(36.)))
                .flex()
                .items_center()
                .justify_center()
                .rounded(c.r(px(6.)))
                .cursor_pointer()
                .when(disabled, |el| el.opacity(0.5))
                .when(!disabled, |el| el.hover(move |s| s.bg(c.row_hover)))
                .child(content)
                .when(selected, |el| {
                    el.child(div().absolute().right(px(0.)).top(c.r(px(8.))).w(c.r(px(3.))).h(c.r(px(20.))).rounded_full().bg(c.primary))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !disabled {
                        this.select_tab(tab.clone(), cx);
                    }
                }))
        };
        let mut children: Vec<AnyElement> = Vec::new();
        children.push(
            button(
                "picker-rail-favorites".into(),
                self.tab == PickerTab::Favorites,
                false,
                Icon::new(IconName::StarFilled).size(IconSize::Custom(c.r(px(20.)))).color(c.foreground).into_any_element(),
                PickerTab::Favorites,
                cx,
            )
            .into_any_element(),
        );
        children.push(div().h(px(1.)).w_full().bg(c.border.opacity(0.7)).into_any_element());
        for entry in entries {
            let selected = self.tab == PickerTab::Instance(entry.instance_id.clone());
            let model = (entry.instance_id == self.selection.instance_id).then(|| selected_model.as_ref());
            let mark = ProviderIcon::for_model(&entry.driver_kind, model);
            let content = div()
                .size(c.r(px(24.)))
                .flex()
                .items_center()
                .justify_center()
                .child(provider_icon(mark, c.r(px(20.)), &entry.display_name, &c, cx))
                .into_any_element();
            children.push(
                button(
                    format!("picker-rail-{}", entry.instance_id.0).into(),
                    selected,
                    !entry.ready,
                    content,
                    PickerTab::Instance(entry.instance_id.clone()),
                    cx,
                )
                .into_any_element(),
            );
        }
        div()
            .w(c.r(MODEL_PICKER_RAIL_WIDTH))
            .flex_none()
            .h_full()
            .bg(c.muted.opacity(0.3))
            .p(c.r(px(4.)))
            .flex()
            .flex_col()
            .gap(c.r(px(4.)))
            .children(children)
    }

    fn render_row(&self, index: usize, row: &Row, colors: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let c = *colors;
        let highlighted = index == self.highlighted;
        let hovered = self.hovered_row == Some(index);
        let base = div()
            .id(SharedString::from(format!("picker-row-{index}")))
            // A rem-tall option and a 2px gap (the Mac capture's rows sit exactly 32px apart at a 15px rem).
            .h(c.r(MODEL_OPTION_HEIGHT) + px(2.))
            .pb(px(2.))
            .child(
                div()
                    .h(c.r(MODEL_OPTION_HEIGHT))
                    .px(c.r(px(8.)))
                    .flex()
                    .items_center()
                    .gap(c.r(px(8.)))
                    .rounded(c.r(px(4.)))
                    .cursor_pointer()
                    .when(highlighted || hovered, |el| el.bg(c.foreground.opacity(0.09)))
                    .child(match row {
                        Row::Legacy { expanded } => div()
                            .text_size(c.r(px(14.)))
                            .text_color(if highlighted { c.foreground } else { c.muted_foreground })
                            .child(if *expanded { "Show less" } else { "Show more" })
                            .into_any_element(),
                        Row::Model(item) => self.render_model_row(index, item, highlighted || hovered, &c, cx),
                    }),
            )
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.hovered_row != Some(index) {
                    this.hovered_row = Some(index);
                    this.highlighted = index;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.pick_row(index, cx)));
        base.test_support().into_any_element()
    }

    fn render_model_row(&self, index: usize, item: &PickerItem, active: bool, c: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let selected = item.instance_id == self.selection.instance_id && item.model.slug == self.selection.model;
        let favorite = self.catalog.is_favorite(&item.instance_id, &item.model.slug);
        let in_favorites = self.tab == PickerTab::Favorites && self.query.trim().is_empty();
        let show_star = (favorite && !in_favorites) || active;
        let mark = ProviderIcon::for_model(&item.entry.driver_kind, Some(&item.model.slug));
        let instance = item.instance_id.clone();
        let slug = item.model.slug.clone();
        let jump = (index < 9).then(|| format!("{}{}", if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" }, index + 1));
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(c.r(px(8.)))
            .child(provider_icon(mark, c.r(px(16.)), &item.entry.display_name, c, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(c.r(px(8.)))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(c.r(px(14.)))
                            .line_height(c.r(px(19.25)))
                            .text_color(c.foreground)
                            .child(display_model_name(&item.model, false)),
                    )
                    .when(selected, |el| el.child(Icon::new(IconName::Check).size(IconSize::Custom(c.r(px(14.)))).color(c.foreground).stroke(1.8)))
                    .when_some(item.model.sub_provider.clone(), |el, sub| {
                        el.child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_size(c.r(px(12.))).text_color(c.muted_foreground.opacity(0.7)).child(sub))
                    })
                    .when(item.model.is_unavailable, |el| {
                        el.child(
                            div()
                                .px(c.r(px(6.)))
                                .h(c.r(px(18.)))
                                .flex()
                                .items_center()
                                .rounded(c.r(px(4.)))
                                .border_1()
                                .border_color(c.border)
                                .text_size(c.r(px(11.)))
                                .text_color(c.muted_foreground)
                                .child("Unavailable"),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(c.r(px(6.)))
                    .child(
                        div()
                            .id(SharedString::from(format!("picker-star-{index}")))
                            .size(c.r(px(24.)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(c.r(px(6.)))
                            .hover(move |s| s.bg(c.row_hover))
                            .when(show_star, |el| {
                                el.child(if favorite {
                                    Icon::new(IconName::StarFilled).size(IconSize::Custom(c.r(px(12.)))).color(c.warning).into_any_element()
                                } else {
                                    Icon::new(IconName::Star).size(IconSize::Custom(c.r(px(12.)))).color(c.muted_foreground).into_any_element()
                                })
                            })
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(ModelPickerEvent::ToggleFavorite { instance_id: instance.clone(), model: slug.clone() });
                            })),
                    )
                    .when_some(jump, |el, jump| el.child(kbd(jump, *c))),
            )
            .into_any_element()
    }

    fn render_effort_column(&self, c: &ComposerColors, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = *c;
        let model = self.catalog.model(&self.selection.instance_id, &self.selection.model).cloned();
        let label = model.as_ref().map(|m| display_model_name(m, true)).unwrap_or_default();
        let traits = self.traits();
        let reasoning = traits.as_ref().and_then(|t| t.reasoning().cloned());
        let has_others = traits.as_ref().is_some_and(|t| !t.others().is_empty());
        let heading: SharedString = match &reasoning {
            Some(d) => d.label().clone(),
            None if has_others => crate::i18n::t!("Options", context = "heading for model options"),
            None => crate::i18n::t!("Effort", context = "heading for reasoning effort levels"),
        };
        let rows = self.column_rows();
        let levels = rows.iter().take_while(|r| matches!(r, ColumnRow::Level(..) | ColumnRow::State(..))).count();
        let mut level_rows = Vec::new();
        let mut other_rows = Vec::new();
        for (i, row) in rows.into_iter().enumerate() {
            let element = self.render_column_row(i, row, c, window, cx);
            if i < levels { level_rows.push(element) } else { other_rows.push(element) }
        }
        let empty = level_rows.is_empty() && other_rows.is_empty();
        let has_levels = !level_rows.is_empty();
        let column_bounds = self.column_bounds.clone();
        div()
            .relative()
            .w(c.r(MODEL_PICKER_EFFORT_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .child(gpui_kit::canvas(move |bounds, _, _| column_bounds.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0())
            .border_l_1()
            .border_color(c.border.opacity(0.7))
            .bg(c.muted.opacity(0.4))
            .child(
                div().flex_none().px(c.r(px(12.))).pt(c.r(px(10.))).child(
                    div()
                        .h(c.r(px(33.)))
                        .flex()
                        .items_center()
                        .gap(c.r(px(8.)))
                        .border_b_1()
                        .border_color(c.border.opacity(0.7))
                        .pb(c.r(px(6.)))
                        .text_size(c.r(px(14.)))
                        .child(div().text_color(c.muted_foreground).child(heading))
                        .child(div().ml_auto().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(c.r(px(12.))).text_color(c.muted_foreground).child(label)),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(c.r(px(2.)))
                    .px(c.r(px(8.)))
                    .pt(c.r(px(10.)))
                    .pb(c.r(px(6.)))
                    .when(empty, |el| {
                        el.child(div().px(c.r(px(8.))).h(c.r(px(32.))).flex().items_center().text_size(c.r(px(14.))).text_color(c.muted_foreground).child(crate::i18n::t!("No effort levels")))
                    })
                    .children(level_rows)
                    .when(!other_rows.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(c.r(px(2.)))
                                .when(has_levels, |el| el.mt(c.r(px(4.))).border_t_1().border_color(c.border.opacity(0.7)).pt(c.r(px(4.))))
                                .children(other_rows),
                        )
                    }),
            )
    }

    /// One column row: `ColumnRow` for a level, `OptionMenuRow` / `OptionSwitchRow` for others.
    fn render_column_row(&self, index: usize, row: ColumnRow, c: ComposerColors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.effort_focus == Some(index);
        let base = div()
            .id(SharedString::from(format!("effort-{index}")))
            .relative()
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(c.r(px(8.)))
            .px(c.r(px(8.)))
            .rounded(c.r(px(4.)))
            .cursor_pointer()
            .hover(move |s| s.bg(c.row_hover))
            .when(focused, |el| el.bg(c.row_hover))
            .text_size(c.r(px(14.)))
            .text_color(c.foreground);
        // `OptionRowText`: the option's name over its current value.
        let option_text = |name: SharedString, value: SharedString| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().truncate().text_size(c.r(px(12.))).line_height(c.r(px(16.))).text_color(c.muted_foreground).child(name))
                .child(div().truncate().line_height(c.r(px(20.))).child(value))
        };
        let pick = row.clone();
        let element = match row {
            ColumnRow::Level(d, option) => {
                let selected = d.value() == Some(&option.id);
                level_row(base.h(c.r(px(32.))), option.label.clone(), selected, option.is_default, c)
            }
            ColumnRow::State(id, on) => {
                let current = self.traits().and_then(|t| t.descriptors.into_iter().find(|d| d.id() == &id)).is_some_and(|d| matches!(d, OptionDescriptor::Boolean(b) if b.current == Some(true)));
                let label = if on { crate::i18n::t!("On", context = "option state") } else { crate::i18n::t!("Off", context = "option state") };
                level_row(base.h(c.r(px(32.))), label, current == on, false, c)
            }
            ColumnRow::Menu(d) => {
                let open = self.option_menu.as_ref().is_some_and(|(id, _)| id == &d.id);
                let value = d.value_label().unwrap_or_default();
                base.py(c.r(px(4.)))
                    .when(open, |el| el.bg(c.row_hover))
                    .child(option_text(d.label.clone(), value))
                    .child(div().mr(c.r(px(-4.))).child(Icon::new(IconName::ChevronRight).size(IconSize::Custom(c.r(px(14.)))).color(c.muted_foreground)))
                    .when(open, |el| el.child(self.render_option_menu(&d, c, window, cx)))
            }
            ColumnRow::Switch(id, name, on) => {
                let label = if on { crate::i18n::t!("On", context = "option state") } else { crate::i18n::t!("Off", context = "option state") };
                let toggle = id.clone();
                base.py(c.r(px(4.))).child(option_text(name, label)).child(
                    crate::components::ui::switch::Switch::new(SharedString::from(format!("option-switch-{id}")), on)
                        .size(crate::components::ui::switch::SwitchSize::Sm)
                        .on_change({
                            let picker = cx.entity().downgrade();
                            move |next, _, cx| {
                                picker.update(cx, |this, cx| this.set_option(&toggle, OptionValue::Bool(next), cx)).ok();
                            }
                        }),
                )
            }
        };
        element
            .on_click(cx.listener(move |this, _, _, cx| {
                this.effort_focus = Some(index);
                this.activate_row(pick.clone(), cx);
            }))
            .test_support()
            .into_any_element()
    }

    /// An option's side menu (`MenuPopup side="right" align="start" sideOffset={6}`, `max-w-72`):
    /// its choices as radio items with their descriptions and the default marked.
    fn render_option_menu(&self, d: &SelectDescriptor, c: ComposerColors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let value = d.value().cloned();
        // Base UI flips the menu to the column's left when it would cross the viewport less its
        // 5px collision padding. The width is the popup's: `min-w-40`, its rows' content, `max-w-72`.
        let width = {
            let mut text = |t: &str, size: f32| crate::components::chat::composer::text_width(t, c.r(px(size)), FontWeight::NORMAL, window);
            let default_label = crate::i18n::t!("Default", context = "marks the default option");
            let widest = d
                .selectable()
                .iter()
                .map(|o| {
                    let wraps = o.description.is_some();
                    let mut w = text(&o.label, 14.) + c.r(px(12.)) + c.r(px(14.));
                    if o.is_default {
                        w += c.r(px(12.)) + text(&default_label, 12.);
                    }
                    if wraps { c.r(px(288.)) } else { w }
                })
                .fold(px(0.), |a, b| a.max(b));
            (widest + c.r(px(16.)) + c.r(px(8.)) + px(2.)).clamp(c.r(px(160.)), c.r(px(288.)))
        };
        let flip = self.column_bounds.get().is_some_and(|b| b.right() + c.r(px(6.)) + width > window.viewport_size().width - px(5.));
        let focus = self.option_menu.as_ref().and_then(|(_, at)| *at);
        let items = d.selectable().into_iter().enumerate().map(|(i, choice)| {
            let checked = value.as_ref() == Some(&choice.id);
            let (id, pick) = (d.id.clone(), choice.id.clone());
            div()
                .id(SharedString::from(format!("option-menu-item-{i}")))
                .flex()
                .items_center()
                .gap(c.r(px(12.)))
                .min_h(c.r(px(28.)))
                .px(c.r(px(8.)))
                .py(c.r(px(4.)))
                .rounded(c.r(px(4.)))
                .cursor_pointer()
                .text_size(c.r(px(14.)))
                .line_height(c.r(px(20.)))
                .text_color(c.foreground)
                .when(checked, |el| el.bg(c.foreground.opacity(0.08)))
                .when(focus == Some(i), |el| el.bg(c.accent).text_color(c.accent_foreground))
                .hover(move |s| s.bg(c.accent).text_color(c.accent_foreground))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().child(choice.label.clone()))
                        .children(choice.description.clone().map(|text| {
                            div().text_size(c.r(px(12.))).line_height(c.r(px(16.))).line_clamp(2).text_color(c.muted_foreground).child(text)
                        })),
                )
                .when(choice.is_default, |el| {
                    el.child(div().flex_none().text_size(c.r(px(12.))).line_height(c.r(px(16.))).text_color(c.muted_foreground.opacity(0.7)).child(crate::i18n::t!("Default", context = "marks the default option")))
                })
                .when(checked, |el| el.child(div().flex_none().child(Icon::new(IconName::Check).size(IconSize::Custom(c.r(px(14.)))).color(c.foreground))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.option_menu = None;
                    if this.selection.traits(&this.catalog).and_then(|t| t.descriptors.into_iter().find(|d| d.id() == &id)).and_then(|d| d.value()) != Some(OptionValue::Text(pick.clone())) {
                        this.set_option(&id, OptionValue::Text(pick.clone()), cx);
                    }
                    cx.notify();
                }))
                .test_support()
        });
        let menu = div()
            .id("model-picker-option-menu")
            .occlude()
            .min_w(c.r(px(160.)))
            .max_w(c.r(px(288.)))
            .rounded(POPUP_RADIUS)
            .border_1()
            .border_color(c.popup_border)
            .glass(c.popup_background)
            .shadow(popup_shadow(&c))
            .p(c.r(px(4.)))
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.option_menu = None;
                cx.notify();
            }))
            .children(items)
            .child({
                let cell = self.option_menu_bounds.clone();
                gpui_kit::canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0()
            })
            .test_support();
        let anchor = div().absolute().top_0();
        let anchor = if flip { anchor.right_full().pr(c.r(px(6.))) } else { anchor.left_full().pl(c.r(px(6.))) };
        gpui_kit::deferred(anchor.child(menu)).with_priority(3).into_any_element()
    }

    fn render_footer(&self, c: &ComposerColors, cx: &mut Context<Self>) -> impl IntoElement {
        let c = *c;
        let segment = |id: SharedString, checked: bool, content: AnyElement| {
            div()
                .id(id)
                .h(c.r(px(24.)))
                .px(c.r(px(8.)))
                .flex()
                .items_center()
                .gap(c.r(px(4.)))
                .rounded(c.r(px(6.)))
                .cursor_pointer()
                .text_size(c.r(px(12.)))
                .text_color(if checked { c.foreground } else { c.muted_foreground })
                .when(checked, |el| el.bg(c.input.opacity(0.72)))
                .child(content)
        };
        let traits = self.traits();
        let group = || div().flex().rounded(c.r(px(8.))).bg(c.input.opacity(0.4)).p(c.r(px(2.)));
        let bolt = |speed: &str, checked: bool| {
            matches!(speed, "Fast" | "Ultrafast").then(|| Icon::new(IconName::ZapFilled).size(IconSize::Custom(c.r(px(12.)))).color(if checked { c.foreground } else { c.muted_foreground }))
        };
        // `SpeedControl`: a service tier's levels, else Fast mode as Standard / Fast.
        let speed = traits.as_ref().and_then(|t| {
            if let Some(tier) = t.speed_select() {
                let current = tier.value().cloned();
                let id = tier.id.clone();
                return Some(group().children(tier.selectable().into_iter().enumerate().map(|(i, option)| {
                    let checked = current.as_ref() == Some(&option.id);
                    let (id, value) = (id.clone(), option.id.clone());
                    segment(
                        format!("speed-{i}").into(),
                        checked,
                        div().flex().items_center().gap(c.r(px(4.))).children(bolt(&option.label, checked)).child(option.label.clone()).into_any_element(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.set_option(&id, OptionValue::Text(value.clone()), cx)))
                    .test_support()
                })));
            }
            let fast = t.fast_toggle()?;
            let on = fast.current == Some(true);
            let id = fast.id.clone();
            Some(group().children([false, true].into_iter().map(|value| {
                let checked = on == value;
                let (speed, label) = if value {
                    ("Fast", crate::i18n::t!("Fast", context = "response speed"))
                } else {
                    ("Standard", crate::i18n::t!("Standard", context = "response speed"))
                };
                let id = id.clone();
                segment(
                    format!("speed-{}", usize::from(value)).into(),
                    checked,
                    div().flex().items_center().gap(c.r(px(4.))).children(bolt(speed, checked)).child(label).into_any_element(),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_option(&id, OptionValue::Bool(value), cx)))
                .test_support()
            })))
        });
        let ultracode = traits.as_ref().and_then(|t| t.ultracode_toggle().cloned()).map(|toggle| {
            let on = toggle.current == Some(true);
            group().child(
                segment(
                    "ultracode".into(),
                    on,
                    div().flex().items_center().gap(c.r(px(4.))).child(Icon::new(IconName::Workflow).size(IconSize::Custom(c.r(px(12.)))).color(if on { c.foreground } else { c.muted_foreground })).child(toggle.label.clone()).into_any_element(),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_option(&toggle.id, OptionValue::Bool(!on), cx)))
                .test_support(),
            )
        });
        let mode = self.runtime_mode;
        div()
            .flex()
            .h(c.r(FOOTER_HEIGHT))
            .flex_none()
            .items_center()
            .gap(c.r(px(8.)))
            .border_t_1()
            .border_color(c.border.opacity(0.7))
            .bg(c.muted.opacity(0.4))
            .px(c.r(px(12.)))
            .py(c.r(px(8.)))
            .children(speed)
            .children(ultracode)
            .child(
                div()
                    .id("picker-permissions")
                    .relative()
                    .ml_auto()
                    .h(c.r(px(28.)))
                    .px(c.r(px(8.)))
                    .flex()
                    .items_center()
                    .gap(c.r(px(6.)))
                    .rounded(c.r(px(6.)))
                    .cursor_pointer()
                    .text_size(c.r(px(12.)))
                    .text_color(c.muted_foreground)
                    .hover(move |s| s.bg(c.row_hover).text_color(c.foreground))
                    .child(Icon::new(runtime_mode_icon(mode)).size(IconSize::Custom(c.r(px(14.)))).color(c.muted_foreground).stroke(composer_icon_stroke(runtime_mode_icon(mode))))
                    .child(mode.label())
                    .child(Icon::new(IconName::ChevronUp).size(IconSize::Custom(c.r(px(12.)))).color(c.muted_foreground))
                    .when(self.permissions_open, |d| d.bg(c.row_hover).text_color(c.foreground))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.permissions_open = !this.permissions_open;
                        this.permissions_preview = None;
                        cx.notify();
                    }))
                    .children(self.render_permissions_menu(c, cx)),
            )
    }

    /// `PermissionsMenu` (`side="top" align="end"`) above the footer trigger.
    fn render_permissions_menu(&self, c: ComposerColors, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if !self.permissions_open {
            return None;
        }
        let panel = permissions_panel(
            self.runtime_mode,
            self.permissions_preview,
            c,
            cx,
            |this: &mut Self, mode| this.permissions_preview = mode,
            |this, mode, cx| {
                this.permissions_open = false;
                this.permissions_preview = None;
                if this.runtime_mode != mode {
                    this.runtime_mode = mode;
                    cx.emit(ModelPickerEvent::RuntimeModeChanged(mode));
                }
            },
            |this| this.permissions_open = false,
        );
        Some(
            gpui_kit::deferred(div().absolute().bottom_full().right_0().pb(c.r(px(4.))).child(panel))
                .with_priority(3)
                .into_any_element(),
        )
    }
}

/// `ComposerPermissionsSection` in its menu popup: a `w-72` panel with a segmented row of the
/// four modes, then the shown mode's name and its two-line description. The picker footer and
/// the composer's permissions chip both open it; `preview` is the mode a hover shows.
pub fn permissions_panel<V: 'static>(
    current: RuntimeMode,
    preview: Option<RuntimeMode>,
    c: ComposerColors,
    cx: &mut Context<V>,
    on_hover: fn(&mut V, Option<RuntimeMode>),
    on_pick: fn(&mut V, RuntimeMode, &mut Context<V>),
    on_close: fn(&mut V),
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let shown = preview.unwrap_or(current);
    let segments = RuntimeMode::ALL.iter().copied().enumerate().map(|(i, mode)| {
        let checked = mode == current;
        let last = i + 1 == RuntimeMode::ALL.len();
        div()
            .id(("permission-mode", i))
            .flex()
            .flex_1()
            .h(c.r(px(24.)))
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(i == 0, |d| d.rounded_l(c.r(px(6.))))
            .when(last, |d| d.rounded_r(c.r(px(6.))))
            .when(checked, |d| d.bg(c.input.opacity(0.72)))
            .when(!checked, |d| d.hover(move |s| s.bg(c.accent)))
            .child(
                Icon::new(runtime_mode_icon(mode))
                    .size(IconSize::Custom(c.r(px(14.))))
                    .color(if checked { c.foreground } else { c.muted_foreground })
                    .stroke(composer_icon_stroke(runtime_mode_icon(mode))),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                on_hover(this, hovered.then_some(mode));
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                on_pick(this, mode, cx);
                cx.notify();
            }))
    });
    div()
        .id("permissions-menu")
        .occlude()
        .rounded(POPUP_RADIUS)
        .border_1()
        .border_color(c.popup_border)
        .glass(c.popup_background)
        .shadow(popup_shadow(&c))
        .p(c.r(px(4.)))
        .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
            on_close(this);
            cx.notify();
        }))
        .child(
            div()
                .w(c.r(px(288.)))
                .flex()
                .flex_col()
                .gap(c.r(px(6.)))
                .px(c.r(px(8.)))
                .py(c.r(px(6.)))
                .child(
                    div()
                        .text_size(c.r(px(12.)))
                        .line_height(c.r(px(16.)))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.muted_foreground)
                        .child(crate::i18n::t!("Permissions")),
                )
                .child(div().flex().rounded(c.r(px(8.))).bg(c.input.opacity(0.4)).p(c.r(px(2.))).children(segments))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(c.r(px(2.)))
                        .child(
                            div()
                                .text_size(c.r(px(14.)))
                                .line_height(c.r(px(20.)))
                                .text_color(if shown == current { c.foreground } else { c.muted_foreground })
                                .child(shown.label()),
                        )
                        .child(
                            div()
                                .min_h(c.r(px(32.)))
                                .text_size(c.r(px(12.)))
                                .line_height(c.r(px(16.)))
                                .text_color(c.muted_foreground)
                                .child(shown.description()),
                        ),
                ),
        )
}

/// A level row's label and its check or "Default" mark (`ColumnRow`).
fn level_row(row: gpui_kit::Stateful<gpui_kit::Div>, label: SharedString, selected: bool, is_default: bool, c: ComposerColors) -> gpui_kit::Stateful<gpui_kit::Div> {
    row.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label)).map(|el| {
        if selected {
            el.child(Icon::new(IconName::Check).size(IconSize::Custom(c.r(px(14.)))).color(c.foreground).stroke(1.8))
        } else if is_default {
            el.child(div().text_size(c.r(px(12.))).text_color(c.muted_foreground.opacity(0.7)).child(crate::i18n::t!("Default", context = "marks the default option")))
        } else {
            el
        }
    })
}

/// A `Kbd` hint: `h-5 min-w-5 rounded px-1 text-xs font-medium`.
pub fn kbd(label: impl Into<SharedString>, c: ComposerColors) -> impl IntoElement {
    div()
        .h(c.r(px(20.)))
        .min_w(c.r(px(20.)))
        .px(c.r(px(4.)))
        .flex()
        .items_center()
        .justify_center()
        .rounded(c.r(px(4.)))
        .bg(c.kbd_background)
        .text_size(c.r(px(12.)))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c.muted_foreground)
        .child(label.into())
}

/// The popup shadow: `0 18px 44px -18px rgb(0 0 0 / 80%)` (dark), 55% (light).
pub fn popup_shadow(c: &ComposerColors) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., if c.is_dark() { 0.8 } else { 0.55 }),
        offset: point(px(0.), px(if c.is_dark() { 18. } else { 16. })),
        blur_radius: px(if c.is_dark() { 44. } else { 40. }),
        spread_radius: px(-18.),
        inset: false,
    }]
}

impl Render for ModelPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `focus-within:border-ring`: the search field owns the picker's focus.
        let search_focused = self.focus_handle.is_focused(window);
        let c = ComposerColors::new(cx);
        let rows = self.rows.clone();
        let empty = rows.is_empty();
        let empty_label = if self.tab == PickerTab::Favorites && self.query.trim().is_empty() { "No favorites yet" } else { "No models found" };
        let show_rail = self.query.trim().is_empty();
        let query = self.query.clone();
        let list = uniform_list(
            "model-picker-list",
            rows.len(),
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let colors = ComposerColors::new(cx);
                range.map(|i| this.render_row(i, &rows[i], &colors, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .py(c.r(px(6.)))
        .px(c.r(px(4.)));
        let search = div().flex_none().px(c.r(px(12.))).pt(c.r(px(10.))).child(
            div()
                .border_b_1()
                .border_color(if search_focused { c.ring } else { c.border.opacity(0.7) })
                .pb(c.r(px(6.)))
                .relative()
                .h(c.r(px(26.)) + c.r(px(7.)))
                .flex()
                .items_center()
                .child(div().absolute().left(px(0.)).child(Icon::new(IconName::Search).size(IconSize::Custom(c.r(px(16.)))).color(c.muted_foreground.opacity(0.55))))
                .child(
                    div()
                        .pl(c.r(px(20.)))
                        .h(c.r(px(26.)))
                        .flex()
                        .items_center()
                        .text_size(c.r(px(14.)))
                        .text_color(if query.is_empty() { c.placeholder } else { c.foreground })
                        .child(if query.is_empty() { SharedString::from("Search models...") } else { query.clone().into() })
                        .when(!query.is_empty(), |el| el.child(div().w(c.r(px(1.5))).h(c.r(px(17.))).bg(c.foreground))),
                ),
        );
        div()
            .id("model-picker")
            .key_context(PICKER_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_backspace))
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .on_action(cx.listener(Self::on_previous_provider))
            .on_action(cx.listener(Self::on_next_provider))
            // The shortcut registry's commands (`modelPicker.*`), which users may rebind.
            .on_action(cx.listener(|this, _: &commands::PreviousModelProvider, _, cx| this.adjacent_provider(-1, cx)))
            .on_action(cx.listener(|this, _: &commands::NextModelProvider, _, cx| this.adjacent_provider(1, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel1, _, cx| this.jump(0, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel2, _, cx| this.jump(1, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel3, _, cx| this.jump(2, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel4, _, cx| this.jump(3, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel5, _, cx| this.jump(4, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel6, _, cx| this.jump(5, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel7, _, cx| this.jump(6, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel8, _, cx| this.jump(7, cx)))
            .on_action(cx.listener(|this, _: &commands::JumpToModel9, _, cx| this.jump(8, cx)))
            .on_mouse_down_out(cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                let in_menu = this.option_menu.is_some() && this.option_menu_bounds.get().is_some_and(|b| b.contains(&event.position));
                if !in_menu {
                    cx.emit(ModelPickerEvent::Close);
                }
            }))
            .font_family(ui_font_family())
            .flex()
            .flex_col()
            .glass(c.popup_background)
            .border_1()
            .border_color(c.popup_border)
            .w(model_picker_size(self.plain, &c).width)
            .h(model_picker_size(self.plain, &c).height)
            .rounded(POPUP_RADIUS)
            .shadow(popup_shadow(&c))
            .overflow_hidden()
            .child(
                div()
                    .h(c.r(MODEL_PICKER_HEIGHT))
                    .flex()
                    .child(
                        div()
                            .w(c.r(MODEL_PICKER_LIST_WIDTH))
                            .h_full()
                            .flex()
                            .when(show_rail, |el| el.child(self.render_rail(&c, cx)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .bg(c.muted.opacity(0.4))
                                    .when(show_rail, |el| el.border_l_1().border_color(c.border.opacity(0.7)))
                                    .child(search)
                                    .map(|el| {
                                        if empty {
                                            el.child(div().p(c.r(px(12.))).text_size(c.r(px(12.))).text_color(c.muted_foreground).child(empty_label))
                                        } else {
                                            el.child(list)
                                        }
                                    }),
                            ),
                    )
                    .when(!self.plain, |el| el.child(self.render_effort_column(&c, window, cx))),
            )
            .when(!self.plain, |el| el.child(self.render_footer(&c, cx)))
    }
}

impl EntityInputHandler for ModelPicker {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        Some(self.query.clone())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let end = self.query.encode_utf16().count();
        Some(UTF16Selection { range: end..end, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let from = self.marked_from?;
        let start = self.query[..from].encode_utf16().count();
        Some(start..self.query.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_from = None;
    }

    /// Commits text, replacing what an input method was composing.
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(from) = self.marked_from.take() {
            self.query.truncate(from);
        }
        self.type_query(text, cx);
    }

    /// Shows an input method's uncommitted text in the search, replacing its previous version.
    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let from = self.marked_from.unwrap_or(self.query.len());
        self.query.truncate(from);
        self.type_query(text, cx);
        self.marked_from = (!text.is_empty()).then_some(from);
    }

    fn bounds_for_range(&mut self, _: Range<usize>, bounds: gpui_kit::Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<gpui_kit::Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(&mut self, _: gpui_kit::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// Registers the picker's text input for the focused frame. The picker draws
/// its own query, so a zero-size canvas carries the handler.
pub fn input_handler_canvas(picker: Entity<ModelPicker>) -> impl IntoElement {
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let focus = picker.read(cx).focus_handle.clone();
            window.handle_input(&focus, gpui_kit::ElementInputHandler::new(bounds, picker.clone()), cx);
        },
    )
    .size_0()
}
