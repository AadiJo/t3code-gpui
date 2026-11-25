//! Prompt links (`shared/promptLinks.ts`): links that open a new thread with a prompt, and
//! optionally a repository, provider and model, or that open a pull request's thread.

/// `PROMPT_LINK_MAX_PROMPT_LENGTH`, in UTF-16 units as JavaScript counts them.
pub const PROMPT_LINK_MAX_PROMPT_LENGTH: usize = 8_000;

/// `DEFAULT_HOSTED_APP_URL`: the hosted web app prompt links point at.
pub const DEFAULT_HOSTED_APP_URL: &str = "https://app.t3.codes";

/// A GitHub repository a link names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptLinkRepository {
    pub owner: String,
    pub name: String,
}

impl PromptLinkRepository {
    /// `owner/name`.
    pub fn path(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// A GitHub pull request a link names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptLinkPullRequest {
    pub repository: PromptLinkRepository,
    pub number: u64,
}

impl PromptLinkPullRequest {
    /// `promptLinkPullRequestUrl`.
    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}/pull/{}", self.repository.owner, self.repository.name, self.number)
    }
}

/// A link's prompt with hidden characters removed, and how many there were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedPrompt {
    pub text: String,
    pub hidden_characters: usize,
}

/// What a `/new` link asks for (`NewThreadLink`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewThreadLink {
    New { prompt: LinkedPrompt, repository: Option<PromptLinkRepository>, provider: Option<String>, model: Option<String> },
    PullRequest { pull_request: PromptLinkPullRequest, prompt: Option<LinkedPrompt> },
}

impl NewThreadLink {
    pub fn prompt(&self) -> Option<&LinkedPrompt> {
        match self {
            Self::New { prompt, .. } => Some(prompt),
            Self::PullRequest { prompt, .. } => prompt.as_ref(),
        }
    }

    /// The repository whose project the link opens in, if it names one.
    pub fn repository(&self) -> Option<&PromptLinkRepository> {
        match self {
            Self::New { repository, .. } => repository.as_ref(),
            Self::PullRequest { pull_request, .. } => Some(&pull_request.repository),
        }
    }
}

/// Why a link could not be read (`PromptLinkError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptLinkError {
    PromptTooLong,
    PromptOnlyHidden,
    UnreadablePullRequest,
    MissingPrompt,
    UnreadableRepository,
}

/// `HIDDEN_CHARACTERS`: characters that let a model read instructions the reviewer cannot see.
fn is_hidden(c: char) -> bool {
    matches!(c as u32,
        0x0000..=0x0008 | 0x000B | 0x000C | 0x000E..=0x001F | 0x007F..=0x009F | 0x00AD | 0x034F | 0x061C
        | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F | 0x200B | 0x200C | 0x200E | 0x200F
        | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0D | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFFB
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// `isEmojiPart`: what a zero-width joiner may sit between.
fn is_emoji_part(c: Option<&char>) -> bool {
    c.is_some_and(|&c| matches!(c as u32, 0x1F000..=0x1FAFF | 0x2190..=0x2BFF | 0xFE0F))
}

/// `sanitizeLinkedPrompt`: normalizes line breaks, removes hidden characters (keeping joiners
/// inside emoji) and trims.
pub fn sanitize_linked_prompt(raw: &str) -> LinkedPrompt {
    let normalized = raw.replace("\r\n", "\n").replace(['\r', '\u{2028}', '\u{2029}'], "\n");
    let mut hidden_characters = 0;
    let visible: Vec<char> = normalized
        .chars()
        .filter(|&c| {
            let hidden = is_hidden(c);
            hidden_characters += usize::from(hidden);
            !hidden
        })
        .collect();
    let mut text = String::with_capacity(normalized.len());
    for (index, &c) in visible.iter().enumerate() {
        if c == '\u{200D}' {
            let kept = index > 0 && is_emoji_part(visible.get(index - 1)) && is_emoji_part(visible.get(index + 1));
            if !kept {
                hidden_characters += 1;
                continue;
            }
        }
        text.push(c);
    }
    LinkedPrompt { text: js_trim(&text).to_owned(), hidden_characters }
}

/// `String.prototype.trim`: whitespace and line terminators, which include U+FEFF.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

/// `OWNER` and `NAME`, then `repositoryOf`.
fn repository_of(owner: &str, name: &str) -> Option<PromptLinkRepository> {
    let name = match name.len().checked_sub(4) {
        Some(cut) if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".git") => &name[..cut],
        _ => name,
    };
    let owner_ok = (1..=39).contains(&owner.len())
        && owner.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let name_ok = (1..=100).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    (owner_ok && name_ok && !name.bytes().all(|b| b == b'.')).then(|| PromptLinkRepository { owner: owner.to_owned(), name: name.to_owned() })
}

/// `https://github.com/` or `https://www.github.com/`, any case, with `http` too; the rest.
fn strip_github(value: &str) -> Option<&str> {
    ["https://github.com/", "http://github.com/", "https://www.github.com/", "http://www.github.com/"]
        .iter()
        .find(|prefix| value.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix)))
        .map(|prefix| &value[prefix.len()..])
}

