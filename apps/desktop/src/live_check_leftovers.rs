// live-check phase `snooze-decimal`, after `seed`: "Snooze → Custom…" takes a decimal duration as
// the fork's `step="any"` field does: 1.5 hours snoozes the thread, and the server's row wakes about
// an hour and a half from now.

use super::*;
use t3code_web::components::sidebar::context_menu::MenuCommand;

pub(super) fn run_snooze_decimal(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, &format!("Snooze decimal {}", now_ms() % 100_000))?;
    check.go(&key);
    check.wait(Duration::from_millis(400));
    let root = check.root.clone();
    let keys = vec![key.clone()];
    check.cx.update_window(check.window, |_, w, cx| root.update(cx, |root, cx| root.thread_command(&keys, MenuCommand::SnoozeCustom, w, cx))).ok();
    check.wait(Duration::from_millis(300));
    let dialog = check.read(|root, _| root.custom_snooze_dialog()).context("the snooze dialog opens")?;
    check.cx.update_window(check.window, |_, w, cx| dialog.update(cx, |d, cx| d.show_duration(w, cx))).ok();
    check.cx.update_window(check.window, |_, w, cx| dialog.update(cx, |d, cx| d.set_amount("1.5", w, cx))).ok();
    check.shot("snooze-decimal-00-dialog.png")?;
    let before = now_ms();
    check.cx.update_window(check.window, |_, w, cx| dialog.update(cx, |d, cx| d.submit_for_check(w, cx))).ok();
    let k = key.clone();
    check.pump(move |c| sidebar_row(c, &k).is_some_and(|t| t.snoozed_until.is_some()), Duration::from_secs(15), "the server snoozes the thread")?;
    let until = sidebar_row(check, &key).and_then(|t| t.snoozed_until).map(|t| t.0).unwrap_or_default();
    let hours = (until - before) as f64 / 3_600_000.;
    check.check("the thread wakes an hour and a half from now", (1.48..1.52).contains(&hours), format!("{hours:.3} h"));
    Ok(())
}
