// Headless end-to-end checks for the terminal.
// Usage:
//   terminal-check pty <out-dir>    drives bash in a local PTY (ls --color, vim, resize, top) and screenshots each step
//   terminal-check bench <out-dir>  throughput, frame times and memory; writes bench.txt
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, Entity, HeadlessAppContext, IntoElement, ParentElement as _, Render,
    Styled as _, Window, WindowBounds, WindowOptions, component::TitleBar, div, px, size,
    test::TestWindowExt as _,
};
use t3code_web::terminal::TerminalSurface;
use t3code_web::terminal::backend::RecordedBackend;
use t3code_web::terminal::local_pty::LocalPty;

struct Host(Entity<TerminalSurface>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

struct Harness {
    cx: HeadlessAppContext,
    handle: AnyWindowHandle,
    surface: Entity<TerminalSurface>,
}

impl Harness {
    fn open(width: f32, height: f32, backend: impl FnOnce() -> Box<dyn t3code_web::terminal::TerminalBackend>) -> anyhow::Result<Self> {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(t3code_web::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.allow_parking();
        cx.update(t3code_web::init);
        let backend = backend();
        let mut surface = None;
        let (handle, _host) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)),
                show: false,
                focus: true,
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                let view = cx.new(|cx| TerminalSurface::new(backend, window, cx));
                let focus = view.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                surface = Some(view.clone());
                cx.new(|_| Host(view))
            })
        })?;
        let surface = surface.context("surface created")?;
        let mut harness = Self { cx, handle, surface };
        harness.frame();
        Ok(harness)
    }

    fn any(&self) -> AnyWindowHandle {
        self.handle
    }

    /// Runs pending work and renders a frame; returns the frame's CPU time.
    fn frame(&mut self) -> Duration {
        self.cx.run_until_parked();
        let started = Instant::now();
        self.cx.update_window(self.any(), |_, window, cx| window.render_frame(cx)).ok();
        started.elapsed()
    }

    fn screen(&mut self) -> String {
        self.cx.update(|cx| self.surface.read(cx).core().screen_text())
    }

    /// Waits in real time for the PTY until the screen satisfies `ready`.
    fn wait_for(&mut self, what: &str, ready: impl Fn(&str) -> bool) -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.frame();
            if ready(&self.screen()) {
                // Let trailing output settle.
                std::thread::sleep(Duration::from_millis(150));
                self.frame();
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for {what}; screen:\n{}", self.screen());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn type_text(&mut self, text: &str) {
        let any = self.any();
        self.cx.update_window(any, |_, window, cx| window.input(text, cx)).ok();
    }

    fn press(&mut self, key: &str) {
        let any = self.any();
        self.cx.update_window(any, |_, window, cx| window.press(key, cx)).ok();
    }

    fn resize(&mut self, width: f32, height: f32) {
        let any = self.any();
        self.cx
            .update_window(any, |_, window, cx| {
                window.resize(size(px(width), px(height)));
                window.bounds_changed(cx);
            })
            .ok();
        self.frame();
        // The PTY hears settled sizes after the debounce.
        self.cx.advance_clock(Duration::from_millis(200));
        self.cx.update(|cx| self.surface.update(cx, |surface, _| surface.flush_resize()));
        self.frame();
    }

    fn shot(&mut self, path: &Path) -> anyhow::Result<()> {
        self.frame();
        let image = self.cx.capture_screenshot(self.any())?;
        image.save(path)?;
        println!("{}: {}x{}", path.display(), image.width(), image.height());
        Ok(())
    }
}

fn fixture_dir(out: &Path) -> anyhow::Result<PathBuf> {
    let dir = out.join("pty-fixture");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(dir.join("apps"))?;
    std::fs::create_dir_all(dir.join("packages"))?;
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\n")?;
    std::fs::write(dir.join("README.md"), "# demo\n")?;
    std::fs::write(dir.join("archive.tgz"), [0u8; 16])?;
    std::fs::write(dir.join("build.sh"), "#!/bin/sh\necho build\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir.join("build.sh"), std::fs::Permissions::from_mode(0o755))?;
        std::os::unix::fs::symlink("README.md", dir.join("link"))?;
    }
    Ok(dir)
}

