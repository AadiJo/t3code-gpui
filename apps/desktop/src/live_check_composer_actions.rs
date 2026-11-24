//! `live-check composer-actions`: the composer's own shortcuts (`composer.*`) driven through real
//! key events on a seeded server: effort and mode open the model picker where the fork does,
//! effort steps, the workspace and branch shortcuts open the strip's menus on a draft, dictation
//! with no transcription destination leaves the key alone, a repeated action is ignored until the
//! key comes up, and send-and-new-thread sends then opens a new draft. Run `seed` first.

use std::time::Duration;

use t3code_web::router::Route;
use t3code_web::shortcuts::commands as k;

use super::{Check, composer_of, load_state, thread_with_turn};

fn picker_state(check: &mut Check) -> Option<(Option<usize>, bool)> {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).picker().map(|p| (p.read(cx).effort_focus(), p.read(cx).permissions_open())))
}

fn effort(check: &mut Check) -> Option<String> {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).state().selection.effort.as_ref().map(|e| e.to_string()))
}

fn strip_menu(check: &mut Check) -> Option<&'static str> {
    check.read(|root, cx| root.thread_view().read(cx).branch_strip().and_then(|s| s.read(cx).open_menu_name()))
}

fn prompt(check: &mut Check) -> String {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).editor().read(cx).text().to_owned())
}

fn close_strip(check: &mut Check) {
    check.press("escape");
    let strip = check.read(|root, cx| root.thread_view().read(cx).branch_strip().cloned());
    if let Some(strip) = strip {
        check.cx.update(|cx| strip.update(cx, |s, cx| s.open_menu("", cx)));
    }
    check.frame();
}

pub(super) fn run_composer_actions(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    page_keys(check, &state)?;
    let key = thread_with_turn(check, &state, "Composer actions")?;
    check.go(&key);
    check.pump(|c| c.shown_input().entries.len() >= 2, Duration::from_secs(20), "thread shown")?;
    // The Claude instance's models have effort levels; Replay's have none.
    let picked = super::pick_instance(check, "claudeAgent");
    check.check("the composer can pick the Claude instance", picked, picked);
    let levels = {
        let composer = composer_of(check);
        check.cx.update(|cx| {
            let state = composer.read(cx).state();
            state.catalog.model(&state.selection.instance_id, &state.selection.model).and_then(|m| m.effort.as_ref()).map_or(0, |d| d.options.len())
        })
    };
    check.note(format!("selected model has {levels} effort levels"));
    check.focus_composer();

    // composer.effort: the picker opens with the keyboard on the selected level.
    check.press("mod-shift-e");
    let opened = picker_state(check);
    check.check("mod+shift+e opens the model picker on the effort column", matches!(opened, Some((Some(_), false))), format!("{opened:?}"));
    check.shot("a0-effort-open.png")?;
    let before = effort(check);
    let start = opened.and_then(|o| o.0);
    check.press("down");
    let moved = picker_state(check).and_then(|p| p.0);
    check.check("Down moves to the next level", moved.is_some() && moved != start, format!("{start:?} -> {moved:?}"));
    check.press("enter");
    let picked = effort(check);
    check.check("Enter picks that level", picked.is_some() && picked != before, format!("{before:?} -> {picked:?}"));
    check.press("escape");
    let closed = picker_state(check).is_none();
    check.check("Escape closes the picker", closed, "");

    // composer.increaseEffort / decreaseEffort have no default chord: dispatch on the focused composer.
    check.focus_composer();
    let before = effort(check);
    check.action(k::IncreaseEffort);
    let up = effort(check);
    check.check("increaseEffort steps the level", up.is_some() && up != before, format!("{before:?} -> {up:?}"));
    check.cx.update_window(check.window, |_, w, cx| w.dispatch_action(Box::new(k::IncreaseEffort), cx)).ok();
    check.frame();
    let repeat = effort(check);
    check.check("a repeat of the same press is ignored until a key comes up", repeat == up, format!("{repeat:?}"));
    check.press("escape");
    check.action(k::DecreaseEffort);
    let down = effort(check);
    check.check("decreaseEffort steps back", down == before, format!("{up:?} -> {down:?}"));

    // composer.mode with the default mode: no chip, so the picker answers with its permissions menu.
    check.press("mod-shift-a");
    let mode = picker_state(check);
    check.check("mod+shift+a opens the picker's permissions menu (no chip)", matches!(mode, Some((_, true))), format!("{mode:?}"));
    check.shot("a1-mode-open.png")?;
    check.press("escape");
    check.press("escape");

    // composer.dictation with no destination: the key falls through and nothing starts.
    check.focus_composer();
    check.type_text("dictation probe");
    check.press("mod-shift-d");
    let text = prompt(check);
    check.check("mod+shift+d with no transcription destination changes nothing", text == "dictation probe", text);

    // composer.host on a server thread with one machine: no picker to open.
    check.press("mod-shift-h");
    let host = strip_menu(check);
    check.check("mod+shift+h with one machine opens nothing", host.is_none(), format!("{host:?}"));

    // composer.sendAndNewThread: the message goes, then a new draft opens in the project.
    check.press("mod-alt-enter");
    check.pump(|c| matches!(c.route(), Route::Draft(_)), Duration::from_secs(10), "a new draft opened")?;
    let route = check.route();
    check.check("mod+alt+enter on a thread opens a new draft", matches!(route, Route::Draft(_)), format!("{route:?}"));
    let k2 = key.clone();
    check.pump(
        move |c| super::projection_of(c, &k2).is_some_and(|p| p.runs.len() >= 2),
        Duration::from_secs(30),
        "the message went to the thread",
    )?;
    check.check("and the message was sent to the thread", true, "");

    // On the draft the strip's workspace and branch menus open from the keyboard.
    check.pump(|c| strip_ready(c), Duration::from_secs(20), "draft strip refs loaded")?;
    check.focus_composer();
    check.press("mod-shift-x");
    let workspace = strip_menu(check);
    check.check("mod+shift+x opens the workspace menu", matches!(workspace, Some("workspace" | "run-context")), format!("{workspace:?}"));
    check.shot("a2-workspace-open.png")?;
    close_strip(check);
    check.focus_composer();
    check.press("mod-shift-g");
    let branch = strip_menu(check);
    check.check("mod+shift+g opens the branch picker", branch == Some("branch"), format!("{branch:?}"));
    check.shot("a3-branch-open.png")?;
    close_strip(check);
    Ok(())
}

