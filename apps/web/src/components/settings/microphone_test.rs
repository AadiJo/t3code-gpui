//! Test microphone (`MicrophoneTest.tsx`): records ten seconds and plays them back on this
//! device. No environment, transcriber or provider is involved, so a silent result points at
//! the microphone. While recording it shows the level as a waveform and the elapsed time.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task, Window, div};

use super::settings_layout::SettingsRow;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;
use crate::speech::pcm_capture::{CaptureEvent, CaptureFailure, CaptureOptions, OpenError, PcmCapture};
use crate::speech::voice_input_session::WAVEFORM_BARS;
use crate::theme_color::with_alpha;

/// `LOOPBACK_SECONDS`.
const LOOPBACK: Duration = Duration::from_secs(10);

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TestPhase {
    Idle,
    Starting,
    Recording,
    Ready,
}

/// A finished recording at the device rate.
struct Recorded {
    duration: f64,
}

/// Plays a recording on its own thread; the position is shared.
struct Player {
    commands: mpsc::Sender<PlayerCommand>,
    position_ms: Arc<AtomicU64>,
    playing: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}

enum PlayerCommand {
    Play,
    Pause,
    Seek(Duration),
}

impl Player {
    fn start(samples: Arc<[f32]>, rate: u32) -> Self {
        let (commands, receiver) = mpsc::channel();
        let (position_ms, playing, failed) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (position, is_playing, has_failed) = (position_ms.clone(), playing.clone(), failed.clone());
        std::thread::spawn(move || {
            let Ok(mut stream) = rodio::OutputStreamBuilder::open_default_stream() else {
                has_failed.store(true, Ordering::SeqCst);
                return;
            };
            stream.log_on_drop(false);
            let load = || {
                let sink = rodio::Sink::connect_new(stream.mixer());
                sink.pause();
                sink.append(rodio::buffer::SamplesBuffer::new(1, rate.max(1), samples.to_vec()));
                sink
            };
            let mut sink = load();
            loop {
                match receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok(PlayerCommand::Play) => {
                        if sink.empty() {
                            sink = load();
                        }
                        sink.play();
                    }
                    Ok(PlayerCommand::Pause) => sink.pause(),
                    Ok(PlayerCommand::Seek(to)) => {
                        if sink.empty() {
                            sink = load();
                        }
                        let _ = sink.try_seek(to);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                let ended = sink.empty();
                is_playing.store(!ended && !sink.is_paused(), Ordering::SeqCst);
                if !ended {
                    position.store(sink.get_pos().as_millis() as u64, Ordering::SeqCst);
                }
            }
        });
        Self { commands, position_ms, playing, failed }
    }
}

/// The Test microphone row.
pub struct MicrophoneTest {
    /// The device to record from; `None` is System default.
    pub device: Option<String>,
    phase: TestPhase,
    error: Option<SharedString>,
    levels: [f32; WAVEFORM_BARS],
    started: Option<Instant>,
    capture: Option<PcmCapture>,
    recorded: Option<Recorded>,
    player: Option<Player>,
    _task: Option<Task<()>>,
    _ticker: Option<Task<()>>,
}

impl MicrophoneTest {
    pub fn new(device: Option<String>) -> Self {
        Self {
            device,
            phase: TestPhase::Idle,
            error: None,
            levels: [0.; WAVEFORM_BARS],
            started: None,
            capture: None,
            recorded: None,
            player: None,
            _task: None,
            _ticker: None,
        }
    }

    pub fn phase(&self) -> TestPhase {
        self.phase
    }

    /// The level the meter shows now (`meter value`).
    pub fn level(&self) -> f32 {
        self.levels[WAVEFORM_BARS - 1]
    }

