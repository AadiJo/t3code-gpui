//! How a source control host names and draws change requests. Ports
//! `sourceControlPresentation.tsx` and the `changeRequest` terminology of each
//! built-in definition in `packages/source-control-*/src/shared/definition.ts`.

use gpui_kit::SharedString;
use t3code_contracts::SourceControlProviderInfo;

use crate::components::icons::{BrandIcon, IconName, IconSource};

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
    /// The Publish repository card's name (`pickerLabel`).
    pub picker_label: &'static str,
    /// The repository field's placeholder (`repositoryPathHint`).
    pub path_hint: &'static str,
    /// The host the repository field shows before the path (`publishHost`);
    /// `None` means the signed-in server, "your server" before sign-in.
    pub publish_host: Option<&'static str>,
}

/// Cursor Origin's git host (`CURSOR_ORIGIN_GIT_HOST`).
const CURSOR_ORIGIN_GIT_HOST: &str = "origin.cursor.com";

/// The built-in hosts, in registry order. GitHub is first: a repository with
/// no recognised host reads as GitHub.
pub const SOURCE_CONTROL_DEFINITIONS: &[SourceControlDefinition] = &[
    SourceControlDefinition { kind: "github", label: "GitHub", terminology: PULL_REQUEST, picker_label: "GitHub", path_hint: "owner/repo", publish_host: Some("github.com") },
    SourceControlDefinition { kind: "cursor-origin", label: "Cursor Origin", terminology: PULL_REQUEST, picker_label: "Cursor Origin", path_hint: "owner/repo", publish_host: Some(CURSOR_ORIGIN_GIT_HOST) },
    SourceControlDefinition { kind: "gitlab", label: "GitLab", terminology: MERGE_REQUEST, picker_label: "GitLab", path_hint: "group/project", publish_host: Some("gitlab.com") },
    SourceControlDefinition { kind: "gitcafe", label: "GitCafe", terminology: PULL_REQUEST, picker_label: "GitCafe", path_hint: "owner/repo", publish_host: Some("git.cafe") },
    SourceControlDefinition { kind: "forgejo", label: "Forgejo", terminology: PULL_REQUEST, picker_label: "Forgejo / Gitea", path_hint: "owner/repo", publish_host: None },
    SourceControlDefinition { kind: "bitbucket", label: "Bitbucket", terminology: PULL_REQUEST, picker_label: "Bitbucket", path_hint: "workspace/repository", publish_host: Some("bitbucket.org") },
    SourceControlDefinition { kind: "azure-devops", label: "Azure DevOps", terminology: PULL_REQUEST, picker_label: "Azure DevOps", path_hint: "project/repository", publish_host: Some("dev.azure.com") },
];

/// `UNKNOWN_SOURCE_CONTROL_CLIENT`.
pub const UNKNOWN_SOURCE_CONTROL: SourceControlDefinition = SourceControlDefinition {
    kind: "unknown",
    label: "source control",
    terminology: UNKNOWN,
    picker_label: "source control",
    path_hint: "owner/repo",
    publish_host: None,
};

/// `sourceControlClients.get(kind)`: no kind is the first host; an unrecognised
/// kind is the unknown host.
pub fn source_control_definition(kind: Option<&str>) -> SourceControlDefinition {
    match kind {
        None => SOURCE_CONTROL_DEFINITIONS[0],
        Some(kind) => SOURCE_CONTROL_DEFINITIONS.iter().find(|d| d.kind == kind).copied().unwrap_or(UNKNOWN_SOURCE_CONTROL),
    }
}

/// What the git controls show for a repository's host.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceControlPresentation {
    pub definition: SourceControlDefinition,
    pub provider_name: SharedString,
    pub terminology: ChangeRequestTerminology,
    /// The host's mark (`sourceControlIcon`); a host without artwork draws the
    /// pull request glyph.
    pub icon: IconSource,
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
        icon: source_control_icon(definition),
    }
}

/// `sourceControlIcon`: each built-in host's mark from its definition's artwork.
pub fn source_control_icon(definition: SourceControlDefinition) -> IconSource {
    let brand = match definition.kind {
        "github" => BrandIcon::GitHub,
        "cursor-origin" => BrandIcon::CursorOrigin,
        "gitlab" => BrandIcon::GitLab,
        "gitcafe" => BrandIcon::GitCafe,
        "forgejo" => BrandIcon::Forgejo,
        "bitbucket" => BrandIcon::Bitbucket,
        "azure-devops" => BrandIcon::AzureDevOps,
        _ => return IconName::GitPullRequestArrow.into(),
    };
    brand.into()
}
