//! The worktree setup card in the timeline (`WorktreeSetupCard.tsx`): a header for a failed or
//! cancelled setup, one work-log row per stage with its state icon, detail and elapsed time, the
//! setup script's last output lines, the error, and Details / Open terminal / Cancel. Once the
//! agent took over it folds into one summary row (`embedded`).

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, Hsla, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, div,
};

use super::chat_style::{alpha, mono};
use super::messages_timeline::{MessagesTimeline, TimelineAction};
use super::timeline_entries::{SetupPhase, SetupStage, SetupStageId, SetupStageStatus as Status, Timestamp, WorktreeSetup};
use super::timeline_row_parts::shimmer_label;
use crate::ActiveAppTheme as _;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::spinner::{Spinner, SpinnerSize};
use crate::i18n::duration_format::format_duration;
use crate::i18n::t;

const OUTPUT_TAIL_LINES: usize = 4;

/// `Elapsed`: a finished duration, or a ticking one while setup runs.
fn elapsed(started_at: Option<Timestamp>, ended_at: Option<Timestamp>, live: bool, now: i64) -> Option<SharedString> {
    let start = started_at?.0;
    let end = match ended_at {
        Some(end) => end.0,
        None if live => now,
        None => return None,
    };
    Some(format_duration((end - start).max(0) as f64))
}

/// `headerLabel`.
fn header_label(setup: &WorktreeSetup) -> SharedString {
    match setup.phase {
        SetupPhase::Running => t!("Setting up worktree…"),
        SetupPhase::Done if setup.stages.iter().any(|stage| stage.status == Status::Failed) => t!("Worktree ready, setup script failed"),
        SetupPhase::Done => t!("Worktree ready"),
        SetupPhase::Failed => t!("Worktree setup failed"),
        SetupPhase::Cancelled => t!("Worktree setup cancelled"),
    }
}

impl MessagesTimeline {
    /// The setup card. Its Details disclosure is local to the card.
    pub(super) fn render_worktree_setup(&mut self, setup: &WorktreeSetup, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let colors = self.colors;
        let now = self.now_millis;
        let running = setup.phase == SetupPhase::Running;
        let details = window.use_keyed_state("worktree-setup-details", cx, |_, _| false);
        let details_open = *details.read(cx);
        let setup_stage = setup.stages.iter().find(|stage| stage.id == SetupStageId::SetupScript);
        let terminal_id = setup.setup_script.as_ref().map(|script| script.terminal_id.clone());
        let show_terminal = terminal_id.is_some() && setup_stage.is_some_and(|stage| stage.status != Status::Pending);
        let collapsed = setup.embedded && !running;
        let show_header = !setup.embedded && !running && setup.phase != SetupPhase::Done;
        let show_tail = setup_stage.is_some_and(|stage| stage.status == Status::Failed || (stage.status == Status::Running && !stage.tail.is_empty()));
        let text_sm = (theme.rem(0.875), theme.rem(0.875 * 1.625));
        let text_xs = (theme.rem(0.75), theme.rem(1.));
        let muted = colors.theme.muted_foreground;

        // `WorkLogRow`: icon slot, label, trailing parts.
        let work_row = |icon: AnyElement, label: AnyElement, trailing: Vec<AnyElement>| -> Div {
            div().w_full().min_w_0().rounded(theme.radii.md).px(theme.sp(0.5)).py(theme.sp(0.5)).child(
                div()
                    .flex()
                    .items_center()
                    .min_h(theme.sp(6.))
                    .min_w_0()
                    .gap(theme.sp(1.5))
                    .text_size(text_sm.0)
                    .line_height(text_sm.1)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(theme.sp(1.5))
                            .child(div().size(theme.sp(6.)).flex_none().flex().items_center().justify_center().child(icon))
                            .child(div().flex_1().min_w_0().truncate().child(label)),
                    )
                    .children(trailing),
            )
        };
        let stage_icon = |status: Status| -> AnyElement {
            let glyph = match status {
                Status::Running => return Spinner::new(SpinnerSize::Md).into_any_element(),
                Status::Done => IconName::Check,
                Status::Failed => IconName::X,
                Status::Warning => IconName::CircleAlert,
                Status::Skipped => IconName::Minus,
                Status::Pending => IconName::Circle,
            };
            div()
                .when(status == Status::Pending, |d| d.opacity(0.4))
                .child(Icon::new(glyph).size(IconSize::Custom(theme.sp(4.))).color(colors.theme.icon_muted).stroke(2.))
                .into_any_element()
        };
        // `stageRowClassName`: (colour, opacity).
        let stage_tone = |status: Status| -> (Hsla, f32) {
            match status {
                Status::Failed => (colors.theme.error_foreground, 1.),
                Status::Warning => (colors.theme.warning_foreground, 1.),
                Status::Pending => (colors.theme.secondary_label, 0.4),
                _ => (colors.theme.secondary_label, 1.),
            }
        };
        let small = |text: SharedString, max_half: bool| -> AnyElement {
            div()
                .min_w_0()
                .when(max_half, |d| d.max_w(gpui_kit::relative(0.5)).truncate())
                .when(!max_half, |d| d.flex_none())
                .text_size(text_xs.0)
                .line_height(text_xs.1)
                .text_color(muted)
                .child(text)
                .into_any_element()
        };
        let step = self.animation_step;
        let label_of = |text: SharedString, status: Status| -> AnyElement {
            if status == Status::Running {
                return shimmer_label(text, step, colors.theme.secondary_label, colors.theme.text);
            }
            let (color, opacity) = stage_tone(status);
            div().truncate().text_color(color).opacity(opacity).child(text).into_any_element()
        };

