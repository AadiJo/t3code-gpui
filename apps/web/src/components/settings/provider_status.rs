//! How a provider's health reads in the Providers panel. Ports
//! `settings/providerStatus.ts`.

use gpui_kit::{Hsla, SharedString};
use t3code_contracts::{
    ServerProviderAuthStatus, ServerProviderCompatibilityAdvisory, ServerProviderCompatibilityStatus,
    ServerProviderState,
};

use crate::AppTheme;
use crate::i18n::t;
use crate::provider_instances::ServerProvider;
use crate::theme_color::with_alpha;

/// `ProviderStatusKey`: which dot colour a provider gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderStatusKey {
    Disabled,
    Error,
    Neutral,
    Ready,
    Warning,
}

impl ProviderStatusKey {
    /// `PROVIDER_STATUS_STYLES[key].dot`.
    pub fn dot(self, theme: &AppTheme) -> Hsla {
        let c = &theme.colors;
        match self {
            Self::Disabled => with_alpha(c.muted_foreground, 0.5),
            Self::Error => c.destructive(),
            Self::Neutral => c.muted_foreground,
            Self::Ready => theme.fixed.success,
            Self::Warning => c.warning,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Error => "error",
            Self::Neutral => "neutral",
            Self::Ready => "ready",
            Self::Warning => "warning",
        }
    }
}

/// `providerAuthLabel`: a custom endpoint gets a translated label; others are plan names.
pub fn provider_auth_label(label: Option<&str>, auth_type: Option<&str>) -> Option<SharedString> {
    if auth_type == Some("customEndpoint") {
        return Some(t!("Custom API endpoint"));
    }
    label.or(auth_type).map(|s| SharedString::from(s.to_owned()))
}

fn is_antigravity_sign_in_unchecked(provider: &ServerProvider) -> bool {
    provider.driver == "antigravity"
        && provider.installed
        && provider.status == ServerProviderState::Warning
        && provider.auth.status == ServerProviderAuthStatus::Unknown
        && matches!(provider.auth.r#type.as_deref(), None | Some("oauth-personal" | "oauth-business"))
}

/// `getProviderStatusKey`.
pub fn get_provider_status_key(provider: Option<&ServerProvider>) -> ProviderStatusKey {
    let Some(provider) = provider else { return ProviderStatusKey::Warning };
    if !provider.enabled {
        return ProviderStatusKey::Disabled;
    }
    if is_antigravity_sign_in_unchecked(provider) {
        return ProviderStatusKey::Neutral;
    }
    match provider.status {
        ServerProviderState::Ready => ProviderStatusKey::Ready,
        ServerProviderState::Error => ProviderStatusKey::Error,
        ServerProviderState::Disabled => ProviderStatusKey::Disabled,
        _ => ProviderStatusKey::Warning,
    }
}

/// A status headline and its optional detail.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderSummary {
    pub headline: SharedString,
    pub detail: Option<SharedString>,
}

/// `getProviderSummary`.
pub fn get_provider_summary(provider: Option<&ServerProvider>) -> ProviderSummary {
    let message = |p: &ServerProvider| p.message.clone().map(SharedString::from);
    let Some(p) = provider else {
        return ProviderSummary {
            headline: t!("Checking provider status"),
            detail: Some(t!("Waiting for the server to report installation and authentication details.")),
        };
    };
    if !p.enabled || p.status == ServerProviderState::Disabled {
        return ProviderSummary {
            headline: t!("Disabled", context = "provider status"),
            detail: Some(message(p).unwrap_or_else(|| t!("This provider is installed but disabled for new sessions in T3 Code."))),
        };
    }
    if !p.installed {
        return ProviderSummary {
            headline: t!("Not found", context = "provider status, CLI is missing"),
            detail: Some(message(p).unwrap_or_else(|| t!("CLI not detected on PATH."))),
        };
    }
    let auth_label = || provider_auth_label(p.auth.label.as_deref(), p.auth.r#type.as_deref());
    if p.auth.status == ServerProviderAuthStatus::Unauthenticated {
        return ProviderSummary {
            headline: match auth_label() {
                Some(method) => t!("Not authenticated · {method}", method = method),
                None => t!("Not authenticated"),
            },
            detail: message(p),
        };
    }
    if is_antigravity_sign_in_unchecked(p) {
        return ProviderSummary { headline: t!("Installed · Sign-in not checked"), detail: message(p) };
    }
    match p.status {
        ServerProviderState::Warning => ProviderSummary {
            headline: t!("Needs attention"),
            detail: Some(
                message(p).unwrap_or_else(|| t!("The provider is installed, but the server could not fully verify it.")),
            ),
        },
        ServerProviderState::Error => ProviderSummary {
            headline: t!("Unavailable", context = "provider status"),
            detail: Some(message(p).unwrap_or_else(|| t!("The provider failed its startup checks."))),
        },
        _ if p.auth.status == ServerProviderAuthStatus::Authenticated => ProviderSummary {
            headline: match auth_label() {
                Some(method) => t!("Authenticated · {method}", method = method),
                None => t!("Authenticated"),
            },
            detail: message(p),
        },
        _ => ProviderSummary { headline: t!("Available", context = "provider status"), detail: message(p) },
    }
}

/// `getProviderVersionLabel`: `v1.2.3`, or a date for Antigravity's build ids.
pub fn get_provider_version_label(version: Option<&str>) -> Option<String> {
    let version = version.filter(|v| !v.is_empty())?;
    if let Some(rest) = version.strip_prefix("agy_acp_server_") {
        let (date, tail) = rest.split_at(rest.len().min(8));
        let mut tail_parts = tail.strip_prefix('_').map(|t| t.splitn(2, '_'));
        let build_ok = tail_parts.as_mut().and_then(|p| p.next()).is_some_and(|b| !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()));
        let candidate = tail_parts.as_mut().and_then(|p| p.next());
        let candidate_ok = candidate.is_none_or(|c| !c.is_empty() && c.chars().all(|ch| ch.is_alphanumeric() || ch == '_'));
        if date.len() == 8 && date.bytes().all(|c| c.is_ascii_digit()) && build_ok && candidate_ok {
            let label = format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..]);
            return Some(match candidate {
                Some(c) => format!("{label} {c}"),
                None => label,
            });
        }
    }
    Some(if version.starts_with(|c: char| c.is_ascii_digit()) { format!("v{version}") } else { version.to_owned() })
}

