//! `live-check usage-limits`: the `/usage-limits` banner on a server whose usage-limit source is a
//! stand-in CLI proxy hub (two Claude logins, one Codex login with a reset credit). Creates a
//! thread on the server's Claude instance and one on its Codex instance (`usage-state.json`),
//! then on each: `/usage-limits` offered in the `/` menu, picked or sent alone, the banner's
//! accounts, windows and credits against the hub, dismiss, a redeemed credit read back, and a
//! send closing the banner. `T3_PAIRING_URL`, `T3_PROJECT_ROOT`.

use std::time::Duration;

use t3code_client_runtime::operations::CreateThread;
use t3code_contracts::{ModelSelection, ProviderInteractionMode, RuntimeMode, ThreadId};
use t3code_web::components::chat::composer::{ComposerBanner, ComposerEvent};
use t3code_web::components::sidebar::model::ThreadKey;
use t3code_web::composer_logic::ComposerSubmissionIntent;
use t3code_web::state::StateBridge;
use t3code_web::state::shell::thread_key;

use super::live_check_families::{all_connected, pair_if_needed, project_at};
use super::{Check, commands_of};

const STATE_FILE: &str = "usage-state.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct UsageState {
    environment_id: String,
    claude_thread: String,
    codex_thread: String,
}

fn usage_banner(check: &mut Check) -> Option<ComposerBanner> {
    check.read(|root, cx| root.composer(cx).read(cx).state().banners.iter().find(|b| b.id.starts_with("usage-limits:")).cloned())
}

fn emit(check: &mut Check, event: ComposerEvent) {
    let root = check.root.clone();
    check.cx.update(|cx| {
        let composer = root.read(cx).composer(cx);
        composer.update(cx, |_, cx| cx.emit(event));
    });
    check.frame();
}

fn prompt(check: &mut Check) -> String {
    check.read(|root, cx| root.composer(cx).read(cx).editor().read(cx).text().to_owned())
}

/// Sends `/usage-limits` alone, as Enter does.
fn send_command(check: &mut Check) {
    let root = check.root.clone();
    check.cx.update(|cx| {
        let composer = root.read(cx).composer(cx);
        composer.update(cx, |composer, cx| {
            composer.editor().update(cx, |editor, cx| editor.set_text("/usage-limits", cx));
            composer.submit(ComposerSubmissionIntent::Foreground, cx);
        });
    });
    check.frame();
}

fn open_thread(check: &mut Check, key: &ThreadKey) -> anyhow::Result<()> {
    check.go(key);
    check.pump(|c| c.read(|root, cx| root.composer(cx).read(cx).state().usage_limits), Duration::from_secs(30), "the provider offers /usage-limits")?;
    Ok(())
}