fn pty(out: &Path) -> anyhow::Result<()> {
    let dir = fixture_dir(out)?;
    let mut h = Harness::open(1000., 560., || {
        Box::new(
            LocalPty::new("bash", &["--norc", "--noprofile", "-i"], Some(dir.clone()))
                .with_env("PS1", r"\[\e[1;32m\]user@host\[\e[0m\]:\[\e[1;34m\]\W\[\e[0m\]$ ")
                .with_env("LS_COLORS", "di=1;34:ln=1;36:ex=1;32:*.tgz=1;31")
                .with_env("HOME", &dir.display().to_string()),
        )
    })?;
    let mut log = String::new();
    h.wait_for("prompt", |s| s.contains("user@host"))?;

    h.type_text("ls --color=always");
    h.press("enter");
    h.wait_for("ls output", |s| s.contains("archive.tgz") && s.matches("user@host").count() >= 2)?;
    h.type_text("printf '\\e[3mitalic\\e[0m \\e[4munder\\e[0m \\e[9mstrike\\e[0m \\e[38;2;255;120;0mtruecolor\\e[0m 日本 🚀\\n'");
    h.press("enter");
    h.wait_for("printf output", |s| s.contains("truecolor"))?;
    h.shot(&out.join("pty-ls.png"))?;
    writeln!(log, "ls --color:\n{}", h.screen())?;

    h.type_text("vim -u NONE -N -c 'syntax on' demo.sh");
    h.press("enter");
    h.wait_for("vim", |s| s.contains("demo.sh"))?;
    h.press("i");
    h.type_text("#!/bin/sh");
    h.press("enter");
    h.type_text("echo \"native terminal\" # typed through the GPUI input path");
    h.press("escape");
    h.wait_for("vim insert", |s| s.contains("native terminal"))?;
    h.shot(&out.join("pty-vim.png"))?;
    writeln!(log, "vim:\n{}", h.screen())?;

    let before = h.cx.update(|cx| h.surface.read(cx).core().size());
    h.resize(700., 380.);
    let after = h.cx.update(|cx| h.surface.read(cx).core().size());
    h.type_text(":set ruler");
    h.press("enter");
    h.wait_for("vim redraw after resize", |s| s.lines().filter(|l| !l.is_empty()).count() > 2)?;
    // vim prints its window size through `:echo`.
    h.type_text(":echo &columns . 'x' . &lines");
    h.press("enter");
    let expected = format!("{}x{}", after.cols, after.rows);
    h.wait_for("vim sees new size", |s| s.contains(&expected))?;
    h.shot(&out.join("pty-vim-resized.png"))?;
    writeln!(log, "resize {before:?} -> {after:?}; vim reported {expected}\n{}", h.screen())?;
    h.press("escape");
    h.type_text(":q!");
    h.press("enter");
    h.wait_for("shell after vim", |s| s.lines().any(|l| l.ends_with("$")))?;

    h.resize(1000., 560.);
    h.type_text("top -d 0.5 -u nobody");
    h.press("enter");
    h.wait_for("top", |s| s.contains("PID") && s.contains("Tasks"))?;
    h.shot(&out.join("pty-top.png"))?;
    writeln!(log, "top:\n{}", h.screen())?;
    h.press("q");
    h.wait_for("shell after top", |s| s.lines().any(|l| l.ends_with("$")))?;

    // Scrollback with the wheel and a drag selection over ls output.
    h.type_text("seq 1 400");
    h.press("enter");
    h.wait_for("seq", |s| s.contains("400"))?;
    let any = h.any();
    h.cx.update_window(any, |_, window, cx| {
        let event = gpui_kit::ScrollEvent {
            position: gpui_kit::point(px(300.), px(200.)),
            delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., 12.)),
            ..Default::default()
        };
        window.dispatch_event(gpui_kit::InputEvent::to_platform_input(event), cx);
    })?;
    h.frame();
    let offset = h.cx.update(|cx| h.surface.read(cx).core().display_offset());
    h.shot(&out.join("pty-scrolled.png"))?;
    writeln!(log, "wheel scrolled history by {offset} lines")?;

    std::fs::write(out.join("pty-transcript.txt"), log)?;
    Ok(())
}

