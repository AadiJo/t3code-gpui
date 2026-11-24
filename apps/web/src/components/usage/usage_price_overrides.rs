//! Ports `UsagePriceOverrides.tsx`: the "Custom model prices" dialog. One
//! table of custom prices and model mappings across the environments chosen
//! under "Apply to", saved to each environment's server settings.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Window, div, prelude::*, relative,
};
use t3code_contracts::EnvironmentId;

use super::usage_price_form::{UsagePriceField, is_usage_price_edit};
use super::usage_price_table::{
    UsagePriceDraft, UsagePriceTarget, UsagePriceWriteResult, usage_alias_cell, usage_price_cell, usage_price_patch,
    usage_price_table_changes, usage_price_table_errors,
};
use super::usage_source::UsageSource;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::input::{InputSize, TextInput};
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::components::ui::tooltip::tooltip;
use crate::i18n::t;
use crate::theme_color::with_alpha;
use crate::{ActiveAppTheme as _, AppTheme};

/// The dialog asks its owner to close it.
#[derive(Clone, Debug)]
pub struct UsagePriceOverridesClosed;

/// One editable cell of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Column {
    Model,
    Alias,
    Price(UsagePriceField),
}

/// One cell's input. `value` is the draft text it last showed or sent: typing reaches the
/// draft a frame after the field changes, so only a draft that moves away from `value` for
/// another reason (discard, a rejected edit) is written back into the field.
struct CellInput {
    state: Entity<InputState>,
    _changes: Subscription,
    placeholder: SharedString,
    value: String,
}

/// One save (`SaveAttempt`): what was sent where, and how each destination ended.
struct SaveAttempt {
    drafts: Vec<UsagePriceDraft>,
    destinations: Vec<(EnvironmentId, SharedString)>,
    results: HashMap<EnvironmentId, UsagePriceWriteResult>,
}

/// The dialog view. The page feeds it environments and the models its usage reports.
pub struct UsagePriceOverrides {
    source: Rc<dyn UsageSource>,
    environments: Vec<UsagePriceTarget>,
    /// Models in each environment's usage summary, for the model suggestions.
    usage_models: Vec<(EnvironmentId, Vec<String>)>,
    selected_ids: Option<BTreeSet<EnvironmentId>>,
    drafts: Vec<UsagePriceDraft>,
    pending: bool,
    attempt: Option<SaveAttempt>,
    next_row_id: u64,
    focus_row: Option<SharedString>,
    inputs: HashMap<(SharedString, Column), CellInput>,
}

impl EventEmitter<UsagePriceOverridesClosed> for UsagePriceOverrides {}

impl UsagePriceOverrides {
    /// Opens on `initial_selected` (the page's environment filter); a
    /// `initial_model` without a custom price starts as a new row.
    pub fn new(
        source: Rc<dyn UsageSource>,
        environments: Vec<UsagePriceTarget>,
        usage_models: Vec<(EnvironmentId, Vec<String>)>,
        initial_selected: Option<BTreeSet<EnvironmentId>>,
        initial_model: Option<String>,
    ) -> Self {
        let mut this = Self {
            source,
            environments,
            usage_models,
            selected_ids: initial_selected,
            drafts: Vec::new(),
            pending: false,
            attempt: None,
            next_row_id: 0,
            focus_row: None,
            inputs: HashMap::new(),
        };
        if let Some(model) = initial_model {
            let known = this.selected().iter().any(|e| {
                e.prices.as_ref().is_some_and(|p| p.contains_key(&model)) || e.aliases.as_ref().is_some_and(|a| a.contains_key(&model))
            });
            if !known {
                this.drafts.push(UsagePriceDraft { id: "new:initial".into(), model, is_new: true, ..Default::default() });
            }
        }
        this
    }

    /// Follows the page's environments as connections and settings change.
    pub fn set_environments(&mut self, environments: Vec<UsagePriceTarget>, usage_models: Vec<(EnvironmentId, Vec<String>)>, cx: &mut Context<Self>) {
        if self.environments != environments || self.usage_models != usage_models {
            self.environments = environments;
            self.usage_models = usage_models;
            cx.notify();
        }
    }

