//! Turns a saved route into an authorized socket URL. Every direct route
//! re-reads the public descriptor before it sends a credential.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use url::Url;

use super::remote::{
    ConnectionMethod, RequestCredential, SubjectToken, exchange_token, now_epoch_ms, resolve_socket_url,
};
use super::{DpopRenewal, HttpAuthorization};
use crate::connection::catalog::ConnectionCredential;
use crate::connection::credential_store::CredentialStore;
use crate::connection::errors::{relay_error, remote_dpop_environment_error, remote_environment_error};
use crate::connection::model::{
    BlockedReason, ConnectionError, ConnectionHop, ConnectionVia, DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS,
    FailureCause, TransientReason,
};
use crate::connection::routes::is_nearby_address;
use crate::environment::{EnvironmentDescriptor, EnvironmentId, fetch_descriptor};
use crate::platform::ClientPresentation;
use crate::relay::{CloudSession, DpopKey, RelayClient, RelayEndpoint};
use crate::rpc::http::{DEFAULT_REQUEST_TIMEOUT, EnvironmentHttpError, RemoteRequestError};

/// A loopback or LAN host answers well inside this.
const NEARBY_DESCRIPTOR_TIMEOUT: Duration = Duration::from_secs(3);
/// A route check's descriptor is reused by the connect it leads to for this long.
const CHECKED_DESCRIPTOR_TTL: Duration = Duration::from_secs(5);

/// The relay and the signed-in account, when T3 Connect is configured.
#[derive(Clone)]
pub struct RelayAccess {
    pub client: Arc<RelayClient>,
    pub session: Arc<dyn CloudSession>,
}

/// A route that passed authorization: open `socket_url` next.
#[derive(Clone, Debug)]
pub struct Authorized {
    pub http_base_url: Url,
    pub socket_url: Url,
    pub http_authorization: HttpAuthorization,
    pub descriptor: Arc<EnvironmentDescriptor>,
    pub via: ConnectionVia,
}

/// A relay-minted environment session for this key.
#[derive(Clone)]
struct RelayEnvironmentToken {
    account_id: String,
    endpoint: RelayEndpoint,
    access_token: Arc<str>,
    expires_at_epoch_ms: i64,
    descriptor: Option<Arc<EnvironmentDescriptor>>,
}

/// Authorizes routes for every environment. One per app.
pub struct RemoteAuthorization {
    presentation: ClientPresentation,
    key: DpopKey,
    credentials: CredentialStore,
    relay: Option<RelayAccess>,
    direct_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    relay_tokens: tokio::sync::Mutex<HashMap<EnvironmentId, RelayEnvironmentToken>>,
    checked: std::sync::Mutex<HashMap<Url, (Arc<EnvironmentDescriptor>, Instant)>>,
}

impl RemoteAuthorization {
    pub fn new(
        presentation: ClientPresentation,
        key: DpopKey,
        credentials: CredentialStore,
        relay: Option<RelayAccess>,
    ) -> Self {
        Self {
            presentation,
            key,
            credentials,
            relay,
            direct_locks: Default::default(),
            relay_tokens: Default::default(),
            checked: Default::default(),
        }
    }

    pub fn key(&self) -> &DpopKey {
        &self.key
    }

    pub fn presentation(&self) -> &ClientPresentation {
        &self.presentation
    }

    pub fn credentials(&self) -> &CredentialStore {
        &self.credentials
    }

    /// Lets the next authorization of `http_base_url` skip its descriptor
    /// read. Kept only for addresses that reach the same machine on every
    /// network (https or loopback); any answer replaces an older one.
    pub fn remember_checked_descriptor(
        &self,
        http_base_url: &Url,
        descriptor: Option<Arc<EnvironmentDescriptor>>,
        keeps_its_host: bool,
    ) {
        let mut checked = self.checked.lock().expect("descriptor cache lock");
        match descriptor {
            Some(descriptor) if keeps_its_host => {
                checked.insert(http_base_url.clone(), (descriptor, Instant::now()));
            }
            _ => {
                checked.remove(http_base_url);
            }
        }
    }

