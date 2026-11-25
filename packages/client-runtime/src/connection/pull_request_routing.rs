//! Pull request sharing between this device's machines. Ports
//! `connection/pullRequestRoutingPermissions.ts` and `state/pullRequestRouting.ts`.
//!
//! A machine the user trusts here may answer another machine's pull request reads (and, with
//! "Read and act" on both, its writes) through its own source control access. Trust belongs to
//! the saved endpoints, never to an environment id a server advertises: each grant stores the
//! connection key of the routes it was given for, and a grant whose key no longer matches is off.
//! Credentials stay on their machines; only a host and account verified on both sides cross.
//!
//! [`PullRequestRouting::request`] runs one pull request RPC for its origin machine: when another
//! connected machine is allowed and reports the same host and account, it may answer instead,
//! local machines first; reads fall back along the candidates and finally to the origin.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use t3code_contracts::rpc::RpcMethod;
use t3code_contracts::rpc::methods::*;
use t3code_contracts::{
    PullRequestInvalidateInput, PullRequestRef, PullRequestReportStateInput, PullRequestRoutingIdentityInput, PullRequestRoutingResult,
    PullRequestState,
};
use tokio::sync::watch;

use super::catalog::{CatalogEntry, ConnectionProfile, TargetRoute};
use super::registry::ConnectionRegistry;
use crate::environment::EnvironmentId;
use crate::rpc::client::RpcError;
use crate::rpc::RpcSession;

/// How far a machine is trusted with pull requests (`PullRequestRoutingPermission`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullRequestRoutingPermission {
    #[default]
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "read-write")]
    ReadWrite,
}

/// One grant as stored (`StoredPullRequestRoutingPermission`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPullRequestRoutingPermission {
    pub environment_id: EnvironmentId,
    pub connection_key: String,
    pub permission: PullRequestRoutingPermission,
}

/// The key a grant is bound to (`pullRequestRoutingConnectionKey`): the saved endpoints of the
/// entry's routes, learned routes left out. With several routes the key covers all of them,
/// sorted, so reordering keeps trust but adding or changing an address revokes it. `None` for a
/// route with no saved endpoint. The JSON matches the web's, so grants made there still match.
pub fn connection_key(entry: &CatalogEntry) -> Option<String> {
    let routes: Vec<_> = entry
        .routes
        .iter()
        .filter(|route| !matches!(&route.profile, Some(ConnectionProfile::Bearer(profile)) if profile.learned))
        .collect();
    if let [route] = routes.as_slice() {
        return route_key(entry, route);
    }
    let mut keys = routes.iter().map(|route| route_key(entry, route)).collect::<Option<Vec<_>>>()?;
    keys.sort();
    serde_json::to_string(&keys).ok()
}

fn route_key(entry: &CatalogEntry, route: &super::catalog::ConnectionRoute) -> Option<String> {
    use serde_json::Value;
    let id = Value::from(entry.environment_id.as_str());
    let parts: Vec<Value> = match (&route.target.route, &route.profile) {
        (TargetRoute::Relay, _) => vec!["RelayConnectionTarget".into(), id],
        (TargetRoute::Ssh { .. }, Some(ConnectionProfile::Ssh(profile))) => {
            let target = &profile.target;
            vec![
                "SshConnectionTarget".into(),
                id,
                target.alias.clone().into(),
                target.hostname.clone().into(),
                target.username.clone().map_or(Value::Null, Value::from),
                target.port.map_or(Value::Null, Value::from),
            ]
        }
        (TargetRoute::Bearer { .. }, Some(ConnectionProfile::Bearer(profile))) => {
            let (http, ws) = (&profile.http_base_url, &profile.ws_base_url);
            let ok = matches!(http.scheme(), "http" | "https")
                && matches!(ws.scheme(), "ws" | "wss")
                && [http, ws].iter().all(|url| url.username().is_empty() && url.password().is_none());
            if !ok {
                return None;
            }
            vec![
                "BearerConnectionTarget".into(),
                id,
                http.as_str().trim_end_matches('/').into(),
                ws.as_str().trim_end_matches('/').into(),
            ]
        }
        _ => return None,
    };
    serde_json::to_string(&parts).ok()
}

