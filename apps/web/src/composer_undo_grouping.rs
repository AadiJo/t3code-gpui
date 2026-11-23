//! Undo history for the composer. Ports `composer-undo-grouping.ts` plus the
//! ProseMirror history it configures: typing merges into one step until the
//! change kind changes or a second passes, and paste, cut and drop always
//! stand alone. IME composition counts as typing.

use std::ops::Range;
use std::time::{Duration, Instant};

/// Edits closer together than this, of the same kind, undo as one step.
pub const COMPOSER_UNDO_GROUP_DELAY: Duration = Duration::from_millis(1000);
const MAX_UNDO_DEPTH: usize = 200;

/// The kind of a change, for grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerChangeKind {
    Insert,
    Delete,
    /// Replacements, clipboard edits, drops and programmatic rewrites.
    Other,
}

impl ComposerChangeKind {
    /// Classifies a replacement of `range` with `inserted` bytes.
    pub fn of(range: &Range<usize>, inserted: usize) -> Self {
        match (range.is_empty(), inserted == 0) {
            (true, false) => Self::Insert,
            (false, true) => Self::Delete,
            _ => Self::Other,
        }
    }
}

/// The draft as undo restores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoSnapshot {
    pub text: String,
    pub selection: Range<usize>,
}

/// Undo and redo stacks with change-kind grouping.
#[derive(Debug, Default)]
pub struct UndoHistory {
    undo: Vec<UndoSnapshot>,
    redo: Vec<UndoSnapshot>,
    last_kind: Option<ComposerChangeKind>,
    last_at: Option<Instant>,
}

impl UndoHistory {
    /// Records the state before a change. Opens a new step unless the change
    /// continues the previous one. Ports `groupUndoByChangeKind`.
    pub fn record(&mut self, before: UndoSnapshot, kind: ComposerChangeKind, now: Instant) {
        let continues = self.last_kind == Some(kind)
            && kind != ComposerChangeKind::Other
            && self.last_at.is_some_and(|at| now.duration_since(at) < COMPOSER_UNDO_GROUP_DELAY);
        if !continues {
            self.undo.push(before);
            if self.undo.len() > MAX_UNDO_DEPTH {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_kind = Some(kind);
        self.last_at = Some(now);
    }

    /// Ends the current step, so the next change starts a new one.
    pub fn close_group(&mut self) {
        self.last_kind = None;
    }

    /// Returns the state to restore, given the current one.
    pub fn undo(&mut self, current: UndoSnapshot) -> Option<UndoSnapshot> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        self.last_kind = None;
        Some(previous)
    }

    /// Returns the state to restore after an undo.
    pub fn redo(&mut self, current: UndoSnapshot) -> Option<UndoSnapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        self.last_kind = None;
        Some(next)
    }

    /// Drops all history, as after a send or a thread switch.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}
