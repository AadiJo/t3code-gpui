// End-to-end checks and measurements for the diff panel, headless.
//
//   diffs_harness e2e <out-dir>    scripted clicks, drags, typing and keys, with a
//                                  screenshot per step and a transcript
//   diffs_harness perf <out-dir>   first paint, scroll frame times (cold and warm),
//                                  5,000-line expand and resident memory
use std::{
    cell::RefCell,
    fmt::Write as _,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use gpui_kit::{test::TestWindowExt as _, *};
use t3code_web::{
    components::{
        diff_panel::{DiffPanel, DiffPanelEvent},
        diffs::{CodeView, ViewMode},
    },
    diff_panel_store::{DiffLayout, DiffPanelStore},
    diff_patch::{FileContents, parse_patch},
    scenes::diffs::{Fixture, fixture_input},
    syntax::SyntaxStore,
    theme_palette::{AppTheme, Appearance, ThemeDocument, set_app_theme},
};

struct Host(AnyView);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

struct Harness {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    out: PathBuf,
    log: String,
}

fn headless() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(t3code_web::init);
    cx
}

fn set_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
    set_app_theme(AppTheme::new(&ThemeDocument::graphite(), appearance), cx);
    let mode = if appearance == Appearance::Light { component::ThemeMode::Light } else { component::ThemeMode::Dark };
    component::Theme::change(mode, Some(window), cx);
}

impl Harness {
    fn open(
        mut cx: HeadlessAppContext,
        width: f32,
        height: f32,
        out: &Path,
        build: impl FnOnce(&mut Window, &mut App) -> AnyView,
    ) -> Result<Self> {
        let (window, _) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)),
                show: false,
                focus: false,
                ..component::TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                let view = build(window, cx);
                cx.new(|_| Host(view))
            })
        })?;
        let mut harness = Self { cx, window, out: out.to_owned(), log: String::new() };
        harness.settle();
        Ok(harness)
    }

    fn frame(&mut self) -> Duration {
        let started = Instant::now();
        self.cx.update_window(self.window, |_, window, cx| window.render_frame(cx)).ok();
        started.elapsed()
    }

    /// Renders, lets background work (highlighting) finish, renders again.
    fn settle(&mut self) {
        self.frame();
        self.cx.run_until_parked();
        self.frame();
        self.cx.run_until_parked();
        self.frame();
    }

    fn with_window<R>(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        self.cx.update_window(self.window, |_, window, cx| f(window, cx)).expect("window open")
    }

    fn shot(&mut self, name: &str) -> Result<()> {
        self.settle();
        let image = self.cx.capture_screenshot(self.window)?;
        let path = self.out.join(format!("{name}.png"));
        image.save(&path)?;
        let _ = writeln!(self.log, "screenshot {}", path.display());
        Ok(())
    }

    fn note(&mut self, line: impl AsRef<str>) {
        println!("{}", line.as_ref());
        let _ = writeln!(self.log, "{}", line.as_ref());
    }
}

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| status.lines().find(|l| l.starts_with("VmRSS:")).map(str::to_owned))
        .and_then(|line| line.split_whitespace().nth(1).and_then(|kb| kb.parse::<f64>().ok()))
        .map_or(0., |kb| kb / 1024.)
}

fn element_bounds(harness: &mut Harness, id: impl Into<ElementId>) -> Bounds<Pixels> {
    let id = id.into();
    harness.with_window(|window, _| window.find(id).bounds())
}