/// The entry's permission under `permissions` (`pullRequestRoutingPermissionFor`): off unless a
/// grant matches its connection key.
pub fn permission_for(entry: &CatalogEntry, permissions: &[StoredPullRequestRoutingPermission]) -> PullRequestRoutingPermission {
    connection_key(entry)
        .and_then(|key| permissions.iter().find(|p| p.connection_key == key).map(|p| p.permission))
        .unwrap_or_default()
}

/// What a routed method does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteKind {
    Read,
    Write,
}

/// An input that names one pull request (`isRef`): every routed method's payload.
pub trait PullRequestRefInput: Clone + Send + 'static {
    fn reference(&self) -> PullRequestRef;
    /// Pins the call to a verified host and account (`host`, `expectedAccountId`).
    fn route_to(&mut self, host: &str, account_id: &str);
    fn allow_stale(&self) -> Option<bool>;
    fn set_allow_stale(&mut self, allow: Option<bool>);
}

macro_rules! ref_input {
    ($($ty:ty),* $(,)?) => {$(
        impl PullRequestRefInput for $ty {
            fn reference(&self) -> PullRequestRef {
                PullRequestRef {
                    project_id: self.project_id.clone(),
                    host: self.host.clone(),
                    expected_account_id: self.expected_account_id.clone(),
                    allow_stale: self.allow_stale,
                    repository: self.repository.clone(),
                    number: self.number.clone(),
                }
            }
            fn route_to(&mut self, host: &str, account_id: &str) {
                self.host = Some(host.to_owned());
                self.expected_account_id = Some(account_id.to_owned());
            }
            fn allow_stale(&self) -> Option<bool> {
                self.allow_stale
            }
            fn set_allow_stale(&mut self, allow: Option<bool>) {
                self.allow_stale = allow;
            }
        }
    )*};
}
ref_input!(
    PullRequestRef,
    t3code_contracts::PullRequestThreadCommentsInput,
    t3code_contracts::PullRequestDiffFileContentsInput,
    t3code_contracts::PullRequestSetFilesViewedInput,
    t3code_contracts::PullRequestActionInput,
    t3code_contracts::PullRequestUpdateInput,
    t3code_contracts::PullRequestCommentInput,
    t3code_contracts::PullRequestCommentUpdateInput,
    t3code_contracts::PullRequestSubmitReviewInput,
    t3code_contracts::PullRequestThreadReplyInput,
    t3code_contracts::PullRequestThreadResolutionInput,
    t3code_contracts::PullRequestReactionInput,
    t3code_contracts::PullRequestReviewerCandidatesInput,
    t3code_contracts::PullRequestReviewerRequestInput,
    t3code_contracts::PullRequestLabelChangeInput,
);

/// A routed answer: the pull request state it saw, and the project fields the origin owns.
pub trait RoutedResult: Send + 'static {
    /// The state and draft flag, for `pullRequests.reportState`.
    fn state(&self) -> Option<(PullRequestState, Option<bool>)> {
        None
    }
    /// Puts back the origin's project (`projectId`, and for a detail its title and root).
    fn adopt_project(&mut self, _reference: &PullRequestRef, _identity: &PullRequestRoutingResult) {}
}

macro_rules! plain_result {
    ($($ty:ty),* $(,)?) => {$( impl RoutedResult for $ty {} )*};
}
plain_result!(
    (),
    t3code_contracts::PullRequestActivity,
    t3code_contracts::PullRequestChangeBreakdown,
    t3code_contracts::PullRequestFilesViewedResult,
    t3code_contracts::PullRequestThreadCommentsResult,
    t3code_contracts::PullRequestDiffFileContentsResult,
    t3code_contracts::PullRequestReviewerCandidateList,
    t3code_contracts::PullRequestLabelCandidateList,
    Option<t3code_contracts::PullRequestStack>,
);