    /// Seconds recorded, once a recording is ready.
    pub fn recorded_seconds(&self) -> Option<f64> {
        self.recorded.as_ref().map(|r| r.duration)
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// `releaseCapture` and `discard`.
    fn release(&mut self) {
        self.capture = None;
        self._task = None;
        self._ticker = None;
        self.levels = [0.; WAVEFORM_BARS];
    }

    fn discard(&mut self) {
        self.recorded = None;
        self.player = None;
        self.phase = TestPhase::Idle;
    }

    /// Record (and Record again).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.release();
        self.discard();
        self.error = None;
        self.phase = TestPhase::Starting;
        cx.notify();
        let device = self.device.clone();
        let opening = cx.background_executor().spawn(async move { PcmCapture::open(CaptureOptions { device, meter_only: true }) });
        self._task = Some(cx.spawn(async move |this, cx| {
            let opened = opening.await;
            let mut events = match opened {
                Err(error) => {
                    _ = this.update(cx, |this, cx| {
                        this.phase = TestPhase::Idle;
                        this.error = Some(match error {
                            OpenError::PermissionDenied => t!(
                                "Microphone access is required for voice input. Allow access in your browser or system settings and try again."
                            ),
                            OpenError::NotFound => t!("No microphone found."),
                            OpenError::Failed(_) => t!("Could not access the microphone."),
                        });
                        cx.notify();
                    });
                    return;
                }
                Ok((capture, events)) => {
                    let started = this.update(cx, |this, cx| {
                        capture.start();
                        this.capture = Some(capture);
                        this.started = Some(Instant::now());
                        this.phase = TestPhase::Recording;
                        this.start_ticker(cx);
                        cx.notify();
                    });
                    if started.is_err() {
                        return;
                    }
                    events
                }
            };
            let deadline = cx.background_executor().timer(LOOPBACK);
            let mut deadline = std::pin::pin!(deadline);
            loop {
                let next = events.next();
                futures::pin_mut!(next);
                match futures::future::select(next, deadline.as_mut()).await {
                    futures::future::Either::Right(_) => {
                        _ = this.update(cx, |this, _| this.capture.as_ref().map(PcmCapture::stop));
                        // Keep reading until Stopped arrives.
                        while let Some(event) = events.next().await {
                            if this.update(cx, |this, cx| this.on_event(event, cx)).unwrap_or(true) {
                                return;
                            }
                        }
                        return;
                    }
                    futures::future::Either::Left((Some(event), _)) => {
                        if this.update(cx, |this, cx| this.on_event(event, cx)).unwrap_or(true) {
                            return;
                        }
                    }
                    futures::future::Either::Left((None, _)) => return,
                }
            }
        }));
    }

