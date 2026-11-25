// live-check phase `snooze-custom`, after `seed`: "Snooze → Custom…" on a thread opens the dialog;
// a zero duration is refused; 3 hours snoozes the thread, and the server's row wakes about three
// hours from now.

use super::*;
use t3code_web::components::sidebar::context_menu::MenuCommand;

pub(super) fn run_snooze_custom(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, &format!("Snooze custom {}", now_ms() % 100_000))?;
    check.go(&key);
    check.wait(Duration::from_millis(400));
    let root = check.root.clone();
    let keys = vec![key.clone()];
    check.cx.update_window(check.window, |_, w, cx| root.update(cx, |root, cx| root.thread_command(&keys, MenuCommand::SnoozeCustom, w, cx))).ok();
    check.wait(Duration::from_millis(300));
    let dialog = check.read(|root, _| root.custom_snooze_dialog()).context("the snooze dialog opens")?;
    check.shot("snooze-00-dialog.png")?;
    check.cx.update_window(check.window, |_, w, cx| dialog.update(cx, |d, cx| d.show_duration(w, cx))).ok();
    check.cx.update_window(check.window, |_, w, cx| dialog.update(cx, |d, cx| d.set_amount("0", w, cx))).ok();
    check.cx.update_window(check.window, |_, w, cx| dialog.update(cx, |d, cx| d.submit_for_check(w, cx))).ok();
    check.wait(Duration::from_millis(200));
    let error = check.cx.update(|cx| dialog.read(cx).error().map(|e| e.to_string()));
    check.check("a zero duration is refused", error.as_deref() == Some("Enter a positive duration."), format!("{error:?}"));
    check.cx.update_window(check.window, |_, w, cx| dialog.update(cx, |d, cx| d.set_amount("3", w, cx))).ok();
    let before = now_ms();
    check.cx.update_window(check.window, |_, w, cx| dialog.update(cx, |d, cx| d.submit_for_check(w, cx))).ok();
    let k = key.clone();
    check.pump(move |c| sidebar_row(c, &k).is_some_and(|t| t.snoozed_until.is_some()), Duration::from_secs(15), "the server snoozes the thread")?;
    let until = sidebar_row(check, &key).and_then(|t| t.snoozed_until).map(|t| t.0).unwrap_or_default();
    let hours = (until - before) as f64 / 3_600_000.;
    check.check("the thread wakes three hours from now", (2.98..3.02).contains(&hours), format!("{hours:.3} h"));
    let closed = check.read(|root, _| root.custom_snooze_dialog().is_none());
    check.check("the dialog closes", closed, "");
    check.shot("snooze-01-snoozed.png")?;
    Ok(())
}
