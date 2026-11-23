//! A ChatGPT sign-in that finished in the browser and came back to the app as
//! a link whose fragment carries the one-time code. Ports
//! `providerAuthDelivery.ts`, `readCodexAuthDelivery` and
//! `providerAuthReturnUrl` from `shared/codexAuthHandoff.ts`.
//!
//! The deep-link handler calls [`read_codex_auth_delivery`] on the incoming
//! address; once the named environment connects, the caller sends
//! `provider.auth.complete` with [`CodexAuthDelivery::complete_input`].

use serde::Deserialize;
use t3code_contracts::{ProviderAuthCompleteInput, ProviderInstanceId};
use url::Url;

/// What the fragment carries, after validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexAuthDelivery {
    pub environment_id: String,
    pub instance_id: String,
    pub flow_id: String,
    pub callback_url: String,
    /// The return address's fragment as the helper saw it.
    pub return_hash: String,
    /// Where the app returns to, sanitized.
    pub return_url: String,
}

impl CodexAuthDelivery {
    /// The `provider.auth.complete` payload.
    pub fn complete_input(&self) -> ProviderAuthCompleteInput {
        ProviderAuthCompleteInput {
            instance_id: ProviderInstanceId::new(self.instance_id.as_str()),
            flow_id: self.flow_id.clone(),
            callback_url: self.callback_url.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Delivery {
    environment_id: String,
    instance_id: String,
    flow_id: String,
    callback_url: String,
    return_hash: String,
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]")
}

/// `ProviderInstanceId`: trimmed, at most 64 characters, a letter then `[a-zA-Z0-9_-]`.
fn valid_instance_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    let ok = trimmed.chars().count() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    ok.then(|| trimmed.to_owned())
}

/// `providerAuthReturnUrl`: the app address to return to, or `None` when it is not one.
pub fn provider_auth_return_url(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let mut url = Url::parse(value).ok()?;
    let host = url.host_str().unwrap_or("");
    let desktop = matches!(url.scheme(), "t3code" | "t3code-dev") && host == "app";
    let web = matches!(url.scheme(), "http" | "https")
        && (is_loopback_host(host) || url.origin().ascii_serialization() == "https://app.t3.codes");
    let path = url.path().to_owned();
    if !url.username().is_empty()
        || url.password().is_some()
        || (!desktop && !web)
        || (path != "/welcome" && path != "/settings" && !path.starts_with("/settings/"))
    {
        return None;
    }
    let all: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let kept: Vec<(String, String)> = all
        .iter()
        .filter(|(key, _)| {
            path != "/welcome" && ["machine", "project", "checkout", "environmentId", "instanceId"].contains(&key.as_str())
        })
        .cloned()
        .collect();
    // `searchParams.delete` reserializes the query only when it removes something.
    if kept.len() != all.len() {
        if kept.is_empty() {
            url.set_query(None);
        } else {
            url.query_pairs_mut().clear().extend_pairs(kept);
        }
    }
    let agents_hash = url.fragment().is_some_and(|f| {
        f.strip_prefix("agents:").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    });
    if path != "/welcome" || !agents_hash {
        url.set_fragment(None);
    }
    Some(url.to_string())
}

/// `readCodexAuthDelivery`: the delivery in a `#codex-auth=` link, or `None`.
pub fn read_codex_auth_delivery(value: &str) -> Option<CodexAuthDelivery> {
    let url = Url::parse(value).ok()?;
    let fragment = url.fragment()?;
    let encoded = fragment.strip_prefix("codex-auth=")?;
    if fragment.len() + 1 > 32_768 {
        return None;
    }
    let json = percent_decode(encoded)?;
    let delivery: Delivery = serde_json::from_str(&json).ok()?;
    let environment_id = Some(delivery.environment_id.trim()).filter(|s| !s.is_empty())?.to_owned();
    let instance_id = valid_instance_id(&delivery.instance_id)?;
    let flow_len = delivery.flow_id.chars().count();
    if flow_len == 0
        || flow_len > 128
        || delivery.callback_url.chars().count() > 16_384
        || delivery.return_hash.chars().count() > 128
    {
        return None;
    }
    let destination = provider_auth_return_url(value)?;
    let mut return_url = Url::parse(&destination).ok()?;
    let hash = delivery.return_hash.strip_prefix('#').unwrap_or(&delivery.return_hash);
    return_url.set_fragment(if hash.is_empty() { None } else { Some(hash) });
    let return_url = provider_auth_return_url(return_url.as_str())?;
    Some(CodexAuthDelivery {
        environment_id,
        instance_id,
        flow_id: delivery.flow_id,
        callback_url: delivery.callback_url,
        return_hash: delivery.return_hash,
        return_url,
    })
}

/// `decodeURIComponent`: `None` on a malformed escape, as the source throws.
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
