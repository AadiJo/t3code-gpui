// Composer typing latency and scripted interaction checks, headless.
// Usage: composer-bench <out-dir>
// Writes typing.txt (per-keystroke timings), checks.txt (scripted behaviour)
// and PNG screenshots of each scripted step, in dark and light.
use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, HeadlessAppContext, IntoElement, Keystroke, Modifiers,
    Focusable as _, InputEvent as _, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Render, Styled as _, Window, WindowBounds,
    WindowOptions, component::TitleBar, div, px, size,
};
use t3code_web::ActiveAppTheme as _;
use t3code_web::components::chat::composer::{ChatComposer, ComposerEvent};
use t3code_web::composer_draft_store::{ComposerDraftStore, InMemoryDraftStore};
use t3code_web::scenes::composer::{FixtureMenuSource, fixture_draft, fixture_state};
use t3code_web::theme_palette::{AppTheme, Appearance, ThemeDocument, set_app_theme};

/// Hosts one composer at the bottom of a chat-sized window.
struct Host {
    composer: Entity<ChatComposer>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        div().size_full().bg(colors.canvas).flex().flex_col().justify_end().p(px(24.)).child(self.composer.clone())
    }
}

struct Harness {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    composer: Entity<ChatComposer>,
    events: Rc<RefCell<Vec<ComposerEvent>>>,
    out: std::path::PathBuf,
    log: String,
}

/// A ~5,000-character draft with chips, inline Markdown and a code block.
fn long_draft() -> String {
    let mut text = String::new();
    while text.len() < 4600 {
        text.push_str(&fixture_draft());
        text.push_str("\n\n");
    }
    text.push_str("Closing paragraph with a few more words to type into.");
    text
}

impl Harness {
    fn new(appearance: Appearance, out: &std::path::Path) -> anyhow::Result<Self> {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(t3code_web::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(t3code_web::init);
        cx.update(|cx| {
            set_app_theme(AppTheme::new(&ThemeDocument::graphite(), appearance), cx);
            t3code_web::components::chat::composer::init(cx);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = events.clone();
        let (window, composer) = cx.update(|cx| -> anyhow::Result<_> {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(880.), px(760.)), cx)),
                show: false,
                focus: true,
                ..TitleBar::window_options()
            };
            let mut composer_out = None;
            let handle = gpui_kit::open_window(options, cx, |_, cx| {
                let store: Rc<RefCell<dyn ComposerDraftStore>> = Rc::new(RefCell::new(InMemoryDraftStore::default()));
                let composer = cx.new(|cx| ChatComposer::new(fixture_state("bench"), Rc::new(FixtureMenuSource), store, cx));
                cx.subscribe(&composer, move |_, event: &ComposerEvent, _| recorded.borrow_mut().push(event.clone())).detach();
                composer_out = Some(composer.clone());
                cx.new(|_| Host { composer })
            })?;
            Ok((handle.0, composer_out.context("composer")?))
        })?;
        let mut this = Self { cx, window, composer, events, out: out.to_owned(), log: String::new() };
        this.with(|composer, window, cx| {
            let focus = composer.read(cx).editor().focus_handle(cx);
            window.focus(&focus, cx);
        });
        this.frame();
        Ok(this)
    }

    fn with<R>(&mut self, f: impl FnOnce(&Entity<ChatComposer>, &mut Window, &mut App) -> R) -> R {
        let composer = self.composer.clone();
        self.cx.update_window(self.window, |_, w, cx| f(&composer, w, cx)).expect("window")
    }

    /// Draws a frame from whatever is dirty, as the platform's frame callback does.
    fn frame(&mut self) {
        self.cx.update_window(self.window, |_, window, cx| window.draw(cx).clear(cx)).ok();
        self.cx.run_until_parked();
    }

    fn key(&mut self, source: &str) {
        let os = self.cx.update(|cx| cx.platform_info().os);
        let mut keystroke = Keystroke::parse_with_os(source, os).expect("keystroke");
        if source.chars().count() == 1 {
            keystroke.key_char = Some(source.to_owned());
        }
        self.cx.update_window(self.window, |_, window, cx| window.dispatch_keystroke(keystroke, cx)).ok();
    }

    fn press(&mut self, source: &str) {
        self.key(source);
        self.frame();
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let key = if c == ' ' { "space".to_owned() } else { c.to_string() };
            self.press(&key);
        }
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame();
        let image = self.cx.capture_screenshot(self.window)?;
        let path = self.out.join(format!("{name}.png"));
        image.save(&path)?;
        writeln!(self.log, "  screenshot {}", path.display()).ok();
        Ok(())
    }

    fn text(&mut self) -> String {
        let composer = self.composer.clone();
        self.cx.update(|cx| composer.read(cx).editor().read(cx).text().to_owned())
    }

    fn check(&mut self, name: &str, ok: bool, detail: impl std::fmt::Display) {
        writeln!(self.log, "{} {name}: {detail}", if ok { "PASS" } else { "FAIL" }).ok();
    }

    fn set_text(&mut self, text: &str, cursor: Option<usize>) {
        let composer = self.composer.clone();
        let text = text.to_owned();
        self.cx.update(|cx| {
            let editor = composer.read(cx).editor().clone();
            editor.update(cx, |e, cx| {
                e.set_text(text, cx);
                if let Some(c) = cursor {
                    e.set_cursor(c, cx);
                }
            });
        });
        self.frame();
    }
}

