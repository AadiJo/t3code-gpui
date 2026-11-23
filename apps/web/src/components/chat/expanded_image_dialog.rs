//! The full-window media viewer. Ports `ExpandedImageDialog.tsx` and the
//! item model of `ExpandedImagePreview.tsx`: a black/75 backdrop, a Back
//! button, previous and next buttons and arrow keys for a gallery, the
//! zoomable image with its name and position below, and the media actions on
//! right click. Videos show the open-in-player card (no decoder here).

use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString, Styled as _, Task, Window, anchored,
    deferred, div, point, prelude::*, px,
};

use super::zoomable_image::ZoomableImage;
use crate::{
    ActiveAppTheme as _,
    components::{
        icons::{Icon, IconName, IconSize},
        media::{
            media_actions::{MediaActionSource, MediaKind, OpenFileHandler, media_actions_menu},
            media_source::{DecodedImage, MediaBytes, load_image},
        },
        ui::spinner::{Spinner, SpinnerSize},
    },
    i18n::t,
};

/// `ExpandedImageItem`.
#[derive(Clone)]
pub struct ExpandedImageItem {
    pub name: SharedString,
    pub kind: MediaKind,
    pub bytes: MediaBytes,
    /// Workspace-relative path, for "Copy relative path" and "Open in file viewer".
    pub relative_path: Option<SharedString>,
    pub absolute_path: Option<SharedString>,
}

/// What the dialog asks its owner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpandedImageEvent {
    Close,
    OpenFile(SharedString),
}

enum Current {
    Loading,
    Ready(Entity<ZoomableImage>, DecodedImage),
    Failed,
}

/// The dialog view. Render it only while open, above everything else.
pub struct ExpandedImageDialog {
    items: Vec<ExpandedImageItem>,
    index: usize,
    current: Current,
    menu: Option<Point<Pixels>>,
    focus_handle: FocusHandle,
    _load: Option<Task<()>>,
}

impl EventEmitter<ExpandedImageEvent> for ExpandedImageDialog {}