    /// Reads the descriptor and checks it names `expected`. A saved LAN
    /// address can belong to another machine on another network.
    pub async fn verify_endpoint(
        &self,
        expected: &EnvironmentId,
        http_base_url: &Url,
        method: ConnectionMethod,
    ) -> Result<Arc<EnvironmentDescriptor>, ConnectionError> {
        let remembered = self.checked.lock().expect("descriptor cache lock").remove(http_base_url);
        let descriptor = match remembered {
            Some((descriptor, at)) if descriptor.environment_id == *expected && at.elapsed() <= CHECKED_DESCRIPTOR_TTL => {
                descriptor
            }
            _ => {
                let timeout = if matches!(method, ConnectionMethod::Direct) && is_nearby_address(http_base_url) {
                    NEARBY_DESCRIPTOR_TIMEOUT
                } else {
                    DEFAULT_REQUEST_TIMEOUT
                };
                Arc::new(
                    fetch_descriptor(http_base_url, timeout)
                        .await
                        .map_err(|error| remote_environment_error(&error, method))?,
                )
            }
        };
        if descriptor.environment_id != *expected {
            return Err(ConnectionError::environment_mismatch(expected, &descriptor.environment_id));
        }
        Ok(descriptor)
    }

    /// A direct route with a plain bearer token.
    pub async fn authorize_bearer(
        &self,
        expected: &EnvironmentId,
        http_base_url: &Url,
        ws_base_url: &Url,
        token: &str,
    ) -> Result<Authorized, ConnectionError> {
        let descriptor = self.verify_endpoint(expected, http_base_url, ConnectionMethod::Direct).await?;
        let socket_url = resolve_socket_url(
            http_base_url,
            ws_base_url,
            RequestCredential::Bearer(token),
            &self.presentation,
            ConnectionMethod::Direct,
        )
        .await
        .map_err(|error| remote_environment_error(&error, ConnectionMethod::Direct))?;
        Ok(Authorized {
            http_base_url: http_base_url.clone(),
            socket_url,
            http_authorization: HttpAuthorization::Bearer { token: token.into() },
            descriptor,
            via: ConnectionVia::Direct,
        })
    }

    /// A direct route paired with this key. A token the host refuses before
    /// its expiry (a clock jump, say) renews once.
    pub async fn authorize_direct_dpop(
        &self,
        expected: &EnvironmentId,
        connection_id: &str,
        http_base_url: &Url,
        ws_base_url: &Url,
    ) -> Result<Authorized, ConnectionError> {
        let descriptor = self.verify_endpoint(expected, http_base_url, ConnectionMethod::Direct).await?;
        let mut token = self.direct_dpop_token(connection_id, http_base_url, None).await?;
        let mut socket = self.dpop_socket(http_base_url, ws_base_url, &token.0, ConnectionMethod::Direct).await;
        if matches!(&socket, Err(RemoteRequestError::Environment(EnvironmentHttpError::EnvironmentAuthInvalidError { .. }))) {
            token = self.direct_dpop_token(connection_id, http_base_url, Some(&token.0)).await?;
            socket = self.dpop_socket(http_base_url, ws_base_url, &token.0, ConnectionMethod::Direct).await;
        }
        let socket_url = socket.map_err(|error| remote_dpop_environment_error(&error, ConnectionMethod::Direct))?;
        Ok(Authorized {
            http_base_url: http_base_url.clone(),
            socket_url,
            http_authorization: HttpAuthorization::Dpop {
                access_token: token.0.into(),
                expires_at_epoch_ms: token.1,
                renewal: DpopRenewal::Direct {
                    connection_id: connection_id.to_owned(),
                    http_base_url: http_base_url.clone(),
                },
            },
            descriptor,
            via: ConnectionVia::Direct,
        })
    }

    async fn dpop_socket(
        &self,
        http_base_url: &Url,
        ws_base_url: &Url,
        access_token: &str,
        method: ConnectionMethod,
    ) -> Result<Url, RemoteRequestError> {
        let credential = RequestCredential::Dpop { access_token, key: &self.key };
        resolve_socket_url(http_base_url, ws_base_url, credential, &self.presentation, method).await
    }

