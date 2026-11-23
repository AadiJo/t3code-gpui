//! Opening a project in an editor when the environment runs on another machine.
//! Ports `remoteOpen.ts` and `buildRemoteOpenUrl` from `contracts/src/editor.ts`:
//! a local environment runs the editor through `shell.openInEditor`; a remote
//! one opens an editor URL on this machine that connects over SSH.

use t3code_contracts::{EditorId, RemoteOpenTargetKind};

use crate::editor_preferences::{EDITORS, remote_scheme_for_editor};

/// The SSH destination an editor URL connects to (`RemoteOpenHost`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteOpenHost {
    /// `None` for a saved SSH alias, else the advertised route kind.
    pub kind: Option<RemoteOpenTargetKind>,
    pub host: String,
    pub username: Option<String>,
}

/// How "Open in editor" reaches an environment (`RemoteOpenState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOpenState {
    /// The server can launch an editor the user sees.
    LocalExec,
    /// Open `vscode://vscode-remote/ssh-remote+…` style links here.
    RemoteLinks(RemoteOpenHost),
    /// No SSH route is known.
    RemoteUnavailable,
}

/// Where an environment's connection points, as far as remote open cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOpenTarget {
    /// This app's own host, or a server on a loopback address.
    Local,
    /// Another machine.
    Remote {
        /// The saved SSH profile's `alias` (`user@host` or `host`) and username.
        ssh_alias: Option<(String, Option<String>)>,
        /// The server's advertised `remoteOpenTargets`, first one wins.
        advertised: Vec<RemoteOpenHost>,
    },
}

/// `resolveRemoteOpenState`.
pub fn resolve_remote_open_state(target: Option<&RemoteOpenTarget>) -> RemoteOpenState {
    let Some(RemoteOpenTarget::Remote { ssh_alias, advertised }) = target else {
        return RemoteOpenState::LocalExec;
    };
    if let Some((alias, username)) = ssh_alias.as_ref().filter(|(alias, _)| !alias.is_empty()) {
        let (user_from_alias, host) = match alias.rfind('@') {
            Some(i) => (Some(&alias[..i]), &alias[i + 1..]),
            None => (None, alias.as_str()),
        };
        let username = username
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .or(user_from_alias)
            .map(str::to_owned);
        return RemoteOpenState::RemoteLinks(RemoteOpenHost { kind: None, host: host.to_owned(), username });
    }
    match advertised.first() {
        Some(host) => RemoteOpenState::RemoteLinks(host.clone()),
        None => RemoteOpenState::RemoteUnavailable,
    }
}

/// `REMOTE_CAPABLE_EDITOR_IDS`: editors with a remote URL scheme.
pub fn remote_capable_editors() -> Vec<EditorId> {
    EDITORS.iter().map(|(id, ..)| *id).filter(|id| remote_scheme_for_editor(*id).is_some()).collect()
}

/// Editors a remote environment offers before the desktop probe answers.
pub const REMOTE_FALLBACK_EDITORS: &[EditorId] = &[EditorId::Vscode];

/// `encodeURIComponent`.
fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `URLSearchParams` value encoding (spaces become `+`).
fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b' ' => out.push('+'),
            b if b.is_ascii_alphanumeric() || b"*-._".contains(&b) => out.push(b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `buildRemoteOpenUrl`: the URL that opens `absolute_path` on `host` in `editor`.
pub fn build_remote_open_url(
    editor: EditorId,
    host: &str,
    username: Option<&str>,
    absolute_path: &str,
    is_file: bool,
) -> Option<String> {
    let scheme = remote_scheme_for_editor(editor)?;
    let jetbrains = EDITORS.iter().find(|(id, ..)| *id == editor).and_then(|(.., code)| *code);
    if let Some(code) = jetbrains {
        let mut params = vec![
            ("h", host.to_owned()),
            ("launchIde", "true".to_owned()),
            ("ideHint", code.to_owned()),
            ("projectHint", absolute_path.replace('\\', "/")),
        ];
        if let Some(user) = username {
            params.push(("u", user.to_owned()));
        }
        let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", encode_query(v))).collect();
        return Some(format!("{scheme}://gateway/ssh/environment?{}", query.join("&")));
    }
    // Windows server paths (`C:\...`) appear as `/C:/...` in vscode-remote URIs.
    let posix = absolute_path.replace('\\', "/");
    let rooted = if posix.starts_with('/') { posix } else { format!("/{posix}") };
    let encoded_host = encode_component(host);
    let encode_path = |path: &str| path.split('/').map(encode_component).collect::<Vec<_>>().join("/");
    if editor == EditorId::Zed {
        // Zed roots paths on the system drive, so the Windows drive prefix goes.
        let b = absolute_path.as_bytes();
        let has_c_drive = b.len() >= 3 && (b[0] == b'C' || b[0] == b'c') && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\');
        let zed_path = if has_c_drive { &rooted[3..] } else { rooted.as_str() };
        let destination = match username {
            Some(user) => format!("{}@{encoded_host}", encode_component(user)),
            None => encoded_host,
        };
        return Some(format!("{scheme}://ssh/{destination}{}", encode_path(zed_path)));
    }
    let plain = match username {
        Some(user) => format!("{user}@{host}"),
        None => host.to_owned(),
    };
    // Remote-SSH splits the authority on "+", so such a destination goes as hex-encoded JSON.
    let destination = if plain.contains('+') {
        let json = match username {
            Some(user) => serde_json::json!({ "hostName": host, "user": user }),
            None => serde_json::json!({ "hostName": host }),
        };
        json.to_string().bytes().map(|b| format!("{b:02x}")).collect()
    } else {
        plain
    };
    Some(format!(
        "{scheme}://vscode-remote/ssh-remote+{}{}{}",
        encode_component(&destination),
        encode_path(&rooted),
        if is_file { ":1:1" } else { "" }
    ))
}
