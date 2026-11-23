//! One streamed browser tab. Ports `browser/ServerBrowserSurface.tsx`: the
//! server's JPEG frames decoded off the UI thread and painted scaled to the
//! pane, mouse, wheel and keys sent back in page CSS pixels, the control bar,
//! page dialogs, the file chooser, and the refused, host-setup and stalled states.
//!
//! Memory: at most three frames exist per tab: one encoded frame waiting in
//! [`LatestFrame`], one being decoded, and the one on screen. Older frames
//! are dropped, and the on-screen frame's GPU texture is freed when replaced.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, ImageSource, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers,
    MouseButton as GpuiButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, Render, RenderImage, ScrollDelta,
    ScrollWheelEvent, SharedString, Task, Window, canvas, div, img, prelude::*, px,
};
use t3code_client_runtime::preview::server_browser_stream::{
    Controller, FileChooser, HostSetup, HostSetupNeed, KeyAction, MouseAction, MouseButton, PreviewStreamControl, PreviewStreamEvent,
    PreviewStreamInput, PreviewStreamTarget, PreviewStreamViewport, modifier_bits,
};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::preview::preview_source::{PreviewSource, PreviewStream};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::{t, t_rich};

/// Wait this long after the last size change before sending it.
const RESIZE_THROTTLE: Duration = Duration::from_millis(150);
/// The server answers a new viewer within seconds; silence means Chromium is not painting.
const NO_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
const MULTI_CLICK: Duration = Duration::from_millis(500);
const MULTI_CLICK_SLOP: f32 = 4.;
const WHEEL_LINE_PX: f32 = 16.;

/// Timings measured while streaming, for checks and the verification report.
#[derive(Clone, Debug, Default)]
pub struct SurfaceMetrics {
    pub frames_painted: u64,
    pub frames_skipped: u64,
    /// JPEG decode plus BGRA conversion, per frame, in milliseconds.
    pub decode_ms: Vec<f64>,
    /// From an input leaving to the next frame reaching the screen, in milliseconds.
    pub input_to_frame_ms: Vec<f64>,
    pub last_frame_size: (u32, u32),
}

/// What the surface tells its tab strip.
#[derive(Clone, Debug)]
pub enum SurfaceEvent {
    /// The page opened a new tab.
    Popup(String),
    /// The tab is gone on the server.
    Gone,
    /// Control changed, for the chrome row.
    Control(Option<PreviewStreamControl>),
}

struct ClickRecord {
    button: MouseButton,
    at: Instant,
    position: Point<Pixels>,
    count: u32,
}

/// The streamed tab view.
pub struct ServerBrowserSurface {
    source: Rc<dyn PreviewSource>,
    thread_id: String,
    tab_id: String,
    stream: Option<PreviewStream>,
    frame: Option<Arc<RenderImage>>,
    frame_size: (u32, u32),
    viewport: Option<PreviewStreamViewport>,
    control: Option<PreviewStreamControl>,
    connected: bool,
    stalled: bool,
    access_denied: bool,
    host_setup: Option<HostSetup>,
    file_chooser: Option<FileChooser>,
    prompt_text: SharedString,
    /// The pane in window pixels, from the last paint.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The pane size last sent, in CSS pixels.
    sent_size: Option<(u32, u32)>,
    /// The frame cap the stream was opened with, in device pixels.
    cap: Option<(u32, u32)>,
    buttons: u8,
    last_click: Option<ClickRecord>,
    pending_move: Option<PreviewStreamInput>,
    awaiting_since: Option<Instant>,
    metrics: Arc<Mutex<SurfaceMetrics>>,
    focus: FocusHandle,
    resize_task: Option<Task<()>>,
    stream_tasks: Vec<Task<()>>,
}

impl EventEmitter<SurfaceEvent> for ServerBrowserSurface {}

