//! The Views menu of the pull requests and issues pages: saved list controls
//! under a name, kept in `localStorage` under the web's keys and in the web's
//! shape so views carry over from the web app. Ports `SavedViewsMenu.tsx`.

use std::marker::PhantomData;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use t3code_client_runtime::saved_views::{SAVED_VIEW_NAME_MAX_LENGTH, saved_view_name_error};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::client_persistence_storage::LocalStorage;
use crate::components::icons::{IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::input::TextInput;
use crate::components::ui::menu::{MenuItem, MenuPopup, dropdown_menu};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;

/// Which list a menu belongs to; picks whole translated sentences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedViewsList {
    PullRequest,
    Issue,
}

impl SavedViewsList {
    fn select(self) -> &'static str {
        match self {
            Self::PullRequest => "pullRequest",
            Self::Issue => "issue",
        }
    }
}

/// What one list stores and how it compares its controls. Ports `SavedViewsKind`.
pub trait SavedViewsKind: 'static {
    /// The list controls a view keeps, in the web's stored shape.
    type Preferences: Clone + PartialEq + Serialize + DeserializeOwned + 'static;
    /// The `localStorage` key the web uses.
    const STORAGE_KEY: &'static str;
    const LIST: SavedViewsList;
    /// The name field's placeholder.
    fn placeholder() -> SharedString;
    /// The name offered for a new view of `preferences`, or "".
    fn suggested_name(preferences: &Self::Preferences) -> SharedString;
    /// Whether a stored view holds the same controls as `current`.
    fn matches(view: &Self::Preferences, current: &Self::Preferences) -> bool;
}

/// One saved view as the web stores it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredView<P> {
    pub id: String,
    pub name: String,
    pub preferences: P,
}

/// Asks the page to restore a view's controls.
#[derive(Clone, Debug)]
pub struct ApplySavedView<P>(pub P);

struct Editor {
    /// The view being renamed; `None` saves a new one.
    id: Option<String>,
    input: Entity<InputState>,
    /// `autoFocus`: the field takes focus after the modal's first frame, which takes it first.
    focus_pending: bool,
    _events: Subscription,
}

/// The Views button, its menu and the save and rename dialog.
pub struct SavedViewsMenu<K: SavedViewsKind> {
    views: Vec<StoredView<K::Preferences>>,
    selected_id: Option<String>,
    current: K::Preferences,
    editor: Option<Editor>,
    _kind: PhantomData<K>,
}

impl<K: SavedViewsKind> EventEmitter<ApplySavedView<K::Preferences>> for SavedViewsMenu<K> {}

impl<K: SavedViewsKind> SavedViewsMenu<K> {
    /// A menu over the stored views, matched against `current`.
    pub fn new(current: K::Preferences, cx: &mut Context<Self>) -> Self {
        Self { views: Self::read(cx), selected_id: None, current, editor: None, _kind: PhantomData }
    }

    fn read(cx: &App) -> Vec<StoredView<K::Preferences>> {
        LocalStorage::get(cx, K::STORAGE_KEY).unwrap_or_default()
    }

    /// Follows the page's controls, so the menu shows the matching view or the modified mark.
    pub fn set_current(&mut self, current: K::Preferences, cx: &mut Context<Self>) {
        if self.current != current {
            self.current = current;
            cx.notify();
        }
    }

    /// The name typed in the open dialog, for scripts.
    pub fn editor_text(&self, cx: &App) -> Option<String> {
        self.editor.as_ref().map(|e| e.input.read(cx).value().to_string())
    }

    /// The stored views.
    pub fn views(&self) -> &[StoredView<K::Preferences>] {
        &self.views
    }

    fn selected(&self) -> Option<&StoredView<K::Preferences>> {
        self.selected_id
            .as_ref()
            .and_then(|id| self.views.iter().find(|v| &v.id == id))
            .or_else(|| self.views.iter().find(|v| K::matches(&v.preferences, &self.current)))
    }

    /// Rewrites the stored list from its latest value.
    fn persist(&mut self, update: impl FnOnce(Vec<StoredView<K::Preferences>>) -> Vec<StoredView<K::Preferences>>, cx: &mut Context<Self>) {
        let written = update(Self::read(cx));
        LocalStorage::set(cx, K::STORAGE_KEY, Some(&written));
        self.views = written;
        cx.notify();
    }

