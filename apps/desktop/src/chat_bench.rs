// Scripted screenshots and performance measurements of the chat timeline,
// headless. Usage:
//   chat-bench shots <out-dir>   pages of the fixture thread, dark and light, plus expand/collapse
//   chat-bench perf <out-dir>    first paint, scroll and streaming frame times, resident memory
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, anyhow};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, IntoElement, Render, WindowBounds,
    WindowOptions, component::TitleBar, px, size, test::TestWindowExt as _,
};
use t3code_web::{
    components::{
        chat::{
            messages_timeline::MessagesTimeline,
            messages_timeline_logic::RowKind,
            timeline_entries::TimelineInput,
            timeline_fixtures::{FIXTURE_NOW, fixture_thread, perf_thread, streaming_reply, streaming_thread},
            timeline_viewport::TimelineFollow,
        },
        chat_view::{ChatView, chat_view},
    },
    theme_palette::{Appearance, ThemeDocument, set_app_theme},
    AppTheme,
};

struct Host(Entity<ChatView>);

impl Render for Host {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

struct Session {
    cx: HeadlessAppContext,
    handle: AnyWindowHandle,
    view: Entity<ChatView>,
}

impl Session {
    fn open(appearance: Appearance, width: f32, height: f32) -> anyhow::Result<Self> {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(t3code_web::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(t3code_web::init);
        cx.update(|cx| set_app_theme(AppTheme::new(&ThemeDocument::graphite(), appearance), cx));
        let (handle, view) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)),
                show: false,
                focus: false,
                ..TitleBar::window_options()
            };
            let view = chat_view(cx);
            let host_view = view.clone();
            let handle = gpui_kit::open_window(options, cx, move |_, cx| cx.new(|_| Host(host_view)))?;
            anyhow::Ok((handle.0, view))
        })?;
        Ok(Self { cx, handle, view })
    }

    fn timeline(&mut self) -> Entity<MessagesTimeline> {
        self.cx.update(|cx| self.view.read(cx).timeline().clone())
    }

    fn frame(&mut self) -> anyhow::Result<()> {
        self.cx.update_window(self.handle, |_, window, cx| window.render_frame(cx))?;
        Ok(())
    }

    /// Two frames: the composer inset measured in the first lands in the second.
    fn settle(&mut self) -> anyhow::Result<()> {
        self.frame()?;
        self.cx.run_until_parked();
        self.frame()?;
        self.frame()
    }

    fn shot(&mut self, path: &Path) -> anyhow::Result<()> {
        self.settle()?;
        let image = self.cx.capture_screenshot(self.handle)?;
        image.save(path)?;
        println!("{}", path.display());
        Ok(())
    }

    /// Clicks inside the rendered row whose key is `row_key`, `dx` from its left.
    fn click_row(&mut self, row_key: u64, dx: f32) -> anyhow::Result<()> {
        let timeline = self.timeline();
        let point = self
            .cx
            .update(|cx| {
                let t = timeline.read(cx);
                let index = t.rows().iter().position(|r| r.key == row_key)?;
                let viewport = t.viewport();
                viewport.row_bounds.iter().find(|(i, _)| *i == index).map(|(_, b)| b.origin + gpui_kit::point(lane_left(b.size.width) + px(dx), px(12.)))
            })
            .ok_or_else(|| anyhow!("row {row_key} not on screen"))?;
        self.click_point(point)
    }

    fn click_point(&mut self, position: gpui_kit::Point<gpui_kit::Pixels>) -> anyhow::Result<()> {
        use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
        let events = [
            PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() }),
            PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }),
            PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: 1 }),
        ];
        for event in events {
            self.cx.update_window(self.handle, |_, window, cx| {
                window.dispatch_event(event, cx);
                window.render_frame(cx);
            })?;
        }
        self.settle()
    }

    /// Clicks a work entry by key, inside a nested group list or as its own row.
    fn click_entry(&mut self, entry_key: u64) -> anyhow::Result<()> {
        let timeline = self.timeline();
        let in_rows = self.cx.update(|cx| timeline.read(cx).rows().iter().any(|r| matches!(&r.kind, RowKind::Work { entry, .. } if MessagesTimeline::key_of(entry.id.as_str()) == entry_key)));
        if in_rows {
            let row = self.cx.update(|cx| {
                timeline.read(cx).rows().iter().find(|r| matches!(&r.kind, RowKind::Work { entry, .. } if MessagesTimeline::key_of(entry.id.as_str()) == entry_key)).map(|r| r.key)
            });
            return self.click_row(row.unwrap(), 40.);
        }
        // Inside an expanded group: the nested list's rows sit under the details row.
        let (details_key, offset) = self
            .cx
            .update(|cx| {
                timeline.read(cx).rows().iter().find_map(|r| match &r.kind {
                    RowKind::WorkGroupDetails { entries, .. } => entries
                        .iter()
                        .position(|e| MessagesTimeline::key_of(e.id.as_str()) == entry_key)
                        .map(|i| (r.key, i)),
                    _ => None,
                })
            })
            .ok_or_else(|| anyhow!("entry not found"))?;
        let timeline2 = timeline.clone();
        let origin = self
            .cx
            .update(|cx| {
                let t = timeline2.read(cx);
                let index = t.rows().iter().position(|r| r.key == details_key)?;
                t.viewport().row_bounds.iter().find(|(i, _)| *i == index).map(|(_, b)| b.origin)
            })
            .ok_or_else(|| anyhow!("group not on screen"))?;
        let width = self.cx.update(|cx| timeline2.read(cx).viewport().bounds.size.width);
        self.click_point(origin + gpui_kit::point(lane_left(width) + px(28. + 40.), px(12. + 24. * offset as f32)))
    }

    fn set_thread(&mut self, key: &str, input: TimelineInput) {
        let view = self.view.clone();
        self.cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.timeline().update(cx, |t, _| t.set_now_millis(FIXTURE_NOW));
                view.set_thread(key.to_owned().into(), "t3code".into(), "Sync engine audit".into(), input, cx)
            })
        });
    }

    /// Scrolls so the row with `key` sits near the top, then returns its index.
    fn reveal(&mut self, find: impl Fn(&RowKind) -> bool) -> anyhow::Result<u64> {
        let timeline = self.timeline();
        let (index, key) = self.cx.update(|cx| {
            let t = timeline.read(cx);
            t.rows().iter().enumerate().find(|(_, r)| find(&r.kind)).map(|(i, r)| (i, r.key))
        })
        .ok_or_else(|| anyhow!("row not found"))?;
        self.cx.update(|cx| timeline.read(cx).list_state().scroll_to_row(index, px(80.)));
        self.settle()?;
        Ok(key)
    }
}

