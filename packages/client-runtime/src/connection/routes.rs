//! Route identity, classification, the cleartext rule, and learned routes.

use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

use super::catalog::{
    BearerProfile, CatalogEntry, ConnectionProfile, ConnectionRoute, ConnectionTarget,
    ProfileAuthorization, ProfileNetwork, TargetRoute,
};

/// The route id of the T3 Connect route.
pub const RELAY_ROUTE_ID: &str = "relay";

/// What kind of network a route crosses. Orders new routes by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteKind {
    Loopback,
    Lan,
    Tailnet,
    /// 100.64.0.0/10 without the server confirming Tailscale: shared CGNAT space.
    Vpn,
    Public,
    Ssh,
    Relay,
}

impl RouteKind {
    fn rank(self) -> u8 {
        match self {
            Self::Loopback => 0,
            Self::Lan => 1,
            Self::Tailnet | Self::Vpn => 2,
            Self::Public => 3,
            Self::Ssh => 4,
            Self::Relay => 5,
        }
    }
}

/// The stable id of a route inside its environment.
pub fn route_id(target: &ConnectionTarget) -> &str {
    match &target.route {
        TargetRoute::Relay => RELAY_ROUTE_ID,
        TargetRoute::Bearer { connection_id } | TargetRoute::Ssh { connection_id } => connection_id,
    }
}

/// A learned route `learned:<env>:<origin>@<id>` borrows the credential of
/// route `<id>`; every other route uses its own.
pub fn credential_connection_id(connection_id: &str) -> &str {
    match connection_id.find('@') {
        Some(at) if connection_id.starts_with("learned:") => &connection_id[at + 1..],
        _ => connection_id,
    }
}

pub fn is_loopback_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

fn is_tailnet_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => domain.trim_end_matches('.').to_ascii_lowercase().ends_with(".ts.net"),
        Host::Ipv6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
        Host::Ipv4(_) => false,
    }
}

fn is_shared_address_space(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..=127).contains(&b)
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 0
        || a == 10
        || a == 127
        || is_shared_address_space(ip)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
        || (a == 198 && (18..=19).contains(&b))
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_ipv4(v4);
    }
    let first = ip.segments()[0];
    ip.is_unspecified() || ip.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

fn is_private_network_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
                || domain == "home.arpa"
                || domain.ends_with(".home.arpa")
                || domain.ends_with(".ts.net")
                || !domain.contains('.')
        }
        Host::Ipv4(ip) => is_private_ipv4(*ip),
        Host::Ipv6(ip) => is_private_ipv6(*ip),
    }
}

/// Classifies a direct address. `tailscale` is the server's confirmation.
pub fn direct_address_kind(url: &Url, tailscale: bool) -> RouteKind {
    let Some(host) = url.host() else { return RouteKind::Public };
    if is_loopback_host(&host) {
        RouteKind::Loopback
    } else if tailscale || is_tailnet_host(&host) {
        RouteKind::Tailnet
    } else if matches!(host, Host::Ipv4(ip) if is_shared_address_space(ip)) {
        RouteKind::Vpn
    } else if is_private_network_host(&host) {
        RouteKind::Lan
    } else {
        RouteKind::Public
    }
}

pub fn route_kind(route: &ConnectionRoute) -> RouteKind {
    match &route.target.route {
        TargetRoute::Relay => RouteKind::Relay,
        TargetRoute::Ssh { .. } => RouteKind::Ssh,
        TargetRoute::Bearer { .. } => match route.bearer_profile() {
            Some(profile) => direct_address_kind(&profile.http_base_url, profile.network == Some(ProfileNetwork::Tailscale)),
            None => RouteKind::Public,
        },
    }
}

/// A loopback or LAN host answers within a few seconds or is not there.
pub fn is_nearby_address(http_base_url: &Url) -> bool {
    matches!(direct_address_kind(http_base_url, false), RouteKind::Loopback | RouteKind::Lan)
}

