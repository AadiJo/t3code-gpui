//! The apps the "Use T3 Code in another app" wizard knows (`mcp/mcpApps.ts`): how each one is
//! handed T3 Code's MCP URL, its setup steps and its help page. Every link is https.

use gpui_kit::SharedString;

use crate::i18n::t;

/// How the Connect step hands the MCP URL to an app.
#[derive(Clone, Copy, Debug)]
pub enum McpAppHandoff {
    /// Opens the app's add form with T3 Code filled in.
    Open { link: fn(&str) -> String },
    /// Copies the URL, then opens the page it goes on.
    CopyUrl { link: &'static str },
    /// Copies a message for the app's chat, then opens the app. `app_link` opens the installed
    /// app first, given the message; `link` is the fallback.
    CopyMessage { link: &'static str, app_link: Option<fn(&str) -> String>, message: fn(&str) -> String },
    /// Commands to run where the app runs.
    Commands { commands: fn(&str) -> Vec<String> },
}

/// One app in the grid.
#[derive(Clone, Copy, Debug)]
pub struct McpApp {
    pub id: &'static str,
    pub name: &'static str,
    /// Path under `apps/web/assets`.
    pub logo: &'static str,
    pub handoff: McpAppHandoff,
    /// "Set up another way", next to the URL.
    pub steps: fn() -> Vec<SharedString>,
    /// One line under the primary action.
    pub note: Option<fn() -> SharedString>,
    pub docs: Option<&'static str>,
    /// Kept out of the grid until its sign-in works.
    pub hidden: bool,
}

fn encoded(value: &str) -> String {
    // `encodeURIComponent`: everything but A-Z a-z 0-9 - _ . ! ~ * ' ( ).
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `MCP_APPS`, in grid order.
pub const MCP_APPS: [McpApp; 9] = [
    McpApp {
        id: "poke",
        name: "Poke",
        logo: "mcp/logos/poke.png",
        handoff: McpAppHandoff::Open { link: |url| format!("https://poke.com/integrations/new?name=T3%20Code&url={}", encoded(url)) },
        steps: || vec![t!("Open poke.com/integrations/new."), t!("Paste the URL and add the integration.")],
        note: None,
        docs: Some("https://poke.com/docs/mcp-servers"),
        hidden: false,
    },
    McpApp {
        id: "claude",
        name: "Claude",
        logo: "mcp/logos/claude.png",
        handoff: McpAppHandoff::Open {
            link: |url| {
                format!(
                    "https://claude.ai/customize/connectors?modal=add-custom-connector&connectorName=T3%20Code&connectorUrl={}",
                    encoded(url)
                )
            },
        },
        steps: || {
            vec![
                t!("In Claude, open Customize, then Connectors."),
                t!("Choose Add custom connector and paste the URL."),
                t!("The Free plan allows one custom connector."),
            ]
        },
        note: None,
        docs: Some("https://claude.com/docs/connectors/building/directory-vs-custom"),
        hidden: false,
    },
    McpApp {
        id: "chatgpt",
        name: "ChatGPT",
        logo: "mcp/logos/chatgpt.png",
        handoff: McpAppHandoff::CopyUrl { link: "https://chatgpt.com/plugins" },
        steps: || {
            vec![
                t!("Open chatgpt.com/plugins on the web, then + and Add custom MCP server."),
                t!("Paste the URL. Choose OAuth."),
                t!("Create it as a plugin, then install it."),
            ]
        },
        note: None,
        docs: Some("https://developers.openai.com/api/docs/guides/custom-mcp-server"),
        hidden: false,
    },
    McpApp {
        id: "grok-bot",
        name: "Grok Bot",
        logo: "mcp/logos/grokbot.png",
        // No web chat. The app link prefills the message only into an empty box, so it is copied too.
        handoff: McpAppHandoff::CopyMessage {
            link: "https://x.ai/bot",
            app_link: Some(|message| format!("grokbot://app/v1/prompt?text={}", encoded(message))),
            message: |url| format!("Add the MCP server {url} and connect it."),
        },
        steps: || vec![t!("Send this message to any of your Grok Bots.")],
        note: Some(|| t!("Shared with all your Grok Bots.")),
        docs: Some("https://docs.x.ai/grok-bot/computer-and-apps"),
        hidden: false,
    },
    McpApp {
        id: "gemini",
        name: "Gemini",
        logo: "mcp/logos/gemini.png",
        handoff: McpAppHandoff::CopyUrl { link: "https://gemini.google.com/apps" },
        steps: || vec![t!("In Gemini, open Settings, then Connected apps."), t!("Choose Custom apps, then Add, and paste the URL.")],
        note: None,
        docs: Some("https://support.google.com/gemini/answer/17209137"),
        // Gemini's sign-in and its tool schema checks do not work with T3 Code yet.
        hidden: true,
    },
    McpApp {
        id: "muse",
        name: "Muse",
        logo: "mcp/logos/muse.png",
        handoff: McpAppHandoff::CopyMessage {
            link: "https://muse.ai",
            app_link: None,
            message: |url| format!("Create a custom connector for the MCP server {url}."),
        },
        steps: || vec![t!("Send this message to Muse.")],
        note: None,
        docs: Some("https://www.meta.com/help/artificial-intelligence/1687253048996149/"),
        hidden: false,
    },
    McpApp {
        id: "instinct",
        name: "Instinct",
        logo: "mcp/logos/instinct.png",
        handoff: McpAppHandoff::CopyMessage {
            link: "https://app.instinct.com",
            app_link: None,
            message: |url| format!("Connect the MCP server {url}."),
        },
        steps: || vec![t!("In Instinct, open Settings, then MCP connections, if your account has it.")],
        note: None,
        docs: None,
        hidden: false,
    },
    McpApp {
        id: "openclaw",
        name: "OpenClaw",
        logo: "mcp/logos/openclaw.png",
        handoff: McpAppHandoff::Commands {
            commands: |url| {
                vec![
                    // Its MCP commands refuse to write while the Gateway runs.
                    "openclaw gateway stop".into(),
                    format!("openclaw mcp add t3code --url {url} --transport streamable-http --auth oauth"),
                    "openclaw mcp login t3code".into(),
                    "openclaw gateway start".into(),
                ]
            },
        },
        steps: || {
            vec![
                t!("Open the Control UI, then + and Plugins, and choose Add MCP server."),
                t!("Paste the URL, and choose Streamable HTTP and OAuth."),
                t!("Select Sign in for it under Settings → MCP."),
            ]
        },
        note: None,
        docs: Some("https://docs.openclaw.ai/tools/mcp"),
        hidden: false,
    },
    McpApp {
        id: "hermes-agent",
        name: "Hermes Agent",
        logo: "mcp/logos/hermes.png",
        handoff: McpAppHandoff::Commands {
            // `add` signs in on its first connection (30 s by default, too short to get a code).
            commands: |url| vec![format!("hermes mcp add t3code --url {url} --auth oauth --connect-timeout 315")],
        },
        steps: || {
            vec![
                t!("Add t3code under mcp_servers in ~/.hermes/config.yaml, with the URL and auth: oauth."),
                t!("Run hermes mcp login t3code."),
            ]
        },
        note: Some(|| t!("On a remote host, paste the final URL back when asked.")),
        docs: Some("https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp"),
        hidden: false,
    },
];

/// The "Something else…" choice: the URL and general steps only.
pub fn other_mcp_app_steps() -> Vec<SharedString> {
    vec![t!("Add a remote MCP server with OAuth.")]
}

/// What the primary button copies first.
pub struct HandoffCopy {
    pub text: String,
    pub title: SharedString,
}

/// `mcpHandoffAction`: the primary button's label, the https page it opens, the installed
/// app's link, and what it copies first. `None` for apps set up with commands. It never copies
/// the approval code.
pub struct HandoffAction {
    pub label: SharedString,
    pub link: String,
    pub app_link: Option<String>,
    pub copy: Option<HandoffCopy>,
}

pub fn mcp_handoff_action(app: &McpApp, mcp_url: &str) -> Option<HandoffAction> {
    Some(match app.handoff {
        McpAppHandoff::Open { link } => {
            HandoffAction { label: t!("Open in {app}", app = app.name), link: link(mcp_url), app_link: None, copy: None }
        }
        McpAppHandoff::CopyUrl { link } => HandoffAction {
            label: t!("Copy URL and open {app}", app = app.name),
            link: link.into(),
            app_link: None,
            copy: Some(HandoffCopy { text: mcp_url.into(), title: t!("URL copied") }),
        },
        McpAppHandoff::CopyMessage { link, app_link, message } => {
            let message = message(mcp_url);
            HandoffAction {
                label: t!("Copy message and open {app}", app = app.name),
                link: link.into(),
                app_link: app_link.map(|f| f(&message)),
                copy: Some(HandoffCopy { text: message, title: t!("Message copied") }),
            }
        }
        McpAppHandoff::Commands { .. } => return None,
    })
}