/// Left edge of the 736px content lane inside a row of `width`.
fn lane_left(width: gpui_kit::Pixels) -> gpui_kit::Pixels {
    let lane = (width - px(96.)).min(px(736.));
    (width - lane) / 2.
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(|l| l.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0)))
        .unwrap_or(0)
}

fn shots(out: &Path) -> anyhow::Result<()> {
    for (name, appearance) in [("dark", Appearance::Dark), ("light", Appearance::Light)] {
        let mut s = Session::open(appearance, 1100., 780.)?;
        s.set_thread("fixture", fixture_thread());
        s.settle()?;
        let timeline = s.timeline();
        // Page through from the top.
        s.cx.update(|cx| timeline.read(cx).list_state().scroll_to_row(0, px(0.)));
        for page in 0..40 {
            s.shot(&out.join(format!("{name}-page-{page:02}.png")))?;
            let viewport = s.cx.update(|cx| timeline.read(cx).viewport());
            if viewport.distance_from_end() <= px(1.) {
                break;
            }
            let composer = s.cx.update(|cx| timeline.read(cx).composer_bottom());
            let step = viewport.bounds.size.height - composer - px(80.);
            s.cx.update(|cx| {
                timeline.read(cx).list_state().scroll_by(step, t3code_web::components::chat::timeline_viewport::TimelineNavigation::Command { releases_follow: true })
            });
        }
        if name != "dark" {
            continue;
        }
        // Expand and collapse: the turn fold, then a tool group inside it, then a command entry.
        let fold = s.reveal(|k| matches!(k, RowKind::TurnFold { .. }))?;
        s.shot(&out.join("dark-fold-collapsed.png"))?;
        s.click_row(fold, 4.)?;
        s.shot(&out.join("dark-fold-expanded.png"))?;
        let toggle = s.reveal(|k| matches!(k, RowKind::WorkToggle { .. }))?;
        s.click_row(toggle, 30.)?;
        s.shot(&out.join("dark-group-expanded.png"))?;
        let command_entry = s.cx.update(|cx| {
            timeline.read(cx).rows().iter().find_map(|r| match &r.kind {
                RowKind::WorkGroupDetails { entries, .. } => entries.iter().find(|e| matches!(e.item, t3code_web::components::chat::timeline_entries::WorkItem::Command { .. })).map(|e| MessagesTimeline::key_of(e.id.as_str())),
                _ => None,
            })
        });
        if let Some(entry) = command_entry {
            s.click_entry(entry)?;
            s.shot(&out.join("dark-entry-expanded.png"))?;
            s.click_entry(entry)?;
        }
        let toggle = s.reveal(|k| matches!(k, RowKind::WorkToggle { expanded: true, .. }))?;
        s.click_row(toggle, 30.)?;
        let fold = s.reveal(|k| matches!(k, RowKind::TurnFold { .. }))?;
        s.click_row(fold, 4.)?;
        s.shot(&out.join("dark-fold-recollapsed.png"))?;
        // A standalone todo-list row in the unfolded turn, opened.
        let single = s.reveal(|k| matches!(k, RowKind::Work { entry, .. } if matches!(entry.item, t3code_web::components::chat::timeline_entries::WorkItem::TodoList { .. })))?;
        let entry_key = s.cx.update(|cx| {
            timeline.read(cx).rows().iter().find(|r| r.key == single).and_then(|r| match &r.kind {
                RowKind::Work { entry, .. } => Some(MessagesTimeline::key_of(entry.id.as_str())),
                _ => None,
            })
        });
        if let Some(entry) = entry_key {
            s.click_entry(entry)?;
            s.shot(&out.join("dark-todo-expanded.png"))?;
        }
    }
    Ok(())
}