impl RoutedResult for t3code_contracts::PullRequestSummary {
    fn state(&self) -> Option<(PullRequestState, Option<bool>)> {
        Some((self.state.clone(), self.is_draft))
    }
    fn adopt_project(&mut self, reference: &PullRequestRef, _: &PullRequestRoutingResult) {
        self.project_id = reference.project_id.clone();
    }
}

impl RoutedResult for t3code_contracts::PullRequestPreview {
    fn state(&self) -> Option<(PullRequestState, Option<bool>)> {
        Some((self.state.clone(), Some(self.is_draft)))
    }
    fn adopt_project(&mut self, reference: &PullRequestRef, _: &PullRequestRoutingResult) {
        self.project_id = reference.project_id.clone();
    }
}

impl RoutedResult for t3code_contracts::PullRequestDetail {
    fn state(&self) -> Option<(PullRequestState, Option<bool>)> {
        Some((self.state.clone(), Some(self.is_draft)))
    }
    fn adopt_project(&mut self, reference: &PullRequestRef, identity: &PullRequestRoutingResult) {
        self.project_id = reference.project_id.clone();
        self.project_title = identity.project_title.clone();
        self.workspace_root = identity.workspace_root.clone();
    }
}

impl RoutedResult for Option<t3code_contracts::PullRequestSummary> {
    fn state(&self) -> Option<(PullRequestState, Option<bool>)> {
        self.as_ref().and_then(RoutedResult::state)
    }
    fn adopt_project(&mut self, reference: &PullRequestRef, identity: &PullRequestRoutingResult) {
        if let Some(summary) = self {
            summary.adopt_project(reference, identity);
        }
    }
}

impl RoutedResult for Option<t3code_contracts::PullRequestChecks> {
    fn state(&self) -> Option<(PullRequestState, Option<bool>)> {
        self.as_ref().map(|checks| (checks.state.clone(), None))
    }
}

/// A pull request method the router may send elsewhere.
pub trait RoutedMethod: RpcMethod<Payload: PullRequestRefInput, Success: RoutedResult, Error: std::fmt::Debug> {
    const KIND: RouteKind;
}

macro_rules! routed {
    ($kind:ident: $($method:ty),* $(,)?) => {$( impl RoutedMethod for $method { const KIND: RouteKind = RouteKind::$kind; } )*};
}
routed!(Read:
    PullRequestsSummary, PullRequestsStack, PullRequestsDetail, PullRequestsPreview, PullRequestsChecks,
    PullRequestsActivity, PullRequestsThreadComments, PullRequestsDiffFileContents, PullRequestsFilesViewed,
    PullRequestsChangeBreakdown, PullRequestsReviewerCandidates, PullRequestsLabelCandidates,
);
// Review guides are not routed: each server keeps its own.
routed!(Write:
    PullRequestsSetFilesViewed, PullRequestsRunAction, PullRequestsUpdate, PullRequestsComment,
    PullRequestsUpdateComment, PullRequestsSubmitReview, PullRequestsReplyToThread, PullRequestsSetReaction,
    PullRequestsSetThreadResolution, PullRequestsRequestReviewers, PullRequestsSetLabels,
);

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const INVALIDATE_TIMEOUT: Duration = Duration::from_secs(1);
const REPORT_TIMEOUT: Duration = Duration::from_secs(5);
const ROUTED_READS_LIMIT: usize = 256;

/// Where reads of one pull request went, so a write can tell their readers (`RoutedRead`).
struct RoutedRead {
    origin: EnvironmentId,
    reference: PullRequestRef,
    targets: HashSet<EnvironmentId>,
}

/// The routing state and its requests. See the module docs.
pub struct PullRequestRouting {
    registry: ConnectionRegistry,
    permissions: watch::Sender<Arc<[StoredPullRequestRoutingPermission]>>,
    /// Reads answered elsewhere, oldest first, by origin and pull request.
    used: Mutex<Vec<(String, RoutedRead)>>,
}

