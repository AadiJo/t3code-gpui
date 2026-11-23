//! Maps HTTP and relay failures onto [`ConnectionError`], deciding what
//! retries and what waits for the user.

use super::model::{
    BlockedReason, ConnectionError, ConnectionHop, ConnectionVia, FailureCause, TransientReason,
};
use crate::authorization::remote::ConnectionMethod;
use crate::relay::{RelayError, RelayProtectedError};
use crate::rpc::http::{AuthInvalidReason, DpopFailureReason, EnvironmentHttpError, RemoteRequestError};

/// Appended to failures that may be a firewall or VPN between this device and the tunnel.
pub const NETWORK_BLOCKING_HINT: &str =
    "A firewall, VPN, or content filter on this network may be blocking the connection.";

fn via(method: ConnectionMethod) -> ConnectionVia {
    match method {
        ConnectionMethod::Relay => ConnectionVia::Relay,
        ConnectionMethod::Direct => ConnectionVia::Direct,
    }
}

/// A failed environment HTTP request (descriptor, token, ticket).
pub fn remote_environment_error(error: &RemoteRequestError, method: ConnectionMethod) -> ConnectionError {
    // Over T3 Connect the relay already reached the host, so a request that
    // never arrives was blocked between this device and the tunnel.
    let unreachable = match method {
        ConnectionMethod::Relay => ConnectionHop::Tunnel,
        ConnectionMethod::Direct => ConnectionHop::Server,
    };
    let hinted = |text: String| match method {
        ConnectionMethod::Relay => format!("{text} {NETWORK_BLOCKING_HINT}"),
        ConnectionMethod::Direct => text,
    };
    let server = |error: ConnectionError, trace: Option<&str>| error.at(ConnectionHop::Server).via(via(method)).trace(trace);
    match error {
        RemoteRequestError::Environment(body) => {
            let trace = body.trace_id();
            match body {
                EnvironmentHttpError::EnvironmentAuthInvalidError { .. } => server(
                    ConnectionError::blocked(BlockedReason::Authentication, "The environment credential is invalid."),
                    trace,
                ),
                EnvironmentHttpError::EnvironmentScopeRequiredError { .. }
                | EnvironmentHttpError::EnvironmentOperationForbiddenError { .. } => server(
                    ConnectionError::blocked(
                        BlockedReason::Permission,
                        "The environment credential does not grant the required access.",
                    ),
                    trace,
                ),
                EnvironmentHttpError::EnvironmentRequestInvalidError { .. } => server(
                    ConnectionError::blocked(
                        BlockedReason::Configuration,
                        "The environment rejected the authentication request.",
                    ),
                    trace,
                ),
                EnvironmentHttpError::EnvironmentResourceNotFoundError { .. } => server(
                    ConnectionError::blocked(BlockedReason::Configuration, "The environment endpoint could not be found."),
                    trace,
                ),
                EnvironmentHttpError::EnvironmentConnectApprovalPendingError { .. } => server(
                    ConnectionError::transient(
                        TransientReason::ApprovalPending,
                        "This device is waiting for approval on the host.",
                    )
                    .cause(FailureCause::ApprovalPending),
                    trace,
                ),
                EnvironmentHttpError::EnvironmentInternalError { .. } => server(
                    ConnectionError::transient(
                        TransientReason::RemoteUnavailable,
                        "The environment could not authorize the connection.",
                    ),
                    trace,
                ),
            }
        }
        RemoteRequestError::Timeout => {
            ConnectionError::transient(TransientReason::Timeout, hinted("The environment did not answer in time.".into()))
                .at(unreachable)
                .via(via(method))
        }
        RemoteRequestError::Network(detail) => {
            ConnectionError::transient(TransientReason::Network, hinted(format!("Could not reach the environment: {detail}")))
                .at(unreachable)
                .via(via(method))
        }
        RemoteRequestError::UndeclaredStatus { status, .. } => {
            let mut error = server(
                ConnectionError::transient(
                    TransientReason::RemoteUnavailable,
                    format!("The environment answered HTTP {status}."),
                ),
                None,
            );
            error.http_status = Some(*status);
            error
        }
        RemoteRequestError::InvalidJson(detail) => server(
            ConnectionError::transient(
                TransientReason::RemoteUnavailable,
                format!("The environment's answer did not decode: {detail}"),
            ),
            None,
        ),
    }
}