        let stage_row = |stage: &SetupStage| -> AnyElement {
            let trailing_text = match stage.status {
                Status::Pending => None,
                Status::Skipped => Some(stage.detail.clone().unwrap_or_else(|| t!("skipped", context = "worktree setup stage status"))),
                Status::Running if stage.id == SetupStageId::Checkout && stage.percent.is_some() => {
                    stage.percent.map(|percent| SharedString::from(format!("{percent}%")))
                }
                _ => stage.detail.clone(),
            };
            let mut trailing: Vec<AnyElement> = trailing_text.filter(|text| !text.is_empty()).map(|text| small(text, true)).into_iter().collect();
            if !matches!(stage.status, Status::Skipped | Status::Pending) {
                // A stage interrupted by the setup ending stops with it.
                trailing.extend(elapsed(stage.started_at, stage.ended_at.or(setup.ended_at), running, now).map(|text| small(text, false)));
            }
            work_row(stage_icon(stage.status), label_of(stage.label.clone(), stage.status), trailing).into_any_element()
        };

        let output_tail = |stage: &SetupStage| -> AnyElement {
            let failed = stage.status == Status::Failed;
            let start = stage.tail.len().saturating_sub(OUTPUT_TAIL_LINES);
            div()
                .mb(theme.sp(1.))
                .ml(theme.sp(8.))
                .overflow_hidden()
                .rounded(theme.radii.md)
                .border_1()
                .px(theme.sp(2.5))
                .py(theme.sp(1.5))
                .font(mono())
                .text_size(theme.rem(0.6875))
                .line_height(theme.rem(0.6875 * 1.625))
                .map(|d| {
                    if failed {
                        d.border_color(alpha(colors.theme.error, 0.2)).bg(colors.theme.error_surface).text_color(colors.theme.error_foreground)
                    } else {
                        d.border_color(colors.theme.border).bg(colors.theme.code_background).text_color(muted)
                    }
                })
                .children((0..OUTPUT_TAIL_LINES).map(|slot| {
                    let line = stage.tail.get(start + slot).filter(|_| start + slot < stage.tail.len()).cloned().unwrap_or_default();
                    // An empty slot keeps its height with a no-break space.
                    div().truncate().whitespace_nowrap().child(if line.is_empty() { SharedString::from("\u{a0}") } else { line })
                }))
                .into_any_element()
        };

        let header = show_header.then(|| {
            let failed = setup.phase == SetupPhase::Failed;
            let tone = if failed { colors.theme.error_foreground } else { muted };
            div()
                .border_b_1()
                .border_color(alpha(colors.theme.border, 0.6))
                .pb(theme.sp(2.))
                .pt(theme.sp(1.))
                .child(
                    div()
                        .flex()
                        .h(theme.sp(6.))
                        .min_w_0()
                        .items_center()
                        .gap(theme.sp(2.))
                        .px(theme.sp(1.))
                        .text_size(text_sm.0)
                        .line_height(text_sm.1)
                        .text_color(tone)
                        .child(div().min_w_0().flex_shrink(1.).truncate().child(header_label(setup)))
                        .children(elapsed(Some(setup.started_at), setup.ended_at, false, now).map(|text| div().ml_auto().child(small(text, false)))),
                )
        });