fn stats(samples: &mut [f64]) -> (f64, f64, f64) {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |q: f64| samples[((samples.len() as f64 - 1.) * q).round() as usize];
    (at(0.5), at(0.95), *samples.last().unwrap_or(&0.))
}

/// Times `count` keystrokes from dispatch to a drawn frame.
fn measure(h: &mut Harness, count: usize) -> Vec<f64> {
    let chars: Vec<char> = "the quick brown fox jumps over the lazy dog ".chars().collect();
    let mut samples = Vec::with_capacity(count);
    for i in 0..count {
        let c = chars[i % chars.len()];
        let key = if c == ' ' { "space".to_owned() } else { c.to_string() };
        // The headless app draws the dirty window when the keystroke's update
        // flushes, as the platform's next frame would: this is input to drawn frame.
        let start = Instant::now();
        h.key(&key);
        samples.push(start.elapsed().as_secs_f64() * 1000.);
        h.cx.run_until_parked();
    }
    samples
}

fn typing_report(out: &std::path::Path) -> anyhow::Result<String> {
    let mut h = Harness::new(Appearance::Dark, out)?;
    let draft = long_draft();
    let caret = draft.find("Closing paragraph").unwrap_or(draft.len() / 2) + 8;
    h.set_text(&draft, Some(caret));
    // Warm caches (fonts, glyph atlas, highlighter) before timing.
    measure(&mut h, 30);
    let mut report = String::new();
    let chars = h.text().chars().count();
    writeln!(report, "draft: {chars} chars, chips and a rust code block, caret mid-paragraph")?;
    writeln!(report, "time per keystroke: dispatch_keystroke through the frame drawn when its update flushes (CPU layout, prepaint, paint; no GPU raster), ms")?;
    let mut plain = measure(&mut h, 500);
    let (m, p95, max) = stats(&mut plain);
    writeln!(report, "composer focused:        median {m:.3}  p95 {p95:.3}  max {max:.3}  (500 keys)")?;
    h.shot("bench-typing-dark")?;

    // Picker open, focus back in the editor: every keystroke also lays out the open popover.
    h.with(|composer, window, cx| composer.update(cx, |c, cx| c.open_picker(window, cx)));
    h.frame();
    h.with(|composer, window, cx| {
        let focus = composer.read(cx).editor().focus_handle(cx);
        window.focus(&focus, cx);
    });
    h.frame();
    measure(&mut h, 20);
    let mut open = measure(&mut h, 500);
    let (m, p95, max) = stats(&mut open);
    writeln!(report, "model picker open:       median {m:.3}  p95 {p95:.3}  max {max:.3}  (500 keys)")?;
    // Typing into the picker's own search.
    h.with(|composer, window, cx| {
        if let Some(picker) = composer.read(cx).picker() {
            let focus = gpui_kit::Focusable::focus_handle(picker.read(cx), cx);
            window.focus(&focus, cx);
        }
    });
    h.frame();
    let mut search = Vec::new();
    for c in "claude opus".chars().cycle().take(40) {
        let key = if c == ' ' { "space".to_owned() } else { c.to_string() };
        let start = Instant::now();
        h.key(&key);
        search.push(start.elapsed().as_secs_f64() * 1000.);
        h.cx.run_until_parked();
        if search.len() % 11 == 0 {
            for _ in 0..11 {
                h.press("backspace");
            }
        }
    }
    let (m, p95, max) = stats(&mut search);
    writeln!(report, "picker search field:     median {m:.3}  p95 {p95:.3}  max {max:.3}  (40 keys)")?;
    writeln!(report, "frame budget at 120 Hz: 8.333 ms")?;
    let mut raw = String::new();
    for (name, s) in [("composer", &plain), ("picker_open", &open)] {
        writeln!(raw, "{name}: {}", s.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(" "))?;
    }
    std::fs::write(out.join("typing-samples-sorted.txt"), raw)?;
    Ok(report)
}

