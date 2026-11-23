//! Comment reactions: which ones a host takes, their order and emoji, and the
//! optimistic overlay shown while a toggle waits for the host. Ports
//! `pullRequestReactions.ts`; the localized tooltip lives in the web crate.

use std::collections::BTreeMap;

use t3code_contracts::{
    IssueCommentReactionsItem as PullRequestReaction, IssueCommentReactionsItemContent as ReactionContent,
    PullRequestCommentKind, PullRequestDetail, SourceControlProviderKind,
};

/// Whether the viewer may react to a comment of `kind` on this pull request.
pub fn can_react_pull_request_comment(detail: &PullRequestDetail, kind: PullRequestCommentKind) -> bool {
    can_react(detail.capabilities.reactions, &detail.provider, kind)
}

/// [`can_react_pull_request_comment`] over the fields it reads.
pub fn can_react(reactions: Option<bool>, provider: &SourceControlProviderKind, kind: PullRequestCommentKind) -> bool {
    reactions == Some(true) && !(provider.as_str() == "gitcafe" && kind == PullRequestCommentKind::Review)
}

/// The order hosts show reactions in.
pub const PULL_REQUEST_REACTION_ORDER: [ReactionContent; 8] = [
    ReactionContent::ThumbsUp,
    ReactionContent::ThumbsDown,
    ReactionContent::Laugh,
    ReactionContent::Hooray,
    ReactionContent::Confused,
    ReactionContent::Heart,
    ReactionContent::Rocket,
    ReactionContent::Eyes,
];

/// The emoji a reaction shows as.
pub fn pull_request_reaction_emoji(content: ReactionContent) -> &'static str {
    match content {
        ReactionContent::ThumbsUp => "👍",
        ReactionContent::ThumbsDown => "👎",
        ReactionContent::Laugh => "😄",
        ReactionContent::Hooray => "🎉",
        ReactionContent::Confused => "😕",
        ReactionContent::Heart => "❤️",
        ReactionContent::Rocket => "🚀",
        ReactionContent::Eyes => "👀",
    }
}

/// Changes whenever the host reports different reactions, so pending overlays can reset.
pub fn pull_request_reactions_signature(reactions: &[PullRequestReaction]) -> String {
    reactions
        .iter()
        .map(|r| format!("{}:{}:{}", r.content.as_str(), r.count, u8::from(r.viewer_has_reacted)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Toggles awaiting the host, by reaction: `true` means the viewer added it.
pub type PendingReactions = BTreeMap<ReactionContent, bool>;

/// The host's reactions with the viewer's pending toggles applied, in host order.
pub fn apply_pending_pull_request_reactions(
    reactions: &[PullRequestReaction],
    pending: &PendingReactions,
) -> Vec<PullRequestReaction> {
    if pending.is_empty() {
        return reactions.to_vec();
    }
    let mut by_content: BTreeMap<ReactionContent, PullRequestReaction> =
        reactions.iter().map(|r| (r.content, r.clone())).collect();
    for (&content, &reacted) in pending {
        let Some(current) = by_content.get(&content) else {
            if reacted {
                by_content.insert(content, PullRequestReaction { content, count: 1, actors: vec![], viewer_has_reacted: true });
            }
            continue;
        };
        if current.viewer_has_reacted == reacted {
            continue;
        }
        let count = current.count as i64 + if reacted { 1 } else { -1 };
        if count <= 0 {
            by_content.remove(&content);
        } else {
            let next = PullRequestReaction { count: count as u64, viewer_has_reacted: reacted, ..current.clone() };
            by_content.insert(content, next);
        }
    }
    PULL_REQUEST_REACTION_ORDER.iter().filter_map(|c| by_content.remove(c)).collect()
}