        let body: AnyElement = if collapsed {
            // `CollapsedSummaryRow`.
            let status = match setup.phase {
                SetupPhase::Failed | SetupPhase::Cancelled => Status::Failed,
                _ if setup.stages.iter().any(|stage| stage.id == SetupStageId::SetupScript && stage.status == Status::Failed) => Status::Failed,
                _ => Status::Done,
            };
            let (color, _) = stage_tone(status);
            let trailing = elapsed(Some(setup.started_at), setup.ended_at, false, now).map(|text| small(text, false)).into_iter().collect();
            work_row(stage_icon(status), div().text_color(color).child(header_label(setup)).into_any_element(), trailing).into_any_element()
        } else {
            div()
                .when(show_header, |d| d.pt(theme.sp(1.5)))
                .children(setup.stages.iter().map(|stage| {
                    div()
                        .child(stage_row(stage))
                        .when(stage.id == SetupStageId::SetupScript && show_tail, |d| d.child(output_tail(stage)))
                }))
                .into_any_element()
        };

        let error = (setup.phase == SetupPhase::Failed).then(|| setup.error.clone()).flatten().map(|error| {
            div().mt(theme.sp(1.)).ml(theme.sp(8.)).text_size(text_xs.0).line_height(text_xs.1).text_color(muted).child(error)
        });

        let toggle = details.clone();
        let buttons = div()
            .mt(theme.sp(0.5))
            // `ml-[calc(--spacing(6)+2px-(--spacing(2)-1px))]`.
            .ml(theme.sp(6.) + gpui_kit::px(2.) - (theme.sp(2.) - gpui_kit::px(1.)))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.sp(0.5))
            .child(
                Button::new("worktree-setup-details")
                    .size(ButtonSize::Xs)
                    .variant(ButtonVariant::GhostMuted)
                    .icon(if details_open { IconName::ChevronDown } else { IconName::ChevronRight })
                    .label(t!("Details"))
                    .on_click(move |_, _, cx| toggle.update(cx, |open, cx| {
                        *open = !*open;
                        cx.notify();
                    })),
            )
            .when_some(terminal_id.filter(|_| show_terminal), |d, terminal_id| {
                let timeline = cx.weak_entity();
                d.child(
                    Button::new("worktree-setup-terminal")
                        .size(ButtonSize::Xs)
                        .variant(ButtonVariant::GhostMuted)
                        .icon(IconName::Terminal)
                        .label(t!("Open terminal"))
                        .on_click(move |_, _, cx| {
                            _ = timeline.update(cx, |_, cx| cx.emit(TimelineAction::OpenTerminal(terminal_id.clone())));
                        }),
                )
            })
            .when(running && !setup.embedded, |d| {
                let timeline = cx.weak_entity();
                d.child(
                    Button::new("worktree-setup-cancel")
                        .size(ButtonSize::Xs)
                        .variant(ButtonVariant::GhostMuted)
                        .icon(IconName::X)
                        .label(t!("Cancel"))
                        .on_click(move |_, _, cx| {
                            _ = timeline.update(cx, |_, cx| cx.emit(TimelineAction::CancelWorktreeSetup));
                        }),
                )
            });

        // `SetupDetails`: a two-column grid, terms sized to the widest.
        let details_list = details_open.then(|| {
            let rows: Vec<(SharedString, SharedString, bool)> = [
                (t!("Branch"), setup.branch.clone(), true),
                (t!("Base", context = "git base branch"), setup.base_ref.clone(), true),
                (t!("Path"), setup.worktree_path.clone(), true),
                (t!("Setup", context = "setup script"), setup.setup_script.as_ref().map(|script| script.command.clone()), false),
            ]
            .into_iter()
            .filter_map(|(label, value, middle)| Some((label, value?, middle)))
            .collect();
            let column = || div().flex().flex_col().gap(theme.sp(0.5));
            div()
                .mt(theme.sp(1.))
                .mb(theme.sp(1.5))
                .ml(theme.sp(8.))
                .flex()
                .gap(theme.sp(3.))
                .text_size(text_xs.0)
                .line_height(text_xs.1)
                .text_color(muted)
                .child(column().flex_none().text_color(alpha(colors.theme.text, 0.8)).children(rows.iter().map(|(label, _, _)| div().child(label.clone()))))
                .child(column().flex_1().min_w_0().font(mono()).children(rows.iter().map(|(_, value, middle)| {
                    if *middle {
                        crate::components::sidebar::sidebar_thread_row::middle_truncate(value).into_any_element()
                    } else {
                        div().min_w_0().truncate().child(value.clone()).into_any_element()
                    }
                })))
        });

        div()
            .children(header)
            .child(body)
            .children(error)
            .child(buttons)
            .children(details_list)
            .into_any_element()
    }
}
