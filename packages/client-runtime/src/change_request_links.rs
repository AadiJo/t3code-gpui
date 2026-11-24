//! Change request and issue links on every supported host: reading a pull
//! request or issue out of a URL and writing a project's pull request URL.
//! Ports the URL parts of the source control definitions (GitHub, GitLab,
//! GitCafe, Cursor Origin, Forgejo, Bitbucket, Azure DevOps), the registry's
//! claim order, and `changeRequestLinks.ts`.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

/// A source control host this client knows how to read links for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceControlHostKind {
    GitHub,
    CursorOrigin,
    GitLab,
    GitCafe,
    Forgejo,
    Bitbucket,
    AzureDevOps,
}

impl SourceControlHostKind {
    /// The wire `SourceControlProviderKind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::CursorOrigin => "cursor-origin",
            Self::GitLab => "gitlab",
            Self::GitCafe => "gitcafe",
            Self::Forgejo => "forgejo",
            Self::Bitbucket => "bitbucket",
            Self::AzureDevOps => "azure-devops",
        }
    }

    /// Reads a wire provider kind.
    pub fn from_wire(kind: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == kind)
    }

    /// The host's display name.
    pub fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::CursorOrigin => "Cursor Origin",
            Self::GitLab => "GitLab",
            Self::GitCafe => "GitCafe",
            Self::Forgejo => "Forgejo",
            Self::Bitbucket => "Bitbucket",
            Self::AzureDevOps => "Azure DevOps",
        }
    }

    /// Registry order: GitHub first keeps the client default for unknown hosts.
    pub const ALL: [Self; 7] =
        [Self::GitHub, Self::CursorOrigin, Self::GitLab, Self::GitCafe, Self::Forgejo, Self::Bitbucket, Self::AzureDevOps];

    /// The order hosts claim a URL in. GitCafe and Cursor Origin go first because GitHub and
    /// Forgejo accept the same routes on any hostname.
    pub const CLAIM_ORDER: [Self; 7] =
        [Self::GitCafe, Self::CursorOrigin, Self::AzureDevOps, Self::GitHub, Self::GitLab, Self::Forgejo, Self::Bitbucket];
}

/// A change request as a link names it. `host` and `repository` are lower case and match what a
/// project's repository identity holds; `authority` keeps the port for hosts whose identity has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRequestLink {
    pub host: String,
    pub repository: String,
    pub number: u64,
    pub authority: Option<String>,
    /// The repository's browser URL, without query or hash.
    pub repository_url: String,
}

/// An issue as a URL names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLink {
    pub host: String,
    pub scope: String,
    /// An equivalent binding scope when the provider allows another spelling in URLs.
    pub scope_alias: Option<String>,
    pub key: String,
}

macro_rules! re {
    ($name:ident, $pattern:literal) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).unwrap());
    };
}

re!(GITHUB_CR, r"^/([^/]+/[^/]+)/pull/(\d+)(?:/|$)");
re!(GITHUB_ISSUE, r"^/([^/]+/[^/]+)/issues/(\d+)(?:/|$)");
re!(GITLAB_CR, r"^/([^/]+(?:/[^/]+)+)/-/merge_requests/(\d+)(?:/|$)");
re!(GITLAB_ISSUE, r"^/([^/]+(?:/[^/]+)+)/-/(?:issues|work_items)/(\d+)(?:/|$)");
re!(GITCAFE_CR, r"^/([^/]+/[^/]+)/pulls?/(\d+)(?:/|$)");
re!(CURSOR_CR, r"^/codebase/([^/]+/[^/]+)/pull/(\d+)(?:/|$)");
re!(FORGEJO_CR, r"^/([^/]+(?:/[^/]+)+)/pulls/(\d+)(?:/|$)");
re!(FORGEJO_ISSUE, r"^/([^/]+(?:/[^/]+)+)/issues/(\d+)(?:/|$)");
re!(BITBUCKET_CR, r"^/([^/]+/[^/]+)/pull-requests/(\d+)(?:/|$)");
re!(AZURE_CR, r"^/((?:[^/]+/)*_git/[^/]+)/pullrequest/(\d+)(?:/|$)");
re!(AZURE_WORK_ITEM, r"^/([^/]+/[^/]+)/_workitems/edit/(\d+)(?:/|$)");
re!(AZURE_LEGACY_WORK_ITEM, r"^/([^/]+)/_workitems/edit/(\d+)(?:/|$)");
re!(AZURE_DEFAULT_COLLECTION, r"^defaultcollection/([^/]+)$");

const CURSOR_ORIGIN_GIT_HOST: &str = "origin.cursor.com";
const CURSOR_ORIGIN_WEB_BASE_URL: &str = "https://cursor.com/codebase";

