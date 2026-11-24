//! The ref picker popup. Ports `BranchPicker.tsx` and the item list of
//! `BranchToolbarBranchSelector.tsx`: a search field, a virtualized list of
//! refs with current / worktree / remote / default badges, "Checkout pull
//! request" and "Create new ref" items driven by the query, the "Start from
//! remote" switch for a new worktree's base, and status lines.
//!
//! The picker draws its own query, as the model picker does, so arrow keys,
//! Enter and Escape reach it while typing.

use std::ops::Range;

use gpui_kit::{rems, 
    App, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable, IntoElement, KeyBinding,
    ParentElement as _, Pixels, Render, ScrollStrategy, SharedString, Styled as _, UTF16Selection,
    TestSupportExt as _, UniformListScrollHandle, Window, actions, div, prelude::*, px, uniform_list,
};
use t3code_contracts::VcsRef;

use super::branch_toolbar_logic::{sanitize_new_ref_name, should_include_branch_picker_item};
use super::icons::{Icon, IconName, IconSize, IconSource};
use super::ui::popover::glass_surface;
use super::ui::switch::{Switch, SwitchSize};
use super::ui::tooltip::tooltip;
use super::ui::{DISABLED_OPACITY, TextStyleExt as _};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::i18n::{t, t_rich};
use crate::theme_color::with_alpha;

const CONTEXT: &str = "BranchPicker";
const CREATE_PREFIX: &str = "__create_new_branch__:";
const CHECKOUT_PREFIX: &str = "__checkout_pull_request__:";

actions!(branch_picker, [PickerUp, PickerDown, PickerConfirm, PickerClose, PickerBackspace]);

/// Binds the picker's keys. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", PickerUp, c),
        KeyBinding::new("down", PickerDown, c),
        KeyBinding::new("enter", PickerConfirm, c),
        KeyBinding::new("escape", PickerClose, c),
        KeyBinding::new("backspace", PickerBackspace, c),
    ]);
}

/// `parsePullRequestReference`, for the hosts' common shapes: `123`, `#123`,
/// a pull or merge request URL, or a `gh pr checkout 123` style command.
pub fn parse_pull_request_reference(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let digits = |s: &str| (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.to_owned());
    if let Some(n) = digits(trimmed.strip_prefix('#').unwrap_or(trimmed)) {
        return Some(n);
    }
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        let segments: Vec<&str> = trimmed.split(['/', '?', '#']).collect();
        let is_reference = segments.windows(2).any(|w| {
            matches!(w[0], "pull" | "pulls" | "merge_requests" | "pullrequest" | "pull-requests") && digits(w[1]).is_some()
        });
        return is_reference.then(|| trimmed.to_owned());
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    match words.as_slice() {
        [.., "checkout", last] if words.len() >= 3 => digits(last.trim_start_matches('#')),
        _ => None,
    }
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
pub enum BranchPickerItem {
    CheckoutPullRequest(String),
    CreateRef(String),
    Ref(VcsRef),
}

impl BranchPickerItem {
    fn value(&self) -> String {
        match self {
            Self::CheckoutPullRequest(reference) => format!("{CHECKOUT_PREFIX}{reference}"),
            Self::CreateRef(query) => format!("{CREATE_PREFIX}{query}"),
            Self::Ref(r) => r.name.clone(),
        }
    }
}

/// What the picker offers besides the refs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BranchPickerOptions {
    /// A typed name can become a new ref (not while choosing a worktree base).
    pub can_create_ref: bool,
    /// A typed pull request reference can be checked out into a thread.
    pub can_checkout_pull_request: bool,
    /// Change request noun for the checkout item.
    pub term: &'static str,
    /// The host's mark on the checkout item; the pull request glyph when unset.
    pub host_icon: Option<IconSource>,
    /// `Some(on)` shows the "Start from remote" switch.
    pub start_from_origin: Option<bool>,
    /// Shown above the status line ("Stop the current turn to switch worktrees.").
    pub notice: Option<SharedString>,
    /// "Loading refs..." or "Showing 100 of 240 refs".
    pub status: Option<SharedString>,
    /// Refs that cannot be picked right now.
    pub disabled_refs: Vec<String>,
    /// The checkout the project lives in, so other worktrees get a badge.
    pub project_cwd: Option<String>,
    pub width: Option<Pixels>,
}

