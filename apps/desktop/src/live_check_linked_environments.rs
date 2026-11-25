//! `live-check linked-environments`: Settings → Environments → "Linked environments" with two
//! real servers (`T3_PAIRING_URL`, `T3_PAIRING_URL_B`, and their addresses as the other server
//! reaches them, `T3_LINK_ADDRESS_A` / `T3_LINK_ADDRESS_B`). Turns the Linked environments beta
//! on for both, links each to the other from its section's "Link an environment" dialog (the
//! saved addresses are loopback, so the dialog asks for an address and a pairing code minted on
//! the other server), reads both links back from the servers (`peerLinks.list`), then forgets
//! one and reads that back.

use std::time::Duration;

use t3code_client_runtime::state::peer_pairing_code::mint_peer_pairing_code;
use t3code_contracts::rpc::methods::PeerLinksList;
use t3code_contracts::{AuthMcpClientAccess, ClientServerSettingsPatch, EnvironmentId};
use t3code_web::components::settings::linked_environments_settings::{LinkedEnvironmentsSection, section};
use t3code_web::hooks::use_settings::ServerSettingsStore;
use t3code_web::live::rpc::RpcLink;
use t3code_web::router::Route;

use super::live_check_families::{all_connected, pair_if_needed};
use super::*;

fn click(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let at = check
        .cx
        .update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center()))
        .ok()
        .flatten()
        .with_context(|| format!("no visible {id}"))?;
    click_at(check, at)
}

fn section_of(check: &mut Check, id: &EnvironmentId) -> Option<gpui_kit::Entity<LinkedEnvironmentsSection>> {
    let id = id.clone();
    check.cx.update(move |cx| section(&id, cx))
}

/// The links `host` reports, straight from the server.
fn server_links(check: &mut Check, host: &EnvironmentId) -> anyhow::Result<Vec<(EnvironmentId, AuthMcpClientAccess)>> {
    let id = host.clone();
    let link = check.cx.update(move |cx| RpcLink::new(&id, cx)).context("no link")?;
    let answer = check.block(link.call::<PeerLinksList>(Default::default()))?.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(answer.links.into_iter().map(|l| (l.environment_id, l.access)).collect())
}

/// Links `host` to `other` from `host`'s section: Link, the other machine, its address and a
/// code minted there, the access, Link.
fn link_from_section(check: &mut Check, host: &EnvironmentId, other: &EnvironmentId, address: &str, access: AuthMcpClientAccess, shot: &str) -> anyhow::Result<()> {
    click(check, &format!("linked-link-{}", host.as_str()))?;
    let dialog = section_of(check, host).and_then(|s| check.cx.update(|cx| s.read(cx).dialog())).context("the dialog opens")?;
    check.check("Link opens Link an environment", true, "");
    let pick = other.clone();
    check.cx.update(|cx| dialog.update(cx, |d, cx| d.choose(Some(pick), cx)));
    check.frame();
    let address_field = visible(check, "link-address");
    let code_field = visible(check, "link-pairing-code");
    check.check("the other machine's saved address is loopback, so the dialog asks for an address and a code", address_field && code_field, "");
    let other_link = check.cx.update(|cx| ConnectionRuntime::global(cx).environment_link(other)).context("no link to the other")?;
    let code = check.block(async move { mint_peer_pairing_code(&other_link, access).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let (address, code_text) = (address.to_owned(), code.clone());
    check.cx.update_window(check.window, |_, window, cx| {
        let (a, p) = { let d = dialog.read(cx); (d.address().clone(), d.pairing_code().clone()) };
        a.update(cx, |s, cx| s.set_value(address.clone(), window, cx));
        p.update(cx, |s, cx| s.set_value(code_text.clone(), window, cx));
    })?;
    check.cx.update(|cx| dialog.update(cx, |d, cx| d.set_access(access, cx)));
    check.wait(Duration::from_millis(300));
    check.shot(shot)?;
    click(check, "link-submit")?;
    let (h, o) = (host.clone(), other.clone());
    let linked = check.pump(
        move |c| section_of(c, &h).and_then(|s| c.cx.update(|cx| s.read(cx).links())).is_some_and(|links| links.iter().any(|l| l.environment_id == o)),
        Duration::from_secs(30),
        "the section lists the new link",
    );
    check.check("Link links the machine and the section lists it", linked.is_ok(), format!("{linked:?}"));
    Ok(())
}

fn visible(check: &mut Check, id: &str) -> bool {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).is_some_and(|e| e.visible())).unwrap_or(false)
}

