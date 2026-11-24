//! The thread details card's data sections. Ports `ThreadServersPanel.tsx`
//! (dev servers the thread started), `ThreadAutomationsPanel.tsx` (scheduled
//! tasks bound to the thread) and `ThreadDetailsUsage.tsx` (the provider's limit
//! windows and the context window bar). Each renders from typed
//! rows its owner supplies and reports what a click asks for as
//! [`ThreadDetailsSectionEvent`]; an empty input renders nothing.

use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, TestSupportExt as _, Window, div,
    prelude::*, px, relative,
};

use super::thread_details_panel_styles::{row_base, row_hover, row_icon, row_icon_size, row_inset, row_text};
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::spinner::{Spinner, SpinnerSize};
use crate::components::ui::switch::Switch;
use crate::components::ui::tooltip::tooltip;
use crate::i18n::{t, t_rich};
use crate::theme_color::with_alpha;

/// `SERVER_PAGE_COUNT`: servers shown before "Show N more".
pub const SERVER_PAGE_COUNT: usize = 4;

/// A dev server the thread's agent or a terminal started.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadServerRow {
    pub port: u16,
    pub process_name: Option<SharedString>,
    /// Started from a terminal tab rather than by the agent.
    pub from_terminal: bool,
    /// Forwarded to this machine on that local port.
    pub forwarded_to: Option<u16>,
    /// The session may stop it (needs a pid and the server's capability).
    pub can_stop: bool,
    /// The environment is remote, so the port can be forwarded here.
    pub can_forward: bool,
}

/// `ScheduledTask.lastRunStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationRunStatus {
    Never,
    Running,
    Succeeded,
    Failed,
}

/// A scheduled task bound to the thread.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadAutomationRow {
    pub id: SharedString,
    pub title: SharedString,
    /// `taskScheduleLine`: "Every day at 9:00 AM · next in 3h".
    pub schedule_line: SharedString,
    pub last_run: AutomationRunStatus,
    pub enabled: bool,
    /// A webhook task runs from its URL only, so it has no Run now.
    pub webhook: bool,
}

/// The context window snapshot (`ContextWindowSnapshot`).
#[derive(Clone, Debug, PartialEq)]
pub struct ContextWindowRow {
    pub used_tokens: u64,
    pub max_tokens: Option<u64>,
    pub used_percentage: Option<f32>,
    pub auto_compact_threshold: Option<u64>,
}

/// What a section asks its owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadDetailsSectionEvent {
    /// Open the server in the browser panel; `external` for a cmd/ctrl or middle click.
    OpenServer { port: u16, external: bool },
    StopServer(u16),
    /// Start or stop forwarding the port to this machine.
    ToggleForward { port: u16, forwarded: bool },
    /// Settings > Scheduled tasks, on a task when given.
    ManageAutomations(Option<SharedString>),
    RunAutomation(SharedString),
    SetAutomationEnabled { id: SharedString, enabled: bool },
    /// A usage limit row: open the usage page.
    OpenUsage,
}

/// The selected provider's limits (`ThreadDetailsUsage`).
#[derive(Clone, Debug, PartialEq)]
pub enum UsageLimitsInput {
    /// `limitsNotice`: why there are no windows to show.
    Notice(SharedString),
    Windows(Vec<UsageWindowRow>),
}

/// One limit window (`LimitRow`).
#[derive(Clone, Debug, PartialEq)]
pub struct UsageWindowRow {
    pub id: SharedString,
    pub label: SharedString,
    /// Room left, 0 to 100.
    pub left: u8,
    /// The share of the window's time left, 0 to 100 (the pace tick).
    pub tick: Option<f32>,
    /// The tooltip's muted lines: pace, then the reset.
    pub lines: Vec<SharedString>,
}

/// Emits a section event from a click.
pub type SectionEmit = Rc<dyn Fn(ThreadDetailsSectionEvent, &mut Window, &mut App)>;

