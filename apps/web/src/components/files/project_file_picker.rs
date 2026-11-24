//! Go to file (Mod+P). Ports `ProjectFilePicker.tsx` and
//! `ProjectFilePicker.logic.ts`: the server ranks paths
//! (`projects.searchEntries`, files only, 200 max) and the picker keeps that
//! order, highlighting the query as a subsequence of the name and the path.
//!
//! Each keystroke narrows the shown results locally in the same frame when
//! the query only grew, then the server's ranking replaces them. One request
//! is in flight at a time; a newer query waits for it and only the latest runs.

use std::sync::Arc;

use gpui_kit::{rems, 
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, ParentElement as _, Render, ScrollStrategy, SharedString, Styled as _, Subscription, Task,
    UniformListScrollHandle, Window, component::input::{InputEvent, InputState}, div, prelude::*, px, uniform_list,
};
use t3code_contracts::{ProjectEntry, ProjectEntryKind};

use crate::{
    ActiveAppTheme as _,
    components::search::palette_shell::{PALETTE_WIDTH, highlighted_text, palette_footer, palette_frame, palette_input},
    file_source::{SharedWorkspaceFiles, normalize_path_query},
    i18n::t,
    pierre_icons::file_icon,
};

/// `PROJECT_FILE_PICKER_RESULT_LIMIT`.
pub const PICKER_LIMIT: u64 = 200;
const ROW_HEIGHT: f32 = 44.;
/// `max-h-[min(34rem,76vh)]`, inside the popup's `max-h-105` less the field and footer.
const LIST_MAX_HEIGHT: f32 = 328.;

/// One shown result with its highlight positions (byte offsets).
#[derive(Clone, Debug)]
pub struct PickerMatch {
    pub name: SharedString,
    pub name_indices: Vec<usize>,
    pub path: SharedString,
    pub path_indices: Vec<usize>,
}

/// `findMatchIndices`: positions of `query`'s characters in order, or `None`.
fn match_indices(value: &str, query: &str) -> Option<Vec<usize>> {
    if query.is_empty() {
        return Some(Vec::new());
    }
    let mut wanted = query.chars();
    let mut next = wanted.next();
    let mut indices = Vec::with_capacity(query.len());
    for (index, c) in value.char_indices() {
        let Some(target) = next else { break };
        if c.to_lowercase().eq(std::iter::once(target)) {
            indices.push(index);
            next = wanted.next();
        }
    }
    next.is_none().then_some(indices)
}

/// `getProjectFilePickerMatches`: keeps the server order, files only.
pub fn picker_matches(entries: &[ProjectEntry], raw_query: &str) -> Vec<PickerMatch> {
    let query = normalize_path_query(raw_query);
    entries
        .iter()
        .filter(|entry| entry.kind == ProjectEntryKind::File)
        .take(PICKER_LIMIT as usize)
        .map(|entry| {
            let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
            PickerMatch {
                name_indices: match_indices(name, &query).unwrap_or_default(),
                name: name.to_owned().into(),
                path_indices: match_indices(&entry.path, &query).unwrap_or_default(),
                path: entry.path.clone().into(),
            }
        })
        .collect()
}

/// What the picker asks its owner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePickerEvent {
    Open(SharedString),
    Close,
}

/// The go-to-file dialog.
pub struct ProjectFilePicker {
    files: SharedWorkspaceFiles,
    project_name: SharedString,
    input: Entity<InputState>,
    query: String,
    /// Query of the shown results, which the highlights use.
    matched_query: String,
    entries: Arc<Vec<ProjectEntry>>,
    matches: Arc<Vec<PickerMatch>>,
    selected: usize,
    pending: bool,
    /// A newer query typed while a request was in flight.
    queued: bool,
    error: Option<SharedString>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    _task: Option<Task<()>>,
    _subscription: Subscription,
}

impl EventEmitter<FilePickerEvent> for ProjectFilePicker {}

impl Focusable for ProjectFilePicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ProjectFilePicker {
    pub fn new(files: SharedWorkspaceFiles, project_name: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search files…")));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let query = input.read(cx).value().to_string();
                if query != this.query {
                    this.set_query(query, cx);
                }
            }
            InputEvent::PressEnter { .. } => this.open_selected(cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let mut picker = Self {
            files,
            project_name,
            input,
            query: String::new(),
            matched_query: String::new(),
            entries: Arc::default(),
            matches: Arc::default(),
            selected: 0,
            pending: false,
            queued: false,
            error: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _task: None,
            _subscription: subscription,
        };
        picker.request(cx);
        picker
    }

    pub fn matches(&self) -> &[PickerMatch] {
        &self.matches
    }

