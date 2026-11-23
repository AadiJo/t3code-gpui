//! HTTP calls to an environment's auth API: token exchange, renewal, and
//! WebSocket tickets (spec 01, section 2).

use serde::{Deserialize, Serialize};
use url::Url;

use crate::environment::{endpoint_url, rpc_socket_url};
use crate::platform::ClientPresentation;
use crate::relay::DpopKey;
use crate::rpc::http::{DEFAULT_REQUEST_TIMEOUT, RemoteRequestError, http_client, send_json};

const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const BOOTSTRAP_TOKEN_TYPE: &str = "urn:t3:params:oauth:token-type:environment-bootstrap";
const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";

/// `token_type` of an issued access token.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssuedTokenType {
    Bearer,
    DPoP,
}

/// `POST /oauth/token` success body.
#[derive(Deserialize, Debug, Clone)]
pub struct AccessTokenResult {
    pub access_token: String,
    pub token_type: IssuedTokenType,
    /// Seconds.
    pub expires_in: f64,
    #[serde(default)]
    pub scope: String,
}

impl AccessTokenResult {
    /// Absolute expiry, measured from when the answer arrived.
    pub fn expires_at_epoch_ms(&self) -> i64 {
        now_epoch_ms() + (self.expires_in * 1000.0) as i64
    }
}

pub fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// What the exchange spends: a one-time pairing credential, or the current
/// access token of a DPoP session that renews itself.
#[derive(Clone, Copy, Debug)]
pub enum SubjectToken<'a> {
    Bootstrap(&'a str),
    RenewAccessToken(&'a str),
}

#[derive(Serialize)]
struct TokenExchangeForm<'a> {
    grant_type: &'static str,
    subject_token: &'a str,
    subject_token_type: &'static str,
    requested_token_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_label: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_device_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_os: Option<&'a str>,
}

/// `POST /oauth/token`. With `dpop`, the session is bound to that key.
/// A renewal sends no scope and no client metadata.
pub async fn exchange_token(
    http_base_url: &Url,
    subject: SubjectToken<'_>,
    dpop: Option<&DpopKey>,
    presentation: &ClientPresentation,
) -> Result<AccessTokenResult, RemoteRequestError> {
    let url = endpoint_url(http_base_url, "/oauth/token");
    let (subject_token, subject_token_type, metadata) = match subject {
        SubjectToken::Bootstrap(token) => (token, BOOTSTRAP_TOKEN_TYPE, true),
        SubjectToken::RenewAccessToken(token) => (token, ACCESS_TOKEN_TYPE, false),
    };
    let form = TokenExchangeForm {
        grant_type: GRANT_TYPE,
        subject_token,
        subject_token_type,
        requested_token_type: ACCESS_TOKEN_TYPE,
        client_label: metadata.then_some(presentation.label.as_str()),
        client_device_type: metadata.then_some("desktop"),
        client_os: (metadata && presentation.os != "unknown").then_some(presentation.os),
    };
    let mut request = http_client().post(url.clone()).form(&form);
    if let Some(key) = dpop {
        request = request.header("dpop", key.proof("POST", &url, None));
    }
    send_json(request, DEFAULT_REQUEST_TIMEOUT).await
}

/// `{ticket, expiresAt}`. The ticket lives 5 minutes; mint one per socket.
#[derive(Deserialize, Debug, Clone)]
pub struct WebSocketTicket {
    pub ticket: String,
}

/// How a request authenticates to the environment.
#[derive(Clone, Copy)]
pub enum RequestCredential<'a> {
    Bearer(&'a str),
    Dpop { access_token: &'a str, key: &'a DpopKey },
}

impl RequestCredential<'_> {
    /// Adds `authorization` (and a proof for `method url`) to `request`.
    pub fn apply(self, request: reqwest::RequestBuilder, method: &str, url: &Url) -> reqwest::RequestBuilder {
        match self {
            Self::Bearer(token) => request.header("authorization", format!("Bearer {token}")),
            Self::Dpop { access_token, key } => request
                .header("authorization", format!("DPoP {access_token}"))
                .header("dpop", key.proof(method, url, Some(access_token))),
        }
    }
}

/// `POST /api/auth/websocket-ticket/rpc`, falling back to the bare path on
/// a 404 from servers that predate ticket audiences.
pub async fn issue_rpc_ticket(
    http_base_url: &Url,
    credential: RequestCredential<'_>,
) -> Result<WebSocketTicket, RemoteRequestError> {
    let url = endpoint_url(http_base_url, "/api/auth/websocket-ticket/rpc");
    let request = credential.apply(http_client().post(url.clone()), "POST", &url);
    match send_json(request, DEFAULT_REQUEST_TIMEOUT).await {
        Err(RemoteRequestError::UndeclaredStatus { status: 404, .. }) => {
            let url = endpoint_url(http_base_url, "/api/auth/websocket-ticket");
            let request = credential.apply(http_client().post(url.clone()), "POST", &url);
            send_json(request, DEFAULT_REQUEST_TIMEOUT).await
        }
        result => result,
    }
}

/// How this socket reached the environment, for the server's analytics.
#[derive(Clone, Copy, Debug)]
pub enum ConnectionMethod {
    Direct,
    Relay,
}

/// Mints a ticket and builds the `/ws` URL with it, the client parameters,
/// and for DPoP an upgrade proof whose `ath` hashes the ticket.
pub async fn resolve_socket_url(
    http_base_url: &Url,
    ws_base_url: &Url,
    credential: RequestCredential<'_>,
    presentation: &ClientPresentation,
    method: ConnectionMethod,
) -> Result<Url, RemoteRequestError> {
    let ticket = issue_rpc_ticket(http_base_url, credential).await?.ticket;
    let mut url = rpc_socket_url(ws_base_url);
    let upgrade_proof = match credential {
        RequestCredential::Bearer(_) => None,
        RequestCredential::Dpop { key, .. } => {
            let mut htu = url.clone();
            let scheme = if url.scheme() == "wss" { "https" } else { "http" };
            htu.set_scheme(scheme).expect("http schemes are valid");
            Some(key.proof("GET", &htu, Some(&ticket)))
        }
    };
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("wsTicket", &ticket);
        if let Some(proof) = &upgrade_proof {
            query.append_pair("dpop", proof);
        }
        query
            .append_pair("clientSurface", "desktop")
            .append_pair("clientAppVersion", &presentation.app_version)
            .append_pair("clientDeviceType", "desktop")
            .append_pair("clientOs", presentation.os)
            .append_pair(
                "connectionMethod",
                match method {
                    ConnectionMethod::Direct => "direct",
                    ConnectionMethod::Relay => "relay",
                },
            );
    }
    Ok(url)
}