/// `ThreadDetailsSection` with its heading shown: `px-2 pt-2 pb-2.5`, a top
/// rule, and a 2rem heading row in muted `text-2xs` with optional actions.
pub fn headed_section(
    theme: &AppTheme,
    title: SharedString,
    actions: Option<AnyElement>,
    children: Vec<AnyElement>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .px(theme.rem(0.5))
        .pt(theme.rem(0.5))
        .pb(theme.rem(0.625))
        .border_t_1()
        .border_color(with_alpha(theme.colors.border, 0.65))
        .child(
            div()
                .flex()
                .min_h(theme.rem(2.))
                .min_w_0()
                .mb(theme.rem(0.25))
                .px(row_inset(theme))
                .items_center()
                .justify_between()
                .gap(theme.rem(0.5))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .ui_text(theme, TextSize::Xs2, weight::MEDIUM)
                        .text_color(theme.colors.muted_foreground)
                        .child(title),
                )
                .children(actions.map(|a| div().flex().flex_none().items_center().gap(theme.rem(0.25)).child(a))),
        )
        .children(children)
        .into_any_element()
}

/// A 1.5rem ghost icon control (`part="icon"`, `size="icon-xs"`).
fn icon_control(id: impl Into<gpui_kit::ElementId>, theme: &AppTheme, icon: IconName, tone: Option<Hsla>, disabled: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    row_base(id, theme, disabled, false)
        .flex_none()
        .size(theme.rem(1.5))
        .px(px(0.))
        .justify_center()
        .rounded(theme.radii.md)
        .child(Icon::new(icon).size(IconSize::Custom(theme.rem(0.875))).color(tone.unwrap_or(theme.colors.muted_foreground)))
}

