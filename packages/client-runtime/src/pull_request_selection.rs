//! Picker drafts for labels, reviewers and assignees: picks that differ from
//! the host, sent as one change when the picker closes. Ports
//! `pullRequestSelection.ts`.

/// The most items one label or reviewer write may carry.
pub const PULL_REQUEST_CHANGE_BATCH_LIMIT: usize = 25;

/// Picks that differ from the host, keyed by candidate, in the order a `Map` keeps them.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionDraft<T> {
    entries: Vec<(String, T, bool)>,
}

impl<T> Default for SelectionDraft<T> {
    fn default() -> Self {
        Self { entries: Vec::new() }
    }
}

impl<T: Clone> SelectionDraft<T> {
    /// No picks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether nothing differs from the host.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The pick for `key`, or the host's state when there is none.
    pub fn is_selected(&self, key: &str, host_selected: bool) -> bool {
        self.entries.iter().find(|(k, ..)| k == key).map_or(host_selected, |(.., selected)| *selected)
    }

    /// Flips `key`. A pick that lands back on the host's state is dropped.
    pub fn toggle(&self, key: &str, item: T, host_selected: bool) -> Self {
        let selected = !self.is_selected(key, host_selected);
        let mut next = self.clone();
        let position = next.entries.iter().position(|(k, ..)| k == key);
        match (selected == host_selected, position) {
            (true, Some(index)) => {
                next.entries.remove(index);
            }
            (true, None) => {}
            (false, Some(index)) => next.entries[index] = (key.to_owned(), item, selected),
            (false, None) => next.entries.push((key.to_owned(), item, selected)),
        }
        next
    }

    /// What to send on close, diffed against the host's latest state rather than the state when
    /// the pick was made, so a refresh while the picker was open does not resend anything.
    pub fn changes(&self, host_selected: impl Fn(&str, &T) -> bool) -> SelectionChanges<T> {
        let mut changes = SelectionChanges { add: Vec::new(), remove: Vec::new() };
        for (key, item, selected) in &self.entries {
            if *selected == host_selected(key, item) {
                continue;
            }
            if *selected { changes.add.push(item.clone()) } else { changes.remove.push(item.clone()) }
        }
        changes
    }
}

/// Items to add and remove when a picker closes.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionChanges<T> {
    pub add: Vec<T>,
    pub remove: Vec<T>,
}

/// Splits a change into writes the contract accepts.
pub fn change_batches<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    items.chunks(PULL_REQUEST_CHANGE_BATCH_LIMIT).map(<[T]>::to_vec).collect()
}