struct Stats(Vec<Duration>);

impl Stats {
    fn summary(&mut self) -> String {
        self.0.sort();
        let pick = |q: f64| self.0[((self.0.len() as f64 - 1.) * q).round() as usize];
        let mean = self.0.iter().sum::<Duration>() / self.0.len() as u32;
        format!(
            "n={} mean={:.2}ms p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms",
            self.0.len(),
            mean.as_secs_f64() * 1e3,
            pick(0.5).as_secs_f64() * 1e3,
            pick(0.95).as_secs_f64() * 1e3,
            pick(0.99).as_secs_f64() * 1e3,
            self.0.last().unwrap().as_secs_f64() * 1e3
        )
    }
}

/// Scroll frames over a thread of 200 formulas and streaming a reply with math, with the
/// Render math setting on; counts formula layouts to show finished ones are not laid out again.
fn perf_math(out: &Path) -> anyhow::Result<()> {
    use t3code_web::components::chat::timeline_fixtures::{math_reply, math_thread};
    let mut report = String::new();
    let mut s = Session::open(Appearance::Dark, 1100., 780.)?;
    s.cx.update(|cx| t3code_web::hooks::use_settings::ClientSettingsStore::update(cx, |settings| settings.math_rendering_enabled = true));
    s.settle()?;
    let input = math_thread(100);
    let started = Instant::now();
    s.set_thread("math", input.clone());
    s.frame()?;
    writeln!(report, "math thread: 100 turns, 200 formulas; first paint {:.1}ms, formula layouts so far {}", started.elapsed().as_secs_f64() * 1e3, t3code_web::math::layouts())?;
    s.settle()?;
    let timeline = s.timeline();
    for pass in ["cold", "warm"] {
        let layouts = t3code_web::math::layouts();
        let mut scroll = Stats(Vec::new());
        let mut up = true;
        for step in 0..400 {
            if step == 200 { up = false; }
            let started = Instant::now();
            s.cx.update(|cx| {
                timeline.read(cx).list_state().scroll_by(px(if up { -120. } else { 120. }), t3code_web::components::chat::timeline_viewport::TimelineNavigation::Wheel { up })
            });
            s.frame()?;
            scroll.0.push(started.elapsed());
        }
        let over = scroll.0.iter().filter(|d| d.as_secs_f64() > 0.016).count();
        writeln!(report, "scroll frames, {pass} (200 wheel steps up and back, 120px each): {} ; over 16ms: {over}; formula layouts during the pass: {}", scroll.summary(), t3code_web::math::layouts() - layouts)?;
    }
    s.shot(&out.join("perf-math-scrolled.png"))?;
    let reply = math_reply(12, 1000);
    let base = streaming_thread(input, "");
    s.set_thread("math-stream", base);
    s.settle()?;
    s.cx.update(|cx| timeline.read(cx).list_state().follow_end());
    let reply_id = t3code_web::components::chat::timeline_entries::MessageId::new("stream-reply");
    let layouts = t3code_web::math::layouts();
    let mut stream = Stats(Vec::new());
    let mut cut = 0;
    let mut deltas = 0;
    while cut < reply.len() {
        cut = (cut + 12).min(reply.len());
        while !reply.is_char_boundary(cut) { cut += 1; }
        let text: gpui_kit::SharedString = reply[..cut].to_owned().into();
        let started = Instant::now();
        s.cx.update(|cx| timeline.update(cx, |t, cx| t.update_message_text(&reply_id, text, cut < reply.len(), cx)));
        s.frame()?;
        stream.0.push(started.elapsed());
        deltas += 1;
    }
    let streamed = t3code_web::math::layouts() - layouts;
    writeln!(report, "streaming a reply with 24 formulas in 12-char deltas ({deltas} deltas): {}; formula layouts {streamed} (each finished formula is laid out once; the growing one per delta that touches it)", stream.summary())?;
    s.shot(&out.join("perf-math-streamed.png"))?;
    print!("{report}");
    std::fs::write(out.join("perf-math.txt"), report)?;
    Ok(())
}