/// TLS proves who answers and loopback cannot move, so an answer from such
/// an address can be reused. Any other plain http address can belong to
/// another machine after a network change.
pub fn route_keeps_its_host(route: &ConnectionRoute) -> bool {
    route.bearer_profile().is_some_and(|profile| profile.http_base_url.scheme() == "https")
        || route_kind(route) == RouteKind::Loopback
}

/// A token goes over plain HTTP outside the tailnet only if it was paired that way.
fn token_allows_plain_http(routes: &[ConnectionRoute], credential_id: &str) -> bool {
    routes.iter().find(|route| route_id(&route.target) == credential_id).is_some_and(|route| {
        route.bearer_profile().is_some_and(|profile| profile.http_base_url.scheme() == "http")
            && matches!(route_kind(route), RouteKind::Lan | RouteKind::Public)
    })
}

/// Whether connecting over `route` would send a borrowed token over plain
/// HTTP it was not paired over.
pub fn sends_token_in_clear(routes: &[ConnectionRoute], route: &ConnectionRoute, tailscale: Option<bool>) -> bool {
    let Some(profile) = route.bearer_profile() else { return false };
    if profile.authorization == Some(ProfileAuthorization::T3Connect) {
        return false;
    }
    let tailscale = tailscale.unwrap_or(profile.network == Some(ProfileNetwork::Tailscale));
    if tailscale || profile.http_base_url.scheme() != "http" {
        return false;
    }
    let own_id = route_id(&route.target);
    let credential_id = credential_connection_id(own_id);
    credential_id != own_id && !token_allows_plain_http(routes, credential_id)
}

/// Routes a connect may use, in priority order.
pub fn usable_routes(entry: &CatalogEntry) -> Vec<&ConnectionRoute> {
    entry
        .routes
        .iter()
        .filter(|route| !sends_token_in_clear(&entry.routes, route, None))
        .collect()
}

pub fn is_learned(route: &ConnectionRoute) -> bool {
    route.bearer_profile().is_some_and(|profile| profile.learned)
}

/// Inserts `route` after every route of its rank or better.
pub fn insert_route(routes: &mut Vec<ConnectionRoute>, route: ConnectionRoute) {
    let rank = route_kind(&route).rank();
    let index = routes
        .iter()
        .position(|existing| route_kind(existing).rank() > rank)
        .unwrap_or(routes.len());
    routes.insert(index, route);
}

/// An address the server reports in `config.directEndpoints`.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedEndpoint {
    pub http_base_url: String,
    #[serde(default)]
    pub kind: Option<String>,
}

