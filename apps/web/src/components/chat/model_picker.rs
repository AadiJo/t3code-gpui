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
    POPUP_RADIUS, UI_FONT_FAMILY,
};
use crate::model_ordering::{ProviderModelItem, order_claude_before_openai, sort_provider_model_items};
use crate::model_search::{SearchableModel, rank_model_search};
use crate::model_selection::{
    ModelCatalog, ModelOption, ModelSelection, ProviderIcon, ProviderInstanceEntry, ProviderInstanceId,
    display_model_name,
};

/// Row height in the virtualized list: a 32px option (`py-1` around the
/// 24px icon-and-label content) and the list's 2px gap.
pub const MODEL_ROW_HEIGHT: Pixels = px(34.);
/// One option's own height.
const MODEL_OPTION_HEIGHT: Pixels = px(32.);
const PICKER_CONTEXT: &str = "ModelPicker";
/// Footer: `py-2` around a 28px control, plus its 1px top border.
const FOOTER_HEIGHT: Pixels = px(45.);
/// The popup's outer size: list, effort column, footer and the 1px border.
/// Fixed, so the composer can cache the picker's subtree with a known layout.
pub const MODEL_PICKER_SIZE: gpui_kit::Size<Pixels> = gpui_kit::Size {
    width: px(360. + 192. + 2.),
    height: px(320. + 45. + 2.),
};

