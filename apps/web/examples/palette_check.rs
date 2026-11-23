//! Drives keyboard control end to end in a headless window: the palette
//! shortcut, typing, arrows, running a command, a four-key chord, a `when`
//! clause that follows focus, a user rule overriding a default, a removal
//! rule, modifier-hold hints, navigation history, the update toast, and
//! keystroke-to-frame timing over 5,000 threads. Every keystroke and the
//! command GPUI dispatched for it goes to `transcript.txt`, next to what the
//! reference resolver (`resolveShortcutCommand`) says it should be.
//!
//!   cargo run -p t3code-web --features test-support --example palette_check -- <out dir>

use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui_kit::component::input::InputState;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Capslock, Context, Entity, FocusHandle, HeadlessAppContext, InteractiveElement as _,
    IntoElement, Keystroke, Modifiers, ModifiersChangedEvent, ParentElement as _, PlatformInput, Render, Styled as _,
    Subscription, Window, WindowBounds, WindowOptions, div, px, size, test::TestWindowExt as _,
};
use t3code_shared::keybindings::{
    KeybindingCommand, KeybindingRule, ModifierKeys, ServerConfigIssue, ShortcutPlatform, StaticCommand,
    compile_resolved_keybindings_config, default_resolved_keybindings, merge_with_default_keybindings, report_keybindings,
};
use t3code_web::command_palette_store::OpenIntent;
use t3code_web::components::command_palette::CommandPalette;
use t3code_web::components::command_palette_logic::PaletteEvent;
use t3code_web::components::command_palette_lookups::FixtureSource;
use t3code_web::components::keybindings_update_toast::{KeybindingsUpdateToastController, show_keybindings_update_toast};
use t3code_web::components::sidebar::fixtures;
use t3code_web::components::ui::input::TextInput;
use t3code_web::keybindings::{KeyPress, resolve_shortcut_command};
use t3code_web::navigation_history::{HistoryDirection, NavigationHistory};
use t3code_web::shortcut_modifier_state::ShortcutModifierStore;
use t3code_web::shortcuts::commands::{command_for_action, command_title, on_commands};
use t3code_web::shortcuts::keymap::{self, ShortcutState, current_context};
use t3code_web::shortcuts::ShortcutRoot;
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

type Log = Rc<RefCell<Vec<String>>>;

/// Commands nothing in this window handles, so the palette lists them as unavailable.
const UNHANDLED: [StaticCommand; 3] = [StaticCommand::ThemeEditorToggle, StaticCommand::UsageOpen, StaticCommand::PreviewFind];
/// Commands the shortcut root answers; the workspace leaves them alone.
const ROOT_COMMANDS: [StaticCommand; 3] =
    [StaticCommand::CommandPaletteToggle, StaticCommand::ThemeSelect, StaticCommand::AppearanceCycle];

/// A stand-in for the app: a pane, a terminal and a text field. It handles
/// every command except [`UNHANDLED`] and logs what reached it.
struct Workspace {
    pane: FocusHandle,
    terminal: FocusHandle,
    input: Entity<InputState>,
    input_focus: FocusHandle,
    handled: Log,
    history: Rc<RefCell<NavigationHistory<String>>>,
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let handled = self.handled.clone();
        let history = self.history.clone();
        let listener: Rc<dyn Fn(StaticCommand, &mut Window, &mut gpui_kit::App)> = Rc::new(move |command, _, _| {
            match command {
                StaticCommand::NavigationBack => {
                    let at = history.borrow_mut().navigate(HistoryDirection::Back).cloned();
                    handled.borrow_mut().push(format!("{} -> {at:?}", command.id()));
                }
                StaticCommand::NavigationForward => {
                    let at = history.borrow_mut().navigate(HistoryDirection::Forward).cloned();
                    handled.borrow_mut().push(format!("{} -> {at:?}", command.id()));
                }
                _ => handled.borrow_mut().push(command.id().to_owned()),
            }
        });
        let pane = div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.pane)
            .size_full()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_color(gpui_kit::white())
            .child("workspace pane")
            .child(div().id("terminal").key_context(keymap::TERMINAL_CONTEXT).track_focus(&self.terminal).h(px(60.)).child("terminal"))
            .child(div().w(px(300.)).child(TextInput::new(&self.input)));
        on_commands(pane, |c| !UNHANDLED.contains(&c) && !ROOT_COMMANDS.contains(&c), listener)
    }
}

