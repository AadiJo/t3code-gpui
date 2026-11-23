//! Permission (runtime) modes as the composer shows them. Ports
//! `runtimeModeConfig.ts` and the chip of `ComposerFooterControls.tsx`.

use gpui_kit::SharedString;

use crate::components::chat::composer_icons::Icon;

/// How much a thread's agent may do without asking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RuntimeMode {
    /// Ask before commands and file changes.
    #[default]
    ApprovalRequired,
    /// Auto-approve edits, ask before other actions.
    AutoAcceptEdits,
    /// Supported providers approve routine actions; others still ask.
    Auto,
    /// Allow commands and edits without prompts.
    FullAccess,
}

impl RuntimeMode {
    pub const ALL: [Self; 4] = [Self::ApprovalRequired, Self::AutoAcceptEdits, Self::Auto, Self::FullAccess];

    pub fn label(self) -> SharedString {
        match self {
            Self::ApprovalRequired => "Supervised",
            Self::AutoAcceptEdits => "Auto-accept edits",
            Self::Auto => "Auto",
            Self::FullAccess => "Full access",
        }
        .into()
    }

    pub fn description(self) -> SharedString {
        match self {
            Self::ApprovalRequired => "Ask before commands and file changes.",
            Self::AutoAcceptEdits => "Auto-approve edits, ask before other actions.",
            Self::Auto => "Supported providers approve routine actions; others still ask.",
            Self::FullAccess => "Allow commands and edits without prompts.",
        }
        .into()
    }

    /// The next mode, for cycling from a single control.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// The mode's icon.
pub fn runtime_mode_icon(mode: RuntimeMode) -> Icon {
    match mode {
        RuntimeMode::ApprovalRequired => Icon::Lock,
        RuntimeMode::AutoAcceptEdits => Icon::PenLine,
        RuntimeMode::Auto => Icon::ShieldCheck,
        // An open lock reads like a closed one at small sizes; a crossed shield does not.
        RuntimeMode::FullAccess => Icon::ShieldOff,
    }
}
