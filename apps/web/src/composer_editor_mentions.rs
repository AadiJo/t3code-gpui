//! Inline chips in composer text. Ports `composer-editor-mentions.ts` and the
//! token scanners it uses (`composerInlineTokens.ts`, `composerContextReferences.ts`,
//! `assistantCitations.ts`, `composerModelMentions.ts`), written as hand scanners
//! so a keystroke costs no regex work in a debug build.
//!
//! Scanning runs one source line at a time: a token must be followed by
//! whitespace, so the caller says whether a newline follows the line.
//! Math ranges are not excluded (see the final report).

use std::ops::Range;

use gpui_kit::SharedString;

/// The kind of a context reference chip, from its `t3-context://v1/<kind>/<id>` link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContextKind {
    Image,
    Text,
    File,
    Terminal,
    Thread,
    ReviewComment,
    PreviewAnnotation,
    Element,
    PullRequest,
    Other,
}

impl ContextKind {
    fn parse(kind: &str) -> Self {
        match kind {
            "image" => Self::Image,
            "text" | "pasted-text" => Self::Text,
            "file" => Self::File,
            "terminal" => Self::Terminal,
            "thread" => Self::Thread,
            "review-comment" => Self::ReviewComment,
            "preview-annotation" => Self::PreviewAnnotation,
            "element" => Self::Element,
            "pull-request" => Self::PullRequest,
            _ => Self::Other,
        }
    }
}

/// What a chip stands for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChipKind {
    /// `@path`, `@"quoted path"` or a `[basename](path)` file link.
    Mention { path: SharedString },
    /// `$skill`.
    Skill { name: SharedString },
    /// `model:<instance>/<slug>`.
    Model { instance_id: SharedString, model: SharedString },
    /// `[Assistant quote](t3-citation://v1/...)`.
    Citation,
    /// `[label](t3-context://v1/<kind>/<id>)`, `!`-prefixed for images.
    Context { kind: ContextKind, context_id: SharedString },
}

/// One chip in a line: its source byte range and the label the editor draws.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InlineToken {
    pub range: Range<usize>,
    pub kind: ChipKind,
    pub label: SharedString,
}

/// A run of prompt text or one chip. Ports `ComposerPromptSegment`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerPromptSegment<'a> {
    Text(&'a str),
    Chip(InlineToken),
}

const MAX_FILE_LINK_LABEL: usize = 512;
const MAX_CONTEXT_LABEL: usize = 512;
const CONTEXT_HREF_PREFIX: &str = "t3-context://v1/";
const CITATION_PREFIX: &str = "[Assistant quote](t3-citation://v1/";
const CITATION_LABEL_MAX: usize = 64;

fn is_ws(c: char) -> bool {
    c.is_whitespace()
}

/// Whether the byte at `end` is whitespace, counting a trailing newline after the line.
fn followed_by_ws(line: &str, end: usize, newline_after: bool) -> bool {
    match line[end..].chars().next() {
        Some(c) => is_ws(c),
        None => newline_after,
    }
}

fn preceded_by_ws_or_start(line: &str, start: usize) -> bool {
    line[..start].chars().next_back().is_none_or(is_ws)
}

fn basename(path: &str) -> &str {
    let sep = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    &path[sep..]
}

/// Removes backslash escapes, as `label.replace(/\\(.)/g, "$1")`.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Percent-decodes a link destination; invalid sequences leave the input as is.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = |b: Option<&u8>| b.and_then(|b| (*b as char).to_digit(16));
            let (Some(h), Some(l)) = (hex(bytes.get(i + 1)), hex(bytes.get(i + 2))) else {
                return text.to_owned();
            };
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

fn has_external_scheme(path: &str) -> bool {
    let b = path.as_bytes();
    let windows_drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\');
    if windows_drive {
        return false;
    }
    let mut chars = path.char_indices();
    match chars.next() {
        Some((_, c)) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for (_, c) in chars {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) {
            return false;
        }
    }
    false
}

