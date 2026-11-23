//! T3 Connect sign-in for a native app: Clerk's Frontend API (FAPI) in
//! "native" mode, the way `@clerk/electron` drives it, with an email code as
//! the first factor (spec 02, section 4.2).
//!
//! Clerk only allowlists the Electron app's `t3code://app/` OAuth redirect,
//! so a browser round trip would have to take over that scheme. An email
//! code needs no redirect. Every FAPI response may carry a rotated client
//! JWT in its `Authorization` header; it replaces the stored one at once.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, watch};
use url::Url;

use super::managed_relay::CloudSession;
use crate::platform::{SecretStore, SecretStoreError};
use crate::rpc::http::{DEFAULT_REQUEST_TIMEOUT, http_client};

/// Secret store key for the rotating Clerk client JWT.
const CLIENT_JWT_SECRET: &str = "clerk-client-jwt";

/// Public T3 Connect configuration. All values are public (repo `.env.example`).
#[derive(Clone, Debug)]
pub struct CloudConfig {
    pub relay_url: String,
    pub clerk_publishable_key: String,
    pub jwt_template: String,
}

impl CloudConfig {
    /// The production deployment.
    pub fn production() -> Self {
        Self {
            relay_url: "https://relay.t3.codes".into(),
            clerk_publishable_key: "pk_live_Y2xlcmsudDMuY29kZXMk".into(),
            jwt_template: "t3-relay".into(),
        }
    }
}

/// The FAPI host encoded in a publishable key: `pk_live_<base64("host$")>`.
pub fn frontend_api_host(publishable_key: &str) -> Option<String> {
    let encoded = publishable_key.trim().strip_prefix("pk_live_").or_else(|| publishable_key.trim().strip_prefix("pk_test_"))?;
    let decoded = STANDARD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded.trim_end_matches('=')))
        .ok()?;
    let host = String::from_utf8(decoded).ok()?;
    let host = host.strip_suffix('$')?;
    (!host.is_empty() && !host.contains('/')).then(|| host.to_owned())
}

/// The signed-in Clerk user, as the settings UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudAccount {
    pub user_id: String,
    pub session_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// Whether there is a T3 Connect sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloudAccountState {
    /// The stored session has not been checked yet.
    Loading,
    SignedOut,
    SignedIn(CloudAccount),
}

