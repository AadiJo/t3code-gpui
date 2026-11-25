//! A thread's linked pull requests as lists and stacks (`threadPullRequests.ts` in
//! `source-control-core`, `shared` and `client-runtime`, `threadPullRequestCompatibility.ts`).
//!
//! Links are identified by their normalised `host/repository#number`; native stacks group by the
//! host's stack id, the rest chain by base branch onto another link's head branch.

use std::collections::{HashMap, HashSet};

use t3code_contracts::{
    OrchestrationV2AppThreadLineageRelationshipToParent as Relationship, OrchestrationV2ThreadShell,
    OrchestrationV2ThreadShellSettledOverride as Settled, PullRequestState, ThreadPullRequestLink, ThreadPullRequestLinkSource,
};

use crate::change_request_links::{canonical_repository_key, canonical_repository_path, parse_change_request_url};

use super::models::parse_ms;

/// A link's normalised identity (`ThreadPullRequestKey`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThreadPullRequestKey {
    pub host: String,
    pub repository: String,
    pub number: u64,
}

/// `normalizeThreadPullRequestKey`: the URL's authority when it names the same pull request,
/// lowercased, through the repository aliases the hosts declare.
pub fn normalize_thread_pull_request_key(host: &str, repository: &str, number: u64, url: Option<&str>) -> ThreadPullRequestKey {
    let authority = url
        .and_then(parse_change_request_url)
        .filter(|(_, parsed)| parsed.repository == canonical_repository_path(repository.trim()) && parsed.number == number)
        .and_then(|(_, parsed)| parsed.authority);
    let canonical = canonical_repository_key(&format!(
        "{}/{}",
        authority.as_deref().unwrap_or(host).trim().to_lowercase(),
        repository.trim().to_lowercase()
    ));
    let (host, repository) = canonical.split_once('/').unwrap_or((canonical.as_str(), ""));
    ThreadPullRequestKey { host: host.to_owned(), repository: repository.to_owned(), number }
}

/// The normalised key of a link.
pub fn link_key(link: &ThreadPullRequestLink) -> ThreadPullRequestKey {
    normalize_thread_pull_request_key(&link.host, &link.repository, link.number, Some(&link.url))
}

/// `threadPullRequestKeyOf`: `host/repository#number`.
pub fn thread_pull_request_key_of(link: &ThreadPullRequestLink) -> String {
    let key = link_key(link);
    format!("{}/{}#{}", key.host, key.repository, key.number)
}

/// `visibleThreadPullRequests`: links the user did not dismiss from a stack.
pub fn visible_thread_pull_requests(links: &[ThreadPullRequestLink]) -> Vec<&ThreadPullRequestLink> {
    links.iter().filter(|link| link.source != ThreadPullRequestLinkSource::StackDismissed).collect()
}

/// How a chain was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainKind {
    /// The host's own stack.
    Native,
    /// Base branches onto head branches.
    Derived,
}

/// Links that land on one another (`ThreadPullRequestChain`): `depths[i]` is 0 for a base and
/// its parent's depth + 1 above it.
#[derive(Clone, Debug)]
pub struct ThreadPullRequestChain<'a> {
    pub kind: ChainKind,
    pub layers: Vec<&'a ThreadPullRequestLink>,
    pub depths: Vec<usize>,
}