static ROUTING: OnceLock<Arc<PullRequestRouting>> = OnceLock::new();

impl PullRequestRouting {
    /// Starts routing over `registry` with the stored grants; later calls return the first.
    pub fn install(registry: ConnectionRegistry, stored: Vec<StoredPullRequestRoutingPermission>) -> Arc<Self> {
        ROUTING
            .get_or_init(|| {
                Arc::new(Self { registry, permissions: watch::channel(Arc::from(stored)).0, used: Mutex::new(Vec::new()) })
            })
            .clone()
    }

    /// The installed routing, if any.
    pub fn global() -> Option<Arc<Self>> {
        ROUTING.get().cloned()
    }

    /// Follows the grants.
    pub fn permissions(&self) -> watch::Receiver<Arc<[StoredPullRequestRoutingPermission]>> {
        self.permissions.subscribe()
    }

    fn entry(&self, environment_id: &EnvironmentId) -> Option<CatalogEntry> {
        self.registry.entries().borrow().iter().find(|e| &e.environment_id == environment_id).cloned()
    }

    /// Grants `permission` to the environment's saved endpoints (`set`); off removes the grant.
    /// Returns the grants to persist, or `None` when the entry has no saved endpoint.
    pub fn set(&self, environment_id: &EnvironmentId, permission: PullRequestRoutingPermission) -> Option<Vec<StoredPullRequestRoutingPermission>> {
        let entry = self.entry(environment_id)?;
        let connection_key = connection_key(&entry)?;
        let mut next: Vec<_> = self.permissions.borrow().iter().filter(|p| &p.environment_id != environment_id).cloned().collect();
        if permission != PullRequestRoutingPermission::Off {
            next.push(StoredPullRequestRoutingPermission { environment_id: environment_id.clone(), connection_key, permission });
        }
        self.permissions.send_replace(Arc::from(next.clone()));
        Some(next)
    }

    /// Drops every grant whose environment's endpoints changed or that was removed, so a later
    /// change back cannot restore trust (`forget`). Returns the environments it dropped.
    pub fn forget_stale(&self) -> Vec<EnvironmentId> {
        let entries = self.registry.entries().borrow().clone();
        let current = self.permissions.borrow().clone();
        let stale: Vec<EnvironmentId> = current
            .iter()
            .filter(|p| {
                entries.iter().find(|e| e.environment_id == p.environment_id).and_then(connection_key).as_deref() != Some(p.connection_key.as_str())
            })
            .map(|p| p.environment_id.clone())
            .collect();
        if !stale.is_empty() {
            let next: Vec<_> = current.iter().filter(|p| !stale.contains(&p.environment_id)).cloned().collect();
            self.permissions.send_replace(Arc::from(next));
        }
        stale
    }

    /// `routingAllowed`: both machines enabled and trusted enough for this kind of call.
    fn allowed(&self, origin: &EnvironmentId, destination: &EnvironmentId, kind: RouteKind) -> bool {
        let (Some(origin), Some(destination)) = (self.entry(origin), self.entry(destination)) else { return false };
        if !origin.enabled || !destination.enabled {
            return false;
        }
        let permissions = self.permissions.borrow();
        let (source, target) = (permission_for(&origin, &permissions), permission_for(&destination, &permissions));
        match kind {
            RouteKind::Write => source == PullRequestRoutingPermission::ReadWrite && target == PullRequestRoutingPermission::ReadWrite,
            RouteKind::Read => source != PullRequestRoutingPermission::Off && target != PullRequestRoutingPermission::Off,
        }
    }

    fn session(&self, environment_id: &EnvironmentId) -> Option<Arc<RpcSession>> {
        self.registry.supervisor(environment_id).and_then(|s| s.session.borrow().clone())
    }

    /// A machine reached at a loopback address (`isLocal`).
    fn is_local(entry: &CatalogEntry) -> bool {
        entry
            .routes
            .first()
            .and_then(|route| route.bearer_profile())
            .and_then(|profile| profile.http_base_url.host_str().map(str::to_owned))
            .is_some_and(|host| matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]"))
    }

