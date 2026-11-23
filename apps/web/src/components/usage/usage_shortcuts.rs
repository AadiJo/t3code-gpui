//! The page's metric and period choices and their commands. Ports
//! `usageShortcuts.ts` and `usagePagePreferences.ts`.

use std::path::Path;

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};
use t3code_contracts::UsageProviderKind;

use crate::i18n::t;
use t3code_shared::keybindings::StaticCommand;

/// What the page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageMetric {
    Cost,
    Tokens,
    #[default]
    Limits,
}

impl UsageMetric {
    pub const ALL: [UsageMetric; 3] = [Self::Cost, Self::Tokens, Self::Limits];

    pub fn label(self) -> SharedString {
        match self {
            Self::Cost => t!("Cost"),
            Self::Tokens => t!("Tokens"),
            Self::Limits => t!("Limits"),
        }
    }

    /// The command that selects it (`usage.cost`, …).
    pub fn command(self) -> StaticCommand {
        match self {
            Self::Cost => StaticCommand::UsageCost,
            Self::Tokens => StaticCommand::UsageTokens,
            Self::Limits => StaticCommand::UsageLimits,
        }
    }
}

/// A usage period in days.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum UsageWindowDays {
    Day = 1,
    Week = 7,
    Month = 30,
    Quarter = 90,
}

impl UsageWindowDays {
    pub const ALL: [UsageWindowDays; 4] = [Self::Day, Self::Week, Self::Month, Self::Quarter];

    pub fn days(self) -> u32 {
        self as u32
    }

    pub fn label(self) -> SharedString {
        match self {
            Self::Day => t!("Past 24h"),
            Self::Week => t!("7 days"),
            Self::Month => t!("30 days"),
            Self::Quarter => t!("90 days"),
        }
    }

    pub fn command(self) -> StaticCommand {
        match self {
            Self::Day => StaticCommand::UsagePeriodDay,
            Self::Week => StaticCommand::UsagePeriodWeek,
            Self::Month => StaticCommand::UsagePeriodMonth,
            Self::Quarter => StaticCommand::UsagePeriodQuarter,
        }
    }
}

impl TryFrom<u32> for UsageWindowDays {
    type Error = String;
    fn try_from(days: u32) -> Result<Self, Self::Error> {
        Self::ALL.into_iter().find(|d| d.days() == days).ok_or_else(|| format!("unsupported window {days}"))
    }
}

impl From<UsageWindowDays> for u32 {
    fn from(days: UsageWindowDays) -> Self {
        days.days()
    }
}

/// The page's remembered choices (`t3code:usage-page-preferences:v1`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePagePreferences {
    pub metric: UsageMetric,
    pub window_days: UsageWindowDays,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_providers: Vec<UsageProviderKind>,
}

impl Default for UsagePagePreferences {
    fn default() -> Self {
        Self { metric: UsageMetric::Limits, window_days: UsageWindowDays::Month, hidden_providers: Vec::new() }
    }
}

impl UsagePagePreferences {
    /// Reads the saved choices; anything unreadable is the default.
    pub fn read(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    /// Saves the choices; a failed write only loses them for the next launch.
    pub fn save(&self, path: &Path) {
        if let Ok(bytes) = serde_json::to_vec(self)
            && let Err(error) = std::fs::write(path, bytes)
        {
            tracing::warn!(%error, "could not save usage page preferences");
        }
    }
}
