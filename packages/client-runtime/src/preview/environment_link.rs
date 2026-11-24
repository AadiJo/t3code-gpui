//! An environment's live session and HTTP credential, for features that
//! reach the server outside the RPC socket: media tickets for the preview
//! stream and tunnel (spec 01, section 6.3), and the session's grants.

use std::sync::Arc;
use std::time::Duration;

use t3code_contracts::{AuthSessionState, EnvironmentId};
use url::Url;

use crate::authorization::RemoteAuthorization;
use crate::authorization::remote::WebSocketTicket;
use crate::connection::supervisor::SupervisorHandle;
use crate::environment::endpoint_url;
use crate::rpc::RpcSession;
use crate::rpc::http::{DEFAULT_REQUEST_TIMEOUT, RemoteRequestError, http_client, send_json};

/// Why a request outside the RPC socket could not be made.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LinkError {
    #[error("This environment is not connected.")]
    NotConnected,
    #[error("{0}")]
    Request(String),
}

impl From<RemoteRequestError> for LinkError {
    fn from(error: RemoteRequestError) -> Self {
        Self::Request(error.to_string())
    }
}

/// One environment's supervisor and the key its requests are signed with.
#[derive(Clone)]
pub struct EnvironmentLink {
    pub environment_id: EnvironmentId,
    pub supervisor: SupervisorHandle,
    pub auth: Arc<RemoteAuthorization>,
}

/// Where media routes live and the ticket that authorizes them. One ticket
/// may be reused until it expires (5 minutes); mint another on a refusal.
#[derive(Clone, Debug)]
pub struct MediaAccess {
    pub http_base: Url,
    pub ticket: String,
}

impl MediaAccess {
    /// `path` on the environment with `query` and the ticket, as http(s).
    pub fn http_url(&self, path: &str, query: &[(&str, &str)]) -> Url {
        let mut url = endpoint_url(&self.http_base, path);
        url.query_pairs_mut().extend_pairs(query).append_pair("wsTicket", &self.ticket);
        url
    }

    /// Like [`Self::http_url`] with a ws(s) scheme.
    pub fn ws_url(&self, path: &str, query: &[(&str, &str)]) -> Url {
        let mut url = self.http_url(path, query);
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme).expect("ws schemes are valid");
        url
    }
}

impl EnvironmentLink {
    /// The live session, if there is one now.
    pub fn current_session(&self) -> Option<Arc<RpcSession>> {
        self.supervisor.session.borrow().clone()
    }

    /// Waits up to `timeout` for a live session.
    pub async fn session(&self, timeout: Duration) -> Result<Arc<RpcSession>, LinkError> {
        let mut sessions = self.supervisor.session.clone();
        let wait = async {
            loop {
                if let Some(session) = sessions.borrow_and_update().clone() {
                    return Ok(session);
                }
                if sessions.changed().await.is_err() {
                    return Err(LinkError::NotConnected);
                }
            }
        };
        tokio::time::timeout(timeout, wait).await.unwrap_or(Err(LinkError::NotConnected))
    }

    fn request(&self, session: &RpcSession, method: reqwest::Method, path: &str) -> (reqwest::RequestBuilder, Url) {
        let url = endpoint_url(&session.prepared.http_base_url, path);
        let authorization = self
            .supervisor
            .http_authorization
            .borrow()
            .clone()
            .unwrap_or_else(|| session.prepared.http_authorization.clone());
        let request = http_client().request(method.clone(), url.clone());
        (authorization.apply(request, method.as_str(), &url, self.auth.key()), url)
    }

    /// `POST /api/auth/websocket-ticket/media`: a bearer ticket the preview
    /// stream, its uploads and downloads, and the tunnel accept in `?wsTicket=`.
    pub async fn media_access(&self) -> Result<MediaAccess, LinkError> {
        let session = self.session(Duration::from_secs(10)).await?;
        let (request, _) = self.request(&session, reqwest::Method::POST, "/api/auth/websocket-ticket/media");
        let ticket: WebSocketTicket = send_json(request, DEFAULT_REQUEST_TIMEOUT).await?;
        Ok(MediaAccess { http_base: session.prepared.http_base_url.clone(), ticket: ticket.ticket })
    }

    /// Sends `body` to a server-relative URL (a signed asset or upload URL, query included)
    /// with the session's HTTP credential, and returns the response bytes.
    pub async fn fetch_relative(
        &self,
        method: reqwest::Method,
        relative_url: &str,
        body: Option<(Vec<u8>, String)>,
    ) -> Result<Vec<u8>, LinkError> {
        let session = self.session(Duration::from_secs(10)).await?;
        let url = session.prepared.http_base_url.join(relative_url).map_err(|error| LinkError::Request(error.to_string()))?;
        let authorization = self
            .supervisor
            .http_authorization
            .borrow()
            .clone()
            .unwrap_or_else(|| session.prepared.http_authorization.clone());
        let mut request = authorization.apply(http_client().request(method.clone(), url.clone()), method.as_str(), &url, self.auth.key());
        if let Some((bytes, content_type)) = body {
            request = request.header(reqwest::header::CONTENT_TYPE, content_type).body(bytes);
        }
        let response = tokio::time::timeout(Duration::from_secs(60), request.send())
            .await
            .map_err(|_| LinkError::Request("the request timed out".into()))?
            .map_err(|error| LinkError::Request(error.to_string()))?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| LinkError::Request(error.to_string()))?;
        if !status.is_success() {
            return Err(LinkError::Request(format!("{status}: {}", String::from_utf8_lossy(&bytes[..bytes.len().min(200)]))));
        }
        Ok(bytes.to_vec())
    }

    /// `GET /api/auth/session`: the session's grants.
    pub async fn session_state(&self) -> Result<AuthSessionState, LinkError> {
        let session = self.session(Duration::from_secs(10)).await?;
        let (request, _) = self.request(&session, reqwest::Method::GET, "/api/auth/session");
        Ok(send_json(request, DEFAULT_REQUEST_TIMEOUT).await?)
    }
}
