//! One terminal on screen: owns the model and its backend, turns keyboard,
//! IME, mouse and wheel input into bytes, and keeps the cursor blink,
//! selection, context menu and link hover state. Ports `terminal/ghostty/surface.ts`.

use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::index::{Column, Point as GridPoint, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use futures::StreamExt as _;
use gpui_kit::*;

use super::backend::{GridSize, InputKind, TerminalBackend, TerminalInbound, TerminalOutbound, TerminalSink, inbound_channel};
use super::core::TerminalCore;
use super::key_codes::{KeyEventKind, encode_key};
use super::mouse::{MouseAction, alternate_scroll, mouse_report, reports_mouse};
use super::palette::{TerminalPalette, to_hsla};
use super::renderer::{CellMetrics, GridGeometry, GridLayout, TerminalGrid, terminal_font, terminal_font_size};
use crate::terminal_links::{TerminalLinkKind, is_terminal_url, terminal_link_at};
use crate::components::ui::menu::{MenuItem, MenuPopup};
use crate::i18n::t;
use crate::theme_palette::{ActiveAppTheme as _, Appearance};

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
/// The PTY hears only settled sizes; reflowing the shell on every drag step jitters.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);
/// Output is parsed in slices this long so a flood cannot stall a frame.
const PARSE_BUDGET: Duration = Duration::from_millis(8);
const SYNCHRONIZED_OUTPUT_TIMEOUT: Duration = Duration::from_millis(500);

actions!(
    terminal,
    [
        /// Copies the selection.
        Copy,
        /// Pastes the clipboard, bracketed when the program asked for it.
        Paste,
        SelectAll,
        /// Scrolls history up one page (normal screen only).
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom,
    ]
);

/// Registers the terminal key bindings. Call once at startup.
pub fn init(cx: &mut App) {
    let context = Some("Terminal");
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-c", Copy, context),
            KeyBinding::new("cmd-v", Paste, context),
            KeyBinding::new("cmd-a", SelectAll, context),
            KeyBinding::new("shift-cmd-home", ScrollToTop, context),
            KeyBinding::new("shift-cmd-end", ScrollToBottom, context),
        ]);
    } else {
        cx.bind_keys([
            KeyBinding::new("ctrl-shift-c", Copy, context),
            KeyBinding::new("ctrl-insert", Copy, context),
            // Plain Ctrl+C copies only while text is selected; otherwise it is SIGINT.
            KeyBinding::new("ctrl-c", Copy, context),
            KeyBinding::new("ctrl-shift-v", Paste, context),
            KeyBinding::new("shift-insert", Paste, context),
            KeyBinding::new("ctrl-shift-a", SelectAll, context),
            KeyBinding::new("ctrl-shift-home", ScrollToTop, context),
            KeyBinding::new("ctrl-shift-end", ScrollToBottom, context),
        ]);
    }
    cx.bind_keys([
        KeyBinding::new("shift-pageup", ScrollPageUp, context),
        KeyBinding::new("shift-pagedown", ScrollPageDown, context),
    ]);
}

/// Session lifecycle as the surface last heard it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Exited,
    Closed,
}

/// What a surface tells its owner.
#[derive(Clone, Debug)]
pub enum TerminalSurfaceEvent {
    /// A URL or path link was activated with the platform's link modifier.
    LinkActivated { kind: TerminalLinkKind, text: SharedString },
    TitleChanged(Option<SharedString>),
    /// "Add to chat" on the selection: its text (CRLF folded, outer blank lines trimmed)
    /// and 1-based buffer lines.
    AddToChat { text: SharedString, line_start: usize, line_end: usize },
    /// The process exited or the session closed; the owner may close the tab.
    SessionEnded(SessionStatus),
    Bell,
}

impl EventEmitter<TerminalSurfaceEvent> for TerminalSurface {}

#[derive(Clone, Copy, PartialEq)]
enum Pointer {
    Idle,
    Selecting,
    Reporting(MouseButton),
}

/// A link under the pointer: where to underline and what it opens.
#[derive(Clone, Debug, PartialEq)]
struct HoveredLink {
    kind: TerminalLinkKind,
    text: SharedString,
    start: GridPoint,
    end: GridPoint,
}


