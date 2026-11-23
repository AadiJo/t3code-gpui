//! Scope checks clients run on a session (`auth.ts`). The server stays authoritative; these only
//! decide what a client shows.

use crate::{AuthEnvironmentScope, AuthSessionState};
use AuthEnvironmentScope as Scope;

impl AuthEnvironmentScope {
    /// The broader scope an old server granted instead of this one, if any. Client-only fallback;
    /// servers never expand grants.
    pub const fn legacy_parent(self) -> Option<Self> {
        match self {
            Scope::FilesystemRead | Scope::DiagnosticsRead => Some(Scope::OrchestrationRead),
            Scope::SettingsWrite
            | Scope::ProvidersManage
            | Scope::EnvironmentMaintain
            | Scope::PreviewOperate
            | Scope::SourceControlWrite
            | Scope::FilesystemWrite => Some(Scope::OrchestrationOperate),
            Scope::TerminalRead => Some(Scope::TerminalOperate),
            _ => None,
        }
    }

    /// Whether old servers already knew this scope.
    pub const fn is_legacy(self) -> bool {
        matches!(
            self,
            Scope::OrchestrationRead
                | Scope::OrchestrationOperate
                | Scope::TerminalOperate
                | Scope::ReviewWrite
                | Scope::AccessRead
                | Scope::AccessWrite
                | Scope::RelayRead
                | Scope::RelayWrite
        )
    }
}

impl AuthSessionState {
    /// Whether this session may use `scope` (`sessionGrantsScope`). Uses `permissions` when the
    /// server sends them, else `scopes` plus the legacy parent of `scope` for old servers.
    pub fn grants_scope(&self, scope: AuthEnvironmentScope) -> bool {
        if !self.authenticated {
            return false;
        }
        if let Some(permissions) = &self.permissions {
            return permissions.contains(&scope);
        }
        let scopes = self.scopes.as_deref().unwrap_or_default();
        if scopes.contains(&scope) {
            return true;
        }
        if self.auth.server_update_scope.is_some() {
            return false;
        }
        scope.legacy_parent().is_some_and(|parent| scopes.contains(&parent))
    }

    /// Whether the session's permissions are all legacy scopes and include a legacy parent
    /// (`sessionHasLegacyPermissions`): a grant from before fine-grained scopes.
    pub fn has_legacy_permissions(&self) -> bool {
        let Some(permissions) = &self.permissions else { return false };
        self.authenticated
            && permissions.iter().all(|scope| scope.is_legacy())
            && AuthEnvironmentScope::ALL
                .iter()
                .filter_map(|scope| scope.legacy_parent())
                .any(|parent| permissions.contains(&parent))
    }
}