/// A sign-in waiting for the code Clerk emailed.
#[derive(Clone, Debug)]
pub struct EmailCodeSignIn {
    pub sign_in_id: String,
    pub email: String,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ClerkError {
    /// Clerk refused the request; `message` is Clerk's English text.
    #[error("{message}")]
    Rejected { code: String, message: String },
    #[error("T3 Connect sign-in did not answer: {0}")]
    Request(String),
    #[error("T3 Connect sign-in sent an answer this app could not read: {0}")]
    Decode(String),
    #[error("could not store the T3 Connect sign-in: {0}")]
    Store(String),
    #[error("the T3 Connect configuration has no valid Clerk key")]
    InvalidConfig,
    #[error("the sign-in needs a step this app does not support ({0})")]
    Unsupported(String),
}

impl From<SecretStoreError> for ClerkError {
    fn from(error: SecretStoreError) -> Self {
        Self::Store(error.to_string())
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    response: Option<T>,
    client: Option<ClientResource>,
}

#[derive(Deserialize)]
struct ErrorBody {
    errors: Vec<ErrorItem>,
}

#[derive(Deserialize)]
struct ErrorItem {
    code: String,
    #[serde(default)]
    message: String,
    long_message: Option<String>,
}

#[derive(Deserialize, Clone)]
struct ClientResource {
    #[serde(default)]
    sessions: Vec<SessionResource>,
    last_active_session_id: Option<String>,
}

#[derive(Deserialize, Clone)]
struct SessionResource {
    id: String,
    status: String,
    user: Option<UserResource>,
}

#[derive(Deserialize, Clone)]
struct UserResource {
    id: String,
    first_name: Option<String>,
    last_name: Option<String>,
    primary_email_address_id: Option<String>,
    #[serde(default)]
    email_addresses: Vec<EmailAddressResource>,
}

#[derive(Deserialize, Clone)]
struct EmailAddressResource {
    id: String,
    email_address: String,
}

#[derive(Deserialize)]
struct SignInResource {
    id: String,
    status: String,
    #[serde(default)]
    supported_first_factors: Vec<FactorResource>,
    created_session_id: Option<String>,
}

#[derive(Deserialize)]
struct FactorResource {
    strategy: String,
    email_address_id: Option<String>,
}

#[derive(Deserialize)]
struct TokenResource {
    jwt: String,
}

/// The token endpoint answers either bare or inside the usual envelope.
#[derive(Deserialize)]
#[serde(untagged)]
enum TokenAnswer {
    Wrapped { response: TokenResource },
    Bare(TokenResource),
}

impl ClientResource {
    /// The active session as an account, or `None` when signed out.
    fn account(&self) -> Option<CloudAccount> {
        let session = self
            .sessions
            .iter()
            .filter(|session| session.status == "active")
            .find(|session| Some(&session.id) == self.last_active_session_id.as_ref())
            .or_else(|| self.sessions.iter().find(|session| session.status == "active"))?;
        let user = session.user.as_ref()?;
        let email = user
            .email_addresses
            .iter()
            .find(|address| Some(&address.id) == user.primary_email_address_id.as_ref())
            .or(user.email_addresses.first())
            .map(|address| address.email_address.clone());
        let name = [user.first_name.as_deref(), user.last_name.as_deref()]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Some(CloudAccount {
            user_id: user.id.clone(),
            session_id: session.id.clone(),
            email,
            name: (!name.is_empty()).then_some(name),
        })
    }
}

/// This install's own Clerk client. One per app; implements [`CloudSession`]
/// so the relay client can mint tokens with it.
pub struct ClerkSession {
    fapi: Url,
    template: String,
    secrets: Arc<dyn SecretStore>,
    /// The rotating client JWT. Held across each request so rotations never race.
    client_jwt: Mutex<Option<String>>,
    account: watch::Sender<CloudAccountState>,
}

impl ClerkSession {
    pub fn new(config: &CloudConfig, secrets: Arc<dyn SecretStore>) -> Result<Self, ClerkError> {
        let host = frontend_api_host(&config.clerk_publishable_key).ok_or(ClerkError::InvalidConfig)?;
        let fapi = Url::parse(&format!("https://{host}/")).map_err(|_| ClerkError::InvalidConfig)?;
        Ok(Self {
            fapi,
            template: config.jwt_template.clone(),
            secrets,
            client_jwt: Mutex::new(None),
            account: watch::channel(CloudAccountState::Loading).0,
        })
    }

    /// The account state; starts `Loading` until [`Self::restore`] runs.
    pub fn account(&self) -> watch::Receiver<CloudAccountState> {
        self.account.subscribe()
    }

    /// Loads the stored client JWT and asks Clerk for its sessions.
    pub async fn restore(&self) -> Result<(), ClerkError> {
        let stored = self.read_secret().await?;
        *self.client_jwt.lock().await = stored;
        self.refresh().await
    }

    /// Re-reads the client from Clerk. A 401 or no session means signed out.
    pub async fn refresh(&self) -> Result<(), ClerkError> {
        let mut jwt = self.client_jwt.lock().await;
        if jwt.is_none() {
            self.account.send_replace(CloudAccountState::SignedOut);
            return Ok(());
        }
        let result: Result<Envelope<ClientResource>, ClerkError> =
            self.request(&mut jwt, reqwest::Method::GET, "v1/client", &[]).await;
        drop(jwt);
        match result {
            Ok(envelope) => {
                let account = envelope.response.as_ref().and_then(ClientResource::account);
                self.publish(account);
                Ok(())
            }
            Err(ClerkError::Rejected { code, .. }) if code == "unauthorized" => {
                self.forget().await?;
                Ok(())
            }
            Err(error) => {
                // Keep a stored session through a network failure; it may load later.
                if matches!(*self.account.borrow(), CloudAccountState::Loading) {
                    self.account.send_replace(CloudAccountState::SignedOut);
                }
                Err(error)
            }
        }
    }