/// Cursor to paint this frame.
pub struct CursorPaint {
    pub row: usize,
    pub col: usize,
    pub wide: bool,
    pub shape: CursorShape,
    pub color: Hsla,
    pub glyph: Option<ShapedLine>,
}

/// IME composition text drawn at the cursor.
pub struct MarkedPaint {
    pub row: usize,
    pub col: usize,
    pub line: ShapedLine,
}

/// Everything painted over the cached rows: selection, matches, link, cursor, scrollbar.
pub struct FrameOverlay {
    pub background: Hsla,
    /// (row, col, cols, colour) rectangles painted under text.
    pub highlights: Vec<(usize, usize, usize, Hsla)>,
    /// (row, col, cols, colour) underlines painted over text.
    pub underlines: Vec<(usize, usize, usize, Hsla)>,
    pub cursor: Option<CursorPaint>,
    pub marked_text: Option<MarkedPaint>,
    /// Thumb top offset, height and colour.
    pub scrollbar: Option<(Pixels, Pixels, Hsla)>,
}

/// A terminal view. Create with [`TerminalSurface::new`] and a backend.
pub struct TerminalSurface {
    core: TerminalCore,
    backend: Box<dyn TerminalBackend>,
    pending_sink: Option<TerminalSink>,
    focus: FocusHandle,
    palette: TerminalPalette,
    palette_generation: u64,
    theme_key: (String, Appearance),
    layout: Option<GridLayout>,
    geometry: Option<GridGeometry>,
    font: Font,
    font_size: Pixels,
    cell_size: (f32, f32),
    focused: bool,
    cursor_on: bool,
    blink_task: Option<Task<()>>,
    resize_task: Option<Task<()>>,
    sync_task: Option<Task<()>>,
    reported_size: Option<GridSize>,
    pointer: Pointer,
    last_motion_cell: Option<(usize, usize)>,
    scroll_remainder: f32,
    marked_text: Option<String>,
    hovered_link: Option<HoveredLink>,
    /// Where the context menu is open, in window coordinates.
    context_menu: Option<Point<Pixels>>,
    status: SessionStatus,
    read_only: bool,
    can_add_to_chat: bool,
    title: Option<SharedString>,
    _drain: Task<()>,
}

