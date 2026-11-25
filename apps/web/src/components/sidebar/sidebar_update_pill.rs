//! The sidebar footer's update control. Ports `SidebarUpdatePill.tsx`
//! (`SidebarUpdateControl`), `SidebarUpdateCapsule.tsx`, `UpdateEverythingDialog.tsx` and the
//! plan half of `useUpdateEverything.ts` for a client with no app updater of its own:
//!
//! - nothing to update: the desktop app's idle control with no update state (disabled,
//!   "Check for updates");
//! - servers behind this client or providers behind their latest release: a round
//!   `CircleArrowUp` button whose tooltip says what it updates; it opens the confirm dialog and
//!   looks for more provider updates meanwhile ("Checking for more…");
//! - while a run goes or after one failed: the progress capsule, with a popover listing each
//!   machine and Retry or Dismiss.
//!
//! [`SidebarUpdatePill`] is one view per app; the sidebar mounts it with [`SidebarUpdatePill::view`].

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use t3code_contracts::OrchestrationV2ShellThreadStatus as ThreadStatus;

use super::update_everything_logic::{
    CapsuleAction, CapsuleTone, ProviderMachine, RestartingMachine, UpdateEverythingMachine, UpdateEverythingView, UpdateRunMachineStatus,
    collect_restarting_machines, collect_server_update_targets, describe_update_capsule, describe_update_everything,
    format_update_elapsed, resolve_update_run_machine_status,
};
use super::update_everything_run::UpdateRunStore;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, mono_font, ui_font, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::provider_updates_action::ProviderUpdates;
use crate::components::server_update_action::{ServerUpdateStore, ServerUpdateTarget};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{Modal, dialog_glass};
use crate::components::ui::popover::{PopoverPadding, PopoverSurface, PopoverWidth};
use crate::components::ui::tooltip::{TooltipAnchor, TooltipSide, TooltipSurface, TooltipVariant};
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::t;
use crate::state::StateBridge;
use crate::theme_color::with_alpha;
use crate::version_skew::client_version;

/// What a run would do now (`useUpdateEverything`'s plan).
#[derive(Clone, Debug)]
pub struct UpdateEverythingPlan {
    pub servers: Vec<ServerUpdateTarget>,
    pub provider_machines: Vec<ProviderMachine>,
    pub restarts: Vec<RestartingMachine>,
    pub view: UpdateEverythingView,
}

impl UpdateEverythingPlan {
    /// The plan over every saved environment, updating servers to this client's version.
    pub fn current(cx: &mut App) -> Self {
        let environments = if ConnectionRuntime::is_running(cx) { ConnectionRuntime::state(cx).read(cx).environments.clone() } else { Vec::new() };
        let machines: Vec<UpdateEverythingMachine> = environments
            .iter()
            .map(|env| UpdateEverythingMachine {
                environment_id: env.id().clone(),
                label: SharedString::from(env.entry.label.clone()),
                config: ServerSettingsStore::config(cx, env.id()),
                connected: env.is_connected(),
                can_maintain: ServerUpdateStore::can_maintain(env.id(), cx),
                update_state: ServerUpdateStore::state(env.id(), cx),
            })
            .collect();
        let (servers, skipped) = collect_server_update_targets(&machines, &client_version(cx));
        let restarts = collect_restarting_machines(&machines, &servers);
        let provider_machines: Vec<ProviderMachine> = ProviderUpdates::machines(cx)
            .into_iter()
            .map(|m| ProviderMachine { providers: m.provider_names(), environment_id: m.environment_id, label: m.label })
            .collect();
        let view = describe_update_everything(&servers, &provider_machines, &skipped);
        Self { servers, provider_machines, restarts, view }
    }

    /// `hasUpdates`.
    pub fn has_updates(&self) -> bool {
        !self.servers.is_empty() || !self.provider_machines.is_empty()
    }
}

