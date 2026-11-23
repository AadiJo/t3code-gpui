//! Project content search (Mod+Shift+F). Ports `ProjectContentSearchDialog.tsx`:
//! a palette with the query field and Match case / Match whole word / Use
//! regular expression toggles, a status row (`n results in m files`), and the
//! results grouped by file with highlighted matches. Arrow keys move the
//! selection; Enter or a click opens the file at the line. The query goes to
//! `projects.searchContents` 120ms after the last keystroke, 500 results max.

use std::{sync::Arc, time::Duration};

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, ListAlignment, ListState, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task,
    Window, component::input::{InputEvent, InputState}, div, list, prelude::*, px,
};
use t3code_contracts::ProjectContentMatch;

use super::{
    highlighted_search_line::highlighted_search_line,
    palette_shell::{PALETTE_WIDTH, palette_footer, palette_frame, palette_input},
};
use crate::{
    ActiveAppTheme as _,
    components::ui::{spinner::{Spinner, SpinnerSize}, tooltip::tooltip},
    file_source::{ContentQuery, SharedWorkspaceFiles},
    i18n::t,
    pierre_icons::file_icon,
};

const DEBOUNCE: Duration = Duration::from_millis(120);
/// `PROJECT_CONTENT_SEARCH_LIMIT`.
pub const CONTENT_SEARCH_LIMIT: u64 = 500;
/// `PROJECT_SEARCH_CONTENTS_QUERY_MAX_LENGTH`.
const QUERY_MAX_LENGTH: usize = 256;
/// `h-105`.
const DIALOG_HEIGHT: f32 = 420.;

/// What the dialog asks its owner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentSearchEvent {
    /// Open the file and reveal the 1-based line; the dialog should close.
    Open { path: SharedString, line: u32 },
    Close,
}

#[derive(Clone, Debug)]
enum Row {
    Header { path: SharedString, count: usize, first: bool },
    Match { index: usize },
}

/// The dialog view.
pub struct ProjectContentSearch {
    files: SharedWorkspaceFiles,
    input: Entity<InputState>,
    case_sensitive: bool,
    whole_word: bool,
    use_regex: bool,
    query: String,
    matches: Arc<Vec<ProjectContentMatch>>,
    truncated: bool,
    invalid_regex: bool,
    pending: bool,
    error: Option<SharedString>,
    selected: usize,
    rows: Arc<Vec<Row>>,
    file_count: usize,
    list: ListState,
    focus_handle: FocusHandle,
    task: Option<Task<()>>,
    _subscription: Subscription,
}

impl EventEmitter<ContentSearchEvent> for ProjectContentSearch {}

impl Focusable for ProjectContentSearch {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ProjectContentSearch {
    pub fn new(files: SharedWorkspaceFiles, project_name: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = t!("Search in {project}", project = project_name.clone());
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let query = input.read(cx).value().to_string();
                this.set_query(query, cx);
            }
            InputEvent::PressEnter { .. } => this.open_selected(cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            files,
            input,
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            query: String::new(),
            matches: Arc::default(),
            truncated: false,
            invalid_regex: false,
            pending: false,
            error: None,
            selected: 0,
            rows: Arc::default(),
            file_count: 0,
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            focus_handle: cx.focus_handle(),
            task: None,
            _subscription: subscription,
        }
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    pub fn selected_match(&self) -> Option<&ProjectContentMatch> {
        self.matches.get(self.selected)
    }

    /// Types a query, for scripted checks.
    pub fn set_query_text(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.set_value(query.to_owned(), window, cx));
        self.set_query(query.to_owned(), cx);
    }

    pub fn set_options(&mut self, case_sensitive: bool, whole_word: bool, use_regex: bool, cx: &mut Context<Self>) {
        (self.case_sensitive, self.whole_word, self.use_regex) = (case_sensitive, whole_word, use_regex);
        self.search(Duration::ZERO, cx);
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.search(DEBOUNCE, cx);
    }