/// Mixed output: plain lines and lines dense with SGR colour changes.
fn flood_chunk(index: usize) -> Vec<u8> {
    let mut out = String::with_capacity(70_000);
    let mut line = 0;
    while out.len() < 64 * 1024 {
        line += 1;
        if (index + line) % 2 == 0 {
            let _ = write!(out, "{index:06}:{line:04} plain build output: compiling crate number {line} with features default\r\n");
        } else {
            for word in 0..12 {
                let _ = write!(out, "\x1b[38;5;{}m\x1b[48;2;{};{};40mw{word}\x1b[1;4m!\x1b[0m ", (line * 7 + word) % 256, word * 20, line % 255);
            }
            out.push_str("\r\n");
        }
    }
    out.into_bytes()
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find(|line| line.starts_with("VmRSS:")).and_then(|line| line.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn percentile(samples: &mut [Duration], p: f64) -> f64 {
    samples.sort();
    let index = ((samples.len() as f64 - 1.) * p).round() as usize;
    samples[index].as_secs_f64() * 1000.
}

fn bench(out: &Path) -> anyhow::Result<()> {
    let mut report = String::new();
    let chunks: Vec<Vec<u8>> = (0..800).map(flood_chunk).collect();
    let total: usize = chunks.iter().map(Vec::len).sum();
    let megabytes = total as f64 / (1024. * 1024.);
    writeln!(report, "flood: {} chunks of ~64 KiB, {megabytes:.1} MiB, half plain lines, half 12 SGR runs per line", chunks.len())?;

    // 1. Parser alone.
    let mut core = t3code_web::terminal::core::TerminalCore::new(t3code_web::terminal::GridSize { cols: 160, rows: 48 }, 10_000);
    let started = Instant::now();
    for chunk in &chunks {
        core.feed(chunk);
    }
    let parse = started.elapsed();
    writeln!(report, "parse only (160x48, 10k scrollback): {:.2} s, {:.1} MiB/s", parse.as_secs_f64(), megabytes / parse.as_secs_f64())?;

    // 2. Through the view, rendering a frame per flow-control window (8 chunks = 512 KiB).
    let mut h = Harness::open(1280., 800., || Box::new(RecordedBackend::new(Vec::new())))?;
    let mut frames = Vec::new();
    let started = Instant::now();
    for window in chunks.chunks(8) {
        h.cx.update(|cx| {
            h.surface.update(cx, |surface, cx| {
                for chunk in window {
                    surface.write_output(chunk, cx);
                }
            })
        });
        frames.push(h.frame());
    }
    let flood = started.elapsed();
    let size = h.cx.update(|cx| h.surface.read(cx).core().size());
    writeln!(
        report,
        "flood through view ({}x{} grid, 1280x800 window): {:.2} s, {:.1} MiB/s; frame CPU time p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms over {} frames",
        size.cols,
        size.rows,
        flood.as_secs_f64(),
        megabytes / flood.as_secs_f64(),
        percentile(&mut frames.clone(), 0.5),
        percentile(&mut frames.clone(), 0.95),
        percentile(&mut frames, 1.0),
        chunks.len() / 8,
    )?;
    h.shot(&out.join("bench-flood.png"))?;
    drop(h);

    // 3. 100,000-line scrollback: memory and scrolling frame time.
    let rss_before = rss_kib();
    let mut h = Harness::open(1280., 800., || Box::new(RecordedBackend::new(Vec::new())))?;
    h.cx.update(|cx| h.surface.update(cx, |surface, _| surface.core_mut().set_scrollback(100_000)));
    let mut text = String::new();
    for line in 0..100_100 {
        let _ = write!(text, "\x1b[3{}m{line:06}\x1b[0m scrollback line with some ordinary text to fill the row \x1b[1mbold\x1b[0m tail\r\n", line % 8);
        if text.len() > 64 * 1024 {
            h.cx.update(|cx| h.surface.update(cx, |surface, cx| surface.write_output(text.as_bytes(), cx)));
            text.clear();
        }
    }
    h.cx.update(|cx| h.surface.update(cx, |surface, cx| surface.write_output(text.as_bytes(), cx)));
    h.frame();
    let history = h.cx.update(|cx| h.surface.read(cx).core().history_size());
    let rss_after = rss_kib();
    let mut scroll_frames = Vec::new();
    for step in 0..600 {
        let delta = if (step / 100) % 2 == 0 { 30 } else { -30 };
        h.cx.update(|cx| h.surface.update(cx, |surface, cx| {
            surface.core_mut().scroll_lines(delta);
            cx.notify();
        }));
        scroll_frames.push(h.frame());
    }
    writeln!(
        report,
        "scrollback: {history} lines kept; RSS {:.1} MiB before window, {:.1} MiB with full scrollback (+{:.1} MiB)",
        rss_before as f64 / 1024.,
        rss_after as f64 / 1024.,
        (rss_after.saturating_sub(rss_before)) as f64 / 1024.
    )?;
    writeln!(
        report,
        "scrolling 30 lines per frame through history: frame CPU time p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms over {} frames",
        percentile(&mut scroll_frames.clone(), 0.5),
        percentile(&mut scroll_frames.clone(), 0.95),
        percentile(&mut scroll_frames, 1.0),
        600
    )?;
    h.shot(&out.join("bench-scrollback.png"))?;

    print!("{report}");
    std::fs::write(out.join("bench.txt"), report)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: terminal-check <pty|bench> <out-dir>";
    let command = args.next().context(usage)?;
    let out = PathBuf::from(args.next().context(usage)?);
    std::fs::create_dir_all(&out)?;
    match command.as_str() {
        "pty" => pty(&out),
        "bench" => bench(&out),
        _ => bail!(usage),
    }
}