/// What the picker asks its owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum BranchPickerEvent {
    Select(BranchPickerItem),
    StartFromOriginChanged(bool),
    /// Closed by Escape (`None`) or a press outside at this point.
    Dismiss(Option<gpui_kit::Point<Pixels>>),
}

/// The open picker.
pub struct BranchPicker {
    focus_handle: FocusHandle,
    query: String,
    refs: Vec<VcsRef>,
    value: Option<String>,
    options: BranchPickerOptions,
    items: Vec<BranchPickerItem>,
    highlighted: usize,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<BranchPickerEvent> for BranchPicker {}

impl Focusable for BranchPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BranchPicker {
    pub fn new(refs: Vec<VcsRef>, value: Option<String>, options: BranchPickerOptions, cx: &mut Context<Self>) -> Self {
        let mut picker = Self {
            focus_handle: cx.focus_handle(),
            query: String::new(),
            refs,
            value,
            options,
            items: Vec::new(),
            highlighted: 0,
            scroll: UniformListScrollHandle::new(),
        };
        picker.rebuild();
        picker
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Replaces the listed refs (a page loaded, or the list refreshed).
    pub fn set_refs(&mut self, refs: Vec<VcsRef>, cx: &mut Context<Self>) {
        self.refs = refs;
        self.rebuild();
        cx.notify();
    }

    pub fn set_options(&mut self, options: BranchPickerOptions, cx: &mut Context<Self>) {
        self.options = options;
        self.rebuild();
        cx.notify();
    }

    /// Types into the search field.
    pub fn type_query(&mut self, text: &str, cx: &mut Context<Self>) {
        self.query.push_str(text);
        self.highlighted = 0;
        self.rebuild();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// The items the query keeps, in order: checkout, refs, create.
    fn rebuild(&mut self) {
        let trimmed = self.query.trim();
        let normalized = trimmed.to_lowercase();
        let reference = self.options.can_checkout_pull_request.then(|| parse_pull_request_reference(trimmed)).flatten();
        let new_name = sanitize_new_ref_name(trimmed);
        let exact = self.refs.iter().any(|r| r.name == new_name);
        let create = (self.options.can_create_ref && !trimmed.is_empty() && !exact).then(|| trimmed.to_owned());
        let checkout_value = reference.as_ref().map(|r| format!("{CHECKOUT_PREFIX}{r}"));
        let create_value = create.as_ref().map(|q| format!("{CREATE_PREFIX}{q}"));
        let all = reference
            .map(BranchPickerItem::CheckoutPullRequest)
            .into_iter()
            .chain(self.refs.iter().cloned().map(BranchPickerItem::Ref))
            .chain(create.map(BranchPickerItem::CreateRef));
        self.items = all
            .filter(|item| {
                should_include_branch_picker_item(&item.value(), &normalized, create_value.as_deref(), checkout_value.as_deref())
            })
            .collect();
        if self.highlighted >= self.items.len() {
            self.highlighted = 0;
        }
    }

    fn is_disabled(&self, item: &BranchPickerItem) -> bool {
        matches!(item, BranchPickerItem::Ref(r) if self.options.disabled_refs.contains(&r.name))
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        self.highlighted = (self.highlighted as isize + delta).rem_euclid(len as isize) as usize;
        self.scroll_to_highlight();
        cx.notify();
    }

    /// The list scrolls by its own rows, which skip a leading checkout row.
    fn scroll_to_highlight(&self) {
        let offset = usize::from(matches!(self.items.first(), Some(BranchPickerItem::CheckoutPullRequest(_))));
        if let Some(row) = self.highlighted.checked_sub(offset) {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    /// Moves the keyboard highlight (also what the arrow keys do).
    pub fn highlight(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.items.len() {
            self.highlighted = index;
            self.scroll_to_highlight();
            cx.notify();
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(item) = self.items.get(index).cloned()
            && !self.is_disabled(&item)
        {
            cx.emit(BranchPickerEvent::Select(item));
        }
    }

    fn on_up(&mut self, _: &PickerUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_highlight(-1, cx);
    }

    fn on_down(&mut self, _: &PickerDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_highlight(1, cx);
    }

    fn on_confirm(&mut self, _: &PickerConfirm, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.highlighted, cx);
    }

    fn on_close(&mut self, _: &PickerClose, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(BranchPickerEvent::Dismiss(None));
    }

    fn on_backspace(&mut self, _: &PickerBackspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.query.pop().is_some() {
            self.highlighted = 0;
            self.rebuild();
            cx.notify();
        }
    }

    fn render_row(&self, index: usize, item: &BranchPickerItem, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let highlighted = index == self.highlighted;
        let disabled = self.is_disabled(item);
        let selected = matches!(item, BranchPickerItem::Ref(r) if Some(&r.name) == self.value.as_ref());
        let content = match item {
            BranchPickerItem::CheckoutPullRequest(reference) => div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(rems(0.5))
                .child(
                    Icon::new(self.options.host_icon.unwrap_or(IconName::GitPullRequestArrow.into()))
                        .size(IconSize::Custom(theme.rem(0.875)))
                        .color(c.muted_foreground),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(div().truncate().font_weight(weight::MEDIUM).child(t!(
                            "Checkout {term, select, mr {merge request} cr {change request} other {pull request}}",
                            term = self.options.term
                        )))
                        .child(
                            div().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(reference.clone()),
                        ),
                ),
            BranchPickerItem::CreateRef(query) => div()
                .truncate()
                .child(t_rich!("Create new ref \"{name}\"", name = sanitize_new_ref_name(query)).plain()),
            BranchPickerItem::Ref(r) => {
                let other_worktree = r
                    .worktree_path
                    .as_deref()
                    .is_some_and(|w| self.options.project_cwd.as_deref().is_some_and(|cwd| cwd != w));
                let badge = if r.current {
                    Some(t!("current", context = "git ref badge, the checked-out branch"))
                } else if other_worktree {
                    Some(t!("worktree", context = "git ref badge, checked out in another worktree"))
                } else if r.is_remote == Some(true) {
                    Some(t!("remote", context = "git ref badge, a remote branch"))
                } else if r.is_default {
                    Some(t!("default", context = "git ref badge, the default branch"))
                } else {
                    None
                };
                div()
                    .flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap(rems(0.5))
                    .child(div().flex_1().min_w_0().truncate().child(r.name.clone()))
                    .children(badge.map(|badge| {
                        div()
                            .flex_none()
                            .ui_text(theme, TextSize::Xs3, weight::NORMAL)
                            .text_color(with_alpha(c.muted_foreground, 0.45))
                            .child(badge)
                    }))
            }
        };
        let highlight = c.highlight();
        div()
            .id(("branch-picker-row", index))
            .w_full()
            .h(theme.rem(1.75))
            .flex()
            .items_center()
            .px(theme.rem(0.5))
            .rounded(theme.radii.sm)
            .ui_text(theme, TextSize::Sm, weight::NORMAL)
            .when(matches!(item, BranchPickerItem::CheckoutPullRequest(_)), |r| r.h(theme.rem(2.75)))
            .when(selected, |r| r.bg(theme.derived.item_selected))
            .when(highlighted && !disabled, |r| r.bg(highlight).text_color(c.highlight_foreground()))
            .when(disabled, |r| r.opacity(DISABLED_OPACITY))
            .when(!disabled, |r| {
                r.cursor_pointer()
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.highlighted != index {
                            this.highlighted = index;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(index, cx)))
            })
            .child(content)
            .into_any_element()
    }
}

impl Render for BranchPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        // A checkout row is two lines tall and always first, so it sits above
        // the uniform list of 28px rows.
        let offset = usize::from(matches!(self.items.first(), Some(BranchPickerItem::CheckoutPullRequest(_))));
        let count = self.items.len() - offset;
        // `py-2` on the list plus the virtualized wrapper's `py-1`, under `max-h-56`.
        let list_height = (theme.rem(1.75) * count as f32 + theme.rem(1.5)).min(theme.rem(14.));
        let checkout_row = (offset == 1).then(|| {
            let item = self.items[0].clone();
            div().px(theme.rem(0.5)).pt(theme.rem(0.75)).child(self.render_row(0, &item, cx))
        });
        let query = self.query.clone();
        let focused = self.focus_handle.is_focused(window);
        // `ComboboxSearchInput`: an underlined field whose rule turns the ring
        // colour while it has focus, the icon inside its start padding.
        let search = div().flex_none().px(theme.rem(0.75)).pt(theme.rem(0.625)).child(
            div()
                .relative()
                .top(px(-1.))
                .flex()
                .items_center()
                .pb(theme.rem(0.375))
                .border_b_1()
                .border_color(if focused { c.ring() } else { with_alpha(c.border, 0.7) })
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top(theme.rem(0.375))
                        .child(Icon::new(IconName::Search).size(IconSize::Custom(theme.rem(1.))).color(with_alpha(c.muted_foreground, 0.55))),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .min_w_0()
                        .h(theme.rem(1.625))
                        .pl(theme.rem(1.25))
                        .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                        .line_height(theme.rem(1.625))
                        .text_color(if query.is_empty() { c.placeholder } else { c.foreground() })
                        .when(!query.is_empty(), |q| q.child(query.clone()))
                        .when(focused, |q| q.child(div().w(px(1.)).h(theme.rem(1.)).bg(c.foreground())))
                        .when(query.is_empty(), |q| q.child(t!("Search refs..."))),
                )
                .child(picker_input_canvas(cx.entity())),
        );
        let list = if count == 0 && offset == 0 {
            div()
                .p(rems(0.75))
                .text_center()
                .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(t!("No refs found."))
                .into_any_element()
        } else {
            uniform_list(
                "branch-picker-list",
                count,
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range.map(|i| this.render_row(i + offset, &this.items[i + offset].clone(), cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .h(list_height)
            .py(theme.rem(0.75))
            .pl(theme.rem(0.5))
            .pr(theme.rem(0.25) + theme.spacing.app_scrollbar_width)
            .into_any_element()
        };
        let origin = self.options.start_from_origin.map(|on| {
            let this = cx.entity();
            div()
                .id("start-from-remote")
                .flex()
                .items_center()
                .justify_between()
                .gap(rems(0.75))
                .border_t_1()
                .border_color(with_alpha(c.border, 0.6))
                .px(rems(0.75))
                .py(rems(0.5))
                .ui_text(&theme, TextSize::Xs, weight::MEDIUM)
                .tooltip(tooltip(t!("Uses the latest commit of the base branch’s tracked remote branch, even if its name differs. Otherwise uses the same-named branch on origin. Uses your local branch if that remote branch is missing.")))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rems(0.375))
                        .text_color(c.muted_foreground)
                        .child(Icon::new(IconName::RefreshCw).size(IconSize::Xs))
                        .child(t!("Start from remote")),
                )
                .child(Switch::new("start-from-remote-switch", on).size(SwitchSize::Sm).on_change(move |checked, _, cx| {
                    this.update(cx, |this, cx| {
                        this.options.start_from_origin = Some(checked);
                        cx.emit(BranchPickerEvent::StartFromOriginChanged(checked));
                        cx.notify();
                    })
                }))
        });
        let status_line = |text: SharedString| {
            div().px(rems(0.75)).py(rems(0.375)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(text)
        };
        glass_surface(&theme)
            .id("branch-picker")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_backspace))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, event: &gpui_kit::MouseDownEvent, _, cx| {
                cx.emit(BranchPickerEvent::Dismiss(Some(event.position)))
            }))
            .w(self.options.width.unwrap_or(theme.rem(20.)))
            .overflow_hidden()
            .child(search)
            .children(checkout_row)
            .child(list)
            .children(origin)
            .children(self.options.notice.clone().map(status_line))
            .children(self.options.status.clone().map(status_line))
            .test_support()
            .track_focus(&self.focus_handle)
    }
}

impl EntityInputHandler for BranchPicker {
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

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.type_query(text, cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: gpui_kit::Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<gpui_kit::Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(&mut self, _: gpui_kit::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// Registers the picker's text input for the focused frame.
fn picker_input_canvas(picker: gpui_kit::Entity<BranchPicker>) -> impl IntoElement {
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let focus = picker.read(cx).focus_handle.clone();
            window.handle_input(&focus, gpui_kit::ElementInputHandler::new(bounds, picker.clone()), cx);
        },
    )
    .size_0()
}