fn perf(out: &Path) -> anyhow::Result<()> {
    let mut report = String::new();
    let rss_start = rss_kib();
    let mut s = Session::open(Appearance::Dark, 1100., 780.)?;
    s.settle()?;
    let rss_window = rss_kib();

    let input = perf_thread(1_000);
    let entries = input.entries.len();
    let started = Instant::now();
    s.set_thread("perf", input.clone());
    s.frame()?;
    let first_paint = started.elapsed();
    s.settle()?;
    let timeline = s.timeline();
    let rows = s.cx.update(|cx| timeline.read(cx).rows().len());
    let rss_open = rss_kib();
    writeln!(report, "thread: {entries} entries -> {rows} rows (1000 turns, long rust code every other reply)")?;
    writeln!(report, "time to first paint (set_thread + first frame, cold caches): {:.1}ms", first_paint.as_secs_f64() * 1e3)?;

    // Switch away and back: per-thread memory restores the position, caches are warm.
    s.set_thread("other", fixture_thread());
    s.settle()?;
    let started = Instant::now();
    s.set_thread("perf", input.clone());
    s.frame()?;
    writeln!(report, "time to first paint, reopening (warm caches): {:.1}ms", started.elapsed().as_secs_f64() * 1e3)?;
    s.settle()?;

    // Scroll up from the end in 120px wheel steps, one frame each.
    let mut scroll = Stats(Vec::new());
    for _ in 0..1_500 {
        let started = Instant::now();
        s.cx.update(|cx| {
            timeline.read(cx).list_state().scroll_by(px(-120.), t3code_web::components::chat::timeline_viewport::TimelineNavigation::Wheel { up: true })
        });
        s.frame()?;
        scroll.0.push(started.elapsed());
    }
    let follow = s.cx.update(|cx| timeline.read(cx).follow());
    writeln!(report, "scroll frames (120px wheel steps from the end, 1500 frames = 180,000px): {}", scroll.summary())?;
    writeln!(report, "  follow after scrolling up: {follow:?} (expected Released)")?;
    // Page-sized jumps land on unmeasured rows.
    let mut jumps = Stats(Vec::new());
    for _ in 0..200 {
        let started = Instant::now();
        s.cx.update(|cx| {
            timeline.read(cx).list_state().scroll_by(px(2_000.), t3code_web::components::chat::timeline_viewport::TimelineNavigation::Command { releases_follow: true })
        });
        s.frame()?;
        jumps.0.push(started.elapsed());
    }
    writeln!(report, "jump frames (2000px per frame, mostly unmeasured rows): {}", jumps.summary())?;
    let rss_scrolled = rss_kib();

    // Stream a 20,000-character reply at the end, 40 characters per delta.
    let reply = streaming_reply(20_000);
    let base = streaming_thread(input.clone(), "");
    s.set_thread("stream", base.clone());
    s.settle()?;
    s.cx.update(|cx| timeline.read(cx).list_state().follow_end());
    let mut stream = Stats(Vec::new());
    let reply_id = t3code_web::components::chat::timeline_entries::MessageId::new("stream-reply");
    let mut cut = 0;
    while cut < reply.len() {
        cut = (cut + 40).min(reply.len());
        while !reply.is_char_boundary(cut) {
            cut += 1;
        }
        let text: gpui_kit::SharedString = reply[..cut].to_owned().into();
        let streaming = cut < reply.len();
        let started = Instant::now();
        s.cx.update(|cx| timeline.update(cx, |t, cx| t.update_message_text(&reply_id, text, streaming, cx)));
        s.frame()?;
        stream.0.push(started.elapsed());
    }
    // The cost the fast path avoids: re-deriving every row from the entries.
    let mut derive = Stats(Vec::new());
    for _ in 0..50 {
        let next = s.cx.update(|cx| timeline.read(cx).input().clone());
        let started = Instant::now();
        s.cx.update(|cx| timeline.update(cx, |t, cx| t.update_input(next, cx)));
        derive.0.push(started.elapsed());
    }
    let following = s.cx.update(|cx| timeline.read(cx).follow()) == TimelineFollow::Following;
    writeln!(report, "streaming frames (20,000 chars in 40-char deltas at the end of the 3000-row thread, delta + frame): {}", stream.summary())?;
    writeln!(report, "full row re-derivation of the 3000-row thread (update_input, not used per delta): {}", derive.summary())?;
    writeln!(report, "  still following the end after streaming: {following}")?;
    s.shot(&out.join("perf-streamed-end.png"))?;
    let rss_end = rss_kib();
    writeln!(
        report,
        "resident memory: process start {} MiB, window open {} MiB, 3000-row thread open {} MiB, after scrolling all {} MiB, after streaming {} MiB",
        rss_start / 1024,
        rss_window / 1024,
        rss_open / 1024,
        rss_scrolled / 1024,
        rss_end / 1024
    )?;
    print!("{report}");
    std::fs::write(out.join("perf.txt"), report)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().context("usage: chat-bench <shots|perf> <out-dir>")?;
    let out = PathBuf::from(args.next().context("missing out-dir")?);
    std::fs::create_dir_all(&out)?;
    match mode.as_str() {
        "shots" => shots(&out),
        "perf" => perf(&out),
        "perf-math" => perf_math(&out),
        other => Err(anyhow!("unknown mode {other}")),
    }
}