actions!(model_picker, [PickerUp, PickerDown, PickerConfirm, PickerClose, PickerBackspace, PreviousProvider, NextProvider]);

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
        };
        this.rebuild_rows();
        this.highlight_selected();
        this
    }

    /// Updates the catalog and selection from the composer.
    pub fn update_state(&mut self, catalog: Arc<ModelCatalog>, selection: ModelSelection, runtime_mode: RuntimeMode, cx: &mut Context<Self>) {
        self.catalog = catalog;
        self.selection = selection;
        self.runtime_mode = runtime_mode;
        self.rebuild_rows();
        cx.notify();
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
                let effort = item
                    .model
                    .effort
                    .as_ref()
                    .and_then(|d| d.options.iter().find(|o| o.is_default).or(d.options.first()))
                    .map(|o| o.id.clone());
                let same = item.instance_id == self.selection.instance_id && item.model.slug == self.selection.model;
                let selection = ModelSelection {
                    instance_id: item.instance_id,
                    model: item.model.slug,
                    effort: if same { self.selection.effort.clone() } else { effort },
                    speed: if same { self.selection.speed.clone() } else { None },
                    ultracode: same && self.selection.ultracode,
                };
                self.selection = selection.clone();
                cx.emit(ModelPickerEvent::Select(selection));
                cx.notify();
            }
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
        if !self.rows.is_empty() {
            self.highlighted = (self.highlighted + self.rows.len() - 1) % self.rows.len();
            self.scroll.scroll_to_item(self.highlighted, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn on_down(&mut self, _: &PickerDown, _: &mut Window, cx: &mut Context<Self>) {
        if !self.rows.is_empty() {
            self.highlighted = (self.highlighted + 1) % self.rows.len();
            self.scroll.scroll_to_item(self.highlighted, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn on_confirm(&mut self, _: &PickerConfirm, _: &mut Window, cx: &mut Context<Self>) {
        self.pick_row(self.highlighted, cx);
    }

    fn on_close(&mut self, _: &PickerClose, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ModelPickerEvent::Close);
    }

    fn on_backspace(&mut self, _: &PickerBackspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.query.pop().is_some() {
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

    /// Types into the search field (also used by scripted tests).
    pub fn type_query(&mut self, text: &str, cx: &mut Context<Self>) {
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
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(disabled, |el| el.opacity(0.5))
                .when(!disabled, |el| el.hover(move |s| s.bg(c.row_hover)))
                .child(content)
                .when(selected, |el| {
                    el.child(div().absolute().right(px(0.)).top(px(8.)).w(px(3.)).h(px(20.)).rounded_full().bg(c.primary))
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
                Icon::new(IconName::StarFilled).size(IconSize::Custom(px(20.))).color(c.foreground).into_any_element(),
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
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .child(provider_icon(mark, px(20.), &entry.display_name, &c, cx))
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
            .w(MODEL_PICKER_RAIL_WIDTH)
            .flex_none()
            .h_full()
            .bg(c.muted.opacity(0.3))
            .p(px(4.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .children(children)
    }

    fn render_row(&self, index: usize, row: &Row, colors: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let c = *colors;
        let highlighted = index == self.highlighted;
        let hovered = self.hovered_row == Some(index);
        let base = div()
            .id(SharedString::from(format!("picker-row-{index}")))
            .h(MODEL_ROW_HEIGHT)
            .pb(px(2.))
            .child(
                div()
                    .h(MODEL_OPTION_HEIGHT)
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(highlighted || hovered, |el| el.bg(c.foreground.opacity(0.09)))
                    .child(match row {
                        Row::Legacy { expanded } => div()
                            .text_size(px(14.))
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
        base.into_any_element()
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
            .gap(px(8.))
            .child(provider_icon(mark, px(16.), &item.entry.display_name, c, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(14.))
                            .line_height(px(19.25))
                            .text_color(c.foreground)
                            .child(display_model_name(&item.model, false)),
                    )
                    .when(selected, |el| el.child(Icon::new(IconName::Check).size(IconSize::Custom(px(14.))).color(c.foreground).stroke(1.8)))
                    .when_some(item.model.sub_provider.clone(), |el, sub| {
                        el.child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(c.muted_foreground.opacity(0.7)).child(sub))
                    })
                    .when(item.model.is_unavailable, |el| {
                        el.child(
                            div()
                                .px(px(6.))
                                .h(px(18.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(c.border)
                                .text_size(px(11.))
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
                    .gap(px(6.))
                    .child(
                        div()
                            .id(SharedString::from(format!("picker-star-{index}")))
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .hover(move |s| s.bg(c.row_hover))
                            .when(show_star, |el| {
                                el.child(if favorite {
                                    Icon::new(IconName::StarFilled).size(IconSize::Custom(px(12.))).color(c.warning).into_any_element()
                                } else {
                                    Icon::new(IconName::Star).size(IconSize::Custom(px(12.))).color(c.muted_foreground).into_any_element()
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

    fn render_effort_column(&self, c: &ComposerColors, cx: &mut Context<Self>) -> impl IntoElement {
        let c = *c;
        let model = self.catalog.model(&self.selection.instance_id, &self.selection.model).cloned();
        let label = model.as_ref().map(|m| display_model_name(m, true)).unwrap_or_default();
        let descriptor = model.as_ref().and_then(|m| m.effort.clone());
        let heading: SharedString = descriptor.as_ref().map_or("Effort".into(), |d| d.label.clone());
        let current = self.selection.effort.clone();
        let rows: Vec<AnyElement> = match descriptor {
            None => vec![div().px(px(8.)).h(px(32.)).flex().items_center().text_size(px(14.)).text_color(c.muted_foreground).child("No options").into_any_element()],
            Some(d) => d
                .options
                .iter()
                .enumerate()
                .map(|(i, option)| {
                    let selected = current.as_ref() == Some(&option.id) || (current.is_none() && option.is_default);
                    let id = option.id.clone();
                    div()
                        .id(SharedString::from(format!("effort-{i}")))
                        .h(px(32.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(8.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(c.row_hover))
                        .text_size(px(14.))
                        .text_color(c.foreground)
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(option.label.clone()))
                        .map(|el| {
                            if selected {
                                el.child(Icon::new(IconName::Check).size(IconSize::Custom(px(14.))).color(c.foreground).stroke(1.8))
                            } else if option.is_default {
                                el.child(div().text_size(px(12.)).text_color(c.muted_foreground.opacity(0.7)).child("Default"))
                            } else {
                                el
                            }
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selection.effort = Some(id.clone());
                            cx.emit(ModelPickerEvent::Select(this.selection.clone()));
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect(),
        };
        div()
            .w(MODEL_PICKER_EFFORT_WIDTH)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(c.border.opacity(0.7))
            .bg(c.muted.opacity(0.4))
            .child(
                div().flex_none().px(px(12.)).pt(px(10.)).child(
                    div()
                        .h(px(33.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .border_b_1()
                        .border_color(c.border.opacity(0.7))
                        .pb(px(6.))
                        .text_size(px(14.))
                        .child(div().text_color(c.muted_foreground).child(heading))
                        .child(div().ml_auto().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(12.)).text_color(c.muted_foreground).child(label)),
                ),
            )
            .child(div().flex().flex_col().gap(px(2.)).px(px(8.)).pt(px(10.)).pb(px(6.)).children(rows))
    }

    fn render_footer(&self, c: &ComposerColors, cx: &mut Context<Self>) -> impl IntoElement {
        let c = *c;
        let model = self.catalog.model(&self.selection.instance_id, &self.selection.model).cloned();
        let segment = |id: SharedString, checked: bool, content: AnyElement| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(if checked { c.foreground } else { c.muted_foreground })
                .when(checked, |el| el.bg(c.input.opacity(0.72)))
                .child(content)
        };
        let speed = model.as_ref().and_then(|m| m.speed.clone()).map(|speed| {
            let current = self.selection.speed.clone();
            div().flex().rounded(px(8.)).bg(c.input.opacity(0.4)).p(px(2.)).children(speed.options.into_iter().enumerate().map(|(i, option)| {
                let checked = current.as_ref() == Some(&option.id) || (current.is_none() && option.is_default);
                let bolt = matches!(option.label.as_ref(), "Fast" | "Ultrafast");
                let id = option.id.clone();
                segment(
                    format!("speed-{i}").into(),
                    checked,
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .when(bolt, |el| el.child(Icon::new(IconName::ZapFilled).size(IconSize::Custom(px(12.))).color(if checked { c.foreground } else { c.muted_foreground })))
                        .child(option.label.clone())
                        .into_any_element(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selection.speed = Some(id.clone());
                    cx.emit(ModelPickerEvent::Select(this.selection.clone()));
                    cx.notify();
                }))
            }))
        });
        let ultracode = model.as_ref().filter(|m| m.ultracode).map(|_| {
            let on = self.selection.ultracode;
            div().flex().rounded(px(8.)).bg(c.input.opacity(0.4)).p(px(2.)).child(
                segment(
                    "ultracode".into(),
                    on,
                    div().flex().items_center().gap(px(4.)).child(Icon::new(IconName::Workflow).size(IconSize::Custom(px(12.))).color(if on { c.foreground } else { c.muted_foreground })).child("Ultracode").into_any_element(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selection.ultracode = !this.selection.ultracode;
                    cx.emit(ModelPickerEvent::Select(this.selection.clone()));
                    cx.notify();
                })),
            )
        });
        let mode = self.runtime_mode;
        div()
            .flex()
            .h(FOOTER_HEIGHT)
            .flex_none()
            .items_center()
            .gap(px(8.))
            .border_t_1()
            .border_color(c.border.opacity(0.7))
            .bg(c.muted.opacity(0.4))
            .px(px(12.))
            .py(px(8.))
            .children(speed)
            .children(ultracode)
            .child(
                div()
                    .id("picker-permissions")
                    .relative()
                    .ml_auto()
                    .h(px(28.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(c.muted_foreground)
                    .hover(move |s| s.bg(c.row_hover).text_color(c.foreground))
                    .child(Icon::new(runtime_mode_icon(mode)).size(IconSize::Custom(px(14.))).color(c.muted_foreground).stroke(composer_icon_stroke(runtime_mode_icon(mode))))
                    .child(mode.label())
                    .child(Icon::new(IconName::ChevronUp).size(IconSize::Custom(px(12.))).color(c.muted_foreground))
                    .when(self.permissions_open, |d| d.bg(c.row_hover).text_color(c.foreground))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.permissions_open = !this.permissions_open;
                        this.permissions_preview = None;
                        cx.notify();
                    }))
                    .children(self.render_permissions_menu(c, cx)),
            )
    }

    /// `PermissionsMenu` (`side="top" align="end"`) holding
    /// `ComposerPermissionsSection`: a `w-72` panel with a segmented row of the
    /// four modes, then the shown mode's name and its two-line description.
    fn render_permissions_menu(&self, c: ComposerColors, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if !self.permissions_open {
            return None;
        }
        let current = self.runtime_mode;
        let shown = self.permissions_preview.unwrap_or(current);
        let segments = RuntimeMode::ALL.iter().copied().enumerate().map(|(i, mode)| {
            let checked = mode == current;
            let last = i + 1 == RuntimeMode::ALL.len();
            div()
                .id(("permission-mode", i))
                .flex()
                .flex_1()
                .h(px(24.))
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(i == 0, |d| d.rounded_l(px(6.)))
                .when(last, |d| d.rounded_r(px(6.)))
                .when(checked, |d| d.bg(c.input.opacity(0.72)))
                .when(!checked, |d| d.hover(move |s| s.bg(c.accent)))
                .child(
                    Icon::new(runtime_mode_icon(mode))
                        .size(IconSize::Custom(px(14.)))
                        .color(if checked { c.foreground } else { c.muted_foreground })
                        .stroke(composer_icon_stroke(runtime_mode_icon(mode))),
                )
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.permissions_preview = hovered.then_some(mode);
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.permissions_open = false;
                    this.permissions_preview = None;
                    if this.runtime_mode != mode {
                        this.runtime_mode = mode;
                        cx.emit(ModelPickerEvent::RuntimeModeChanged(mode));
                    }
                    cx.notify();
                }))
        });
        let panel = div()
            .id("permissions-menu")
            .occlude()
            .rounded(POPUP_RADIUS)
            .border_1()
            .border_color(c.popup_border)
            .glass(c.popup_background)
            .shadow(popup_shadow(&c))
            .p(px(4.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.permissions_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(288.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(6.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(c.muted_foreground)
                            .child(crate::i18n::t!("Permissions")),
                    )
                    .child(div().flex().rounded(px(8.)).bg(c.input.opacity(0.4)).p(px(2.)).children(segments))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(20.))
                                    .text_color(if shown == current { c.foreground } else { c.muted_foreground })
                                    .child(shown.label()),
                            )
                            .child(
                                div()
                                    .min_h(px(32.))
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .text_color(c.muted_foreground)
                                    .child(shown.description()),
                            ),
                    ),
            );
        Some(
            gpui_kit::deferred(div().absolute().bottom_full().right_0().pb(px(4.)).child(panel))
                .with_priority(3)
                .into_any_element(),
        )
    }
}

/// A `Kbd` hint: `h-5 min-w-5 rounded px-1 text-xs font-medium`.
pub fn kbd(label: impl Into<SharedString>, c: ComposerColors) -> impl IntoElement {
    div()
        .h(px(20.))
        .min_w(px(20.))
        .px(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(c.kbd_background)
        .text_size(px(12.))
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
        .py(px(6.))
        .px(px(4.));
        let search = div().flex_none().px(px(12.)).pt(px(10.)).child(
            div()
                .border_b_1()
                .border_color(if search_focused { c.ring } else { c.border.opacity(0.7) })
                .pb(px(6.))
                .relative()
                .h(px(26.) + px(7.))
                .flex()
                .items_center()
                .child(div().absolute().left(px(0.)).child(Icon::new(IconName::Search).size(IconSize::Custom(px(16.))).color(c.muted_foreground.opacity(0.55))))
                .child(
                    div()
                        .pl(px(20.))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .text_size(px(14.))
                        .text_color(if query.is_empty() { c.placeholder } else { c.foreground })
                        .child(if query.is_empty() { SharedString::from("Search models...") } else { query.clone().into() })
                        .when(!query.is_empty(), |el| el.child(div().w(px(1.5)).h(px(17.)).bg(c.foreground))),
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
            .on_action(cx.listener(Self::on_previous_provider))
            .on_action(cx.listener(Self::on_next_provider))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(ModelPickerEvent::Close)))
            .font_family(UI_FONT_FAMILY)
            .flex()
            .flex_col()
            .glass(c.popup_background)
            .border_1()
            .border_color(c.popup_border)
            .w(MODEL_PICKER_SIZE.width)
            .h(MODEL_PICKER_SIZE.height)
            .rounded(POPUP_RADIUS)
            .shadow(popup_shadow(&c))
            .overflow_hidden()
            .child(
                div()
                    .h(MODEL_PICKER_HEIGHT)
                    .flex()
                    .child(
                        div()
                            .w(MODEL_PICKER_LIST_WIDTH)
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
                                            el.child(div().p(px(12.)).text_size(px(12.)).text_color(c.muted_foreground).child(empty_label))
                                        } else {
                                            el.child(list)
                                        }
                                    }),
                            ),
                    )
                    .child(self.render_effort_column(&c, cx)),
            )
            .child(self.render_footer(&c, cx))
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
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.type_query(text, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.type_query(text, cx);
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