/// `[label](dest)` starting at `start` (which must be `[`). Returns (label range, dest range, end).
fn scan_link(line: &str, start: usize, max_label: usize, label_allows_brackets: bool) -> Option<(Range<usize>, Range<usize>, usize)> {
    let bytes = line.as_bytes();
    let mut i = start + 1;
    let mut units = 0;
    while i < bytes.len() {
        let c = line[i..].chars().next()?;
        if c == '\\' && label_allows_brackets {
            let next = line[i + 1..].chars().next()?;
            i += 1 + next.len_utf8();
        } else if c == ']' {
            break;
        } else if c == '[' && !label_allows_brackets {
            return None;
        } else {
            i += c.len_utf8();
        }
        units += 1;
        if units > max_label {
            return None;
        }
    }
    if bytes.get(i) != Some(&b']') || bytes.get(i + 1) != Some(&b'(') {
        return None;
    }
    let label = start + 1..i;
    let dest_start = i + 2;
    let mut j = dest_start;
    while j < bytes.len() {
        let c = line[j..].chars().next()?;
        if c == ')' || is_ws(c) {
            break;
        }
        j += c.len_utf8();
    }
    if j == dest_start || bytes.get(j) != Some(&b')') {
        return None;
    }
    Some((label, dest_start..j, j + 1))
}

fn file_link_tokens(line: &str, newline_after: bool, out: &mut Vec<InlineToken>) {
    for (start, _) in line.match_indices('[') {
        if !preceded_by_ws_or_start(line, start) {
            continue;
        }
        let Some((label, dest, end)) = scan_link(line, start, MAX_FILE_LINK_LABEL, true) else {
            continue;
        };
        if !followed_by_ws(line, end, newline_after) {
            continue;
        }
        let path = percent_decode(&line[dest]);
        let label = unescape(&line[label]);
        if path.is_empty() || has_external_scheme(&path) || label != basename(&path) {
            continue;
        }
        out.push(InlineToken {
            range: start..end,
            label: label.into(),
            kind: ChipKind::Mention { path: path.into() },
        });
    }
}

fn is_scoped_package_reference(path: &str) -> bool {
    let seg_ok = |s: &str| {
        let mut chars = s.chars();
        let Some(c) = chars.next() else { return false };
        (c.is_ascii_lowercase() || c.is_ascii_digit())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    };
    let mut parts = path.splitn(3, '/');
    let (Some(a), Some(b)) = (parts.next(), parts.next()) else {
        return false;
    };
    seg_ok(a) && seg_ok(b) && parts.next().is_none_or(|rest| !rest.is_empty())
}

fn is_decorator_call(path: &str) -> bool {
    let mut chars = path.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    for c in chars {
        if c == '(' {
            return true;
        }
        if !(c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\u{200c}' | '\u{200d}')) {
            return false;
        }
    }
    false
}

fn at_mention_tokens(line: &str, newline_after: bool, out: &mut Vec<InlineToken>) {
    for (start, _) in line.match_indices('@') {
        if !preceded_by_ws_or_start(line, start) {
            continue;
        }
        let rest = &line[start + 1..];
        let (path, end, quoted) = if let Some(body) = rest.strip_prefix('"') {
            let mut i = 0;
            let bytes = body.as_bytes();
            let mut closed = None;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' => i += 1 + body[i + 1..].chars().next().map_or(0, char::len_utf8),
                    b'"' => {
                        closed = Some(i);
                        break;
                    }
                    _ => i += body[i..].chars().next().map_or(1, char::len_utf8),
                }
            }
            let Some(close) = closed else { continue };
            (unescape(&body[..close]), start + 2 + close + 1, true)
        } else {
            let len = rest
                .char_indices()
                .find(|(_, c)| is_ws(*c) || *c == '@' || *c == '"')
                .map_or(rest.len(), |(i, _)| i);
            (rest[..len].to_owned(), start + 1 + len, false)
        };
        if path.is_empty() || !followed_by_ws(line, end, newline_after) {
            continue;
        }
        if !quoted && (is_scoped_package_reference(&path) || is_decorator_call(&path)) {
            continue;
        }
        out.push(InlineToken {
            range: start..end,
            label: basename(&path).to_owned().into(),
            kind: ChipKind::Mention { path: path.into() },
        });
    }
}