fn origin_key(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// Merges the server's reported direct addresses into the entry's routes,
/// learned through `active`. Loopback is never learned. Learned routes the
/// server stops reporting are dropped; user routes are never touched.
/// Returns `None` when nothing changed.
pub fn merge_learned_routes(
    entry: &CatalogEntry,
    active: &ConnectionRoute,
    reported: &[ReportedEndpoint],
) -> Option<Vec<ConnectionRoute>> {
    let saved = &entry.routes;
    let active_profile = active.bearer_profile();
    let authorization = match &active.target.route {
        TargetRoute::Relay => Some(ProfileAuthorization::T3Connect),
        TargetRoute::Bearer { .. } => active_profile.and_then(|profile| profile.authorization),
        TargetRoute::Ssh { .. } => return None,
    };
    let shared_credential = match (&active.target.route, authorization) {
        (TargetRoute::Bearer { connection_id }, None) => Some(credential_connection_id(connection_id).to_owned()),
        _ => None,
    };
    let mut reported_urls: Vec<Url> = Vec::new();
    let mut tailscale = HashSet::new();
    for endpoint in reported {
        let Ok(url) = Url::parse(&endpoint.http_base_url) else { continue };
        if !matches!(url.scheme(), "http" | "https") || url.host().as_ref().is_some_and(is_loopback_host) {
            continue;
        }
        if endpoint.kind.as_deref() == Some("tailnet") {
            tailscale.insert(origin_key(&url));
        }
        if !reported_urls.iter().any(|known| origin_key(known) == origin_key(&url)) {
            reported_urls.push(url);
        }
    }
    let is_reported = |url: &Url| reported_urls.iter().any(|known| origin_key(known) == origin_key(url));
    let mut known: HashSet<String> = saved
        .iter()
        .filter(|route| !is_learned(route))
        .filter_map(|route| route.bearer_profile().map(|profile| origin_key(&profile.http_base_url)))
        .collect();
    let mut next: Vec<ConnectionRoute> = Vec::new();
    for route in saved.iter() {
        let Some(profile) = route.bearer_profile() else {
            next.push(route.clone());
            continue;
        };
        let origin = origin_key(&profile.http_base_url);
        let on_tailnet = tailscale.contains(&origin);
        if !is_learned(route) {
            next.push(if is_reported(&profile.http_base_url) { with_network(route, on_tailnet) } else { route.clone() });
            continue;
        }
        if !is_reported(&profile.http_base_url)
            || known.contains(&origin)
            || sends_token_in_clear(saved, route, Some(on_tailnet))
        {
            continue;
        }
        known.insert(origin);
        next.push(with_network(route, on_tailnet));
    }
    for url in &reported_urls {
        let origin = origin_key(url);
        if known.contains(&origin) {
            continue;
        }
        let on_tailnet = tailscale.contains(&origin);
        if url.scheme() == "http"
            && let Some(credential) = &shared_credential
            && !on_tailnet
            && !token_allows_plain_http(saved, credential)
        {
            continue;
        }
        let connection_id = match &shared_credential {
            Some(credential) => format!("learned:{}:{origin}@{credential}", entry.environment_id),
            None => format!("learned:{}:{origin}", entry.environment_id),
        };
        let ws_scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        let http_base_url = Url::parse(&format!("{origin}/")).expect("origin is a URL");
        let ws_base_url = Url::parse(&format!("{ws_scheme}://{}/", &origin[origin.find("://").map_or(0, |i| i + 3)..]))
            .expect("origin host is a URL");
        insert_route(
            &mut next,
            ConnectionRoute {
                target: ConnectionTarget {
                    environment_id: entry.environment_id.clone(),
                    label: entry.label.clone(),
                    route: TargetRoute::Bearer { connection_id: connection_id.clone() },
                },
                profile: Some(ConnectionProfile::Bearer(BearerProfile {
                    connection_id,
                    environment_id: entry.environment_id.clone(),
                    label: entry.label.clone(),
                    http_base_url,
                    ws_base_url,
                    learned: true,
                    authorization,
                    network: on_tailnet.then_some(ProfileNetwork::Tailscale),
                })),
            },
        );
        known.insert(origin);
    }
    (next.as_slice() != &saved[..]).then_some(next)
}

fn with_network(route: &ConnectionRoute, tailscale: bool) -> ConnectionRoute {
    let Some(profile) = route.bearer_profile() else { return route.clone() };
    if (profile.network == Some(ProfileNetwork::Tailscale)) == tailscale {
        return route.clone();
    }
    let mut profile = profile.clone();
    profile.network = tailscale.then_some(ProfileNetwork::Tailscale);
    ConnectionRoute { target: route.target.clone(), profile: Some(ConnectionProfile::Bearer(profile)) }
}

/// Removing a route also removes learned routes that borrow its credential,
/// and removing T3 Connect removes routes learned through it.
pub fn routes_after_removing(routes: &[ConnectionRoute], removed_id: &str) -> Vec<ConnectionRoute> {
    let Some(removed) = routes.iter().find(|route| route_id(&route.target) == removed_id) else {
        return routes.to_vec();
    };
    let removed_relay = removed.target.route == TargetRoute::Relay;
    routes
        .iter()
        .filter(|route| route_id(&route.target) != removed_id)
        .filter(|route| {
            if !is_learned(route) {
                return true;
            }
            match route.bearer_profile().and_then(|profile| profile.authorization) {
                Some(ProfileAuthorization::T3Connect) => !removed_relay,
                None => credential_connection_id(route_id(&route.target)) != removed_id,
            }
        })
        .cloned()
        .collect()
}
