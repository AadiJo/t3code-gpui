//! Window layout: client-drawn titlebar, resizable sidebar, main pane, right
//! panel and bottom terminal slots. Ports `AppSidebarLayout.tsx`,
//! `PanelLayoutControls.tsx` (the fork's gray ghost toggles),
//! `threadSidebarWidth.ts` and `workspaceTitlebar.ts`.
//!
//! The top strip (topbar height) is a window drag area. Every control drawn
//! over it occludes, and so does every overlay, so presses on them never
//! move the window (`05-ui-shell.md` 4).

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::sidebar::style::*;
use super::sidebar::surface_grain::surface_grain;
use super::sidebar::tooltip::tooltip;
use super::sidebar::{Sidebar, SidebarEvent};
use crate::ActiveAppTheme as _;

/// Which titlebar layout the window uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarPlatform {
    /// `hiddenInset`: 52px topbar, traffic lights on the left.
    Mac,
    /// Caption buttons on the right, 40px topbar (Windows, Linux).
    Overlay,
}

impl TitlebarPlatform {
    /// The platform this build runs on.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Overlay
        }
    }

    pub fn topbar_height(self) -> Pixels {
        match self {
            Self::Mac => TOPBAR_HEIGHT_MAC,
            Self::Overlay => TOPBAR_HEIGHT_OVERLAY,
        }
    }

    /// x of the sidebar toggle (`--workspace-controls-left` + `ml-px`).
    pub fn controls_left(self) -> Pixels {
        match self {
            Self::Mac => MAC_CONTROLS_LEFT,
            Self::Overlay => OVERLAY_CONTROLS_LEFT,
        }
    }

    /// Left padding header content needs while the sidebar is collapsed
    /// (`--workspace-titlebar-content-left`).
    pub fn titlebar_content_left(self) -> Pixels {
        self.controls_left() - px(1.) + TITLEBAR_CONTROL_SIZE + px(12.)
    }
}

/// A titlebar toggle the app owns when the layout is controlled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelToggle {
    Terminal,
    RightPanel,
    Maximize,
    Details,
}

/// What the layout tells the app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutEvent {
    /// A titlebar toggle was pressed (controlled layouts only).
    Toggle(PanelToggle),
    /// The sidebar opened or closed, or a resize finished: persist widths.
    Changed,
}

impl EventEmitter<LayoutEvent> for AppSidebarLayout {}

/// A pane edge being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Resize {
    Sidebar,
    RightPanel,
    Terminal,
}

/// The root layout view. Other areas fill the slots with [`AppSidebarLayout::set_main`]
/// and friends; until then each slot shows a labelled placeholder.
pub struct AppSidebarLayout {
    sidebar: Entity<Sidebar>,
    platform: TitlebarPlatform,
    sidebar_open: bool,
    sidebar_width: Pixels,
    /// Below 768px the sidebar is a sheet over the main pane, closed by default.
    sheet_mode: bool,
    sheet_open: bool,
    right_open: bool,
    right_maximized: bool,
    right_width: Pixels,
    terminal_open: bool,
    terminal_height: Pixels,
    details_open: bool,
    resizing: Option<Resize>,
    /// Pressed on the drag strip; the next move starts a window move.
    drag_pending: bool,
    /// Window moves started from the drag strip (checked by scripted tests).
    window_moves: usize,
    /// Count window moves without asking the platform (headless platforms
    /// do not implement window moves).
    count_moves_only: bool,
    /// Draw caption buttons even when the platform would not (scenes).
    force_caption_buttons: bool,
    /// Panel toggles emit [`LayoutEvent::Toggle`] and the app sets the
    /// panels with [`AppSidebarLayout::set_panels`].
    controlled: bool,
    main: Option<AnyView>,
    right_panel: Option<AnyView>,
    terminal: Option<AnyView>,
    _subscriptions: Vec<Subscription>,
}