/// Like [`remote_environment_error`], but names a DPoP clock failure.
pub fn remote_dpop_environment_error(error: &RemoteRequestError, method: ConnectionMethod) -> ConnectionError {
    if let RemoteRequestError::Environment(EnvironmentHttpError::EnvironmentAuthInvalidError {
        reason: AuthInvalidReason::InvalidCredential,
        dpop_failure_reason,
        trace_id,
    }) = error
    {
        let clock = *dpop_failure_reason == Some(DpopFailureReason::TimeWindow);
        let detail = if clock {
            "The environment credential is invalid. This device's clock may be wrong."
        } else {
            "The environment credential is invalid."
        };
        let mut mapped = ConnectionError::blocked(BlockedReason::Authentication, detail)
            .at(ConnectionHop::Server)
            .via(via(method))
            .trace(trace_id.as_deref());
        if clock {
            mapped.failure_cause = Some(FailureCause::ClockSkew);
        }
        return mapped;
    }
    remote_environment_error(error, method)
}

/// A failed relay request.
pub fn relay_error(error: &RelayError) -> ConnectionError {
    match error {
        RelayError::Protected(protected) => relay_protected_error(protected),
        RelayError::Request { status, detail } => {
            let mut mapped = ConnectionError::transient(TransientReason::RelayUnavailable, detail.clone()).at(ConnectionHop::Relay);
            if let Some(status) = status {
                mapped.http_status = Some(*status);
                mapped.failure_cause = Some(FailureCause::RelayError);
            }
            mapped
        }
        RelayError::Timeout => {
            ConnectionError::transient(TransientReason::Timeout, "T3 Connect did not answer in time.").at(ConnectionHop::Relay)
        }
        RelayError::InvalidUrl => {
            ConnectionError::blocked(BlockedReason::Configuration, "The T3 Connect URL is invalid.").at(ConnectionHop::Relay)
        }
        RelayError::ScopesUnexpected { .. } => ConnectionError::blocked(
            BlockedReason::Permission,
            "T3 Connect granted different access than requested.",
        )
        .at(ConnectionHop::Relay),
    }
}

fn relay_protected_error(error: &RelayProtectedError) -> ConnectionError {
    use RelayProtectedError as E;
    let detail = error.to_string();
    let trace = error.trace_id();
    let blocked = |reason, hop| ConnectionError::blocked(reason, detail.clone()).at(hop).trace(trace);
    let transient = |reason, hop| ConnectionError::transient(reason, detail.clone()).at(hop).trace(trace);
    match error {
        E::RelayAuthInvalidError { dpop_failure_reason, .. } => {
            let mut mapped = blocked(BlockedReason::Authentication, ConnectionHop::Relay);
            if *dpop_failure_reason == Some(DpopFailureReason::TimeWindow) {
                mapped.failure_cause = Some(FailureCause::ClockSkew);
            }
            mapped
        }
        E::RelayEnvironmentConnectNotAuthorizedError { reason, .. } => match reason.as_deref() {
            Some("managed_endpoint_allocation_not_ready") => {
                transient(TransientReason::EndpointUnavailable, ConnectionHop::Tunnel).cause(FailureCause::TunnelPending)
            }
            Some("endpoint_provider_not_managed") => {
                blocked(BlockedReason::Permission, ConnectionHop::Tunnel).cause(FailureCause::TunnelNotEnabled)
            }
            Some("environment_link_not_found") => {
                blocked(BlockedReason::Permission, ConnectionHop::Relay).cause(FailureCause::NotLinked)
            }
            _ => blocked(BlockedReason::Permission, ConnectionHop::Relay),
        },
        E::RelayEnvironmentEndpointTimedOutError { .. } => {
            transient(TransientReason::Timeout, ConnectionHop::Tunnel).cause(FailureCause::Timeout)
        }
        E::RelayEnvironmentEndpointUnavailableError { reason, endpoint_failure, endpoint_status, .. } => {
            let failure = endpoint_failure.or((reason == "endpoint_response_invalid").then_some(FailureCause::InvalidResponse));
            let hop = match failure {
                Some(FailureCause::OriginUnreachable | FailureCause::InvalidResponse) => ConnectionHop::Server,
                _ => ConnectionHop::Tunnel,
            };
            let mut mapped = transient(TransientReason::EndpointUnavailable, hop).via(ConnectionVia::Relay);
            mapped.failure_cause = failure;
            mapped.http_status = *endpoint_status;
            mapped
        }
        E::RelayEnvironmentLinkUnavailableError { .. } => transient(TransientReason::EndpointUnavailable, ConnectionHop::Tunnel),
        E::RelayEnvironmentLinkFailedError { .. } | E::RelayInternalError { .. } | E::Unknown => {
            transient(TransientReason::RelayUnavailable, ConnectionHop::Relay).cause(FailureCause::RelayError)
        }
        E::Other { .. } => blocked(BlockedReason::Permission, ConnectionHop::Relay),
    }
}