fn is_github_host(hostname: &str) -> bool {
    let host = hostname.to_lowercase();
    host == "github.com" || host.ends_with(".ghe.com") || host.split('.').any(|p| p == "github")
}

fn is_forgejo_host(hostname: &str) -> bool {
    let host = hostname.to_lowercase();
    host == "codeberg.org" || host.split('.').any(|p| p == "forgejo" || p == "gitea")
}

fn is_bitbucket_host(hostname: &str) -> bool {
    let host = hostname.to_lowercase();
    host == "bitbucket.org" || host.ends_with(".bitbucket.org") || host.split('.').any(|p| p == "bitbucket")
}

fn is_gitcafe_host(hostname: &str) -> bool {
    matches!(hostname.to_lowercase().as_str(), "git.cafe" | "staging.git.cafe")
}

/// `decodeURIComponent`, or `None` where it throws.
pub fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `encodeURIComponent`.
pub fn encode_uri_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn canonical_path_segment(segment: &str) -> String {
    // Lowercase before encoding: lowercasing the escape `%C3%84` never reaches the `Ä` it names.
    match decode_uri_component(segment) {
        Some(decoded) => encode_uri_component(&decoded.to_lowercase()).to_lowercase(),
        None => segment.to_lowercase(),
    }
}

/// A repository path in one lower-case spelling.
pub fn canonical_repository_path(path: &str) -> String {
    path.split('/').map(canonical_path_segment).collect::<Vec<_>>().join("/")
}

fn http_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// `url.host` in JavaScript: the hostname with a non-default port.
fn js_host(url: &Url) -> String {
    let hostname = url.host_str().unwrap_or("");
    match url.port() {
        Some(port) => format!("{hostname}:{port}"),
        None => hostname.to_owned(),
    }
}

fn js_origin(url: &Url) -> String {
    format!("{}://{}", url.scheme(), js_host(url))
}

fn safe_number(raw: &str) -> Option<u64> {
    let n: f64 = raw.parse().ok()?;
    (n.fract() == 0.0 && n > 0.0 && n <= 9_007_199_254_740_991.0).then_some(n as u64)
}

fn parse_change_request_path(url: &Url, route: &Regex) -> Option<ChangeRequestLink> {
    let captures = route.captures(url.path())?;
    let repository = captures.get(1)?.as_str();
    let number = safe_number(captures.get(2)?.as_str())?;
    (!repository.is_empty()).then(|| ChangeRequestLink {
        host: url.host_str().unwrap_or("").to_lowercase(),
        repository: canonical_repository_path(repository),
        number,
        authority: None,
        repository_url: format!("{}/{repository}", js_origin(url)),
    })
}

fn parse_issue_path(url: &Url, route: &Regex) -> Option<IssueLink> {
    let captures = route.captures(url.path())?;
    let scope = captures.get(1)?.as_str();
    let number = safe_number(captures.get(2)?.as_str())?;
    (!scope.is_empty()).then(|| IssueLink {
        host: url.host_str().unwrap_or("").to_lowercase(),
        scope: scope.to_lowercase(),
        scope_alias: None,
        key: number.to_string(),
    })
}

fn host_parse_change_request(kind: SourceControlHostKind, url: &Url) -> Option<ChangeRequestLink> {
    use SourceControlHostKind as K;
    let hostname = url.host_str().unwrap_or("");
    match kind {
        K::GitHub => parse_change_request_path(url, &GITHUB_CR),
        K::GitLab => parse_change_request_path(url, &GITLAB_CR),
        K::GitCafe => is_gitcafe_host(hostname).then(|| parse_change_request_path(url, &GITCAFE_CR)).flatten(),
        K::CursorOrigin => {
            let host = hostname.to_lowercase();
            if host != "cursor.com" && host != "www.cursor.com" {
                return None;
            }
            let captures = CURSOR_CR.captures(url.path())?;
            let repository = captures.get(1)?.as_str();
            let number = safe_number(captures.get(2)?.as_str())?;
            Some(ChangeRequestLink {
                host: CURSOR_ORIGIN_GIT_HOST.into(),
                repository: repository.to_lowercase(),
                number,
                authority: None,
                repository_url: format!("{CURSOR_ORIGIN_WEB_BASE_URL}/{repository}"),
            })
        }
        // A server's identity includes its port, so the link carries it.
        K::Forgejo => parse_change_request_path(url, &FORGEJO_CR)
            .map(|link| ChangeRequestLink { authority: Some(js_host(url).to_lowercase()), ..link }),
        K::Bitbucket => is_bitbucket_host(hostname).then(|| parse_change_request_path(url, &BITBUCKET_CR)).flatten(),
        // Servers on different ports are different servers, so a port stays in the link.
        K::AzureDevOps => parse_change_request_path(url, &AZURE_CR).map(|link| match url.port() {
            None => link,
            Some(_) => ChangeRequestLink { authority: Some(js_host(url).to_lowercase()), ..link },
        }),
    }
}