impl Focusable for ExpandedImageDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ExpandedImageDialog {
    pub fn new(items: Vec<ExpandedImageItem>, index: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut dialog = Self { items, index, current: Current::Loading, menu: None, focus_handle: cx.focus_handle(), _load: None };
        window.focus(&dialog.focus_handle, cx);
        dialog.load(cx);
        dialog
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// The zoomable image once loaded, for checks.
    pub fn image(&self) -> Option<&Entity<ZoomableImage>> {
        match &self.current {
            Current::Ready(image, _) => Some(image),
            _ => None,
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.current = Current::Loading;
        let Some(item) = self.items.get(self.index).cloned() else { return };
        if item.kind == MediaKind::Video {
            self._load = None;
            return;
        }
        let task = load_image(&item.bytes, item.name.clone(), cx);
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |dialog, cx| {
                dialog.current = match result {
                    Ok(decoded) => Current::Ready(cx.new(|cx| ZoomableImage::new(decoded.clone(), cx)), decoded),
                    Err(_) => Current::Failed,
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// Moves through the gallery, wrapping at the ends.
    pub fn navigate(&mut self, direction: isize, cx: &mut Context<Self>) {
        let count = self.items.len() as isize;
        if count <= 1 {
            return;
        }
        self.index = ((self.index as isize + direction).rem_euclid(count)) as usize;
        self.menu = None;
        self.load(cx);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            if self.menu.take().is_some() {
                cx.notify();
            } else {
                cx.emit(ExpandedImageEvent::Close);
            }
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if let Current::Ready(image, _) = &self.current
            && !(modifiers.control || modifiers.platform || modifiers.alt)
        {
            let image = image.clone();
            if image.update(cx, |image, cx| image.handle_key(key, event.is_held, cx)) {
                return;
            }
        }
        match key {
            "left" => self.navigate(-1, cx),
            "right" => self.navigate(1, cx),
            _ => {}
        }
    }

    fn action_source(&self) -> Option<MediaActionSource> {
        let item = self.items.get(self.index)?;
        Some(MediaActionSource {
            kind: item.kind,
            name: item.name.clone(),
            relative_path: item.relative_path.clone(),
            absolute_path: item.absolute_path.clone(),
            bytes: item.bytes.clone(),
            decoded: match &self.current {
                Current::Ready(_, decoded) => Some(decoded.clone()),
                _ => None,
            },
        })
    }

    fn nav_button(&self, id: &'static str, icon: IconName, direction: isize, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(id)
            .size(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(gpui_kit::black().opacity(0.5))
            .hover(|el| el.bg(gpui_kit::black().opacity(0.7)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.navigate(direction, cx)))
            .child(Icon::new(icon).size(IconSize::Xl).color(gpui_kit::white()))
            .into_any_element()
    }
}

impl Render for ExpandedImageDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let theme = cx.app_theme().clone();
        let count = self.items.len();
        let item = self.items.get(self.index).cloned();
        let caption: SharedString = match &item {
            Some(item) if count > 1 => format!("{} ({}/{})", item.name, self.index + 1, count).into(),
            Some(item) => item.name.clone(),
            None => t!("Media"),
        };
        let content: AnyElement = match (&self.current, item.as_ref().map(|i| i.kind)) {
            (_, Some(MediaKind::Video)) => div()
                .w(px(420.))
                .h(px(260.))
                .rounded(theme.radii.lg)
                .bg(theme.colors.canvas)
                .border_1()
                .border_color(theme.colors.border.opacity(0.7))
                .child(div().size_full().flex().flex_col().items_center().justify_center().gap(px(8.)).text_size(px(12.)).text_color(theme.colors.muted_foreground)
                    .child(Icon::new(IconName::Film).size(IconSize::Xl))
                    .child(t!("Video preview")))
                .into_any_element(),
            (Current::Ready(image, _), _) => image.clone().into_any_element(),
            (Current::Loading, _) => Spinner::new(SpinnerSize::Lg).color(gpui_kit::white()).into_any_element(),
            (Current::Failed, _) => div()
                .px(px(16.))
                .py(px(12.))
                .rounded(theme.radii.lg)
                .bg(theme.colors.canvas)
                .text_size(px(12.))
                .text_color(theme.colors.muted_foreground)
                .child(t!("Image unavailable. The file may have been moved or deleted."))
                .into_any_element(),
        };
        let open_file: Option<OpenFileHandler> = item.as_ref().and_then(|item| item.relative_path.clone()).map(|path| {
            let dialog = cx.entity().downgrade();
            std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
                dialog.update(cx, |_, cx| cx.emit(ExpandedImageEvent::OpenFile(path.clone()))).ok();
            }) as OpenFileHandler
        });
        let menu = self.menu.zip(self.action_source()).map(|(position, source)| {
            let dialog = cx.entity().downgrade();
            let popup = media_actions_menu(&source, open_file).on_dismiss(move |_, cx| {
                dialog.update(cx, |dialog, cx| {
                    dialog.menu = None;
                    cx.notify();
                })
                .ok();
            });
            deferred(anchored().position(position).snap_to_window_with_margin(px(8.)).child(popup)).with_priority(3)
        });
        let popup_width = viewport.width * 0.92;
        let popup_height = viewport.height * 0.92;
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("expanded-image-dialog")
                    .track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .bg(gpui_kit::black().opacity(0.75))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        if this.menu.take().is_none() {
                            cx.emit(ExpandedImageEvent::Close);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id("expanded-image-popup")
                            .relative()
                            .w(popup_width)
                            .h(popup_height)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .id("expanded-image-content")
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                            this.menu = Some(event.position);
                                            cx.stop_propagation();
                                            cx.notify();
                                        }),
                                    )
                                    .child(content)
                                    .child(
                                        div()
                                            .mt(px(8.))
                                            .max_w(popup_width)
                                            .text_size(px(12.))
                                            .text_color(gpui_kit::white().opacity(0.8))
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(caption),
                                    ),
                            )
                            .child(
                                div()
                                    .id("expanded-image-back")
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .h(px(32.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .rounded(theme.radii.lg)
                                    .bg(gpui_kit::black().opacity(0.5))
                                    .hover(|el| el.bg(gpui_kit::black().opacity(0.7)))
                                    .text_size(px(13.))
                                    .text_color(gpui_kit::white())
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ExpandedImageEvent::Close)))
                                    .child(Icon::new(IconName::ArrowLeft).size(IconSize::Md).color(gpui_kit::white()))
                                    .child(t!("Back")),
                            )
                            .when(count > 1, |el| {
                                el.child(div().absolute().left_0().top(popup_height / 2. - px(18.)).child(self.nav_button(
                                    "expanded-image-previous",
                                    IconName::ChevronLeft,
                                    -1,
                                    cx,
                                )))
                                .child(div().absolute().right_0().top(popup_height / 2. - px(18.)).child(self.nav_button(
                                    "expanded-image-next",
                                    IconName::ChevronRight,
                                    1,
                                    cx,
                                )))
                            }),
                    )
                    .children(menu),
            ),
        )
        .with_priority(2)
    }
}