/// `parsePromptLinkRepository`: `https://github.com/owner/name` or `owner/name`.
pub fn parse_prompt_link_repository(value: &str) -> Option<PromptLinkRepository> {
    let trimmed = js_trim(value);
    if let Some(rest) = strip_github(trimmed) {
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let (owner, name) = rest.split_once('/')?;
        let bad = |s: &str| s.is_empty() || s.contains(['/', '?', '#']);
        return if bad(owner) || bad(name) { None } else { repository_of(owner, name) };
    }
    let (owner, name) = trimmed.split_once('/')?;
    let bad = |s: &str| s.is_empty() || s.contains('/') || s.chars().any(char::is_whitespace);
    if bad(owner) || bad(name) { None } else { repository_of(owner, name) }
}

/// `parsePromptLinkPullRequest`: a GitHub pull request URL, `owner/name/123` or `owner/name#123`.
fn parse_prompt_link_pull_request(value: &str) -> Option<PromptLinkPullRequest> {
    let trimmed = js_trim(value);
    let (owner, name, number) = if let Some(rest) = strip_github(trimmed) {
        let mut parts = rest.splitn(4, '/');
        let owner = parts.next()?;
        let name = parts.next()?;
        if parts.next()?.to_ascii_lowercase() != "pull" {
            return None;
        }
        let tail = parts.next()?;
        // `(\d+)(?:[/?#].*)?$`.
        let digits = tail.find(['/', '?', '#']).map_or(tail, |i| &tail[..i]);
        let bad = |s: &str| s.is_empty() || s.contains(['?', '#']);
        if bad(owner) || bad(name) {
            return None;
        }
        (owner, name, digits)
    } else {
        let (owner, rest) = trimmed.split_once('/')?;
        let split = rest.rfind(['/', '#'])?;
        let (name, number) = (&rest[..split], &rest[split + 1..]);
        let bad = |s: &str| s.is_empty() || s.contains(['/', '#']) || s.chars().any(char::is_whitespace);
        if bad(owner) || bad(name) {
            return None;
        }
        (owner, name, number)
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // `Number.isSafeInteger(number) && number > 0`.
    let number: u64 = number.parse().ok().filter(|n| (1..=9_007_199_254_740_991).contains(n))?;
    Some(PromptLinkPullRequest { repository: repository_of(owner, name)?, number })
}

/// `URLSearchParams.get`: the first value of `key`.
fn param(search: &str, key: &str) -> Option<String> {
    let search = search.strip_prefix('?').unwrap_or(search);
    url::form_urlencoded::parse(search.as_bytes()).find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

/// `readPrompt`.
fn read_prompt(search: &str) -> Result<Option<LinkedPrompt>, PromptLinkError> {
    let Some(raw) = param(search, "prompt") else { return Ok(None) };
    if raw.encode_utf16().count() > PROMPT_LINK_MAX_PROMPT_LENGTH {
        return Err(PromptLinkError::PromptTooLong);
    }
    let prompt = sanitize_linked_prompt(&raw);
    if prompt.text.is_empty() && prompt.hidden_characters > 0 {
        return Err(PromptLinkError::PromptOnlyHidden);
    }
    Ok((!prompt.text.is_empty()).then_some(prompt))
}

/// `parseNewThreadLink`: reads a `/new` link's raw query.
pub fn parse_new_thread_link(search: &str) -> Result<NewThreadLink, PromptLinkError> {
    let prompt = read_prompt(search)?;
    if let Some(pr) = param(search, "pr") {
        let pull_request = parse_prompt_link_pull_request(&pr).ok_or(PromptLinkError::UnreadablePullRequest)?;
        return Ok(NewThreadLink::PullRequest { pull_request, prompt });
    }
    let prompt = prompt.ok_or(PromptLinkError::MissingPrompt)?;
    let repository = match param(search, "repo") {
        Some(repo) => Some(parse_prompt_link_repository(&repo).ok_or(PromptLinkError::UnreadableRepository)?),
        None => None,
    };
    let optional = |key: &str| param(search, key).map(|v| js_trim(&v).to_owned()).filter(|v| !v.is_empty());
    Ok(NewThreadLink::New { prompt, repository, provider: optional("provider").map(|p| p.to_lowercase()), model: optional("model") })
}

/// `buildNewThreadLink`: `<origin>/new?prompt&repo&provider&model`; the model only with a provider.
pub fn build_new_thread_link(prompt: &str, repository: Option<&PromptLinkRepository>, provider: Option<&str>, model: Option<&str>, origin: &str) -> String {
    let mut url = url::Url::parse(origin).and_then(|base| base.join("/new")).unwrap_or_else(|_| url::Url::parse("https://app.t3.codes/new").expect("static url"));
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("prompt", prompt);
        if let Some(repository) = repository {
            query.append_pair("repo", &repository.path());
        }
        if let Some(provider) = provider.filter(|p| !p.is_empty()) {
            query.append_pair("provider", provider);
            if let Some(model) = model.filter(|m| !m.is_empty()) {
                query.append_pair("model", model);
            }
        }
    }
    url.to_string()
}

/// `promptLinkAppPath`: the app path a prompt link opens, from an app-scheme link
/// (`t3code://new?…`, `t3code-<name>://threads/<env>/<thread>?…`) or a hosted web link
/// (`<hosted>/new?…`, `<hosted>/threads/<env>/<thread>`, `<hosted>/<env>/<thread>?prompt=…`).
pub fn prompt_link_app_path(value: &str, hosted_app_url: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    let search = url.query().map(|q| format!("?{q}")).unwrap_or_default();
    let thread = |path: &str| {
        let path = path.strip_prefix('/')?;
        let path = path.strip_suffix('/').unwrap_or(path);
        let (env, thread) = path.split_once('/')?;
        (!env.is_empty() && !thread.is_empty() && !thread.contains('/')).then(|| format!("/{env}/{thread}{search}"))
    };
    let scheme = url.scheme();
    let app_scheme = scheme == "t3code" || scheme.strip_prefix("t3code-").is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase()));
    if app_scheme {
        return match url.host_str() {
            Some("new") if url.path().is_empty() || url.path() == "/" => Some(format!("/new{search}")),
            Some("threads") => thread(url.path()),
            _ => None,
        };
    }
    let hosted = url::Url::parse(hosted_app_url).ok()?;
    if url.origin() != hosted.origin() {
        return None;
    }
    if url.path() == "/new" {
        return Some(format!("/new{search}"));
    }
    // The web app's own thread route is a link only with a prompt, so reloading a tab never asks.
    if let Some(rest) = url.path().strip_prefix("/threads/") {
        return thread(&format!("/{rest}"));
    }
    if url.query_pairs().any(|(k, _)| k == "prompt") { thread(url.path()) } else { None }
}