impl Focusable for ServerBrowserSurface {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `(key, code, keyCode, text)` for a GPUI key, as DOM `KeyboardEvent` names it.
pub fn dom_key(key: &str, key_char: Option<&str>, shortcut: bool) -> Option<(String, String, Option<u32>, Option<String>)> {
    let named = |k: &str, code: &str, key_code: u32| Some((k.to_owned(), code.to_owned(), Some(key_code), None));
    match key {
        "enter" => Some(("Enter".into(), "Enter".into(), Some(13), (!shortcut).then(|| "\r".into()))),
        "backspace" => named("Backspace", "Backspace", 8),
        "tab" => named("Tab", "Tab", 9),
        "escape" => named("Escape", "Escape", 27),
        "delete" => named("Delete", "Delete", 46),
        "left" => named("ArrowLeft", "ArrowLeft", 37),
        "up" => named("ArrowUp", "ArrowUp", 38),
        "right" => named("ArrowRight", "ArrowRight", 39),
        "down" => named("ArrowDown", "ArrowDown", 40),
        "home" => named("Home", "Home", 36),
        "end" => named("End", "End", 35),
        "pageup" => named("PageUp", "PageUp", 33),
        "pagedown" => named("PageDown", "PageDown", 34),
        "space" => Some((" ".into(), "Space".into(), Some(32), (!shortcut).then(|| " ".into()))),
        _ => {
            let typed = key_char.filter(|c| c.chars().count() == 1).unwrap_or(key);
            let mut chars = key.chars();
            let (first, rest) = (chars.next()?, chars.next());
            if rest.is_some() {
                return None;
            }
            let (code, key_code) = if first.is_ascii_alphabetic() {
                (format!("Key{}", first.to_ascii_uppercase()), Some(first.to_ascii_uppercase() as u32))
            } else if first.is_ascii_digit() {
                (format!("Digit{first}"), Some(first as u32))
            } else {
                (String::new(), None)
            };
            // Only printable keys without Ctrl or Meta carry text.
            Some((typed.to_owned(), code, key_code, (!shortcut).then(|| typed.to_owned())))
        }
    }
}

fn modifiers_of(m: &Modifiers) -> u8 {
    modifier_bits(m.alt, m.control, m.platform, m.shift)
}

fn button_of(button: GpuiButton) -> (MouseButton, u8) {
    match button {
        GpuiButton::Left => (MouseButton::Left, 1),
        GpuiButton::Right => (MouseButton::Right, 2),
        GpuiButton::Middle => (MouseButton::Middle, 4),
        _ => (MouseButton::None, 0),
    }
}

/// Decodes a JPEG straight into the BGRA frame GPUI uploads.
fn decode(jpeg: &[u8]) -> Option<(Arc<RenderImage>, (u32, u32))> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    let pixels = decoder.decode().ok()?;
    let (width, height) = decoder.dimensions()?;
    let (width, height) = (width as u32, height as u32);
    let buffer = image::RgbaImage::from_raw(width, height, pixels)?;
    Some((Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])), (width, height)))
}

