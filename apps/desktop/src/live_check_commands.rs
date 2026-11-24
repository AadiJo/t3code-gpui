// live-check phase `wiring`, after `seed` on one server: the thread and project commands this
// app wires, each dispatched from the focused composer of a live thread (the path a key binding
// takes once it resolves) and read back from the server's shell row: settle then undo, pin,
// unpin then undo, archive from the chat header's menu then undo, a project pinned and unpinned
// from the palette, a new local thread in the shown project, and the visit watermark holding
// while the window is in the background.

use super::*;
use t3code_web::components::chat_view::ChatViewEvent;
use t3code_web::components::command_palette_logic::PaletteEvent;
use t3code_web::components::sidebar::context_menu::MenuCommand;
use t3code_web::shortcuts::commands::{NewLocalChat, PinThread, SettleThread, UndoThreadAction};

/// The shown row's (settled, pinned, archived), as the server's shell row says.
fn row_state(check: &mut Check, key: &ThreadKey) -> Option<(bool, bool, bool)> {
    let key = key.clone();
    check.cx.update(|cx| {
        let shell = StateBridge::shell(cx);
        let shell = shell.read(cx);
        let row = shell.row(&key)?;
        let settled = shell.snapshot().threads.iter().find(|t| t.key == key).is_some_and(|t| {
            t.settled_override == Some(t3code_web::components::sidebar::model::SettledOverride::Settled)
        });
        Some((settled, row.source.pinned_at.is_some(), row.source.archived_at.is_some()))
    })
}

fn wait_for(check: &mut Check, key: &ThreadKey, what: &str, want: impl Fn((bool, bool, bool)) -> bool + 'static) -> anyhow::Result<()> {
    let k = key.clone();
    let result = check.pump(move |c| row_state(c, &k).is_some_and(&want), Duration::from_secs(20), what);
    let state = row_state(check, key);
    check.check(what, result.is_ok(), format!("(settled, pinned, archived) = {state:?}"));
    Ok(())
}

pub(super) fn run_commands(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, &format!("Commands check {}", now_ms() % 100_000))?;
    check.go(&key);
    check.wait(Duration::from_millis(500));
    check.focus_composer();

    check.action(SettleThread);
    wait_for(check, &key, "thread.settle settles the thread", |(settled, _, _)| settled)?;
    check.shot("commands-00-settled.png")?;
    check.action(UndoThreadAction);
    wait_for(check, &key, "thread.undo un-settles it", |(settled, _, _)| !settled)?;

    check.focus_composer();
    check.action(PinThread);
    wait_for(check, &key, "thread.pin pins the thread", |(_, pinned, _)| pinned)?;
    check.action(PinThread);
    wait_for(check, &key, "thread.pin again unpins it", |(_, pinned, _)| !pinned)?;
    check.shot("commands-01-unpinned-notice.png")?;
    check.action(UndoThreadAction);
    wait_for(check, &key, "thread.undo pins it back", |(_, pinned, _)| pinned)?;

    let root = check.root.clone();
    let event = ChatViewEvent::ThreadCommand(key.clone(), MenuCommand::Archive);
    check.cx.update_window(check.window, |_, w, cx| {
        let view = root.read(cx).thread_view().clone();
        view.update(cx, |view, cx| view.run_chat_event(&event, w, cx));
    })?;
    check.frame();
    // An archived thread leaves the shell.
    let k = key.clone();
    let gone = check.pump(move |c| row_state(c, &k).is_none_or(|(_, _, archived)| archived), Duration::from_secs(20), "archived");
    check.check("the header menu's Archive archives the thread", gone.is_ok(), "");
    let notice = check.cx.update(|cx| t3code_web::thread_undo::ThreadUndo::notice(cx));
    let route = check.route();
    check.note(format!("undo notice {notice:?}, route {route:?}"));
    check.action(UndoThreadAction);
    let notice = check.cx.update(|cx| t3code_web::thread_undo::ThreadUndo::notice(cx));
    check.note(format!("after thread.undo: notice {notice:?}"));
    wait_for(check, &key, "thread.undo unarchives it", |(_, _, archived)| !archived)?;

    // The palette's Pin project, read back from the server's project row.
    let project = check.read(|root, cx| {
        root.sidebar().read(cx).snapshot().threads.iter().find(|t| t.key == key).map(|t| t.project_ref())
    });
    if let Some(project) = project {
        let pinned = |c: &mut Check, project: &t3code_web::components::sidebar::model::ProjectRef| {
            c.read(|root, cx| root.sidebar().read(cx).snapshot().projects.iter().find(|p| p.project_ref == *project).is_some_and(|p| p.pinned))
        };
        let before = pinned(check, &project);
        let event = PaletteEvent::ToggleProjectPin(project.clone());
        check.cx.update_window(check.window, |_, w, cx| root.update(cx, |root, cx| root.run_palette_event(&event, w, cx)))?;
        let p = project.clone();
        let flipped = check.pump(move |c| pinned(c, &p) != before, Duration::from_secs(20), "the project's pin flips");
        check.check("the palette's Pin project changes the server's project", flipped.is_ok(), format!("pinned before {before}"));
        check.cx.update_window(check.window, |_, w, cx| root.update(cx, |root, cx| root.run_palette_event(&event, w, cx)))?;
        let p = project.clone();
        let back = check.pump(move |c| pinned(c, &p) == before, Duration::from_secs(20), "the project's pin flips back");
        check.check("running it again restores the pin", back.is_ok(), "");
    } else {
        check.check("the thread's project is listed", false, "");
    }

    // A background window records no visit; coming back to the front records one.
    check.go(&key);
    check.wait(Duration::from_millis(300));
    check.cx.update(|cx| { let data = root.read(cx).data().clone(); data.set_window_active(false, cx) });
    let visited = |c: &mut Check, key: &ThreadKey| {
        let key = key.clone();
        c.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&key).and_then(|row| row.source.last_visited_at.clone()))
    };
    let before = visited(check, &key);
    let sent = send_turn_on(check, &key, "background turn");
    check.wait(Duration::from_secs(4));
    let during = visited(check, &key);
    check.check("a turn finishing in the background moves no visit", sent && during == before, format!("{before:?} -> {during:?}"));
    check.cx.update(|cx| { let data = root.read(cx).data().clone(); data.set_window_active(true, cx) });
    let k = key.clone();
    let b = during.clone();
    let moved = check.pump(move |c| visited(c, &k) != b, Duration::from_secs(20), "the visit moves once in front");
    let after = visited(check, &key);
    check.check("coming to the front records the visit", moved.is_ok(), format!("{after:?}"));

    check.focus_composer();
    check.action(NewLocalChat);
    check.wait(Duration::from_millis(500));
    let route = check.route();
    check.check("chat.newLocal opens a draft", matches!(route, Route::Draft(_)), format!("{route:?}"));
    check.shot("commands-02-new-local.png")?;
    Ok(())
}

/// Sends `text` to `key` through the server and reports whether it was accepted.
fn send_turn_on(check: &mut Check, key: &ThreadKey, text: &str) -> bool {
    let (environment_id, thread_id) = t3code_web::state::shell::runtime_ids(key);
    let commands = check.cx.update(|cx| StateBridge::environment(&environment_id, cx).map(|e| e.read(cx).commands.clone()));
    let Some(commands) = commands else { return false };
    let message = SendMessage::text(thread_id, text.to_owned());
    check.block(async move { commands.send_message(message).await.is_ok() }).unwrap_or(false)
}