/// See the module docs.
pub struct SidebarUpdatePill {
    confirming: bool,
    /// The dialog's quiet provider check is running.
    dialog_checking: bool,
    /// The dialog's "N skipped" list is open.
    skipped_open: bool,
    popover_open: bool,
    hovered: bool,
    /// The control's slot width, for the web's `@min-[12rem]/sidebar-update` query.
    width: f32,
    /// Redraws the capsule while its elapsed times run.
    ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

struct GlobalSidebarUpdatePill(Entity<SidebarUpdatePill>);

impl Global for GlobalSidebarUpdatePill {}

impl SidebarUpdatePill {
    /// The app's control, created on first use.
    pub fn view(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalSidebarUpdatePill>() {
            return global.0.clone();
        }
        let view = cx.new(Self::new);
        cx.set_global(GlobalSidebarUpdatePill(view.clone()));
        view
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let updates = ProviderUpdates::entity(cx);
        let run = UpdateRunStore::entity(cx);
        let mut subscriptions = vec![cx.observe(&updates, |_, _, cx| cx.notify()), cx.observe(&run, |_, _, cx| cx.notify())];
        if StateBridge::is_running(cx) {
            subscriptions.push(cx.observe(&StateBridge::shell(cx), |this, _, cx| {
                if this.confirming {
                    cx.notify();
                }
            }));
        }
        Self {
            confirming: false,
            dialog_checking: false,
            skipped_open: false,
            popover_open: false,
            hovered: false,
            width: 0.,
            ticker: None,
            _subscriptions: subscriptions,
        }
    }

