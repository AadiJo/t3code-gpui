//! Back and forward through visited places. Ports `navigationHistory.ts`,
//! which leans on the browser's history: a visit drops the forward entries,
//! back and forward move along the list, and revisiting the current place
//! is not a new entry. The router owns one and handles the
//! `navigation.back` / `navigation.forward` command actions
//! ([`crate::shortcuts::commands::NavigateBack`] and `NavigateForward`).

/// Which way to move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryDirection {
    Back,
    Forward,
}

/// Places visited, oldest first, and where the user is in that list.
#[derive(Clone, Debug)]
pub struct NavigationHistory<L> {
    entries: Vec<L>,
    index: usize,
    limit: usize,
}

impl<L: Clone + PartialEq> NavigationHistory<L> {
    /// A history starting at `initial`, keeping at most `limit` entries.
    pub fn new(initial: L, limit: usize) -> Self {
        Self { entries: vec![initial], index: 0, limit: limit.max(1) }
    }

    pub fn current(&self) -> &L {
        &self.entries[self.index]
    }

    /// Records a visit, like `router.navigate`: forward entries are dropped.
    pub fn push(&mut self, location: L) {
        if *self.current() == location {
            return;
        }
        self.entries.truncate(self.index + 1);
        self.entries.push(location);
        if self.entries.len() > self.limit {
            self.entries.remove(0);
        }
        self.index = self.entries.len() - 1;
    }

    /// Changes the current entry in place, like `navigate({ replace: true })`.
    pub fn replace(&mut self, location: L) {
        self.entries[self.index] = location;
    }

    /// `canNavigateHistory`.
    pub fn can_navigate(&self, direction: HistoryDirection) -> bool {
        match direction {
            HistoryDirection::Back => self.index > 0,
            HistoryDirection::Forward => self.index + 1 < self.entries.len(),
        }
    }

    /// Moves one entry and returns where to go, or `None` at the end (`navigateHistory`).
    pub fn navigate(&mut self, direction: HistoryDirection) -> Option<&L> {
        if !self.can_navigate(direction) {
            return None;
        }
        match direction {
            HistoryDirection::Back => self.index -= 1,
            HistoryDirection::Forward => self.index += 1,
        }
        Some(&self.entries[self.index])
    }
}