pub(super) fn run_usage_limits(check: &mut Check) -> anyhow::Result<()> {
    // Shots compare with the fork at the owner's look: dark, at the T3CODE_* theme overrides.
    check.cx.update(|cx| {
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let environment_id = check.cx.update(|cx| {
        t3code_web::connection::ConnectionRuntime::state(cx).read(cx).environments.first().map(|e| t3code_contracts::EnvironmentId::new(e.entry.environment_id.as_str()))
    });
    let environment_id = environment_id.ok_or_else(|| anyhow::anyhow!("no environment"))?;
    let path = check.out.join(STATE_FILE);
    let state = match std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<UsageState>(&bytes).ok()) {
        Some(state) => state,
        None => {
            let root = std::env::var("T3_PROJECT_ROOT")?;
            let project = project_at(check, &environment_id, &root)?;
            let commands = commands_of(check, &environment_id)?;
            let mut ids = Vec::new();
            for (title, instance, model) in [("Usage limits", "claudeAgent", "claude-sonnet-4-5"), ("Usage credits", "codex", "gpt-5.5")] {
                let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
                let input = CreateThread {
                    thread_id: thread_id.clone(),
                    project_id: project.clone(),
                    title: title.into(),
                    model_selection: ModelSelection { instance_id: instance.into(), model: model.into(), options: None },
                    runtime_mode: RuntimeMode::FullAccess,
                    interaction_mode: ProviderInteractionMode::Default,
                    branch: None,
                    worktree_path: None,
                };
                let c = commands.clone();
                check.block(async move { c.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
                ids.push(thread_id.to_string());
            }
            let state = UsageState { environment_id: environment_id.to_string(), claude_thread: ids[0].clone(), codex_thread: ids[1].clone() };
            std::fs::write(&path, serde_json::to_vec_pretty(&state)?)?;
            state
        }
    };
    let claude = thread_key(&environment_id, &ThreadId::new(state.claude_thread.as_str()));
    let codex = thread_key(&environment_id, &ThreadId::new(state.codex_thread.as_str()));
    let id = environment_id.clone();
    check.pump(
        move |c| c.cx.update(|cx| StateBridge::environment(&id, cx).and_then(|env| env.read(cx).config.clone()).is_some_and(|config| config.usage_limit_sources.as_ref().is_some_and(|s| s.iter().any(|s| !s.accounts.is_empty())))),
        Duration::from_secs(60),
        "the hub's accounts reach the config",
    )?;

    // The server lists `usage-limits` among the providers' commands once it has read the hub.
    let id = environment_id.clone();
    check.pump(
        move |c| {
            c.cx.update(|cx| StateBridge::environment(&id, cx).and_then(|env| env.read(cx).config.clone())).is_some_and(|config| {
                config.providers.iter().any(|p| p.instance_id.as_str() == "claudeAgent" && p.slash_commands.iter().flatten().any(|c| c.name == "usage-limits"))
            })
        },
        Duration::from_secs(30),
        "the Claude provider lists usage-limits",
    )?;
    // Claude: the `/` menu offers the command; picking it opens the banner and keeps the prompt.
    open_thread(check, &claude)?;
    check.focus_composer();
    let root = check.root.clone();
    check.cx.update(|cx| {
        let composer = root.read(cx).composer(cx);
        composer.update(cx, |composer, cx| composer.editor().update(cx, |editor, cx| editor.set_text("", cx)));
    });
    check.type_text("/usage");
    check.wait(Duration::from_millis(500));
    let listed = check.read(|root, cx| {
        let composer = root.composer(cx).read(cx);
        (composer.state().usage_limits, composer.editor().read(cx).text().to_owned(), composer.menu().map(|m| m.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>()))
    });
    check.note(format!("after typing: offered, prompt, menu = {listed:?}"));
    check.pump(|c| c.read(|root, cx| root.composer(cx).read(cx).menu().is_some_and(|m| m.items.iter().any(|i| i.label.as_ref() == "/usage-limits"))), Duration::from_secs(10), "the / menu lists /usage-limits")?;
    let description = check.read(|root, cx| root.composer(cx).read(cx).menu().and_then(|m| m.items.iter().find(|i| i.label.as_ref() == "/usage-limits").map(|i| i.description.to_string())));
    check.check("the menu item names the command's purpose", description.as_deref() == Some("Show this provider's usage limits"), format!("{description:?}"));
    check.press("enter");
    check.pump(|c| usage_banner(c).is_some(), Duration::from_secs(10), "picking /usage-limits opens the banner")?;
    let banner = usage_banner(check).expect("banner");
    let body = banner.body.clone().expect("body");
    let text = prompt(check);
    check.check("picking leaves no command text", text.trim().is_empty(), format!("{text:?}"));
    check.check("the banner is titled Usage limits with its account count", banner.title.as_ref() == "Usage limits" && banner.description.as_deref() == Some("2 accounts"), format!("{} · {:?}", banner.title, banner.description));
    let windows: Vec<String> = body.accounts.iter().flat_map(|a| a.windows.iter().map(|w| format!("{} {}%", w.label, w.remaining))).collect();
    check.check("both hub logins list their windows (100 minus the hub's use)", body.accounts.len() == 2 && windows.len() == 4 && windows.iter().any(|w| w.ends_with(" 62%")) && windows.iter().any(|w| w.ends_with(" 18%")), format!("{windows:?}"));
    check.check("the banner has a dismiss button and no credits for Claude", banner.dismiss.is_some() && body.accounts.iter().all(|a| a.credits.is_empty()), format!("{:?}", banner.dismiss));
    check.shot("usage-01-claude.png")?;

    emit(check, ComposerEvent::BannerDismissed(banner.id.clone()));
    check.pump(|c| usage_banner(c).is_none(), Duration::from_secs(5), "dismiss closes the banner")?;
    send_command(check);
    check.pump(|c| usage_banner(c).is_some(), Duration::from_secs(10), "sending /usage-limits alone opens the banner")?;
    let text = prompt(check);
    check.check("the sent command leaves the composer", text.trim().is_empty(), format!("{text:?}"));

    // Codex: one login with a reset credit; redeeming it reads the hub's outcome back. The server
    // rereads the hub on a provider refresh, so a credit spent by an earlier run is back.
    let id = environment_id.clone();
    let session = check.cx.update(|cx| StateBridge::environment(&id, cx).and_then(|env| env.read(cx).store.session()));
    if let Some(session) = session {
        check.block(async move { session.client.call::<t3code_contracts::rpc::methods::ServerRefreshProviders>(t3code_contracts::ServerRefreshProvidersPayload::default()).await })?.ok();
    }
    open_thread(check, &codex)?;
    let none = usage_banner(check).is_none();
    check.check("the banner belongs to its thread", none, "none on the Codex thread");
    check.focus_composer();
    send_command(check);
    check.pump(
        |c| usage_banner(c).and_then(|b| b.body).is_some_and(|b| b.accounts.first().is_some_and(|a| a.can_manage && !a.credits.is_empty())),
        Duration::from_secs(15),
        "the Codex banner, with manage access and its credit",
    )?;
    let banner = usage_banner(check).expect("banner");
    let account = banner.body.as_ref().and_then(|b| b.accounts.first().cloned()).expect("account");
    let credit = account.credits.first().cloned();
    check.check(
        "the Codex login shows its windows and one banked credit",
        banner.description.as_ref().is_some_and(|d| d.contains(" · ")) && account.windows.len() == 2 && credit.as_ref().is_some_and(|c| c.available_count == 1 && c.summary.starts_with("1 reset credit banked")),
        format!("{:?}; {:?}; {:?}", banner.description, account.windows.iter().map(|w| (w.label.to_string(), w.remaining)).collect::<Vec<_>>(), credit.as_ref().map(|c| c.summary.to_string())),
    );
    check.shot("usage-02-codex.png")?;
    let credit = credit.ok_or_else(|| anyhow::anyhow!("no credit"))?;
    emit(check, ComposerEvent::RedeemResetCredit { account: account.id.clone(), choice: credit });
    check.pump(
        |c| usage_banner(c).and_then(|b| b.body).is_some_and(|b| b.accounts.first().and_then(|a| a.status.clone()).is_some()),
        Duration::from_secs(20),
        "the redemption's outcome",
    )?;
    let status = usage_banner(check).and_then(|b| b.body).and_then(|b| b.accounts.first().and_then(|a| a.status.clone()));
    check.check("the hub's reset reads back as applied", status.as_deref() == Some("Reset applied. Your windows have cleared."), format!("{status:?}"));
    check.shot("usage-03-redeemed.png")?;

    // A send resumes quota-spending work and closes the banner (`clearUsageLimitsFor`).
    open_thread(check, &claude)?;
    let open = usage_banner(check).is_some();
    check.check("the Claude banner stays open on its thread", open, "open");
    let root = check.root.clone();
    check.cx.update(|cx| {
        let composer = root.read(cx).composer(cx);
        composer.update(cx, |composer, cx| {
            composer.editor().update(cx, |editor, cx| editor.set_text("Check the limits again.", cx));
            composer.submit(ComposerSubmissionIntent::Foreground, cx);
        });
    });
    check.pump(|c| usage_banner(c).is_none(), Duration::from_secs(10), "a send closes the banner")?;
    Ok(())
}