    /// The stored DPoP credential for `connection_id`, renewed with the host
    /// when it is within a minute of expiry or was just refused. Calls for
    /// one credential share a renewal.
    pub async fn direct_dpop_token(
        &self,
        connection_id: &str,
        http_base_url: &Url,
        rejected: Option<&str>,
    ) -> Result<(String, i64), ConnectionError> {
        let lock = self
            .direct_locks
            .lock()
            .expect("direct lock map")
            .entry(connection_id.to_owned())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        let stored = self.credentials.get(connection_id).await.map_err(credential_store_error)?;
        let Some(ConnectionCredential::Dpop { access_token, expires_at_epoch_ms }) = stored else {
            return Err(credential_missing(connection_id));
        };
        if Some(access_token.as_str()) != rejected
            && expires_at_epoch_ms - DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS > now_epoch_ms()
        {
            return Ok((access_token, expires_at_epoch_ms));
        }
        let renewed = exchange_token(
            http_base_url,
            SubjectToken::RenewAccessToken(&access_token),
            Some(&self.key),
            &self.presentation,
        )
        .await
        .map_err(|error| remote_dpop_environment_error(&error, ConnectionMethod::Direct))?;
        let credential = ConnectionCredential::Dpop {
            access_token: renewed.access_token.clone(),
            expires_at_epoch_ms: renewed.expires_at_epoch_ms(),
        };
        self.credentials.put(connection_id, &credential).await.map_err(credential_store_error)?;
        Ok((renewed.access_token.clone(), renewed.expires_at_epoch_ms()))
    }

    /// A T3 Connect route, or a learned route that borrows its token: the
    /// token is not origin-bound because each proof names its own URL.
    pub async fn authorize_relay(
        &self,
        expected: &EnvironmentId,
        direct_endpoint: Option<(&Url, &Url)>,
    ) -> Result<Authorized, ConnectionError> {
        let direct_descriptor = match direct_endpoint {
            Some((http, _)) => Some(self.verify_endpoint(expected, http, ConnectionMethod::Direct).await?),
            None => None,
        };
        let method = if direct_endpoint.is_some() { ConnectionMethod::Direct } else { ConnectionMethod::Relay };
        let mut token = self.relay_environment_token(expected, None).await?;
        let (mut http, mut ws) = direct_endpoint
            .map(|(http, ws)| (http.clone(), ws.clone()))
            .unwrap_or_else(|| (token.endpoint.http_base_url.clone(), token.endpoint.ws_base_url.clone()));
        let mut socket = self.dpop_socket(&http, &ws, &token.access_token, method).await;
        // A cached token the host refuses is minted again once.
        if matches!(&socket, Err(RemoteRequestError::Environment(EnvironmentHttpError::EnvironmentAuthInvalidError { .. }))) {
            token = self.relay_environment_token(expected, Some(&token.access_token)).await?;
            if direct_endpoint.is_none() {
                http = token.endpoint.http_base_url.clone();
                ws = token.endpoint.ws_base_url.clone();
            }
            socket = self.dpop_socket(&http, &ws, &token.access_token, method).await;
        }
        let socket_url = match socket {
            Ok(url) => url,
            Err(error) => {
                if direct_endpoint.is_none() {
                    self.forget_relay_token(expected, &token.access_token).await;
                }
                return Err(remote_dpop_environment_error(&error, method));
            }
        };
        let descriptor = match direct_descriptor.or(token.descriptor) {
            Some(descriptor) => descriptor,
            None => self.verify_endpoint(expected, &http, ConnectionMethod::Relay).await?,
        };
        Ok(Authorized {
            http_base_url: http,
            socket_url,
            http_authorization: HttpAuthorization::Dpop {
                access_token: token.access_token,
                expires_at_epoch_ms: token.expires_at_epoch_ms,
                renewal: DpopRenewal::Relay { environment_id: expected.clone() },
            },
            descriptor,
            via: if direct_endpoint.is_some() { ConnectionVia::Direct } else { ConnectionVia::Relay },
        })
    }

    async fn forget_relay_token(&self, environment_id: &EnvironmentId, access_token: &str) {
        let mut tokens = self.relay_tokens.lock().await;
        if tokens.get(environment_id).is_some_and(|token| &*token.access_token == access_token) {
            tokens.remove(environment_id);
        }
    }

