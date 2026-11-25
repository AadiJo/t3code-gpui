//! `live-check context-menus`: the context menus that act on a thread's host, against a real
//! server whose editors and file manager are stand-ins that log their arguments
//! (`T3_LAUNCH_LOG`). Checks the menus the timeline gets for the shown thread (the server's
//! editor and reveal labels), runs Open in editor, Reveal and Open with from the timeline and the
//! Files tab and reads each launch back from the log, opens the branch trigger's copy menu and
//! picks it, and unarchives a thread from the Archive page's row menu and its button, read back
//! from the server's shell. `T3_PAIRING_URL`, `T3_PROJECT_ROOT`, `T3_LAUNCH_LOG`.

use std::time::Duration;

use t3code_client_runtime::operations::CreateThread;
use t3code_contracts::{EditorId, ModelSelection, OrchestrationV2Command, ProviderInteractionMode, RuntimeMode, ThreadId};
use t3code_web::components::chat_markdown::MarkdownMenuAction;
use t3code_web::file_context_menu::FileContextMenuAction;
use t3code_web::router::SettingsPanel;
use t3code_web::state::StateBridge;
use t3code_web::state::shell::thread_key;

use super::live_check_families::{all_connected, pair_if_needed, project_at};
use super::*;

fn launches() -> String {
    std::env::var("T3_LAUNCH_LOG").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()
}

/// Waits for a new launch line containing every one of `parts`; returns it.
fn await_launch(check: &mut Check, before: usize, parts: &[&str], what: &str) -> Option<String> {
    let parts: Vec<String> = parts.iter().map(|p| p.to_string()).collect();
    let found = |_: &mut Check| launches().lines().skip(before).any(|l| parts.iter().all(|p| l.contains(p.as_str())));
    let ok = check.pump(found, Duration::from_secs(15), what).is_ok();
    let line = launches().lines().skip(before).find(|l| parts.iter().all(|p| l.contains(p.as_str()))).map(str::to_owned);
    check.check(what, ok, line.clone().unwrap_or_else(|| format!("log: {:?}", launches().lines().skip(before).collect::<Vec<_>>())));
    line
}

/// A right press then a left click on the first row of the menu it opens.
fn right_click_first_row(check: &mut Check, at: Point<Pixels>, row: usize) -> anyhow::Result<()> {
    check.cx.update_window(check.window, |_, w, cx| {
        w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
        w.dispatch_event(MouseDownEvent { button: MouseButton::Right, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(), cx);
        w.dispatch_event(MouseUpEvent { button: MouseButton::Right, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
    })?;
    check.frame();
    // The menu opens at the pointer: 4px padding, then rows of `min-h-7` at the interface size.
    let rem = check.cx.update(|cx| t3code_web::ActiveAppTheme::app_theme(cx).rem(1.));
    let row_at = at + point(px(40.), px(4.) + rem * 1.75 * (row as f32 + 0.5));
    click_at(check, row_at)
}

fn element_center(check: &mut Check, id: &str) -> Option<Point<Pixels>> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center())).ok().flatten()
}