    /// Redraws the elapsed time and playback position four times a second.
    fn start_ticker(&mut self, cx: &mut Context<Self>) {
        self._ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        }));
    }

    /// Handles a capture event; true once the capture is over.
    fn on_event(&mut self, event: CaptureEvent, cx: &mut Context<Self>) -> bool {
        match event {
            CaptureEvent::Level(level) => {
                self.levels.copy_within(1.., 0);
                self.levels[WAVEFORM_BARS - 1] = level;
                cx.notify();
                false
            }
            CaptureEvent::Recorded { samples, rate } => {
                let duration = self.started.map_or(0., |at| at.elapsed().as_secs_f64()).min(LOOPBACK.as_secs_f64());
                self.release();
                if samples.is_empty() {
                    self.error = Some(t!("No audio was recorded. Try again."));
                    self.phase = TestPhase::Idle;
                } else {
                    let samples: Arc<[f32]> = samples.into();
                    self.player = Some(Player::start(samples, rate));
                    self.recorded = Some(Recorded { duration });
                    self.phase = TestPhase::Ready;
                    self.start_ticker(cx);
                }
                cx.notify();
                false
            }
            CaptureEvent::Stopped => true,
            CaptureEvent::Failed(failure) => {
                if failure == CaptureFailure::Disconnected {
                    self.error = Some(t!("The microphone disconnected."));
                } else {
                    self.error = Some(t!("Could not record microphone audio. Try again."));
                }
                self.capture.as_ref().map(PcmCapture::stop);
                cx.notify();
                false
            }
            CaptureEvent::Pcm(_) | CaptureEvent::Limit => false,
        }
    }

    /// Stop: ends the recording early and keeps what was recorded.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(capture) = &self.capture {
            capture.stop();
        }
        cx.notify();
    }

    /// The X: cancels the test or discards the recording.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.release();
        self.discard();
        self.error = None;
        cx.notify();
    }

    /// Play or pause the recording.
    pub fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        let Some(player) = &self.player else { return };
        let command = if player.playing.load(Ordering::SeqCst) { PlayerCommand::Pause } else { PlayerCommand::Play };
        let _ = player.commands.send(command);
        cx.notify();
    }

    fn render_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let close = Button::new("microphone-test-close")
            .variant(ButtonVariant::GhostMuted)
            .size(ButtonSize::IconSm)
            .icon(IconName::X)
            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)));
        let content: AnyElement = if let Some(error) = &self.error {
            div().min_w_0().flex_1().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.error).child(error.clone()).into_any_element()
        } else {
            match self.phase {
                TestPhase::Starting => div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(c.muted_foreground)
                    .child(t!("Opening microphone…"))
                    .into_any_element(),
                TestPhase::Recording => {
                    let elapsed = self.started.map_or(0., |at| at.elapsed().as_secs_f64()).min(LOOPBACK.as_secs_f64());
                    let full = theme.rem(1.);
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(theme.sp(3.))
                        .child(div().flex_none().size(theme.sp(1.5)).rounded_full().bg(c.error))
                        .child(
                            div().flex().w(theme.rem(6.)).min_w_0().child(
                                div()
                                    .flex()
                                    .h(full)
                                    .min_w_0()
                                    .flex_1()
                                    .items_center()
                                    .justify_end()
                                    .gap(theme.sp(0.5))
                                    .overflow_hidden()
                                    .children(self.levels.iter().map(|&level| {
                                        div()
                                            .flex_none()
                                            .w(theme.sp(0.5))
                                            .h(full * level.max(0.2))
                                            .rounded_full()
                                            .bg(c.foreground())
                                            .opacity(0.3 + level * 0.55)
                                    })),
                            ),
                        )
                        .child(
                            div()
                                .ml_auto()
                                .flex_none()
                                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                .text_color(c.muted_foreground)
                                .child(format!("{} / {}", format_time(elapsed), format_time(LOOPBACK.as_secs_f64()))),
                        )
                        .into_any_element()
                }
                TestPhase::Ready => {
                    let recorded = self.recorded.as_ref()?;
                    let player = self.player.as_ref()?;
                    let playing = player.playing.load(Ordering::SeqCst);
                    let position = (player.position_ms.load(Ordering::SeqCst) as f64 / 1000.).min(recorded.duration);
                    let progress = if recorded.duration > 0. { (position / recorded.duration).min(1.) as f32 } else { 0. };
                    let failed = player.failed.load(Ordering::SeqCst);
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(theme.sp(3.))
                        .child(
                            Button::new("microphone-test-play")
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::IconSmPill)
                                .icon(if playing { IconName::Pause } else { IconName::Play })
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_playback(cx))),
                        )
                        .child(if failed {
                            div()
                                .min_w_0()
                                .flex_1()
                                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                .text_color(c.error)
                                .child(t!("Could not play this recording. Try recording again."))
                                .into_any_element()
                        } else {
                            // The range input: a 4px track filled to the position, a 10px thumb.
                            div()
                                .relative()
                                .flex_1()
                                .min_w_0()
                                .h(theme.sp(1.))
                                .rounded_full()
                                .bg(c.muted)
                                .child(div().absolute().left_0().top_0().bottom_0().w(gpui_kit::relative(progress)).rounded_full().bg(c.primary()))
                                .into_any_element()
                        })
                        .child(
                            div()
                                .flex_none()
                                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                .text_color(c.muted_foreground)
                                .child(format!("{} / {}", format_time(position), format_time(recorded.duration))),
                        )
                        .into_any_element()
                }
                TestPhase::Idle => return None,
            }
        };
        let ready = self.phase == TestPhase::Ready && self.error.is_none();
        Some(
            div()
                .mt(theme.sp(3.))
                .mb(theme.sp(2.))
                .flex()
                .min_h(theme.rem(2.5))
                .items_center()
                .gap(theme.sp(3.))
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(with_alpha(c.border, 0.6))
                .bg(with_alpha(c.muted, 0.3))
                .py(theme.sp(1.))
                .pr(theme.sp(1.))
                .pl(if ready { theme.sp(1.) } else { theme.sp(3.) })
                .child(content)
                .child(close)
                .into_any_element(),
        )
    }

    /// The settings row with its control and panel.
    pub fn row(&self, cx: &mut Context<Self>) -> SettingsRow {
        let control = if matches!(self.phase, TestPhase::Recording | TestPhase::Starting) {
            Button::new("voice-test-stop")
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .icon(IconName::Square)
                .label(t!("Stop"))
                .disabled(self.phase != TestPhase::Recording)
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            let ready = self.phase == TestPhase::Ready;
            Button::new("voice-test-record")
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .icon(if ready { IconName::RotateCcw } else { IconName::Mic })
                .label(if ready { t!("Record again") } else { t!("Record") })
                .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
        };
        let mut row = SettingsRow::new(t!("Test microphone"))
            .description(t!("Record ten seconds and play it back. The audio stays on this device."))
            .control(control);
        if let Some(panel) = self.render_panel(cx) {
            row = row.child(panel);
        }
        row
    }
}

impl Render for MicrophoneTest {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.row(cx)
    }
}