    /// A cached relay-minted token with more than a minute left, or a new one.
    async fn relay_environment_token(
        &self,
        environment_id: &EnvironmentId,
        rejected: Option<&str>,
    ) -> Result<RelayEnvironmentToken, ConnectionError> {
        let relay = self.relay.as_ref().ok_or_else(|| {
            ConnectionError::blocked(BlockedReason::Configuration, "T3 Connect is not configured in this app.")
                .at(ConnectionHop::Device)
        })?;
        let Some(jwt) = relay.session.relay_jwt().await else {
            return Err(ConnectionError::transient(
                TransientReason::SessionPending,
                "Waiting for the T3 Connect sign-in to load.",
            ));
        };
        let account_id = crate::relay::managed_relay::jwt_subject(&jwt).unwrap_or_default();
        let mut tokens = self.relay_tokens.lock().await;
        if let Some(cached) = tokens.get(environment_id)
            && cached.account_id == account_id
            && Some(&*cached.access_token) != rejected
            && cached.expires_at_epoch_ms > now_epoch_ms() + DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS
        {
            return Ok(RelayEnvironmentToken { descriptor: None, ..cached.clone() });
        }
        tokens.remove(environment_id);
        let minted = self.mint_relay_environment_token(relay, &jwt, environment_id).await?;
        tokens.insert(
            environment_id.clone(),
            RelayEnvironmentToken { account_id, ..minted.clone() },
        );
        Ok(minted)
    }

    /// Relay connect, descriptor check at the tunnel, then the bootstrap
    /// exchange with this key. A host that requires device approval answers
    /// `approval-pending` with the code to compare.
    async fn mint_relay_environment_token(
        &self,
        relay: &RelayAccess,
        jwt: &str,
        environment_id: &EnvironmentId,
    ) -> Result<RelayEnvironmentToken, ConnectionError> {
        let connected = relay
            .client
            .connect_environment(jwt, environment_id, None)
            .await
            .map_err(|error| relay_error(&error))?;
        if connected.environment_id != *environment_id {
            return Err(ConnectionError::environment_mismatch(environment_id, &connected.environment_id));
        }
        let http = &connected.endpoint.http_base_url;
        let descriptor = Arc::new(
            fetch_descriptor(http, DEFAULT_REQUEST_TIMEOUT)
                .await
                .map_err(|error| remote_environment_error(&error, ConnectionMethod::Relay))?,
        );
        if descriptor.environment_id != *environment_id {
            return Err(ConnectionError::environment_mismatch(environment_id, &descriptor.environment_id));
        }
        let access = exchange_token(http, SubjectToken::Bootstrap(&connected.credential), Some(&self.key), &self.presentation)
            .await
            .map_err(|error| match &error {
                RemoteRequestError::Environment(EnvironmentHttpError::EnvironmentConnectApprovalPendingError { trace_id }) => {
                    let code = self.key.verification_code();
                    let mut pending = ConnectionError::transient(
                        TransientReason::ApprovalPending,
                        format!("Approve this device on {}. It shows code {code}.", descriptor.label),
                    )
                    .at(ConnectionHop::Server)
                    .via(ConnectionVia::Relay)
                    .cause(FailureCause::ApprovalPending)
                    .trace(trace_id.as_deref());
                    pending.verification_code = Some(code);
                    pending
                }
                _ => remote_dpop_environment_error(&error, ConnectionMethod::Relay),
            })?;
        Ok(RelayEnvironmentToken {
            account_id: String::new(),
            endpoint: connected.endpoint,
            access_token: access.access_token.as_str().into(),
            expires_at_epoch_ms: access.expires_at_epoch_ms(),
            descriptor: Some(descriptor),
        })
    }

    /// Renews a live session's DPoP token. The server moves the socket to
    /// the renewed session instead of closing it.
    pub async fn renew(&self, current: &HttpAuthorization) -> Result<HttpAuthorization, ConnectionError> {
        let HttpAuthorization::Dpop { renewal, .. } = current else {
            return Ok(current.clone());
        };
        let (access_token, expires_at_epoch_ms) = match renewal {
            DpopRenewal::Direct { connection_id, http_base_url } => {
                let (token, expires) = self.direct_dpop_token(connection_id, http_base_url, None).await?;
                (token.into(), expires)
            }
            DpopRenewal::Relay { environment_id } => {
                let token = self.relay_environment_token(environment_id, None).await?;
                (token.access_token, token.expires_at_epoch_ms)
            }
        };
        Ok(HttpAuthorization::Dpop { access_token, expires_at_epoch_ms, renewal: renewal.clone() })
    }
}

fn credential_missing(connection_id: &str) -> ConnectionError {
    ConnectionError::blocked(
        BlockedReason::Authentication,
        format!("Connection credential {connection_id} is unavailable."),
    )
    .at(ConnectionHop::Device)
}

fn credential_store_error(error: crate::platform::SecretStoreError) -> ConnectionError {
    ConnectionError::blocked(BlockedReason::Configuration, format!("Could not read saved credentials: {error}"))
        .at(ConnectionHop::Device)
}