pub(super) fn run_context_menus(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let environment_id = check
        .cx
        .update(|cx| ConnectionRuntime::state(cx).read(cx).environments.first().map(|e| t3code_contracts::EnvironmentId::new(e.entry.environment_id.as_str())))
        .context("no environment")?;
    let root = std::env::var("T3_PROJECT_ROOT")?;
    let project = project_at(check, &environment_id, &root)?;
    let commands = commands_of(check, &environment_id)?;
    let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
    let input = CreateThread {
        thread_id: thread_id.clone(),
        project_id: project.clone(),
        title: "Context menus".into(),
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: Some("feature/clamp".into()),
        worktree_path: None,
    };
    let c = commands.clone();
    check.block(async move { c.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let key = thread_key(&environment_id, &thread_id);
    let k = key.clone();
    check.pump(move |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&k).is_some()), Duration::from_secs(20), "thread in the shell")?;
    check.go(&key);
    check.frame();

    // What the host can do, as the server's config says (loopback: shell actions allowed).
    let editors = check.cx.update(|cx| StateBridge::environment(&environment_id, cx).and_then(|e| e.read(cx).config.clone()).map(|c| c.available_editors.clone())).unwrap_or_default();
    check.note(format!("server editors: {editors:?}"));
    let timeline = check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().clone());
    let (preview, editor, reveal, cwd, files) = check.cx.update(|cx| {
        let m = timeline.read(cx).markdown_menus();
        (m.preview, m.editor_label.clone(), m.reveal_label.clone(), m.cwd.clone(), m.files.clone())
    });
    check.check("the timeline's file links offer the server's editor", editor.as_deref() == Some("Open in VS Code"), format!("{editor:?}"));
    check.check("the timeline's file links offer Reveal in the server's file manager", reveal.as_deref() == Some("Reveal in Files"), format!("{reveal:?}"));
    check.check("message menus may open the integrated browser", preview, preview);
    check.check("relative links resolve against the thread's checkout", cwd.as_deref() == Some(root.as_str()), format!("{cwd:?}"));
    check.check(
        "the changed-files menu lists the server's editors and reveal",
        files.editor_ids.contains(&EditorId::Vscode) && files.reveal.is_some(),
        format!("{files:?}"),
    );

    // Timeline rows run on the host.
    let emit = |check: &mut Check, action: MarkdownMenuAction| {
        let t = timeline.clone();
        check.cx.update(|cx| t.update(cx, |_, cx| cx.emit(action)));
        check.frame();
    };
    let before = launches().lines().count();
    emit(check, MarkdownMenuAction::OpenFileInEditor("src/math.rs:2".into()));
    await_launch(check, before, &["code", &format!("{root}/src/math.rs")], "Open in VS Code on a file link launches the editor on the host at that path");
    let before = launches().lines().count();
    emit(check, MarkdownMenuAction::RevealFile("src/math.rs".into()));
    await_launch(check, before, &["xdg-open", &format!("{root}/src")], "Reveal in Files opens the file's folder on the host");
    let before = launches().lines().count();
    emit(check, MarkdownMenuAction::File(FileContextMenuAction::OpenWith(EditorId::Vscode), "src/main.rs".into()));
    await_launch(check, before, &["code", &format!("{root}/src/main.rs")], "a changed file's Open with › VS Code opens it on the host");

    // The Files tab's menu goes through the right panel to the host.
    let panel = check.read(|root, _| root.right_panel().clone());
    check.cx.update_window(check.window, |_, w, cx| panel.update(cx, |p, cx| p.open_file("README.md".into(), None, w, cx)))?;
    check.wait(Duration::from_millis(300));
    let files_panel = check.cx.update(|cx| panel.update(cx, |p, cx| p.files_panel(cx))).context("no Files panel")?;
    let before = launches().lines().count();
    check.cx.update(|cx| files_panel.update(cx, |f, cx| f.run_file_action(FileContextMenuAction::RevealInFolder, "README.md".into(), cx)));
    check.frame();
    await_launch(check, before, &["xdg-open", &root], "the Files tab's Reveal in Files opens the folder on the host");

    // The branch trigger's copy menu.
    // A started thread's branch sits in the details panel.
    let root_view = check.root.clone();
    check.cx.update(|cx| root_view.read(cx).thread_view().clone().update(cx, |view, cx| view.set_details_open(true, cx)));
    check.wait(Duration::from_millis(600));
    check.shot("menus-00-thread.png")?;
    let trigger = element_center(check, "strip-branch").or_else(|| element_center(check, "branch-select"));
    if let Some(at) = trigger {
        right_click_first_row(check, at, 0)?;
        check.shot("menus-01-branch-copied.png")?;
        let copied = check.cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
        check.check("the branch trigger's Copy branch name copies the thread's branch", copied.as_deref() == Some("feature/clamp"), format!("{copied:?}"));
    } else {
        check.check("the details panel shows the branch trigger", false, "no branch-select");
    }

    // Archive the thread on the server, then unarchive it from the Archive page's row menu.
    let archive = |id: &ThreadId| -> anyhow::Result<OrchestrationV2Command> {
        Ok(serde_json::from_value(serde_json::json!({ "type": "thread.archive", "commandId": uuid::Uuid::new_v4().to_string(), "threadId": id.as_str() }))?)
    };
    let shown = |key: &t3code_web::components::sidebar::model::ThreadKey| {
        let key = key.clone();
        move |c: &mut Check| c.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&key).is_some())
    };
    let c = commands.clone();
    let command = archive(&thread_id)?;
    check.block(async move { c.dispatch(command).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let k = key.clone();
    check.pump(move |c| !shown(&k)(c), Duration::from_secs(20), "archived on the server")?;
    let router = check.read(|root, _| root.router().clone());
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(Route::Settings { panel: SettingsPanel::Archived, query: SharedString::default() }, cx)));
    let tip = format!("archived-tip-unarchive-{}", thread_id.as_str());
    let t = tip.clone();
    check.pump(move |c| element_center(c, &t).is_some(), Duration::from_secs(20), "the archived row is listed")?;
    check.shot("menus-02-archived-row.png")?;
    let at = element_center(check, &tip).context("archived row")?;
    // The row's menu opens anywhere on the row; its first row is Unarchive.
    let row_at = at - point(px(300.), px(0.));
    check.cx.update_window(check.window, |_, w, cx| {
        w.dispatch_event(MouseDownEvent { button: MouseButton::Right, position: row_at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(), cx);
        w.dispatch_event(MouseUpEvent { button: MouseButton::Right, position: row_at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
    })?;
    check.shot("menus-03-archived-menu.png")?;
    let rem = check.cx.update(|cx| t3code_web::ActiveAppTheme::app_theme(cx).rem(1.));
    click_at(check, row_at + point(px(40.), px(4.) + rem * 1.75 * 0.5))?;
    let k = key.clone();
    let back = check.pump(move |c| shown(&k)(c), Duration::from_secs(20), "unarchived from the row menu");
    check.check("the Archive row menu's Unarchive reads back as unarchived on the server", back.is_ok(), format!("{back:?}"));

    // Again with the row's own Unarchive button.
    let c = commands.clone();
    let command = archive(&thread_id)?;
    check.block(async move { c.dispatch(command).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let k = key.clone();
    check.pump(move |c| !shown(&k)(c), Duration::from_secs(20), "archived again")?;
    let archived = check.read(|root, _| root.router().clone());
    check.cx.update(|cx| archived.update(cx, |router, cx| router.navigate(Route::Settings { panel: SettingsPanel::Archived, query: SharedString::default() }, cx)));
    let t = tip.clone();
    check.pump(move |c| element_center(c, &t).is_some(), Duration::from_secs(20), "listed again")?;
    let at = element_center(check, &tip).context("archived button")?;
    click_at(check, at)?;
    let k = key.clone();
    let back = check.pump(move |c| shown(&k)(c), Duration::from_secs(20), "unarchived from the button");
    check.check("the Archive row's Unarchive button reads back as unarchived on the server", back.is_ok(), format!("{back:?}"));
    Ok(())
}