impl TerminalSurface {
    /// A surface that connects `backend` once its first layout measures the grid.
    pub fn new(backend: Box<dyn TerminalBackend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (sink, mut source) = inbound_channel();
        let drain = cx.spawn(async move |this, cx| {
            while let Some(first) = source.next().await {
                let mut next = Some(first);
                while let Some(event) = next.take() {
                    let updated = this.update(cx, |this, cx| {
                        let started = Instant::now();
                        let mut event = Some(event);
                        while let Some(current) = event.take() {
                            this.apply_inbound(current, cx);
                            if started.elapsed() >= PARSE_BUDGET {
                                break;
                            }
                            event = source.try_recv().ok();
                        }
                        this.after_output(cx);
                    });
                    if updated.is_err() {
                        return;
                    }
                    next = source.try_recv().ok();
                    if next.is_some() {
                        YieldNow(false).await;
                    }
                }
            }
        });
        let focus = cx.focus_handle();
        cx.on_focus(&focus, window, |this, _, cx| this.set_focused(true, cx)).detach();
        cx.on_blur(&focus, window, |this, _, cx| this.set_focused(false, cx)).detach();
        let theme = cx.app_theme();
        Self {
            core: TerminalCore::new(GridSize { cols: 80, rows: 24 }),
            backend,
            pending_sink: Some(sink),
            focus,
            palette: TerminalPalette::new(theme),
            palette_generation: 0,
            theme_key: (theme.id.clone(), theme.appearance),
            layout: Some(GridLayout::default()),
            geometry: None,
            font: terminal_font(),
            font_size: terminal_font_size(cx),
            cell_size: (7., 16.),
            focused: false,
            cursor_on: true,
            blink_task: None,
            resize_task: None,
            sync_task: None,
            reported_size: None,
            pointer: Pointer::Idle,
            last_motion_cell: None,
            scroll_remainder: 0.,
            marked_text: None,
            hovered_link: None,
            context_menu: None,
            status: SessionStatus::Running,
            read_only: false,
            can_add_to_chat: false,
            title: None,
            _drain: drain,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn font(&self) -> Font {
        self.font.clone()
    }

    pub fn font_size(&self) -> Pixels {
        self.font_size
    }

    /// Re-reads the terminal font family and size from the appearance settings.
    fn sync_font(&mut self, cx: &App) {
        let size = terminal_font_size(cx);
        let family = crate::appearance_fonts::terminal_font(FontWeight::NORMAL).family;
        if size != self.font_size || family != self.font.family {
            self.font = terminal_font();
            self.font_size = size;
        }
    }

    /// Whether the context menu offers "Add to chat" (the host takes `AddToChat`).
    pub fn set_can_add_to_chat(&mut self, can_add: bool) {
        self.can_add_to_chat = can_add;
    }

    /// Without the operate grant the terminal shows output but sends nothing.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn status(&self) -> SessionStatus {
        self.status
    }

    pub fn title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    pub fn core(&self) -> &TerminalCore {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut TerminalCore {
        &mut self.core
    }

    /// Parses bytes as if the backend sent them. Scenes and benchmarks use this.
    pub fn write_output(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.core.feed(bytes);
        self.after_output(cx);
    }

    /// Prints a `[terminal]` status line, as the web drawer does for errors and exits.
    pub fn write_system_message(&mut self, message: &str, cx: &mut Context<Self>) {
        self.write_output(format!("\r\n[terminal] {message}\r\n").as_bytes(), cx);
    }

    fn apply_inbound(&mut self, event: TerminalInbound, cx: &mut Context<Self>) {
        match event {
            TerminalInbound::Output(bytes) => self.core.feed(&bytes),
            TerminalInbound::Reset(bytes) => {
                self.core.reset_with(&bytes);
                self.status = SessionStatus::Running;
            }
            TerminalInbound::Cleared => self.core.reset_with(b""),
            TerminalInbound::Error(message) => self.core.feed(format!("\r\n[terminal] {message}\r\n").as_bytes()),
            TerminalInbound::Exited { .. } => self.end_session(SessionStatus::Exited, "Process exited", cx),
            TerminalInbound::Closed => self.end_session(SessionStatus::Closed, "Terminal closed", cx),
        }
    }

    fn end_session(&mut self, status: SessionStatus, message: &str, cx: &mut Context<Self>) {
        if self.status != SessionStatus::Running {
            return;
        }
        self.status = status;
        self.core.feed(format!("\r\n[terminal] {message}\r\n").as_bytes());
        cx.emit(TerminalSurfaceEvent::SessionEnded(status));
    }

    /// Handles parser events and schedules a repaint after new output.
    fn after_output(&mut self, cx: &mut Context<Self>) {
        for event in self.core.take_events() {
            match event {
                TermEvent::PtyWrite(text) => self.send_input(text.into_bytes(), InputKind::Reply),
                TermEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                TermEvent::ColorRequest(index, format) => {
                    let rgb = match index {
                        0..256 => self.palette.indexed(index),
                        256 => self.palette.foreground,
                        257 => self.palette.background,
                        _ => self.palette.foreground,
                    };
                    self.send_input(format(rgb).into_bytes(), InputKind::Reply);
                }
                TermEvent::TextAreaSizeRequest(format) => {
                    let size = self.core.window_size(self.cell_size.0, self.cell_size.1);
                    self.send_input(format(size).into_bytes(), InputKind::Reply);
                }
                TermEvent::Title(title) => {
                    self.title = Some(title.into());
                    cx.emit(TerminalSurfaceEvent::TitleChanged(self.title.clone()));
                }
                TermEvent::ResetTitle => {
                    self.title = None;
                    cx.emit(TerminalSurfaceEvent::TitleChanged(None));
                }
                TermEvent::Bell => cx.emit(TerminalSurfaceEvent::Bell),
                TermEvent::CursorBlinkingChange => self.restart_blink(cx),
                _ => {}
            }
        }
        if let Some(deadline) = self.core.sync_deadline() {
            if self.sync_task.is_none() {
                self.sync_task = Some(cx.spawn(async move |this, cx| {
                    let wait = deadline.saturating_duration_since(Instant::now()).min(SYNCHRONIZED_OUTPUT_TIMEOUT);
                    cx.background_executor().timer(wait).await;
                    this.update(cx, |this, cx| {
                        this.sync_task = None;
                        this.core.flush_expired_sync();
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        cx.notify();
    }

    fn send_input(&mut self, bytes: Vec<u8>, kind: InputKind) {
        if self.read_only || self.status != SessionStatus::Running || bytes.is_empty() {
            return;
        }
        self.backend.send(TerminalOutbound::Input { bytes, kind });
    }

    /// User typing: goes to the program, follows the cursor, restarts the blink.
    fn send_typed(&mut self, bytes: impl Into<Vec<u8>>, cx: &mut Context<Self>) {
        self.send_input(bytes.into(), InputKind::Typed);
        if self.core.display_offset() > 0 {
            self.core.scroll_to_bottom();
        }
        self.restart_blink(cx);
        cx.notify();
    }

    /// Asks the backend to clear server-side history.
    pub fn clear_history(&mut self, cx: &mut Context<Self>) {
        self.backend.send(TerminalOutbound::Clear);
        self.core.reset_with(b"");
        cx.notify();
    }

    /// Asks the backend to close the session.
    pub fn close_session(&mut self) {
        self.backend.send(TerminalOutbound::Close);
    }

    // Layout, called by the grid element.

    /// Follows the element size: reflows now, tells the backend once the size settles.
    pub fn resize_grid(&mut self, size: GridSize, metrics: &CellMetrics, cx: &mut Context<Self>) {
        self.cell_size = (f32::from(metrics.cell_width), f32::from(metrics.line_height));
        let theme = cx.app_theme();
        if (theme.id.as_str(), theme.appearance) != (self.theme_key.0.as_str(), self.theme_key.1) {
            self.theme_key = (theme.id.clone(), theme.appearance);
            self.palette = TerminalPalette::new(theme);
            self.palette_generation += 1;
        }
        self.core.resize(size);
        if let Some(sink) = self.pending_sink.take() {
            // The first measured size opens the session at the right width.
            self.backend.connect(sink, size);
            self.reported_size = Some(size);
            return;
        }
        if self.reported_size == Some(size) {
            self.resize_task = None;
            return;
        }
        self.resize_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESIZE_DEBOUNCE).await;
            this.update(cx, |this, _| this.flush_resize()).ok();
        }));
    }

    /// Sends a pending resize now, for example when a drawer drag ends.
    pub fn flush_resize(&mut self) {
        self.resize_task = None;
        let size = self.core.size();
        if self.reported_size != Some(size) {
            self.reported_size = Some(size);
            self.backend.send(TerminalOutbound::Resize(size));
        }
    }

    /// Sends the current size again: after a reconnect the server has not seen it.
    pub fn resend_size(&mut self) {
        self.reported_size = None;
        self.flush_resize();
    }

    pub(super) fn take_layout(&mut self) -> (GridLayout, TerminalPalette, u64) {
        (self.layout.take().unwrap_or_default(), self.palette.clone(), self.palette_generation)
    }

    pub(super) fn restore_layout(&mut self, layout: GridLayout) {
        self.layout = Some(layout);
    }

    pub(super) fn set_geometry(&mut self, geometry: GridGeometry) {
        self.geometry = Some(geometry);
    }

    /// Builds what is painted over the cached rows this frame.
    pub(super) fn frame_overlay(&mut self, geometry: &GridGeometry, window: &Window, cx: &App) -> FrameOverlay {
        let colors = &cx.app_theme().colors;
        let rows = geometry.size.rows as usize;
        let cols = geometry.size.cols as usize;
        let offset = self.core.display_offset() as i32;
        let mut highlights = Vec::new();
        let push_range = |out: &mut Vec<(usize, usize, usize, Hsla)>, start: GridPoint, end: GridPoint, block: bool, color: Hsla| {
            for line in start.line.0.max(-offset)..=end.line.0.min(rows as i32 - 1 - offset) {
                let row = (line + offset) as usize;
                let first = if block || line == start.line.0 { start.column.0 } else { 0 };
                let last = if block || line == end.line.0 { end.column.0 } else { cols - 1 };
                if last >= first {
                    out.push((row, first, last - first + 1, color));
                }
            }
        };

        if let Some(range) = self.core.term().selection.as_ref().and_then(|s| s.to_range(self.core.term())) {
            push_range(&mut highlights, range.start, range.end, range.is_block, self.palette.selection);
        }

        let mut underlines = Vec::new();
        if let Some(link) = &self.hovered_link {
            let fg = to_hsla(self.palette.foreground);
            push_range(&mut underlines, link.start, link.end, false, fg);
        }

        let term = self.core.term();
        let content = term.renderable_content();
        let cursor_point = content.cursor.point;
        let cursor_row = cursor_point.line.0 + offset;
        let cursor_visible = content.cursor.shape != CursorShape::Hidden
            && (0..rows as i32).contains(&cursor_row)
            && self.marked_text.is_none()
            && (self.cursor_on || !self.focused);
        let cursor = cursor_visible.then(|| {
            let cell = &term.grid()[cursor_point];
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let shape = if self.focused { content.cursor.shape } else { CursorShape::HollowBlock };
            let glyph = (shape == CursorShape::Block && cell.c != ' ').then(|| {
                let mut text = String::from(cell.c);
                text.extend(cell.zerowidth().unwrap_or_default());
                let mut font = self.font.clone();
                if cell.flags.contains(Flags::BOLD) {
                    font.weight = FontWeight::BOLD;
                }
                let run = TextRun { len: text.len(), font, color: self.palette.background_hsla, background_color: None, underline: None, strikethrough: None };
                window.text_system().shape_line(text.into(), self.font_size, &[run], None)
            });
            CursorPaint { row: cursor_row as usize, col: cursor_point.column.0, wide, shape, color: self.palette.cursor, glyph }
        });

        let marked_text = self.marked_text.as_ref().filter(|_| (0..rows as i32).contains(&cursor_row)).map(|text| {
            let run = TextRun {
                len: text.len(),
                font: self.font.clone(),
                color: to_hsla(self.palette.foreground),
                background_color: None,
                underline: Some(UnderlineStyle { color: None, thickness: px(1.), wavy: false }),
                strikethrough: None,
            };
            MarkedPaint {
                row: cursor_row as usize,
                col: cursor_point.column.0,
                line: window.text_system().shape_line(text.clone().into(), self.font_size, &[run], None),
            }
        });

        let history = self.core.history_size();
        let scrollbar = (history > 0 && !self.core.is_alternate_screen()).then(|| {
            let total = (history + rows) as f32;
            let track = geometry.line_height * rows as f32;
            let height = track * (rows as f32 / total);
            let top = track * ((history - self.core.display_offset()) as f32 / total);
            (top, height, self.palette.scrollbar)
        });

        FrameOverlay { background: colors.terminal_background, highlights, underlines, cursor, marked_text, scrollbar }
    }

    // Focus and blink.

    fn set_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        self.core.set_focused(focused);
        if self.core.mode().contains(TermMode::FOCUS_IN_OUT) {
            let report: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.send_input(report.to_vec(), InputKind::Typed);
        }
        if !focused {
            self.hovered_link = None;
        }
        self.restart_blink(cx);
        cx.notify();
    }

    /// Shows the cursor now and blinks it in 500 ms steps while focused and the program asks for blinking.
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_on = true;
        self.blink_task = None;
        if !self.focused || !self.core.cursor_style().blinking {
            return;
        }
        self.blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                let alive = this
                    .update(cx, |this, cx| {
                        this.cursor_on = !this.cursor_on;
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    // Keyboard.

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let kind = if event.is_held { KeyEventKind::Repeat } else { KeyEventKind::Press };
        if let Some(bytes) = encode_key(&event.keystroke, self.core.mode(), kind) {
            self.core.clear_selection();
            self.send_typed(bytes.as_bytes(), cx);
            cx.stop_propagation();
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if self.core.mode().contains(TermMode::REPORT_EVENT_TYPES) {
            if let Some(bytes) = encode_key(&event.keystroke, self.core.mode(), KeyEventKind::Release) {
                self.send_input(bytes.as_bytes().to_vec(), InputKind::Typed);
            }
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.core.selection_text() else {
            // Ctrl+C with nothing selected is an interrupt for the program.
            cx.propagate();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if !cfg!(target_os = "macos") {
            self.core.clear_selection();
            cx.notify();
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    /// Sends pasted text, bracketed when the program enabled bracketed paste.
    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        let bytes = if self.core.mode().contains(TermMode::BRACKETED_PASTE) {
            // A pasted end marker would let the paste escape the bracket.
            let inner = text.replace('\x1b', "");
            format!("\x1b[200~{inner}\x1b[201~")
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.send_typed(bytes, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.core.select_all();
        cx.notify();
    }

    fn scroll_history(&mut self, cx: &mut Context<Self>, scroll: impl FnOnce(&mut TerminalCore)) {
        if self.core.is_alternate_screen() {
            cx.propagate();
            return;
        }
        scroll(&mut self.core);
        cx.notify();
    }

    // Mouse.

    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize, bool)> {
        self.geometry.map(|geometry| geometry.cell_at(position))
    }

    fn link_modifier(modifiers: &Modifiers) -> bool {
        if cfg!(target_os = "macos") {
            modifiers.platform && !modifiers.control
        } else {
            modifiers.control && !modifiers.platform
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((row, col, right_half)) = self.cell_at(event.position) else { return };
        let mode = self.core.mode();

        if event.button == MouseButton::Left && Self::link_modifier(&event.modifiers) {
            self.update_hovered_link(event.position, event.modifiers, cx);
            if let Some(link) = self.hovered_link.clone() {
                cx.emit(TerminalSurfaceEvent::LinkActivated { kind: link.kind, text: link.text });
                return;
            }
        }

        if reports_mouse(mode, event.modifiers) {
            if let Some(report) = mouse_report(MouseAction::Press(event.button), row, col, event.modifiers, mode) {
                self.send_input(report, InputKind::Typed);
            }
            self.pointer = Pointer::Reporting(event.button);
            return;
        }
        if event.button == MouseButton::Right {
            self.context_menu = Some(event.position);
            cx.notify();
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let point = self.core.viewport_point(row, col);
        let side = if right_half { Side::Right } else { Side::Left };
        if event.modifiers.shift && self.core.has_selection() {
            self.core.update_selection(point, side);
        } else {
            let kind = match event.click_count {
                2 => SelectionType::Semantic,
                n if n >= 3 => SelectionType::Lines,
                _ if event.modifiers.alt => SelectionType::Block,
                _ => SelectionType::Simple,
            };
            self.core.start_selection(point, side, kind);
        }
        self.pointer = Pointer::Selecting;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((row, col, right_half)) = self.cell_at(event.position) else { return };
        match self.pointer {
            Pointer::Selecting if event.pressed_button == Some(MouseButton::Left) => {
                // Dragging past the top or bottom edge scrolls history.
                if let Some(geometry) = self.geometry {
                    let bottom = geometry.origin.y + geometry.line_height * geometry.size.rows as f32;
                    if event.position.y < geometry.origin.y {
                        self.core.scroll_lines(1);
                    } else if event.position.y > bottom {
                        self.core.scroll_lines(-1);
                    }
                }
                let point = self.core.viewport_point(row, col);
                self.core.update_selection(point, if right_half { Side::Right } else { Side::Left });
                cx.notify();
            }
            _ => {
                let mode = self.core.mode();
                if (matches!(self.pointer, Pointer::Reporting(_)) || reports_mouse(mode, event.modifiers))
                    && self.last_motion_cell != Some((row, col))
                {
                    self.last_motion_cell = Some((row, col));
                    let held = match self.pointer {
                        Pointer::Reporting(button) => Some(button),
                        _ => event.pressed_button,
                    };
                    if let Some(report) = mouse_report(MouseAction::Move(held), row, col, event.modifiers, mode) {
                        self.send_input(report, InputKind::Typed);
                    }
                }
                self.update_hovered_link(event.position, event.modifiers, cx);
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        match std::mem::replace(&mut self.pointer, Pointer::Idle) {
            Pointer::Reporting(button) => {
                if let Some((row, col, _)) = self.cell_at(event.position) {
                    if let Some(report) = mouse_report(MouseAction::Release(button), row, col, event.modifiers, self.core.mode()) {
                        self.send_input(report, InputKind::Typed);
                    }
                }
            }
            Pointer::Selecting => {
                if !self.core.has_selection() {
                    self.core.clear_selection();
                }
                cx.notify();
            }
            Pointer::Idle => {}
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line_height = self.geometry.map_or(px(16.), |geometry| geometry.line_height);
        let delta = event.delta.pixel_delta(line_height).y / line_height + self.scroll_remainder;
        let lines = delta.trunc() as i32;
        self.scroll_remainder = delta.fract();
        if lines == 0 {
            return;
        }
        let mode = self.core.mode();
        if reports_mouse(mode, event.modifiers) {
            if let Some((row, col, _)) = self.cell_at(event.position) {
                let action = if lines > 0 { MouseAction::WheelUp } else { MouseAction::WheelDown };
                for _ in 0..lines.unsigned_abs() {
                    if let Some(report) = mouse_report(action, row, col, event.modifiers, mode) {
                        self.send_input(report, InputKind::Typed);
                    }
                }
            }
        } else if let Some(arrows) = alternate_scroll(lines, mode) {
            self.send_input(arrows, InputKind::Typed);
        } else {
            self.core.scroll_lines(lines);
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn on_modifiers_changed(&mut self, event: &ModifiersChangedEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.update_hovered_link(window.mouse_position(), event.modifiers, cx);
    }

    /// Underlines the link under the pointer while the link modifier is held.
    fn update_hovered_link(&mut self, position: Point<Pixels>, modifiers: Modifiers, cx: &mut Context<Self>) {
        let hovered = if Self::link_modifier(&modifiers) {
            self.cell_at(position).and_then(|(row, col, _)| self.link_at(row, col))
        } else {
            None
        };
        if hovered != self.hovered_link {
            self.hovered_link = hovered;
            cx.notify();
        }
    }

    /// The OSC 8 hyperlink or detected URL/path at a viewport cell.
    fn link_at(&self, row: usize, col: usize) -> Option<HoveredLink> {
        let point = self.core.viewport_point(row, col);
        let cell = &self.core.term().grid()[point];
        if let Some(hyperlink) = cell.hyperlink() {
            let uri = hyperlink.uri().to_string();
            let kind = if is_terminal_url(&uri) { TerminalLinkKind::Url } else { TerminalLinkKind::Path };
            // OSC 8 spans are contiguous cells sharing the same link.
            let grid = self.core.term().grid();
            let same = |c: usize| grid[GridPoint::new(point.line, Column(c))].hyperlink().as_ref() == Some(&hyperlink);
            let cols = self.core.size().cols as usize;
            let start = (0..=col).rev().take_while(|&c| same(c)).last().unwrap_or(col);
            let end = (col..cols).take_while(|&c| same(c)).last().unwrap_or(col);
            return Some(HoveredLink {
                kind,
                text: uri.into(),
                start: GridPoint::new(point.line, Column(start)),
                end: GridPoint::new(point.line, Column(end)),
            });
        }
        let line = self.core.logical_line(point);
        let offset = line.offset_of(point)?;
        let link = terminal_link_at(&line.text, offset)?;
        Some(HoveredLink {
            kind: link.kind,
            text: link.text.into(),
            start: line.point_at(link.start)?,
            end: line.point_at(link.end.saturating_sub(1))?,
        })
    }
}

impl TerminalSurface {
    // Context menu (`terminalContextMenuItems`).

    /// The selection as "Add to chat" sends it, or `None` when nothing usable is selected.
    fn selection_for_chat(&self) -> Option<(SharedString, usize, usize)> {
        let text = self.core.selection_text()?.replace("\r\n", "\n");
        let text = text.trim_matches('\n');
        let (line_start, line_end) = self.core.selection_buffer_lines()?;
        (!text.is_empty()).then(|| (text.to_string().into(), line_start, line_end))
    }

    fn close_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let position = self.context_menu?;
        let has_selection = self.core.has_selection();
        let this = cx.weak_entity();
        let act = move |action: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let this = this.clone();
            move |window: &mut Window, cx: &mut App| {
                this.update(cx, |surface, cx| {
                    surface.context_menu = None;
                    action(surface, window, cx);
                    window.focus(&surface.focus, cx);
                    cx.notify();
                })
                .ok();
            }
        };
        let mut menu = MenuPopup::new().context();
        if self.can_add_to_chat {
            menu = menu.item(MenuItem::new(t!("Add to chat")).disabled(!has_selection).on_select(act(|surface, _, cx| {
                if let Some((text, line_start, line_end)) = surface.selection_for_chat() {
                    cx.emit(TerminalSurfaceEvent::AddToChat { text, line_start, line_end });
                    surface.core.clear_selection();
                }
            })));
        }
        menu = menu
            .item(MenuItem::new(t!("Copy")).disabled(!has_selection).on_select(act(|surface, window, cx| surface.copy(&Copy, window, cx))))
            .item(MenuItem::new(t!("Paste")).disabled(self.read_only).on_select(act(|surface, window, cx| surface.paste(&Paste, window, cx))))
            .item(MenuItem::new(t!("Select all", context = "select all terminal text")).on_select(act(|surface, window, cx| surface.select_all(&SelectAll, window, cx))))
            .item(MenuItem::new(t!("Jump to latest")).on_select(act(|surface, _, _| surface.core.scroll_to_bottom())));
        let dismiss = cx.listener(|this, _: &MouseDownEvent, window, cx| this.close_context_menu(window, cx));
        let escape = cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" {
                this.close_context_menu(window, cx);
            }
        });
        Some(
            deferred(
                anchored().position(position).snap_to_window_with_margin(px(8.)).child(
                    div().occlude().on_mouse_down_out(dismiss).on_key_down(escape).child(menu),
                ),
            )
            .with_priority(5),
        )
    }
}

impl EntityInputHandler for TerminalSurface {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        // An empty selection at the cursor keeps the IME candidate window there.
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.as_ref().map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        self.core.clear_selection();
        self.send_typed(text.as_bytes(), cx);
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let geometry = self.geometry?;
        let cursor = self.core.term().grid().cursor.point;
        let row = cursor.line.0 + self.core.display_offset() as i32;
        let origin = point(
            geometry.origin.x + geometry.cell_width * (cursor.column.0 + range.start) as f32,
            geometry.origin.y + geometry.line_height * row as f32,
        );
        Some(Bounds::new(origin, size(geometry.cell_width, geometry.line_height)))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for TerminalSurface {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_font(cx);
        let background = cx.app_theme().colors.terminal_background;
        let cursor = if self.hovered_link.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam };
        div()
            .id("terminal-surface")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(background)
            .cursor(cursor)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(|this, _: &ScrollPageUp, _, cx| this.scroll_history(cx, |core| core.scroll_page(true))))
            .on_action(cx.listener(|this, _: &ScrollPageDown, _, cx| this.scroll_history(cx, |core| core.scroll_page(false))))
            .on_action(cx.listener(|this, _: &ScrollToTop, _, cx| this.scroll_history(cx, TerminalCore::scroll_to_top)))
            .on_action(cx.listener(|this, _: &ScrollToBottom, _, cx| this.scroll_history(cx, TerminalCore::scroll_to_bottom)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(TerminalGrid::new(cx.entity()))
            .children(self.render_context_menu(cx))
    }
}

/// Returns `Pending` once so other foreground work, frames included, runs between parse slices.
struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, task: &mut TaskContext<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        task.waker().wake_by_ref();
        Poll::Pending
    }
}
