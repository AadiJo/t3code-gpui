//! `live-check link-several`: "Link several environments" with three real servers: the host
//! (`T3_PAIRING_URL`) and two more paired at an address another machine can reach
//! (`T3_PAIRING_URL_B`, `T3_PAIRING_URL_C`). Turns Linked environments on for all three, opens
//! the batch from its section, checks both machines are offered once the host's links load,
//! links both in one go (each code minted on its machine with this client's session there),
//! reads both links back from the host (`peerLinks.list`), then opens the batch again and
//! checks both now read "Already linked". It forgets both links at the end.

use std::time::Duration;

use t3code_contracts::rpc::methods::{PeerLinksList, PeerLinksUnlink};
use t3code_contracts::{AuthMcpClientAccess, ClientServerSettingsPatch, EnvironmentId, PeerLinkRemoveInput};
use t3code_web::components::settings::link_several_environments::{LinkBatchBlock, LinkBatchOutcome, LinkSeveralEnvironments};
use t3code_web::components::settings::linked_environments_settings::section;
use t3code_web::hooks::use_settings::ServerSettingsStore;
use t3code_web::live::rpc::RpcLink;
use t3code_web::router::Route;

use super::live_check_families::{all_connected, pair_if_needed};
use super::live_check_settings::click_id;
use super::*;

fn dialog(check: &mut Check, host: &EnvironmentId) -> Option<gpui_kit::Entity<LinkSeveralEnvironments>> {
    let id = host.clone();
    check.cx.update(move |cx| section(&id, cx).and_then(|s| s.read(cx).several_dialog()))
}

fn blocks(check: &mut Check, dialog: &gpui_kit::Entity<LinkSeveralEnvironments>) -> Vec<(String, Option<LinkBatchBlock>)> {
    check.cx.update(|cx| dialog.read(cx).candidates(cx).into_iter().map(|(m, b)| (m.label, b)).collect())
}

pub(super) fn run_link_several(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    pair_if_needed(check, 2, "T3_PAIRING_URL_B")?;
    pair_if_needed(check, 3, "T3_PAIRING_URL_C")?;
    check.pump(|c| all_connected(c, 3), Duration::from_secs(60), "three servers connected")?;
    let ids: Vec<EnvironmentId> = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.iter().map(|e| e.entry.environment_id.clone()).collect());
    let host = ids[0].clone();
    // Start from no links on the host, with the beta on.
    for other in &ids[1..] {
        let id = host.clone();
        let link = check.cx.update(move |cx| RpcLink::new(&id, cx)).context("no link")?;
        let _ = check.block(link.call::<PeerLinksUnlink>(PeerLinkRemoveInput { environment_id: other.clone() }));
    }
    // Every machine must have Linked environments on: a peer with it off refuses the link.
    for id in &ids {
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
    let sh = host.clone();
    check.pump(move |c| { let id = sh.clone(); c.cx.update(move |cx| section(&id, cx).is_some_and(|s| s.read(cx).links().is_some())) }, Duration::from_secs(30), "host section")?;
    check.wait(Duration::from_millis(300));
    check.shot("several-01-section.png")?;
    click_id(check, &format!("linked-several-{}", host.as_str()))?;
    let d = dialog(check, &host).context("Link several opens the dialog")?;
    let dd = d.clone();
    check.pump(move |c| blocks(c, &dd).iter().all(|(_, b)| b.is_none()), Duration::from_secs(20), "both machines offered")?;
    let offered = blocks(check, &d);
    check.check("both other machines are offered once the host's links load", offered.len() == 2 && offered.iter().all(|(_, b)| b.is_none()), format!("{offered:?}"));
    check.wait(Duration::from_millis(300));
    check.shot("several-02-dialog.png")?;
    click_id(check, "link-several-all")?;
    check.cx.update(|cx| d.update(cx, |d, cx| d.set_access(AuthMcpClientAccess::AutoAcceptEdits, cx)));
    check.wait(Duration::from_millis(200));
    check.shot("several-03-selected.png")?;
    click_id(check, "link-several-submit")?;
    let dd = d.clone();
    check.pump(
        move |c| c.cx.update(|cx| dd.read(cx).outcomes().len() == 2 && dd.read(cx).outcomes().iter().all(|(_, o)| *o == LinkBatchOutcome::Linked)),
        Duration::from_secs(60),
        "both linked",
    )?;
    let outcomes: Vec<_> = check.cx.update(|cx| d.read(cx).outcomes().to_vec());
    check.check("Link 2 machines links both in one go", outcomes.iter().all(|(_, o)| *o == LinkBatchOutcome::Linked), format!("{outcomes:?}"));
    check.wait(Duration::from_millis(500));
    check.shot("several-04-linked.png")?;
    let id = host.clone();
    let link = check.cx.update(move |cx| RpcLink::new(&id, cx)).context("no link")?;
    let listed = check.block(link.call::<PeerLinksList>(Default::default()))?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut on_host: Vec<(String, AuthMcpClientAccess)> = listed.links.iter().map(|l| (l.environment_id.as_str().to_owned(), l.access)).collect();
    on_host.sort();
    let mut want: Vec<(String, AuthMcpClientAccess)> = ids[1..].iter().map(|id| (id.as_str().to_owned(), AuthMcpClientAccess::AutoAcceptEdits)).collect();
    want.sort();
    check.check("the host reports both links at the access picked", on_host == want, format!("{on_host:?}"));
    click_id(check, "link-several-close")?;
    let sh = host.clone();
    check.pump(move |c| { let id = sh.clone(); c.cx.update(move |cx| section(&id, cx).and_then(|s| s.read(cx).links()).is_some_and(|l| l.len() == 2)) }, Duration::from_secs(20), "section lists both")?;
    check.check("Done closes the dialog and the section lists both links", true, "");
    check.wait(Duration::from_millis(400));
    check.shot("several-05-section-linked.png")?;
    click_id(check, &format!("linked-several-{}", host.as_str()))?;
    let d = dialog(check, &host).context("the dialog opens again")?;
    let dd = d.clone();
    check.pump(move |c| blocks(c, &dd).iter().all(|(_, b)| *b == Some(LinkBatchBlock::Linked)), Duration::from_secs(20), "already linked")?;
    let shown = blocks(check, &d);
    check.check("opened again, both machines read Already linked and none can be picked", true, format!("{shown:?}"));
    check.wait(Duration::from_millis(300));
    check.shot("several-06-already-linked.png")?;
    click_id(check, "link-several-close")?;
    // Leave the host as it was found: other phases expect it unlinked.
    for other in &ids[1..] {
        let id = host.clone();
        let link = check.cx.update(move |cx| RpcLink::new(&id, cx)).context("no link")?;
        let _ = check.block(link.call::<PeerLinksUnlink>(PeerLinkRemoveInput { environment_id: other.clone() }));
    }
    Ok(())
}