struct Harness {
    cx: HeadlessAppContext,
    handle: AnyWindowHandle,
    root: Entity<ShortcutRoot>,
    workspace: Entity<Workspace>,
    palette: Entity<CommandPalette>,
    handled: Log,
    events: Log,
    transcript: String,
    mismatches: usize,
    out: PathBuf,
    _subscriptions: Vec<Subscription>,
}

fn press_from_keystroke(keystroke: &Keystroke) -> KeyPress {
    let key = match keystroke.key.as_str() {
        "up" => "arrowup",
        "down" => "arrowdown",
        "left" => "arrowleft",
        "right" => "arrowright",
        "space" => " ",
        other => other,
    };
    let m = keystroke.modifiers;
    KeyPress { key: key.to_owned(), modifiers: ModifierKeys { meta: m.platform, ctrl: m.control, shift: m.shift, alt: m.alt }, repeat: false }
}

impl Harness {
    fn open(out: &Path) -> anyhow::Result<Self> {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(t3code_web::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(|cx| {
            t3code_web::init(cx);
            ThemeSettings::set_mode(ThemeMode::Dark, cx);
            // The check presses Ctrl chords; pin `mod` to Ctrl on every host.
            keymap::set_platform(ShortcutPlatform::Other, cx);
        });
        let snapshot = fixtures::large_snapshot(5_000, 40);
        let source = Rc::new(FixtureSource::new(&snapshot));
        let handled: Log = Rc::default();
        let history = Rc::new(RefCell::new(NavigationHistory::new("thread:a".to_owned(), 50)));
        history.borrow_mut().push("thread:b".into());
        history.borrow_mut().push("settings".into());
        let mut parts = None;
        let (handle, _) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(1100.), px(780.)), cx)),
                show: false,
                focus: true,
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
                let input_focus = gpui_kit::Focusable::focus_handle(input.read(cx), cx);
                let workspace = cx.new(|cx| Workspace {
                    pane: cx.focus_handle(),
                    terminal: cx.focus_handle(),
                    input,
                    input_focus,
                    handled: handled.clone(),
                    history: history.clone(),
                });
                let root = cx.new(|cx| ShortcutRoot::new(workspace.clone().into(), source, window, cx));
                parts = Some((root.clone(), workspace));
                root
            })
        })?;
        let (root, workspace) = parts.context("window built")?;
        let palette = cx.update(|cx| root.read(cx).palette().clone());
        let events: Log = Rc::default();
        let subscriptions = cx.update(|cx| {
            let events = events.clone();
            vec![cx.subscribe(&palette, move |_, event: &PaletteEvent, _| events.borrow_mut().push(format!("{event:?}")))]
        });
        let mut harness = Self {
            cx,
            handle,
            root,
            workspace,
            palette,
            handled,
            events,
            transcript: String::new(),
            mismatches: 0,
            out: out.to_owned(),
            _subscriptions: subscriptions,
        };
        harness.frame();
        harness.focus_pane();
        Ok(harness)
    }

    fn frame(&mut self) {
        self.cx.run_until_parked();
        self.cx.update_window(self.handle, |_, window, cx| window.render_frame(cx)).ok();
    }

    fn focus(&mut self, pick: impl Fn(&Workspace) -> FocusHandle) {
        let focus = self.cx.update(|cx| pick(self.workspace.read(cx)));
        self.cx.update_window(self.handle, |_, window, cx| window.focus(&focus, cx)).ok();
        self.frame();
    }

    fn focus_pane(&mut self) {
        self.focus(|w| w.pane.clone());
    }

    fn note(&mut self, line: impl AsRef<str>) {
        println!("{}", line.as_ref());
        let _ = writeln!(self.transcript, "{}", line.as_ref());
    }

    /// Presses a GPUI key and records what ran and what the reference resolver expects.
    fn press(&mut self, key: &str) -> Option<KeybindingCommand> {
        let keystroke = Keystroke::parse(Box::leak(key.to_owned().into_boxed_str())).expect("valid keystroke");
        let (expected, context) = self.cx.update_window(self.handle, |_, window, cx| {
            let context = current_context(window, cx);
            let state = ShortcutState::get(cx);
            let expected = resolve_shortcut_command(&press_from_keystroke(&keystroke), &state.config, context, state.platform).cloned();
            (expected, context)
        }).unwrap();
        let before_handled = self.handled.borrow().len();
        let ran: Rc<RefCell<Option<KeybindingCommand>>> = Rc::default();
        let observer = {
            let ran = ran.clone();
            self.cx.update(|cx| {
                cx.observe_keystrokes(move |event, _, _| {
                    if let Some(action) = &event.action {
                        *ran.borrow_mut() = command_for_action(action.as_ref());
                    }
                })
            })
        };
        self.cx.update_window(self.handle, |_, window, cx| window.press(key, cx)).ok();
        self.frame();
        drop(observer);
        let dispatched = ran.borrow().clone();
        let reached = self.handled.borrow()[before_handled..].join(", ");
        let agrees = dispatched == expected;
        if !agrees {
            self.mismatches += 1;
        }
        let name = |c: &Option<KeybindingCommand>| c.as_ref().map_or("-".to_owned(), |c| c.to_string());
        self.note(format!(
            "key {key:<18} context {context:?}\n    dispatched {:<28} reference {:<28} {}{}",
            name(&dispatched),
            name(&expected),
            if agrees { "ok" } else { "MISMATCH" },
            if reached.is_empty() { String::new() } else { format!("  handled: {reached}") },
        ));
        dispatched
    }

    fn type_text(&mut self, text: &str) {
        self.cx.update_window(self.handle, |_, window, cx| window.input(text, cx)).ok();
        self.frame();
        self.note(format!("typed {text:?}"));
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame();
        let path = self.out.join(name);
        self.cx.capture_screenshot(self.handle)?.save(&path)?;
        self.note(format!("screenshot {}", path.display()));
        Ok(())
    }

    fn palette<R>(&mut self, read: impl FnOnce(&CommandPalette) -> R) -> R {
        self.cx.update(|cx| read(self.palette.read(cx)))
    }

    fn install(&mut self, custom: &[KeybindingRule]) {
        let merged = merge_with_default_keybindings(&compile_resolved_keybindings_config(custom));
        self.cx.update(|cx| keymap::install(merged, Vec::new(), cx));
        self.frame();
    }

    /// Types `query` one key at a time; each sample is dispatch to finished frame.
    fn time_typing(&mut self, query: &str, samples: &mut Vec<Duration>) {
        for ch in query.chars() {
            let mut keystroke = Keystroke::parse(if ch == ' ' { "space" } else { Box::leak(ch.to_string().into_boxed_str()) }).unwrap();
            keystroke.key_char = Some(ch.to_string());
            let started = Instant::now();
            self.cx
                .update_window(self.handle, |_, window, cx| {
                    window.dispatch_keystroke(keystroke, cx);
                    window.render_frame(cx);
                })
                .ok();
            samples.push(started.elapsed());
        }
        for _ in 0..query.chars().count() {
            let started = Instant::now();
            self.cx
                .update_window(self.handle, |_, window, cx| {
                    window.dispatch_keystroke(Keystroke::parse("backspace").unwrap(), cx);
                    window.render_frame(cx);
                })
                .ok();
            samples.push(started.elapsed());
        }
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).context("usage: palette_check <out dir>")?);
    std::fs::create_dir_all(&out)?;
    let mut h = Harness::open(&out)?;
    let total_threads = 5_000;
    h.note(format!("== keymap: {} default rules compiled to GPUI bindings", default_resolved_keybindings().rules().len()));

    h.note("== open the palette with its shortcut, from the workspace pane");
    h.press("ctrl-k");
    if !h.palette(|p| p.is_open()) {
        bail!("ctrl-k did not open the palette");
    }
    h.shot("01-open-root.png")?;

    h.note("== type, then move with the arrows");
    h.type_text("toggle");
    let titles = h.palette(|p| p.shown_titles());
    h.note(format!("results for \"toggle\": {titles:?}"));
    h.shot("02-typed-toggle.png")?;
    h.press("down");
    h.press("down");
    let target = h.palette(|p| p.target_value());
    h.note(format!("highlighted after down, down: {target:?}"));
    h.shot("03-arrowed.png")?;

    h.note("== Enter on a page row opens the page; Backspace on an empty query goes back");
    h.press("enter");
    let titles = h.palette(|p| p.shown_titles());
    h.note(format!("page rows: {titles:?}"));
    h.press("backspace");
    let query = h.palette(|p| p.query());
    h.note(format!("back on the root page, query {query:?}"));
    h.press("escape");

    h.note("== run a command row with Enter; its action reaches the workspace");
    h.press("ctrl-k");
    h.type_text("focus next");
    h.press("down");
    let before = h.handled.borrow().len();
    h.press("enter");
    let ran = h.handled.borrow()[before..].to_vec();
    let open = h.palette(|p| p.is_open());
    h.note(format!("palette open: {open}, workspace handled: {ran:?}"));
    if ran.is_empty() {
        bail!("Enter ran nothing");
    }

    h.note("== unavailable commands: no handler on the opener's path");
    h.press("ctrl-k");
    h.type_text("theme editor");
    h.shot("04-unavailable.png")?;
    h.press("escape");

    h.note("== a thread row emits OpenThread with the query that found it");
    h.press("ctrl-k");
    h.type_text("relay auth #42");
    h.press("enter");
    h.note(format!("palette events: {:?}", h.events.borrow()));

    h.note("== nested page: New thread in... then a jump shortcut inside it");
    h.cx.update_window(h.handle, |_, window, cx| h.palette.update(cx, |p, cx| p.open(OpenIntent::NewThreadIn, window, cx))).ok();
    h.frame();
    h.shot("05-nested.png")?;
    h.press("ctrl-3");
    h.note(format!("palette events: {:?}", h.events.borrow().last()));

    h.note("== a four-key chord: mod+alt+shift+a cycles the appearance");
    let mode_before = h.cx.update(|cx| cx.global::<ThemeSettings>().mode);
    h.press("ctrl-alt-shift-a");
    let mode_after = h.cx.update(|cx| cx.global::<ThemeSettings>().mode);
    h.note(format!("appearance {mode_before:?} -> {mode_after:?}"));
    h.cx.update(|cx| ThemeSettings::set_mode(ThemeMode::Dark, cx));
    h.frame();

    h.note("== a when clause that follows focus: mod+d splits a focused terminal, else toggles the diff");
    h.focus(|w| w.terminal.clone());
    h.press("ctrl-d");
    h.focus_pane();
    h.press("ctrl-d");
    h.note("   mod+k is !terminalFocus: nothing from the terminal");
    h.focus(|w| w.terminal.clone());
    h.press("ctrl-k");
    let open = h.palette(|p| p.is_open());
    h.note(format!("   palette open: {open}"));
    h.note("   mod+z is !editableFocus: nothing from the text field, undo from the pane");
    h.focus(|w| w.input_focus.clone());
    h.press("ctrl-z");
    h.focus_pane();
    h.press("ctrl-z");

    h.note("== navigation history through navigation.back / navigation.forward");
    h.press("ctrl-[");
    h.press("ctrl-[");
    h.press("ctrl-]");

    h.note("== a user rule overriding a default: mod+g toggles the terminal, mod+j no longer does");
    h.install(&[KeybindingRule::new("mod+g", StaticCommand::TerminalToggle, None)]);
    h.press("ctrl-g");
    h.press("ctrl-j");

    h.note("== a removal rule: sidebar.toggle on mod+b with when \"false\"");
    h.press("ctrl-b");
    h.install(&[KeybindingRule::new("mod+b", StaticCommand::SidebarToggle, Some("false"))]);
    h.press("ctrl-b");

    h.note("== a later rule shadowing an earlier one on the same chord");
    h.install(&[KeybindingRule::new("mod+d", StaticCommand::ThreadPin, Some("!terminalFocus"))]);
    h.press("ctrl-d");
    h.focus(|w| w.terminal.clone());
    h.press("ctrl-d");
    h.focus_pane();

    h.note("== invalid and conflicting rules");
    let custom = vec![
        KeybindingRule::new("mod+shift+d+o", StaticCommand::TerminalNew, None),
        KeybindingRule::new("mod+k", StaticCommand::ThreadSettle, Some("!(terminalFocus")),
        KeybindingRule::new("mod+u", StaticCommand::ThreadSettle, None),
    ];
    let merged = merge_with_default_keybindings(&compile_resolved_keybindings_config(&custom));
    let report = report_keybindings(&custom, &merged, ShortcutPlatform::Other);
    h.note(format!("invalid: {:?}", report.invalid));
    for conflict in &report.conflicts {
        let (a, b) = (&merged.rules()[conflict.shadowed], &merged.rules()[conflict.winner]);
        h.note(format!("conflict: {} ({}) loses to {} ({})", a.command, command_title(&a.command), b.command, command_title(&b.command)));
    }
    h.install(&[]);

    h.note("== the update toast for a bad keybindings.json, shown once for a repeated event");
    let issues = vec![ServerConfigIssue::InvalidEntry { index: 0, message: "Invalid shortcut \"mod+shift+d+o\".".into() }];
    let shown = h.cx.update(|cx| {
        let mut controller = KeybindingsUpdateToastController::default();
        let first = show_keybindings_update_toast(&mut controller, &issues, 10_000, Some("keybindings.json"), Some(Rc::new(|_, _, _| {})), cx);
        let repeat = show_keybindings_update_toast(&mut controller, &issues, 11_000, Some("keybindings.json"), None, cx);
        (first.is_some(), repeat.is_some())
    });
    h.note(format!("toast shown: {}, repeat within 2s shown: {}", shown.0, shown.1));
    h.shot("06-toast.png")?;

    h.note("== modifier hold: Ctrl shows thread jump hints, release hides them");
    let hints = |h: &mut Harness| h.cx.update(|cx| ShortcutModifierStore::global(cx).read(cx).show_thread_jump_hints());
    let modifiers = |control| PlatformInput::ModifiersChanged(ModifiersChangedEvent { modifiers: Modifiers { control, ..Default::default() }, capslock: Capslock::default() });
    h.cx.update_window(h.handle, |_, window, cx| { window.dispatch_event(modifiers(true), cx); }).ok();
    let held = hints(&mut h);
    h.cx.update_window(h.handle, |_, window, cx| { window.dispatch_event(modifiers(false), cx); }).ok();
    let released = hints(&mut h);
    h.focus(|w| w.terminal.clone());
    h.cx.update_window(h.handle, |_, window, cx| { window.dispatch_event(modifiers(true), cx); }).ok();
    let over_terminal = hints(&mut h);
    h.cx.update_window(h.handle, |_, window, cx| { window.dispatch_event(modifiers(false), cx); }).ok();
    h.focus_pane();
    h.note(format!("jump hints: held {held}, released {released}, held over terminal {over_terminal}"));

    h.note(format!("== timing: keystroke to finished frame, {total_threads} threads"));
    h.press("ctrl-k");
    let mut samples = Vec::new();
    for query in ["fix sidebar scroll", "relay auth", "port theme tokens", "investigate composer paste", "t3"] {
        h.time_typing(query, &mut samples);
    }
    h.press("escape");
    samples.sort();
    let median = percentile(&samples, 0.5);
    let p95 = percentile(&samples, 0.95);
    let max = *samples.last().unwrap();
    let timing = format!(
        "samples {} (typing and deleting), median {:.2} ms, p95 {:.2} ms, max {:.2} ms, build {}",
        samples.len(),
        median.as_secs_f64() * 1e3,
        p95.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3,
        if cfg!(debug_assertions) { "debug" } else { "release" },
    );
    h.note(&timing);
    std::fs::write(out.join(if cfg!(debug_assertions) { "timing-debug.txt" } else { "timing-release.txt" }), format!("{timing}\n"))?;

    h.note(format!("== reference mismatches: {}", h.mismatches));
    let _ = h.root.clone();
    std::fs::write(out.join("transcript.txt"), &h.transcript)?;
    if h.mismatches > 0 {
        bail!("{} keystrokes resolved differently from the reference", h.mismatches);
    }
    Ok(())
}