fn scripted(appearance: Appearance, suffix: &str, out: &std::path::Path) -> anyhow::Result<String> {
    let mut h = Harness::new(appearance, out)?;
    writeln!(h.log, "== {suffix}")?;

    // Typing and Enter-to-send.
    h.type_text("Hello composer");
    h.shot(&format!("01-typed-{suffix}"))?;
    let typed = h.text();
    h.check("typing inserts text", typed == "Hello composer", format!("{typed:?}"));
    h.press("shift-enter");
    h.type_text("second line");
    let multi = h.text();
    h.check("shift+enter adds a newline", multi == "Hello composer\nsecond line", format!("{multi:?}"));

    // Keyboard selection.
    let word = if cfg!(target_os = "macos") { "alt-shift-left" } else { "ctrl-shift-left" };
    h.press(word);
    h.press(word);
    h.shot(&format!("02-selection-{suffix}"))?;
    let sel = h.cx.update(|cx| h.composer.read(cx).editor().read(cx).selection());
    h.check("word selection by keyboard", sel.len() == "second line".len(), format!("{sel:?}"));

    // Undo groups typing.
    h.press("right");
    let undo = if cfg!(target_os = "macos") { "cmd-z" } else { "ctrl-z" };
    h.press(undo);
    let after_undo = h.text();
    h.check("undo removes the typed run", after_undo.len() < multi.len(), format!("{after_undo:?}"));

    // Enter sends.
    h.set_text("Ship it", None);
    h.events.borrow_mut().clear();
    h.press("enter");
    let sent = h.events.borrow().iter().any(|e| matches!(e, ComposerEvent::Submit { prompt, .. } if prompt == "Ship it"));
    let cleared = h.text().is_empty();
    h.check("enter sends and clears", sent && cleared, format!("submitted={sent} cleared={cleared}"));

    // IME: Enter while composing never sends.
    h.events.borrow_mut().clear();
    h.with(|composer, window, cx| {
        let editor = composer.read(cx).editor().clone();
        editor.update(cx, |e, cx| gpui_kit::EntityInputHandler::replace_and_mark_text_in_range(e, None, "にほ", None, window, cx));
    });
    h.frame();
    h.shot(&format!("03-ime-marked-{suffix}"))?;
    h.press("enter");
    let ime_sent = h.events.borrow().iter().any(|e| matches!(e, ComposerEvent::Submit { .. }));
    let ime_text = h.text();
    h.check("enter during IME composition does not send", !ime_sent, format!("text {ime_text:?}"));
    h.with(|composer, window, cx| {
        let editor = composer.read(cx).editor().clone();
        editor.update(cx, |e, cx| gpui_kit::EntityInputHandler::unmark_text(e, window, cx));
    });

    // Chip insertion through the @ menu, with keyboard navigation.
    h.set_text("Review ", None);
    h.type_text("@comp");
    let open = h.cx.update(|cx| h.composer.read(cx).menu().map(|m| m.items.len()));
    h.check("@ opens the mention menu", open.is_some_and(|n| n > 0), format!("{open:?} items"));
    h.press("down");
    h.press("down");
    h.shot(&format!("04-menu-navigated-{suffix}"))?;
    let active = h.cx.update(|cx| h.composer.read(cx).menu().map(|m| m.active));
    h.check("arrow keys move the menu highlight", active == Some(2), format!("{active:?}"));
    h.press("up");
    h.press("up");
    h.press("enter");
    let chip = h.text();
    h.check("enter picks a file chip", chip.starts_with("Review [composer.rs](apps/web/src/components/chat/composer.rs) "), format!("{chip:?}"));
    h.shot(&format!("05-chip-inserted-{suffix}"))?;
    // The chip is one caret stop.
    let before = h.cx.update(|cx| h.composer.read(cx).editor().read(cx).cursor());
    h.press("left");
    h.press("left");
    let after = h.cx.update(|cx| h.composer.read(cx).editor().read(cx).cursor());
    h.check("two lefts cross the space and the whole chip", after == "Review ".len(), format!("{before} -> {after}"));

    // Double click selects a word.
    let composer = h.composer.clone();
    let pos = h.cx.update(|cx| composer.read(cx).editor().read(cx).window_point_for(1));
    if let Some(p) = pos {
        for count in [1, 2] {
            h.cx.update_window(h.window, |_, window, cx| {
                window.dispatch_event(MouseDownEvent { button: MouseButton::Left, position: p, modifiers: Modifiers::default(), click_count: count, first_mouse: false }.to_platform_input(), cx);
                window.dispatch_event(MouseUpEvent { button: MouseButton::Left, position: p, modifiers: Modifiers::default(), click_count: count }.to_platform_input(), cx);
            }).ok();
            h.frame();
        }
        let sel = h.cx.update(|cx| h.composer.read(cx).editor().read(cx).selection());
        h.check("double click selects a word", sel == (0..6), format!("{sel:?}"));
        h.shot(&format!("06-double-click-{suffix}"))?;
    }

    // Lists continue on Shift+Enter, code fences open on Enter.
    h.set_text("- first", None);
    h.press("shift-enter");
    h.type_text("second");
    let list = h.text();
    h.check("shift+enter continues a list", list == "- first\n- second", format!("{list:?}"));
    h.set_text("```rust", None);
    h.press("enter");
    h.type_text("let x = 1;");
    h.press("enter");
    let code = h.text();
    h.check("enter opens a fence and stays in code", code == "```rust\nlet x = 1;\n\n```", format!("{code:?}"));
    h.shot(&format!("07-code-block-{suffix}"))?;

    // Steer versus queue while running.
    let composer = h.composer.clone();
    h.cx.update(|cx| {
        composer.update(cx, |c, cx| {
            let mut state = t3code_web::scenes::composer::fixture_state("bench");
            state.running = true;
            state.can_interrupt = true;
            c.set_state(state, cx);
        })
    });
    h.set_text("", None);
    h.shot(&format!("08-running-stop-{suffix}"))?;
    h.set_text("Also check the light theme", None);
    let steer = h.cx.update(|cx| composer.read(cx).running_dispatch());
    h.cx.update(|cx| composer.update(cx, |c, cx| c.set_modifier_held(true, cx)));
    h.shot(&format!("09-queue-while-mod-held-{suffix}"))?;
    let queue = h.cx.update(|cx| composer.read(cx).running_dispatch());
    h.check("holding Cmd/Ctrl flips steer to queue", format!("{steer:?}") == "Steer" && format!("{queue:?}") == "Queue", format!("{steer:?} -> {queue:?}"));
    h.cx.update(|cx| composer.update(cx, |c, cx| c.set_modifier_held(false, cx)));

    // Model picker: search and provider switching.
    h.with(|composer, window, cx| composer.update(cx, |c, cx| c.open_picker(window, cx)));
    h.frame();
    h.type_text("gpt");
    h.shot(&format!("10-picker-search-{suffix}"))?;
    h.press("escape");
    let closed = h.cx.update(|cx| composer.read(cx).picker().is_none());
    h.check("escape closes the picker", closed, "");

    // Prompt history: ArrowUp in an empty composer recalls.
    h.cx.update(|cx| composer.update(cx, |c, cx| c.set_state(t3code_web::scenes::composer::fixture_state("bench"), cx)));
    h.set_text("", None);
    h.press("up");
    let recalled = h.text();
    h.check("arrow up recalls the last prompt", recalled == "Run the composer scene again", format!("{recalled:?}"));
    Ok(h.log)
}

fn main() -> anyhow::Result<()> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).context("usage: composer-bench <out-dir>")?);
    std::fs::create_dir_all(&out)?;
    let typing = typing_report(&out)?;
    print!("{typing}");
    if std::env::var("BENCH_TYPING_ONLY").is_ok() { return Ok(()); }
    std::fs::write(out.join("typing.txt"), &typing)?;
    let mut checks = scripted(Appearance::Dark, "dark", &out)?;
    checks.push_str(&scripted(Appearance::Light, "light", &out)?);
    print!("{checks}");
    std::fs::write(out.join("checks.txt"), checks)?;
    Ok(())
}
