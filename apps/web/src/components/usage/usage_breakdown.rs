//! Share bar segments and per-model figures. Ports `usageBreakdown.ts`.

use gpui_kit::{Hsla, SharedString};
use t3code_shared::usage_merge::{CategoryCost, ModelTotals, SpeedCost, TokenTotals};

use crate::AppTheme;
use crate::i18n::t;
use crate::theme_color::mix_oklab;

/// One part of a share bar.
#[derive(Clone, Debug, PartialEq)]
pub struct ShareSegment {
    pub label: SharedString,
    pub value: f64,
    pub color: Hsla,
}

/// Models by tokens, most first; cost breaks ties.
pub fn sort_models_by_tokens(models: &[ModelTotals]) -> Vec<ModelTotals> {
    let mut sorted = models.to_vec();
    sorted.sort_by(|a, b| b.total_tokens.total_cmp(&a.total_tokens).then(b.cost_usd.total_cmp(&a.cost_usd)));
    sorted
}

/// A model's share of the metric; an unknown cost has no cost share.
pub fn model_share(model: &ModelTotals, by_cost: bool) -> Option<f64> {
    if !by_cost {
        return Some(model.token_share);
    }
    (!model.is_cost_unknown()).then_some(model.cost_share)
}

/// Cached input over all input; cache writes count as misses.
pub fn cache_hit_rate(model: &ModelTotals) -> Option<f64> {
    let t = &model.tokens;
    let input = t.uncached_input_tokens + t.cached_input_tokens + t.cache_creation_tokens;
    (input != 0.0).then(|| t.cached_input_tokens / input)
}

/// Effective USD per million priced tokens.
pub fn cost_per_million_tokens(model: &ModelTotals) -> Option<f64> {
    let priced = model.total_tokens - model.unpriced_tokens;
    (priced > 0.0 && !model.is_cost_unknown()).then(|| model.cost_usd / priced * 1_000_000.0)
}

/// `color-mix(in oklab, var(--foreground) p%, var(--background))`.
fn ink(theme: &AppTheme, percent: f32) -> Hsla {
    mix_oklab(theme.colors.foreground(), percent / 100.0, theme.colors.background())
}

/// "Cost by type". Unsplit cost under half a cent is dropped.
pub fn cost_type_segments(cost: &CategoryCost, theme: &AppTheme) -> Vec<ShareSegment> {
    vec![
        ShareSegment { label: t!("Input"), value: cost.input, color: ink(theme, 60.) },
        ShareSegment { label: t!("Cache read"), value: cost.cache_read, color: ink(theme, 30.) },
        ShareSegment { label: t!("Cache write"), value: cost.cache_write, color: ink(theme, 72.) },
        ShareSegment { label: t!("Output"), value: cost.output, color: ink(theme, 100.) },
        ShareSegment { label: t!("Other"), value: if cost.unsplit >= 0.005 { cost.unsplit } else { 0.0 }, color: ink(theme, 44.) },
    ]
}

/// "Tokens by type".
pub fn token_type_segments(tokens: &TokenTotals, theme: &AppTheme) -> Vec<ShareSegment> {
    vec![
        ShareSegment { label: t!("Input"), value: tokens.uncached_input_tokens, color: ink(theme, 60.) },
        ShareSegment { label: t!("Cache read"), value: tokens.cached_input_tokens, color: ink(theme, 30.) },
        ShareSegment { label: t!("Cache write"), value: tokens.cache_creation_tokens, color: ink(theme, 72.) },
        ShareSegment { label: t!("Output"), value: tokens.output_tokens, color: ink(theme, 100.) },
    ]
}

/// "Cost by speed".
pub fn speed_cost_segments(cost: &SpeedCost, theme: &AppTheme) -> Vec<ShareSegment> {
    vec![
        ShareSegment { label: t!("Standard"), value: cost.standard, color: ink(theme, 34.) },
        ShareSegment { label: t!("Fast"), value: cost.fast, color: ink(theme, 66.) },
        ShareSegment { label: t!("Ultrafast"), value: cost.ultrafast, color: ink(theme, 100.) },
    ]
}
