//! How each usage provider is labelled and coloured. Ports `usageProviders.ts`.

use gpui_kit::{AnyElement, App, Hsla, IntoElement as _, ParentElement as _, Pixels, Styled as _, div, rgb, svg};
use t3code_contracts::UsageProviderKind;
use t3code_shared::usage_merge::ProviderTotals;

use crate::AppTheme;
use crate::components::chat::composer_icons::provider_icon;
use crate::components::chat::composer_theme::ComposerColors;
use crate::components::icons::BrandIcon;
use crate::model_selection::{ProviderDriverKind, ProviderIcon};
use crate::theme_color::mix_oklab;

/// Providers in the order the page lists them (`PROVIDER_ORDER`).
pub const PROVIDER_ORDER: [UsageProviderKind; 8] = [
    UsageProviderKind::Codex,
    UsageProviderKind::Claude,
    UsageProviderKind::Grok,
    UsageProviderKind::Cursor,
    UsageProviderKind::Opencode,
    UsageProviderKind::Antigravity,
    UsageProviderKind::Pi,
    UsageProviderKind::Muse,
];

/// The provider's name on the page.
pub fn provider_label(provider: UsageProviderKind) -> &'static str {
    match provider {
        UsageProviderKind::Codex => "Codex",
        UsageProviderKind::Claude => "Claude Code",
        UsageProviderKind::Grok => "Grok Build",
        UsageProviderKind::Cursor => "Cursor",
        UsageProviderKind::Opencode => "OpenCode",
        UsageProviderKind::Antigravity => "Antigravity",
        UsageProviderKind::Pi => "Pi",
        UsageProviderKind::Muse => "Muse Code",
    }
}

/// The driver a provider's sessions run on.
pub fn provider_driver(provider: UsageProviderKind) -> &'static str {
    match provider {
        UsageProviderKind::Claude => "claudeAgent",
        other => other.as_str(),
    }
}

/// The series colour: Codex follows the foreground, Grok a softened foreground.
pub fn provider_color(provider: UsageProviderKind, theme: &AppTheme) -> Hsla {
    let foreground = theme.colors.foreground();
    match provider {
        UsageProviderKind::Codex => foreground,
        UsageProviderKind::Claude => rgb(0xd97757).into(),
        UsageProviderKind::Grok => mix_oklab(foreground, 0.72, theme.colors.background()),
        UsageProviderKind::Cursor => rgb(0x8b8b8b).into(),
        UsageProviderKind::Opencode => rgb(0x5b9bbd).into(),
        UsageProviderKind::Antigravity => rgb(0x8c7bd1).into(),
        UsageProviderKind::Pi => rgb(0x4d9f8a).into(),
        UsageProviderKind::Muse => rgb(0x0866ff).into(),
    }
}

/// `barColor`: a limit bar's colour for a driver, the foreground when no usage provider uses it.
pub fn driver_color(driver: &str, theme: &AppTheme) -> Hsla {
    PROVIDER_ORDER
        .into_iter()
        .find(|provider| provider_driver(*provider) == driver)
        .map_or(theme.colors.foreground(), |provider| provider_color(provider, theme))
}

/// The label a limits pool uses for a driver (`providerClients` labels).
pub fn driver_label(driver: &str) -> String {
    match driver {
        "codex" => "Codex".into(),
        "claudeAgent" => "Claude".into(),
        "cursor" => "Cursor".into(),
        "grok" => "Grok".into(),
        "opencode" => "OpenCode".into(),
        "antigravity" => "Antigravity".into(),
        "pi" => "Pi".into(),
        "muse" => "Muse Code".into(),
        "kiro" => "Kiro".into(),
        other => other.into(),
    }
}

/// A driver's mark at `size` (`ProviderInstanceIcon`).
pub fn driver_mark(driver: &str, label: &str, size: Pixels, cx: &App) -> AnyElement {
    if driver == "antigravity" {
        return svg().path(BrandIcon::Antigravity.path()).size(size).flex_none().into_any_element();
    }
    let kind = match driver {
        "codex" => ProviderDriverKind::Codex,
        "claudeAgent" => ProviderDriverKind::ClaudeAgent,
        "cursor" => ProviderDriverKind::Cursor,
        "opencode" => ProviderDriverKind::OpenCode,
        "grok" => ProviderDriverKind::Grok,
        other => ProviderDriverKind::Other(other.to_owned().into()),
    };
    let colors = ComposerColors::new(cx);
    div().flex_none().child(provider_icon(ProviderIcon::for_model(&kind, None), size, label, &colors, cx)).into_any_element()
}

/// A usage provider's mark.
pub fn provider_mark(provider: UsageProviderKind, size: Pixels, cx: &App) -> AnyElement {
    driver_mark(provider_driver(provider), provider_label(provider), size, cx)
}

/// Providers with any tokens or cost, in page order.
pub fn providers_with_usage(totals: &[ProviderTotals]) -> Vec<UsageProviderKind> {
    PROVIDER_ORDER
        .into_iter()
        .filter(|provider| totals.iter().any(|t| t.provider == *provider && (t.total_tokens > 0.0 || t.cost_usd > 0.0)))
        .collect()
}

/// `rankProvidersByMetric`: highest first, ties in their given order.
pub fn rank_providers_by_metric(providers: &[UsageProviderKind], totals: &[ProviderTotals], by_cost: bool) -> Vec<UsageProviderKind> {
    let value = |provider: &UsageProviderKind| {
        totals
            .iter()
            .find(|t| t.provider == *provider)
            .map_or(0.0, |t| if by_cost { t.cost_usd } else { t.total_tokens })
    };
    let mut ranked = providers.to_vec();
    ranked.sort_by(|a, b| value(b).total_cmp(&value(a)));
    ranked
}