    fn apply(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(view) = self.views.iter().find(|v| v.id == id) else { return };
        let preferences = view.preferences.clone();
        self.selected_id = Some(id);
        cx.emit(ApplySavedView(preferences));
        cx.notify();
    }

    /// Opens the dialog to save the current controls, or with `id` to rename that view.
    pub fn open_editor(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &id {
            Some(id) => self.views.iter().find(|v| &v.id == id).map(|v| v.name.clone()).unwrap_or_default(),
            None => K::suggested_name(&self.current).to_string(),
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(K::placeholder()).default_value(name));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => this.save(cx),
            _ => {}
        });
        self.editor = Some(Editor { id, input, focus_pending: true, _events: events });
        cx.notify();
    }

    fn name_error(&self, cx: &App) -> Option<SharedString> {
        let editor = self.editor.as_ref()?;
        let name = editor.input.read(cx).value();
        let views = self.views.iter().map(|v| (v.id.as_str(), v.name.as_str()));
        saved_view_name_error(&name, views, editor.id.as_deref()).map(|error| match error {
            t3code_client_runtime::saved_views::SavedViewNameError::Empty => t!("Enter a name for this view."),
            t3code_client_runtime::saved_views::SavedViewNameError::TooLong => {
                t!("Use {max} characters or fewer.", max = SAVED_VIEW_NAME_MAX_LENGTH)
            }
            t3code_client_runtime::saved_views::SavedViewNameError::Duplicate => t!("A view with this name already exists."),
        })
    }

    /// Saves the dialog's name; keeps the dialog open when it cannot.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.name_error(cx).is_some() {
            return;
        }
        let Some(editor) = &self.editor else { return };
        let name = editor.input.read(cx).value().trim().to_owned();
        let editing = editor.id.clone();
        if let Some(editing) = &editing
            && !Self::read(cx).iter().any(|v| &v.id == editing)
        {
            if let Some(editor) = &mut self.editor {
                editor.id = None;
            }
            Toaster::push(
                ToastData::new(ToastKind::Error, t!("This saved view was removed"))
                    .description(t!("Your name has been kept. Save it as a new view or cancel.")),
                cx,
            );
            cx.notify();
            return;
        }
        let id = editing.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let preferences = self.current.clone();
        let new_id = id.clone();
        self.persist(
            move |current| match editing {
                Some(editing) => current.into_iter().map(|v| if v.id == editing { StoredView { name: name.clone(), ..v } } else { v }).collect(),
                None => current.into_iter().chain([StoredView { id: new_id, name, preferences }]).collect(),
            },
            cx,
        );
        self.selected_id = Some(id);
        self.editor = None;
    }

    fn render_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if let Some(editor) = self.editor.as_mut()
            && std::mem::take(&mut editor.focus_pending)
        {
            let input = editor.input.clone();
            window.defer(cx, move |window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
        }
        let editor = self.editor.as_ref()?;
        let theme = cx.app_theme().clone();
        let list = K::LIST.select();
        let title = if editor.id.is_some() {
            t!("{list, select, pullRequest {Rename PR view} other {Rename issue view}}", list = list)
        } else {
            t!("{list, select, pullRequest {Save PR view} other {Save issue view}}", list = list)
        };
        let error = self.name_error(cx);
        let typed = !editor.input.read(cx).value().is_empty();
        let shown_error = error.clone().filter(|_| typed);
        let close = cx.listener(|this, _: &(), _, cx| {
            this.editor = None;
            cx.notify();
        });
        let close = std::rc::Rc::new(close);
        let surface = DialogSurface::new(title)
            .description(t!("Save your search, filters, and sort order on this device."))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(2.))
                    .child(
                        div()
                            .ui_text(&theme, TextSize::Sm, weight::MEDIUM)
                            .line_height(theme.sp(4.))
                            .text_color(theme.colors.foreground())
                            .child(t!("View name")),
                    )
                    .child(TextInput::new(&editor.input).invalid(shown_error.is_some()))
                    .children(shown_error.map(|e| {
                        div().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.destructive_foreground()).child(e)
                    })),
            )
            .footer(Button::new("saved-view-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).on_click({
                let close = close.clone();
                move |_, window, cx| close(&(), window, cx)
            }))
            .footer(
                Button::new("saved-view-save")
                    .label(t!("Save"))
                    .disabled(error.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .on_close({
                let close = close.clone();
                move |window, cx| close(&(), window, cx)
            });
        Some(Modal::dialog(
            "saved-view-dialog",
            div().id("dialog-content").occlude().w_full().max_w(theme.rem(32.)).child(surface),
            move |window, cx| close(&(), window, cx),
        ))
    }
}