/// The scripted flow for one appearance and layout.
fn e2e_flow(out: &Path, appearance: Appearance, layout: DiffLayout, width: f32) -> Result<String> {
    let tag = format!(
        "{}-{}-{}",
        if appearance == Appearance::Dark { "dark" } else { "light" },
        if layout == DiffLayout::Stacked { "stacked" } else { "split" },
        width as u32
    );
    let fixture = Fixture::standard();
    let chat_contents = FileContents::new(&fixture.chat_view.0, &fixture.chat_view.1);
    let events: Rc<RefCell<Vec<String>>> = Rc::default();
    let panel_slot: Rc<RefCell<Option<Entity<DiffPanel>>>> = Rc::default();
    let mut cx = headless();
    cx.update(|cx| cx.global_mut::<DiffPanelStore>().settings.diff_layout = layout);
    let input = fixture_input(&fixture);
    let (slot, log) = (panel_slot.clone(), events.clone());
    let mut h = Harness::open(cx, width, 780., out, move |window, cx| {
        set_appearance(appearance, window, cx);
        let panel = cx.new(|cx| DiffPanel::new(window, cx));
        panel.update(cx, |panel, cx| panel.set_input(input, cx));
        cx.subscribe(&panel, move |_, event: &DiffPanelEvent, _| {
            log.borrow_mut().push(match event {
                DiffPanelEvent::RequestFileContents { key, .. } => format!("RequestFileContents {}", key.replace('\0', " | ")),
                DiffPanelEvent::ReviewComment(comment) => format!("ReviewComment {}", serde_json::to_string(comment).unwrap_or_default()),
                other => format!("{other:?}"),
            });
        })
        .detach();
        *slot.borrow_mut() = Some(panel.clone());
        panel.into()
    })?;
    let panel = panel_slot.borrow().clone().context("panel built")?;
    let code_view = h.cx.update(|cx| panel.read(cx).code_view().clone());
    h.note(format!("== {tag}"));
    h.shot(&format!("{tag}-01-collapsed"))?;

    // Expand the first file by clicking its header's empty space (the title opens the file).
    let header = element_bounds(&mut h, ("diff-header", 0usize));
    h.with_window(|window, cx| window.click_at(("diff-header", 0usize), point(header.size.width - px(90.), px(16.)), cx));
    h.shot(&format!("{tag}-02-header-click-expands"))?;

    // Collapse it again with the chevron, then expand it again.
    h.with_window(|window, cx| window.click(("diff-collapse", 0usize), cx));
    h.shot(&format!("{tag}-03-chevron-collapses"))?;
    h.with_window(|window, cx| window.click(("diff-collapse", 0usize), cx));

    // Expanding context asks for contents; deliver them as the server would, then expand.
    h.with_window(|window, cx| window.click(("sep-up", 1usize), cx));
    h.settle();
    let key = format!("{0}\0{0}\0change", fixture.chat_view_path);
    h.cx.update(|cx| panel.update(cx, |panel, cx| panel.set_file_contents(&key, chat_contents.clone(), cx)));
    h.with_window(|window, cx| window.click(("sep-up", 1usize), cx));
    h.shot(&format!("{tag}-04-context-expanded-up"))?;

    // Drag a text selection across lines, then copy it.
    let code = element_bounds(&mut h, "code-view");
    let ch = 7.8;
    let gutter = ch * 6. + 2.;
    let from = code.origin + point(px(gutter + ch * 5.), px(32. + 24. + 30.));
    let to = code.origin + point(px(gutter + ch * 20.), px(32. + 24. + 130.));
    h.with_window(|window, cx| window.drag(from, to, cx));
    h.with_window(|window, cx| window.press(if cfg!(target_os = "macos") { "cmd-c" } else { "ctrl-c" }, cx));
    let selected = h.cx.update(|cx| code_view.read(cx).selected_text()).unwrap_or_default();
    let clipboard = h.cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text())).unwrap_or_default();
    h.note(format!("selected text ({} lines):\n{selected}", selected.lines().count()));
    h.note(format!("clipboard matches selection: {}", clipboard == selected));
    h.shot(&format!("{tag}-05-text-selection"))?;

    // Select lines in the gutter, hover, click "+", type a comment and submit it.
    let gutter_x = code.origin.x + px(20.);
    let row_y = |offset: f32| code.origin.y + px(32. + 24. + 4. * 20. + offset);
    h.with_window(|window, cx| window.drag(point(gutter_x, row_y(10.)), point(gutter_x, row_y(70.)), cx));
    h.with_window(|window, cx| {
        let position = point(gutter_x, row_y(70.));
        window.dispatch_event(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
        window.render_frame(cx);
    });
    h.with_window(|window, cx| window.click("gutter-comment", cx));
    h.settle();
    h.with_window(|window, cx| window.input("Use the shared helper here.", cx));
    h.shot(&format!("{tag}-06-comment-draft"))?;
    h.with_window(|window, cx| window.click("draft-submit", cx));
    h.shot(&format!("{tag}-07-comment-filed"))?;

    // Find: ctrl/cmd-f, type, Enter steps to the next match.
    h.with_window(|window, cx| {
        let position = code.origin + point(px(300.), px(400.));
        window.dispatch_event(MouseDownEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(), cx);
        window.dispatch_event(MouseUpEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
        window.render_frame(cx);
    });
    h.with_window(|window, cx| window.press(if cfg!(target_os = "macos") { "cmd-f" } else { "ctrl-f" }, cx));
    h.settle();
    h.with_window(|window, cx| window.input("handleMessage", cx));
    h.with_window(|window, cx| window.press("enter", cx));
    h.with_window(|window, cx| window.press("enter", cx));
    let status = h.cx.update(|cx| code_view.read(cx).find_status());
    h.note(format!("find status (matches, current): {status:?}"));
    h.shot(&format!("{tag}-08-find"))?;
    h.with_window(|window, cx| window.press("escape", cx));

    // Scroll inside a long file: its header sticks to the top.
    h.with_window(|window, cx| window.click("diff-collapse-all", cx));
    h.with_window(|window, cx| window.click("diff-collapse-all", cx));
    h.with_window(|window, cx| window.scroll("code-view", ScrollDelta::Pixels(point(px(0.), px(-600.))), cx));
    h.shot(&format!("{tag}-09-sticky-header"))?;

    // Keyboard: next hunk, then next file.

    // File tree: open it, pick a file, check it is revealed.
    h.with_window(|window, cx| window.click("diff-file-tree-toggle", cx));
    h.settle();
    h.with_window(|window, cx| window.click(("tree-row", 4usize), cx));
    h.shot(&format!("{tag}-12-tree-reveal"))?;

    for event in events.borrow().iter() {
        let _ = writeln!(h.log, "event {event}");
    }
    Ok(h.log)
}

/// Base screenshots: every layout, appearance and width, files collapsed and expanded.
fn base_shots(out: &Path) -> Result<String> {
    let mut log = String::new();
    for appearance in [Appearance::Dark, Appearance::Light] {
        for layout in [DiffLayout::Stacked, DiffLayout::Split] {
            for width in [540., 1000.] {
                let mut cx = headless();
                cx.update(|cx| cx.global_mut::<DiffPanelStore>().settings.diff_layout = layout);
                let input = fixture_input(&Fixture::standard());
                let slot: Rc<RefCell<Option<Entity<DiffPanel>>>> = Rc::default();
                let s = slot.clone();
                let mut h = Harness::open(cx, width, 780., out, move |window, cx| {
                    set_appearance(appearance, window, cx);
                    let panel = cx.new(|cx| DiffPanel::new(window, cx));
                    panel.update(cx, |panel, cx| panel.set_input(input, cx));
                    *s.borrow_mut() = Some(panel.clone());
                    panel.into()
                })?;
                let tag = format!(
                    "base-{}-{}-{}",
                    if appearance == Appearance::Dark { "dark" } else { "light" },
                    if layout == DiffLayout::Stacked { "stacked" } else { "split" },
                    width as u32
                );
                h.shot(&format!("{tag}-collapsed"))?;
                let panel = slot.borrow().clone().context("panel")?;
                h.cx.update(|cx| panel.update(cx, |panel, cx| panel.toggle_all_files(cx)));
                h.shot(&format!("{tag}-expanded"))?;
                log.push_str(&h.log);
            }
        }
    }
    Ok(log)
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() - 1) as f64 * p).round() as usize]
}