/// `ThreadServersPanel`: each server's `localhost:PORT` over who started it;
/// forward and stop show on hover (a forward stays, it holds a local port).
pub fn servers_section(
    theme: &AppTheme,
    servers: &[ThreadServerRow],
    visible: usize,
    busy_port: Option<u16>,
    on_page: impl Fn(usize, &mut Window, &mut App) + 'static,
    emit: SectionEmit,
) -> Option<AnyElement> {
    if servers.is_empty() {
        return None;
    }
    let external = if cfg!(target_os = "macos") { "⌘-click" } else { "Ctrl-click" };
    let hidden = servers.len().saturating_sub(visible);
    let rows = servers.iter().take(visible).enumerate().map(|(index, server)| {
        let port = server.port;
        let forwarded = server.forwarded_to.is_some();
        let actions = usize::from(server.can_stop) + usize::from(server.can_forward);
        let subline: SharedString = [
            server.forwarded_to.map(|p| t!("Forwarded to :{port}", context = "a remote dev server reachable on this machine's port", port = p)),
            Some(if server.from_terminal {
                t!("Terminal", context = "who started a dev server")
            } else {
                t!("Agent", context = "who started a dev server")
            }),
            server.process_name.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
        .into();
        let group: SharedString = format!("server-{index}").into();
        let open = emit.clone();
        let main = row_base(("server-row", index), theme, false, false)
            .w_full()
            .h_auto()
            .min_h(theme.rem(2.))
            .py(theme.rem(0.375))
            .tooltip(tooltip(SharedString::from(t_rich!("Open in the browser panel. {key} for your browser.", key = external).plain())))
            .on_click(move |event, window, cx| {
                let external = event.modifiers().platform || event.modifiers().control;
                open(ThreadDetailsSectionEvent::OpenServer { port, external }, window, cx)
            })
            .child(row_icon(theme, IconName::Globe))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(theme.rem(0.125))
                    .when(actions == 1, |d| d.pr(theme.rem(1.5)))
                    .when(actions == 2, |d| d.pr(theme.rem(3.)))
                    .child(
                        div()
                            .truncate()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .line_height(theme.rem(1.))
                            .text_color(with_alpha(theme.colors.foreground(), 0.85))
                            .child(format!("localhost:{port}")),
                    )
                    .child(
                        div()
                            .truncate()
                            .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                            .line_height(theme.rem(1.))
                            .text_color(theme.colors.muted_foreground)
                            .child(subline),
                    ),
            );
        let forward = server.can_forward.then(|| {
            let toggle = emit.clone();
            let tip: SharedString = if forwarded {
                t_rich!("Forwarded to localhost:{port} on this machine. Click to stop.", port = server.forwarded_to.unwrap_or_default()).plain().into()
            } else {
                t!("Forward to this machine, for any app here")
            };
            let control = icon_control(("server-forward", index), theme, IconName::Cable, forwarded.then(|| theme.colors.primary()), busy_port.is_some())
                .tooltip(tooltip(tip))
                .on_click(move |_, window, cx| toggle(ThreadDetailsSectionEvent::ToggleForward { port, forwarded }, window, cx));
            if busy_port == Some(port) {
                control.child(div().absolute().child(Spinner::new(SpinnerSize::Xs))).into_any_element()
            } else {
                control.into_any_element()
            }
        });
        let stop = server.can_stop.then(|| {
            let stop = emit.clone();
            // `SquareIcon size-3 fill-current`: lucide's square is a 9px rounded rect in 12px.
            row_base(("server-stop", index), theme, busy_port.is_some(), false)
                .flex_none()
                .size(theme.rem(1.5))
                .px(px(0.))
                .justify_center()
                .rounded(theme.radii.md)
                .child(div().size(theme.rem(0.75) * 0.75).rounded(px(1.5)).bg(theme.colors.destructive()))
                .tooltip(tooltip(t!("Stop server")))
                .on_click(move |_, window, cx| stop(ThreadDetailsSectionEvent::StopServer(port), window, cx))
                .into_any_element()
        });
        div()
            .group(group.clone())
            .relative()
            .flex()
            .items_center()
            .rounded(theme.radii.lg)
            .child(main)
            .when(actions > 0, |row| {
                row.child(
                    div()
                        .absolute()
                        .right(theme.rem(0.25))
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .gap(theme.rem(0.125))
                        .when(!forwarded, |d| d.invisible().group_hover(group.clone(), |d| d.visible()))
                        .children(forward)
                        .children(stop),
                )
            })
            .into_any_element()
    });
    let mut children: Vec<AnyElement> = rows.collect();
    if servers.len() > SERVER_PAGE_COUNT {
        let next = if hidden > 0 { visible + SERVER_PAGE_COUNT } else { SERVER_PAGE_COUNT };
        children.push(
            row_base("servers-more", theme, false, false)
                .w_full()
                .text_color(theme.colors.muted_foreground)
                .child(Icon::new(if hidden > 0 { IconName::ChevronRight } else { IconName::ChevronDown }).size(row_icon_size(theme)))
                .child(if hidden > 0 {
                    SharedString::from(t_rich!("Show {count} more", count = hidden.min(SERVER_PAGE_COUNT)).plain())
                } else {
                    t!("Show less")
                })
                .on_click(move |_, window, cx| on_page(next, window, cx))
                .into_any_element(),
        );
    }
    Some(headed_section(theme, t!("Servers", context = "thread details section: running dev servers"), None, children))
}