fn host_parse_issue(kind: SourceControlHostKind, url: &Url, known: bool) -> Option<IssueLink> {
    use SourceControlHostKind as K;
    let hostname = url.host_str().unwrap_or("");
    match kind {
        // `/issues/` is too generic a path to believe from a host that is not GitHub's, unless a
        // known project's host vouches for it as a GitHub Enterprise Server.
        K::GitHub => (known || is_github_host(hostname)).then(|| parse_issue_path(url, &GITHUB_ISSUE)).flatten(),
        K::GitLab => parse_issue_path(url, &GITLAB_ISSUE),
        K::Forgejo => is_forgejo_host(hostname).then(|| parse_issue_path(url, &FORGEJO_ISSUE)).flatten(),
        K::AzureDevOps => {
            let legacy = hostname.to_lowercase().ends_with(".visualstudio.com");
            let link = parse_issue_path(url, &AZURE_WORK_ITEM)
                .or_else(|| legacy.then(|| parse_issue_path(url, &AZURE_LEGACY_WORK_ITEM)).flatten())?;
            // A project named with spaces arrives percent-encoded; the binding it is matched against is not.
            let scope = link
                .scope
                .split('/')
                .map(|s| decode_uri_component(s).unwrap_or_else(|| s.to_owned()))
                .collect::<Vec<_>>()
                .join("/")
                .to_lowercase();
            if !legacy {
                return Some(IssueLink { scope, ..link });
            }
            // On legacy hosts the default collection is optional in a work item URL.
            let scope_alias = AZURE_DEFAULT_COLLECTION
                .captures(&scope)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_owned())
                .or_else(|| (!scope.contains('/')).then(|| format!("defaultcollection/{scope}")));
            Some(IssueLink { scope, scope_alias, ..link })
        }
        K::GitCafe | K::CursorOrigin | K::Bitbucket => None,
    }
}

/// The host that claims `url` as a change request, with the link it reads.
pub fn parse_change_request_url(value: &str) -> Option<(SourceControlHostKind, ChangeRequestLink)> {
    let url = http_url(value)?;
    SourceControlHostKind::CLAIM_ORDER.iter().find_map(|&kind| host_parse_change_request(kind, &url).map(|l| (kind, l)))
}

/// The issue `value` names. With `kind`, only that host is asked and it trusts the URL's host.
pub fn parse_issue_url(value: &str, kind: Option<SourceControlHostKind>) -> Option<(SourceControlHostKind, IssueLink)> {
    let url = http_url(value)?;
    match kind {
        Some(kind) => host_parse_issue(kind, &url, true).map(|l| (kind, l)),
        None => SourceControlHostKind::CLAIM_ORDER.iter().find_map(|&k| host_parse_issue(k, &url, false).map(|l| (k, l))),
    }
}

/// Whether two URLs name the same change request.
pub fn is_same_change_request(left: &str, right: &str) -> bool {
    match (parse_change_request_url(left), parse_change_request_url(right)) {
        (Some((_, a)), Some((_, b))) => {
            a.host == b.host && a.repository == b.repository && a.number == b.number && a.authority == b.authority
        }
        _ => false,
    }
}

/// `changeRequestUrlBelow`: a route appended to a repository's web URL.
fn url_below(web_url: &str, route: &str) -> String {
    format!("{}/{route}", web_url.trim_end_matches('/'))
}

/// An HTTP remote on `host` names the server's web origin, port included; SSH remotes do not.
fn remote_web_origin(host: &str, remote_url: Option<&str>) -> String {
    if let Some(remote) = remote_url.and_then(|r| Url::parse(r).ok())
        && matches!(remote.scheme(), "http" | "https")
    {
        let hostname = remote.host_str().unwrap_or("").to_lowercase();
        if hostname == host.to_lowercase() || js_host(&remote).to_lowercase() == host.to_lowercase() {
            return js_origin(&remote);
        }
    }
    format!("https://{host}")
}

static AZURE_SSH_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:ssh\.dev\.azure\.com|vs-ssh\.visualstudio\.com)/v3/([^/]+)/([^/]+)/([^/]+)$").unwrap());
static AZURE_LEGACY_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^.]+)\.visualstudio\.com/(?:defaultcollection/)?([^/]+)/_git/([^/]+)$").unwrap());