impl AppSidebarLayout {
    pub fn new(sidebar: Entity<Sidebar>, platform: TitlebarPlatform, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&sidebar, |_, _, _: &SidebarEvent, _| {});
        let this = Self {
            sidebar,
            platform,
            sidebar_open: true,
            sheet_mode: false,
            sheet_open: false,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            right_open: false,
            right_maximized: false,
            right_width: RIGHT_PANEL_DEFAULT_WIDTH,
            terminal_open: false,
            terminal_height: TERMINAL_DEFAULT_HEIGHT,
            details_open: false,
            resizing: None,
            drag_pending: false,
            window_moves: 0,
            count_moves_only: false,
            force_caption_buttons: false,
            controlled: false,
            main: None,
            right_panel: None,
            terminal: None,
            _subscriptions: vec![subscription],
        };
        this.sync_sidebar_chrome(cx);
        this
    }

    fn sync_sidebar_chrome(&self, cx: &mut Context<Self>) {
        let (height, inset) = (
            self.platform.topbar_height(),
            self.platform.titlebar_content_left(),
        );
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_chrome_metrics(height, inset, cx));
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    pub fn platform(&self) -> TitlebarPlatform {
        self.platform
    }

    pub fn set_main(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.main = Some(view);
        cx.notify();
    }

    pub fn set_right_panel(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.right_panel = Some(view);
        cx.notify();
    }

    pub fn set_terminal(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.terminal = Some(view);
        cx.notify();
    }

    /// Hands the panel toggles to the app: presses emit [`LayoutEvent::Toggle`].
    pub fn set_controlled(&mut self, controlled: bool) {
        self.controlled = controlled;
    }

    /// Shows or hides the right panel (maximized or not) and the terminal.
    pub fn set_panels(&mut self, right_open: bool, right_maximized: bool, terminal_open: bool, cx: &mut Context<Self>) {
        let next = (right_open, right_open && right_maximized, terminal_open);
        if (self.right_open, self.right_maximized, self.terminal_open) != next {
            (self.right_open, self.right_maximized, self.terminal_open) = next;
            cx.notify();
        }
    }

    pub fn set_details_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.details_open != open {
            self.details_open = open;
            cx.notify();
        }
    }

    pub fn is_right_panel_maximized(&self) -> bool {
        self.right_maximized
    }

    pub fn right_panel_width(&self) -> Pixels {
        self.right_width
    }

    /// Restores saved widths and the sidebar's open state.
    pub fn restore(&mut self, sidebar_open: bool, sidebar_width: Pixels, right_width: Pixels, cx: &mut Context<Self>) {
        self.sidebar_open = sidebar_open;
        self.sidebar_width = sidebar_width;
        self.right_width = right_width;
        cx.notify();
    }

    /// Width the right panel's tab row leaves for the fixed titlebar toggles.
    pub fn right_panel_trailing_inset(&self, window: &Window) -> Pixels {
        px(112.) + self.caption_width(window)
    }

    /// Leading inset of the right panel's tab row: it clears the floating
    /// sidebar toggle while maximized.
    pub fn right_panel_leading_inset(&self) -> Pixels {
        if self.right_maximized { self.platform.titlebar_content_left() } else { px(0.) }
    }

    fn press(&mut self, toggle: PanelToggle, cx: &mut Context<Self>) {
        if self.controlled {
            cx.emit(LayoutEvent::Toggle(toggle));
            return;
        }
        match toggle {
            PanelToggle::Terminal => self.toggle_terminal(cx),
            PanelToggle::RightPanel => self.toggle_right_panel(cx),
            PanelToggle::Details => self.toggle_details(cx),
            PanelToggle::Maximize => {
                self.right_maximized = !self.right_maximized;
                cx.notify();
            }
        }
    }

    /// Draws the caption buttons regardless of decorations (scenes).
    pub fn force_caption_buttons(&mut self, force: bool) {
        self.force_caption_buttons = force;
    }

    /// Records window moves instead of starting them (headless checks).
    pub fn count_window_moves_only(&mut self, only: bool) {
        self.count_moves_only = only;
    }

    /// How many window moves the drag strip has started.
    pub fn window_move_count(&self) -> usize {
        self.window_moves
    }

    pub fn is_terminal_open(&self) -> bool {
        self.terminal_open
    }

    pub fn is_right_panel_open(&self) -> bool {
        self.right_open
    }

    pub fn is_sidebar_open(&self) -> bool {
        self.sidebar_open
    }

    pub fn sidebar_width(&self) -> Pixels {
        self.sidebar_width
    }

    /// Left padding the main pane's header needs to clear the floating toggle.
    pub fn header_content_left(&self) -> Pixels {
        if self.sidebar_open && !self.sheet_mode {
            px(0.)
        } else {
            self.platform.titlebar_content_left()
        }
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.sheet_mode {
            self.sheet_open = !self.sheet_open;
        } else {
            self.sidebar_open = !self.sidebar_open;
        }
        cx.emit(LayoutEvent::Changed);
        cx.notify();
    }

    pub fn toggle_terminal(&mut self, cx: &mut Context<Self>) {
        self.terminal_open = !self.terminal_open;
        cx.notify();
    }

    pub fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        self.right_open = !self.right_open;
        if !self.right_open {
            self.right_maximized = false;
        }
        cx.notify();
    }

    pub fn toggle_details(&mut self, cx: &mut Context<Self>) {
        self.details_open = !self.details_open;
        cx.notify();
    }

    /// Sets the sidebar width, clamped to `[208, viewport - 640]`.
    pub fn set_sidebar_width(&mut self, width: Pixels, viewport_width: Pixels, cx: &mut Context<Self>) {
        self.sidebar_width = clamp_sidebar_width(width, viewport_width);
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag_pending {
            self.drag_pending = false;
            if event.pressed_button == Some(MouseButton::Left) {
                self.window_moves += 1;
                if !self.count_moves_only {
                    window.start_window_move();
                }
            }
            return;
        }
        let Some(resize) = self.resizing else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.resizing = None;
            return;
        }
        let viewport = window.viewport_size();
        match resize {
            Resize::Sidebar => {
                let next = clamp_sidebar_width(event.position.x, viewport.width);
                // Widening is refused while the main pane would drop below 640px.
                if next > self.sidebar_width && viewport.width - next < SIDEBAR_MAIN_RESERVE {
                    return;
                }
                self.sidebar_width = next;
            }
            Resize::RightPanel => {
                let row_width = viewport.width - self.sidebar_gap();
                self.right_width = clamp_right_width(viewport.width - event.position.x, viewport.width, row_width);
            }
            Resize::Terminal => {
                let max = (viewport.height * 0.75).min(viewport.height - self.platform.topbar_height() - px(120.));
                self.terminal_height = (viewport.height - event.position.y)
                    .max(TERMINAL_MIN_HEIGHT)
                    .min(max.max(TERMINAL_MIN_HEIGHT));
            }
        }
        cx.notify();
    }

    fn sidebar_gap(&self) -> Pixels {
        if self.sidebar_open && !self.sheet_mode {
            self.sidebar_width
        } else {
            px(0.)
        }
    }

    /// One 28x28 ghost titlebar toggle (fork: gray icon, no pressed fill unless `pressed`).
    fn titlebar_toggle(
        &self,
        id: &'static str,
        glyph: IconName,
        label: &'static str,
        pressed: bool,
        colors: &ShellColors,
    ) -> Stateful<Div> {
        let hover = colors.accent;
        div()
            .id(id)
            .occlude()
            .size(TITLEBAR_CONTROL_SIZE)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(CONTROL_RADIUS)
            .cursor_pointer()
            .when(pressed, |d| d.bg(colors.accent))
            .hover(move |s| s.bg(hover))
            .child(Icon::new(glyph).size(IconSize::Custom(px(16.))).color(colors.titlebar_icon))
            .tooltip(tooltip(label))
    }

    fn render_caption_buttons(&self, window: &Window, colors: &ShellColors) -> Option<AnyElement> {
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let show = self.platform == TitlebarPlatform::Overlay
            && (cfg!(target_os = "windows") || client_decorated || self.force_caption_buttons);
        if !show {
            return None;
        }
        let symbol = if colors.dark { rgb(0xf8fafc) } else { rgb(0x1f2937) };
        let maximized = window.is_maximized();
        let button = |id: &'static str, area: WindowControlArea, glyph: component::IconName, close: bool| {
            div()
                .id(id)
                .occlude()
                .w(CAPTION_BUTTON_WIDTH)
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(symbol)
                .hover(move |s| {
                    if close {
                        s.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff))
                    } else {
                        s.bg(hsla(0., 0., 0.5, 0.12))
                    }
                })
                .window_control_area(area)
                .when(!cfg!(target_os = "windows"), |d| {
                    d.on_click(move |_, window, _| match area {
                        WindowControlArea::Min => window.minimize_window(),
                        WindowControlArea::Max => window.zoom_window(),
                        WindowControlArea::Close => window.remove_window(),
                        WindowControlArea::Drag => {}
                    })
                })
                .child(component::Icon::new(glyph).size(px(14.)))
        };
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .h(self.platform.topbar_height())
                .flex()
                .child(button("caption-min", WindowControlArea::Min, component::IconName::WindowMinimize, false))
                .child(button(
                    "caption-max",
                    WindowControlArea::Max,
                    if maximized {
                        component::IconName::WindowRestore
                    } else {
                        component::IconName::WindowMaximize
                    },
                    false,
                ))
                .child(button("caption-close", WindowControlArea::Close, component::IconName::WindowClose, true))
                .into_any_element(),
        )
    }

    fn caption_width(&self, window: &Window) -> Pixels {
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let show = self.platform == TitlebarPlatform::Overlay
            && (cfg!(target_os = "windows") || client_decorated || self.force_caption_buttons);
        if show { CAPTION_BUTTON_WIDTH * 3. } else { px(0.) }
    }
}