impl ServerBrowserSurface {
    pub fn new(source: Rc<dyn PreviewSource>, thread_id: String, tab_id: String, cx: &mut Context<Self>) -> Self {
        Self {
            source,
            thread_id,
            tab_id,
            stream: None,
            frame: None,
            frame_size: (0, 0),
            viewport: None,
            control: None,
            connected: false,
            stalled: false,
            access_denied: false,
            host_setup: None,
            file_chooser: None,
            prompt_text: SharedString::default(),
            bounds: Rc::default(),
            sent_size: None,
            cap: None,
            buttons: 0,
            last_click: None,
            pending_move: None,
            awaiting_since: None,
            metrics: Arc::default(),
            focus: cx.focus_handle(),
            resize_task: None,
            stream_tasks: Vec::new(),
        }
    }

    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }

    /// Shared timings; the verification harness reads them.
    pub fn metrics(&self) -> Arc<Mutex<SurfaceMetrics>> {
        self.metrics.clone()
    }

    pub fn control(&self) -> Option<&PreviewStreamControl> {
        self.control.as_ref()
    }

    /// Sends input when this viewer has control. Navigation from the chrome row uses it too.
    pub fn send(&mut self, input: PreviewStreamInput) -> bool {
        let Some(stream) = &self.stream else { return false };
        let sent = stream.send(input);
        if sent && self.awaiting_since.is_none() {
            self.awaiting_since = Some(Instant::now());
        }
        sent
    }

    fn has_control(&self) -> bool {
        self.control.as_ref().is_some_and(|c| c.controller == Controller::You)
    }

    /// Opens (or reopens with a larger cap) the stream.
    fn start_stream(&mut self, cap: (u32, u32), cx: &mut Context<Self>) {
        self.stream_tasks.clear();
        self.stream = None;
        self.cap = Some(cap);
        let mut stream = self.source.stream(PreviewStreamTarget {
            thread_id: self.thread_id.clone(),
            tab_id: self.tab_id.clone(),
            max_width: cap.0,
            max_height: cap.1,
            interactive: true,
        });
        let frames = stream.frames.clone();
        let mut events = std::mem::replace(&mut stream.events, tokio::sync::mpsc::unbounded_channel().1);
        self.stream = Some(stream);
        let metrics = self.metrics.clone();
        let decoder = cx.spawn(async move |this, cx| {
            let mut last_sequence = 0;
            loop {
                let (sequence, jpeg) = frames.next().await;
                let started = Instant::now();
                let decoded = cx.background_spawn(async move { decode(&jpeg) }).await;
                let Some((image, size)) = decoded else { continue };
                {
                    let mut metrics = metrics.lock().unwrap();
                    metrics.decode_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                    metrics.frames_skipped += sequence.saturating_sub(last_sequence + 1);
                }
                last_sequence = sequence;
                let alive = this.update(cx, |this, cx| this.show_frame(image, size, cx));
                if alive.is_err() {
                    break;
                }
            }
        });
        let listener = cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this.update(cx, |this, cx| this.on_stream_event(event, cx)).is_err() {
                    break;
                }
            }
        });
        let stall = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NO_FRAME_TIMEOUT).await;
            this.update(cx, |this, cx| {
                if this.frame.is_none() {
                    this.stalled = true;
                    cx.notify();
                }
            })
            .ok();
        });
        self.stream_tasks = vec![decoder, listener, stall];
    }

    fn show_frame(&mut self, image: Arc<RenderImage>, size: (u32, u32), cx: &mut Context<Self>) {
        if let Some(old) = self.frame.replace(image) {
            // Frees the old frame's texture; GPUI keeps uploaded images until told.
            cx.drop_image(old, None);
        }
        self.frame_size = size;
        self.stalled = false;
        let mut metrics = self.metrics.lock().unwrap();
        metrics.frames_painted += 1;
        metrics.last_frame_size = size;
        if let Some(since) = self.awaiting_since.take() {
            metrics.input_to_frame_ms.push(since.elapsed().as_secs_f64() * 1000.0);
        }
        drop(metrics);
        cx.notify();
    }

    fn on_stream_event(&mut self, event: PreviewStreamEvent, cx: &mut Context<Self>) {
        match event {
            PreviewStreamEvent::Connected(connected) => {
                self.connected = connected;
                if !connected {
                    self.file_chooser = None;
                    self.buttons = 0;
                }
            }
            PreviewStreamEvent::Viewport(viewport) => self.viewport = Some(viewport),
            PreviewStreamEvent::Control(next) => {
                let gained = next.as_ref().is_some_and(|c| c.controller == Controller::You)
                    && !self.control.as_ref().is_some_and(|c| c.controller == Controller::You);
                if next.as_ref().and_then(|c| c.dialog.as_ref()) != self.control.as_ref().and_then(|c| c.dialog.as_ref()) {
                    self.prompt_text = next.as_ref().and_then(|c| c.dialog.as_ref()).map(|d| d.default_value.clone().into()).unwrap_or_default();
                }
                self.control = next.clone();
                if gained && let Some((width, height)) = self.sent_size {
                    self.send(PreviewStreamInput::Resize { width, height });
                }
                cx.emit(SurfaceEvent::Control(next));
            }
            PreviewStreamEvent::Clipboard(text) => cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text)),
            PreviewStreamEvent::Download { file_name, url, .. } => {
                Toaster::push(
                    ToastData::new(ToastKind::Info, t!("Downloaded {fileName}", fileName = file_name)).action(
                        t!("Save", context = "verb, saves a downloaded file"),
                        move |_, cx| cx.open_url(&url),
                    ),
                    cx,
                );
            }
            PreviewStreamEvent::Popup(tab) => cx.emit(SurfaceEvent::Popup(tab)),
            PreviewStreamEvent::FileChooser(chooser) => self.file_chooser = chooser,
            PreviewStreamEvent::Unauthorized => self.access_denied = true,
            PreviewStreamEvent::Gone => cx.emit(SurfaceEvent::Gone),
            PreviewStreamEvent::HostSetup(setup) => self.host_setup = Some(setup),
            PreviewStreamEvent::Probe { .. } | PreviewStreamEvent::Pointer { .. } => return,
        }
        cx.notify();
    }

    /// Follows the pane's size: sends the CSS size to the page and grows the
    /// frame cap, which reopens the stream once resizing settles.
    fn on_bounds(&mut self, bounds: Bounds<Pixels>, scale: f32, cx: &mut Context<Self>) {
        let size = (f32::from(bounds.size.width).round() as u32, f32::from(bounds.size.height).round() as u32);
        if size.0 < 1 || size.1 < 1 || self.sent_size == Some(size) {
            return;
        }
        let first = self.sent_size.is_none();
        self.sent_size = Some(size);
        let cap = ((size.0 as f32 * scale).round() as u32, (size.1 as f32 * scale).round() as u32);
        self.resize_task = Some(cx.spawn(async move |this, cx| {
            if !first {
                cx.background_executor().timer(RESIZE_THROTTLE).await;
            }
            this.update(cx, |this, cx| {
                let Some((width, height)) = this.sent_size else { return };
                this.send(PreviewStreamInput::Resize { width, height });
                let grow = this.cap.is_none_or(|current| cap.0 > current.0 || cap.1 > current.1);
                if grow {
                    let merged = this.cap.map_or(cap, |c| (c.0.max(cap.0), c.1.max(cap.1)));
                    this.start_stream(merged, cx);
                }
            })
            .ok();
        }));
    }

    /// The page point under `position`, in CSS pixels, using the contain fit.
    fn page_point(&self, position: Point<Pixels>, clamp: bool) -> Option<(f64, f64)> {
        let viewport = self.viewport?;
        if self.frame.is_none() || !self.has_control() {
            return None;
        }
        let bounds = self.bounds.get();
        let (fw, fh) = (self.frame_size.0 as f32, self.frame_size.1 as f32);
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let fit = (bw / fw).min(bh / fh);
        let (width, height) = (fw * fit, fh * fit);
        if !(width > 0. && height > 0.) {
            return None;
        }
        let local = position - bounds.origin;
        let x = (f32::from(local.x) - (bw - width) / 2.) as f64 * viewport.width / width as f64;
        let y = (f32::from(local.y) - (bh - height) / 2.) as f64 * viewport.height / height as f64;
        let inside = x >= 0. && y >= 0. && x <= viewport.width && y <= viewport.height;
        if !inside && !clamp {
            return None;
        }
        Some((x.clamp(0., viewport.width), y.clamp(0., viewport.height)))
    }

    /// Where page point `(x, y)` in CSS pixels sits in the window: the
    /// inverse of the input mapping, for checks that click page content.
    pub fn window_point(&self, x: f64, y: f64) -> Option<Point<Pixels>> {
        let viewport = self.viewport?;
        let bounds = self.bounds.get();
        let (fw, fh) = (self.frame_size.0 as f32, self.frame_size.1 as f32);
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let fit = (bw / fw).min(bh / fh);
        let (width, height) = (fw * fit, fh * fit);
        let px_x = (bw - width) / 2. + (x / viewport.width) as f32 * width;
        let px_y = (bh - height) / 2. + (y / viewport.height) as f32 * height;
        Some(bounds.origin + gpui_kit::point(px(px_x), px(px_y)))
    }

    fn count_click(&mut self, button: MouseButton, position: Point<Pixels>) -> u32 {
        let now = Instant::now();
        let count = match &self.last_click {
            Some(last)
                if last.button == button
                    && now - last.at < MULTI_CLICK
                    && f32::from((position - last.position).x).hypot(f32::from((position - last.position).y)) < MULTI_CLICK_SLOP =>
            {
                last.count + 1
            }
            _ => 1,
        };
        self.last_click = Some(ClickRecord { button, at: now, position, count });
        count
    }

    fn flush_move(&mut self) {
        if let Some(input) = self.pending_move.take() {
            self.send(input);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Thumb buttons drive this tab's history, never the page.
        match event.button {
            GpuiButton::Navigate(gpui_kit::NavigationDirection::Back) => return void(self.send(PreviewStreamInput::History { delta: -1 })),
            GpuiButton::Navigate(gpui_kit::NavigationDirection::Forward) => return void(self.send(PreviewStreamInput::History { delta: 1 })),
            _ => {}
        }
        window.focus(&self.focus, cx);
        let Some((x, y)) = self.page_point(event.position, false) else { return };
        self.flush_move();
        let (button, bit) = button_of(event.button);
        self.buttons |= bit;
        let click_count = self.count_click(button, event.position);
        let input = PreviewStreamInput::Mouse {
            action: MouseAction::Down,
            x,
            y,
            button,
            buttons: self.buttons,
            click_count,
            modifiers: modifiers_of(&event.modifiers),
        };
        self.send(input);
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        let (button, bit) = button_of(event.button);
        if self.buttons & bit == 0 {
            return;
        }
        self.buttons &= !bit;
        let Some((x, y)) = self.page_point(event.position, true) else { return };
        self.flush_move();
        let click_count = self.last_click.as_ref().filter(|c| c.button == button).map_or(1, |c| c.count);
        self.send(PreviewStreamInput::Mouse {
            action: MouseAction::Up,
            x,
            y,
            button,
            buttons: self.buttons,
            click_count,
            modifiers: modifiers_of(&event.modifiers),
        });
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((x, y)) = self.page_point(event.position, self.buttons != 0) else { return };
        let button = if self.buttons & 1 != 0 {
            MouseButton::Left
        } else if self.buttons & 2 != 0 {
            MouseButton::Right
        } else if self.buttons & 4 != 0 {
            MouseButton::Middle
        } else {
            MouseButton::None
        };
        let first = self.pending_move.is_none();
        self.pending_move = Some(PreviewStreamInput::Mouse {
            action: MouseAction::Move,
            x,
            y,
            button,
            buttons: self.buttons,
            click_count: 0,
            modifiers: modifiers_of(&event.modifiers),
        });
        // Moves coalesce to one per frame, as `requestAnimationFrame` does on the web.
        if first {
            let entity = cx.entity();
            window.on_next_frame(move |_, cx| entity.update(cx, |this, _| this.flush_move()));
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((x, y)) = self.page_point(event.position, false) else { return };
        let (dx, dy) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            ScrollDelta::Lines(delta) => (delta.x * WHEEL_LINE_PX, delta.y * WHEEL_LINE_PX),
        };
        // GPUI's wheel deltas point the way content moves; the page wants DOM wheel deltas.
        self.send(PreviewStreamInput::Wheel { x, y, delta_x: -dx as f64, delta_y: -dy as f64, modifiers: modifiers_of(&event.modifiers) });
        cx.stop_propagation();
    }

    fn on_key(&mut self, action: KeyAction, keystroke: &gpui_kit::Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_control() {
            return;
        }
        let m = &keystroke.modifiers;
        // Shift+Escape leaves the page.
        if keystroke.key == "escape" && m.shift {
            if action == KeyAction::Down {
                window.blur(cx);
            }
            cx.stop_propagation();
            return;
        }
        let shortcut = m.control || m.platform;
        if shortcut && keystroke.key == "v" && action == KeyAction::Down {
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.send(PreviewStreamInput::Text { text });
            }
            cx.stop_propagation();
            return;
        }
        let Some((key, code, key_code, text)) = dom_key(&keystroke.key, keystroke.key_char.as_deref(), shortcut) else { return };
        self.send(PreviewStreamInput::Key {
            action,
            key,
            code,
            key_code,
            text: if action == KeyAction::Down { text } else { None },
            modifiers: modifiers_of(m),
        });
        if !shortcut {
            cx.stop_propagation();
        }
    }

    fn choose_files(&mut self, cx: &mut Context<Self>) {
        let Some(chooser) = self.file_chooser.clone() else { return };
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: chooser.multiple, prompt: None });
        cx.spawn(async move |this, cx| {
            let files: Vec<(String, Vec<u8>)> = match paths.await {
                Ok(Ok(Some(paths))) => paths
                    .into_iter()
                    .filter_map(|path| {
                        let name = path.file_name()?.to_string_lossy().into_owned();
                        Some((name, std::fs::read(&path).ok()?))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            this.update(cx, |this, cx| this.answer_file_chooser(files, cx)).ok();
        })
        .detach();
    }

    fn answer_file_chooser(&mut self, files: Vec<(String, Vec<u8>)>, cx: &mut Context<Self>) {
        let Some(chooser) = self.file_chooser.take() else { return };
        cx.notify();
        if !crate::connection::ConnectionRuntime::is_running(cx) {
            return;
        }
        let task = crate::connection::ConnectionRuntime::global(cx).spawn(
            async move {
                t3code_client_runtime::preview::server_browser_stream::upload_files(&chooser, files)
                    .await
                    .map_err(crate::connection::CommandError::from)
            },
            cx,
        );
        cx.spawn(async move |_, cx| {
            if let Err(error) = task.await {
                cx.update(|cx| {
                    Toaster::push(ToastData::new(ToastKind::Error, t!("Could not send the files to the page")).description(error.0), cx);
                });
            }
        })
        .detach();
    }

    fn control_label(&self) -> SharedString {
        match &self.control {
            None => t!("Connecting..."),
            Some(c) if !c.can_operate => t!("Read-only"),
            Some(c) => match c.controller {
                Controller::You => t!("You have control"),
                Controller::Agent => t!("Agent has control"),
                Controller::AnotherViewer => t!("Another viewer has control"),
                Controller::Unclaimed => t!("Watching", context = "preview stream status, viewing without control"),
            },
        }
    }
}

fn void(_: bool) {}

impl Render for ServerBrowserSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let bounds_cell = self.bounds.clone();
        let entity = cx.entity();
        let control_bar = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(theme.sp(2.))
            .px(theme.sp(2.))
            .py(theme.sp(1.))
            .border_b_1()
            .border_color(c.border)
            .bg(c.background())
            .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(self.control_label()))
            .children(self.control.as_ref().filter(|c| c.can_operate).map(|control| {
                let you = control.controller == Controller::You;
                Button::new("preview-control")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Xs)
                    .label(if you { t!("Release control") } else { t!("Take control") })
                    .disabled(control.controller == Controller::AnotherViewer)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(if you { PreviewStreamInput::ReleaseControl } else { PreviewStreamInput::TakeControl });
                        cx.notify();
                    }))
            }));
        let frame = self.frame.clone();
        let pane = div()
            .id("preview-pane")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(c.background())
            .track_focus(&self.focus)
            .key_context("PreviewSurface")
            .child(
                canvas(
                    move |bounds, window, cx| {
                        bounds_cell.set(bounds);
                        let scale = window.scale_factor();
                        entity.update(cx, |this, cx| this.on_bounds(bounds, scale, cx));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(frame.map(|frame| img(ImageSource::Render(frame)).object_fit(ObjectFit::Contain).size_full()))
            .on_mouse_down(GpuiButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(GpuiButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(GpuiButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(GpuiButton::Navigate(gpui_kit::NavigationDirection::Back), cx.listener(Self::on_mouse_down))
            .on_mouse_down(GpuiButton::Navigate(gpui_kit::NavigationDirection::Forward), cx.listener(Self::on_mouse_down))
            .on_mouse_up(GpuiButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(GpuiButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(GpuiButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(KeyAction::Down, &event.keystroke, window, cx)))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| this.on_key(KeyAction::Up, &event.keystroke, window, cx)));
        let overlay = |content: gpui_kit::Div| {
            div()
                .absolute()
                .top(theme.sp(2.))
                .left(theme.sp(2.))
                .right(theme.sp(2.))
                .flex()
                .flex_col()
                .gap(theme.sp(2.))
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(c.border)
                .bg(c.background())
                .p(theme.sp(3.))
                .shadow(theme.shadows.lg.clone())
                .child(content)
        };
        let dialog = self.control.as_ref().and_then(|control| control.dialog.clone().map(|dialog| (control.controller == Controller::You, dialog))).map(
            |(you, dialog)| {
                let prompt = dialog.r#type == "prompt";
                let prompt_text = self.prompt_text.to_string();
                overlay(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(2.))
                        .child(div().ui_text(&theme, TextSize::Sm, weight::NORMAL).child(dialog.message.clone()))
                        .when(prompt && you, |el| {
                            el.child(
                                div()
                                    .rounded(theme.radii.md)
                                    .border_1()
                                    .border_color(c.input)
                                    .px(theme.sp(2.))
                                    .py(theme.sp(1.))
                                    .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                                    .child(if prompt_text.is_empty() { " ".to_owned() } else { prompt_text.clone() }),
                            )
                        })
                        .child(if you {
                            div()
                                .flex()
                                .justify_end()
                                .gap(theme.sp(2.))
                                .child(Button::new("dialog-dismiss").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Dismiss")).on_click(
                                    cx.listener(|this, _, _, _| void(this.send(PreviewStreamInput::Dialog { accept: false, prompt_text: None }))),
                                ))
                                .child(Button::new("dialog-accept").size(ButtonSize::Sm).label(t!("Accept")).on_click(cx.listener(move |this, _, _, _| {
                                    let prompt_text = prompt.then(|| this.prompt_text.to_string());
                                    void(this.send(PreviewStreamInput::Dialog { accept: true, prompt_text }))
                                })))
                                .into_any_element()
                        } else {
                            div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("Take control to respond.")).into_any_element()
                        }),
                )
            },
        );
        let chooser = self.file_chooser.clone().filter(|_| self.has_control()).map(|chooser| {
            overlay(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(2.))
                    .child(div().ui_text(&theme, TextSize::Sm, weight::NORMAL).child(if chooser.multiple {
                        t!("The page asks for files.")
                    } else {
                        t!("The page asks for a file.")
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(theme.sp(2.))
                            .child(
                                Button::new("chooser-cancel")
                                    .variant(ButtonVariant::Outline)
                                    .size(ButtonSize::Sm)
                                    .label(t!("Cancel"))
                                    .on_click(cx.listener(|this, _, _, cx| this.answer_file_chooser(Vec::new(), cx))),
                            )
                            .child(
                                Button::new("chooser-choose")
                                    .size(ButtonSize::Sm)
                                    .label(if chooser.multiple { t!("Choose files") } else { t!("Choose file") })
                                    .on_click(cx.listener(|this, _, _, cx| this.choose_files(cx))),
                            ),
                    ),
            )
        });
        let blocking = if let Some(setup) = &self.host_setup {
            Some(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(theme.sp(3.))
                    .child(
                        div().max_w(theme.rem(24.)).ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(match setup.need {
                            HostSetupNeed::Sandbox => t!(
                                "This server's host blocks the sandbox its browser runs in. Run this once on the host, then try again:"
                            ),
                            HostSetupNeed::Libraries => {
                                t!("This server's host is missing libraries its browser needs. Run this once on the host, then try again:")
                            }
                        }),
                    )
                    .child(
                        div()
                            .rounded(theme.radii.md)
                            .bg(theme.derived.inline_code)
                            .px(theme.sp(3.))
                            .py(theme.sp(2.))
                            .mono_text(px(12.), weight::NORMAL)
                            .child(setup.command.clone()),
                    )
                    .child(Button::new("host-setup-retry").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Try again")).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.host_setup = None;
                            if let Some(cap) = this.cap {
                                this.start_stream(cap, cx);
                            }
                        }),
                    )),
            )
        } else if self.access_denied {
            Some(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("Browser connection was refused.")))
                    .child(Button::new("preview-reconnect").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Reconnect")).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.access_denied = false;
                            if let Some(cap) = this.cap {
                                this.start_stream(cap, cx);
                            }
                        }),
                    )),
            )
        } else {
            None
        };
        let stalled = (self.stalled && self.frame.is_none() && blocking.is_none()).then(|| {
            div().absolute().inset_0().flex().items_center().justify_center().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(
                if self.connected { SharedString::from(t_rich!("This site can’t be reached").plain()) } else { t!("Connecting...") },
            )
        });
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(control_bar)
            .child(div().relative().flex_1().min_h_0().flex().flex_col().child(pane).children(dialog).children(chooser).children(stalled))
            .children(blocking.map(|content| {
                div().absolute().inset_0().flex().items_center().justify_center().p(theme.sp(4.)).bg(c.background()).child(content)
            }))
    }
}