/// How loudly a version advisory reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvisoryEmphasis {
    Normal,
    Strong,
}

/// A version advisory as the card's popover shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionAdvisoryPresentation {
    pub title: SharedString,
    pub detail: SharedString,
    pub update_command: Option<String>,
    pub emphasis: AdvisoryEmphasis,
    pub target_version: Option<String>,
}

/// `getProviderVersionAdvisoryPresentation`.
pub fn get_provider_version_advisory_presentation(
    provider: &ServerProvider,
    show_compatibility: bool,
) -> Option<VersionAdvisoryPresentation> {
    let advisory = provider.version_advisory.as_ref();
    let compatibility: Option<&ServerProviderCompatibilityAdvisory> = provider.compatibility_advisory.as_ref();
    let latest_incompatible = compatibility.is_some_and(|c| {
        matches!(
            c.latest_version_status,
            Some(ServerProviderCompatibilityStatus::Broken | ServerProviderCompatibilityStatus::Unsupported)
        )
    });
    if show_compatibility
        && let Some(c) = compatibility
        && matches!(
            c.status,
            ServerProviderCompatibilityStatus::Graceful
                | ServerProviderCompatibilityStatus::Unsupported
                | ServerProviderCompatibilityStatus::Broken
        )
    {
        let target = c.recommended_version.clone();
        let recommendation = get_provider_version_label(target.as_deref()).or_else(|| c.recommended_range.clone());
        let already_latest = advisory.is_some_and(|a| a.status.as_str() == "current");
        return Some(VersionAdvisoryPresentation {
            title: match c.status {
                ServerProviderCompatibilityStatus::Graceful => t!("Limited support"),
                ServerProviderCompatibilityStatus::Unsupported => t!("Unsupported version"),
                _ => t!("Known broken version"),
            },
            detail: c.message.clone().map(SharedString::from).unwrap_or_else(|| match &recommendation {
                Some(version) => t!("Use {version} for full support.", version = version),
                None => t!("Update for full support."),
            }),
            update_command: if target.is_some() || latest_incompatible || already_latest {
                None
            } else {
                advisory.and_then(|a| a.update_command.clone())
            },
            emphasis: if c.status == ServerProviderCompatibilityStatus::Graceful {
                AdvisoryEmphasis::Normal
            } else {
                AdvisoryEmphasis::Strong
            },
            target_version: target,
        });
    }
    let advisory = advisory?;
    if matches!(advisory.status.as_str(), "current" | "unknown") || latest_incompatible {
        return None;
    }
    let label = get_provider_version_label(advisory.latest_version.as_deref());
    Some(VersionAdvisoryPresentation {
        title: t!("Update available"),
        detail: advisory.message.clone().map(SharedString::from).unwrap_or_else(|| match &label {
            Some(version) => t!("Update available: install {version}.", version = version),
            None => t!("Update available: install the latest provider version."),
        }),
        update_command: advisory.update_command.clone(),
        emphasis: AdvisoryEmphasis::Normal,
        target_version: None,
    })
}