/// `ThreadAutomationsPanel`: each bound task with a status dot on its
/// calendar icon, its schedule, Edit, Run now and an enabled switch.
pub fn automations_section(
    theme: &AppTheme,
    tasks: &[ThreadAutomationRow],
    error: Option<SharedString>,
    can_operate: bool,
    busy: Option<&str>,
    emit: SectionEmit,
) -> Option<AnyElement> {
    if tasks.is_empty() && error.is_none() {
        return None;
    }
    let manage = {
        let emit = emit.clone();
        icon_control("automations-manage", theme, IconName::Settings2, None, false)
            .tooltip(tooltip(t!("Manage scheduled tasks")))
            .on_click(move |_, window, cx| emit(ThreadDetailsSectionEvent::ManageAutomations(None), window, cx))
            .into_any_element()
    };
    let rows = tasks.iter().enumerate().map(|(index, task)| {
        let dot = match task.last_run {
            AutomationRunStatus::Never => with_alpha(theme.colors.muted_foreground, 0.4),
            AutomationRunStatus::Running => gpui_kit::rgb(0x0ea5e9).into(),
            AutomationRunStatus::Succeeded => gpui_kit::rgb(0x10b981).into(),
            AutomationRunStatus::Failed => theme.colors.destructive(),
        };
        let edit = {
            let emit = emit.clone();
            let id = task.id.clone();
            icon_control(("automation-edit", index), theme, IconName::Pencil, None, false)
                .tooltip(tooltip(t!("Edit automation")))
                .on_click(move |_, window, cx| emit(ThreadDetailsSectionEvent::ManageAutomations(Some(id.clone())), window, cx))
        };
        let run = (!task.webhook).then(|| {
            let emit = emit.clone();
            let id = task.id.clone();
            let disabled = !can_operate || busy.is_some() || task.last_run == AutomationRunStatus::Running;
            icon_control(("automation-run", index), theme, IconName::Play, None, disabled)
                .tooltip(tooltip(t!("Run now")))
                .when(!disabled, |c| c.on_click(move |_, window, cx| emit(ThreadDetailsSectionEvent::RunAutomation(id.clone()), window, cx)))
        });
        let toggle = {
            let emit = emit.clone();
            let id = task.id.clone();
            Switch::new(("automation-enabled", index), task.enabled)
                .disabled(!can_operate || busy.is_some())
                .on_change(move |enabled, window, cx| {
                    emit(ThreadDetailsSectionEvent::SetAutomationEnabled { id: id.clone(), enabled }, window, cx)
                })
        };
        let detail = format!("{}\n{}", task.title, task.schedule_line);
        div()
            .flex()
            .items_center()
            .gap(row_inset(theme))
            .px(row_inset(theme))
            .py(theme.rem(0.375))
            .rounded(theme.radii.lg)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_none()
                    .size(theme.rem(1.))
                    .items_center()
                    .justify_center()
                    .child(row_icon(theme, IconName::CalendarClock))
                    .child(div().absolute().right(theme.rem(-0.25)).top(theme.rem(-0.25)).size(theme.rem(0.375)).rounded_full().bg(dot)),
            )
            .child(
                div()
                    .id(("automation-title", index))
                    .flex_1()
                    .min_w_0()
                    .tooltip(tooltip(SharedString::from(detail)))
                    .child(
                        div()
                            .truncate()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(with_alpha(theme.colors.foreground(), 0.8))
                            .child(task.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                            .text_color(theme.colors.muted_foreground)
                            .child(task.schedule_line.clone()),
                    ),
            )
            .child(edit)
            .children(run)
            .child(toggle)
            .into_any_element()
    });
    let mut children = vec![div().id("automations-list").max_h(theme.rem(13.5)).overflow_y_scroll().children(rows).into_any_element()];
    if let Some(error) = error {
        children.push(
            div()
                .px(row_inset(theme))
                .py(theme.rem(0.375))
                .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                .text_color(theme.colors.destructive())
                .child(t_rich!("Could not load automations: {error}", error = error.as_ref()).plain())
                .into_any_element(),
        );
    }
    Some(headed_section(theme, t!("Automations"), Some(manage), children))
}

/// `formatContextWindowTokens`: 950, 1.2k, 45k, 1.2m.
pub fn format_context_window_tokens(value: u64) -> String {
    let trim = |s: String| s.strip_suffix(".0").map(str::to_owned).unwrap_or(s);
    match value {
        v if v < 1_000 => v.to_string(),
        v if v < 10_000 => format!("{}k", trim(format!("{:.1}", v as f64 / 1_000.))),
        v if v < 999_500 => format!("{}k", (v as f64 / 1_000.).round()),
        v => format!("{}m", trim(format!("{:.1}", v as f64 / 1_000_000.))),
    }
}