    fn selected(&self) -> Vec<UsagePriceTarget> {
        self.environments.iter().filter(|e| self.selected_ids.as_ref().is_none_or(|ids| ids.contains(&e.environment_id))).cloned().collect()
    }

    fn custom_models(selected: &[UsagePriceTarget]) -> Vec<String> {
        let models: BTreeSet<String> = selected
            .iter()
            .flat_map(|e| e.prices.iter().flat_map(|p| p.keys()).chain(e.aliases.iter().flat_map(|a| a.keys())).cloned())
            .collect();
        models.into_iter().collect()
    }

    fn rows(&self, selected: &[UsagePriceTarget]) -> Vec<UsagePriceDraft> {
        let new_models: BTreeSet<String> = self.drafts.iter().filter(|d| d.is_new).map(|d| d.model.trim().to_owned()).collect();
        let saved = Self::custom_models(selected).into_iter().filter(|m| self.attempt.is_none() || !new_models.contains(m)).map(|model| {
            let id: SharedString = format!("model:{model}").into();
            self.drafts.iter().find(|d| d.id == id).cloned().unwrap_or(UsagePriceDraft { id, model, ..Default::default() })
        });
        saved.chain(self.drafts.iter().filter(|d| d.is_new).cloned()).collect()
    }

    fn failed_destinations(&self) -> Vec<(EnvironmentId, SharedString)> {
        self.attempt.as_ref().map_or_else(Vec::new, |a| {
            a.destinations.iter().filter(|(id, _)| matches!(a.results.get(id), Some(UsagePriceWriteResult::Failed(_)))).cloned().collect()
        })
    }

    fn staged(&self) -> Vec<UsagePriceDraft> {
        self.drafts.iter().filter(|d| !d.is_empty()).cloned().collect()
    }

    fn select_environments(&mut self, ids: Option<BTreeSet<EnvironmentId>>, cx: &mut Context<Self>) {
        self.selected_ids = ids;
        self.attempt = None;
        cx.notify();
    }

    fn update_draft(&mut self, draft: UsagePriceDraft, cx: &mut Context<Self>) {
        match self.drafts.iter_mut().find(|d| d.id == draft.id) {
            Some(entry) => *entry = draft,
            None => self.drafts.push(draft),
        }
        self.attempt = None;
        cx.notify();
    }

    fn update_or_drop_draft(&mut self, draft: UsagePriceDraft, cx: &mut Context<Self>) {
        if !draft.is_new && draft.values.is_empty() && draft.alias.is_none() && !draft.removed {
            self.drafts.retain(|d| d.id != draft.id);
            cx.notify();
        } else {
            self.update_draft(draft, cx);
        }
    }

    /// A saved row's cell typed back to its saved value is no longer an edit.
    fn matches_saved(row: &UsagePriceDraft, value: &str, saved: &str, placeholder: &SharedString) -> bool {
        !row.is_new && *placeholder != t!("Mixed") && *placeholder != t!("Unavailable") && value == saved
    }

    fn row(&self, id: &SharedString) -> Option<UsagePriceDraft> {
        self.rows(&self.selected()).into_iter().find(|r| r.id == *id)
    }