pub(super) fn run_linked_environments(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    pair_if_needed(check, 2, "T3_PAIRING_URL_B")?;
    check.pump(|c| all_connected(c, 2), Duration::from_secs(60), "both servers connected")?;
    let ids: Vec<EnvironmentId> = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.iter().map(|e| e.entry.environment_id.clone()).collect());
    let (a, b) = (ids[0].clone(), ids[1].clone());
    let address_a = std::env::var("T3_LINK_ADDRESS_A").context("T3_LINK_ADDRESS_A")?;
    let address_b = std::env::var("T3_LINK_ADDRESS_B").context("T3_LINK_ADDRESS_B")?;

    // Start from no links, then turn the beta on for both.
    for (host, other) in [(&a, &b), (&b, &a)] {
        let id = host.clone();
        let link = check.cx.update(move |cx| RpcLink::new(&id, cx)).context("no link")?;
        let other = other.clone();
        let _ = check.block(link.call::<t3code_contracts::rpc::methods::PeerLinksUnlink>(t3code_contracts::PeerLinkRemoveInput { environment_id: other }));
    }
    for id in [&a, &b] {
        let id = id.clone();
        let task = check.cx.update(move |cx| ServerSettingsStore::update(id, ClientServerSettingsPatch { linked_environments_enabled: Some(true), ..Default::default() }, cx));
        let saved = check.wait_task(task)?;
        check.check("the server saves Linked environments on", saved.is_ok(), format!("{:?}", saved.err().map(|e| e.0)));
    }
    let root = check.root.clone();
    check.cx.update(|cx| {
        let router = root.read(cx).router().clone();
        router.update(cx, |r, cx| r.navigate(Route::parse("/settings/connections"), cx));
    });
    let (sa, sb) = (a.clone(), b.clone());
    let shown = check.pump(
        move |c| section_of(c, &sa).zip(section_of(c, &sb)).is_some_and(|(x, y)| c.cx.update(|cx| x.read(cx).links().is_some() && y.read(cx).links().is_some())),
        Duration::from_secs(30),
        "a section per linking server",
    );
    check.check("Environments shows a Linked environments section for each server that offers links", shown.is_ok(), format!("{shown:?}"));
    check.wait(Duration::from_millis(400));
    check.shot("linked-01-empty.png")?;

    link_from_section(check, &a, &b, &address_b, AuthMcpClientAccess::AutoAcceptEdits, "linked-02-dialog-a.png")?;
    link_from_section(check, &b, &a, &address_a, AuthMcpClientAccess::ReadOnly, "linked-03-dialog-b.png")?;
    check.wait(Duration::from_millis(400));
    check.shot("linked-04-linked.png")?;
    let on_a = server_links(check, &a)?;
    let on_b = server_links(check, &b)?;
    check.check("server A reports its link to B at Auto-accept edits", on_a == vec![(b.clone(), AuthMcpClientAccess::AutoAcceptEdits)], format!("{on_a:?}"));
    check.check("server B reports its link to A at Read only", on_b == vec![(a.clone(), AuthMcpClientAccess::ReadOnly)], format!("{on_b:?}"));

    // Forget, with its confirm.
    click(check, &format!("linked-forget-{}", b.as_str()))?;
    check.wait(Duration::from_millis(300));
    check.shot("linked-05-forget-confirm.png")?;
    click(check, "linked-forget-confirm")?;
    let sa = a.clone();
    let gone = check.pump(move |c| section_of(c, &sa).and_then(|s| c.cx.update(|cx| s.read(cx).links())).is_some_and(|l| l.is_empty()), Duration::from_secs(20), "the link goes");
    check.check("Forget removes the link from the section", gone.is_ok(), format!("{gone:?}"));
    let on_a = server_links(check, &a)?;
    check.check("server A reports no links after Forget", on_a.is_empty(), format!("{on_a:?}"));
    check.wait(Duration::from_millis(300));
    check.shot("linked-06-forgotten.png")?;
    Ok(())
}
