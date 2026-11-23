//! Helpers over environment types (`environment.ts`).

use crate::RepositoryIdentity;

impl RepositoryIdentity {
    /// The key that groups checkouts of one repository (`repositoryGroupingKeyOf`): the origin's
    /// key when the repository has one.
    pub fn grouping_key(&self) -> &str {
        self.origin.as_ref().map_or(&self.canonical_key, |o| &o.canonical_key)
    }

    /// The name shown for that group (`repositoryGroupingDisplayNameOf`).
    pub fn grouping_display_name(&self) -> Option<&str> {
        match &self.origin {
            Some(origin) => Some(origin.display_name.as_deref().unwrap_or(&origin.canonical_key)),
            None => self.display_name.as_deref(),
        }
    }
}