    /// Applies typed text from one cell.
    fn edit(&mut self, id: SharedString, column: Column, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut row) = self.row(&id) else { return };
        let selected = self.selected();
        match column {
            Column::Model => {
                row.model = text;
                self.update_draft(row, cx);
            }
            Column::Alias => {
                let saved = usage_alias_cell(&selected, &row.model);
                row.alias = (!Self::matches_saved(&row, &text, &saved.value, &saved.placeholder)).then_some(text);
                self.update_or_drop_draft(row, cx);
            }
            Column::Price(field) => {
                let saved = usage_price_cell(&selected, &row.model, field);
                let previous = row.values.get(&field).cloned().unwrap_or_else(|| if row.is_new { String::new() } else { saved.value.clone() });
                if !is_usage_price_edit(&previous, &text) {
                    if let Some(cell) = self.inputs.get_mut(&(id, column)) {
                        cell.value = previous.clone();
                        cell.state.update(cx, |input, cx| input.set_value(previous, window, cx));
                    }
                    return;
                }
                if Self::matches_saved(&row, &text, &saved.value, &saved.placeholder) {
                    row.values.remove(&field);
                } else {
                    row.values.insert(field, text);
                }
                self.update_or_drop_draft(row, cx);
            }
        }
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        self.drafts.clear();
        self.attempt = None;
        cx.notify();
    }

    fn save(&mut self, retry: bool, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let destinations: Vec<(EnvironmentId, SharedString)> = if retry {
            self.failed_destinations()
        } else {
            self.selected().into_iter().map(|e| (e.environment_id, e.label)).collect()
        };
        let edits = match (&self.attempt, retry) {
            (Some(attempt), true) => attempt.drafts.clone(),
            _ => self.staged(),
        };
        if destinations.is_empty() || edits.is_empty() {
            return;
        }
        let targets: Vec<UsagePriceTarget> = destinations
            .iter()
            .map(|(id, label)| {
                self.environments.iter().find(|e| e.environment_id == *id).cloned().unwrap_or(UsagePriceTarget {
                    environment_id: id.clone(),
                    label: label.clone(),
                    prices: None,
                    aliases: None,
                    unavailable: Some(t!("Environment removed")),
                })
            })
            .collect();
        let previous = self.attempt.take().filter(|_| retry);
        let mut results = previous.as_ref().map(|a| a.results.clone()).unwrap_or_default();
        results.extend(targets.iter().map(|t| (t.environment_id.clone(), UsagePriceWriteResult::Saving)));
        self.attempt = Some(SaveAttempt {
            drafts: edits.clone(),
            destinations: previous.map_or_else(|| targets.iter().map(|t| (t.environment_id.clone(), t.label.clone())).collect(), |a| a.destinations),
            results,
        });
        self.pending = true;
        cx.notify();
        let mut remaining = targets.len();
        for target in targets {
            let (changes, errors) = usage_price_table_changes(&target, &edits);
            let blocked = target.unavailable.clone().or_else(|| errors.into_values().next());
            let planned = UsagePriceTarget { unavailable: blocked, ..target };
            let id = planned.environment_id.clone();
            let write = match usage_price_patch(&planned, &changes) {
                Err(reason) => None.ok_or(reason),
                Ok(None) => Ok(None),
                Ok(Some(patch)) => Ok(Some(self.source.update_settings(&id, patch))),
            };
            cx.spawn(async move |this, cx| {
                let result = match write {
                    Err(reason) => UsagePriceWriteResult::Failed(reason),
                    Ok(None) => UsagePriceWriteResult::Saved,
                    Ok(Some(call)) => match call.await {
                        Ok(()) => UsagePriceWriteResult::Saved,
                        Err(_) => UsagePriceWriteResult::Failed(t!("Could not save. Try again.")),
                    },
                };
                _ = this.update(cx, |this, cx| {
                    if let Some(attempt) = &mut this.attempt {
                        attempt.results.insert(id, result);
                    }
                    remaining -= 1;
                    if remaining == 0 {
                        this.pending = false;
                        if this.failed_destinations().is_empty() {
                            this.drafts.clear();
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.pending {
            cx.emit(UsagePriceOverridesClosed);
        }
    }

    /// The cell's input, created on first use and kept in step with `value`.
    fn input(&mut self, id: &SharedString, column: Column, value: &str, placeholder: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let key = (id.clone(), column);
        if let Some(cell) = self.inputs.get_mut(&key) {
            let new_placeholder = (cell.placeholder != placeholder).then(|| {
                cell.placeholder = placeholder.clone();
                placeholder
            });
            let new_value = (cell.value != value).then(|| {
                cell.value = value.to_owned();
                cell.value.clone()
            });
            if new_placeholder.is_some() || new_value.is_some() {
                cell.state.update(cx, |input, cx| {
                    if let Some(value) = new_value {
                        input.set_value(value, window, cx);
                    }
                    if let Some(placeholder) = new_placeholder {
                        input.set_placeholder(placeholder, window, cx);
                    }
                });
            }
            return cell.state.clone();
        }
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder.clone());
            state.set_value(value.to_owned(), window, cx);
            state
        });
        let row_id = id.clone();
        let subscription = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, window, cx| {
            if let InputEvent::Change = event {
                let text = input.read(cx).value().to_string();
                if let Some(cell) = this.inputs.get_mut(&(row_id.clone(), column)) {
                    cell.value = text.clone();
                }
                this.edit(row_id.clone(), column, text, window, cx);
            }
        });
        if self.focus_row.as_ref() == Some(id) && column == Column::Model {
            self.focus_row = None;
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        self.inputs.insert(key, CellInput { state: input.clone(), _changes: subscription, placeholder, value: value.to_owned() });
        input
    }

    fn apply_to_menu(&self, entity: Entity<Self>) -> MenuPopup {
        let selected: BTreeSet<EnvironmentId> = self.selected().into_iter().map(|e| e.environment_id).collect();
        let all = self.environments.len();
        let page = entity.clone();
        let mut menu = MenuPopup::new().item(
            MenuItem::new(t!("All environments")).kind(MenuItemKind::Checkbox(self.selected_ids.is_none())).keep_open().on_select(move |_, cx| {
                page.update(cx, |this, cx| {
                    let next = if this.selected_ids.is_none() { Some(BTreeSet::new()) } else { None };
                    this.select_environments(next, cx);
                })
            }),
        );
        menu = menu.separator();
        for environment in &self.environments {
            let id = environment.environment_id.clone();
            let checked = self.selected_ids.as_ref().is_none_or(|ids| ids.contains(&id));
            let (page, selected) = (entity.clone(), selected.clone());
            let mut item = MenuItem::new(environment.label.clone()).kind(MenuItemKind::Checkbox(checked)).keep_open().on_select(move |_, cx| {
                let mut next = selected.clone();
                if !next.remove(&id) {
                    next.insert(id.clone());
                }
                page.update(cx, |this, cx| this.select_environments(if next.len() == all { None } else { Some(next.clone()) }, cx));
            });
            if let Some(reason) = &environment.unavailable {
                item = item.detail(reason.clone());
            }
            menu = menu.item(item);
        }
        menu
    }

    fn render_row(&mut self, row: &UsagePriceDraft, selected: &[UsagePriceTarget], error: Option<SharedString>, locked: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let alias_cell = usage_alias_cell(selected, &row.model);
        let alias = row.alias.clone().unwrap_or_else(|| alias_cell.value.clone()).trim().to_owned();
        let model_cell = if row.is_new {
            let input = self.input(&row.id, Column::Model, &row.model, t!("Model ID"), window, cx);
            TextInput::new(&input).size(InputSize::Compact).invalid(!row.model.trim().is_empty() && error.is_some()).disabled(locked).into_any_element()
        } else {
            div()
                .text_color(if row.removed { c.muted_foreground } else { c.foreground() })
                .when(row.removed, |d| d.line_through())
                .child(row.model.clone())
                .into_any_element()
        };
        let show_error = error.is_some() && (!row.model.trim().is_empty() || row.values.values().any(|v| !v.is_empty()));
        let model_cell = div()
            .child(model_cell)
            .when_some(error.filter(|_| show_error), |d, error| d.child(div().mt(sp(1.)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(error)));
        let cell_id = |column: &str| SharedString::from(format!("usage-price-{}-{column}", row.id));
        let mut cells: Vec<AnyElement> = vec![cell(&theme, relative(0.22)).id(cell_id("model")).child(model_cell).test_support().into_any_element()];
        if row.removed {
            cells.push(cell(&theme, relative(0.78)).flex_1().text_color(c.muted_foreground).child(t!("Automatic pricing after saving")).into_any_element());
        } else {
            let placeholder = if row.is_new { t!("Optional") } else { alias_cell.placeholder.clone() };
            let value = row.alias.clone().unwrap_or(alias_cell.value.clone());
            let input = self.input(&row.id, Column::Alias, &value, placeholder, window, cx);
            cells.push(cell(&theme, relative(0.18)).id(cell_id("alias")).child(TextInput::new(&input).size(InputSize::Compact).disabled(locked)).test_support().into_any_element());
            if !alias.is_empty() {
                cells.push(
                    cell(&theme, relative(0.))
                        .flex_1()
                        .text_color(c.muted_foreground)
                        .child(crate::i18n::t_rich!("Counted as {alias}", alias = alias).plain())
                        .into_any_element(),
                );
            } else {
                for field in UsagePriceField::ALL {
                    let saved = usage_price_cell(selected, &row.model, field);
                    let value = row.values.get(&field).cloned().unwrap_or_else(|| if row.is_new { String::new() } else { saved.value.clone() });
                    let placeholder = if row.is_new {
                        if field.optional() { t!("Input rate") } else { "0.00".into() }
                    } else {
                        saved.placeholder
                    };
                    let input = self.input(&row.id, Column::Price(field), &value, placeholder, window, cx);
                    cells.push(cell(&theme, relative(0.)).flex_1().id(cell_id(field.key())).child(TextInput::new(&input).size(InputSize::Compact).mono().disabled(locked)).test_support().into_any_element());
                }
            }
        }
        let (icon, hint) = if row.removed {
            (IconName::RotateCcw, t!("Undo reset"))
        } else if row.is_new {
            (IconName::X, t!("Remove row"))
        } else {
            (IconName::RotateCcw, t!("Reset to automatic"))
        };
        let entity = cx.entity();
        let target = row.clone();
        cells.push(
            div()
                .flex_none()
                .w(theme.rem(2.5))
                .p(sp(2.))
                .child(div().id(SharedString::from(format!("usage-price-reset-tip-{}", row.id))).tooltip(tooltip(hint)).child(
                    Button::new(SharedString::from(format!("usage-price-reset-{}", row.id)))
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconXs)
                        .icon(icon)
                        .disabled(locked)
                        .on_click(move |_, _, cx| {
                            let target = target.clone();
                            entity.update(cx, |this, cx| {
                                if target.is_new {
                                    this.drafts.retain(|d| d.id != target.id);
                                    cx.notify();
                                } else if target.removed {
                                    this.update_or_drop_draft(UsagePriceDraft { removed: false, ..target }, cx);
                                } else {
                                    this.update_draft(UsagePriceDraft { removed: true, ..target }, cx);
                                }
                            })
                        }),
                ))
                .into_any_element(),
        );
        table_row(&theme).id(SharedString::from(format!("usage-price-row-{}", row.id))).children(cells).into_any_element()
    }
}

/// `TableCell`: `p-2 align-middle`, sized as its `<col>`.
fn cell(theme: &AppTheme, width: gpui_kit::DefiniteLength) -> gpui_kit::Div {
    div().flex_none().w(width).min_w_0().p(theme.sp(2.))
}

/// `TableRow`: a bottom rule, `hover:bg-muted/50`.
fn table_row(theme: &AppTheme) -> gpui_kit::Div {
    let c = &theme.colors;
    let hover = with_alpha(c.muted, 0.5);
    div().flex().items_center().w_full().border_b_1().border_color(c.border).hover(move |s| s.bg(hover))
}

impl Render for UsagePriceOverrides {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let entity = cx.entity();
        let selected = self.selected();
        let custom_models = Self::custom_models(&selected);
        let rows = self.rows(&selected);
        let staged = self.staged();
        let mut errors = usage_price_table_errors(&selected, &staged);
        // New rows still block Save, but show a price error only once a price or mapping is typed.
        let mut awaiting_price = BTreeSet::new();
        for draft in staged.iter().filter(|d| d.is_new) {
            let model = draft.model.trim();
            if model.is_empty() {
                errors.insert(draft.id.clone(), t!("Enter a model ID."));
            } else if self.attempt.is_none()
                && (custom_models.iter().any(|m| m == model) || self.drafts.iter().any(|o| o.id != draft.id && o.model.trim() == model))
            {
                errors.insert(draft.id.clone(), t!("This model already has a row. Edit its prices there."));
            } else if draft.alias.as_deref().unwrap_or("").trim().is_empty() && draft.values.values().all(|v| v.trim().is_empty()) {
                awaiting_price.insert(draft.id.clone());
            }
        }
        let failed = self.failed_destinations();
        let locked = self.pending || !failed.is_empty();
        let has_changes = !staged.is_empty();
        let destination_label: SharedString = match selected.as_slice() {
            [only] => only.label.clone(),
            _ => t!("{count, plural, one {{count} environments} other {{count} environments}}", count = selected.len()),
        };
        let selection_label = if self.selected_ids.is_none() { t!("All environments") } else { destination_label.clone() };

        let trigger = Button::new("usage-prices-apply")
            .variant(ButtonVariant::Outline)
            .size(ButtonSize::Sm)
            .label(selection_label)
            .trailing_icon(IconName::ChevronDown)
            .disabled(self.pending || has_changes);
        let menu_entity = entity.clone();
        let apply_menu = dropdown_menu("usage-prices-apply-menu", trigger, move |_, cx| menu_entity.read(cx).apply_to_menu(menu_entity.clone()));
        let toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(sp(2.))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(sp(2.))
                    .child(div().flex_none().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(t!("Apply to")))
                    .child(apply_menu),
            )
            .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("USD / million tokens")));

        let mut body: Vec<AnyElement> = vec![toolbar.into_any_element()];
        if selected.is_empty() {
            let message = if self.environments.is_empty() {
                t!("Connect an environment to set model prices.")
            } else {
                t!("Select an environment to see and change its model prices.")
            };
            body.push(div().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(message).into_any_element());
        } else {
            let add_entity = entity.clone();
            let header = table_row(&theme)
                .h(sp(10.))
                .ui_text(&theme, TextSize::Xs, weight::MEDIUM)
                .text_color(c.foreground())
                .child(cell(&theme, relative(0.22)).px(sp(2.)).py_0().child(t!("Model ID")))
                .child(cell(&theme, relative(0.18)).px(sp(2.)).py_0().child(t!("Map to")))
                .children(UsagePriceField::ALL.map(|f| cell(&theme, relative(0.)).flex_1().px(sp(2.)).py_0().child(f.label())))
                .child(
                    div().flex_none().w(theme.rem(2.5)).px(sp(2.)).child(
                        Button::new("usage-price-add")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::IconXs)
                            .icon(IconName::Plus)
                            .disabled(locked)
                            .on_click(move |_, _, cx| {
                                add_entity.update(cx, |this, cx| {
                                    let id: SharedString = format!("new:{}", this.next_row_id).into();
                                    this.next_row_id += 1;
                                    this.focus_row = Some(id.clone());
                                    this.update_draft(UsagePriceDraft { id, is_new: true, ..Default::default() }, cx);
                                })
                            }),
                    ),
                );
            let mut table_rows: Vec<AnyElement> = vec![header.into_any_element()];
            if rows.is_empty() {
                let message = if selected.iter().any(|e| e.prices.is_none()) {
                    t!("Some environment prices are unavailable.")
                } else {
                    t!("No custom prices or mappings. Add a row to set one.")
                };
                table_rows.push(
                    table_row(&theme)
                        .child(div().w_full().p(sp(2.)).flex().justify_center().child(div().py(sp(6.)).text_color(c.muted_foreground).child(message)))
                        .into_any_element(),
                );
            } else {
                for row in &rows {
                    let error = if awaiting_price.contains(&row.id) { None } else { errors.get(&row.id).cloned() };
                    table_rows.push(self.render_row(row, &selected, error, locked, window, cx));
                }
            }
            body.push(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .rounded(theme.radii.lg)
                    .border_1()
                    .border_color(c.border)
                    .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                    // The last row draws no rule (`[&_tr:last-child]:border-0`); the frame's border closes the table.
                    .child(div().flex().flex_col().mb(-gpui_kit::px(1.)).children(table_rows))
                    .into_any_element(),
            );
            let mut note = t!(
                "Blank cache rates use the input price. Enter 0 for free tokens. A mapped model’s usage counts as the model it maps to."
            )
            .to_string();
            if selected.len() > 1 {
                note.push(' ');
                note.push_str(&t!("Mixed cells keep each environment’s rate until you edit them."));
            }
            body.push(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(note).into_any_element());
        }
        if let Some(attempt) = &self.attempt {
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(1.))
                    .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                    .children(attempt.destinations.iter().map(|(id, label)| {
                        let (text, failed) = match attempt.results.get(id) {
                            Some(UsagePriceWriteResult::Failed(error)) => (t!("Not saved · {error}", error = error.clone()), true),
                            Some(UsagePriceWriteResult::Saved) => (t!("Saved"), false),
                            _ => (t!("Saving…"), false),
                        };
                        div()
                            .flex()
                            .justify_between()
                            .gap(sp(3.))
                            .child(div().truncate().text_color(c.muted_foreground).child(label.clone()))
                            .child(div().text_color(if failed { c.destructive() } else { c.muted_foreground }).child(text))
                    }))
                    .into_any_element(),
            );
        }

        let footer_note = div()
            .mr_auto()
            .ui_text(&theme, TextSize::Xs, weight::NORMAL)
            .text_color(c.muted_foreground)
            .child(if has_changes { t!("Changes apply to {destination}", destination = destination_label) } else { SharedString::default() });
        let mut surface = DialogSurface::new(t!("Custom model prices"))
            .description(t!("Prices and mappings apply to all past and future usage on the environments you select."))
            .max_width_rem(56.)
            .bare_footer()
            .footer_centered()
            .footer(footer_note);
        if has_changes {
            let discard = entity.clone();
            surface = surface.footer(
                Button::new("usage-prices-discard")
                    .variant(ButtonVariant::Ghost)
                    .label(if failed.is_empty() { t!("Discard changes") } else { t!("Discard pending changes") })
                    .disabled(self.pending)
                    .on_click(move |_, _, cx| discard.update(cx, |this, cx| this.discard(cx))),
            );
        }
        let save = entity.clone();
        let retry = !failed.is_empty();
        let save_label = if self.pending {
            t!("Saving…")
        } else if retry {
            t!("Retry failed saves")
        } else {
            t!("Save changes")
        };
        surface = surface.footer(
            Button::new("usage-prices-save")
                .label(save_label)
                .disabled(self.pending || (!has_changes && !retry) || (!retry && (!errors.is_empty() || selected.is_empty())))
                .on_click(move |_, _, cx| save.update(cx, |this, cx| this.save(retry, cx))),
        );
        let close = entity.clone();
        let surface = surface.on_close(move |_, cx| close.update(cx, |this, cx| this.close(cx))).children(body);
        // Inputs of rows that no longer exist are dropped.
        let live: BTreeSet<SharedString> = rows.iter().map(|r| r.id.clone()).collect();
        self.inputs.retain(|(id, _), _| live.contains(id));
        let backdrop = entity.clone();
        Modal::dialog(
            "usage-prices-dialog",
            div().id("usage-prices-dialog-content").occlude().w_full().max_w(theme.rem(56.)).child(surface),
            move |_, cx| backdrop.update(cx, |this, cx| this.close(cx)),
        )
    }
}

/// The models the page's usage reports for `environment`, for [`UsagePriceOverrides::set_environments`].
pub fn usage_models(summary: Option<&t3code_contracts::UsageSummary>) -> Vec<String> {
    let models: BTreeSet<String> = summary.into_iter().flat_map(|s| s.buckets.iter().map(|b| b.model.clone())).collect();
    models.into_iter().collect()
}

/// Shares `targets` with the dialog as owned rows.
pub fn price_targets(environments: &[super::usage_source::UsageEnvironment]) -> Vec<UsagePriceTarget> {
    environments.iter().map(|e| e.price_target.clone()).collect()
}

