//! `live-check use-in-another-app`: the "Use T3 Code in another app (MCP)" wizard on a real
//! server behind the T3 Connect stand-in (relay and Clerk, plus a direct-route proxy that
//! reports the server's T3 Connect link state, since a real tunnel cannot run here). Opens it
//! from the palette signed out and signs in from its gate; then walks the gate's Turn on,
//! offline and open states as the stand-in reports them; then the App, Access and Connect
//! steps: the grid, the default level, the copied URL, an approval code minted by the real
//! server at the picked level, the primary hand-off of a link, a copied message and commands,
//! "Something else…", and Manage access.
//! `T3_PAIRING_URL` (through the stand-in's direct route), `STANDIN_CONTROL`, `STANDIN_EMAIL`,
//! `T3CODE_RELAY_URL`, `T3CODE_CLERK_PUBLISHABLE_KEY`, `SSL_CERT_FILE`.

use std::io::{Read as _, Write as _};
use std::time::Duration;

use t3code_client_runtime::relay::CloudAccountState;
use t3code_contracts::AuthMcpClientAccess;
use t3code_web::components::mcp::mcp_gate::McpGate;
use t3code_web::components::sharing_dialogs::SharingDialogs;
use t3code_web::shortcuts::commands::ToggleCommandPalette;

use super::live_check_families::{all_connected, pair_if_needed};
use super::*;

/// One plain-HTTP request to the stand-in's control port.
fn control(method: &str, path: &str) -> anyhow::Result<String> {
    let address = std::env::var("STANDIN_CONTROL").context("STANDIN_CONTROL")?;
    let mut stream = std::net::TcpStream::connect(address)?;
    write!(stream, "{method} {path} HTTP/1.0\r\nHost: control\r\nContent-Length: 0\r\n\r\n")?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer)?;
    Ok(answer.split_once("\r\n\r\n").map(|(_, body)| body.to_owned()).unwrap_or_default())
}

fn run_command(check: &mut Check, title: &str) {
    check.action(ToggleCommandPalette);
    check.wait(Duration::from_millis(300));
    check.type_text(title);
    check.wait(Duration::from_millis(500));
    check.press("enter");
    check.wait(Duration::from_millis(300));
}

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

fn visible(check: &mut Check, id: &str) -> bool {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).is_some_and(|e| e.visible())).unwrap_or(false)
}

fn gate(check: &mut Check) -> Option<McpGate> {
    check.cx.update(|cx| SharingDialogs::use_in_another_app(cx).map(|d| d.read(cx).gate(cx)))
}

fn wait_gate(check: &mut Check, what: &str, matches: impl Fn(&McpGate) -> bool + 'static) -> anyhow::Result<McpGate> {
    let ok = check.pump(move |c| gate(c).is_some_and(|g| matches(&g)), Duration::from_secs(30), what);
    let shown = gate(check);
    check.check(what, ok.is_ok(), format!("{shown:?}"));
    shown.context("the wizard is closed")
}

/// From the App step to Connect: one Next with hosted approval, two without.
fn next_to_connect(check: &mut Check, hosted: bool) -> anyhow::Result<()> {
    click(check, "mcp-next")?;
    if !hosted {
        check.wait(Duration::from_millis(200));
        click(check, "mcp-next")?;
    }
    Ok(())
}

fn open_wizard(check: &mut Check) -> anyhow::Result<()> {
    run_command(check, "Use T3 Code in another app");
    if !check.cx.update(|cx| SharingDialogs::is_open(cx)) {
        check.shot("mcp-open-failed.png")?;
    }
    check.pump(|c| c.cx.update(|cx| SharingDialogs::use_in_another_app(cx).is_some()), Duration::from_secs(5), "the wizard opens")
}