fn strip_ready(check: &mut Check) -> bool {
    check.read(|root, cx| root.thread_view().read(cx).branch_strip().is_some_and(|s| s.read(cx).props().show_git_controls))
}

/// The timeline's scroll top and the page the composer scrolls (viewport less 36px and its inset).
fn timeline_metrics(check: &mut Check) -> (f32, f32) {
    check.read(|root, cx| {
        let timeline = root.thread_view().read(cx).chat().read(cx).timeline().clone();
        let timeline = timeline.read(cx);
        let (bounds, _, top, _) = timeline.list_state().viewport_metrics();
        (f32::from(top), f32::from(bounds.size.height - gpui_kit::px(36.) - timeline.composer_bottom()))
    })
}

/// Page Up / Page Down in an empty composer page the long thread's timeline by one page.
fn page_keys(check: &mut Check, state: &super::Seeded) -> anyhow::Result<()> {
    let long = super::key_of(state, &state.long_thread);
    check.go(&long);
    check.pump(|c| c.shown_input().entries.len() >= 6, Duration::from_secs(20), "long thread shown")?;
    check.wait(Duration::from_millis(400));
    check.focus_composer();
    let (top, page) = timeline_metrics(check);
    check.press("pageup");
    check.wait(Duration::from_millis(300));
    let (after_up, _) = timeline_metrics(check);
    check.check("Page Up in the composer scrolls the timeline up", after_up < top - page * 0.9, format!("top {top:.1} -> {after_up:.1}, page {page:.1}"));
    check.press("pagedown");
    check.wait(Duration::from_millis(300));
    let (after_down, _) = timeline_metrics(check);
    check.check("Page Down scrolls down exactly one page", (after_down - after_up - page).abs() < 1., format!("{after_up:.1} -> {after_down:.1}, page {page:.1}"));
    check.press("pageup");
    check.wait(Duration::from_millis(300));
    let (again, _) = timeline_metrics(check);
    check.check("and Page Up exactly one page from a settled position", (after_down - again - page).abs() < 1., format!("{after_down:.1} -> {again:.1}"));
    Ok(())
}