    /// Runs one pull request method for `origin` (`readOrWrite`).
    pub async fn request<M: RoutedMethod>(&self, origin: &EnvironmentId, session: Arc<RpcSession>, input: M::Payload) -> Result<M::Success, RpcError<M::Error>> {
        if M::KIND == RouteKind::Write {
            return self.routed::<M>(origin, &session, input, true).await;
        }
        let entries = self.registry.entries().borrow().clone();
        let any = entries.iter().any(|e| &e.environment_id != origin && self.allowed(origin, &e.environment_id, RouteKind::Read));
        if entries.len() < 2 || !any {
            return session.client.call::<M>(input).await;
        }
        let mut strict = input.clone();
        strict.set_allow_stale(Some(false));
        let origin_local = entries.iter().find(|e| &e.environment_id == origin).is_some_and(Self::is_local);
        let result = if origin_local {
            match timed(READ_TIMEOUT, session.client.call::<M>(strict.clone())).await {
                Ok(value) => Ok(value),
                Err(_) => self.routed::<M>(origin, &session, strict, false).await,
            }
        } else {
            self.routed::<M>(origin, &session, strict, true).await
        };
        match result {
            Err(_) if input.allow_stale() != Some(false) && matches!(M::TAG, "pullRequests.summary" | "pullRequests.detail") => {
                session.client.call::<M>(input).await
            }
            other => other,
        }
    }

    /// `routedRequest`: the origin's answer, or a trusted machine's for the same host and account.
    /// `include_origin` is false when the origin already failed this read.
    async fn routed<M: RoutedMethod>(&self, origin: &EnvironmentId, session: &Arc<RpcSession>, input: M::Payload, include_origin: bool) -> Result<M::Success, RpcError<M::Error>> {
        let reference = input.reference();
        let entries = self.registry.entries().borrow().clone();
        let mut alternatives = Vec::new();
        for entry in entries.iter().filter(|e| &e.environment_id != origin) {
            if !self.allowed(origin, &entry.environment_id, M::KIND) {
                continue;
            }
            let Some(other) = self.session(&entry.environment_id) else { continue };
            if M::TAG == "pullRequests.checks" && other.config().borrow().environment.capabilities.pull_request_checks != Some(true) {
                continue;
            }
            alternatives.push((entry.environment_id.clone(), Self::is_local(entry)));
        }
        if alternatives.is_empty() {
            let result = session.client.call::<M>(input).await;
            self.after_write::<M>(origin, &reference, result.is_ok()).await;
            return result;
        }
        // The source answers only for a host whose account it can verify.
        let identity = timed(PROBE_TIMEOUT, session.client.call::<PullRequestsRouting>(reference.clone())).await.ok();
        let Some(identity) = identity else {
            let result = session.client.call::<M>(input).await;
            self.after_write::<M>(origin, &reference, result.is_ok()).await;
            return result;
        };
        let mut routed_input = input;
        routed_input.route_to(&identity.host, &identity.account_id);
        let origin_local = entries.iter().find(|e| &e.environment_id == origin).is_some_and(Self::is_local);
        let write = M::KIND == RouteKind::Write;
        let mut candidates: Vec<EnvironmentId> = Vec::new();
        if origin_local && write {
            candidates.push(origin.clone());
        }
        candidates.extend(alternatives.iter().filter(|(_, local)| *local).map(|(id, _)| id.clone()));
        if !origin_local && write {
            candidates.push(origin.clone());
        }
        candidates.extend(alternatives.iter().filter(|(_, local)| !*local).map(|(id, _)| id.clone()));
        if !write && include_origin {
            candidates.push(origin.clone());
        }
        let mut last = None;
        for (index, id) in candidates.iter().enumerate() {
            let more = index + 1 < candidates.len();
            let target = if id == origin {
                session.clone()
            } else {
                if !self.allowed(origin, id, M::KIND) {
                    continue;
                }
                let Some(other) = self.session(id) else { continue };
                // An older server would discard expectedAccountId; verify it implements the guard.
                let probe = PullRequestRoutingIdentityInput { host: identity.host.clone() };
                let alternate = timed(PROBE_TIMEOUT, other.client.call::<PullRequestsRoutingIdentity>(probe)).await.ok();
                let matches = alternate.is_some_and(|a| {
                    a.provider == identity.provider && a.host.eq_ignore_ascii_case(&identity.host) && a.account_id == identity.account_id
                });
                if !matches || !self.allowed(origin, id, M::KIND) {
                    continue;
                }
                other
            };
            let call = target.client.call::<M>(routed_input.clone());
            let result = if write { call.await } else { timed(READ_TIMEOUT, call).await };
            match result {
                Ok(mut value) => {
                    if id != origin
                        && let Some((state, is_draft)) = value.state()
                    {
                        report_state(session.clone(), &reference, state, is_draft);
                    }
                    self.record(origin, &reference, &identity.host, id);
                    value.adopt_project(&reference, &identity);
                    self.after_write::<M>(origin, &reference, true).await;
                    return Ok(value);
                }
                Err(error) if more && (!write || rejected_before_dispatch(&error)) => last = Some(error),
                Err(error) => {
                    // A failed viewed-files batch may have applied some paths.
                    self.after_write::<M>(origin, &reference, M::TAG == "pullRequests.setFilesViewed").await;
                    return Err(error);
                }
            }
        }
        match last {
            Some(error) if !write => Err(error),
            _ => {
                let result = session.client.call::<M>(routed_input).await;
                self.after_write::<M>(origin, &reference, result.is_ok() || M::TAG == "pullRequests.setFilesViewed").await;
                result
            }
        }
    }

