//! Authorizing a route: credentials, DPoP renewal, tickets, socket URLs.

pub mod remote;
pub mod service;

use std::sync::Arc;

use url::Url;

pub use remote::{AccessTokenResult, ConnectionMethod, RequestCredential, SubjectToken};
pub use service::{Authorized, RelayAccess, RemoteAuthorization};

use crate::environment::EnvironmentId;
use crate::relay::DpopKey;

/// How the session's DPoP token renews.
#[derive(Clone, Debug)]
pub enum DpopRenewal {
    /// Paired with this key; the host renews it at `/oauth/token`.
    Direct { connection_id: String, http_base_url: Url },
    /// Minted through T3 Connect; renewed by minting again through the relay.
    Relay { environment_id: EnvironmentId },
}

/// The credential a live session uses for HTTP requests to its environment
/// (shell snapshots, history pages). A DPoP value changes as it renews.
#[derive(Clone, Debug)]
pub enum HttpAuthorization {
    Bearer { token: Arc<str> },
    Dpop { access_token: Arc<str>, expires_at_epoch_ms: i64, renewal: DpopRenewal },
}

impl HttpAuthorization {
    /// Adds `authorization`, and for DPoP a proof for exactly `method url`.
    pub fn apply(&self, request: reqwest::RequestBuilder, method: &str, url: &Url, key: &DpopKey) -> reqwest::RequestBuilder {
        self.credential(key).apply(request, method, url)
    }

    pub fn credential<'a>(&'a self, key: &'a DpopKey) -> RequestCredential<'a> {
        match self {
            Self::Bearer { token } => RequestCredential::Bearer(token),
            Self::Dpop { access_token, .. } => RequestCredential::Dpop { access_token, key },
        }
    }

    pub fn expires_at_epoch_ms(&self) -> Option<i64> {
        match self {
            Self::Bearer { .. } => None,
            Self::Dpop { expires_at_epoch_ms, .. } => Some(*expires_at_epoch_ms),
        }
    }
}