    /// Step one: finds the account for `email` and has Clerk email it a code.
    pub async fn start_email_code(&self, email: &str) -> Result<EmailCodeSignIn, ClerkError> {
        let email = email.trim();
        let mut jwt = self.client_jwt.lock().await;
        if jwt.is_none() {
            // A fresh native client; its JWT comes back in the Authorization header.
            let _: Envelope<ClientResource> = self.request(&mut jwt, reqwest::Method::POST, "v1/client", &[]).await?;
        }
        let created: Envelope<SignInResource> = self
            .request(&mut jwt, reqwest::Method::POST, "v1/client/sign_ins", &[("identifier", email)])
            .await?;
        let sign_in = created.response.ok_or_else(|| ClerkError::Decode("no sign-in in the answer".into()))?;
        let factor = sign_in
            .supported_first_factors
            .iter()
            .find(|factor| factor.strategy == "email_code")
            .ok_or_else(|| ClerkError::Unsupported(sign_in.status.clone()))?;
        let email_address_id = factor.email_address_id.clone().unwrap_or_default();
        let path = format!("v1/client/sign_ins/{}/prepare_first_factor", sign_in.id);
        let _: Envelope<SignInResource> = self
            .request(
                &mut jwt,
                reqwest::Method::POST,
                &path,
                &[("strategy", "email_code"), ("email_address_id", &email_address_id)],
            )
            .await?;
        Ok(EmailCodeSignIn { sign_in_id: sign_in.id, email: email.to_owned() })
    }

    /// Step two: sends the emailed code. On success the account is signed in.
    pub async fn verify_email_code(&self, sign_in: &EmailCodeSignIn, code: &str) -> Result<CloudAccount, ClerkError> {
        let mut jwt = self.client_jwt.lock().await;
        let path = format!("v1/client/sign_ins/{}/attempt_first_factor", sign_in.sign_in_id);
        let attempted: Envelope<SignInResource> = self
            .request(&mut jwt, reqwest::Method::POST, &path, &[("strategy", "email_code"), ("code", code.trim())])
            .await?;
        let status = attempted.response.as_ref().map(|s| s.status.clone()).unwrap_or_default();
        if status != "complete" {
            return Err(ClerkError::Unsupported(status));
        }
        let session_id = attempted.response.and_then(|s| s.created_session_id);
        let account = attempted
            .client
            .as_ref()
            .and_then(ClientResource::account)
            .filter(|account| session_id.as_ref().is_none_or(|id| *id == account.session_id))
            .ok_or_else(|| ClerkError::Decode("the completed sign-in has no active session".into()))?;
        drop(jwt);
        self.publish(Some(account.clone()));
        Ok(account)
    }

    /// Ends the session with Clerk and forgets the client here. The local
    /// sign-out happens even when Clerk cannot be reached.
    pub async fn sign_out(&self) -> Result<(), ClerkError> {
        let session_id = match &*self.account.borrow() {
            CloudAccountState::SignedIn(account) => Some(account.session_id.clone()),
            _ => None,
        };
        if let Some(session_id) = session_id {
            let mut jwt = self.client_jwt.lock().await;
            let path = format!("v1/client/sessions/{session_id}/remove");
            let result: Result<Envelope<SessionResource>, ClerkError> =
                self.request(&mut jwt, reqwest::Method::POST, &path, &[]).await;
            if let Err(error) = result {
                tracing::warn!(%error, "could not end the Clerk session; forgetting it here");
            }
        }
        self.forget().await
    }

    /// Adopts a client JWT taken from the Electron app (spec 02, section 1.3).
    /// Both apps then share one rotating Clerk client, which can sign one out.
    pub async fn adopt_client_jwt(&self, client_jwt: String) -> Result<(), ClerkError> {
        self.write_secret(Some(&client_jwt)).await?;
        *self.client_jwt.lock().await = Some(client_jwt);
        self.refresh().await
    }