/// Signs in from the wizard's gate, reading the code the stand-in "emailed".
fn sign_in(check: &mut Check) -> anyhow::Result<()> {
    click(check, "mcp-sign-in")?;
    check.pump(|c| c.cx.update(|cx| SharingDialogs::sign_in_form(cx).is_some()), Duration::from_secs(5), "the sign-in opens over the app")?;
    check.check("Sign in closes the wizard and opens the T3 Connect sign-in", true, "");
    check.shot("mcp-02-sign-in-email.png")?;
    #[derive(serde::Deserialize)]
    struct Code {
        code: Option<String>,
    }
    let earlier: Code = serde_json::from_str(&control("GET", "/clerk/last-code")?)?;
    check.cx.update_window(check.window, |_, window, cx| {
        if let Some(form) = SharingDialogs::sign_in_form(cx) {
            let field = form.read(cx).email().clone();
            field.update(cx, |state, cx| state.focus(window, cx));
        }
    })?;
    check.frame();
    check.type_text(&std::env::var("STANDIN_EMAIL").unwrap_or_else(|_| "owner@example.test".into()));
    click(check, "t3-connect-submit")?;
    let mut code = None;
    for _ in 0..150 {
        check.wait(Duration::from_millis(100));
        let latest: Code = serde_json::from_str(&control("GET", "/clerk/last-code")?)?;
        if latest.code.is_some() && latest.code != earlier.code {
            code = latest.code;
            break;
        }
    }
    let code = code.context("no email code arrived")?;
    check.wait(Duration::from_millis(300));
    check.cx.update_window(check.window, |_, window, cx| {
        if let Some(form) = SharingDialogs::sign_in_form(cx) {
            let field = form.read(cx).code().clone();
            field.update(cx, |state, cx| state.focus(window, cx));
        }
    })?;
    check.frame();
    check.type_text(&code);
    click(check, "t3-connect-submit")?;
    let signed_in = check.pump(
        |c| c.cx.update(|cx| matches!(ConnectionRuntime::state(cx).read(cx).cloud.as_ref().map(|c| &c.account), Some(CloudAccountState::SignedIn(_)))),
        Duration::from_secs(15),
        "signed in",
    );
    check.check("the email code signs in to T3 Connect", signed_in.is_ok(), format!("{signed_in:?}"));
    check.pump(|c| !c.cx.update(|cx| SharingDialogs::is_open(cx)), Duration::from_secs(5), "the sign-in closes")
}