/// `clampThreadSidebarWidth`: `[208, max(208, viewport - 640)]`.
pub fn clamp_sidebar_width(width: Pixels, viewport_width: Pixels) -> Pixels {
    let max = SIDEBAR_MIN_WIDTH.max((viewport_width - SIDEBAR_MAIN_RESERVE).floor());
    width.max(SIDEBAR_MIN_WIDTH).min(max)
}

/// Right panel: `[360, max(360, min(0.7 viewport, row - 360))]`.
pub fn clamp_right_width(width: Pixels, viewport_width: Pixels, row_width: Pixels) -> Pixels {
    let max = RIGHT_PANEL_MIN_WIDTH.max((viewport_width * 0.7).floor().min(row_width - RIGHT_PANEL_MIN_WIDTH));
    width.max(RIGHT_PANEL_MIN_WIDTH).min(max)
}

/// A labelled stand-in for a slot another area fills.
fn placeholder(label: &'static str, colors: &ShellColors) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(colors.muted_foreground.opacity(0.5))
        .child(label)
}

impl Render for AppSidebarLayout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ShellColors::from_theme(cx.app_theme());
        let viewport = window.viewport_size();
        let topbar = self.platform.topbar_height();
        // `md` breakpoint (`sidebar.tsx:110`): the sheet starts closed each time.
        let sheet_mode = viewport.width < px(768.);
        if sheet_mode != self.sheet_mode {
            self.sheet_mode = sheet_mode;
            self.sheet_open = false;
        }
        let sidebar_visible = if sheet_mode { self.sheet_open } else { self.sidebar_open };
        self.sidebar_width = clamp_sidebar_width(self.sidebar_width, viewport.width);
        let row_width = viewport.width - self.sidebar_gap();
        self.right_width = clamp_right_width(self.right_width, viewport.width, row_width);
        let sidebar_width = self.sidebar_width;
        let caption_width = self.caption_width(window);

        // Window drag strip, drawn first so everything else sits above it.
        let drag_strip = div()
            .id("titlebar-drag")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(topbar)
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, _| {
                    this.drag_pending = !window.default_prevented();
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag_pending = false))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    if cfg!(target_os = "macos") {
                        window.titlebar_double_click();
                    } else {
                        window.zoom_window();
                    }
                }
            });

        let sidebar = div()
            .relative()
            .h_full()
            .flex_none()
            .w(self.sidebar_gap())
            .when(sidebar_visible && !sheet_mode, |gap| {
                gap.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(sidebar_width)
                        .bg(colors.sidebar)
                        .border_r_1()
                        .border_color(colors.sidebar_edge)
                        .child(surface_grain())
                        .child(self.sidebar.clone()),
                )
                .child(
                    // The rail: 16px straddling the edge, col-resize cursor.
                    div()
                        .id("sidebar-rail")
                        .absolute()
                        .top(topbar)
                        .bottom_0()
                        .right(-SIDEBAR_RAIL_WIDTH / 2.)
                        .w(SIDEBAR_RAIL_WIDTH)
                        .occlude()
                        .cursor(CursorStyle::ResizeColumn)
                        .flex()
                        .justify_center()
                        .group("rail")
                        .child(
                            div()
                                .w(px(2.))
                                .h_full()
                                .group_hover("rail", |s| s.bg(colors.sidebar_border)),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, _| this.resizing = Some(Resize::Sidebar)),
                        )
                        .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                            if event.click_count() == 2 {
                                this.sidebar_width = SIDEBAR_DEFAULT_WIDTH;
                                cx.emit(LayoutEvent::Changed);
                                cx.notify();
                            }
                        })),
                )
            });

        let chat_header = div()
            .flex_none()
            .h(topbar)
            .flex()
            .items_center()
            .px_5()
            .pl(px(20.) + self.header_content_left())
            .border_b_1()
            .border_color(colors.border)
            .text_xs()
            .text_color(colors.muted_foreground.opacity(0.5))
            .child("No active thread");
        let main_body = self.main.clone().map(|v| v.into_any_element()).unwrap_or_else(|| {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(chat_header)
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .px_8()
                        .child(
                            div()
                                .text_xl()
                                .line_height(px(28.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.foreground)
                                .child("Pick a thread to continue"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(colors.muted_foreground)
                                .child("Select a thread in the sidebar or start a new one."),
                        ),
                )
                .into_any_element()
        });

        let terminal_view = self.terminal.clone().map(|view| div().flex_none().w_full().child(view));
        let terminal = (self.terminal.is_none() && self.terminal_open).then(|| {
            div()
                .relative()
                .flex_none()
                .h(self.terminal_height)
                .border_t_1()
                .border_color(colors.border.opacity(0.8))
                .bg(colors.canvas)
                .child(
                    self.terminal
                        .clone()
                        .map(|v| v.into_any_element())
                        .unwrap_or_else(|| placeholder("Terminal", &colors).into_any_element()),
                )
                .child(
                    div()
                        .id("terminal-resize")
                        .absolute()
                        .top(px(-3.))
                        .left_0()
                        .right_0()
                        .h(px(6.))
                        .occlude()
                        .cursor(CursorStyle::ResizeRow)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, _| this.resizing = Some(Resize::Terminal)),
                        ),
                )
        });

        let right_panel = self.right_open.then(|| {
            div()
                .relative()
                .h_full()
                .flex()
                .flex_col()
                .when(self.right_maximized, |d| d.flex_1())
                .when(!self.right_maximized, |d| d.flex_none().w(self.right_width))
                .border_l_1()
                .border_color(colors.border)
                .bg(colors.canvas)
                .map(|d| match self.right_panel.clone() {
                    // The panel view draws its own tab row in the titlebar strip.
                    Some(view) => d.child(div().flex_1().min_h_0().child(view)),
                    None => d
                        .child(
                            // Tab bar row: topbar tall, clears the fixed toggles.
                            div().flex_none().h(topbar).pl_2().pr(px(112.) + caption_width),
                        )
                        .child(div().flex_1().child(placeholder("Right panel", &colors))),
                })
                .child(
                    div()
                        .id("right-panel-resize")
                        .absolute()
                        .top(topbar)
                        .bottom_0()
                        .left(px(-3.))
                        .w(px(6.))
                        .occlude()
                        .cursor(CursorStyle::ResizeColumn)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, _| this.resizing = Some(Resize::RightPanel)),
                        ),
                )
        });

        let main = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .bg(colors.canvas)
            .child(surface_grain())
            .when(!self.right_maximized, |d| {
                d.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(div().flex_1().min_h_0().child(main_body))
                        .children(terminal)
                        .children(terminal_view),
                )
            })
            .children(right_panel);

        let sidebar_toggle = div()
            .absolute()
            .top_0()
            .left(self.platform.controls_left())
            .h(topbar)
            .flex()
            .items_center()
            .child(
                self.titlebar_toggle(
                    "toggle-sidebar",
                    if sidebar_visible {
                        IconName::PanelLeftClose
                    } else {
                        IconName::PanelLeft
                    },
                    "Toggle sidebar",
                    false,
                    &colors,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            );

        let right_controls = div()
            .absolute()
            .top_0()
            .right(CONTROLS_RIGHT + caption_width)
            .h(topbar)
            .flex()
            .items_center()
            .gap(TITLEBAR_CONTROL_GAP)
            .when(self.right_open, |d| {
                d.child(
                    self.titlebar_toggle(
                        "toggle-maximize-panel",
                        if self.right_maximized {
                            IconName::Minimize2
                        } else {
                            IconName::Maximize2
                        },
                        "Maximize panel",
                        self.right_maximized,
                        &colors,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.press(PanelToggle::Maximize, cx))),
                )
            })
            .child(
                self.titlebar_toggle("toggle-details", IconName::SquareMenu, "Thread details", self.details_open, &colors)
                    .on_click(cx.listener(|this, _, _, cx| this.press(PanelToggle::Details, cx))),
            )
            .child(
                self.titlebar_toggle(
                    "toggle-terminal",
                    if self.terminal_open {
                        IconName::PanelBottomClose
                    } else {
                        IconName::PanelBottom
                    },
                    "Toggle terminal",
                    false,
                    &colors,
                )
                .on_click(cx.listener(|this, _, _, cx| this.press(PanelToggle::Terminal, cx))),
            )
            .child(
                self.titlebar_toggle(
                    "toggle-right-panel",
                    if self.right_open {
                        IconName::PanelRightClose
                    } else {
                        IconName::PanelRight
                    },
                    "Toggle right panel",
                    false,
                    &colors,
                )
                .on_click(cx.listener(|this, _, _, cx| this.press(PanelToggle::RightPanel, cx))),
            );

        div()
            .id("app-layout")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(colors.chrome)
            .text_color(colors.foreground)
            .when(self.resizing == Some(Resize::Sidebar) || self.resizing == Some(Resize::RightPanel), |d| {
                d.cursor(CursorStyle::ResizeColumn)
            })
            .when(self.resizing == Some(Resize::Terminal), |d| d.cursor(CursorStyle::ResizeRow))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.drag_pending = false;
                    if this.resizing.take().is_some() {
                        cx.emit(LayoutEvent::Changed);
                        cx.notify();
                    }
                }),
            )
            // Under the panes: panes without handlers let presses through,
            // and their controls occlude it.
            .child(drag_strip)
            .child(div().absolute().top_0().left_0().size_full().flex().child(sidebar).child(main))
            .when(sheet_mode && self.sheet_open, |d| {
                d.child(
                    div()
                        .id("sidebar-sheet-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .bg(colors.canvas.opacity(0.6))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sheet_open = false;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("sidebar-sheet")
                        .occlude()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(viewport.width - px(12.))
                        .bg(colors.sidebar)
                        .border_r_1()
                        .border_color(colors.sidebar_edge)
                        .child(surface_grain())
                        .child(self.sidebar.clone()),
                )
            })
            .child(sidebar_toggle)
            .child(right_controls)
            .children(self.render_caption_buttons(window, &colors))
    }
}