/// `resolveThreadPullRequestChains`.
pub fn resolve_thread_pull_request_chains(links: &[ThreadPullRequestLink]) -> Vec<ThreadPullRequestChain<'_>> {
    let visible = visible_thread_pull_requests(links);
    let mut chains = Vec::new();
    let mut placed: HashSet<String> = HashSet::new();

    let mut native: Vec<(String, Vec<&ThreadPullRequestLink>)> = Vec::new();
    for link in &visible {
        let Some(stack) = &link.stack else { continue };
        let key = link_key(link);
        let stack_key = format!("{}/{}#stack:{}", key.host, key.repository, stack.id);
        match native.iter_mut().find(|(k, _)| *k == stack_key) {
            Some((_, members)) => members.push(link),
            None => native.push((stack_key, vec![link])),
        }
    }
    for (_, mut members) in native {
        // Saved snapshots can lag behind newer members of the same stack.
        let numbers: HashSet<u64> = members.iter().map(|m| m.number).collect();
        let coverage = |layers: &[t3code_contracts::ThreadPullRequestStackLayer]| layers.iter().filter(|l| numbers.contains(&l.number)).count();
        let mut layers = &members[0].stack.as_ref().expect("stacked").layers;
        let mut covered = coverage(layers);
        for member in &members[1..] {
            let candidate = &member.stack.as_ref().expect("stacked").layers;
            let candidate_covered = coverage(candidate);
            if candidate_covered > covered || (candidate_covered == covered && candidate.len() > layers.len()) {
                layers = candidate;
                covered = candidate_covered;
            }
        }
        let order: HashMap<u64, usize> = layers.iter().enumerate().map(|(i, l)| (l.number, i)).collect();
        let fallback = layers.len();
        members.sort_by_key(|m| order.get(&m.number).copied().unwrap_or(fallback));
        for member in &members {
            placed.insert(thread_pull_request_key_of(member));
        }
        let depths = (0..members.len()).collect();
        chains.push(ThreadPullRequestChain { kind: ChainKind::Native, layers: members, depths });
    }

    let remaining: Vec<&ThreadPullRequestLink> = visible.iter().copied().filter(|l| !placed.contains(&thread_pull_request_key_of(l))).collect();
    let branch_key = |link: &ThreadPullRequestLink, branch: &str| {
        let key = link_key(link);
        format!("{}/{}:{branch}", key.host, key.repository)
    };
    let mut by_head: HashMap<String, Option<&ThreadPullRequestLink>> = HashMap::new();
    for link in &remaining {
        let Some(snapshot) = &link.snapshot else { continue };
        let key = branch_key(link, &snapshot.head_branch);
        let duplicate = by_head.contains_key(&key);
        by_head.insert(key, if duplicate { None } else { Some(link) });
    }
    let parent_of = |link: &ThreadPullRequestLink| -> Option<&ThreadPullRequestLink> {
        let snapshot = link.snapshot.as_ref()?;
        let parent = (*by_head.get(&branch_key(link, &snapshot.base_branch))?)?;
        (!std::ptr::eq(parent, link)).then_some(parent)
    };
    let mut children: HashMap<String, Vec<&ThreadPullRequestLink>> = HashMap::new();
    for link in &remaining {
        if let Some(parent) = parent_of(link) {
            children.entry(thread_pull_request_key_of(parent)).or_default().push(link);
        }
    }
    for base in &remaining {
        if parent_of(base).is_some() {
            continue;
        }
        let (mut layers, mut depths) = (Vec::new(), Vec::new());
        let mut pending = vec![(*base, 0usize)];
        while let Some((link, depth)) = pending.pop() {
            let key = thread_pull_request_key_of(link);
            // Two links with one identity would otherwise share a child list and expand forever.
            if !placed.insert(key.clone()) {
                continue;
            }
            layers.push(link);
            depths.push(depth);
            for child in children.get(&key).into_iter().flatten().rev() {
                pending.push((child, depth + 1));
            }
        }
        if !layers.is_empty() {
            chains.push(ThreadPullRequestChain { kind: ChainKind::Derived, layers, depths });
        }
    }
    for link in &remaining {
        if placed.insert(thread_pull_request_key_of(link)) {
            chains.push(ThreadPullRequestChain { kind: ChainKind::Derived, layers: vec![link], depths: vec![0] });
        }
    }
    chains
}