    /// Opens the confirm dialog and looks for more provider updates meanwhile (`handleAction`).
    pub fn open_dialog(&mut self, cx: &mut Context<Self>) {
        self.confirming = true;
        self.skipped_open = false;
        self.dialog_checking = true;
        let check = ProviderUpdates::check(true, cx);
        cx.spawn(async move |this, cx| {
            check.await;
            this.update(cx, |this, cx| {
                this.dialog_checking = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Whether the confirm dialog is open.
    pub fn is_confirming(&self) -> bool {
        self.confirming
    }

    /// Opens or closes the capsule's popover (checks and scenes).
    pub fn set_popover_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.popover_open = open;
        cx.notify();
    }

    /// Confirms the dialog (`runEverything`).
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        let plan = UpdateEverythingPlan::current(cx);
        UpdateRunStore::start(plan.servers, plan.provider_machines, cx).detach();
        cx.notify();
    }

    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        cx.notify();
    }

    /// Keeps a 30-second redraw going while a machine's elapsed time shows.
    fn sync_ticker(&mut self, active: bool, cx: &mut Context<Self>) {
        if !active {
            self.ticker = None;
            return;
        }
        if self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(30)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                }
            }));
        }
    }

    /// The round 30px button (`size-8`) around a 15px icon.
    fn round_button(&self, id: &'static str, icon: IconName, emphasized: bool, disabled: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let hovered = self.hovered && !disabled;
        let color = if emphasized || hovered { c.sidebar_foreground } else { super::style::ShellColors::from_theme(theme).sidebar_icon };
        div()
            .id(id)
            .flex_none()
            .size(theme.rem(2.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .when(emphasized, |d| d.bg(c.sidebar_control_surface))
            .when(hovered, |d| d.bg(c.sidebar_row_hover))
            .when(disabled && !emphasized, |d| d.opacity(0.6))
            .when(!disabled, |d| d.cursor_pointer())
            .when(disabled, |d| d.cursor_not_allowed())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
            .child(Icon::new(icon).size(IconSize::Custom(theme.rem(1.))).color(color))
    }

    fn render_capsule(&mut self, run: &super::update_everything_logic::UpdateEverythingRun, cx: &mut Context<Self>) -> AnyElement {
        let now = crate::timestamp_format::now_ms().max(0) as u64;
        let views: Vec<(&super::update_everything_logic::UpdateRunMachine, UpdateRunMachineStatus)> = run
            .machines
            .iter()
            .map(|m| (m, resolve_update_run_machine_status(m, run.phase, &ServerUpdateStore::state(&m.environment_id, cx))))
            .collect();
        let has_active = views.iter().any(|(_, s)| s.is_working());
        self.sync_ticker(has_active, cx);
        let view = describe_update_capsule(&views, now);
        let failed = views.iter().any(|(_, s)| *s == UpdateRunMachineStatus::Failed);
        let settled = !has_active;
        let theme = cx.app_theme();
        let c = &theme.colors;
        let wide = self.width >= f32::from(theme.rem(12.));
        let ring = match view.tone {
            CapsuleTone::Default => None,
            CapsuleTone::Slow => Some(with_alpha(c.warning, 0.4)),
            CapsuleTone::Failed => Some(with_alpha(c.destructive(), 0.4)),
        };
        let aria: SharedString = match &view.slow {
            Some((status, elapsed)) => format!("{} {status} {elapsed}", view.label).into(),
            None => view.label.clone(),
        };
        let run_action = move |action: CapsuleAction, cx: &mut App| match action {
            CapsuleAction::Retry => UpdateRunStore::retry_failed(cx),
            CapsuleAction::Dismiss => UpdateRunStore::dismiss_settled(cx),
        };
        let row_hover = c.sidebar_row_hover;
        let capsule = div()
            .relative()
            .ml_auto()
            .flex()
            .flex_none()
            .h(theme.rem(2.))
            .when(wide, |d| d.w_full())
            .when(!wide, |d| d.w(theme.rem(2.)))
            .items_center()
            .rounded_full()
            .bg(c.sidebar_control_surface)
            .text_color(c.sidebar_foreground)
            .when_some(ring, |d, ring| d.border_1().border_color(ring))
            .child(
                div()
                    .id("sidebar-update-capsule")
                    .aria_label(aria)
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(move |s| s.bg(row_hover))
                    .on_click(cx.listener(|this, _, _, cx| this.set_popover_open(!this.popover_open, cx)))
                    .test_support(),
            )
            .when(wide, |d| {
                d.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .pl(theme.sp(3.))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .child(div().when(view.tone == CapsuleTone::Failed, |d| d.text_color(c.destructive())).child(view.label.clone()))
                        .children(view.slow.as_ref().map(|(_, elapsed)| div().text_color(c.warning).child(format!(" {elapsed}")))),
                )
                .children(view.action.map(|action| {
                    div()
                        .id("sidebar-update-capsule-action")
                        .relative()
                        .flex_none()
                        .cursor_pointer()
                        .pr(theme.sp(3.))
                        .pl(theme.sp(1.5))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .underline()
                        .child(action.label())
                        .on_click(move |_, _, cx| run_action(action, cx))
                }))
            })
            .when(!(wide && view.action.is_some()), |d| {
                d.child(
                    div()
                        .relative()
                        .flex_none()
                        .size(theme.rem(2.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::CircleArrowUp).size(IconSize::Custom(theme.rem(1.))).color(c.sidebar_foreground)),
                )
            });
        let popover = self.popover_open.then(|| {
            let rows = views.iter().map(|(machine, status)| {
                let working = status.is_working();
                let status_text: SharedString = if working {
                    format!("{} {}", status.label(), format_update_elapsed(now.saturating_sub(machine.started_at))).into()
                } else {
                    status.label()
                };
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(theme.sp(3.))
                            .child(div().min_w_0().truncate().child(machine.label.clone()))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(if *status == UpdateRunMachineStatus::Failed { c.destructive() } else { c.muted_foreground })
                                    .child(status_text),
                            ),
                    )
                    .children(machine.failure.clone().map(|failure| div().text_color(c.muted_foreground).child(failure)))
            });
            let actions = view.action.map(|action| {
                div()
                    .mt(theme.sp(1.))
                    .flex()
                    .justify_end()
                    .gap(theme.sp(1.))
                    .child(
                        Button::new("sidebar-update-popover-action")
                            .size(ButtonSize::Xs)
                            .variant(ButtonVariant::Outline)
                            .label(action.label())
                            .on_click(move |_, _, cx| run_action(action, cx)),
                    )
                    .when(settled && failed && action != CapsuleAction::Dismiss, |d| {
                        d.child(
                            Button::new("sidebar-update-popover-dismiss")
                                .size(ButtonSize::Xs)
                                .variant(ButtonVariant::Ghost)
                                .label(t!("Dismiss"))
                                .on_click(|_, _, cx| UpdateRunStore::dismiss_settled(cx)),
                        )
                    })
            });
            let surface = PopoverSurface::new().width(PopoverWidth::Sm).padding(PopoverPadding::Compact).child(
                div().flex().flex_col().gap(theme.sp(1.)).ui_text(theme, TextSize::Xs, weight::NORMAL).children(rows).children(actions),
            );
            let close = cx.entity().downgrade();
            div().absolute().top_0().right_0().child(
                deferred(
                    anchored().anchor(Anchor::BottomRight).offset(point(px(0.), -theme.sp(1.))).snap_to_window_with_margin(px(8.)).child(
                        div()
                            .id("sidebar-update-popover")
                            .occlude()
                            .on_mouse_down_out(move |_, _, cx| {
                                close.update(cx, |this, cx| this.set_popover_open(false, cx)).ok();
                            })
                            .child(surface)
                            .test_support(),
                    ),
                )
                .with_priority(1),
            )
        });
        div().relative().flex().flex_1().min_w(theme.rem(2.)).child(capsule).children(popover).into_any_element()
    }

    fn render_dialog(&self, plan: &UpdateEverythingPlan, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let view = &plan.view;
        let updating: Vec<_> = view.rows.iter().filter(|r| r.app.is_some() || !r.providers.is_empty()).collect();
        let skipped: Vec<_> = view.rows.iter().filter(|r| r.skipped.is_some()).collect();
        let running_threads = |continues: bool, cx: &App| -> usize {
            plan.restarts.iter().filter(|m| m.continues_threads == continues).map(|m| running_thread_count(&m.environment_id, cx)).sum()
        };
        let (stopped, continuing) = (running_threads(false, cx), running_threads(true, cx));
        let running = stopped + continuing;

        // Column widths of the grid `[minmax(4rem,max-content)_max-content_minmax(min-content,1fr)_max-content]`.
        let sm = TextSize::Sm.size(theme.fonts.interface);
        let xs = TextSize::Xs.size(theme.fonts.interface);
        let header_machine = t!("Machine");
        let header_providers = t!("Providers");
        let label_max = f32::from(theme.rem(8.));
        let mut col1 = text_width(window, &header_machine, xs).max(f32::from(theme.rem(4.)));
        let mut col2 = text_width(window, "T3 Code", xs);
        for row in &updating {
            col1 = col1.max(text_width(window, &row.label, sm).min(label_max));
            if let Some(app) = row.app {
                col2 = col2.max(text_width(window, &app.label(), sm));
            } else if row.skipped.is_some() {
                col2 = col2.max(text_width(window, &t!("Skipped"), sm));
            }
        }
        let gap = f32::from(sp(4.));
        let cell = |w: f32| div().flex_none().w(px(w.ceil())).whitespace_nowrap();
        let header = div()
            .flex()
            .gap(px(gap))
            .pb(sp(1.5))
            .border_b_1()
            .border_color(c.border)
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(c.muted_foreground)
            .child(cell(col1).child(header_machine))
            .child(cell(col2).child("T3 Code"))
            .child(div().flex_1().child(header_providers));
        let count = updating.len();
        let rows = updating.iter().enumerate().map(|(ix, row)| {
            let threads = plan.restarts.iter().any(|m| m.environment_id == row.environment_id).then(|| running_thread_count(&row.environment_id, cx));
            div()
                .flex()
                .gap(px(gap))
                .py(sp(1.5))
                .when(ix + 1 < count, |d| d.border_b_1().border_color(with_alpha(c.border, 0.5)))
                .child(cell(col1).min_w_0().truncate().child(row.label.clone()))
                .child(
                    cell(col2)
                        .flex()
                        .flex_col()
                        .children(row.app.map(|a| a.label()))
                        .when(row.app.is_none() && row.skipped.is_some(), |d| d.child(div().text_color(c.muted_foreground).child(t!("Skipped")))),
                )
                .child(div().flex_1().min_w_0().flex().flex_wrap().children(row.providers.iter().enumerate().map(|(i, p)| {
                    let comma = if i + 1 < row.providers.len() { ", " } else { "" };
                    div().whitespace_nowrap().child(format!("{p}{comma}"))
                })))
                .children(threads.filter(|n| *n > 0).map(|n| {
                    div().flex_none().whitespace_nowrap().text_color(c.warning).child(t!("{count, plural, one {{count} running} other {{count} running}}", count = n))
                }))
        });
        let skipped_block = (!skipped.is_empty()).then(|| {
            let open = self.skipped_open;
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("update-everything-skipped")
                        .flex()
                        .items_center()
                        .gap(sp(1.))
                        .cursor_pointer()
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .hover(|s| s.text_color(theme.colors.foreground()))
                        .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Custom(theme.rem(0.875))))
                        .child(t!("{count, plural, one {{count} skipped} other {{count} skipped}}", count = skipped.len()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.skipped_open = !this.skipped_open;
                            cx.notify();
                        })),
                )
                .when(open, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(0.5))
                            .pt(sp(1.5))
                            .pl(sp(4.5))
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .children(skipped.iter().map(|row| {
                                div()
                                    .flex()
                                    .gap(sp(3.))
                                    .child(div().flex_none().max_w(theme.rem(8.)).truncate().child(row.label.clone()))
                                    .child(div().flex_1().min_w_0().child(row.skipped.clone().unwrap_or_default()))
                            })),
                    )
                })
        });
        let mut description: Vec<AnyElement> = Vec::new();
        if let Some(version) = &view.version {
            description.push(div().child("T3 Code ").into_any_element());
            // Same line box as the text around it, so the smaller mono text sits on its line.
            description.push(div().font(mono_font(weight::NORMAL)).text_size(px(xs)).line_height(px(TextSize::Sm.line_height(theme.fonts.interface))).child(version.clone()).into_any_element());
        }
        if view.version.is_some() && view.provider_count > 0 {
            description.push(div().child(" · ").into_any_element());
        }
        if view.provider_count > 0 {
            description.push(
                div()
                    .child(t!(
                        "{count, plural, one {{count} provider update} other {{count} provider updates}}",
                        count = view.provider_count
                    ))
                    .into_any_element(),
            );
        }
        if self.dialog_checking {
            let lead = if view.version.is_some() || view.provider_count > 0 { " · " } else { "" };
            description.push(div().child(format!("{lead}{}", t!("Checking for more…"))).into_any_element());
        }
        let note: Option<SharedString> = (running > 0).then(|| {
            let interrupted = t!(
                "{count, plural, one {{count} running thread will be interrupted.} other {{count} running threads will be interrupted.}}",
                count = running
            );
            if stopped == 0 {
                t!(
                    "{count, plural, one {{count} running thread will be interrupted and can resume after the restart.} other {{count} running threads will be interrupted and can resume after the restart.}}",
                    count = running
                )
            } else if continuing == 0 {
                interrupted
            } else {
                format!(
                    "{interrupted} {}",
                    t!("{count, plural, one {{count} can resume after the restart.} other {{count} can resume after the restart.}}", count = continuing)
                )
                .into()
            }
        });
        let card = dialog_glass(theme)
            .id("update-everything-dialog")
            .w_full()
            .max_w(theme.rem(36.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.))
                    .p(sp(6.))
                    .child(
                        div()
                            .ui_text(theme, TextSize::Xl, weight::SEMIBOLD)
                            .line_height(px(TextSize::Xl.size(theme.fonts.interface) * 1.375))
                            .child(t!("{count, plural, one {Update {count} machine?} other {Update {count} machines?}}", count = count)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .ui_text(theme, TextSize::Sm, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .children(description),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(3.))
                    .px(sp(6.))
                    .pb(sp(4.))
                    .child(div().flex().flex_col().ui_text(theme, TextSize::Sm, weight::NORMAL).child(header).children(rows))
                    .children(skipped_block),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(sp(2.))
                    .px(sp(6.))
                    .py(sp(4.))
                    .border_t_1()
                    .border_color(c.border)
                    .bg(theme.derived.dialog_footer)
                    .rounded_b(theme.radii.xxl - px(1.))
                    .children(note.map(|note| {
                        div().mr_auto().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(note)
                    }))
                    .child(
                        Button::new("update-everything-cancel")
                            .variant(ButtonVariant::Outline)
                            .label(t!("Cancel"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .child(
                        Button::new("update-everything-confirm")
                            .label(t!("Update", context = "verb, confirms the update"))
                            .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                    ),
            );
        let close = cx.entity().downgrade();
        Modal::dialog("update-everything-modal", card, move |_, cx| {
            close.update(cx, |this, cx| this.close_dialog(cx)).ok();
        })
        .into_any_element()
    }
}

/// Threads preparing, starting or running on the environment (`runningThreadCountAtom`).
fn running_thread_count(environment_id: &t3code_client_runtime::environment::EnvironmentId, cx: &App) -> usize {
    if !StateBridge::is_running(cx) {
        return 0;
    }
    let Some(env) = StateBridge::environments(cx).read(cx).get(environment_id, cx) else { return 0 };
    let shell = env.read(cx).shell.clone();
    shell.snapshot.as_ref().map_or(0, |snapshot| {
        snapshot.threads.iter().filter(|t| matches!(t.status, ThreadStatus::Preparing | ThreadStatus::Starting | ThreadStatus::Running)).count()
    })
}

/// The width of `text` in the interface font at `size` px.
fn text_width(window: &mut Window, text: &str, size: f32) -> f32 {
    let run = TextRun { len: text.len(), font: ui_font(weight::NORMAL), color: gpui_kit::white(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(SharedString::from(text.to_owned()), px(size), &[run], None).width)
}

impl Render for SidebarUpdatePill {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let run = UpdateRunStore::run(cx);
        let plan = UpdateEverythingPlan::current(cx);
        if run.is_none() {
            self.ticker = None;
            self.popover_open = false;
        }
        let entity = cx.entity();
        let measure = canvas(
            move |bounds, _, cx| {
                let width = f32::from(bounds.size.width);
                entity.update(cx, |this, cx| {
                    if (this.width - width).abs() > 0.5 {
                        this.width = width;
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let dialog = self.confirming.then(|| self.render_dialog(&plan, window, cx));
        let control = if let Some(run) = &run {
            self.render_capsule(run, cx)
        } else if plan.has_updates() {
            let anchor = TooltipAnchor::new();
            let summary = plan.view.summary.clone();
            self.round_button("sidebar-update-everything", IconName::CircleArrowUp, true, false, cx)
                .aria_label(summary.clone())
                .relative()
                .child(anchor.measure())
                .tooltip(anchor.tooltip(TooltipSurface::new(summary).variant(TooltipVariant::Glass), TooltipSide::Top))
                .on_click(cx.listener(|this, _, _, cx| this.open_dialog(cx)))
                .test_support()
                .into_any_element()
        } else {
            let anchor = TooltipAnchor::new();
            self.round_button("sidebar-check-updates", IconName::RefreshCw, false, true, cx)
                .aria_label(t!("Check for updates"))
                .relative()
                .child(anchor.measure())
                .tooltip(anchor.tooltip(t!("Check for updates"), TooltipSide::Top))
                .test_support()
                .into_any_element()
        };
        let capsule = run.is_some();
        div()
            .relative()
            .ml_auto()
            .flex()
            .justify_end()
            .when(capsule, |d| d.flex_1().min_w(cx.app_theme().rem(2.)))
            .when(!capsule, |d| d.flex_none())
            .child(measure)
            .child(control)
            .children(dialog)
    }
}