    fn search(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let query = self.query.clone();
        if query.trim().is_empty() || query.len() > QUERY_MAX_LENGTH {
            self.task = None;
            self.pending = false;
            self.set_results(Arc::default(), false, false, cx);
            return;
        }
        self.pending = true;
        let request = ContentQuery {
            query,
            limit: CONTENT_SEARCH_LIMIT,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            use_regex: self.use_regex,
        };
        let files = self.files.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let result = cx.background_spawn(files.search_contents(request)).await;
            this.update(cx, |this, cx| {
                this.pending = false;
                match result {
                    Ok(found) => {
                        this.error = None;
                        let invalid = found.regex_fallback_error.is_some();
                        this.set_results(Arc::new(found.matches), found.truncated, invalid, cx);
                    }
                    Err(error) => {
                        this.error = Some(error.message.into());
                        this.set_results(Arc::default(), false, false, cx);
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_results(&mut self, matches: Arc<Vec<ProjectContentMatch>>, truncated: bool, invalid_regex: bool, cx: &mut Context<Self>) {
        let mut rows = Vec::with_capacity(matches.len() + 32);
        let mut index = 0;
        let mut files = 0;
        while index < matches.len() {
            let path = &matches[index].path;
            let end = index + matches[index..].iter().take_while(|m| &m.path == path).count();
            rows.push(Row::Header { path: path.clone().into(), count: end - index, first: files == 0 });
            rows.extend((index..end).map(|index| Row::Match { index }));
            files += 1;
            index = end;
        }
        self.matches = matches;
        self.truncated = truncated;
        self.invalid_regex = invalid_regex;
        self.file_count = files;
        self.rows = Arc::new(rows);
        self.selected = 0;
        self.list.reset(self.rows.len());
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        if let Some(row) = self.rows.iter().position(|r| matches!(r, Row::Match { index } if *index == self.selected)) {
            self.list.scroll_to_reveal_item(row);
        }
        cx.notify();
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if self.pending || self.error.is_some() {
            return;
        }
        if let Some(found) = self.matches.get(self.selected) {
            cx.emit(ContentSearchEvent::Open { path: found.path.clone().into(), line: found.line_number as u32 });
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "escape" => cx.emit(ContentSearchEvent::Close),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn option(&self, id: &'static str, label: SharedString, glyph: &'static str, active: bool, cx: &mut Context<Self>, toggle: fn(&mut Self)) -> AnyElement {
        let theme = cx.app_theme();
        let colors = theme.colors;
        div()
            .id(id)
            .tooltip(tooltip(label))
            .h(px(24.))
            .min_w(px(28.))
            .px(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.radii.md)
            .font(crate::appearance_fonts::mono_font(gpui_kit::FontWeight::NORMAL))
            .text_size(px(12.))
            .text_color(if active { colors.text } else { colors.muted_foreground })
            .when(active, |el| el.bg(theme.derived.toggle_pressed).shadow(theme.shadows.xs5.clone()))
            .when(!active, |el| el.hover(|el| el.bg(colors.accent_surface)))
            .when(glyph == "ab", |el| el.underline())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this);
                this.search(Duration::ZERO, cx);
            }))
            .child(glyph)
            .into_any_element()
    }

    fn render_row(rows: &Arc<Vec<Row>>, matches: &Arc<Vec<ProjectContentMatch>>, selected: usize, entity: &gpui_kit::WeakEntity<Self>, index: usize, cx: &mut App) -> AnyElement {
        let theme = cx.app_theme().clone();
        let colors = theme.colors;
        match &rows[index] {
            Row::Header { path, count, first } => {
                let (directory, name) = match path.rfind('/') {
                    Some(i) => (Some(path[..i].to_owned()), path[i + 1..].to_owned()),
                    None => (None, path.to_string()),
                };
                div()
                    .when(!first, |el| el.pt(px(8.)))
                    .child(
                        div()
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(12.))
                            .text_size(px(12.))
                            .child(file_icon(path, px(14.), cx))
                            .child(div().flex_none().font_weight(gpui_kit::FontWeight::MEDIUM).text_color(colors.text).child(name))
                            .children(directory.map(|d| div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(colors.muted_foreground).child(d)))
                            .child(
                                div()
                                    .ml_auto()
                                    .flex_none()
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded_full()
                                    .bg(colors.muted)
                                    .text_size(px(10.))
                                    .text_color(colors.muted_foreground)
                                    .child(count.to_string()),
                            ),
                    )
                    .into_any_element()
            }
            Row::Match { index: match_index } => {
                let item = &matches[*match_index];
                let active = *match_index == selected;
                let (hover, click) = (entity.clone(), entity.clone());
                let match_index = *match_index;
                div()
                    .id(("content-match", match_index))
                    .h(px(28.))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(12.))
                    .font(crate::appearance_fonts::mono_font(gpui_kit::FontWeight::NORMAL))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(active, |el| el.bg(colors.accent_surface).text_color(colors.accent_surface_foreground))
                    .when(!active, |el| el.hover(|el| el.bg(colors.accent_surface.opacity(0.6))))
                    .on_hover(move |hovered, _, cx| {
                        if *hovered {
                            hover.update(cx, |this, cx| {
                                this.selected = match_index;
                                cx.notify();
                            })
                            .ok();
                        }
                    })
                    .on_click(move |_, _, cx| {
                        click.update(cx, |this, cx| {
                            this.selected = match_index;
                            this.open_selected(cx);
                        })
                        .ok();
                    })
                    .child(div().w(px(40.)).flex_none().text_right().text_color(colors.muted_foreground.opacity(0.7)).child(item.line_number.to_string()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(highlighted_search_line(item, &item.path, cx)))
                    .into_any_element()
            }
        }
    }
}

impl Render for ProjectContentSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let colors = theme.colors;
        let has_query = !self.query.trim().is_empty();
        let accessory = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(colors.border)
            .bg(colors.muted.opacity(0.3))
            .child(self.option("search-case", t!("Match case"), "Aa", self.case_sensitive, cx, |this| this.case_sensitive ^= true))
            .child(self.option("search-word", t!("Match whole word"), "ab", self.whole_word, cx, |this| this.whole_word ^= true))
            .child(self.option("search-regex", t!("Use regular expression"), ".*", self.use_regex, cx, |this| this.use_regex ^= true))
            .into_any_element();
        let status: Option<AnyElement> = (has_query || self.pending || self.error.is_some()).then(|| {
            let content: AnyElement = if self.pending {
                div().flex().items_center().gap(px(8.)).child(Spinner::new(SpinnerSize::Sm)).child(t!("Searching…")).into_any_element()
            } else if let Some(error) = &self.error {
                div().text_color(colors.destructive()).child(error.clone()).into_any_element()
            } else if self.invalid_regex {
                div().text_color(colors.destructive()).child(t!("Invalid regular expression")).into_any_element()
            } else {
                let (results, files) = (
                    crate::components::files::delimited_table::group_digits(self.matches.len()),
                    crate::components::files::delimited_table::group_digits(self.file_count),
                );
                if self.truncated {
                    t!("{results}+ results in {files} files", results = results, files = files).into_any_element()
                } else {
                    t!("{results} results in {files} files", results = results, files = files).into_any_element()
                }
            };
            div()
                .h(px(36.))
                .flex_none()
                .flex()
                .items_center()
                .px(px(12.))
                .border_b_1()
                .border_color(colors.border)
                .text_size(px(12.))
                .text_color(colors.muted_foreground)
                .child(content)
                .into_any_element()
        });
        let body: AnyElement = if self.matches.is_empty() {
            let message = if has_query && !self.pending && self.error.is_none() {
                t!("No results found.")
            } else {
                t!("Type to search across your project.")
            };
            div().flex_1().flex().items_center().justify_center().px(px(24.)).text_size(px(13.)).text_color(colors.muted_foreground).child(message).into_any_element()
        } else {
            let (rows, matches, selected, entity) = (self.rows.clone(), self.matches.clone(), self.selected, cx.entity().downgrade());
            list(self.list.clone(), move |index, _, cx| Self::render_row(&rows, &matches, selected, &entity, index, cx))
                .flex_1()
                .py(px(8.))
                .into_any_element()
        };
        palette_frame(&theme, px(PALETTE_WIDTH), Some(px(DIALOG_HEIGHT)))
            .id("project-content-search")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(palette_input(&theme, &self.input, Some(accessory)))
            .children(status)
            .child(body)
            .child(palette_footer(&theme, t!("Open file"), t!("Back")))
    }
}