/// One row of the Linked items list (`PullRequestListLine`); `stack` is set on a chain's first
/// row when the chain has several layers.
#[derive(Clone, Debug)]
pub struct PullRequestListLine<'a> {
    pub link: &'a ThreadPullRequestLink,
    pub depth: usize,
    pub chain_key: String,
    pub stack: Option<(ChainKind, usize)>,
}

fn updated_ms(link: &ThreadPullRequestLink) -> i64 {
    parse_ms(link.snapshot.as_ref().and_then(|s| s.updated_at.as_deref()).unwrap_or(&link.linked_at)).unwrap_or(0)
}

/// `pullRequestListLines`: chains newest first by their newest layer, each layer in order.
pub fn pull_request_list_lines<'a>(chains: &[ThreadPullRequestChain<'a>]) -> Vec<PullRequestListLine<'a>> {
    let mut ordered: Vec<&ThreadPullRequestChain<'a>> = chains.iter().collect();
    ordered.sort_by_key(|chain| std::cmp::Reverse(chain.layers.iter().map(|l| updated_ms(l)).max().unwrap_or(0)));
    ordered
        .into_iter()
        .flat_map(|chain| {
            let bottom = chain.layers[0];
            let chain_key = format!("{}/{}#{}", bottom.host, bottom.repository, bottom.number);
            let size = chain.layers.len();
            chain.layers.iter().enumerate().map(move |(index, link)| PullRequestListLine {
                link,
                depth: chain.depths[index],
                chain_key: chain_key.clone(),
                stack: (index == 0 && size > 1).then_some((chain.kind, size)),
            })
        })
        .collect()
}

/// A link whose host has not been read yet counts as open.
pub fn is_open(link: &ThreadPullRequestLink) -> bool {
    link.snapshot.as_ref().is_none_or(|s| s.state == PullRequestState::Open)
}

/// `canStartThreadPullRequestWatch`: the server refuses new watches on settled, archived and
/// subagent threads.
pub fn can_start_thread_pull_request_watch(thread: &OrchestrationV2ThreadShell) -> bool {
    thread.archived_at.is_none()
        && thread.settled_at.is_none()
        && thread.settled_override != Some(Settled::Settled)
        && thread.lineage.relationship_to_parent != Some(Relationship::Subagent)
}

/// The pull request a thread is working on (`ThreadCurrentPullRequest`): one link, or the open
/// layers of its stacks from the top down.
#[derive(Clone, Debug)]
pub enum ThreadCurrentPullRequest<'a> {
    Single(&'a ThreadPullRequestLink),
    Stack { open: Vec<&'a ThreadPullRequestLink>, top: &'a ThreadPullRequestLink },
}

/// A chain's layers deepest first; siblings later in the chain first (`layersFromTop`).
fn layers_from_top<'a>(chain: &ThreadPullRequestChain<'a>) -> Vec<&'a ThreadPullRequestLink> {
    let mut order: Vec<usize> = (0..chain.layers.len()).collect();
    order.sort_by(|&left, &right| chain.depths[right].cmp(&chain.depths[left]).then(right.cmp(&left)));
    order.into_iter().map(|index| chain.layers[index]).collect()
}