/// Every spelling of an Azure DevOps Services repository, on `dev.azure.com`.
fn azure_canonical_repository_key(key: &str) -> Option<String> {
    let canonical = AZURE_SSH_KEY.replace(key, "dev.azure.com/$1/$2/_git/$3");
    let canonical = AZURE_LEGACY_KEY.replace(&canonical, "dev.azure.com/$1/$2/_git/$3");
    canonical.starts_with("dev.azure.com/").then(|| canonical.into_owned())
}

/// What a host needs to write a change request's browser URL.
#[derive(Debug, Clone, Copy)]
pub struct ChangeRequestUrlInput<'a> {
    pub host: &'a str,
    pub repository: &'a str,
    pub number: u64,
    pub remote_url: Option<&'a str>,
    /// The repository's browser URL; when given, the result is this plus the host's route.
    pub web_url: Option<&'a str>,
}

/// A change request's browser URL on `kind`, or `None` when the host cannot write one.
pub fn change_request_url(kind: SourceControlHostKind, input: ChangeRequestUrlInput<'_>) -> Option<String> {
    use SourceControlHostKind as K;
    let ChangeRequestUrlInput { host, repository, number, remote_url, web_url } = input;
    let web_url = web_url.filter(|w| !w.is_empty());
    Some(match kind {
        K::GitHub => {
            if number < 1 {
                return None;
            }
            if let Some(web) = web_url {
                return Some(url_below(web, &format!("pull/{number}")));
            }
            let segments: Vec<&str> = repository.split('/').collect();
            let valid = |s: &&str| {
                !s.is_empty() && *s != "." && *s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            };
            if segments.len() != 2 || !segments.iter().all(valid) {
                return None;
            }
            let base = Url::parse(&remote_web_origin(host, remote_url)).ok()?;
            base.join(&format!("/{repository}/pull/{number}")).ok()?.to_string()
        }
        K::GitLab => match web_url {
            Some(web) => url_below(web, &format!("-/merge_requests/{number}")),
            None => format!("{}/{repository}/-/merge_requests/{number}", remote_web_origin(host, remote_url)),
        },
        K::GitCafe => match web_url {
            Some(web) => url_below(web, &format!("pulls/{number}")),
            None => format!("https://{host}/{repository}/pulls/{number}"),
        },
        K::CursorOrigin => format!("{CURSOR_ORIGIN_WEB_BASE_URL}/{repository}/pull/{number}"),
        K::Forgejo => match web_url {
            Some(web) => url_below(web, &format!("pulls/{number}")),
            None => format!("{}/{repository}/pulls/{number}", remote_web_origin(host, remote_url)),
        },
        K::Bitbucket => match web_url {
            Some(web) => url_below(web, &format!("pull-requests/{number}")),
            None => format!("https://{host}/{repository}/pull-requests/{number}"),
        },
        K::AzureDevOps => {
            if let Some(web) = web_url {
                return Some(url_below(web, &format!("pullrequest/{number}")));
            }
            let key = format!("{host}/{repository}").to_lowercase();
            let canonical = azure_canonical_repository_key(&key).unwrap_or(key);
            let mut parts = canonical.split('/');
            let canonical_host = parts.next().filter(|h| !h.is_empty()).unwrap_or(host);
            let path = parts.collect::<Vec<_>>().join("/");
            format!("{}/{path}/pullrequest/{number}", remote_web_origin(canonical_host, remote_url))
        }
    })
}

/// `canonicalRepositoryKey`: one spelling of a `host/path` repository key. Paths take their
/// canonical form; Azure DevOps Services repositories, written with an SSH v3 host or a legacy
/// `<organization>.visualstudio.com` host, become `dev.azure.com/<org>/<project>/_git/<repo>`.
pub fn canonical_repository_key(key: &str) -> String {
    let key = match key.split_once('/') {
        Some((host, path)) => format!("{host}/{}", canonical_repository_path(path)),
        None => return key.to_owned(),
    };
    let parts: Vec<&str> = key.split('/').collect();
    let azure = match parts.as_slice() {
        [host, "v3", organization, project, repository] if matches!(*host, "ssh.dev.azure.com" | "vs-ssh.visualstudio.com") => {
            Some(format!("dev.azure.com/{organization}/{project}/_git/{repository}"))
        }
        [host, rest @ ..] if host.ends_with(".visualstudio.com") && !host.trim_end_matches(".visualstudio.com").contains('.') => {
            let organization = host.trim_end_matches(".visualstudio.com");
            let rest = match rest {
                ["defaultcollection", rest @ ..] => rest,
                rest => rest,
            };
            match rest {
                [project, "_git", repository] if !organization.is_empty() => Some(format!("dev.azure.com/{organization}/{project}/_git/{repository}")),
                _ => None,
            }
        }
        _ => None,
    };
    azure.unwrap_or(key)
}