impl<K: SavedViewsKind> Render for SavedViewsMenu<K> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let selected = self.selected().map(|v| (v.id.clone(), v.name.clone(), v.preferences.clone()));
        if let Some((id, _, _)) = &selected
            && self.selected_id.as_ref() != Some(id)
        {
            self.selected_id = Some(id.clone());
        }
        let modified = selected.as_ref().is_some_and(|(_, _, p)| !K::matches(p, &self.current));
        let label: SharedString = selected
            .as_ref()
            .map(|(_, name, _)| SharedString::from(name.clone()))
            .unwrap_or_else(|| t!("Views", context = "saved filter views menu"));
        let trigger = Button::new("saved-views-trigger")
            .variant(ButtonVariant::Outline)
            .icon(IconName::Bookmark)
            .label(label)
            .label_max_width(theme.rem(10.))
            .when(modified, |b| b.accessory(div().child("*")))
            .trailing_icon(IconName::ChevronDown)
            .trailing_icon_size(IconSize::Xs);
        let this = cx.entity();
        let menu = dropdown_menu("saved-views", trigger, move |_, cx| {
            let menu_state = this.read(cx);
            let selected_id = menu_state.selected().map(|v| v.id.clone());
            let views: Vec<(String, String)> = menu_state.views.iter().map(|v| (v.id.clone(), v.name.clone())).collect();
            let mut menu = MenuPopup::new();
            for (id, name) in &views {
                let on = selected_id.as_ref() == Some(id) && !modified;
                let target = this.clone();
                let id = id.clone();
                menu = menu.item(
                    MenuItem::new(SharedString::from(name.clone()))
                        .icon(if on { IconName::Check } else { IconName::Bookmark })
                        .on_select(move |_, cx| target.update(cx, |m, cx| m.apply(id.clone(), cx))),
                );
            }
            if !views.is_empty() {
                menu = menu.separator();
            }
            let save = this.clone();
            menu = menu.item(
                MenuItem::new(t!("Save current view…"))
                    .id("saved-views-save")
                    .icon(IconName::Plus)
                    .on_select(move |window, cx| save.update(cx, |m, cx| m.open_editor(None, window, cx))),
            );
            if let Some(id) = selected_id {
                if modified {
                    let update = this.clone();
                    let id = id.clone();
                    menu = menu.item(MenuItem::new(t!("Update saved view")).on_select(move |_, cx| {
                        update.update(cx, |m, cx| {
                            let preferences = m.current.clone();
                            let id = id.clone();
                            m.persist(
                                move |current| current.into_iter().map(|v| if v.id == id { StoredView { preferences: preferences.clone(), ..v } } else { v }).collect(),
                                cx,
                            );
                        })
                    }));
                }
                let rename = this.clone();
                let rename_id = id.clone();
                menu = menu.item(
                    MenuItem::new(t!("Rename view…"))
                        .on_select(move |window, cx| rename.update(cx, |m, cx| m.open_editor(Some(rename_id.clone()), window, cx))),
                );
                let delete = this.clone();
                menu = menu.item(MenuItem::new(t!("Delete view")).destructive().on_select(move |_, cx| {
                    delete.update(cx, |m, cx| {
                        let id = id.clone();
                        m.persist(move |current| current.into_iter().filter(|v| v.id != id).collect(), cx);
                        m.selected_id = None;
                    })
                }));
            }
            menu
        });
        div().child(menu).children(self.render_dialog(window, cx))
    }
}