    /// A fresh `t3-relay` template JWT, or `None` while signed out.
    pub async fn template_token(&self) -> Result<Option<String>, ClerkError> {
        let session_id = match &*self.account.borrow() {
            CloudAccountState::SignedIn(account) => account.session_id.clone(),
            _ => return Ok(None),
        };
        let mut jwt = self.client_jwt.lock().await;
        let path = format!("v1/client/sessions/{session_id}/tokens/{}", self.template);
        let result: Result<TokenAnswer, ClerkError> =
            self.request_raw(&mut jwt, reqwest::Method::POST, &path, &[]).await;
        drop(jwt);
        match result {
            Ok(TokenAnswer::Wrapped { response } | TokenAnswer::Bare(response)) => Ok(Some(response.jwt)),
            Err(ClerkError::Rejected { code, .. }) if code == "unauthorized" => {
                self.forget().await?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn publish(&self, account: Option<CloudAccount>) {
        self.account.send_replace(match account {
            Some(account) => CloudAccountState::SignedIn(account),
            None => CloudAccountState::SignedOut,
        });
    }

    async fn forget(&self) -> Result<(), ClerkError> {
        *self.client_jwt.lock().await = None;
        self.write_secret(None).await?;
        self.account.send_replace(CloudAccountState::SignedOut);
        Ok(())
    }

    async fn read_secret(&self) -> Result<Option<String>, ClerkError> {
        let secrets = self.secrets.clone();
        let bytes = tokio::task::spawn_blocking(move || secrets.get(CLIENT_JWT_SECRET))
            .await
            .map_err(|error| ClerkError::Store(error.to_string()))??;
        Ok(bytes.and_then(|bytes| String::from_utf8(bytes).ok()))
    }

    async fn write_secret(&self, value: Option<&str>) -> Result<(), ClerkError> {
        let secrets = self.secrets.clone();
        let value = value.map(str::to_owned);
        tokio::task::spawn_blocking(move || match value {
            Some(value) => secrets.set(CLIENT_JWT_SECRET, value.as_bytes()),
            None => secrets.delete(CLIENT_JWT_SECRET),
        })
        .await
        .map_err(|error| ClerkError::Store(error.to_string()))??;
        Ok(())
    }

    /// A FAPI call whose body is the `{ response, client }` envelope.
    async fn request<T: DeserializeOwned>(
        &self,
        jwt: &mut Option<String>,
        method: reqwest::Method,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<Envelope<T>, ClerkError> {
        self.request_raw(jwt, method, path, form).await
    }

    /// One FAPI call in native mode. Saves a rotated client JWT before
    /// looking at the status, since Clerk rotates it on errors too.
    async fn request_raw<T: DeserializeOwned>(
        &self,
        jwt: &mut Option<String>,
        method: reqwest::Method,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<T, ClerkError> {
        let mut url = self.fapi.join(path).map_err(|error| ClerkError::Request(error.to_string()))?;
        url.query_pairs_mut().append_pair("_is_native", "1");
        let mut request = http_client().request(method.clone(), url);
        if let Some(token) = jwt.as_deref() {
            request = request.header(reqwest::header::AUTHORIZATION, token);
        }
        if method != reqwest::Method::GET {
            request = request.form(form);
        }
        let response = tokio::time::timeout(DEFAULT_REQUEST_TIMEOUT, request.send())
            .await
            .map_err(|_| ClerkError::Request("timed out".into()))?
            .map_err(|error| ClerkError::Request(error.to_string()))?;
        let rotated = response
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if let Some(rotated) = rotated
            && jwt.as_deref() != Some(rotated.as_str())
        {
            self.write_secret(Some(&rotated)).await?;
            *jwt = Some(rotated);
        }
        let status = response.status();
        let body = tokio::time::timeout(DEFAULT_REQUEST_TIMEOUT, response.bytes())
            .await
            .map_err(|_| ClerkError::Request("timed out".into()))?
            .map_err(|error| ClerkError::Request(error.to_string()))?;
        if status.is_success() {
            return serde_json::from_slice(&body).map_err(|error| ClerkError::Decode(error.to_string()));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ClerkError::Rejected { code: "unauthorized".into(), message: "Signed out".into() });
        }
        match serde_json::from_slice::<ErrorBody>(&body).ok().and_then(|body| body.errors.into_iter().next()) {
            Some(error) => Err(ClerkError::Rejected {
                message: error.long_message.filter(|m| !m.is_empty()).unwrap_or(error.message),
                code: error.code,
            }),
            None => Err(ClerkError::Request(format!("HTTP {status}"))),
        }
    }
}

impl CloudSession for ClerkSession {
    fn relay_jwt(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            match self.template_token().await {
                Ok(token) => token,
                Err(error) => {
                    tracing::warn!(%error, "could not get a T3 Connect token");
                    None
                }
            }
        })
    }
}