/// The 3px hairline under a usage row: `fill` from 0 to 100 (at least 6px
/// when above 0) and a 9px tick at `tick` percent.
fn usage_bar(theme: &AppTheme, fill: f32, tick: Option<f32>, color: Hsla) -> gpui_kit::Div {
    let track = if theme.is_dark() { with_alpha(theme.colors.muted_foreground, 0.2) } else { with_alpha(theme.colors.muted_foreground, 0.3) };
    div()
        .relative()
        .w_full()
        .h(px(3.))
        .rounded_full()
        .bg(track)
        .when(fill > 0., |b| b.child(div().absolute().top_0().bottom_0().left_0().w(relative(fill / 100.)).min_w(px(6.)).rounded_full().bg(color)))
        .children(tick.map(|t| {
            div().absolute().top(px(-3.)).left(relative((t / 100.).clamp(0., 1.))).h(px(9.)).w(px(1.)).bg(with_alpha(theme.colors.foreground(), 0.75))
        }))
}

/// `ThreadDetailsUsage`: the selected provider's limit windows, as room left,
/// each opening the usage page; or one "Limits · Unavailable" row with the
/// reason. Labels, bars and values share columns (`grid-cols-[auto_minmax(3rem,1fr)_auto]`):
/// the rows are hover and click targets laid behind three columns.
pub fn usage_limits_section(theme: &AppTheme, input: &UsageLimitsInput, emit: SectionEmit) -> AnyElement {
    struct Row {
        label: SharedString,
        bar: Option<(f32, Option<f32>, Hsla)>,
        value: SharedString,
        value_color: Hsla,
        tip: crate::components::ui::tooltip::TooltipSurface,
    }
    use crate::components::ui::tooltip::TooltipSurface;
    let rows: Vec<Row> = match input {
        UsageLimitsInput::Notice(notice) => vec![Row {
            label: t!("Limits", context = "usage limits row label"),
            bar: None,
            value: t!("Unavailable"),
            value_color: theme.colors.muted_foreground,
            tip: TooltipSurface::new(notice.clone()).line(t!("Open usage")),
        }],
        UsageLimitsInput::Windows(windows) => windows
            .iter()
            .map(|window| {
                let left = window.left;
                let (fill, value) = match left {
                    l if l <= 10 => (theme.colors.destructive(), theme.colors.destructive()),
                    l if l <= 20 => (theme.colors.warning, theme.colors.warning),
                    _ => (with_alpha(theme.colors.foreground(), 0.8), with_alpha(theme.colors.foreground(), 0.8)),
                };
                let mut tip = TooltipSurface::new(t!("{label}: {left}% left", label = window.label.as_ref(), left = left as i64));
                for line in &window.lines {
                    tip = tip.line(line.clone());
                }
                Row {
                    label: window.label.clone(),
                    bar: Some((left as f32, window.tick, fill)),
                    value: format!("{left}%").into(),
                    value_color: value,
                    tip: tip.line(t!("Open usage")),
                }
            })
            .collect(),
    };
    if rows.is_empty() {
        return div().into_any_element();
    }
    let row_h = theme.rem(1.5);
    let hover = row_hover(theme);
    let pressed = if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.12) } else { gpui_kit::hsla(0., 0., 0., 0.06) };
    let targets = rows.iter().enumerate().map(|(ix, row)| {
        let emit = emit.clone();
        div()
            .id(("usage-limit-row", ix))
            .absolute()
            .left_0()
            .right_0()
            .top(row_h * ix as f32)
            .h(row_h)
            .rounded(theme.radii.lg)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .tooltip(tooltip(row.tip.clone()))
            .on_click(move |_, window, cx| emit(ThreadDetailsSectionEvent::OpenUsage, window, cx))
    });
    let cell = || div().h(row_h).flex().items_center();
    let labels = div().flex().flex_col().flex_none().children(rows.iter().map(|row| {
        cell().child(div().max_w(theme.rem(7.)).truncate().text_color(theme.colors.muted_foreground).child(row.label.clone()))
    }));
    let any_bar = rows.iter().any(|row| row.bar.is_some());
    let bars = any_bar.then(|| {
        div().flex().flex_col().flex_1().min_w(theme.rem(3.)).children(
            rows.iter().map(|row| cell().children(row.bar.map(|(fill, tick, color)| usage_bar(theme, fill, tick, color)))),
        )
    });
    let values = div().flex().flex_col().flex_none().ml_auto().items_end().children(rows.iter().map(|row| {
        cell().font_weight(weight::MEDIUM).text_color(row.value_color).child(row.value.clone())
    }));
    let grid = div()
        .relative()
        .flex()
        .gap(theme.rem(0.75))
        .px(row_inset(theme))
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .children(targets)
        .child(labels)
        .children(bars)
        .child(values);
    div()
        .flex()
        .flex_col()
        .px(theme.rem(0.5))
        .pt(theme.rem(0.5))
        .pb(theme.rem(0.625))
        .border_t_1()
        .border_color(with_alpha(theme.colors.border, 0.65))
        .child(grid)
        .into_any_element()
}

