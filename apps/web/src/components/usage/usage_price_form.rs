//! Ports `usagePriceForm.ts`: the four price columns, the text form of one
//! model's override, edit filtering and parsing back to an override.

use std::sync::LazyLock;

use regex::Regex;
use t3code_contracts::UsageModelPriceOverride;

use crate::i18n::t;

/// One price column (`USAGE_PRICE_FIELDS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UsagePriceField {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

impl UsagePriceField {
    pub const ALL: [Self; 4] = [Self::Input, Self::Output, Self::CacheRead, Self::CacheWrite];

    pub fn label(self) -> gpui_kit::SharedString {
        match self {
            Self::Input => t!("Input"),
            Self::Output => t!("Output"),
            Self::CacheRead => t!("Cache read"),
            Self::CacheWrite => t!("Cache write"),
        }
    }

    /// A stable name for element ids.
    pub fn key(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::CacheRead => "cache-read",
            Self::CacheWrite => "cache-write",
        }
    }

    /// Cache rates fall back to the input rate when blank.
    pub fn optional(self) -> bool {
        matches!(self, Self::CacheRead | Self::CacheWrite)
    }

    fn index(self) -> usize {
        self as usize
    }

    fn read(self, price: &UsageModelPriceOverride) -> Option<f64> {
        match self {
            Self::Input => Some(price.input_cost_per_million_tokens),
            Self::Output => Some(price.output_cost_per_million_tokens),
            Self::CacheRead => price.cache_read_cost_per_million_tokens,
            Self::CacheWrite => price.cache_write_cost_per_million_tokens,
        }
    }
}

/// One model's prices as the text the cells show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsagePriceForm {
    pub model: String,
    pub values: [String; 4],
}

impl UsagePriceForm {
    pub fn get(&self, field: UsagePriceField) -> &str {
        &self.values[field.index()]
    }

    pub fn set(&mut self, field: UsagePriceField, value: String) {
        self.values[field.index()] = value;
    }
}

/// A number as JavaScript's `Number.prototype.toString` writes it.
fn js_number(value: f64) -> String {
    if value != 0.0 && value.abs() < 1e-6 { format!("{value:e}") } else { format!("{value}") }
}

/// `usagePriceForm`: blank cells where `price` has no rate.
pub fn usage_price_form(model: &str, price: Option<&UsageModelPriceOverride>) -> UsagePriceForm {
    let values = UsagePriceField::ALL.map(|field| price.and_then(|p| field.read(p)).map(js_number).unwrap_or_default());
    UsagePriceForm { model: model.to_owned(), values }
}

static EDIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:\d*[.,]?\d*|(?:\d+[.,]?\d*|[.,]\d+)e[+-]?\d*)\s*$").expect("edit pattern"));
static RATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$").expect("rate pattern"));

/// Allows decimal and exponent edits and every deletion, even when the
/// result is not yet a valid number; saving validates it.
pub fn is_usage_price_edit(previous: &str, next: &str) -> bool {
    if EDIT.is_match(next) {
        return true;
    }
    let (previous, next): (Vec<char>, Vec<char>) = (previous.chars().collect(), next.chars().collect());
    if next.len() >= previous.len() {
        return false;
    }
    let start = previous.iter().zip(&next).take_while(|(a, b)| a == b).count();
    next[start..] == previous[previous.len() - (next.len() - start)..]
}

/// `parseUsagePriceForm`: blank cache prices use the input rate; zero means free.
pub fn parse_usage_price_form(form: &UsagePriceForm) -> Option<(String, UsageModelPriceOverride)> {
    let model = form.model.trim();
    if model.is_empty() {
        return None;
    }
    let mut rates = [None; 4];
    for field in UsagePriceField::ALL {
        let raw = form.get(field).trim().replacen(',', ".", 1);
        if raw.is_empty() {
            if field.optional() {
                continue;
            }
            return None;
        }
        if !RATE.is_match(&raw) {
            return None;
        }
        let value: f64 = raw.parse().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        rates[field.index()] = Some(value);
    }
    Some((
        model.to_owned(),
        UsageModelPriceOverride {
            input_cost_per_million_tokens: rates[0]?,
            output_cost_per_million_tokens: rates[1]?,
            cache_read_cost_per_million_tokens: rates[2],
            cache_write_cost_per_million_tokens: rates[3],
        },
    ))
}