    pub fn is_pending(&self) -> bool {
        self.pending || self.queued
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Types a query, for scripted checks.
    pub fn set_query_text(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.set_value(query.to_owned(), window, cx));
        self.set_query(query.to_owned(), cx);
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        let narrowed = normalize_path_query(&query);
        if !self.matched_query.is_empty() && narrowed.starts_with(&normalize_path_query(&self.matched_query)) {
            // The query only grew: drop results that no longer match, in this frame.
            let kept: Vec<ProjectEntry> = self
                .entries
                .iter()
                .filter(|entry| match_indices(&entry.path, &narrowed).is_some())
                .cloned()
                .collect();
            self.matches = Arc::new(picker_matches(&kept, &query));
            self.entries = Arc::new(kept);
            self.matched_query = query.clone();
            self.selected = 0;
        }
        self.query = query;
        if self.pending {
            self.queued = true;
        } else {
            self.request(cx);
        }
        cx.notify();
    }

    fn request(&mut self, cx: &mut Context<Self>) {
        self.pending = true;
        self.queued = false;
        let query: String = self.query.chars().take(256).collect();
        let search = self.files.search_entries(query.clone(), PICKER_LIMIT);
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(search).await;
            this.update(cx, |this, cx| {
                this.pending = false;
                match result {
                    Ok(found) => {
                        this.error = None;
                        this.matches = Arc::new(picker_matches(&found.entries, &query));
                        this.entries = Arc::new(found.entries);
                        this.matched_query = query;
                    }
                    Err(error) => this.error = Some(error.message.into()),
                }
                this.selected = this.selected.min(this.matches.len().saturating_sub(1));
                if this.queued {
                    this.request(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(found) = self.matches.get(self.selected) {
            cx.emit(FilePickerEvent::Open(found.path.clone()));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "escape" => cx.emit(FilePickerEvent::Close),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_rows(&mut self, range: std::ops::Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.app_theme().clone();
        let colors = theme.colors;
        let highlight = !self.matched_query.trim().is_empty();
        range
            .filter_map(|index| self.matches.get(index).cloned().map(|m| (index, m)))
            .map(|(index, found)| {
                let active = index == self.selected;
                let (name_indices, path_indices) = if highlight { (found.name_indices.clone(), found.path_indices.clone()) } else { (vec![], vec![]) };
                let row = div()
                    .id(("file-picker-row", index))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px(rems(0.5))
                    .flex()
                    .items_center()
                    .gap(rems(0.625))
                    .rounded(theme.radii.lg)
                    .cursor_pointer()
                    .when(active, |el| el.bg(colors.accent_surface))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.selected != index {
                            this.selected = index;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = index;
                        this.open_selected(cx);
                    }))
                    .child(file_icon(&found.path, theme.rem(1.), cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).text_color(colors.text).whitespace_nowrap().overflow_hidden().text_ellipsis().child(highlighted_text(&found.name, &name_indices, colors.text)))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(colors.muted_foreground)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(highlighted_text(&found.path, &path_indices, colors.text)),
                            ),
                    );
                div().w_full().px(rems(0.5)).child(row).into_any_element()
            })
            .collect()
    }
}

impl Render for ProjectFilePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let colors = theme.colors;
        let body: AnyElement = if self.matches.is_empty() {
            let message = if let Some(error) = &self.error {
                error.clone()
            } else if self.pending {
                if self.query.trim().is_empty() { t!("Indexing workspace files…") } else { t!("Searching workspace files…") }
            } else if self.query.trim().is_empty() {
                t!("No files found.")
            } else {
                t!("No matching files.")
            };
            div().h(px(120.)).flex().items_center().justify_center().text_size(px(13.)).text_color(colors.muted_foreground).child(message).into_any_element()
        } else {
            let height = (self.matches.len() as f32 * ROW_HEIGHT + 40.).min(LIST_MAX_HEIGHT);
            div()
                .h(px(height))
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(rems(1.))
                        .pt(rems(0.5))
                        .pb(rems(0.25))
                        .text_size(px(11.))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(colors.muted_foreground)
                        .child(self.project_name.clone()),
                )
                .child(uniform_list("file-picker-rows", self.matches.len(), cx.processor(Self::render_rows)).track_scroll(&self.scroll).flex_1().pb(rems(0.5)))
                .into_any_element()
        };
        palette_frame(&theme, px(PALETTE_WIDTH), None)
            .id("project-file-picker")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(palette_input(&theme, &self.input, None))
            .child(div().h(px(1.)).bg(colors.border.opacity(0.6)))
            .child(body)
            .child(palette_footer(&theme, t!("Open file"), t!("Back", context = "button, leaves the file picker")))
    }
}
