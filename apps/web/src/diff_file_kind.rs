//! Sorts a changed file by its path alone. Ports `packages/shared/src/changedFileKind.ts`:
//! lockfiles and build output count as generated, test support (mocks,
//! fixtures, snapshots) is kept apart from tests, and prose is its own kind.

use std::sync::LazyLock;

use regex::Regex;

/// The kind of a changed file. Matches `PullRequestChangeBreakdown` keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangedFileKind {
    Code,
    Tests,
    Mocks,
    Generated,
    Docs,
}

const GENERATED_FILES: &[&str] = &[
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "composer.lock",
    "deno.lock",
    "flake.lock",
    "gemfile.lock",
    "go.sum",
    "gradle.lockfile",
    "mix.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "packages.lock.json",
    "pipfile.lock",
    "pnpm-lock.yaml",
    "podfile.lock",
    "poetry.lock",
    "pubspec.lock",
    "uv.lock",
    "yarn.lock",
];
const GENERATED_DIRS: &[&str] = &["__generated__", "_generated", "generated"];
const MOCK_DIRS: &[&str] = &[
    "__fixtures__",
    "__mocks__",
    "__snapshots__",
    "fakes",
    "fixture",
    "fixtures",
    "mock",
    "mocks",
    "stubs",
    "testdata",
];
const TEST_DIRS: &[&str] = &["__tests__", "__test__", "cypress", "e2e", "evals", "spec", "specs", "test", "tests"];

struct Patterns {
    generated: Regex,
    mock: Regex,
    test: Regex,
    class_test: Regex,
    docs: Regex,
    not_docs: Regex,
}

static PATTERNS: LazyLock<Patterns> = LazyLock::new(|| Patterns {
    generated: Regex::new(
        r"(\.generated\.|\.gen\.|\.g\.dart$|\.pb\.go$|_pb2(_grpc)?\.py$|\.pb\.(cc|h)$|\.min\.(js|css)$|\.(js|css)\.map$)",
    )
    .unwrap(),
    mock: Regex::new(r"(\.snap$|\.(mocks?|fixtures?|stubs?|fakes?)\.[^.]+$|^(mocks?|fixtures?|stubs?|fakes?)([-_.A-Z]|$))")
        .unwrap(),
    test: Regex::new(
        r"(?i)(\.(test|spec|e2e|eval|stories)\.[^.]+$|_(test|spec)\.[^.]+$|^test_.+\.py$|^conftest\.py$|^(vitest|jest)\.setup\.[^.]+$)",
    )
    .unwrap(),
    // Case-sensitive: `FooTest.java` is a test class, `Latest.java` is not.
    class_test: Regex::new(r"(Tests?|Spec)\.(java|kt|kts|scala|groovy|cs|php|swift|m)$").unwrap(),
    docs: Regex::new(r"(?i)\.(md|mdx|markdown|rst|adoc|txt)$").unwrap(),
    not_docs: Regex::new(r"(?i)^(requirements.*|constraints.*|CMakeLists|robots|llms)\.txt$").unwrap(),
});

/// Classifies a changed file by naming conventions.
pub fn classify_changed_file(path: &str) -> ChangedFileKind {
    let normalized = path.replace('\\', "/");
    let mut segments: Vec<&str> = normalized.split('/').collect();
    let name = segments.pop().unwrap_or_default();
    let in_dir = |set: &[&str]| segments.iter().any(|dir| set.contains(&dir.to_lowercase().as_str()));
    let patterns = &*PATTERNS;
    if GENERATED_FILES.contains(&name.to_lowercase().as_str()) || patterns.generated.is_match(name) || in_dir(GENERATED_DIRS) {
        return ChangedFileKind::Generated;
    }
    if patterns.mock.is_match(name) || in_dir(MOCK_DIRS) {
        return ChangedFileKind::Mocks;
    }
    if patterns.test.is_match(name) || patterns.class_test.is_match(name) || in_dir(TEST_DIRS) {
        return ChangedFileKind::Tests;
    }
    if patterns.docs.is_match(name) && !patterns.not_docs.is_match(name) {
        return ChangedFileKind::Docs;
    }
    ChangedFileKind::Code
}

/// Tests and test support, which the panel's "hide test files" toggle hides.
pub fn is_test_changed_file(path: &str) -> bool {
    matches!(classify_changed_file(path), ChangedFileKind::Tests | ChangedFileKind::Mocks)
}