    fn reference_key(origin: &EnvironmentId, reference: &PullRequestRef) -> String {
        serde_json::to_string(&[
            Some(origin.as_str().to_owned()),
            Some(reference.project_id.to_string()),
            reference.host.as_ref().map(|h| h.to_lowercase()),
            Some(reference.repository.to_lowercase()),
            Some(reference.number.to_string()),
        ])
        .unwrap_or_default()
    }

    fn record(&self, origin: &EnvironmentId, reference: &PullRequestRef, host: &str, target: &EnvironmentId) {
        let key = Self::reference_key(origin, reference);
        let mut used = self.used.lock().expect("routed reads lock");
        match used.iter_mut().find(|(k, _)| *k == key) {
            Some((_, read)) => _ = read.targets.insert(target.clone()),
            None => {
                if used.len() >= ROUTED_READS_LIMIT {
                    used.remove(0);
                }
                let reference = PullRequestRef { host: Some(host.to_owned()), ..reference.clone() };
                used.push((key, RoutedRead { origin: origin.clone(), reference, targets: HashSet::from([target.clone()]) }));
            }
        }
    }

    /// After a write, tells every machine that read this pull request for `origin`, and the origin,
    /// to read it again (`invalidateReaders`).
    async fn after_write<M: RoutedMethod>(&self, origin: &EnvironmentId, reference: &PullRequestRef, invalidate: bool) {
        if M::KIND != RouteKind::Write || !invalidate {
            return;
        }
        let files_viewed = M::TAG == "pullRequests.setFilesViewed";
        let mut targets: HashMap<EnvironmentId, Vec<PullRequestRef>> = HashMap::from([(origin.clone(), vec![reference.clone()])]);
        {
            let used = self.used.lock().expect("routed reads lock");
            if used.is_empty() {
                return;
            }
            for (_, read) in used.iter().filter(|(_, r)| &r.origin == origin && matches_reference(&r.reference, reference)) {
                for target in read.targets.iter().chain([origin]) {
                    let refs = targets.entry(target.clone()).or_default();
                    if !refs.iter().any(|r| r.host == read.reference.host) {
                        refs.push(read.reference.clone());
                    }
                }
            }
        }
        for (target, refs) in targets {
            let mut inputs: Vec<PullRequestInvalidateInput> = refs
                .into_iter()
                .map(|reference| PullRequestInvalidateInput { reference: Some(reference), files_viewed_only: files_viewed.then_some(true) })
                .collect();
            if !files_viewed {
                inputs.push(PullRequestInvalidateInput::default());
            }
            self.invalidate_target(origin, &target, inputs).await;
        }
    }

