//! Whether the timeline keeps its live end in view. Ports the follow state
//! machine and release rules of `timelineViewport.ts`.

/// The timeline's follow state. While following, new content keeps the end in
/// view; once released, `at_end` records whether the viewport still sat in the
/// end band when it was released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineFollow {
    Following,
    Released { at_end: bool },
}

/// What happened to the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineFollowEvent {
    /// The user navigated in a way that releases follow.
    Navigated,
    /// A command (scroll-to-end button, sending a message) asked to follow.
    FollowEnd,
    /// The viewport entered the end band.
    ReachedEnd,
    /// The viewport left the end band.
    LeftEnd,
}

impl TimelineFollow {
    /// The state a thread opens in: following unless the user left it mid-history.
    pub fn initial(follow_end: bool) -> Self {
        if follow_end {
            Self::Following
        } else {
            Self::Released { at_end: false }
        }
    }

    pub fn is_following(self) -> bool {
        self == Self::Following
    }

    /// Applies one event. A scroll key pressed at the end releases with
    /// `at_end: true`, which `ReachedEnd` cannot re-arm until the viewport has
    /// left the end band and come back.
    pub fn transition(self, event: TimelineFollowEvent) -> Self {
        match event {
            TimelineFollowEvent::Navigated => match self {
                Self::Released { .. } => self,
                Self::Following => Self::Released { at_end: true },
            },
            TimelineFollowEvent::FollowEnd => Self::Following,
            TimelineFollowEvent::ReachedEnd => match self {
                Self::Released { at_end: false } => Self::Following,
                other => other,
            },
            TimelineFollowEvent::LeftEnd => match self {
                Self::Released { at_end: true } => Self::Released { at_end: false },
                other => other,
            },
        }
    }
}

/// An input that may carry the viewport away from the live end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineNavigation {
    Wheel { up: bool },
    Key { up: bool },
    Pointer { on_scrollbar: bool },
    Command { releases_follow: bool },
}

/// The two viewport facts the release rules read.
#[derive(Clone, Copy, Debug)]
pub struct ViewportReach {
    /// The content can still scroll up.
    pub scrolls_up: bool,
    /// The viewport sits outside the end band.
    pub away_from_end: bool,
}

/// Whether a navigation releases follow. Downward input counts only outside
/// the end band: streaming growth leaves the band for a frame, and a gesture
/// then must not release follow.
pub fn navigation_releases_follow(navigation: TimelineNavigation, reach: ViewportReach) -> bool {
    match navigation {
        TimelineNavigation::Wheel { up } => up && reach.scrolls_up,
        TimelineNavigation::Key { up: true } => reach.scrolls_up,
        TimelineNavigation::Key { up: false } => reach.away_from_end,
        TimelineNavigation::Pointer { on_scrollbar: true } => reach.scrolls_up,
        TimelineNavigation::Pointer { on_scrollbar: false } => reach.away_from_end,
        TimelineNavigation::Command { releases_follow } => releases_follow,
    }
}