/// Currency symbols the skill trigger accepts (`\p{Sc}` in the TypeScript).
pub fn is_currency_symbol(c: char) -> bool {
    matches!(
        c,
        '$' | '¢' | '£' | '¤' | '¥' | '֏' | '؋' | '৲' | '৳' | '฿' | '₠'..='₿' | '﷼' | '﹩' | '＄' | '￠' | '￡' | '￥' | '￦'
    )
}

/// `$5`, `$10k`, `$1e6`: money, not a skill.
fn is_money_amount(after: &str) -> bool {
    let mut chars = after.char_indices().peekable();
    match chars.next() {
        Some((_, c)) if c.is_ascii_digit() => {}
        _ => return false,
    }
    let mut end = after.len();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_ascii_digit() || c == '_' {
            chars.next();
        } else {
            end = i;
            break;
        }
    }
    let tail = &after[end..];
    let tail = if let Some(t) = tail.strip_prefix(|c: char| "kKmMbBtT".contains(c)) {
        t
    } else if let Some(t) = tail.strip_prefix(['e', 'E']) {
        let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 { tail } else { &t[digits..] }
    } else {
        tail
    };
    tail.chars().next().is_none_or(is_ws)
}

fn skill_tokens(line: &str, newline_after: bool, out: &mut Vec<InlineToken>) {
    for (start, sym) in line.char_indices().filter(|(_, c)| is_currency_symbol(*c)) {
        if !preceded_by_ws_or_start(line, start) {
            continue;
        }
        let name_start = start + sym.len_utf8();
        let after = &line[name_start..];
        if is_money_amount(after) {
            continue;
        }
        let len = after
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-')))
            .map_or(after.len(), |(i, _)| i);
        let name = &after[..len];
        let starts_ok = name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
        if !starts_ok || !name.chars().any(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        let end = name_start + len;
        if !followed_by_ws(line, end, newline_after) {
            continue;
        }
        out.push(InlineToken {
            range: start..end,
            label: name.to_owned().into(),
            kind: ChipKind::Skill { name: name.to_owned().into() },
        });
    }
}

/// Strips sentence punctuation and unbalanced closing brackets from a model slug.
fn trim_model_slug(slug: &str) -> &str {
    let mut end = slug.len();
    loop {
        let Some(last) = slug[..end].chars().next_back() else { break };
        if ".;:!?".contains(last) {
            end -= 1;
            continue;
        }
        let open = match last {
            ')' => '(',
            ']' => '[',
            _ => break,
        };
        let body = &slug[..end];
        if body.matches(open).count() >= body.matches(last).count() {
            break;
        }
        end -= 1;
    }
    &slug[..end]
}

fn model_tokens(line: &str, newline_after: bool, out: &mut Vec<InlineToken>) {
    for (start, _) in line.match_indices("model:") {
        let ok_before = line[..start].chars().next_back().is_none_or(|c| is_ws(c) || ",([\"'".contains(c));
        if !ok_before {
            continue;
        }
        let rest = &line[start + 6..];
        let mut chars = rest.char_indices();
        if !chars.next().is_some_and(|(_, c)| c.is_ascii_alphabetic()) {
            continue;
        }
        let inst_len = rest
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
            .map_or(rest.len(), |(i, _)| i);
        if rest.as_bytes().get(inst_len) != Some(&b'/') {
            continue;
        }
        let slug_all = &rest[inst_len + 1..];
        let slug_len = slug_all
            .char_indices()
            .find(|(_, c)| is_ws(*c) || ",\"'".contains(*c))
            .map_or(slug_all.len(), |(i, _)| i);
        let slug = trim_model_slug(&slug_all[..slug_len]);
        if slug.is_empty() {
            continue;
        }
        let end = start + 6 + inst_len + 1 + slug.len();
        if !followed_by_ws(line, end, newline_after) {
            continue;
        }
        out.push(InlineToken {
            range: start..end,
            label: slug.to_owned().into(),
            kind: ChipKind::Model {
                instance_id: rest[..inst_len].to_owned().into(),
                model: slug.to_owned().into(),
            },
        });
    }
}

fn valid_context_kind(kind: &str) -> bool {
    let mut chars = kind.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && kind.len() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_context_id(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Collapses whitespace and brackets in a context label, as `sanitizeComposerContextLabel`.
pub fn sanitize_context_label(label: &str, kind: &str) -> String {
    let cleaned: String = label
        .split(|c: char| c.is_whitespace() || matches!(c, '[' | ']' | '\\'))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() { kind.to_owned() } else { cleaned }
}

fn context_tokens(line: &str, out: &mut Vec<InlineToken>) {
    if !line.contains("](t3-context:") {
        return;
    }
    for (bracket, _) in line.match_indices('[') {
        let Some((label, dest, end)) = scan_link(line, bracket, MAX_CONTEXT_LABEL, false) else {
            continue;
        };
        let href = &line[dest];
        let Some(rest) = href.strip_prefix(CONTEXT_HREF_PREFIX) else { continue };
        let mut parts = rest.split('/');
        let (Some(kind), Some(id), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if !valid_context_kind(kind) || !valid_context_id(id) {
            continue;
        }
        let image = bracket > 0 && line.as_bytes()[bracket - 1] == b'!';
        let start = if image { bracket - 1 } else { bracket };
        out.push(InlineToken {
            range: start..end,
            label: sanitize_context_label(&line[label], kind).into(),
            kind: ChipKind::Context { kind: ContextKind::parse(kind), context_id: id.to_owned().into() },
        });
    }
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
}

fn citation_tokens(line: &str, out: &mut Vec<InlineToken>) {
    for (start, _) in line.match_indices(CITATION_PREFIX) {
        let href_start = start + CITATION_PREFIX.len();
        let Some(len) = line[href_start..].find(|c: char| c == ')' || c.is_whitespace()) else {
            continue;
        };
        if line.as_bytes()[href_start + len] != b')' || len == 0 {
            continue;
        }
        let href = &line[href_start..href_start + len];
        let Some((_, query)) = href.split_once('?') else { continue };
        let decode = |v: &str| percent_decode(&v.replace('+', " "));
        let preview = query_param(query, "comment")
            .map(decode)
            .filter(|c| !c.trim().is_empty())
            .or_else(|| query_param(query, "text").map(decode))
            .unwrap_or_default();
        let preview = preview.split_whitespace().collect::<Vec<_>>().join(" ");
        let label = if preview.chars().count() <= CITATION_LABEL_MAX {
            preview
        } else {
            let cut: String = preview.chars().take(CITATION_LABEL_MAX).collect();
            format!("{cut}…")
        };
        out.push(InlineToken { range: start..href_start + len + 1, label: label.into(), kind: ChipKind::Citation });
    }
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && a.end > b.start
}

/// Every chip in one source line, sorted and non-overlapping. Ports
/// `collectComposerPromptInlineTokens` plus the overlap rule of
/// `splitPromptTextIntoComposerSegments`.
pub fn collect_line_tokens(line: &str, newline_after: bool) -> Vec<InlineToken> {
    let mut tokens = Vec::new();
    if !line.contains(['[', '@', 'm']) && !line.chars().any(is_currency_symbol) {
        return tokens;
    }
    file_link_tokens(line, newline_after, &mut tokens);
    at_mention_tokens(line, newline_after, &mut tokens);
    skill_tokens(line, newline_after, &mut tokens);
    let mut models = Vec::new();
    model_tokens(line, newline_after, &mut models);
    models.retain(|m| !tokens.iter().any(|t| overlaps(&t.range, &m.range)));
    tokens.extend(models);
    let mut links = Vec::new();
    citation_tokens(line, &mut links);
    context_tokens(line, &mut links);
    if !links.is_empty() {
        tokens.retain(|t| !links.iter().any(|l| overlaps(&t.range, &l.range)));
        tokens.extend(links);
    }
    tokens.sort_by_key(|t| t.range.start);
    let mut cursor = 0;
    tokens.retain(|t| {
        let keep = t.range.start >= cursor;
        if keep {
            cursor = t.range.end;
        }
        keep
    });
    tokens
}

/// Splits a whole prompt into text runs and chips. Ports `splitPromptIntoComposerSegments`.
pub fn split_prompt_into_segments(prompt: &str) -> Vec<ComposerPromptSegment<'_>> {
    let mut segments = Vec::new();
    let mut line_start = 0;
    let mut text_start = 0;
    for line in prompt.split_inclusive('\n') {
        let (body, newline_after) = match line.strip_suffix('\n') {
            Some(body) => (body, true),
            None => (line, false),
        };
        for mut token in collect_line_tokens(body, newline_after) {
            token.range = token.range.start + line_start..token.range.end + line_start;
            if token.range.start > text_start {
                segments.push(ComposerPromptSegment::Text(&prompt[text_start..token.range.start]));
            }
            text_start = token.range.end;
            segments.push(ComposerPromptSegment::Chip(token));
        }
        line_start += line.len();
    }
    if text_start < prompt.len() {
        segments.push(ComposerPromptSegment::Text(&prompt[text_start..]));
    }
    segments
}

/// Whether a selection covers the whitespace right next to an `@` mention.
/// Ports `selectionTouchesMentionBoundary`.
pub fn selection_touches_mention_boundary(prompt: &str, selection: Range<usize>) -> bool {
    if prompt.is_empty() || selection.start >= selection.end {
        return false;
    }
    let ws_at = |i: usize| prompt[i..].chars().next().is_some_and(is_ws);
    split_prompt_into_segments(prompt).iter().any(|segment| {
        let ComposerPromptSegment::Chip(InlineToken { range, kind: ChipKind::Mention { .. }, .. }) = segment else {
            return false;
        };
        let before = range.start.checked_sub(1).filter(|&i| prompt.is_char_boundary(i) && ws_at(i));
        let after = Some(range.end).filter(|&i| i < prompt.len() && ws_at(i));
        [before, after].into_iter().flatten().any(|i| selection.contains(&i))
    })
}

/// Formats a file reference as the composer stores it. Ports `serializeComposerFileLink`.
pub fn serialize_file_link(path: &str) -> String {
    let mut label = String::new();
    for c in basename(path).chars() {
        if matches!(c, '\\' | '[' | ']' | '*' | '_') {
            label.push('\\');
        }
        label.push(c);
    }
    let mut dest = String::new();
    for b in path.bytes() {
        let keep = b.is_ascii_alphanumeric() || b"-_.!~*';:@&=+$,/".contains(&b);
        if keep {
            dest.push(b as char);
        } else {
            dest.push_str(&format!("%{b:02X}"));
        }
    }
    format!("[{label}]({dest})")
}

/// Formats a context reference chip. Ports `formatComposerContextReference`.
pub fn format_context_reference(kind: &str, context_id: &str, label: &str) -> String {
    let label = sanitize_context_label(label, kind);
    let bang = if kind == "image" { "!" } else { "" };
    format!("{bang}[{label}]({CONTEXT_HREF_PREFIX}{kind}/{context_id})")
}

/// Whether a prompt holds anything besides context chips and whitespace
/// (`stripInlineContextReferences(prompt).trim().length > 0`).
pub fn has_text_besides_context(prompt: &str) -> bool {
    split_prompt_into_segments(prompt).iter().any(|segment| match segment {
        ComposerPromptSegment::Text(text) => !text.trim().is_empty(),
        ComposerPromptSegment::Chip(token) => !matches!(token.kind, ChipKind::Context { .. }),
    })
}
