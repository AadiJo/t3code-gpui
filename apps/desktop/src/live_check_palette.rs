// live-check phase `palette`, after `seed` on one server: the live palette's thread commands run
// through the real palette on a thread with a turn. "Link pull request to thread" opens the
// dialog, a pasted URL links the pull request (the server's shell row lists it), "Show linked
// items" opens the Linked items tab, "Restart agent session" shows its toast, and "Open pull
// requests" opens that page. The palette's gates are read from the live source.

use super::*;
use t3code_web::components::command_palette_lookups::PaletteFeature;
use t3code_web::components::ui::toast::Toaster;
use t3code_web::right_panel_store::RightPanelSurface;
use t3code_web::shortcuts::commands::ToggleCommandPalette;

const PULL_REQUEST_URL: &str = "https://github.com/example/quokka/pull/7";

/// Opens the palette, types `title` and runs the top row.
fn run_command(check: &mut Check, title: &str) {
    check.action(ToggleCommandPalette);
    check.wait(Duration::from_millis(300));
    check.type_text(title);
    check.wait(Duration::from_millis(500));
    check.press("enter");
    check.wait(Duration::from_millis(300));
}

fn linked_numbers(check: &mut Check, key: &ThreadKey) -> Vec<u64> {
    let key = key.clone();
    check.cx.update(|cx| {
        StateBridge::shell(cx)
            .read(cx)
            .row(&key)
            .and_then(|row| row.source.pull_requests.clone())
            .map(|links| links.iter().map(|link| link.number).collect())
            .unwrap_or_default()
    })
}

pub(super) fn run_palette(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, &format!("Palette check {}", now_ms() % 100_000))?;
    check.go(&key);
    check.wait(Duration::from_millis(500));
    let source = check.read(|root, _| root.palette_source());
    for (feature, name) in [
        (PaletteFeature::LinkPullRequest, "Link pull request"),
        (PaletteFeature::PullRequests, "Open pull requests"),
        (PaletteFeature::RestartAgentSession, "Restart agent session"),
    ] {
        check.check(&format!("the live palette offers {name}"), source.supports(feature), "");
    }
    check.note(format!(
        "server offers issues: {}, triage: {}",
        source.supports(PaletteFeature::Issues),
        source.supports(PaletteFeature::Triage)
    ));
    check.check("Show linked items starts disabled", !source.active_thread_has_linked_items(), "");

    run_command(check, "Link pull request to thread");
    check.pump(|c| c.read(|root, _| root.link_dialog_open()), Duration::from_secs(5), "the link dialog opens")?;
    check.shot("palette-00-link-dialog.png")?;
    check.type_text(PULL_REQUEST_URL);
    check.wait(Duration::from_millis(200));
    check.shot("palette-01-link-typed.png")?;
    check.press("enter");
    let k = key.clone();
    check.pump(move |c| linked_numbers(c, &k).contains(&7), Duration::from_secs(30), "the server lists the linked pull request")?;
    check.pump(|c| !c.read(|root, _| root.link_dialog_open()), Duration::from_secs(5), "the link dialog closes")?;
    let linked = linked_numbers(check, &key);
    check.check("the thread links pull request #7", linked.contains(&7), format!("{linked:?}"));

    check.pump(|c| c.read(|root, _| root.palette_source().active_thread_has_linked_items()), Duration::from_secs(10), "Show linked items enables")?;
    run_command(check, "Show linked items");
    let panel_key = t3code_web::fixtures::scoped_key(&key);
    check.pump(
        move |c| {
            c.read(|root, cx| {
                let state = root.right_panels().read(cx).state(&panel_key);
                state.is_open && matches!(state.active(), Some(RightPanelSurface::PullRequests { .. }))
            })
        },
        Duration::from_secs(5),
        "the Linked items tab opens",
    )?;
    check.shot("palette-02-linked-items.png")?;

    run_command(check, "Restart agent session");
    check.pump(
        |c| c.cx.update(|cx| Toaster::titles(cx).iter().any(|t| t.as_ref() == "Agent session will restart")),
        Duration::from_secs(10),
        "the restart toast shows",
    )?;
    check.shot("palette-03-restart.png")?;

    run_command(check, "Open pull requests");
    check.pump(|c| matches!(c.route(), Route::PullRequests { .. }), Duration::from_secs(5), "the pull requests page opens")?;
    check.shot("palette-04-pull-requests.png")?;
    Ok(())
}
