//! How a source control host names and draws change requests. Ports
//! `sourceControlPresentation.tsx` and the `changeRequest` terminology of each
//! built-in definition in `packages/source-control-*/src/shared/definition.ts`.

use gpui_kit::SharedString;
use t3code_contracts::SourceControlProviderInfo;

use crate::components::icons::IconName;

/// Selects the noun in translated sentences (`changeRequestTerm.ts`): every
/// sentence is written once per host term, so a language can inflect the whole thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeRequestTermKind {
    Pr,
    Mr,
    Cr,
}

impl ChangeRequestTermKind {
    /// The ICU `select` value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pr => "pr",
            Self::Mr => "mr",
            Self::Cr => "cr",
        }
    }
}

/// A host's words for a change request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeRequestTerminology {
    pub short_label: &'static str,
    pub singular: &'static str,
}

impl ChangeRequestTerminology {
    /// `changeRequestTermKind`.
    pub fn term_kind(self) -> ChangeRequestTermKind {
        match self.singular {
            "merge request" => ChangeRequestTermKind::Mr,
            "change request" => ChangeRequestTermKind::Cr,
            _ => ChangeRequestTermKind::Pr,
        }
    }
}

const PULL_REQUEST: ChangeRequestTerminology = ChangeRequestTerminology { short_label: "PR", singular: "pull request" };
const MERGE_REQUEST: ChangeRequestTerminology = ChangeRequestTerminology { short_label: "MR", singular: "merge request" };
const UNKNOWN: ChangeRequestTerminology =
    ChangeRequestTerminology { short_label: "change request", singular: "change request" };

/// One built-in host definition, reduced to what the UI reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceControlDefinition {
    pub kind: &'static str,
    pub label: &'static str,
    pub terminology: ChangeRequestTerminology,
}

/// The built-in hosts, in registry order.
pub const SOURCE_CONTROL_DEFINITIONS: &[SourceControlDefinition] = &[
    SourceControlDefinition { kind: "github", label: "GitHub", terminology: PULL_REQUEST },
    SourceControlDefinition { kind: "gitlab", label: "GitLab", terminology: MERGE_REQUEST },
    SourceControlDefinition { kind: "bitbucket", label: "Bitbucket", terminology: PULL_REQUEST },
    SourceControlDefinition { kind: "azure-devops", label: "Azure DevOps", terminology: PULL_REQUEST },
    SourceControlDefinition { kind: "forgejo", label: "Forgejo", terminology: PULL_REQUEST },
    SourceControlDefinition { kind: "gitcafe", label: "GitCafe", terminology: PULL_REQUEST },
    SourceControlDefinition { kind: "cursor-origin", label: "Cursor Origin", terminology: PULL_REQUEST },
];

/// `UNKNOWN_SOURCE_CONTROL_CLIENT`.
pub const UNKNOWN_SOURCE_CONTROL: SourceControlDefinition =
    SourceControlDefinition { kind: "unknown", label: "source control", terminology: UNKNOWN };

/// `sourceControlClients.get(kind)`: the definition for a kind, or the unknown host.
pub fn source_control_definition(kind: Option<&str>) -> SourceControlDefinition {
    kind.and_then(|kind| SOURCE_CONTROL_DEFINITIONS.iter().find(|d| d.kind == kind).copied())
        .unwrap_or(UNKNOWN_SOURCE_CONTROL)
}

/// What the git controls show for a repository's host.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceControlPresentation {
    pub definition: SourceControlDefinition,
    pub provider_name: SharedString,
    pub terminology: ChangeRequestTerminology,
    /// Host artwork is not bundled; every host draws the pull request glyph.
    pub icon: IconName,
}

/// `getSourceControlPresentation`.
pub fn source_control_presentation(provider: Option<&SourceControlProviderInfo>) -> SourceControlPresentation {
    let definition = source_control_definition(provider.map(|p| p.kind.as_str()));
    let provider_name = provider
        .map(|p| p.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(definition.label);
    SourceControlPresentation {
        definition,
        provider_name: SharedString::from(provider_name.to_owned()),
        terminology: definition.terminology,
        icon: IconName::GitPullRequestArrow,
    }
}