pub(super) fn run_use_in_another_app(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        // The appearance first: setting it reloads the stored fonts, which the overrides replace.
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    // `T3_MCP_HOSTED=0`: the socket reports no hosted approval, as servers before it did, so the
    // wizard asks for the level and mints a code.
    let hosted = std::env::var("T3_MCP_HOSTED").map_or(true, |v| v != "0");
    control("POST", &format!("/connect?tunnel=none&reach=reachable&forget=1&hosted={}", u8::from(hosted)))?;
    control("POST", "/relay/listed?mode=tunnel")?;
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let environment_id = check
        .cx
        .update(|cx| ConnectionRuntime::state(cx).read(cx).environments.first().map(|e| e.entry.environment_id.clone()))
        .context("no environment")?;
    let signed_out = check.cx.update(|cx| matches!(ConnectionRuntime::state(cx).read(cx).cloud.as_ref().map(|c| &c.account), Some(CloudAccountState::SignedOut)));

    // Signed out, the gate asks for T3 Connect first.
    open_wizard(check)?;
    if signed_out {
        wait_gate(check, "signed out, the gate asks to sign in to T3 Connect", |g| *g == McpGate::SignIn)?;
        check.wait(Duration::from_millis(300));
        check.shot("mcp-01-sign-in.png")?;
        sign_in(check)?;
        open_wizard(check)?;
    }

    // Linked to T3 Connect without a tunnel: Turn on, with the commands for its host.
    wait_gate(check, "with no tunnel the gate offers to turn on T3 Connect", |g| *g == McpGate::TurnOn)?;
    check.wait(Duration::from_millis(300));
    check.shot("mcp-03-turn-on.png")?;
    let command = visible(check, "mcp-turn-on-command-copy") && visible(check, "mcp-turn-on-check");
    check.check("Turn on shows the host command with its copy button and Check again", command, "");

    // A tunnel that Cloudflare cannot reach: offline with the server's reason and fix.
    control("POST", "/connect?tunnel=active&reach=unreachable")?;
    click(check, "mcp-turn-on-check")?;
    let offline = wait_gate(check, "an unreachable tunnel shows the gate offline", |g| matches!(g, McpGate::Offline { reachability: Some(_) }))?;
    check.wait(Duration::from_millis(300));
    check.shot("mcp-04-offline.png")?;
    let reason = match &offline {
        McpGate::Offline { reachability: Some(r) } => r.reason == "Cloudflare could not reach the tunnel." && r.fix == "Restart T3 Code on the host.",
        _ => false,
    };
    let retry = visible(check, "mcp-check-again");
    check.check("offline carries the check's reason and fix", reason && retry, format!("{offline:?}"));

    // A failed check request keeps the gate closed with a retry.
    control("POST", "/connect?reach=fail")?;
    click(check, "mcp-check-again")?;
    let failed = wait_gate(check, "a failed check keeps the gate closed", |g| matches!(g, McpGate::Offline { .. }))?;
    check.note(format!("after a failed check: {failed:?}"));

    // Reachable: the steps.
    control("POST", "/connect?reach=reachable")?;
    click(check, "mcp-check-again")?;
    let open = wait_gate(check, "a reachable tunnel opens the wizard", |g| matches!(g, McpGate::Open { .. }))?;
    let McpGate::Open { mcp_url } = open else { anyhow::bail!("not open") };
    let relay = std::env::var("T3_TUNNEL_ORIGIN").unwrap_or_else(|_| "https://127.0.0.1:18624".into());
    check.check("the MCP URL is the tunnel's address from T3 Connect plus /mcp", mcp_url.as_ref() == format!("{relay}/mcp"), &mcp_url);
    let state = control("GET", "/connect")?;
    check.note(format!("stand-in connect state: {state}"));
    check.wait(Duration::from_millis(300));
    check.shot("mcp-05-app.png")?;
    let tiles = ["poke", "claude", "chatgpt", "grok-bot", "muse", "instinct", "openclaw", "hermes-agent"];
    let all_tiles = tiles.iter().all(|id| visible(check, &format!("mcp-app-{id}")));
    let gemini = visible(check, "mcp-app-gemini");
    check.check("the grid shows the eight apps, Gemini kept out", all_tiles && !gemini, "");
    click(check, "mcp-next")?;
    let step = check.cx.update(|cx| SharingDialogs::use_in_another_app(cx).map(|d| d.read(cx).step()));
    check.check("Next is disabled until an app is chosen", step == Some(0), format!("{step:?}"));
    click(check, "mcp-app-claude")?;
    check.wait(Duration::from_millis(200));
    check.shot("mcp-06-app-chosen.png")?;
    click(check, "mcp-next")?;
    check.wait(Duration::from_millis(300));
    let step = check.cx.update(|cx| SharingDialogs::use_in_another_app(cx).map(|d| d.read(cx).step()));
    if hosted {
        let third = visible(check, "mcp-step-2");
        check.check("with hosted approval Next goes straight to Connect (two steps)", step == Some(2) && !third, format!("{step:?}"));
        let code = visible(check, "mcp-get-code");
        check.check("the sign-in page asks for the level, so there is no code to get", !code, "");
    } else {
        let third = visible(check, "mcp-step-2");
        check.check("without hosted approval Next opens the Access step", step == Some(1) && third, format!("{step:?}"));
    }
    if !hosted {
    let default = check.cx.update(|cx| SharingDialogs::use_in_another_app(cx).and_then(|d| d.read(cx).selected_access(cx)));
    check.check("the Access step starts at Auto", default == Some(AuthMcpClientAccess::Auto), format!("{default:?}"));
    check.shot("mcp-07-access.png")?;
    click(check, "mcp-access-auto-accept-edits")?;
    check.wait(Duration::from_millis(200));
    click(check, "mcp-next")?;
    check.wait(Duration::from_millis(300));
    }
    check.shot("mcp-08-connect.png")?;

    // The URL row copies the URL.
    click(check, "mcp-copy-URL")?;
    let copied = check.cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    check.check("the URL row copies the MCP URL", copied.as_deref() == Some(mcp_url.as_ref()), format!("{copied:?}"));

    // A code minted by the server at the picked level.
    if !hosted {
    click(check, "mcp-get-code")?;
    let minted = check.pump(|c| c.cx.update(|cx| SharingDialogs::use_in_another_app(cx).and_then(|d| d.read(cx).approval_code()).is_some()), Duration::from_secs(15), "a code");
    let code = check.cx.update(|cx| SharingDialogs::use_in_another_app(cx).and_then(|d| d.read(cx).approval_code())).unwrap_or_default();
    check.check(
        "Get code shows a one-use code for Auto-accept edits from the server",
        minted.is_ok() && code.starts_with("EDITS-") && code.len() == "EDITS-".len() + 12,
        if code.is_empty() { "none".to_owned() } else { format!("{}… ({} chars)", &code[..6], code.len()) },
    );
    let state = control("GET", "/connect")?;
    check.check("the server answered the mint at auto-accept-edits", state.contains("\"auto-accept-edits EDITS-"), "");
    let live = visible(check, "mcp-copy-Code") && !visible(check, "mcp-get-code");
    check.check("a live code shows its copy row and hides Get code", live, "");
    check.wait(Duration::from_millis(200));
    check.shot("mcp-09-code.png")?;
    }

    // The primary action opens Claude's add form with T3 Code filled in.
    click(check, "mcp-primary-action")?;
    let opened = check.cx.update(|cx| SharingDialogs::use_in_another_app(cx).and_then(|d| d.read(cx).opened()));
    check.check(
        "Open in Claude opens Claude's connector form with the URL",
        opened.as_deref().is_some_and(|u| u.starts_with("https://claude.ai/customize/connectors?modal=add-custom-connector&connectorName=T3%20Code&connectorUrl=https%3A%2F%2F")),
        format!("{opened:?}"),
    );

    // Set up another way: the steps and the help link.
    click(check, "mcp-another-way")?;
    check.wait(Duration::from_millis(300));
    check.shot("mcp-10-another-way.png")?;
    let another = visible(check, "mcp-docs") && visible(check, "mcp-back-to") && !visible(check, "mcp-primary-action");
    check.check("another way shows the steps' help link and Back to one click instead of the button", another, "");
    click(check, "mcp-back-to")?;
    check.wait(Duration::from_millis(200));

    // A copy-message app: the message goes to the clipboard first, then the app link opens.
    click(check, "mcp-step-0")?;
    check.wait(Duration::from_millis(200));
    click(check, "mcp-app-grok-bot")?;
    next_to_connect(check, hosted)?;
    check.wait(Duration::from_millis(300));
    check.shot("mcp-11-grok.png")?;
    click(check, "mcp-primary-action")?;
    let copied = check.cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    check.check("Grok Bot copies the message with the URL", copied.as_deref() == Some(format!("Add the MCP server {mcp_url} and connect it.").as_str()), format!("{copied:?}"));
    let opened = check.cx.update(|cx| SharingDialogs::use_in_another_app(cx).and_then(|d| d.read(cx).opened()));
    check.check("then opens the Grok Bot app link", opened.as_deref().is_some_and(|u| u.starts_with("grokbot://app/v1/prompt?text=Add%20the%20MCP")), format!("{opened:?}"));
    check.wait(Duration::from_millis(300));
    check.shot("mcp-12-grok-copied.png")?;

    // A commands app.
    click(check, "mcp-step-0")?;
    click(check, "mcp-app-openclaw")?;
    next_to_connect(check, hosted)?;
    check.wait(Duration::from_millis(300));
    check.shot("mcp-13-openclaw.png")?;
    let commands = (0..4).all(|ix| visible(check, &format!("mcp-command-{ix}-copy"))) && !visible(check, "mcp-primary-action");
    click(check, "mcp-command-1-copy")?;
    let copied = check.cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    check.check(
        "OpenClaw shows its four commands; the second copies the add command with the URL",
        commands && copied.as_deref() == Some(format!("openclaw mcp add t3code --url {mcp_url} --transport streamable-http --auth oauth").as_str()),
        format!("{copied:?}"),
    );

    // Something else: the URL and the general steps only.
    click(check, "mcp-step-0")?;
    click(check, "mcp-app-other")?;
    next_to_connect(check, hosted)?;
    check.wait(Duration::from_millis(300));
    check.shot("mcp-14-other.png")?;
    let other = visible(check, "mcp-copy-URL") && !visible(check, "mcp-primary-action") && !visible(check, "mcp-back-to");
    check.check("Something else shows the URL and steps with no one-click button or way back", other, "");

    // Manage access leaves for Sharing & access.
    click(check, "mcp-manage-access")?;
    check.wait(Duration::from_millis(400));
    let route = check.route();
    let closed = !check.cx.update(|cx| SharingDialogs::is_open(cx));
    check.check(
        "Manage access closes the wizard and opens Sharing & access on this environment",
        closed && format!("{route:?}").contains("Access") && format!("{route:?}").contains(environment_id.as_str()),
        format!("{route:?}"),
    );
    check.shot("mcp-15-manage-access.png")?;
    Ok(())
}