    async fn invalidate_target(&self, origin: &EnvironmentId, target: &EnvironmentId, inputs: Vec<PullRequestInvalidateInput>) {
        if target != origin && !self.allowed(origin, target, RouteKind::Read) {
            return;
        }
        let Some(session) = self.session(target) else { return };
        // A stalled reader must not delay the write's answer.
        let calls = inputs.into_iter().map(|input| {
            let session = session.clone();
            async move { _ = timed(INVALIDATE_TIMEOUT, session.client.call::<PullRequestsInvalidate>(input)).await }
        });
        futures::future::join_all(calls).await;
    }

    /// The origin's own invalidation, passed on to the machines that read for it.
    pub async fn invalidate(&self, origin: &EnvironmentId, session: Arc<RpcSession>, input: PullRequestInvalidateInput) -> Result<(), RpcError<<PullRequestsInvalidate as RpcMethod>::Error>> {
        session.client.call::<PullRequestsInvalidate>(input.clone()).await?;
        let targets: Vec<(EnvironmentId, PullRequestRef)> = {
            let used = self.used.lock().expect("routed reads lock");
            let mut targets = Vec::new();
            for (_, read) in used.iter().filter(|(_, r)| &r.origin == origin) {
                if input.reference.as_ref().is_some_and(|r| !matches_reference(&read.reference, r)) {
                    continue;
                }
                targets.extend(read.targets.iter().map(|t| (t.clone(), read.reference.clone())));
                if input.reference.is_some() {
                    targets.push((origin.clone(), read.reference.clone()));
                }
            }
            targets
        };
        for (target, reference) in targets {
            let input = PullRequestInvalidateInput { reference: input.reference.as_ref().map(|_| reference), ..input.clone() };
            self.invalidate_target(origin, &target, vec![input]).await;
        }
        Ok(())
    }
}

fn matches_reference(reference: &PullRequestRef, filter: &PullRequestRef) -> bool {
    reference.project_id == filter.project_id
        && reference.repository.eq_ignore_ascii_case(&filter.repository)
        && reference.number == filter.number
        && filter.host.as_ref().is_none_or(|host| reference.host.as_ref().is_some_and(|h| h.eq_ignore_ascii_case(host)))
}

/// The origin owns the threads linked to the pull request but never saw a routed read; tell it
/// the state the read saw (`reportState`).
fn report_state(session: Arc<RpcSession>, reference: &PullRequestRef, state: PullRequestState, is_draft: Option<bool>) {
    let reference = PullRequestRef { allow_stale: None, expected_account_id: None, ..reference.clone() };
    let input = PullRequestReportStateInput { reference, state, is_draft };
    tokio::spawn(async move {
        _ = timed(REPORT_TIMEOUT, session.client.call::<PullRequestsReportState>(input)).await;
    });
}

/// Whether the failure happened before the destination acted (`rejectedBeforeDispatch`), so a
/// write may try the next machine.
fn rejected_before_dispatch<E: std::fmt::Debug>(error: &RpcError<E>) -> bool {
    match error {
        RpcError::Closed(_) => true,
        RpcError::Failure(failure) => format!("{failure:?}").contains("routeIdentity"),
        _ => false,
    }
}

/// `call` with a deadline; a timeout reads as an interrupted call.
async fn timed<T, E: std::fmt::Debug>(limit: Duration, call: impl Future<Output = Result<T, RpcError<E>>>) -> Result<T, RpcError<E>> {
    tokio::time::timeout(limit, call).await.unwrap_or(Err(RpcError::Interrupted))
}