/// `toLocaleString` for an English locale: 12,345.
fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, ch) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `ThreadDetailsContext`: one "Context" row with a 3px bar (warning at 80%,
/// error at 90%), a tick at the auto-compact threshold and `used / max`.
pub fn context_section(theme: &AppTheme, usage: &ContextWindowRow) -> AnyElement {
    let max = usage.max_tokens.filter(|m| *m > 0);
    let used = max.and(usage.used_percentage).map(|p| p.clamp(0., 100.).round());
    let tick = max
        .zip(usage.auto_compact_threshold)
        .filter(|(m, t)| *t > 0 && t < m)
        .map(|(m, t)| t as f32 / m as f32);
    let (fill_color, value_color) = match used {
        Some(u) if u >= 90. => (theme.colors.destructive(), theme.colors.destructive()),
        Some(u) if u >= 80. => (theme.colors.warning, theme.colors.warning),
        _ => (theme.colors.muted_foreground, with_alpha(theme.colors.foreground(), 0.8)),
    };
    let used_tokens = format_context_window_tokens(usage.used_tokens);
    let value: SharedString = match max {
        Some(m) => format!("{used_tokens} / {}", format_context_window_tokens(m)).into(),
        None => used_tokens.clone().into(),
    };
    let tip: SharedString = match used {
        Some(u) => t!("Context: {used}% used", used = u as i64),
        None => t!("Context: {usedTokens} tokens used", usedTokens = used_tokens.as_str()),
    };
    let mut tip = crate::components::ui::tooltip::TooltipSurface::new(tip);
    if let Some(m) = max {
        tip = tip.line(t!("{used} of {max} tokens", used = group_thousands(usage.used_tokens).as_str(), max = group_thousands(m).as_str()));
    }
    let hover = row_hover(theme);
    let track = if theme.is_dark() { with_alpha(theme.colors.muted_foreground, 0.2) } else { with_alpha(theme.colors.muted_foreground, 0.3) };
    let bar = used.map(|u| {
        div()
            .relative()
            .flex_1()
            .min_w(theme.rem(3.))
            .h(px(3.))
            .rounded_full()
            .bg(track)
            .when(u > 0., |b| {
                b.child(div().absolute().top_0().bottom_0().left_0().w(relative(u / 100.)).min_w(px(6.)).rounded_full().bg(fill_color))
            })
            .children(tick.map(|t| {
                div().absolute().top(px(-3.)).left(relative(t)).h(px(9.)).w(px(1.)).bg(with_alpha(theme.colors.foreground(), 0.75))
            }))
    });
    let row = div()
        .id("context-window-row")
        .flex()
        .items_center()
        .gap(theme.rem(0.75))
        .h(theme.rem(1.5))
        .px(row_inset(theme))
        .rounded(theme.radii.lg)
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .hover(move |s| s.bg(hover))
        .tooltip(tooltip(tip))
        .child(
            div()
                .max_w(theme.rem(7.))
                .truncate()
                .text_color(theme.colors.muted_foreground)
                .child(t!("Context", context = "context window row label")),
        )
        .children(bar)
        .child(div().flex_none().ml_auto().font_weight(weight::MEDIUM).text_color(value_color).child(value))
        .test_support();
    let _ = row_text;
    div()
        .flex()
        .flex_col()
        .px(theme.rem(0.5))
        .pt(theme.rem(0.5))
        .pb(theme.rem(0.625))
        .border_t_1()
        .border_color(with_alpha(theme.colors.border, 0.65))
        .child(row)
        .into_any_element()
}
