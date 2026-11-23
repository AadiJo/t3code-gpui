//! Which files in the Code tab are folded. Ports `pullRequestDiffFold.ts`: a
//! reader's own choice per file wins, then a bulk expand or fold, then the
//! Viewed mark.

use std::collections::HashMap;
use std::sync::Arc;

/// A bulk "Expand all" or "Collapse all", or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffFoldOverride {
    Expanded,
    Folded,
}

/// The reader's own fold choice per file. It is absolute, so a Viewed mark loading later keeps it.
pub type FileDiffFoldChoices = Arc<HashMap<String, bool>>;

/// Whether `file_key` is folded.
pub fn is_file_diff_collapsed(
    file_key: &str,
    fold_override: Option<DiffFoldOverride>,
    choices: &HashMap<String, bool>,
    viewed: bool,
) -> bool {
    choices.get(file_key).copied().unwrap_or(match fold_override {
        None => viewed,
        Some(o) => o == DiffFoldOverride::Folded,
    })
}

/// `choices` with `file_key` set to `collapsed`, or cleared for `None`. Returns the same `Arc`
/// when nothing changes.
pub fn with_file_diff_fold(choices: &FileDiffFoldChoices, file_key: &str, collapsed: Option<bool>) -> FileDiffFoldChoices {
    if choices.get(file_key).copied() == collapsed {
        return choices.clone();
    }
    let mut next = (**choices).clone();
    match collapsed {
        None => {
            next.remove(file_key);
        }
        Some(c) => {
            next.insert(file_key.to_owned(), c);
        }
    }
    Arc::new(next)
}

/// One group's bulk override and per-file choices.
#[derive(Debug, Clone, Default)]
pub struct DiffFoldGroup {
    pub fold_override: Option<DiffFoldOverride>,
    pub choices: FileDiffFoldChoices,
}

/// Test files keep their own group, so a bulk fold while they are hidden leaves them as they were.
#[derive(Debug, Clone, Default)]
pub struct PullRequestDiffFolds {
    pub source: DiffFoldGroup,
    pub test: DiffFoldGroup,
}

impl PullRequestDiffFolds {
    /// The group a file belongs to.
    pub fn group(&self, test: bool) -> &DiffFoldGroup {
        if test { &self.test } else { &self.source }
    }

    /// Whether `file_key` is folded.
    pub fn is_collapsed(&self, file_key: &str, test: bool, viewed: bool) -> bool {
        let group = self.group(test);
        is_file_diff_collapsed(file_key, group.fold_override, &group.choices, viewed)
    }

    /// Sets one file's own choice.
    pub fn with_fold(&self, test: bool, file_key: &str, collapsed: Option<bool>) -> Self {
        let mut next = self.clone();
        let group = if test { &mut next.test } else { &mut next.source };
        group.choices = with_file_diff_fold(&group.choices, file_key, collapsed);
        next
    }

    /// Expand or fold every file, on later pages too. While test files are hidden, only the tests
    /// still shown (a revealed one) follow; the rest keep their choices and fallback.
    pub fn fold_all(&self, fold_override: DiffFoldOverride, hidden_tests_shown_keys: Option<&[String]>) -> Self {
        let all = DiffFoldGroup { fold_override: Some(fold_override), choices: FileDiffFoldChoices::default() };
        let Some(shown) = hidden_tests_shown_keys else {
            return Self { source: all.clone(), test: all };
        };
        let folded = fold_override == DiffFoldOverride::Folded;
        let choices = shown.iter().fold(self.test.choices.clone(), |c, key| with_file_diff_fold(&c, key, Some(folded)));
        Self { source: all, test: DiffFoldGroup { fold_override: self.test.fold_override, choices } }
    }
}