/// `resolveThreadCurrentPullRequest`: the one open link; several open ones as a stack, ordered
/// top down within each chain and chains by their newest link; with none open, the top of the
/// only chain, else the most recently updated link.
pub fn resolve_thread_current_pull_request(links: &[ThreadPullRequestLink]) -> Option<ThreadCurrentPullRequest<'_>> {
    let visible = visible_thread_pull_requests(links);
    let first = *visible.first()?;
    let open: Vec<_> = visible.iter().copied().filter(|link| is_open(link)).collect();
    if let [single] = open.as_slice() {
        return Some(ThreadCurrentPullRequest::Single(single));
    }
    let chains = resolve_thread_pull_request_chains(links);
    if open.len() > 1 {
        let mut open_chains: Vec<Vec<&ThreadPullRequestLink>> = chains
            .iter()
            .map(|chain| layers_from_top(chain).into_iter().filter(|link| is_open(link)).collect::<Vec<_>>())
            .filter(|layers| !layers.is_empty())
            .collect();
        if open_chains.len() > 1 {
            let newest = |layers: &[&ThreadPullRequestLink]| layers.iter().map(|link| parse_ms(&link.linked_at).unwrap_or(0)).max().unwrap_or(0);
            open_chains.sort_by_key(|layers| std::cmp::Reverse(newest(layers)));
        }
        let ordered: Vec<_> = open_chains.into_iter().flatten().collect();
        let top = *ordered.first()?;
        return Some(ThreadCurrentPullRequest::Stack { open: ordered, top });
    }
    if let [chain] = chains.as_slice() {
        return layers_from_top(chain).first().map(|top| ThreadCurrentPullRequest::Single(top));
    }
    let mut latest = first;
    for link in &visible[1..] {
        if updated_ms(link) > updated_ms(latest) {
            latest = link;
        }
    }
    Some(ThreadCurrentPullRequest::Single(latest))
}

/// `resolveThreadCurrentPullRequestLink`: the single link or the stack's top.
pub fn resolve_thread_current_pull_request_link(links: &[ThreadPullRequestLink]) -> Option<&ThreadPullRequestLink> {
    match resolve_thread_current_pull_request(links)? {
        ThreadCurrentPullRequest::Single(link) => Some(link),
        ThreadCurrentPullRequest::Stack { top, .. } => Some(top),
    }
}

/// `legacyThreadPullRequestKey`: the key of a thread's older single linked pull request, from its
/// URL when that names the same number, else the URL's host name.
pub fn legacy_thread_pull_request_key(repository: &str, number: u64, url: &str) -> ThreadPullRequestKey {
    if let Some((_, parsed)) = parse_change_request_url(url).filter(|(_, parsed)| parsed.number == number) {
        return normalize_thread_pull_request_key(&parsed.host, &parsed.repository, number, Some(url));
    }
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = host.split(':').next().unwrap_or_default().trim().to_lowercase();
    ThreadPullRequestKey {
        host: if host.is_empty() { "unknown".to_owned() } else { host },
        repository: repository.trim().to_lowercase(),
        number,
    }
}

/// What the "Pull request" surface opens for a thread (`threadPullRequestPanelTarget`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestPanelTarget {
    pub project_id: String,
    pub host: Option<String>,
    pub repository: String,
    pub number: u64,
    pub url: String,
}

/// `threadPullRequestPanelTarget`: the current linked pull request (the older single link when it
/// is the same one), else the older single link, else the branch's pull request.
pub fn thread_pull_request_panel_target(thread: &OrchestrationV2ThreadShell) -> Option<PullRequestPanelTarget> {
    let legacy = |linked: &t3code_contracts::ThreadLinkedPullRequest| PullRequestPanelTarget {
        project_id: linked.project_id.to_string(),
        host: None,
        repository: linked.repository.clone(),
        number: u64::from(linked.number),
        url: linked.url.clone(),
    };
    let current = resolve_thread_current_pull_request_link(thread.pull_requests.as_deref().unwrap_or_default());
    match (current, thread.linked_pull_request.as_ref()) {
        (Some(current), Some(linked)) => {
            let key = legacy_thread_pull_request_key(&linked.repository, u64::from(linked.number), &linked.url);
            let legacy_key = normalize_thread_pull_request_key(&key.host, &key.repository, key.number, None);
            if link_key(current) == legacy_key {
                return Some(legacy(linked));
            }
        }
        (None, linked) => return linked.or(thread.branch_pull_request.as_ref()).map(legacy),
        _ => {}
    }
    current.map(|current| PullRequestPanelTarget {
        project_id: thread.project_id.to_string(),
        host: Some(current.host.clone()),
        repository: current.repository.clone(),
        number: current.number,
        url: current.url.clone(),
    })
}