fn perf(out: &Path) -> Result<String> {
    let mut report = String::new();
    let rss_start = rss_mb();
    let started = Instant::now();
    let fixture = Fixture::perf(300, 60_000);
    let generate = started.elapsed();
    let started = Instant::now();
    let parsed = parse_patch(&fixture.patch);
    let parse = started.elapsed();
    let (additions, deletions) = parsed.line_stat();
    let _ = writeln!(report, "fixture: {} files, +{additions} -{deletions} ({} changed lines), patch {:.1} MB, generated in {generate:?}", parsed.files.len(), additions + deletions, fixture.patch.len() as f64 / 1e6);
    let _ = writeln!(report, "parse_patch: {:.1} ms", parse.as_secs_f64() * 1e3);
    drop(parsed);

    {
        let before = rss_mb();
        let empty = Harness::open(headless(), 540., 900., out, |_, cx| cx.new(|_| EmptyView).into())?;
        let _ = writeln!(report, "empty headless window (renderer baseline): RSS {before:.0} → {:.0} MB", rss_mb());
        drop(empty);
    }
    let cx = headless();
    let rss_init = rss_mb();
    let started = Instant::now();
    let _ = t3code_web::syntax::language_for_path("a.tsx");
    let _ = writeln!(report, "syntax set load: {:.1} ms, RSS {rss_init:.0} → {:.0} MB", started.elapsed().as_secs_f64() * 1e3, rss_mb());
    let slot: Rc<RefCell<Option<Entity<DiffPanel>>>> = Rc::default();
    let s = slot.clone();
    let input_started = Instant::now();
    let input = fixture_input(&fixture);
    let input_parse = input_started.elapsed();
    let mut h = {
        let started = Instant::now();
        let h = Harness::open(cx, 540., 900., out, move |window, cx| {
            set_appearance(Appearance::Dark, window, cx);
            let panel = cx.new(|cx| DiffPanel::new(window, cx));
            panel.update(cx, |panel, cx| panel.set_input(input, cx));
            *s.borrow_mut() = Some(panel.clone());
            panel.into()
        })?;
        let _ = writeln!(report, "panel open → first frames settled (collapsed files, includes window setup): {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
        h
    };
    let panel = slot.borrow().clone().context("panel")?;
    let code_view = h.cx.update(|cx| panel.read(cx).code_view().clone());
    // First paint of a freshly opened panel, measured as one frame on a new view.
    let open_started = Instant::now();
    let input = fixture_input(&fixture);
    let reparse = open_started.elapsed();
    let first_paint_started = Instant::now();
    h.cx.update(|cx| panel.update(cx, |panel, cx| panel.set_input(input, cx)));
    let first = h.frame();
    let first_paint = first_paint_started.elapsed();
    let _ = writeln!(report, "time to first paint (parse {:.1} ms + set_input + first frame {:.1} ms): {:.1} ms", reparse.as_secs_f64() * 1e3, first.as_secs_f64() * 1e3, (reparse + first_paint).as_secs_f64() * 1e3);
    let _ = writeln!(report, "  input build incl. parse at harness start: {:.1} ms", input_parse.as_secs_f64() * 1e3);
    let rss_open = rss_mb();

    // Expand every file: 60,000 changed lines laid out.
    let started = Instant::now();
    h.cx.update(|cx| panel.update(cx, |panel, cx| panel.toggle_all_files(cx)));
    let expand_frame = h.frame();
    let _ = writeln!(report, "expand all 300 files: {:.1} ms (frame {:.1} ms), content height {:.0} px", started.elapsed().as_secs_f64() * 1e3, expand_frame.as_secs_f64() * 1e3, h.cx.update(|cx| code_view.update(cx, |v, _| v.content_height())));
    let rss_expanded = rss_mb();

    // Scroll 40px per frame (a fast trackpad) through 2,000 frames: highlighting cold.
    let scroll_frames = 2_000;
    let step = 40.;
    let mut cold = Vec::with_capacity(scroll_frames);
    for i in 0..scroll_frames {
        h.cx.update(|cx| code_view.update(cx, |v, cx| v.set_scroll_offset(i as f32 * step, cx)));
        cold.push(h.frame().as_secs_f64() * 1e3);
    }
    let rss_cold = rss_mb();
    let started = Instant::now();
    h.cx.run_until_parked();
    let rss_drained = rss_mb();
    let highlight_drain = started.elapsed();
    let stats = h.cx.update(|cx| SyntaxStore::global(cx).read(cx).stats);
    // Same path again with highlighting warm.
    let mut warm = Vec::with_capacity(scroll_frames);
    for i in 0..scroll_frames {
        h.cx.update(|cx| code_view.update(cx, |v, cx| v.set_scroll_offset(i as f32 * step, cx)));
        warm.push(h.frame().as_secs_f64() * 1e3);
    }
    let _ = writeln!(
        report,
        "scroll frames, {scroll_frames} frames at {step}px (CPU frame build: layout, shaping, paint list):\n  cold: p50 {:.2} ms, p95 {:.2} ms, p99 {:.2} ms, max {:.2} ms\n  warm: p50 {:.2} ms, p95 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        percentile(&mut cold.clone(), 0.5),
        percentile(&mut cold.clone(), 0.95),
        percentile(&mut cold.clone(), 0.99),
        percentile(&mut cold, 1.0),
        percentile(&mut warm.clone(), 0.5),
        percentile(&mut warm.clone(), 0.95),
        percentile(&mut warm.clone(), 0.99),
        percentile(&mut warm, 1.0),
    );
    let _ = writeln!(report, "highlighting: {} lines parsed in {:.1} ms of background time; drain after scroll {:.1} ms", stats.lines_parsed, stats.busy_micros as f64 / 1e3, highlight_drain.as_secs_f64() * 1e3);
    let rss_warm = rss_mb();
    h.shot("perf-scrolled")?;

    // Expand the 5,000-line file in the standard fixture.
    let cx = headless();
    let slot: Rc<RefCell<Option<Entity<CodeView>>>> = Rc::default();
    let s = slot.clone();
    let standard = Fixture::standard();
    let input = fixture_input(&standard);
    let mut h2 = Harness::open(cx, 540., 900., out, move |window, cx| {
        set_appearance(Appearance::Dark, window, cx);
        let panel = cx.new(|cx| DiffPanel::new(window, cx));
        panel.update(cx, |panel, cx| panel.set_input(input, cx));
        *s.borrow_mut() = Some(panel.read(cx).code_view().clone());
        panel.into()
    })?;
    let view = slot.borrow().clone().context("view")?;
    let large_index = 11usize;
    let started = Instant::now();
    h2.cx.update(|cx| view.update(cx, |v, cx| v.toggle_file(large_index, cx)));
    let frame = h2.frame();
    let expand_large = started.elapsed();
    let _ = writeln!(report, "expand the 5,000-line file: {:.2} ms to the first frame (frame {:.2} ms)", expand_large.as_secs_f64() * 1e3, frame.as_secs_f64() * 1e3);
    let started = Instant::now();
    h2.cx.update(|cx| view.update(cx, |v, cx| v.reveal_file("crates/engine/src/large.rs\0crates/engine/src/large.rs\0new", cx)));
    let frame = h2.frame();
    let _ = writeln!(report, "jump to it and paint (cold highlight): {:.2} ms (frame {:.2} ms)", started.elapsed().as_secs_f64() * 1e3, frame.as_secs_f64() * 1e3);
    h2.shot("perf-large-file")?;

    let _ = writeln!(report, "resident memory (VmRSS): start {rss_start:.0} MB, app+fonts {rss_init:.0} MB, panel open {rss_open:.0} MB, all expanded {rss_expanded:.0} MB, after cold scroll {rss_cold:.0} MB, after highlighting drained {rss_drained:.0} MB, after warm scroll {rss_warm:.0} MB, end {:.0} MB", rss_mb());

    // The plain code view on a 60,000-line file.
    let big: String = (0..60_000).map(|i| format!("    let value_{i} = compute(\"row {i}\", {i}) + offset; // line {i}\n")).collect();
    let cx = headless();
    let slot: Rc<RefCell<Option<Entity<CodeView>>>> = Rc::default();
    let s = slot.clone();
    let started = Instant::now();
    let mut h3 = Harness::open(cx, 900., 900., out, move |window, cx| {
        set_appearance(Appearance::Dark, window, cx);
        let view = cx.new(|cx| {
            let mut view = CodeView::new(ViewMode::File, window, cx);
            view.set_file_contents("src/big.rs".into(), &big, cx);
            view
        });
        *s.borrow_mut() = Some(view.clone());
        view.into()
    })?;
    let _ = writeln!(report, "code view, 60,000-line file: open and settle {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
    let view = slot.borrow().clone().context("view")?;
    let started = Instant::now();
    h3.cx.update(|cx| view.update(cx, |v, cx| v.reveal_line(45_000, cx)));
    let frame = h3.frame();
    let _ = writeln!(report, "  reveal line 45,000: {:.2} ms (frame {:.2} ms)", started.elapsed().as_secs_f64() * 1e3, frame.as_secs_f64() * 1e3);
    h3.shot("perf-code-view-reveal")?;
    Ok(report)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "e2e".into());
    let out = PathBuf::from(args.next().context("usage: diffs_harness <e2e|perf|base> <out-dir>")?);
    std::fs::create_dir_all(&out)?;
    let log = match mode.as_str() {
        "perf" => perf(&out)?,
        "base" => base_shots(&out)?,
        _ => {
            let mut log = String::new();
            log.push_str(&e2e_flow(&out, Appearance::Dark, DiffLayout::Stacked, 540.)?);
            log.push_str(&e2e_flow(&out, Appearance::Light, DiffLayout::Split, 1000.)?);
            log
        }
    };
    std::fs::write(out.join(format!("{mode}.txt")), &log)?;
    println!("{log}");
    Ok(())
}
