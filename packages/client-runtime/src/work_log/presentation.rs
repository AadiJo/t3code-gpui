//! Labels and failure checks for work rows (`presentation.ts`, `toolOutput.ts`,
//! `usageFormat.ts`). Labels are English source strings; the UI translates them.

use t3code_contracts::{OrchestrationV2TurnItemCompaction, OrchestrationV2TurnItemStatus as Status};

/// Cheap pre-filter: every failure phrase below contains one of these.
fn has_failure_hint(lower: &str) -> bool {
    ["enoent", "no such", "no files found", "not found", "commandnotfound", "not recognized", "cannot find path", "cannot be found", "exit"]
        .iter()
        .any(|hint| lower.contains(hint))
}

fn nonzero_number_after(text: &str, prefix_end: usize) -> bool {
    let digits: String = text[prefix_end..].chars().take_while(char::is_ascii_digit).collect();
    !digits.is_empty() && !digits.starts_with('0')
}

/// `exit code` followed by optional `:`/whitespace then a number from 1 up, or
/// `exit(ed) with exit code <n>`.
fn mentions_nonzero_exit_code(lower: &str) -> bool {
    lower.match_indices("exit code").any(|(index, needle)| {
        let rest = &lower[index + needle.len()..];
        let skipped = rest.len() - rest.trim_start_matches([':', ' ', '\t', '\n', '\r']).len();
        let separator = &rest[..skipped];
        // `exit code\s*[:\s]\s*N` needs at least one separator; `exit code\s+N` after "with" too.
        !separator.is_empty() && nonzero_number_after(rest, skipped) && {
            let after = &rest[skipped..];
            let digits = after.chars().take_while(char::is_ascii_digit).count();
            after[digits..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_')
                || lower[..index].trim_end().ends_with("with")
        }
    })
}

/// Whether tool output describes a failure even though the provider reported success
/// (`toolOutputIndicatesFailure`).
pub fn tool_output_indicates_failure(text: &str) -> bool {
    let lower = text.to_lowercase();
    if !has_failure_hint(&lower) {
        return false;
    }
    [
        "file not found",
        "no files found",
        "enoent",
        "no such file",
        "commandnotfoundexception",
        "command not found",
        "is not recognized as the name of a cmdlet",
        "a parameter cannot be found that matches parameter name",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || (lower.contains("cannot find path") && lower.contains("because it does not exist"))
        || (lower.contains("is not recognized") && lower.contains("the term '"))
        || mentions_nonzero_exit_code(&lower)
}

/// Whether a command failed; a reported exit code outranks its output (`commandIndicatesFailure`).
pub fn command_indicates_failure(exit_code: Option<i64>, output_indicates_failure: Option<bool>, output: Option<&str>) -> bool {
    if let Some(code) = exit_code {
        return code != 0;
    }
    output_indicates_failure == Some(true)
        || output.is_some_and(|output| tool_output_indicates_failure(&output[..floor_char_boundary(output, 32_768)]))
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    (0..=index).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0)
}

/// `Number.prototype.toFixed`: an exact tie rounds away from zero (Rust's formatter rounds it
/// to even), so 1.125 reads 1.13 as in the web.
pub fn to_fixed(value: f64, digits: usize) -> String {
    let exact = format!("{:.60}", value.abs());
    let tie = exact.split_once('.').is_some_and(|(_, fraction)| {
        fraction.get(digits..digits + 1) == Some("5") && fraction[digits + 1..].bytes().all(|b| b == b'0')
    });
    if !tie {
        return format!("{value:.digits$}");
    }
    let scale = 10f64.powi(digits as i32);
    let rounded = ((value.abs() * scale).floor() + 1.0) / scale;
    format!("{}{rounded:.digits$}", if value < 0.0 { "-" } else { "" })
}

fn trim_number(value: f64) -> String {
    let abs = value.abs();
    let digits = if abs >= 100.0 { 0 } else if abs >= 10.0 { 1 } else { 2 };
    let text = to_fixed(value, digits);
    // Only an all-zero fraction goes (`/\.0+$/`): 1.90 stays 1.90, 2.00 reads 2.
    match text.split_once('.') {
        Some((whole, fraction)) if fraction.bytes().all(|b| b == b'0') => whole.to_owned(),
        _ => text,
    }
}

/// A token count with K/M/B suffixes (`formatTokens`).
pub fn format_tokens(value: u64) -> String {
    const SUFFIXES: [&str; 4] = ["K", "M", "B", "T"];
    let value = value as f64;
    if value.round().abs() < 1e3 {
        return format!("{}", value.round() as i64);
    }
    let mut scaled = value / 1e3;
    let mut unit = 0;
    while trim_number(scaled).parse::<f64>().unwrap_or(0.0).abs() >= 1e3 && unit < SUFFIXES.len() - 1 {
        scaled /= 1e3;
        unit += 1;
    }
    format!("{}{}", trim_number(scaled), SUFFIXES[unit])
}

/// A compaction row's label (`contextCompactionLabel`): English source text, with the token
/// counts as `(before, after)` for interpolation.
pub fn context_compaction_label(item: &OrchestrationV2TurnItemCompaction) -> (&'static str, Option<(String, String)>) {
    match item.status {
        Status::Failed => ("Context compaction failed", None),
        Status::Cancelled | Status::Interrupted => ("Context compaction stopped", None),
        Status::Pending | Status::Running | Status::Waiting => ("Compacting context", None),
        _ => match (item.before_token_count, item.after_token_count) {
            (Some(before), Some(after)) => {
                ("Context compacted {before} → {after} tokens", Some((format_tokens(before), format_tokens(after))))
            }
            _ => ("Context compacted", None),
        },
    }
}

/// The first sentence (or bold heading) of a reasoning text, as the live thinking row shows
/// it (`liveThoughtLine`).
pub fn live_thought_line(markdown: &str) -> String {
    let trimmed_start = markdown.trim_start_matches([' ', '\t', '\n', '\r']);
    let heading = trimmed_start.strip_prefix("**").and_then(|rest| {
        let end = rest.find("**")?;
        let inner = &rest[..end];
        let after = rest[end + 2..].trim_start_matches([' ', '\t']);
        let after = after.strip_prefix('\r').unwrap_or(after);
        (!inner.is_empty() && !inner.contains(['*', '\n', '\r']) && (after.is_empty() || after.starts_with('\n'))).then_some(inner)
    });
    let source = heading.unwrap_or(markdown);
    let mut text = String::with_capacity(source.len());
    for line in source.split('\n') {
        let stripped = line.trim_start_matches([' ', '\t']);
        let marker = stripped
            .strip_prefix(|c: char| c == '-' || c == '*' || c == '+')
            .filter(|rest| rest.starts_with([' ', '\t']))
            .or_else(|| {
                let hashes = stripped.chars().take_while(|c| *c == '#').count();
                (1..=6).contains(&hashes).then(|| &stripped[hashes..]).filter(|rest| rest.starts_with([' ', '\t']))
            })
            .or_else(|| {
                let digits = stripped.chars().take_while(char::is_ascii_digit).count();
                (digits > 0).then(|| &stripped[digits..]).and_then(|rest| rest.strip_prefix('.')).filter(|rest| rest.starts_with([' ', '\t']))
            });
        text.push_str(marker.map_or(line, |rest| rest.trim_start_matches([' ', '\t'])));
        text.push('\n');
    }
    let text = text.replace("**", "").replace("~~", "").replace('`', "");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if heading.is_some() {
        return text;
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        let closing = |j: usize, set: &[char]| chars.get(j).is_some_and(|(_, c)| set.contains(c));
        if matches!(c, '.' | '?' | '!') {
            let mut end = i + 1;
            if closing(end, &['"', '\'', '”', '’', ')']) {
                end += 1;
            }
            if chars.get(end).is_some_and(|(_, c)| c.is_whitespace()) {
                let stop = chars.get(end).map_or(text.len(), |(index, _)| *index);
                return text[..stop].to_owned();
            }
        } else if matches!(c, '。' | '！' | '？') {
            let mut end = i + 1;
            if closing(end, &['"', '\'', '”', '’', '）', '」', '』']) {
                end += 1;
            }
            let stop = chars.get(end).map_or(text.len(), |(index, _)| *index);
            let _ = at;
            return text[..stop].to_owned();
        }
    }
    text
}
