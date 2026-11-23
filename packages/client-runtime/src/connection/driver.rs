//! Connects an environment over its routes in priority order (spec 02,
//! section 3.4).

use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::time::Instant;

use super::catalog::{CatalogEntry, ConnectionRoute};
use super::model::{AttemptStage, ConnectionError, TransientReason};
use super::resolver::prepare;
use super::routes::{route_id, route_keeps_its_host, sends_token_in_clear, usable_routes};
use crate::authorization::RemoteAuthorization;
use crate::environment::fetch_descriptor;
use crate::rpc::{FrameTap, RpcSession, RpcSessionOptions};

/// A route check is the public descriptor read with this timeout.
const ROUTE_CHECK_TIMEOUT: Duration = Duration::from_millis(2_500);
/// Preferred routes share this head start. A route that misses it is tried
/// after the others, so a LAN the device left does not delay the relay.
const ROUTE_CHECK_HEAD_START: Duration = Duration::from_millis(500);

/// Shared services a connection attempt needs.
pub struct ConnectionContext {
    pub auth: Arc<RemoteAuthorization>,
    pub session_options: RpcSessionOptions,
    /// Sees every frame of every socket; for transcripts and debugging.
    pub frame_tap: Option<FrameTap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteCheck {
    Answered,
    Silent,
    /// Relay and SSH routes have no address to check.
    Unchecked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutePreflight {
    Ready,
    /// The host refused this device's credential.
    Refused,
    /// The route answered but could not be prepared.
    Unprepared,
    Silent,
    Unchecked,
}

/// A connected session and the routes refused on the way to it.
pub struct EnvironmentLease {
    pub session: Arc<RpcSession>,
    pub refused_route_ids: Vec<String>,
}

/// Reads the descriptor at the route's address. Credential-free, so it is
/// safe against whatever machine answers there now.
pub async fn check_route(auth: &RemoteAuthorization, entry: &CatalogEntry, route: &ConnectionRoute) -> RouteCheck {
    let Some(profile) = route.bearer_profile() else { return RouteCheck::Unchecked };
    let descriptor = fetch_descriptor(&profile.http_base_url, ROUTE_CHECK_TIMEOUT).await.ok().map(Arc::new);
    let answered = descriptor.as_ref().is_some_and(|d| d.environment_id == entry.environment_id);
    auth.remember_checked_descriptor(&profile.http_base_url, descriptor, route_keeps_its_host(route));
    if answered { RouteCheck::Answered } else { RouteCheck::Silent }
}

/// Prepares, opens and synchronizes one route.
async fn connect_route(
    context: &ConnectionContext,
    entry: &CatalogEntry,
    route: &ConnectionRoute,
    progress: &(dyn Fn(AttemptStage) + Send + Sync),
) -> Result<Arc<RpcSession>, ConnectionError> {
    progress(AttemptStage::Preparing);
    let prepared = prepare(&context.auth, &entry.environment_id, &entry.label, route).await?;
    progress(AttemptStage::Opening);
    RpcSession::connect(prepared, &context.session_options, context.frame_tap.clone(), || {
        progress(AttemptStage::Synchronizing)
    })
    .await
}

/// Walks the usable routes in order. With several routes, all are checked
/// at once; routes silent after the shared head start are tried last. Stops
/// at once on an unsupported server. Fails with the first transient error,
/// else the first blocked one.
pub async fn connect(
    context: &ConnectionContext,
    entry: &CatalogEntry,
    progress: &(dyn Fn(AttemptStage) + Send + Sync),
) -> Result<EnvironmentLease, ConnectionError> {
    let routes = usable_routes(entry);
    // Checks run as tasks so they progress while earlier routes connect.
    let checks: Vec<Shared<BoxFuture<'static, RouteCheck>>> = if routes.len() > 1 {
        routes
            .iter()
            .map(|route| {
                let (auth, entry, route) = (context.auth.clone(), entry.clone(), (*route).clone());
                tokio::spawn(async move { check_route(&auth, &entry, &route).await })
                    .map(|joined| joined.unwrap_or(RouteCheck::Silent))
                    .boxed()
                    .shared()
            })
            .collect()
    } else {
        Vec::new()
    };
    let head_start = Instant::now() + ROUTE_CHECK_HEAD_START;
    let mut outcome = Attempts::default();
    let mut silent = Vec::new();
    for (index, route) in routes.iter().enumerate() {
        let check = match checks.get(index) {
            None => RouteCheck::Unchecked,
            Some(check) => tokio::time::timeout_at(head_start, check.clone()).await.unwrap_or(RouteCheck::Silent),
        };
        if check == RouteCheck::Silent {
            silent.push(index);
            continue;
        }
        if let Some(result) = outcome.attempt(context, entry, route, progress).await {
            return outcome.finish(result);
        }
    }
    let mut answered_late = Vec::new();
    let mut still_silent = Vec::new();
    for index in silent {
        match checks[index].clone().await {
            RouteCheck::Silent => still_silent.push(index),
            _ => answered_late.push(index),
        }
    }
    for index in answered_late {
        if let Some(result) = outcome.attempt(context, entry, routes[index], progress).await {
            return outcome.finish(result);
        }
    }
    for index in still_silent {
        // A route silent after its whole check can hold the attempt open far
        // longer; once a retryable failure happened, the retry gets there sooner.
        if outcome.transient.is_some() {
            break;
        }
        if let Some(result) = outcome.attempt(context, entry, routes[index], progress).await {
            return outcome.finish(result);
        }
    }
    Err(outcome.transient.or(outcome.blocked).unwrap_or_else(|| {
        ConnectionError::transient(
            TransientReason::EndpointUnavailable,
            format!("{} did not answer on any saved route.", entry.label),
        )
    }))
}

/// Failures collected while walking the routes.
#[derive(Default)]
struct Attempts {
    transient: Option<ConnectionError>,
    blocked: Option<ConnectionError>,
    refused_route_ids: Vec<String>,
}

impl Attempts {
    /// `Some` ends the walk: a session, or an unsupported server, which is
    /// the same server on every route.
    async fn attempt(
        &mut self,
        context: &ConnectionContext,
        entry: &CatalogEntry,
        route: &ConnectionRoute,
        progress: &(dyn Fn(AttemptStage) + Send + Sync),
    ) -> Option<Result<Arc<RpcSession>, ConnectionError>> {
        match connect_route(context, entry, route, progress).await {
            Ok(session) => Some(Ok(session)),
            Err(error) => {
                if error.is_host_credential_refusal() {
                    self.refused_route_ids.push(route_id(&route.target).to_owned());
                }
                if error.is_unsupported() {
                    return Some(Err(error));
                }
                if error.is_blocked() {
                    self.blocked.get_or_insert(error);
                } else {
                    self.transient.get_or_insert(error);
                }
                None
            }
        }
    }

    fn finish(self, result: Result<Arc<RpcSession>, ConnectionError>) -> Result<EnvironmentLease, ConnectionError> {
        result.map(|session| EnvironmentLease { session, refused_route_ids: self.refused_route_ids })
    }
}

/// Whether an earlier route could take over: it answers and authorizes. A
/// route that answers but refuses the credential never replaces a session.
pub async fn preflight(context: &ConnectionContext, entry: &CatalogEntry, route: &ConnectionRoute) -> RoutePreflight {
    let check = if sends_token_in_clear(&entry.routes, route, None) {
        RouteCheck::Silent
    } else {
        check_route(&context.auth, entry, route).await
    };
    match check {
        RouteCheck::Silent => RoutePreflight::Silent,
        RouteCheck::Unchecked => RoutePreflight::Unchecked,
        RouteCheck::Answered => match prepare(&context.auth, &entry.environment_id, &entry.label, route).await {
            Ok(_) => RoutePreflight::Ready,
            Err(error) if error.is_host_credential_refusal() => RoutePreflight::Refused,
            Err(_) => RoutePreflight::Unprepared,
        },
    }
}
